// SPDX-License-Identifier: AGPL-3.0-only
//! The queue's worker (Wave 4c section 6.1): queued jobs run one at a time,
//! oldest first, each as a `nils` process of its own under the detail and the
//! actor the door recorded. `nils jobs work` runs it in the foreground, and
//! `nils serve --worker` runs it beside the doors, so a job queued at a door
//! runs without anyone starting a worker by hand.

use std::collections::VecDeque;
use std::io::{BufRead as _, Write as _};
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

/// How much of a job's stderr the worker keeps for the job's error when
/// the verb ends without finishing its own row (lab 26, defect 6): the
/// last lines, bounded in bytes, so a job that stopped says why as a job
/// run from the command line does.
const TAIL_LINES: usize = 8;
const TAIL_BYTES: usize = 2_000;

/// What the verb printed last: read line by line as the child runs, so a
/// verb that says a great deal never fills a pipe and stalls, echoed to
/// this process's stderr unless the worker is quiet.
fn tail_of(stderr: std::process::ChildStderr, quiet: bool) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept: VecDeque<String> = VecDeque::new();
        let mut bytes = 0usize;
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if !quiet {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
            let trimmed = line.trim_end().to_string();
            if trimmed.is_empty() {
                continue;
            }
            bytes += trimmed.len();
            kept.push_back(trimmed);
            while kept.len() > TAIL_LINES || (bytes > TAIL_BYTES && kept.len() > 1) {
                if let Some(gone) = kept.pop_front() {
                    bytes -= gone.len();
                }
            }
        }
        kept.into_iter()
            .map(|l| l.strip_prefix("nils: ").unwrap_or(&l).to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

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
    /// Lab 26b, finding 4: the file workers a queued run takes where its own
    /// command line names none, which is the engine's `--workers`. None
    /// where nobody set one, and the verb's own default stands.
    pub(crate) workers: Option<usize>,
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
        let argv = with_workers(next.argv().unwrap_or_default(), opts.workers);
        let line = format!("job {}: nils {}", next.id, argv.join(" "));
        let _ = if opts.quiet {
            writeln!(std::io::stderr(), "nils serve: {line}")
        } else {
            writeln!(std::io::stdout(), "{line}")
        };
        // Wave 4c section 6.1: the detail the door recorded reaches the
        // verb, which runs under it and never under the worker's own. A job
        // queued before grants runs under the detail of the highest role it
        // recorded, and one that recorded neither runs as plain.
        let detail = crate::grants::Detail::of_job(&next.args);
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
            .env("NILS_JOB_DETAIL", detail.name())
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
            .stderr(Stdio::piped())
            .spawn();
        // The worker says it is alive while the job runs; a cancel of the
        // worker lets the job finish and then stops. What the verb printed
        // last is kept for the job's error.
        let mut asked_to_stop = false;
        let mut tail: Option<std::thread::JoinHandle<String>> = None;
        let status = spawned.and_then(|mut child| {
            tail = child.stderr.take().map(|e| tail_of(e, opts.quiet));
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
        // the outcome only when the verb did not, and then the error is what
        // the verb printed last, or its exit status when it printed nothing.
        let printed = tail.and_then(|t| t.join().ok()).unwrap_or_default();
        let (state, error) = match status {
            Ok(s) if s.success() => (State::Done, None),
            Ok(s) if printed.is_empty() => (State::Failed, Some(format!("exit status {s}"))),
            Ok(_) => (State::Failed, Some(printed)),
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
        // Record 26 §7: a job that ended done queues the next step of its
        // chain, under what the first job recorded; a step the recorded
        // grants do not reach ends the chain, and the job's result says so.
        match job::show(store, next.id) {
            Ok(Some(ended)) if ended.state == State::Done => {
                match crate::chain::continue_chain(store, &ended) {
                    Ok(Some(queued)) => {
                        let line = format!("job {}: then queued job {queued}", next.id);
                        let _ = if opts.quiet {
                            writeln!(std::io::stderr(), "nils serve: {line}")
                        } else {
                            writeln!(std::io::stdout(), "{line}")
                        };
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = writeln!(
                            std::io::stderr(),
                            "nils: job {}: the chain could not go on: {e}",
                            next.id
                        );
                    }
                }
            }
            Ok(_) => {}
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

/// Whether a verb reads `--workers`: the two that walk a tree file by file,
/// and the pyramid's encoder. Everything else counts its work in windows or
/// in rows and has no such flag.
fn takes_workers(argv: &[String]) -> bool {
    match argv.first().map(String::as_str) {
        Some("digest" | "pseudonymize") => true,
        Some("pyramid") => argv.get(1).is_some_and(|w| w == "build"),
        _ => false,
    }
}

/// Lab 26b, finding 4: a queued run takes the workers the engine was started
/// with where its own command line names none. A run queued with `--workers`
/// keeps what was asked for, and a verb with no such flag is left alone, so
/// the setting reaches the work without rewriting anybody's command line.
fn with_workers(mut argv: Vec<String>, workers: Option<usize>) -> Vec<String> {
    let Some(n) = workers.filter(|n| *n > 0) else {
        return argv;
    };
    if !takes_workers(&argv)
        || argv
            .iter()
            .any(|a| a == "--workers" || a.starts_with("--workers="))
    {
        return argv;
    }
    argv.push("--workers".to_string());
    argv.push(n.to_string());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn a_queued_run_takes_the_engines_workers_where_it_names_none() {
        // Lab 26b, finding 4: every pseudonymise and digest ran with one
        // worker per core under an engine started with four, because the
        // queued line named none and the verb's own default stood.
        assert_eq!(
            with_workers(argv("digest @ward"), Some(4)),
            argv("digest @ward --workers 4")
        );
        assert_eq!(
            with_workers(argv("pseudonymize @ward --name n"), Some(4)),
            argv("pseudonymize @ward --name n --workers 4")
        );
        assert_eq!(
            with_workers(argv("pyramid build --stack 7"), Some(4)),
            argv("pyramid build --stack 7 --workers 4")
        );
    }

    #[test]
    fn what_the_caller_asked_for_stands_and_other_verbs_are_left_alone() {
        for line in [
            // named by the caller, in either spelling
            "digest @ward --workers 16",
            "digest @ward --workers=16",
            // no such flag on these
            "classify --pack mri",
            "release --name r --out /tmp/r",
            "pyramid list",
        ] {
            assert_eq!(with_workers(argv(line), Some(4)), argv(line), "{line}");
        }
        // and a worker started by hand names none, so nothing is added
        assert_eq!(
            with_workers(argv("digest @ward"), None),
            argv("digest @ward")
        );
        assert_eq!(
            with_workers(argv("digest @ward"), Some(0)),
            argv("digest @ward")
        );
    }
}
