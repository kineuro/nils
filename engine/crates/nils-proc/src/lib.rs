// SPDX-License-Identifier: AGPL-3.0-only

//! Process and file helpers that more than one crate of the engine needs
//! and that depend on nothing else of it: waiting on a child while ticking,
//! ending a child with its process group, and moving a file where a rename
//! cannot.

use std::path::Path;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// Wait for `child` to end, polling it every `poll`. About every `every`
/// (checked after each poll that finds it running) `tick` is called, and
/// `tick` answering true stops the wait: the answer is then `None` and the
/// child is left running for the caller to end. An error from the child's
/// status is returned as it came.
pub fn wait_with_tick(
    child: &mut Child,
    poll: Duration,
    every: Duration,
    tick: &mut dyn FnMut() -> bool,
) -> std::io::Result<Option<ExitStatus>> {
    let mut ticked = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if ticked.elapsed() >= every {
            ticked = Instant::now();
            if tick() {
                return Ok(None);
            }
        }
        std::thread::sleep(poll);
    }
}

/// Wait for `child` to end within `cap`, polling every `poll`. A child still
/// running once more than `cap` has passed is killed and reaped, and the
/// answer is `None`.
pub fn wait_within(
    child: &mut Child,
    poll: Duration,
    cap: Duration,
) -> std::io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    let status = wait_with_tick(child, poll, Duration::ZERO, &mut || started.elapsed() > cap)?;
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(status)
}

/// Send SIGKILL to the process group `leader` leads (a child spawned with
/// `process_group(0)`). Nothing where the platform has no process groups.
pub fn kill_group(leader: u32) {
    #[cfg(unix)]
    #[allow(
        unsafe_code,
        reason = "kill(2) on the group this process made for the child, by its recorded pid"
    )]
    {
        if let Ok(pid) = i32::try_from(leader)
            && pid > 0
        {
            // SAFETY: a plain system call with two integers; the group is
            // the child's own, made at its spawn, and the pid is the one
            // this process recorded for it
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
    #[cfg(not(unix))]
    {
        let _ = leader;
    }
}

/// End a spawned child and everything in its group, and reap it; answers how
/// it ended.
pub fn end_group(child: &mut Child) -> std::io::Result<ExitStatus> {
    kill_group(child.id());
    let _ = child.kill();
    child.wait()
}

/// Move a file: a rename, or, where that cannot be one (another
/// filesystem), a copy and then the removal of the source. A failed copy is
/// the error; a source that cannot be removed after a good copy is left.
pub fn rename_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from).ok();
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sleeper(seconds: &str) -> Child {
        Command::new("sleep")
            .arg(seconds)
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn a_short_child_ends_before_the_cap() {
        let mut child = Command::new("true").spawn().unwrap();
        let status = wait_within(
            &mut child,
            Duration::from_millis(5),
            Duration::from_secs(10),
        );
        assert!(status.unwrap().unwrap().success());
    }

    #[test]
    fn a_long_child_is_killed_at_the_cap() {
        let mut child = sleeper("30");
        let status = wait_within(
            &mut child,
            Duration::from_millis(5),
            Duration::from_millis(50),
        );
        assert!(status.unwrap().is_none());
        assert!(child.try_wait().unwrap().is_some(), "reaped");
    }

    #[test]
    fn a_tick_that_stops_leaves_the_child_to_the_caller() {
        let mut child = sleeper("30");
        let mut ticks = 0;
        let waited = wait_with_tick(
            &mut child,
            Duration::from_millis(5),
            Duration::from_millis(10),
            &mut || {
                ticks += 1;
                ticks == 3
            },
        );
        assert!(waited.unwrap().is_none());
        assert!(child.try_wait().unwrap().is_none(), "still running");
        let _ = end_group(&mut child);
    }

    #[test]
    fn a_move_lands_the_file_and_takes_it_from_the_source() {
        let dir = std::env::temp_dir().join(format!("nils-proc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (from, to) = (dir.join("a"), dir.join("b"));
        std::fs::write(&from, b"x").unwrap();
        rename_or_copy(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"x");
        assert!(rename_or_copy(&from, &to).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
