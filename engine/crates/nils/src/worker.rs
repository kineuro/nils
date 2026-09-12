// SPDX-License-Identifier: AGPL-3.0-only
//! The queue's worker (Wave 4c section 6.1): queued jobs run one at a time,
//! oldest first, each as a `nils` process of its own under the roles and the
//! actor the door recorded. `nils jobs work` runs it in the foreground, and
//! `nils serve --worker` runs it beside the doors, so a job queued at a door
//! runs without anyone starting a worker by hand.

use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use nils_registry::home::Home;
use nils_registry::job::{self, State};
use nils_registry::store::Store;

use crate::{Exit, fail};

/// How often a worker whose job is still running says it is alive; well
/// inside the freshness a claim allows, so a long digest never reads as a
/// worker that died.
const BEAT_EVERY: Duration = Duration::from_secs(10);

/// Take the queue: a registry's queue has one worker at a time.
pub(crate) fn claim(store: &mut Store, once: bool) -> Result<i64, job::Error> {
    job::claim(
        store,
        &job::Claim {
            kind: "worker",
            name: "queue",
            args: serde_json::json!({ "once": once }),
        },
    )
}

/// How a worker runs the queue.
pub(crate) struct Options<'a> {
    /// Stop once the queue is empty, rather than wait for more.
    pub(crate) once: bool,
    /// Seconds between looks at an empty queue.
    pub(crate) every: u64,
    /// The registered ingest locations a probe resolves, as `NAME=PATH`.
    pub(crate) ingest_roots: &'a [String],
    /// A worker beside the doors, whose output no one reads: a job's own
    /// output goes nowhere, since its row says how it ended, and the worker's
    /// lines go to stderr, written so that a closed stream never stops the
    /// queue.
    pub(crate) quiet: bool,
}

/// Run queued jobs until `stop` says so, or with `once` until the queue is
/// empty. Answers how many jobs ran; the worker's own row is finished however
/// the run ends.
pub(crate) fn run(
    home: &Home,
    store: &mut Store,
    worker: i64,
    opts: &Options<'_>,
    stop: &dyn Fn() -> bool,
) -> Result<usize, Exit> {
    let err = |e: job::Error| fail(e.to_string());
    // Wave 4c section 6.6: the worker's own registered locations, handed to
    // a probe by name; the queued command line carries no path.
    let roots_env = opts.ingest_roots.join(";");
    let me = std::env::current_exe().map_err(|e| fail(e.to_string()))?;
    let mut ran = 0usize;
    let outcome = loop {
        if stop() {
            break Ok(());
        }
        match job::beat(store, worker, Some(&serde_json::json!({ "ran": ran }))) {
            Ok(job::Asked::Cancel) => break Ok(()),
            Ok(_) => {}
            Err(e) => break Err(err(e)),
        }
        let next = match job::next_queued(store) {
            Ok(next) => next,
            Err(e) => break Err(err(e)),
        };
        let Some(next) = next else {
            if opts.once {
                break Ok(());
            }
            // the stop is looked at every second of the wait, so a server
            // that is stopping is not held up by an empty queue
            for _ in 0..opts.every.max(1) {
                if stop() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            continue;
        };
        match job::take(store, next.id) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => break Err(err(e)),
        }
        let argv = next.argv().unwrap_or_default();
        let line = format!("job {}: nils {}", next.id, argv.join(" "));
        let _ = if opts.quiet {
            writeln!(std::io::stderr(), "nils serve: {line}")
        } else {
            writeln!(std::io::stdout(), "{line}")
        };
        // Wave 4c section 6.1: the roles the door recorded reach the verb,
        // which runs under them and never under the worker's own.
        let roles = next.args["roles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let raw = if next.args["may_project_raw"].as_bool() == Some(true) {
            "1"
        } else {
            "0"
        };
        // The queued command line names no registry; the worker's is the one
        // it runs in.
        let spawned = std::process::Command::new(&me)
            .arg("--registry")
            .arg(home.dir())
            .args(&argv)
            .env(job::ADOPT_VAR, next.id.to_string())
            .env(
                "NILS_PRINCIPAL",
                next.principal().unwrap_or(&crate::actor()),
            )
            .env("NILS_JOB_ROLES", roles)
            .env("NILS_JOB_RAW", raw)
            .env("NILS_INGEST_ROOTS", &roots_env)
            .env(
                nils_registry::actor::VAR,
                if next.args["actor"].is_object() {
                    next.args["actor"].to_string()
                } else {
                    nils_registry::actor::absent().to_string()
                },
            )
            .stdout(if opts.quiet {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .spawn();
        // The worker says it is alive while the job runs; a cancel of the
        // worker lets the job finish and then stops.
        let mut asked_to_stop = false;
        let status = spawned.and_then(|mut child| {
            let mut beaten = Instant::now();
            loop {
                if let Some(status) = child.try_wait()? {
                    break Ok(status);
                }
                if beaten.elapsed() >= BEAT_EVERY {
                    beaten = Instant::now();
                    if job::beat(
                        store,
                        worker,
                        Some(&serde_json::json!({ "ran": ran, "running": next.id })),
                    )
                    .is_ok_and(|asked| asked == job::Asked::Cancel)
                    {
                        asked_to_stop = true;
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        // The verb adopted the row and finished it itself; the worker writes
        // the outcome only when the verb did not.
        let (state, error) = match status {
            Ok(s) if s.success() => (State::Done, None),
            Ok(s) => (State::Failed, Some(format!("exit status {s}"))),
            Err(e) => (State::Failed, Some(e.to_string())),
        };
        match job::show(store, next.id) {
            Ok(now) => {
                if (now.is_some_and(|j| !j.state.is_over()) || state == State::Failed)
                    && let Err(e) = job::finish(store, next.id, state, error.as_deref())
                {
                    break Err(err(e));
                }
            }
            Err(e) => break Err(err(e)),
        }
        ran += 1;
        if asked_to_stop {
            break Ok(());
        }
    };
    let _ = job::finish(store, worker, State::Done, None);
    outcome.map(|()| ran)
}
