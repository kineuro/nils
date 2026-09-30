// SPDX-License-Identifier: AGPL-3.0-only

//! One job model (Wave 4a §9.1). Every verb that runs longer than a second
//! is a row of the `job` table with a heartbeat and a progress record, and
//! this module is the one way such a row is claimed, beaten, finished,
//! cancelled, queued and listed. The digest and the classifier each had a
//! claim path of their own before this; a third would have been a third.
//!
//! The rules, which are Wave 1 §10's: a claim refuses while a job **of the
//! same kind** is fresh, takes over one that is stale or whose process on
//! this host is gone (a job whose own process still runs on this host is
//! never stale, however long it goes without a beat: [`runs_here`]), and records the new job as running with this
//! process's id and host. A stale job of any other kind is failed on the
//! way, since a stale row is a stale row. A cancel from outside sets the
//! state to `cancelling`; the running job sees it at its next heartbeat and
//! stops the way a signal would stop it, so what is written stays written,
//! and a job is resumable because nothing is in flight (principle 4).
//!
//! A kind whose jobs may run side by side, as pipeline runs over stacks no
//! other run holds do, claims with [`claim_beside`]: a fresh job of the
//! kind refuses it only where the verb says the two meet.
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

/// The variable a test sets to shorten [`FRESH_SECS`], so a job left
/// without a heartbeat is stale in seconds rather than a minute. Nothing
/// but a test sets it.
pub const FRESH_VAR: &str = "NILS_TEST_JOB_FRESH_SECS";

/// How long a heartbeat keeps a job fresh: [`FRESH_SECS`], or what a test
/// set in [`FRESH_VAR`].
pub fn fresh_secs() -> u64 {
    std::env::var(FRESH_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(FRESH_SECS)
}

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
    /// A fresh job of the same kind holds what this one would take, in
    /// the words of the verb that asked ([`claim_beside`]).
    Held {
        job_id: i64,
        why: String,
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
            Error::Held { why, .. } => f.write_str(why),
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

/// When a process of this host started, in seconds since the epoch, where
/// the system says: Linux's `/proc/<pid>/stat`, read against the boot time
/// in `/proc/stat` at the 100 ticks a second the kernel reports in. A
/// process that has ended and waits for its parent to reap it (a zombie)
/// runs no more, and has none.
pub fn process_started(pid: i64) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // the fields after the command's closing parenthesis: the state is the
    // third field of the line, the start time the twenty-second
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    if matches!(fields.next()?, "Z" | "X" | "x") {
        return None;
    }
    let ticks: u64 = fields.nth(18)?.parse().ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(boot + ticks / 100)
}

/// Whether a job's own process still runs on this host: its host is this
/// one, a process has its id, and that process started no later than the
/// job was last heard from, so it is the job's and not a later process
/// that took the id after the job's had ended. A job whose process runs
/// is never failed as stale, however long it goes without a heartbeat:
/// one step of it, such as a pipeline run's intake of thousands of
/// proposals, may take longer than [`FRESH_SECS`].
pub fn runs_here(host: Option<&str>, pid: Option<i64>, last: &str) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    if host != Some(hostname().as_str()) || process_alive(pid) != Some(true) {
        return false;
    }
    let Some(heard) = secs_of(last) else {
        return false;
    };
    // a start time and a stamp in whole seconds, and a tick's rounding
    process_started(pid).is_some_and(|started| started <= heard + 2)
}

/// Whether a job is running now: not over, and either heard from within
/// the freshness a claim allows, or its process on this host still there
/// ([`runs_here`]); a job of this host whose process is gone is not,
/// however fresh its last beat.
pub fn is_live(j: &Job) -> bool {
    if j.state.is_over() || j.state == State::Queued {
        return false;
    }
    let last = j
        .heartbeat_at
        .as_deref()
        .unwrap_or(j.started_at.as_str())
        .to_string();
    let here = j.host.as_deref() == Some(hostname().as_str());
    let gone = here && j.pid.is_some_and(|p| process_alive(p) == Some(false));
    if gone {
        return false;
    }
    let fresh = secs_of(&last).is_some_and(|s| now_secs().saturating_sub(s) < fresh_secs());
    fresh || runs_here(j.host.as_deref(), j.pid, &last)
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

/// A process's command line as the words after `nils` and its registry:
/// what a queued command line looks like, so that a job claimed at the
/// keyboard and one queued at a door read alike under `queued`.
pub fn words_of(own: &[String]) -> Vec<String> {
    let mut words: Vec<String> = own.to_vec();
    if words
        .first()
        .is_some_and(|b| b == "nils" || b.ends_with("/nils") || b.ends_with("\\nils.exe"))
    {
        words.remove(0);
    }
    if words.first().is_some_and(|w| w == "--registry") && words.len() >= 2 {
        words.drain(..2);
    } else if let Some(first) = words.first()
        && first.starts_with("--registry=")
    {
        words.remove(0);
    }
    words
}

/// Take the kind, failing what is stale and refusing what is fresh, and
/// record this run as a running job. Returns the job's id.
pub fn claim(store: &mut Store, claim: &Claim<'_>) -> Result<i64, Error> {
    claim_with(store, claim, None)
}

/// What decides whether a fresh job of the claim's kind holds the claim
/// off: given that job's id and args, `Some(why)` when it does.
pub type Beside<'a> =
    dyn FnMut(&mut Store, i64, &serde_json::Value) -> Result<Option<String>, Error> + 'a;

/// [`claim`] for a kind whose jobs may run side by side, as pipeline runs
/// over stacks no other run holds do: a fresh job of the kind refuses the claim only where
/// `holds` says it does, with its words as [`Error::Held`]; a fresh job
/// `holds` lets be is left running. What is stale is failed on the way as
/// ever. Two claims that must not both pass are the caller's to take one
/// at a time.
pub fn claim_beside(
    store: &mut Store,
    claim: &Claim<'_>,
    holds: &mut Beside<'_>,
) -> Result<i64, Error> {
    claim_with(store, claim, Some(holds))
}

fn claim_with(
    store: &mut Store,
    claim: &Claim<'_>,
    mut holds: Option<&mut Beside<'_>>,
) -> Result<i64, Error> {
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
        "SELECT id, kind, {}, {}, pid, host, {} FROM {} WHERE state IN ('running', 'cancelling')",
        stamp(store, "heartbeat_at"),
        stamp(store, "started_at"),
        stamp(store, "args"),
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
        let fresh = secs_of(&last).is_some_and(|s| now_secs().saturating_sub(s) < fresh_secs());
        // a job whose process runs on this host is not stale, however long
        // since its last beat
        let live = fresh || runs_here(Some(its_host), its_pid, &last);
        if live && !gone {
            if its_kind == claim.kind {
                let Some(holds) = holds.as_deref_mut() else {
                    return Err(Error::Busy {
                        kind: its_kind,
                        job_id: id,
                        since: last,
                    });
                };
                let its_args: serde_json::Value = j
                    .opt_text(6)?
                    .and_then(|a| serde_json::from_str(a).ok())
                    .unwrap_or(serde_json::Value::Null);
                if let Some(why) = holds(store, id, &its_args)? {
                    return Err(Error::Held { job_id: id, why });
                }
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
    // The process's own command line, binary and registry included, under
    // `argv`: what ran, and what a resume runs. The command line as words
    // after `nils` and its registry under `queued`: as the door queued it,
    // or as the keyboard claimed it, which is what a card reads and what a
    // cancel's grant is found by (lab 26, defect 19).
    let own: Vec<String> = std::env::args().collect();
    args["argv"] = own.clone().into();
    if let Some(id) = adopted {
        // Wave 4c §6.1: what the queue recorded beside the command line
        // (the principal, the roles, the projection flag, the command line
        // as queued) survives the adoption; the verb's own args are laid
        // over it.
        if let Some(queued) = show(store, id)?.and_then(|j| j.args.as_object().cloned()) {
            for (k, v) in queued {
                if args.get(&k).is_none() {
                    args[k] = v;
                }
            }
        }
        if !args["queued"].is_array() {
            args["queued"] = words_of(&own).into();
        }
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
    args["queued"] = words_of(&own).into();
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

/// Record what a job produced (Wave 4c §6.1): the handle, the hash, the
/// counts, never rows. Written by the verb on the row it adopted.
pub fn set_result(store: &mut Store, job_id: i64, result: &serde_json::Value) -> Result<(), Error> {
    store.update_by_id(
        table("job"),
        &[("result", Param::from(result.to_string()))],
        "id",
        job_id,
    )?;
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
    /// What the job produced, as the verb recorded it (Wave 4c §6.1).
    pub result: Option<serde_json::Value>,
}

impl Job {
    /// Who queued the job, if the queue recorded it.
    pub fn principal(&self) -> Option<&str> {
        self.args["principal"].as_str()
    }

    /// The command line the job was started with, if the row recorded one:
    /// the words as queued until a worker's verb adopts the row, then the
    /// process's own line, binary and registry included, which a resume runs.
    pub fn argv(&self) -> Option<Vec<String>> {
        self.args["argv"].as_array().map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
    }

    /// The command line as words after `nils` and its registry, as a door
    /// queued it or the keyboard claimed it, whoever ran it: what a card
    /// reads. A row from before this was recorded answers its `argv` with
    /// the binary and the registry taken off.
    pub fn queued(&self) -> Option<Vec<String>> {
        let words = |v: &serde_json::Value| {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<String>>()
            })
        };
        words(&self.args["queued"]).or_else(|| words(&self.args["argv"]).map(|a| words_of(&a)))
    }

    /// Record 26 §7: the command lines queued when this job ends done, the
    /// first of them next, each as a list of words.
    pub fn then(&self) -> Vec<Vec<String>> {
        self.args["then"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                c.as_array().map(|words| {
                    words
                        .iter()
                        .filter_map(|w| w.as_str().map(str::to_string))
                        .collect()
                })
            })
            .collect()
    }

    /// The jobs before and after this one in its chain, when it has one.
    pub fn chain(&self) -> serde_json::Value {
        serde_json::json!({
            "before": self.args["chain_before"].as_i64(),
            "after": self.args["chain_after"].as_i64(),
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
            "result": self.result,
            "then": self.then(),
            "chain": self.chain(),
        })
    }
}

/// Set one field of a job's args, keeping the rest: how a chain notes the
/// job after this one, and a result names why a chain stopped.
pub fn set_arg(
    store: &mut Store,
    job_id: i64,
    key: &str,
    value: serde_json::Value,
) -> Result<(), Error> {
    let Some(job) = show(store, job_id)? else {
        return Err(Error::Message(format!("no job {job_id}")));
    };
    let mut args = job.args;
    if !args.is_object() {
        args = serde_json::json!({});
    }
    args[key] = value;
    store.update_by_id(
        table("job"),
        &[("args", Param::from(args.to_string()))],
        "id",
        job_id,
    )?;
    Ok(())
}

fn select_columns(store: &Store) -> String {
    format!(
        "id, kind, name, state, pid, host, {}, {}, {}, {}, error, {}, {}",
        stamp(store, "started_at"),
        stamp(store, "heartbeat_at"),
        stamp(store, "finished_at"),
        stamp(store, "progress"),
        stamp(store, "args"),
        stamp(store, "result"),
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
        result: json(12)?,
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

/// The kind a queued command line is: the verb, and the act where the verb
/// has one. Record 26 §1: `place originals` is an `originals` job, which is
/// the kind it claims when it runs, so a queued row and a running one read
/// alike.
pub fn kind_of(argv: &[String]) -> &str {
    match (
        argv.first().map(String::as_str),
        argv.get(1).map(String::as_str),
    ) {
        (Some("place"), Some("originals")) => "originals",
        // record 43 S2: `run` is a `pipeline` job
        (Some("run"), _) => "pipeline",
        (Some(verb), _) => verb,
        (None, _) => "",
    }
}

/// Put a command line on the queue, to be run by a worker in its turn. The
/// kind is the verb, so that `nils jobs list` reads the same for a queued
/// digest and a running one.
pub fn enqueue(
    store: &mut Store,
    argv: &[String],
    name: Option<&str>,
    principal: Option<&str>,
) -> Result<i64, Error> {
    enqueue_with(store, argv, name, principal, serde_json::Value::Null)
}

/// `enqueue` with more recorded beside the principal: the roles the door
/// saw and whether it may project raw identifiers (Wave 4c §6.1), so the
/// worker runs the verb under the caller's reach and never its own.
pub fn enqueue_with(
    store: &mut Store,
    argv: &[String],
    name: Option<&str>,
    principal: Option<&str>,
    extra: serde_json::Value,
) -> Result<i64, Error> {
    if argv.is_empty() {
        return Err(Error::Message(
            "nothing to queue: the command line is empty".into(),
        ));
    }
    let now = now_iso();
    // Wave 4a §9.2: who asked, carried to the verb the worker runs; the
    // command line as queued stays under `queued` once the verb has run
    // and written its own line over `argv`.
    let mut args = serde_json::json!({ "argv": argv, "queued": argv, "principal": principal });
    if let Some(more) = extra.as_object() {
        for (k, v) in more {
            args[k] = v.clone();
        }
    }
    let rows = store.insert(
        &Insert::new(
            table("job"),
            &["kind", "name", "args", "state", "started_at"],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(kind_of(argv)),
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

/// Wave 4a §13.1: the job log keeps a year of finished jobs. Delete the
/// ones over, done or failed or cancelled, and say how many went. A
/// running or queued job is never pruned.
pub fn prune(store: &mut Store, keep_days: u32) -> Result<u64, Error> {
    let cutoff = now_secs().saturating_sub(u64::from(keep_days) * 86_400);
    let day = crate::day::Day::from_unix(cutoff as i64)
        .map(|d| d.to_string())
        .unwrap_or_else(|| "1970-01-01".to_string());
    let stamp = format!("{day}T00:00:00Z");
    let d = store.dialect();
    let sql = format!(
        "DELETE FROM {} WHERE state IN ('done', 'failed', 'cancelled') AND finished_at IS NOT NULL \
         AND finished_at < {}",
        store.qualified("job"),
        d.param(1, Type::Timestamp)
    );
    Ok(store.execute(&sql, &[Param::from(stamp.as_str())])?)
}

/// The oldest queued job, if any.
pub fn next_queued(store: &mut Store) -> Result<Option<Job>, Error> {
    next_queued_in(store, Lane::All)
}

/// Which queued jobs a worker takes (record 49 A1): pipeline runs have a
/// lane of their own, so a long run never holds up a digest, a classify
/// or a release, which the main lane runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Every queued job, oldest first: a worker started by hand.
    All,
    /// Every job but a pipeline run.
    Main,
    /// Pipeline runs only.
    Pipelines,
}

impl Lane {
    pub fn name(self) -> &'static str {
        match self {
            Lane::All => "all",
            Lane::Main => "main",
            Lane::Pipelines => "pipelines",
        }
    }

    pub fn parse(text: &str) -> Option<Lane> {
        match text.trim() {
            "all" => Some(Lane::All),
            "main" => Some(Lane::Main),
            "pipelines" => Some(Lane::Pipelines),
            _ => None,
        }
    }

    /// The kind of the worker's own row: one worker per lane at a time.
    pub fn worker_kind(self) -> &'static str {
        match self {
            Lane::All | Lane::Main => "worker",
            Lane::Pipelines => PIPELINE_WORKER,
        }
    }
}

/// The kind of the pipeline lane's worker row.
pub const PIPELINE_WORKER: &str = "pipeline-worker";

/// Whether a job's kind is a worker's own row, which the job lists leave
/// out unless asked for everything.
pub fn is_worker(kind: &str) -> bool {
    kind == "worker" || kind == PIPELINE_WORKER
}

/// The oldest queued job a lane takes, if any.
pub fn next_queued_in(store: &mut Store, lane: Lane) -> Result<Option<Job>, Error> {
    let filter = match lane {
        Lane::All => "",
        Lane::Main => " AND kind <> 'pipeline'",
        Lane::Pipelines => " AND kind = 'pipeline'",
    };
    let sql = format!(
        "SELECT {} FROM {} WHERE state = 'queued'{filter} ORDER BY id LIMIT 1",
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// A process that has ended and is not yet reaped is still there to a
    /// signal, but runs no more: it has no start time, so a job whose
    /// process it was is never held as running here by it.
    #[test]
    fn an_ended_process_waiting_to_be_reaped_does_not_run_here() {
        let now = now_iso();
        let own = i64::from(std::process::id());
        assert!(process_started(own).is_some());
        assert!(runs_here(Some(hostname().as_str()), Some(own), &now));

        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = i64::from(child.id());
        // until it is reaped, the ended child is a zombie
        let started = std::time::Instant::now();
        while std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| !s[s.rfind(')').unwrap() + 1..].trim_start().starts_with('Z'))
            .unwrap_or(true)
        {
            assert!(started.elapsed().as_secs() < 10, "the child never ended");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(process_alive(pid), Some(true));
        assert_eq!(process_started(pid), None);
        assert!(!runs_here(Some(hostname().as_str()), Some(pid), &now_iso()));
        child.wait().unwrap();
    }
}
