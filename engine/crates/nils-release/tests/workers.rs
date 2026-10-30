// SPDX-License-Identifier: AGPL-3.0-only

//! A release writes several stacks at once (Wave 7b, 3.x6): a pool of
//! workers converts the stacks, the longest first, while the registry's
//! writes stay on the release's own thread. What is proved here: one worker
//! and four write the same tree, byte for byte, and the same rows, on both
//! backends and in both layouts; a stop with four workers lets the stacks in
//! hand finish, begins no other, and the report says so; an abort gives up
//! every stack in hand; and the seconds of each phase are in the report,
//! stack by stack and summed, and on the release's row.
//!
//! A test binary of its own: it shortens the freshness window through
//! [`job::FRESH_VAR`], so the heart beats often while the stand-in converter
//! sleeps, and the variable is set before any other thread of this binary
//! could read it.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::job;
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::bids::convert::Converter;
use nils_release::policy::Policy;
use nils_release::run::{self, Asked, Layout, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a workers test key of some length";

/// One session of one person: a T1-weighted stack per entry, measured
/// again at a later time each, with that many slices, so the stacks are
/// told apart by `run-` and cost what their slices say.
fn tree(slices: &[u32]) -> TempDir {
    let dir = TempDir::new("workers");
    for (n, count) in slices.iter().enumerate() {
        let n = n + 1;
        for slice in 1..=*count {
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr("1.2.3", &format!("1.2.3.{n}"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "P1"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, &format!("03{:02}15", 10 + n)),
                synth::text(
                    tags::ACQUISITION_TIME,
                    VR::TM,
                    &format!("03{:02}15", 10 + n),
                ),
                synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage_sag"),
                synth::text(tags::PROTOCOL_NAME, VR::LO, "MPRAGE"),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                synth::us(tags::ROWS, 16),
                synth::us(tags::COLUMNS, 16),
                synth::us(tags::BITS_ALLOCATED, 16),
                synth::us(tags::BITS_STORED, 12),
                synth::us(tags::HIGH_BIT, 11),
                synth::us(tags::PIXEL_REPRESENTATION, 0),
                synth::us(tags::SAMPLES_PER_PIXEL, 1),
                synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                synth::text(tags::PIXEL_SPACING, VR::DS, "1.0\\1.0"),
                synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
                synth::text(
                    tags::IMAGE_POSITION_PATIENT,
                    VR::DS,
                    &format!("0\\0\\{slice}"),
                ),
                synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                synth::bytes(
                    tags::PIXEL_DATA,
                    VR::OW,
                    vec![(n as u8).wrapping_mul(16); 16 * 16 * 2],
                ),
            ]);
            dir.file(
                &format!("{n}/{slice}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    dir
}

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| nils_pack::load(&common::shipped_mri(), None).expect("the MRI pack loads"))
}

/// The DSN, or nothing and a word about why.
fn postgres_dsn() -> Option<String> {
    match std::env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

/// The backends to prove against, each with the schema a test owns there.
fn backends(schema: &str) -> Vec<(Backend, Option<String>)> {
    let mut out = vec![(Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let mut store = nils_registry::Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop the test schemas");
        out.push((Backend::Postgres, Some(dsn)));
    }
    out
}

/// A registry over `source`, digested and sorted, so a BIDS release can
/// name its stacks.
fn registry(
    home_dir: &TempDir,
    source: &TempDir,
    backend: Backend,
    dsn: Option<String>,
    schema: &str,
) -> (Home, Registry) {
    let home = Home::new(home_dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend,
        dsn,
        schema: (backend == Backend::Postgres).then(|| schema.to_string()),
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
    nils_classify::job::fingerprint(
        &mut reg,
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();
    nils_classify::classify::classify(
        &mut reg,
        pack(),
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();
    (home, reg)
}

#[allow(clippy::too_many_arguments)]
fn settings<'a>(
    name: &'a str,
    out: &'a Path,
    policy: &'a Policy,
    scheme: &'a SessionScheme,
    layout: Layout,
    converter: Option<&'a Converter>,
    workers: usize,
) -> run::Settings<'a> {
    run::Settings {
        name,
        root: out,
        policy,
        policy_from: nils_release::policy::Source::Flags,
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme,
        private: &pack().release,
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout,
        naming: nils_release::name::Naming::Full,
        places: nils_release::bids::place::Options::default(),
        converter,
        compress: true,
        observations: &[],
        authors: &[],
        workers,
    }
}

/// Every file under the root by its path below it, with its bytes; the lock
/// left out.
fn tree_of(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .is_some_and(|n| n != ".nils-release-lock.json")
            {
                let rel = p.strip_prefix(root).unwrap().display().to_string();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Every directory under the root, by its path below it.
fn dirs_of(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
                stack.push(p);
            }
        }
    }
    out.sort();
    out
}

/// A release's state rows and its history rows, as text, ids of its own
/// left out: the state in stack order, the history in the order written.
fn rows_of(reg: &mut Registry, release_id: i64) -> (Vec<String>, Vec<String>) {
    let store = reg.store();
    let state = store
        .query(
            &format!(
                "SELECT stack_id, content, dir, stem, route, files, bytes, digest, extensions \
                 FROM {} WHERE release_id = {release_id} ORDER BY stack_id",
                store.qualified("release_stack")
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{} {} {} {:?} {} {} {} {} {:?}",
                r.int(0).unwrap(),
                r.text(1).unwrap(),
                r.text(2).unwrap(),
                r.opt_text(3).unwrap(),
                r.text(4).unwrap(),
                r.int(5).unwrap(),
                r.int(6).unwrap(),
                r.text(7).unwrap(),
                r.opt_text(8).unwrap(),
            )
        })
        .collect();
    let moves = store
        .query(
            &format!(
                "SELECT stack_id, action, was, now FROM {} WHERE release_id = {release_id} ORDER BY id",
                store.qualified("release_move")
            ),
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{} {} {:?} {:?}",
                r.int(0).unwrap(),
                r.text(1).unwrap(),
                r.opt_text(2).unwrap(),
                r.opt_text(3).unwrap()
            )
        })
        .collect();
    (state, moves)
}

/// The phases a release's row keeps.
fn row_phases(reg: &mut Registry, release_id: i64) -> Option<run::Phases> {
    let store = reg.store();
    let r = store
        .query_opt(
            &format!(
                "SELECT phases FROM {} WHERE id = {release_id}",
                store.qualified("release")
            ),
            &[],
        )
        .unwrap()
        .unwrap();
    r.opt_text(0)
        .unwrap()
        .map(|t| serde_json::from_str(t).unwrap())
}

/// The seconds in the report are there, never negative, one entry per
/// stack it wrote or tried, in stack order, and summed in its head.
fn phases_hold(report: &run::Report, stacks_written: usize) {
    assert_eq!(report.stack_phases.len(), stacks_written, "{report:?}");
    let mut sum = run::Phases::default();
    let mut last = i64::MIN;
    for p in &report.stack_phases {
        assert!(p.stack > last, "in stack order: {:?}", report.stack_phases);
        last = p.stack;
        for s in [
            p.seconds.staging,
            p.seconds.conversion,
            p.seconds.digest,
            p.seconds.sidecar,
        ] {
            assert!(s >= 0.0, "{p:?}");
        }
        sum.staging += p.seconds.staging;
        sum.conversion += p.seconds.conversion;
        sum.digest += p.seconds.digest;
        sum.sidecar += p.seconds.sidecar;
    }
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(close(sum.staging, report.phases.staging), "{report:?}");
    assert!(
        close(sum.conversion, report.phases.conversion),
        "{report:?}"
    );
    assert!(close(sum.digest, report.phases.digest), "{report:?}");
    assert!(close(sum.sidecar, report.phases.sidecar), "{report:?}");
}

/// A stand-in converter that writes one slice's worth of NIfTI and a
/// sidecar after `pause` seconds, and notes each conversion it begins in
/// `begun` beside itself.
fn stand_in(dir: &Path, pause: &str) -> (Converter, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let fake = dir.join("dcm2niix");
    let begun = dir.join("begun");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo \"Chris Rorden's dcm2niiX version v1.0.20260724 (stand-in)\"; exit 0; fi\n\
             echo x >> '{begun}'\n\
             stem=; dest=\n\
             while [ $# -gt 1 ]; do case \"$1\" in -f) stem=$2; shift;; -o) dest=$2; shift;; esac; shift; done\n\
             sleep {pause}\n\
             echo nifti > \"$dest/$stem.nii.gz\"; echo '{{\"EchoTime\": 0.003, \"RepetitionTime\": 2.3}}' > \"$dest/$stem.json\"\n\
             echo \"Convert 1 DICOM as $dest/$stem (16x16x1x1)\"\n",
            begun = begun.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    (Converter::find(&fake).unwrap(), begun)
}

fn begun(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

/// The NIfTI images under a root.
fn images(root: &Path) -> usize {
    tree_of(root)
        .keys()
        .filter(|p| p.ends_with(".nii.gz") && !p.starts_with("sourcedata"))
        .count()
}

/// One worker and four write the same tree and the same rows, with the
/// dcm2niix found on the search path (else the BIDS half is skipped) and in
/// the descriptive layout, on both backends; the seconds are in the report
/// and on the row; and four workers take the longest stack first.
#[test]
#[allow(unsafe_code)]
fn one_worker_and_four_write_the_same_tree_and_the_same_rows() {
    // SAFETY: the window is set before any other thread of this binary
    // reads it: every test of this binary sets the same value first
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    let real = Converter::find(Path::new("dcm2niix")).ok();
    if real.is_none() {
        eprintln!("dcm2niix is not installed; the BIDS half is skipped");
    }
    let slices = [2, 9, 3, 6, 4, 5];
    for (backend, dsn) in backends("nils_workers_same") {
        let source = tree(&slices);
        let home_dir = TempDir::new("workers-home");
        let (_home, mut reg) = registry(&home_dir, &source, backend, dsn, "nils_workers_same");
        let policy = Policy::default();
        let scheme = SessionScheme::default();
        let mut layouts = vec![(Layout::Descriptive, None)];
        if let Some(c) = &real {
            layouts.push((Layout::Bids, Some(c)));
        }
        for (layout, converter) in layouts {
            let tag = format!("{backend:?} {layout:?}");
            let one_out = TempDir::new("workers-one");
            let four_out = TempDir::new("workers-four");
            let one = run::run(
                &mut reg,
                &settings(
                    &format!("w1x {}", layout.name()),
                    one_out.path(),
                    &policy,
                    &scheme,
                    layout,
                    converter,
                    1,
                ),
            )
            .unwrap();
            let four = run::run(
                &mut reg,
                &settings(
                    &format!("w4x {}", layout.name()),
                    four_out.path(),
                    &policy,
                    &scheme,
                    layout,
                    converter,
                    4,
                ),
            )
            .unwrap();
            assert_eq!((one.workers, four.workers), (1, 4), "{tag}");
            assert_eq!(one.stacks, slices.len() as i64, "{tag}");
            assert_eq!(one.stopped_after, None, "{tag}");
            assert_eq!(four.stopped_after, None, "{tag}");
            // the same counts
            for (a, b, what) in [
                (one.files, four.files, "files"),
                (one.bytes, four.bytes, "bytes"),
                (one.written, four.written, "written"),
                (one.added, four.added, "added"),
            ] {
                assert_eq!(a, b, "{tag}: {what}");
            }
            assert_eq!(one.refused, four.refused, "{tag}");
            assert_eq!(one.changes, four.changes, "{tag}");
            assert_eq!(one.routes, four.routes, "{tag}");
            assert_eq!(one.unconvertible, four.unconvertible, "{tag}");
            // the same tree, byte for byte, but for the files that name
            // the release itself
            let (mut a, mut b) = (tree_of(one_out.path()), tree_of(four_out.path()));
            assert!(a.len() > slices.len(), "{tag}: {:?}", a.keys());
            for named in ["dataset_description.json", "README"] {
                let (x, y) = (a.remove(named), b.remove(named));
                assert_eq!(x.is_some(), y.is_some(), "{tag}: {named}");
                if let (Some(x), Some(y)) = (x, y) {
                    let (x, y) = (String::from_utf8(x).unwrap(), String::from_utf8(y).unwrap());
                    assert_eq!(
                        x.replace("w1x ", "w4x "),
                        y,
                        "{tag}: {named} differs beyond the name"
                    );
                }
            }
            assert_eq!(
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>(),
                "{tag}"
            );
            for (path, bytes) in &a {
                assert!(&b[path] == bytes, "{tag}: {path} differs");
            }
            // no folder left behind by either, the staging folders included
            assert_eq!(dirs_of(one_out.path()), dirs_of(four_out.path()), "{tag}");
            // the same rows: the state and the history, in the same order
            assert_eq!(
                rows_of(&mut reg, one.release_id),
                rows_of(&mut reg, four.release_id),
                "{tag}"
            );
            // the seconds: in the report stack by stack and summed, and on
            // the row
            phases_hold(&one, slices.len());
            phases_hold(&four, slices.len());
            if layout == Layout::Bids {
                assert!(four.phases.conversion > 0.0, "{tag}: {:?}", four.phases);
            }
            assert_eq!(
                row_phases(&mut reg, four.release_id),
                Some(four.phases),
                "{tag}"
            );
            assert_eq!(
                row_phases(&mut reg, one.release_id),
                Some(one.phases),
                "{tag}"
            );
        }
    }
}

/// A stop with four workers, asked once four stacks are in hand: the four
/// finish and are written, no fifth is begun, the report says it stopped
/// after the four, the job ends cancelled, and the next run writes the rest.
#[test]
#[allow(unsafe_code)]
fn a_stop_with_four_workers_finishes_the_stacks_in_hand_and_begins_no_other() {
    // SAFETY: see the first test
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    let tools = TempDir::new("workers-tools");
    let (converter, log) = stand_in(tools.path(), "1");
    let source = tree(&[1, 1, 1, 1, 1, 1, 1, 1]);
    let home_dir = TempDir::new("workers-home");
    let (_home, mut reg) = registry(&home_dir, &source, Backend::Sqlite, None, "");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let out = TempDir::new("workers-out");
    let stop = || match begun(&log) >= 4 {
        true => Asked::Stop,
        false => Asked::Nothing,
    };
    let report = run::run_with(
        &mut reg,
        &settings(
            "stopped",
            out.path(),
            &policy,
            &scheme,
            Layout::Bids,
            Some(&converter),
            4,
        ),
        Some(&stop),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(report.stacks, 8);
    assert_eq!(report.stopped_after, Some(4), "{report:?}");
    assert_eq!(begun(&log), 4, "no stack was begun after the stop");
    assert_eq!(report.added, 4);
    assert_eq!(images(out.path()), 4);
    phases_hold(&report, 4);
    // a stopped release keeps its seconds on its row too
    assert_eq!(row_phases(&mut reg, report.release_id), Some(report.phases));
    let jobs = job::list(reg.store(), true, 100).unwrap();
    let j = jobs.iter().find(|j| j.kind == "release").unwrap();
    assert_eq!(j.state, job::State::Cancelled);
    assert_eq!(j.result.as_ref().unwrap()["stopped_after"], 4);

    // the next run goes on: the four written are left alone, the rest added
    let again = run::run(
        &mut reg,
        &settings(
            "stopped",
            out.path(),
            &policy,
            &scheme,
            Layout::Bids,
            Some(&converter),
            4,
        ),
    )
    .unwrap();
    assert_eq!(again.stopped_after, None);
    assert_eq!((again.unchanged, again.added), (4, 4), "{again:?}");
    assert_eq!(images(out.path()), 8);
    assert_eq!(begun(&log), 8);
}

/// A cancel on the job's row with four workers: read at a beat, it begins
/// no further stack, every stack begun is finished and written, and the
/// report says how many.
#[test]
#[allow(unsafe_code)]
fn a_cancel_with_four_workers_stops_with_every_begun_stack_written() {
    // SAFETY: see the first test
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    let tools = TempDir::new("workers-tools");
    let (converter, log) = stand_in(tools.path(), "1");
    let source = tree(&[1; 12]);
    let home_dir = TempDir::new("workers-home");
    let (_home, mut reg) = registry(&home_dir, &source, Backend::Sqlite, None, "");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let out = TempDir::new("workers-out");
    let report = run::run_watched(
        &mut reg,
        &settings(
            "cancelled",
            out.path(),
            &policy,
            &scheme,
            Layout::Bids,
            Some(&converter),
            4,
        ),
        &mut |store, job_id, done| {
            if done == 1 {
                job::request_cancel(store, job_id).unwrap();
            }
        },
    )
    .unwrap();
    let after = report.stopped_after.expect("it stopped");
    assert!(after < 12, "{report:?}");
    assert_eq!(begun(&log), after as usize, "every stack begun was written");
    assert_eq!(report.added, after);
    assert_eq!(images(out.path()), after as usize);
    let jobs = job::list(reg.store(), true, 100).unwrap();
    let j = jobs.iter().find(|j| j.kind == "release").unwrap();
    assert_eq!(j.state, job::State::Cancelled);
    assert_eq!(j.error.as_deref(), Some(run::STOPPED));
}

/// An abort with four workers gives up every stack in hand: their
/// converters are ended, nothing is recorded of them, and the next run
/// writes them.
#[test]
#[allow(unsafe_code)]
fn an_abort_with_four_workers_gives_up_every_stack_in_hand() {
    // SAFETY: see the first test
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    let tools = TempDir::new("workers-tools");
    let (converter, log) = stand_in(tools.path(), "30");
    let source = tree(&[1, 1, 1, 1, 1, 1]);
    let home_dir = TempDir::new("workers-home");
    let (_home, mut reg) = registry(&home_dir, &source, Backend::Sqlite, None, "");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let out = TempDir::new("workers-out");
    let stop = || match begun(&log) >= 4 {
        true => Asked::Abort,
        false => Asked::Nothing,
    };
    let started = std::time::Instant::now();
    let report = run::run_with(
        &mut reg,
        &settings(
            "aborted",
            out.path(),
            &policy,
            &scheme,
            Layout::Bids,
            Some(&converter),
            4,
        ),
        Some(&stop),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the converters were ended, not waited for"
    );
    assert_eq!(report.stopped_after, Some(0), "{report:?}");
    assert_eq!(report.added, 0);
    assert_eq!(begun(&log), 4);
    assert_eq!(images(out.path()), 0);
}

/// How long one worker and eight take over the same synthetic stacks with
/// the dcm2niix found on the search path: a measurement, not a check, so it
/// runs only when asked (`--ignored`) and prints the seconds and the phases.
#[test]
#[ignore]
#[allow(unsafe_code)]
fn measure_one_worker_against_eight() {
    // SAFETY: see the first test
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    let Ok(real) = Converter::find(Path::new("dcm2niix")) else {
        eprintln!("dcm2niix is not installed; nothing to measure");
        return;
    };
    // 24 stacks of 30 to 145 slices of 256 x 256, one of them much longer
    let slices: Vec<u32> = (0..24).map(|n| 30 + (n * 37) % 90).chain([300]).collect();
    let source = TempDir::new("workers-measure");
    let side = 256usize;
    for (n, count) in slices.iter().enumerate() {
        let n = n + 1;
        for slice in 1..=*count {
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr("1.2.3", &format!("1.2.3.{n}"), &sop);
            let mut pixels = vec![0u8; side * side * 2];
            let mut x = (n as u32).wrapping_mul(2654435761) ^ slice;
            for p in pixels.chunks_mut(2) {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                p[0] = (x & 0xff) as u8;
                p[1] = ((x >> 8) & 0x0f) as u8;
            }
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "P1"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, &format!("03{:02}15", 10 + n)),
                synth::text(
                    tags::ACQUISITION_TIME,
                    VR::TM,
                    &format!("03{:02}15", 10 + n),
                ),
                synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage_sag"),
                synth::text(tags::PROTOCOL_NAME, VR::LO, "MPRAGE"),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                synth::us(tags::ROWS, side as u16),
                synth::us(tags::COLUMNS, side as u16),
                synth::us(tags::BITS_ALLOCATED, 16),
                synth::us(tags::BITS_STORED, 12),
                synth::us(tags::HIGH_BIT, 11),
                synth::us(tags::PIXEL_REPRESENTATION, 0),
                synth::us(tags::SAMPLES_PER_PIXEL, 1),
                synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                synth::text(tags::PIXEL_SPACING, VR::DS, "1.0\\1.0"),
                synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
                synth::text(
                    tags::IMAGE_POSITION_PATIENT,
                    VR::DS,
                    &format!("0\\0\\{slice}"),
                ),
                synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                synth::bytes(tags::PIXEL_DATA, VR::OW, pixels),
            ]);
            source.file(
                &format!("{n}/{slice}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    let home_dir = TempDir::new("workers-home");
    let (_home, mut reg) = registry(&home_dir, &source, Backend::Sqlite, None, "");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    for workers in [1usize, 8] {
        let out = TempDir::new("workers-measure-out");
        let started = std::time::Instant::now();
        let r = run::run(
            &mut reg,
            &settings(
                &format!("measure {workers}"),
                out.path(),
                &policy,
                &scheme,
                Layout::Bids,
                Some(&real),
                workers,
            ),
        )
        .unwrap();
        eprintln!(
            "workers {workers}: {} stacks, {} files written, {:.1} s wall; phases {:?}",
            r.stacks,
            r.written,
            started.elapsed().as_secs_f64(),
            r.phases
        );
    }
}
