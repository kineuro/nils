// SPDX-License-Identifier: AGPL-3.0-only

//! One writer per tree, and a release that beats and stops (Wave 7b, the
//! maintenance track). A release never beat its job's heart before, so a
//! second release started by mistake into the same folder found the first
//! stale after a minute on another node, and a cancel was not seen until the
//! release ended. Now the job's row moves with every stack, a cancel stops
//! the release after the stack in hand and leaves its row unfinished (a
//! stopped release is not a version; the next run goes on with its number),
//! and the folder's own lock, beating with the job, refuses a second writer
//! on any host and of any registry while it is fresh.
//!
//! The lock tests stand both ways of the freshness window: a fresh stamp is
//! now, a stale one an hour ago, so a test binary that shortens the window
//! ([`job::FRESH_VAR`]) reads them the same.

mod common;

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::job::{self, Claim, State};
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::lock::{self, Refusal};
use nils_release::policy::Policy;
use nils_release::run::{self, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a release test key of some length";

/// One study of one person with this many series, each a stack of its own.
fn tree(stacks: u32) -> TempDir {
    let dir = TempDir::new("one-writer");
    for n in 1..=stacks {
        let series = format!("A.1.{n}");
        let sop = format!("A.1.{n}.1");
        let mut e = synth::minimal_mr("A.1", &series, &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, "P1"),
            synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
            synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()),
            synth::text(
                tags::SERIES_DESCRIPTION,
                VR::LO,
                &format!("ax t2 series {n}"),
            ),
            synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
            synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("s{n}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

fn registry(home_dir: &TempDir, source: &TempDir) -> (Home, Registry) {
    let home = Home::new(home_dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend: Backend::Sqlite,
        dsn: None,
        schema: None,
        scheme: Scheme::Blake2b32,
        key: "k".to_string(),
        display_length: 12,
        session_scheme: None,
    })
    .unwrap();
    let mut reg = home.open().unwrap();
    let mut s = nils_digest::Settings::new(source.path());
    s.name = "t".into();
    digest(&s, &mut reg).unwrap();
    (home, reg)
}

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| nils_pack::load(&common::shipped_mri(), None).expect("the MRI pack loads"))
}

fn settings<'a>(out: &'a Path, policy: &'a Policy, scheme: &'a SessionScheme) -> run::Settings<'a> {
    run::Settings {
        name: "test",
        root: out,
        policy,
        policy_from: nils_release::policy::Source::Flags,
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme,
        private: &[],
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout: run::Layout::Descriptive,
        naming: nils_release::name::Naming::Full,
        places: nils_release::bids::place::Options::default(),
        converter: None,
        compress: true,
        observations: &[],
        authors: &[],
    }
}

/// The files under the root, the lock left out, sorted.
fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|f| f != lock::FILE) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// An ISO stamp an hour ago: stale under any freshness a test sets.
fn an_hour_ago() -> String {
    nils_registry::time::iso_of(nils_registry::time::now_secs() - 3600)
}

/// The `error` and `finished_at` of a release's row.
fn row_of(reg: &mut Registry, release_id: i64) -> (Option<String>, Option<String>) {
    let store = reg.store();
    let sql = format!(
        "SELECT error, finished_at FROM {} WHERE id = {}",
        store.qualified("release"),
        release_id
    );
    let r = store.query_opt(&sql, &[]).unwrap().unwrap();
    (
        r.opt_text(0).unwrap().map(str::to_string),
        r.opt_text(1).unwrap().map(str::to_string),
    )
}

/// The finished versions of the registry's releases, oldest first.
fn finished_versions(reg: &mut Registry) -> Vec<String> {
    let store = reg.store();
    let sql = format!(
        "SELECT version FROM {} WHERE finished_at IS NOT NULL ORDER BY id",
        store.qualified("release")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

/// The one release job of the registry.
fn the_release_job(reg: &mut Registry) -> job::Job {
    let jobs = job::list(reg.store(), true, 100).unwrap();
    let mut found: Vec<job::Job> = jobs.into_iter().filter(|j| j.kind == "release").collect();
    assert_eq!(found.len(), 1, "one release job");
    found.remove(0)
}

/// (a) The job's row moves with every stack: the heartbeat is later after
/// the second stack than after the first, and the progress names the
/// stacks done of the stacks planned and the version being written.
#[test]
fn the_release_beats_its_heart_once_per_stack_with_the_stacks_done() {
    let source = tree(3);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let mut seen: Vec<(i64, String, serde_json::Value)> = Vec::new();
    let report = run::run_watched(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        &mut |store, job_id, done| {
            let j = job::show(store, job_id).unwrap().unwrap();
            seen.push((done, j.heartbeat_at.unwrap(), j.progress.unwrap()));
            if done == 1 {
                // the stamps are whole seconds: let one pass
                std::thread::sleep(std::time::Duration::from_millis(1100));
            }
        },
    )
    .unwrap();

    assert_eq!(report.stacks, 3);
    assert_eq!(report.stopped_after, None);
    assert_eq!(seen.len(), 3, "{seen:?}");
    for (done, _, progress) in &seen {
        assert_eq!(progress["done"], *done, "{progress}");
        assert_eq!(progress["stacks"], 3, "{progress}");
        assert_eq!(progress["version"], report.version, "{progress}");
        assert_eq!(progress["do"], "write", "{progress}");
        assert!(
            progress["beats"].as_i64().is_some_and(|b| b >= *done),
            "{progress}"
        );
    }
    assert!(seen[1].1 > seen[0].1, "the heartbeat moved: {seen:?}");
    let j = the_release_job(&mut reg);
    assert_eq!(j.state, State::Done);
    assert_eq!(j.progress.unwrap()["done"], 3);
    // (e) and the lock is gone once the release is over
    assert_eq!(lock::read(out.path()), None);
    assert_eq!(files_under(out.path()).len(), 3);
}

/// (b) A cancel set on the row while the second stack is in hand stops the
/// release after that stack: the job ends cancelled with the words, the two
/// stacks written are whole, the rest were not written, the report says it
/// stopped, the lock is gone; the release's row is left unfinished with the
/// stopped words, so it is not the newest finished version; and the next
/// run writes the rest under the same version number and finishes it.
#[test]
fn a_cancel_stops_the_release_after_the_stack_in_hand_and_the_next_run_goes_on() {
    let source = tree(4);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let report = run::run_watched(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        &mut |store, job_id, done| {
            if done == 1 {
                let now = job::request_cancel(store, job_id).unwrap();
                assert_eq!(now, Some(State::Cancelling));
            }
        },
    )
    .unwrap();

    assert_eq!(report.stacks, 4);
    assert_eq!(
        report.stopped_after,
        Some(2),
        "stopped after the stack in hand"
    );
    assert_eq!(report.added, 2);
    assert_eq!(report.written, 2);
    let written = files_under(out.path());
    assert_eq!(written.len(), 2, "{written:?}");
    for path in &written {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            &bytes[128..132],
            b"DICM",
            "a whole part 10 file: {}",
            path.display()
        );
        assert!(
            path.extension().is_some_and(|e| e != "part"),
            "no half-written file: {}",
            path.display()
        );
    }
    let j = the_release_job(&mut reg);
    assert_eq!(j.state, State::Cancelled);
    assert_eq!(j.error.as_deref(), Some(run::STOPPED));
    assert_eq!(j.result.unwrap()["stopped_after"], 2);
    assert_eq!(lock::read(out.path()), None);

    // the row of the stopped release says so and is not finished: a
    // stopped release is not a version
    let (error, finished) = row_of(&mut reg, report.release_id);
    assert_eq!(
        error.as_deref(),
        Some("stopped after 2 of 4 stacks; what was written stays written")
    );
    assert_eq!(finished, None, "a stopped release is not finished");
    assert!(finished_versions(&mut reg).is_empty());

    // run again to go on: the two left are added, the two written are left
    // alone, under the same version number, and that run is the version
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(again.stopped_after, None);
    assert_eq!(
        again.version, report.version,
        "the version number did not move"
    );
    assert_ne!(again.release_id, report.release_id);
    assert_eq!((again.unchanged, again.added), (2, 2), "{again:?}");
    assert_eq!(files_under(out.path()).len(), 4);
    assert_eq!(lock::read(out.path()), None);
    assert_eq!(finished_versions(&mut reg), vec![report.version.clone()]);
    let (error, finished) = row_of(&mut reg, again.release_id);
    assert_eq!(error, None);
    assert!(finished.is_some());
    // and a third run numbers the next version after the finished one
    let third = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_ne!(third.version, again.version);
    assert_eq!(third.unchanged, 4);
}

/// A cancel read at the beat after the last stack still ends the release
/// as stopped, with every stack counted as done: a stop the person asked
/// for is never turned into a done release.
#[test]
fn a_cancel_read_after_the_last_stack_still_ends_the_release_stopped() {
    let source = tree(2);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let report = run::run_watched(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        &mut |store, job_id, done| {
            if done == 2 {
                job::request_cancel(store, job_id).unwrap();
            }
        },
    )
    .unwrap();
    // the cancel was set after the last stack's beat: the release reads it
    // at its end and ends stopped all the same
    assert_eq!(report.stacks, 2);
    assert_eq!(report.added, 2);
    assert_eq!(report.stopped_after, Some(2), "{report:?}");
    assert_eq!(files_under(out.path()).len(), 2);
    let j = the_release_job(&mut reg);
    assert_eq!(j.state, State::Cancelled);
    assert_eq!(j.error.as_deref(), Some(run::STOPPED));
    let (error, finished) = row_of(&mut reg, report.release_id);
    assert_eq!(
        error.as_deref(),
        Some("stopped after 2 of 2 stacks; what was written stays written")
    );
    assert_eq!(finished, None);
    assert_eq!(lock::read(out.path()), None);
    // the next run finishes the version with nothing left to write
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(again.version, report.version);
    assert_eq!(
        (again.unchanged, again.added, again.stopped_after),
        (2, 0, None)
    );
    assert_eq!(finished_versions(&mut reg), vec![report.version]);
}

/// A stop from outside the registry, as the command line's Ctrl-C, is read
/// at the top of the next stack: the stack in hand is whole, no further one
/// is begun, the row is stopped, the job cancelled and the lock gone.
#[test]
fn a_stop_from_the_keyboard_stops_the_release_at_the_next_stack() {
    let source = tree(3);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let asked = std::sync::atomic::AtomicBool::new(false);
    let stop = || asked.load(std::sync::atomic::Ordering::SeqCst);
    let report = run::run_with(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        Some(&stop),
        &mut |_, _, done| {
            if done == 1 {
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        },
    )
    .unwrap();
    assert_eq!(report.stopped_after, Some(1), "{report:?}");
    assert_eq!(report.added, 1);
    assert_eq!(files_under(out.path()).len(), 1);
    let j = the_release_job(&mut reg);
    assert_eq!(j.state, State::Cancelled);
    assert_eq!(j.error.as_deref(), Some(run::STOPPED));
    let (error, finished) = row_of(&mut reg, report.release_id);
    assert_eq!(
        error.as_deref(),
        Some("stopped after 1 of 3 stacks; what was written stays written")
    );
    assert_eq!(finished, None);
    assert_eq!(lock::read(out.path()), None);
}

/// A stop in before the compare stops there: nothing is compared, removed
/// or written, and the row says it stopped after none of its stacks.
#[test]
fn a_stop_in_before_the_compare_stops_there() {
    let source = tree(2);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let stop = || true;
    let report = run::run_with(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        Some(&stop),
        &mut |_, _, _| panic!("no stack is handled"),
    )
    .unwrap();
    assert_eq!(report.stopped_after, Some(0), "{report:?}");
    assert_eq!(report.stacks, 2);
    assert_eq!((report.added, report.removed, report.written), (0, 0, 0));
    assert!(files_under(out.path()).is_empty());
    assert_eq!(the_release_job(&mut reg).state, State::Cancelled);
    let (error, finished) = row_of(&mut reg, report.release_id);
    assert_eq!(
        error.as_deref(),
        Some("stopped after 0 of 2 stacks; what was written stays written")
    );
    assert_eq!(finished, None);
    assert_eq!(lock::read(out.path()), None);
}

/// (c) A lock in the root naming a job the registry calls running refuses
/// the release in words that name the job and its host, and nothing is
/// written.
#[test]
fn a_lock_of_a_running_release_refuses_a_second_writer_and_names_it() {
    let source = tree(2);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    // another release, running: claimed by this process, so the registry
    // sees it alive, as a node of a cluster sees a fresh heartbeat
    let other = job::claim(
        reg.store(),
        &Claim {
            kind: "release",
            name: "other",
            args: serde_json::json!({}),
        },
    )
    .unwrap();
    let registry_id = reg.meta().registry_id.clone();
    lock::take(
        out.path(),
        &lock::Lock {
            job: other,
            registry: Some(registry_id),
            host: "a-node".to_string(),
            pid: 1,
            started_at: nils_registry::time::now_iso(),
            beat_at: None,
        },
    )
    .unwrap();

    let refused = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap_err();
    let words = refused.to_string();
    assert!(matches!(refused, run::Error::Refused(_)), "{words}");
    assert!(
        words.contains(&format!(
            "another release (job {other} on {})",
            job::hostname()
        )),
        "{words}"
    );
    assert!(
        words.contains("wait for it, or cancel it and run again"),
        "{words}"
    );
    assert!(files_under(out.path()).is_empty(), "nothing written");
    // the lock is the other release's still
    assert_eq!(lock::read(out.path()).map(|l| l.job), Some(other));
    // and no job of this release was made: the other is the only one
    assert_eq!(the_release_job(&mut reg).id, other);
}

/// (d) A lock naming a job that is over is taken over: the release runs,
/// the report says so in one line, and (e) the lock is gone after.
#[test]
fn a_lock_of_a_finished_release_is_taken_over_and_the_report_says_so() {
    let source = tree(2);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let other = job::claim(
        reg.store(),
        &Claim {
            kind: "release",
            name: "other",
            args: serde_json::json!({}),
        },
    )
    .unwrap();
    job::finish(reg.store(), other, State::Done, None).unwrap();
    // fresh by its stamp; the registry's word that the job is over wins
    lock::take(
        out.path(),
        &lock::Lock {
            job: other,
            registry: None,
            host: "a-node".to_string(),
            pid: 1,
            started_at: nils_registry::time::now_iso(),
            beat_at: Some(nils_registry::time::now_iso()),
        },
    )
    .unwrap();

    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let line = report.lock_taken_over.clone().expect("the report says so");
    assert!(line.contains(&format!("job {other}")), "{line}");
    assert!(line.contains("which was done"), "{line}");
    assert_eq!(report.added, 2);
    assert_eq!(files_under(out.path()).len(), 2);
    assert_eq!(lock::read(out.path()), None);
    // the report of a release that took nothing over has no such line
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(again.lock_taken_over, None);
    let doc = serde_json::to_value(&again).unwrap();
    assert!(doc.get("lock_taken_over").is_none() && doc.get("stopped_after").is_none());
}

/// A lock whose job the registry does not know, as one left in a copied
/// tree, blocks nothing.
#[test]
fn a_lock_naming_an_unknown_job_blocks_nothing() {
    let source = tree(1);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let registry_id = reg.meta().registry_id.clone();
    lock::take(
        out.path(),
        &lock::Lock {
            job: 4242,
            registry: Some(registry_id),
            host: "a-node".to_string(),
            pid: 1,
            started_at: nils_registry::time::now_iso(),
            beat_at: Some(nils_registry::time::now_iso()),
        },
    )
    .unwrap();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let line = report.lock_taken_over.unwrap();
    assert!(
        line.contains("job 4242") && line.contains("which was unknown"),
        "{line}"
    );
    assert_eq!(files_under(out.path()).len(), 1);
    assert_eq!(lock::read(out.path()), None);
}

/// A lock of another registry cannot be asked about: it refuses while its
/// beat is fresh, in words that name the job, its host and its registry,
/// and is taken over once its beat is older than the freshness window.
#[test]
fn another_registrys_lock_refuses_while_fresh_and_is_taken_over_once_stale() {
    let source = tree(1);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let theirs = lock::Lock {
        job: 77,
        registry: Some("some-other-registry".to_string()),
        host: "their-node".to_string(),
        pid: 1,
        started_at: an_hour_ago(),
        beat_at: Some(nils_registry::time::now_iso()),
    };
    lock::take(out.path(), &theirs).unwrap();

    let refused = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap_err();
    let words = refused.to_string();
    assert!(matches!(refused, run::Error::Refused(_)), "{words}");
    assert!(
        words.contains("another release (job 77 on their-node, of registry some-other-registry)"),
        "{words}"
    );
    assert!(files_under(out.path()).is_empty(), "nothing written");
    assert_eq!(lock::read(out.path()), Some(theirs.clone()));
    assert!(
        job::list(reg.store(), true, 100)
            .unwrap()
            .iter()
            .all(|j| j.kind != "release")
    );

    // the same lock, its beat an hour old: taken over
    lock::take(
        out.path(),
        &lock::Lock {
            beat_at: Some(an_hour_ago()),
            ..theirs
        },
    )
    .unwrap();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let line = report.lock_taken_over.unwrap();
    assert!(
        line.contains("job 77 on their-node") && line.contains("another registry's, silent since"),
        "{line}"
    );
    assert_eq!(files_under(out.path()).len(), 1);
    assert_eq!(lock::read(out.path()), None);
}

/// A lock file that cannot be read refuses, naming the file, until it has
/// gone untouched for longer than the freshness window; then it is taken
/// over and the report says so.
#[test]
fn an_unreadable_lock_refuses_until_it_is_old() {
    let source = tree(1);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let file = lock::path(out.path());
    std::fs::write(&file, "{ not json").unwrap();

    let refused = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap_err();
    let words = refused.to_string();
    assert!(matches!(refused, run::Error::Refused(_)), "{words}");
    assert!(
        words.contains("holds a release lock that cannot be read")
            && words.contains(&file.display().to_string())
            && words.contains("remove it if no release is writing here"),
        "{words}"
    );
    assert!(files_under(out.path()).is_empty(), "nothing written");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");

    // untouched for an hour: taken over
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let line = report.lock_taken_over.unwrap();
    assert!(
        line.contains("cannot be read") && line.contains("untouched for"),
        "{line}"
    );
    assert_eq!(files_under(out.path()).len(), 1);
    assert_eq!(lock::read(out.path()), None);
}

/// A registry that cannot be asked about the lock's job is a store failure,
/// not a refusal: the words of a refusal would send the person to wait for
/// a release nobody can see.
#[test]
fn a_store_failure_while_asking_about_the_lock_stays_a_store_failure() {
    let source = tree(1);
    let home_dir = TempDir::new("one-writer-home");
    let out = TempDir::new("one-writer-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let registry_id = reg.meta().registry_id.clone();
    lock::take(
        out.path(),
        &lock::Lock {
            job: 1,
            registry: Some(registry_id.clone()),
            host: "a-node".to_string(),
            pid: 1,
            started_at: an_hour_ago(),
            beat_at: Some(an_hour_ago()),
        },
    )
    .unwrap();
    let sql = format!("DROP TABLE {}", reg.store().qualified("job"));
    reg.store().execute(&sql, &[]).unwrap();
    let found = lock::check(reg.store(), &registry_id, out.path());
    assert!(matches!(found, Err(Refusal::Store(_))), "{found:?}");
}
