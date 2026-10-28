// SPDX-License-Identifier: AGPL-3.0-only

//! The release and its converter when the converter is not what the
//! preflight found (Wave 7b, the maintenance track). A converter that cannot
//! be started at all mid-run (gone, or not to be run) stops the release with
//! the error: the stacks done are recorded, nothing of the rest, the row is
//! left unfinished with the words and the job fails, and the next run goes
//! on under the same version. An abort while a stack rewritten in its place
//! is converting ends the converter and leaves the stack's old files, whole:
//! the new ones were staged beside the tree and are let go.
//!
//! A test binary of its own, as `heart.rs` is: it shortens the freshness
//! window through [`job::FRESH_VAR`], which the heart's tick interval
//! follows, so an abort reaches a converter in flight within a fraction of
//! a second; the variable is set before any other thread of this binary
//! could read it.

mod common;

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::job::{self, State};
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::bids::convert::Converter;
use nils_release::lock;
use nils_release::policy::{Policy, Uids};
use nils_release::run::{self, Asked, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a release test key of some length";

/// One session of one person, two T1w stacks of two slices each, as the
/// BIDS tests make one: described, fingerprinted and classified, so the
/// pack gives each a suffix and the converter is asked.
fn bids_tree() -> TempDir {
    let dir = TempDir::new("converter-bids");
    for n in 1..=2u32 {
        for slice in 1..=2u32 {
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr("1.2.3", &format!("1.2.3.{n}.0"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "P1"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, &format!("03141{n}")),
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
                synth::bytes(tags::PIXEL_DATA, VR::OW, vec![0x40u8; 16 * 16 * 2]),
            ]);
            dir.file(
                &format!("{n}/{slice}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
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

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| nils_pack::load(&common::shipped_mri(), None).expect("the MRI pack loads"))
}

/// A stand-in converter at `path`: answers its version, runs `before` (a
/// shell line) and writes the image and sidecar it was asked for, the
/// image's bytes its own process id, so two runs' files differ.
fn stand_in(path: &Path, before: &str) -> Converter {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(
        path,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo \"Chris Rorden's dcm2niiX version v1.0.20260724 (stand-in)\"; exit 0; fi\n\
             stem=; dest=\n\
             while [ $# -gt 1 ]; do case \"$1\" in -f) stem=$2; shift;; -o) dest=$2; shift;; esac; shift; done\n\
             {before}\n\
             echo \"$$\" > \"$dest/$stem.nii.gz\"; echo '{{\"EchoTime\": 0.1, \"RepetitionTime\": 3}}' > \"$dest/$stem.json\"\n\
             echo \"Convert 1 DICOM as $dest/$stem (16x16x1x1)\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    Converter::find(path).unwrap()
}

fn settings<'a>(
    out: &'a Path,
    policy: &'a Policy,
    scheme: &'a SessionScheme,
    converter: &'a Converter,
) -> run::Settings<'a> {
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
        layout: run::Layout::Bids,
        naming: nils_release::name::Naming::Full,
        places: nils_release::bids::place::Options::default(),
        converter: Some(converter),
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

/// The `error` and `finished_at` of a release's row, and how many stacks
/// its state holds.
fn row_of(reg: &mut Registry, release_id: i64) -> (Option<String>, Option<String>, i64) {
    let store = reg.store();
    let sql = format!(
        "SELECT error, finished_at, (SELECT COUNT(*) FROM {} WHERE release_id = {}) FROM {} \
         WHERE id = {}",
        store.qualified("release_stack"),
        release_id,
        store.qualified("release"),
        release_id
    );
    let r = store.query_opt(&sql, &[]).unwrap().unwrap();
    (
        r.opt_text(0).unwrap().map(str::to_string),
        r.opt_text(1).unwrap().map(str::to_string),
        r.int(2).unwrap(),
    )
}

/// The newest release job of the registry.
fn newest_release_job(reg: &mut Registry) -> job::Job {
    job::list(reg.store(), true, 100)
        .unwrap()
        .into_iter()
        .filter(|j| j.kind == "release")
        .max_by_key(|j| j.id)
        .expect("a release job")
}

#[test]
// the one place a test sets the environment in process: see the module
#[allow(unsafe_code)]
fn a_converter_that_is_gone_stops_the_release_and_an_abort_mid_rewrite_keeps_the_old_files() {
    // SAFETY: this is the binary's one test, on the harness's one thread
    // for it, and nothing else reads the environment meanwhile
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    assert_eq!(job::fresh_secs(), 1);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    // ---- a converter that is gone after the first stack stops the release
    let source = bids_tree();
    let home_dir = TempDir::new("converter-home");
    let out = TempDir::new("converter-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let fake = home_dir.path().join("dcm2niix");
    // converts the stack and then is gone: the next stack cannot start it
    let gone = stand_in(&fake, "rm -f \"$0\"");
    let refused = run::run(&mut reg, &settings(out.path(), &policy, &scheme, &gone)).unwrap_err();
    let words = refused.to_string();
    assert!(matches!(refused, run::Error::Refused(_)), "{words}");
    assert!(
        words.starts_with("dcm2niix could not be run (")
            && words.ends_with("): the release stopped after 1 of 2 stacks"),
        "{words}"
    );
    let j = newest_release_job(&mut reg);
    assert_eq!(j.state, State::Failed);
    assert_eq!(j.error.as_deref(), Some(words.as_str()));
    // the row: unfinished, the words, the one stack done recorded
    let release_id = {
        let store = reg.store();
        let sql = format!(
            "SELECT id FROM {} ORDER BY id DESC LIMIT 1",
            store.qualified("release")
        );
        store.query_opt(&sql, &[]).unwrap().unwrap().int(0).unwrap()
    };
    let (error, finished, stacks) = row_of(&mut reg, release_id);
    assert_eq!(error.as_deref(), Some(words.as_str()));
    assert_eq!(finished, None, "not a version");
    assert_eq!(stacks, 1, "the stack done is recorded, nothing else");
    let written = files_under(out.path());
    assert_eq!(
        written.len(),
        4,
        "one image, its sidecar and its two exported slices: {written:?}"
    );
    assert_eq!(lock::read(out.path()), None);

    // the next run, with the converter back, goes on under the same version
    let back = stand_in(&fake, "");
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme, &back)).unwrap();
    assert_eq!(again.stopped_after, None);
    assert_eq!((again.unchanged, again.added), (1, 1), "{again:?}");
    let (error, finished, stacks) = row_of(&mut reg, again.release_id);
    assert_eq!((error, stacks), (None, 2));
    assert!(finished.is_some());
    // the two stacks' eight files, and the dataset's own files with them
    assert!(files_under(out.path()).len() > 8);

    // ---- an abort while a stack rewritten in its place converts
    // another policy: every stack is rewritten in its place; the converter
    // takes two seconds, and the abort comes in once a stack is staged
    // beside the tree (the heart ticks every twelfth of a second here)
    let remapped = Policy {
        uids: Uids::Preserve,
        ..Policy::default()
    };
    let slow = stand_in(&fake, "sleep 2");
    let before: Vec<(std::path::PathBuf, Vec<u8>)> = files_under(out.path())
        .into_iter()
        .map(|p| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    let scratch = out.path().join(".nils-rewrite");
    let stop = || match scratch.exists() {
        true => Asked::Abort,
        false => Asked::Nothing,
    };
    let started = std::time::Instant::now();
    let report = run::run_with(
        &mut reg,
        &settings(out.path(), &remapped, &scheme, &slow),
        Some(&stop),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1800),
        "the converter was ended at a tick, not waited for"
    );
    assert_eq!(report.stopped_after, Some(0), "{report:?}");
    assert_eq!((report.rewritten, report.written), (0, 0), "{report:?}");
    // the tree is the version before's, whole, and nothing staged is left
    let after = files_under(out.path());
    assert_eq!(after.len(), before.len(), "{after:?}");
    for (path, bytes) in &before {
        assert_eq!(std::fs::read(path).unwrap(), *bytes, "{}", path.display());
    }
    assert!(!scratch.exists());
    assert_eq!(lock::read(out.path()), None);
    assert_eq!(newest_release_job(&mut reg).state, State::Cancelled);

    // and the run that goes on rewrites both, in their places, new bytes
    let again = run::run(&mut reg, &settings(out.path(), &remapped, &scheme, &back)).unwrap();
    assert_eq!(again.version, report.version);
    assert_eq!((again.rewritten, again.written), (2, 8), "{again:?}");
    let now = files_under(out.path());
    assert_eq!(
        now,
        before.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
        "the same paths"
    );
    let images = before
        .iter()
        .filter(|(p, _)| p.extension().is_some_and(|e| e == "gz"));
    for (path, bytes) in images {
        assert_ne!(std::fs::read(path).unwrap(), *bytes, "{}", path.display());
    }
    assert!(!scratch.exists());
}
