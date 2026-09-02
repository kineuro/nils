// SPDX-License-Identifier: AGPL-3.0-only

//! How a run is asked to stop (§10). The first request lets the writer
//! finish what is already read and commit it; the second abandons the batch
//! in flight, whose transaction rolls back. The binary raises the requests
//! from SIGINT (and SIGTERM); the tests raise them through a scripted stop.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// What has been asked of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Asked {
    /// Nothing: the run goes on.
    Nothing,
    /// Once: write what is read, commit, stop.
    Stop,
    /// Twice: abandon the batch in flight.
    Abort,
}

/// A token every stage of a run holds a handle to.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicU8>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    /// One request more: the first asks for a stop, the second for an abort,
    /// any later one changes nothing. Returns what is asked now.
    pub fn request(&self) -> Asked {
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                Some(n.saturating_add(1).min(2))
            });
        self.asked()
    }

    pub fn asked(&self) -> Asked {
        match self.0.load(Ordering::SeqCst) {
            0 => Asked::Nothing,
            1 => Asked::Stop,
            _ => Asked::Abort,
        }
    }

    /// A stop or an abort is asked: nothing new is read.
    pub fn stop(&self) -> bool {
        self.asked() >= Asked::Stop
    }

    /// An abort is asked: the batch in flight is abandoned.
    pub fn abort(&self) -> bool {
        self.asked() >= Asked::Abort
    }
}

/// How a run ended when it did not run to the end, as the report and the
/// batch record it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cancelled {
    /// One request: everything read was written and committed.
    Stopped,
    /// Two: the batch in flight was abandoned; its files are read again next
    /// time.
    Aborted,
}

impl Cancelled {
    pub fn name(self) -> &'static str {
        match self {
            Cancelled::Stopped => "stopped",
            Cancelled::Aborted => "aborted",
        }
    }
}

/// The environment variable a scripted stop is read from.
pub const SCRIPT_ENV: &str = "NILS_DEBUG_STOP";

/// What a scripted stop does, after the batch it waits for has committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Request a stop, as one signal would.
    Stop,
    /// Request an abort, as two signals would.
    Abort,
    /// Raise SIGINT at the process, as a hand on Ctrl-C would (unix).
    Interrupt,
    /// Raise SIGTERM at the process (unix).
    Terminate,
    /// End the process at once, as a power cut would.
    Kill,
    /// End the process inside the next transaction, after its rows are
    /// written and before it commits.
    KillInside,
}

/// A stop the tests script for the binary: `NILS_DEBUG_STOP=<action>:<n>`,
/// acting once `n` batches have committed. `stop`, `abort`, `interrupt`,
/// `terminate`, `kill` and `kill-inside` are the actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scripted {
    pub action: Action,
    pub after: u64,
}

impl Scripted {
    pub fn parse(text: &str) -> Option<Scripted> {
        let (action, after) = text.trim().split_once(':')?;
        let action = match action {
            "stop" => Action::Stop,
            "abort" => Action::Abort,
            "interrupt" => Action::Interrupt,
            "terminate" => Action::Terminate,
            "kill" => Action::Kill,
            "kill-inside" => Action::KillInside,
            _ => return None,
        };
        Some(Scripted {
            action,
            after: after.parse().ok()?,
        })
    }

    /// The scripted stop in the environment, if any.
    pub fn from_env() -> Option<Scripted> {
        std::env::var(SCRIPT_ENV)
            .ok()
            .and_then(|text| Scripted::parse(&text))
    }

    /// The moment `writes` batches have committed: act, unless the action is
    /// the one inside a transaction. A raised signal is given a few seconds
    /// to reach the token through the binary's handler, so that what follows
    /// sees the request the way a run under a hand on Ctrl-C would.
    pub fn after_commit(&self, writes: u64, cancel: &Cancel) {
        if writes != self.after {
            return;
        }
        match self.action {
            Action::Stop => {
                cancel.request();
            }
            Action::Abort => {
                cancel.request();
                cancel.request();
            }
            Action::Interrupt => raise(Signal::Interrupt, cancel),
            Action::Terminate => raise(Signal::Terminate, cancel),
            Action::Kill => std::process::abort(),
            Action::KillInside => {}
        }
    }

    /// Inside the transaction that follows `writes` committed batches, with
    /// its rows written: end the process if that is the action.
    pub fn inside_transaction(&self, writes: u64) {
        if self.action == Action::KillInside && writes == self.after {
            std::process::abort();
        }
    }
}

enum Signal {
    Interrupt,
    Terminate,
}

#[cfg(unix)]
fn raise(signal: Signal, cancel: &Cancel) {
    let signal = match signal {
        Signal::Interrupt => nix::sys::signal::Signal::SIGINT,
        Signal::Terminate => nix::sys::signal::Signal::SIGTERM,
    };
    let _ = nix::sys::signal::raise(signal);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !cancel.stop() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(not(unix))]
fn raise(_signal: Signal, _cancel: &Cancel) {}

/// Whether the process `pid` is alive on this host: no such process means
/// it is gone; a process that exists but cannot be signalled (another user's)
/// counts as alive. Only unix can tell; elsewhere nothing is known.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_step_from_nothing_to_stop_to_abort_and_stay() {
        let c = Cancel::new();
        assert_eq!(c.asked(), Asked::Nothing);
        assert!(!c.stop() && !c.abort());
        assert_eq!(c.request(), Asked::Stop);
        assert!(c.stop() && !c.abort());
        let other = c.clone();
        assert_eq!(other.request(), Asked::Abort);
        assert!(c.stop() && c.abort());
        assert_eq!(c.request(), Asked::Abort);
    }

    #[test]
    fn a_scripted_stop_parses_and_acts_once() {
        assert_eq!(
            Scripted::parse("abort:3"),
            Some(Scripted {
                action: Action::Abort,
                after: 3
            })
        );
        assert_eq!(
            Scripted::parse(" kill-inside:0 ").map(|s| s.action),
            Some(Action::KillInside)
        );
        assert_eq!(Scripted::parse("kill"), None);
        assert_eq!(Scripted::parse("halt:1"), None);
        assert_eq!(Scripted::parse("stop:x"), None);
        let c = Cancel::new();
        let s = Scripted::parse("stop:2").unwrap();
        s.after_commit(1, &c);
        assert_eq!(c.asked(), Asked::Nothing);
        s.after_commit(2, &c);
        assert_eq!(c.asked(), Asked::Stop);
        s.after_commit(3, &c);
        assert_eq!(c.asked(), Asked::Stop, "acts on its count only");
        Scripted::parse("abort:3").unwrap().after_commit(3, &c);
        assert_eq!(c.asked(), Asked::Abort);
        // the one inside a transaction does nothing at a commit
        Scripted::parse("kill-inside:0")
            .unwrap()
            .after_commit(0, &Cancel::new());
    }

    #[test]
    fn this_process_is_alive_and_a_wild_pid_is_not() {
        let me = i64::from(std::process::id());
        if cfg!(unix) {
            assert_eq!(process_alive(me), Some(true));
            assert_eq!(process_alive(0), None);
            assert_eq!(process_alive(i64::from(i32::MAX) + 1), None);
        } else {
            assert_eq!(process_alive(me), None);
        }
    }

    #[test]
    fn cancelled_serialises_as_a_word() {
        assert_eq!(
            serde_json::to_string(&Cancelled::Aborted).unwrap(),
            "\"aborted\""
        );
        assert_eq!(
            serde_json::from_str::<Cancelled>("\"stopped\"").unwrap(),
            Cancelled::Stopped
        );
        assert_eq!(Cancelled::Stopped.name(), "stopped");
    }
}
