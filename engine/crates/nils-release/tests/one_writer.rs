// SPDX-License-Identifier: AGPL-3.0-only

//! One writer per tree, and a release that beats and stops (Wave 7b, the
//! maintenance track). A release never beat its job's heart before, so a
//! second release started by mistake into the same folder found the first
//! stale after a minute on another node, and a cancel was not seen until the
//! release ended. Now the job's row moves with every stack, a cancel stops
//! the release after the stack in hand, and the folder's own lock refuses a
//! second writer on any host.

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
use nils_release::lock;
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
/// stopped, the lock is gone; and the next run writes the rest.
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

    // the row of the stopped version says so too
    let (error, finished): (Option<String>, Option<String>) = {
        let store = reg.store();
        let sql = format!(
            "SELECT error, finished_at FROM {} WHERE id = {}",
            store.qualified("release"),
            report.release_id
        );
        let r = store.query_opt(&sql, &[]).unwrap().unwrap();
        (
            r.opt_text(0).unwrap().map(str::to_string),
            r.opt_text(1).unwrap().map(str::to_string),
        )
    };
    assert_eq!(
        error.as_deref(),
        Some("stopped after 2 of 4 stacks; what was written stays written")
    );
    assert!(finished.is_some());

    // run again to go on: the two left are added, the two written are left alone
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(again.stopped_after, None);
    assert_eq!((again.unchanged, again.added), (2, 2), "{again:?}");
    assert_eq!(files_under(out.path()).len(), 4);
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
    lock::take(
        out.path(),
        &lock::Lock {
            job: other,
            registry: None,
            host: "a-node".to_string(),
            pid: 1,
            started_at: nils_registry::time::now_iso(),
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
    lock::take(
        out.path(),
        &lock::Lock {
            job: 4242,
            registry: Some("some-other-registry".to_string()),
            host: "a-node".to_string(),
            pid: 1,
            started_at: nils_registry::time::now_iso(),
        },
    )
    .unwrap();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let line = report.lock_taken_over.unwrap();
    assert!(
        line.contains("job 4242") && line.contains("another registry's"),
        "{line}"
    );
    assert_eq!(files_under(out.path()).len(), 1);
    assert_eq!(lock::read(out.path()), None);
}
