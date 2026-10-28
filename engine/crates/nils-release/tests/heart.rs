// SPDX-License-Identifier: AGPL-3.0-only

//! The release's heart beats inside one long stack (Wave 7b, the
//! maintenance track): a Descriptive-layout stack of many instances on slow
//! storage took minutes between two beats before, and another node's claim
//! took the release for stale after one. Now the heart ticks per file
//! written, so the job's row moves while a stack is in hand.
//!
//! A test binary of its own: it shortens the freshness window through
//! [`job::FRESH_VAR`], which the heart's interval follows, and the variable
//! is set before any other thread of this binary could read it.

mod common;

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::job;
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::policy::Policy;
use nils_release::run::{self, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a release test key of some length";

/// One study of one person, one series per entry with that many instances.
fn tree(instances: &[u32]) -> TempDir {
    let dir = TempDir::new("heart");
    for (n, count) in instances.iter().enumerate() {
        let n = n as u32 + 1;
        let series = format!("A.1.{n}");
        for i in 1..=*count {
            let sop = format!("A.1.{n}.{i}");
            let mut e = synth::minimal_mr("A.1", &series, &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "P1"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_NUMBER, VR::IS, &n.to_string()),
                synth::text(tags::INSTANCE_NUMBER, VR::IS, &i.to_string()),
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
                &format!("s{n}/{i}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    dir
}

/// One session of one person, two T1w stacks of two slices each, as the
/// BIDS tests make one: described, fingerprinted and classified, so the
/// pack gives each a suffix and the converter is asked.
fn bids_tree() -> TempDir {
    let dir = TempDir::new("heart-bids");
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
    (home, reg)
}

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| nils_pack::load(&common::shipped_mri(), None).expect("the MRI pack loads"))
}

/// A stand-in converter that answers its version, sleeps two seconds and
/// writes the image and sidecar it was asked for: a conversion long enough
/// for the heart to tick inside it under a one-second window.
fn slow_converter(dir: &Path) -> nils_release::bids::convert::Converter {
    use std::os::unix::fs::PermissionsExt as _;
    let fake = dir.join("dcm2niix");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo \"Chris Rorden's dcm2niiX version v1.0.20260724 (stand-in)\"; exit 0; fi\n\
         stem=; dest=\n\
         while [ $# -gt 1 ]; do case \"$1\" in -f) stem=$2; shift;; -o) dest=$2; shift;; esac; shift; done\n\
         sleep 2\n\
         : > \"$dest/$stem.nii.gz\"; echo '{\"EchoTime\": 0.1, \"RepetitionTime\": 3}' > \"$dest/$stem.json\"\n\
         echo \"Convert 1 DICOM as $dest/$stem (16x16x1x1)\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    nils_release::bids::convert::Converter::find(&fake).unwrap()
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

/// A Descriptive-layout release of a small stack and then a long one: the
/// heart beats more than once while the long stack is written, and the
/// folder's lock moves with it.
#[test]
// the one place a test sets the environment in process: see the module
#[allow(unsafe_code)]
fn the_heart_beats_inside_one_long_stack() {
    // the freshness window is one second, so the heart's interval is a
    // twelfth of it; set before any other thread of this binary runs
    // SAFETY: this is the binary's one test, on the harness's one thread
    // for it, and nothing else reads the environment meanwhile
    unsafe { std::env::set_var(job::FRESH_VAR, "1") };
    assert_eq!(job::fresh_secs(), 1);

    let source = tree(&[1, 1500]);
    let home_dir = TempDir::new("heart-home");
    let out = TempDir::new("heart-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let mut beats: Vec<(i64, i64, String)> = Vec::new();
    let mut lock_beats: Vec<Option<String>> = Vec::new();
    let root = out.path().to_path_buf();
    let report = run::run_watched(
        &mut reg,
        &settings(out.path(), &policy, &scheme),
        &mut |store, job_id, done| {
            let j = job::show(store, job_id).unwrap().unwrap();
            let progress = j.progress.unwrap();
            beats.push((
                done,
                progress["beats"].as_i64().unwrap(),
                j.heartbeat_at.unwrap(),
            ));
            lock_beats.push(nils_release::lock::read(&root).and_then(|l| l.beat_at));
            if done == 1 {
                // the stamps are whole seconds: let one pass, so the long
                // stack's beats are later than this one
                std::thread::sleep(std::time::Duration::from_millis(1100));
            }
        },
    )
    .unwrap();
    assert_eq!(report.stacks, 2);
    assert_eq!(report.stopped_after, None);
    assert_eq!(report.written, 1501);
    assert_eq!(beats.len(), 2, "{beats:?}");
    // between the beat after the small stack and the beat after the long
    // one, the heart beat inside the long stack at least once more
    let inside = beats[1].1 - beats[0].1;
    assert!(
        inside >= 2,
        "the heart beat {inside} time(s) for the long stack, the one at its end included: \
         {beats:?}"
    );
    assert!(beats[1].2 > beats[0].2, "the heartbeat moved: {beats:?}");
    // and the lock moved with it
    assert!(
        lock_beats[1] > lock_beats[0],
        "the lock's beat moved: {lock_beats:?}"
    );
    assert_eq!(nils_release::lock::read(out.path()), None);

    // and a conversion ticks at the heart's interval too: a BIDS release of
    // two stacks through a converter that takes two seconds each beats
    // several times inside the second one
    let source = bids_tree();
    let home_dir = TempDir::new("heart-home-bids");
    let out = TempDir::new("heart-out-bids");
    let (_home, mut reg) = registry(&home_dir, &source);
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
    let converter = slow_converter(home_dir.path());
    let mut settings = settings(out.path(), &policy, &scheme);
    settings.layout = run::Layout::Bids;
    settings.converter = Some(&converter);
    let mut beats: Vec<(i64, i64)> = Vec::new();
    let report = run::run_watched(&mut reg, &settings, &mut |store, job_id, done| {
        let j = job::show(store, job_id).unwrap().unwrap();
        beats.push((done, j.progress.unwrap()["beats"].as_i64().unwrap()));
    })
    .unwrap();
    assert_eq!(report.stopped_after, None);
    assert_eq!(report.stacks, 2, "{report:?}");
    assert_eq!(beats.len(), 2, "{beats:?} {report:?}");
    let inside = beats[1].1 - beats[0].1;
    assert!(
        inside >= 5,
        "the heart beat {inside} time(s) through a two-second conversion: {beats:?} {report:?}"
    );
    assert_eq!(nils_release::lock::read(out.path()), None);
}
