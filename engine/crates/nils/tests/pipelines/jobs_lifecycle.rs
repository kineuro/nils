// SPDX-License-Identifier: AGPL-3.0-only

//! UX round 3 (2026-11-08), a job's life on a site whose engine runs as a
//! service: `nils jobs resume` queues and runs nothing in the shell, a run
//! typed at the keyboard beside the engine's worker is refused with the
//! line that queues it, apptainer is refused before any unit where it
//! could not apply the run's limits and is started in `/`, a cancel stops
//! an image's pull at once, and a run whose process was killed while its
//! engine stayed up is not taken up by itself.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{FAKE_APPTAINER, Lab, have, stack_slow};

/// A process this test started, killed and reaped however the test ends:
/// only the pid its own spawn recorded.
struct Held(Option<Child>);

impl Held {
    fn wait_within(&mut self, secs: u64) -> Option<std::process::ExitStatus> {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if let Some(s) = self.0.as_mut().unwrap().try_wait().unwrap() {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Gates the stand-in units wait at, opened at the end so nothing a run
/// started outlives the test.
struct Gates(PathBuf);

impl Gates {
    fn open_all(&self) {
        let dir = self.0.parent().unwrap();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(gate) = name.strip_suffix(".pid").filter(|n| n.starts_with("gate-")) {
                let _ = std::fs::write(dir.join(gate), "");
            }
        }
    }

    fn waiting(&self) -> usize {
        let dir = self.0.parent().unwrap();
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with("gate-") && n.ends_with(".pid")
            })
            .count()
    }
}

impl Drop for Gates {
    fn drop(&mut self) {
        self.open_all();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn alive(pid: i64) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit(')').next().map(|r| r.trim_start().to_string()))
        .is_some_and(|state| !state.starts_with('Z'))
}

fn until(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed().as_secs() < secs, "{what} within {secs} s");
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Lab {
    /// A worker of the pipeline lane that keeps going, as the engine's
    /// service runs one, its units noting themselves in `trace`.
    fn worker(&self, trace: &Path, gate: Option<&Path>) -> Held {
        let mut c = self.command(&self.path);
        c.args(["jobs", "work", "--lane", "pipelines", "--every", "1"])
            .env("LANE_TRACE", trace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.work.path().join("worker.err")).unwrap());
        if let Some(g) = gate {
            c.env("SLOW_GATE", g);
        }
        let held = Held(Some(c.spawn().unwrap()));
        until("the worker's row", 30, || {
            self.count(
                "SELECT COUNT(*) FROM job WHERE kind = 'pipeline-worker' AND state = 'running'",
            ) == 1
        });
        held
    }

    fn job(&self, id: i64) -> Value {
        self.json(&["jobs", "show", &id.to_string(), "--json"])
    }

    fn jobs(&self) -> Vec<Value> {
        self.json(&["jobs", "list", "--all", "--json"])
            .as_array()
            .unwrap()
            .clone()
    }
}

/// The number at the end of "queued job N" or "queued as job N".
fn queued_id(said: &str) -> i64 {
    let at = said.find("job ").unwrap() + 4;
    said[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// Item 1: `nils jobs resume` puts the job back on the queue under what it
/// recorded and returns at once; the worker runs it. A job not over, or
/// one already queued again, is refused with why.
#[test]
fn jobs_resume_queues_the_job_for_the_worker_and_runs_nothing_here() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("jobs-resume-queues");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    let said = lab.ok(
        &[
            "jobs",
            "enqueue",
            "--name",
            "slow",
            "--",
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=0.1",
        ],
        None,
    );
    let first = queued_id(&said);
    lab.ok(&["jobs", "cancel", &first.to_string()], None);
    // a job not over is refused; the cancelled one goes again
    let started = Instant::now();
    let said = lab.ok(&["jobs", "resume", &first.to_string()], None);
    assert!(started.elapsed().as_secs() < 20, "the resume waited");
    assert!(said.contains("queued as job"), "{said}");
    assert!(said.contains("waits until the engine"), "{said}");
    let again = queued_id(&said);
    assert_ne!(again, first);
    assert_eq!(
        lab.count("SELECT COUNT(*) FROM pipeline_run"),
        0,
        "nothing ran in the shell"
    );
    let j = lab.job(again);
    assert_eq!(j["state"], "queued", "{j}");
    assert_eq!(j["args"]["again_of"], first, "{j}");
    assert_eq!(j["args"]["principal"], "anna@ward-3", "{j}");
    assert!(j["args"]["detail"].is_string(), "{j}");
    let (ok, _, err) = lab.run(&["jobs", "resume", &again.to_string()], None);
    assert!(!ok && err.contains("once it is over"), "{err}");
    let (ok, _, err) = lab.run(&["jobs", "resume", &first.to_string()], None);
    assert!(
        !ok && err.contains(&format!("already going again as job {again}")),
        "{err}"
    );
    // the worker takes it in its turn
    lab.ok(&["jobs", "work", "--lane", "pipelines", "--once"], None);
    let j = lab.job(again);
    assert_eq!(j["state"], "done", "{j}");
    let run: Value = lab.json(&["pipeline", "runs", "1", "--json"]);
    assert_eq!(run["status"], "done", "{run}");
}

/// Item 2: a run typed at the keyboard while the engine's worker runs
/// pipelines for the registry is refused before anything starts, with the
/// `nils jobs enqueue` line for the same words; `--here` runs it all the
/// same, and the worker's own runs are never refused.
#[test]
fn a_run_typed_beside_the_engine_s_worker_is_refused_with_the_line_that_queues_it() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("jobs-foreground-refused");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    let t = lab.work.path().join("trace");
    let words = [
        "run",
        "slow",
        "--select",
        "selection:every@1",
        "--param",
        "sleep=0.1",
    ];
    let mut worker = lab.worker(&t, None);
    let (ok, _, err) = lab.run(&words, None);
    assert!(!ok, "{err}");
    assert!(err.contains("the engine is running"), "{err}");
    assert!(
        err.contains(
            "nils jobs enqueue --name slow -- run slow --select selection:every@1 --param sleep=0.1"
        ),
        "{err}"
    );
    assert!(err.contains("--here"), "{err}");
    assert_eq!(lab.count("SELECT COUNT(*) FROM pipeline_run"), 0);
    // a pre-flight starts nothing, and is answered
    let (ok, _, err) = lab.run(
        &[
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--preflight",
            "--json",
        ],
        None,
    );
    assert!(ok, "{err}");
    // queued, the worker's own run goes
    let said = lab.ok(
        &[&["jobs", "enqueue", "--name", "slow", "--"][..], &words[..]].concat(),
        None,
    );
    let queued = queued_id(&said);
    until("the queued run", 120, || {
        lab.job(queued)["state"]
            .as_str()
            .is_some_and(|s| s == "done" || s == "failed")
    });
    assert_eq!(lab.job(queued)["state"], "done", "{}", lab.job(queued));
    // --here runs in the shell all the same
    let mut here: Vec<&str> = words.to_vec();
    here.push("--here");
    let (ok, _, err) = lab.run(&here, None);
    assert!(ok, "{err}");
    // with no live worker the run goes in the shell as before
    let _ = worker.0.as_mut().unwrap().kill();
    assert!(worker.wait_within(10).is_some());
    let (ok, _, err) = lab.run(&words, None);
    assert!(ok, "{err}");
    assert_eq!(
        lab.count("SELECT COUNT(*) FROM pipeline_run WHERE status = 'done'"),
        3
    );
}

/// Item 2, pre-flight: apptainer run by an account that is not root, where
/// the run's containers would be held to cores and memory and no session
/// bus is named, is refused before any unit starts, saying so and what to
/// do; with a bus it runs, and apptainer is started in `/` whatever folder
/// the run was started in.
#[test]
fn apptainer_without_a_session_bus_is_refused_first_and_is_started_in_the_root() {
    if !have("python3") {
        eprintln!("python3 is not installed; the stand-ins need it, so this test is skipped");
        return;
    }
    let lab = Lab::new("jobs-session-bus");
    let fake = lab.bin.file("apptainer.py", FAKE_APPTAINER.as_bytes());
    let cwds = lab.bin.path().join("apptainer.cwd");
    let wrapper = lab.bin.file(
        "apptainer",
        format!(
            "#!/bin/sh\n[ \"$1\" = build ] || [ \"$1\" = run ] && pwd >> '{}'\nexec python3 '{}' \"$@\"\n",
            cwds.display(),
            fake.display()
        )
        .as_bytes(),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let args_log = lab.bin.path().join("apptainer.log");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    lab.ok(&["pipeline", "runtime", "--set", "apptainer"], None);
    let words = [
        "run",
        "slow",
        "--select",
        "selection:every@1",
        "--param",
        "sleep=0.1",
    ];
    let out = lab
        .command(&lab.path)
        .args(words)
        .env("NILS_TEST_APPTAINER_LIMITS", "1")
        .env("FAKE_APPTAINER_ARGS", &args_log)
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(
        err.contains("apptainer cannot apply this run's memory and thread limits without a user session bus (DBUS_SESSION_BUS_ADDRESS is not set)"),
        "{err}"
    );
    assert!(
        err.contains("queue it with nils jobs enqueue so the engine runs it"),
        "{err}"
    );
    assert!(!err.contains("install rootless podman"), "{err}");
    assert_eq!(lab.count("SELECT COUNT(*) FROM pipeline_run"), 0);
    assert!(!cwds.exists(), "apptainer was started");
    // the pre-flight names the same check
    let out = lab
        .command(&lab.path)
        .args([
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--preflight",
            "--json",
        ])
        .env("NILS_TEST_APPTAINER_LIMITS", "1")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("\"session-bus\""), "{text}");
    // with a bus, from a folder of its own, every apptainer call starts in /
    let elsewhere = lab.work.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let out = lab
        .command(&lab.path)
        .args(words)
        .current_dir(&elsewhere)
        .env("NILS_TEST_APPTAINER_LIMITS", "1")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
        .env("FAKE_APPTAINER_ARGS", &args_log)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = std::fs::read_to_string(&cwds).unwrap();
    assert!(seen.lines().count() >= 2, "{seen}");
    assert!(seen.lines().all(|l| l == "/"), "{seen}");
}

/// Item 3a: a cancel of a run building its image stops the build at once,
/// with every process the build started, and the job and the run end
/// cancelled.
#[test]
fn a_cancel_during_the_image_s_pull_stops_it_at_once() {
    if !have("python3") {
        eprintln!("python3 is not installed; the stand-ins need it, so this test is skipped");
        return;
    }
    let lab = Lab::new("jobs-cancel-pull");
    let pulling = lab.work.path().join("pull.pid");
    // a build that fetches in a process of its own, for minutes
    let fake = lab.bin.file(
        "apptainer",
        format!(
            "#!/usr/bin/env python3\nimport subprocess, sys\na = sys.argv[1:]\nif not a or a[0] == '--version':\n    print('apptainer version 1.3.4'); sys.exit(0)\nif a[0] == 'build':\n    p = subprocess.Popen(['sleep', '300'])\n    open('{}', 'w').write(str(p.pid))\n    sys.exit(p.wait())\nsys.exit(0)\n",
            pulling.display()
        )
        .as_bytes(),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    lab.ok(&["pipeline", "runtime", "--set", "apptainer"], None);
    let mut run = Held(Some(
        lab.command(&lab.path)
            .args(["run", "slow", "--select", "selection:every@1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(lab.work.path().join("run.err")).unwrap())
            .spawn()
            .unwrap(),
    ));
    let pid_of_pull = || -> Option<i64> {
        std::fs::read_to_string(&pulling)
            .ok()
            .and_then(|p| p.trim().parse().ok())
    };
    until("the pull", 60, || pid_of_pull().is_some());
    let pull = pid_of_pull().unwrap();
    // the sleep is the build's own: whatever happens, it goes at the end
    struct Reap(i64);
    impl Drop for Reap {
        fn drop(&mut self) {
            if alive(self.0) {
                let _ = std::process::Command::new("kill")
                    .args(["-KILL", &self.0.to_string()])
                    .status();
            }
        }
    }
    let _reap = Reap(pull);
    let job: i64 = lab.count("SELECT id FROM job WHERE kind = 'pipeline'");
    let asked = Instant::now();
    lab.ok(&["jobs", "cancel", &job.to_string()], None);
    let status = run.wait_within(20);
    assert!(status.is_some(), "the run went on after its cancel");
    assert!(
        asked.elapsed().as_secs() < 10,
        "the cancel took {} s",
        asked.elapsed().as_secs()
    );
    until("the pull's own process to go", 10, || !alive(pull));
    let j = lab.job(job);
    assert_eq!(j["state"], "cancelled", "{j}");
    let r: Value = lab.json(&["pipeline", "runs", "1", "--json"]);
    assert_eq!(r["status"], "cancelled", "{r}");
}

/// Item 3b: a run whose process is killed while its engine stays up is
/// marked interrupted, says how to take it up, and is not queued again by
/// itself; `nils jobs resume` then takes it up where it stopped. (A run
/// whose engine went away is still taken up by itself:
/// `a_killed_run_is_taken_up_where_it_stopped`.)
#[test]
fn a_run_killed_while_its_engine_runs_is_not_taken_up_by_itself() {
    if !have("python3") {
        eprintln!(
            "python3 is not installed; the stand-in podman needs it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new("jobs-killed-by-hand");
    lab.add_descriptor("slow", &stack_slow("slow", "{cores: 1, memory-gb: 1}", ""));
    lab.ok(
        &["pipeline", "lane", "--cores", "2", "--memory-gb", "100"],
        None,
    );
    let t = lab.work.path().join("trace");
    let gates = Gates(lab.work.path().join("gate"));
    let worker = lab.worker(&t, Some(&gates.0));
    // a second later than the worker's start, as any job it runs is
    std::thread::sleep(Duration::from_millis(1100));
    let said = lab.ok(
        &[
            "jobs",
            "enqueue",
            "--name",
            "slow",
            "--",
            "run",
            "slow",
            "--select",
            "selection:every@1",
            "--param",
            "sleep=0.1",
        ],
        None,
    );
    let queued = queued_id(&said);
    until("two units at their gates", 120, || gates.waiting() >= 2);
    let pid = lab.job(queued)["pid"].as_i64().unwrap();
    assert!(alive(pid));
    // the job's own process, as it recorded it, killed by hand
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    until("the run marked", 60, || {
        lab.count("SELECT COUNT(*) FROM pipeline_run WHERE status = 'interrupted'") == 1
    });
    // the worker looks again, every second, and queues nothing
    std::thread::sleep(Duration::from_secs(4));
    let r: Value = lab.json(&["pipeline", "runs", "1", "--json"]);
    assert_eq!(r["status"], "interrupted", "{r}");
    let error = r["error"].as_str().unwrap_or_default();
    assert!(error.contains("ended while the engine ran"), "{r}");
    assert!(error.contains(&format!("nils jobs resume {queued}")), "{r}");
    let resumed = |jobs: &[Value]| {
        jobs.iter()
            .filter(|j| j["args"]["queued"] == serde_json::json!(["run", "--resume", "1"]))
            .count()
    };
    assert_eq!(resumed(&lab.jobs()), 0, "{:?}", lab.jobs());
    // taken up by hand: queued as `run --resume 1`, and the worker ends it
    gates.open_all();
    let said = lab.ok(&["jobs", "resume", &queued.to_string()], None);
    assert!(said.contains("nils run --resume 1"), "{said}");
    assert!(said.contains("the engine's worker"), "{said}");
    let again = queued_id(&said);
    // every unit that comes to its gate from now on finds it open
    until("the run taken up", 120, || {
        gates.open_all();
        lab.job(again)["state"]
            .as_str()
            .is_some_and(|s| s == "done" || s == "failed")
    });
    gates.open_all();
    let r: Value = lab.json(&["pipeline", "runs", "1", "--json"]);
    assert_eq!(r["status"], "done", "{r}");
    assert_eq!(r["summary"]["units"]["succeeded"], 4, "{r}");
    drop(worker);
}
