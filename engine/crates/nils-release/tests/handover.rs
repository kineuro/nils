// SPDX-License-Identifier: AGPL-3.0-only

//! The handover from end to end
//! (`docs/specs/wave3-anonymize-and-bids.md`, §11).
//!
//! It needs `7z`, which is a prerequisite of a deployment rather than of a
//! checkout, so the tests say so and stop when it is absent.

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::store::Param;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::handover::archive::Archiver;
use nils_release::handover::plan::Strategy;
use nils_release::handover::run as handover;
use nils_release::policy::Policy;
use nils_release::run::{self, Layout, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a handover test key of a length";

/// Two people, one session each.
fn tree() -> TempDir {
    let dir = TempDir::new("handover");
    for (person, study) in [("a", "1"), ("b", "2")] {
        for slice in 1..=2 {
            let sop = format!("1.2.3.{study}.{slice}");
            let mut e =
                synth::minimal_mr(&format!("1.2.3.{study}"), &format!("1.2.3.{study}.0"), &sop);
            e.extend([
                synth::text(
                    tags::PATIENT_ID,
                    VR::LO,
                    &format!("1980010{study}-123{study}"),
                ),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage sag"),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
            ]);
            let _ = person;
            dir.file(
                &format!("{study}/{slice}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    dir
}

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    })
}

fn archiver() -> Option<Archiver> {
    match Archiver::find(Path::new("7z")) {
        Ok(a) => Some(a),
        Err(_) => {
            eprintln!("7z is not installed; the handover tests are skipped");
            None
        }
    }
}

/// A registry with one release written into `out`.
fn released(home_dir: &TempDir, source: &TempDir, out: &Path) -> (Home, Registry) {
    let home = Home::new(home_dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend: Backend::Sqlite,
        dsn: None,
        schema: None,
        scheme: Scheme::DEFAULT,
        key: "k".to_string(),
        display_length: 12,
        session_scheme: None,
    })
    .unwrap();
    let mut reg = home.open().unwrap();
    let mut s = nils_digest::Settings::new(source.path());
    s.name = "t".into();
    digest(&s, &mut reg).unwrap();

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let settings = run::Settings {
        name: "a cohort",
        root: out,
        policy: &policy,
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme: &scheme,
        private: &[],
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout: Layout::Descriptive,
        places: nils_release::bids::place::Options::default(),
        converter: None,
        compress: true,
        authors: &[],
    };
    let report = run::run(&mut reg, &settings).unwrap();
    assert!(report.files > 0, "the release wrote something to hand over");
    (home, reg)
}

fn settings<'a>(
    out: &'a Path,
    archiver: &'a Archiver,
    password: &'a str,
    cap: i64,
    strategy: Strategy,
) -> handover::Settings<'a> {
    handover::Settings {
        release: "a cohort",
        out,
        archiver,
        key_name: "k",
        password,
        cap,
        strategy,
        level: 1,
        par2: None,
        verify: true,
        actor: "a test",
    }
}

#[test]
fn a_handover_packs_the_release_and_reads_it_back() {
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());

    let password = nils_release::handover::password(KEY);
    let report = handover::run(
        &mut reg,
        &settings(
            ship.path(),
            &archiver,
            &password,
            1 << 30,
            Strategy::Ordered,
        ),
    )
    .unwrap();
    assert_eq!(report.archives, 1);
    assert_eq!(report.verified, 1, "read back before it is called done");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.missing, 0);
    assert!(report.packed_bytes > 0);

    // The set says what it is, because a recipient has the archives and not
    // the registry.
    assert!(ship.path().join("handover.tsv").is_file());
    assert!(ship.path().join("HANDOVER").is_file());
    let manifest = std::fs::read_to_string(ship.path().join("handover.tsv")).unwrap();
    assert!(
        manifest.starts_with("archive\tdigest\tbytes\tfiles\tsubjects\n"),
        "{manifest}"
    );
}

#[test]
fn the_archive_set_is_part_of_the_release_record() {
    // §11: "what did we send them, and is it still intact" is a query rather
    // than a folder somebody remembers.
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());
    let password = nils_release::handover::password(KEY);
    let report = handover::run(
        &mut reg,
        &settings(
            ship.path(),
            &archiver,
            &password,
            1 << 30,
            Strategy::Ordered,
        ),
    )
    .unwrap();

    let store = reg.store();
    let rows = store
        .query(
            &format!(
                "SELECT name, digest, files, subjects FROM {} WHERE handover_id = {}",
                store.qualified("handover_archive"),
                store.dialect().param(1, nils_registry::schema::Type::Int),
            ),
            &[Param::Int(report.handover_id)],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text(1).unwrap().len(), 64, "a checksum per archive");
    assert_eq!(rows[0].int(3).unwrap(), 2, "and the people in it");

    // Which people, by the subject the registry knows and not by a string
    // somebody read off a path.
    let store = reg.store();
    let people = store
        .query(
            &format!(
                "SELECT COUNT(*) FROM {} s JOIN {} k ON k.id = s.subject_id",
                store.qualified("handover_subject"),
                store.qualified("subject"),
            ),
            &[],
        )
        .unwrap();
    assert_eq!(people[0].int(0).unwrap(), 2);

    // And the password is nowhere in any of it.
    let store = reg.store();
    let row = store
        .query(
            &format!(
                "SELECT key_name, tool FROM {} WHERE id = {}",
                store.qualified("handover"),
                store.dialect().param(1, nils_registry::schema::Type::Int),
            ),
            &[Param::Int(report.handover_id)],
        )
        .unwrap();
    assert_eq!(row[0].text(0).unwrap(), "k", "the key by name");
    assert!(row[0].text(1).unwrap().contains("7-Zip"));
}

#[test]
fn a_damaged_or_missing_archive_is_found_when_it_is_read_back() {
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());
    let password = nils_release::handover::password(KEY);
    // A cap under one person, so each gets an archive of its own: a subject is
    // the unit and is never split.
    let report = handover::run(
        &mut reg,
        &settings(ship.path(), &archiver, &password, 512, Strategy::Ordered),
    )
    .unwrap();
    assert!(report.archives >= 2, "{report:?}");

    let mut archives: Vec<std::path::PathBuf> = std::fs::read_dir(ship.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "7z"))
        .collect();
    archives.sort();
    // One byte of the first, in the middle, which no checksum forgives.
    let bytes = std::fs::read(&archives[0]).unwrap();
    let mut damaged = bytes.clone();
    let at = damaged.len() / 2;
    damaged[at] ^= 0xFF;
    std::fs::write(&archives[0], &damaged).unwrap();
    std::fs::remove_file(&archives[1]).unwrap();

    let again = handover::verify(&mut reg, report.handover_id, &archiver, &password).unwrap();
    assert_eq!(again.failed.len(), 2, "{:?}", again.failed);
    assert!(
        again.failed.iter().any(|f| f.contains("checksum")),
        "{:?}",
        again.failed
    );
    assert!(
        again.failed.iter().any(|f| f.contains("not there")),
        "{:?}",
        again.failed
    );
    assert_eq!(again.verified, again.archives - 2);
}

#[test]
fn a_wrong_password_is_told_apart_from_a_damaged_archive() {
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());
    let password = nils_release::handover::password(KEY);
    let report = handover::run(
        &mut reg,
        &settings(
            ship.path(),
            &archiver,
            &password,
            1 << 30,
            Strategy::Ordered,
        ),
    )
    .unwrap();

    let wrong = nils_release::handover::password(b"another key entirely!!!");
    let again = handover::verify(&mut reg, report.handover_id, &archiver, &wrong).unwrap();
    assert_eq!(again.failed.len(), 1);
    assert!(
        again.failed[0].contains("the password does not open it"),
        "{:?}",
        again.failed
    );
}

#[test]
fn a_tree_somebody_edited_is_reported_rather_than_packed_quietly() {
    // A handover of a tree that is not the release is not a handover of the
    // release, so a file the record names and the disk does not is counted.
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![out.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            match e.path().is_dir() {
                true => stack.push(e.path()),
                false => files.push(e.path()),
            }
        }
    }
    files.sort();
    std::fs::remove_file(&files[0]).unwrap();

    let password = nils_release::handover::password(KEY);
    let report = handover::run(
        &mut reg,
        &settings(
            ship.path(),
            &archiver,
            &password,
            1 << 30,
            Strategy::Ordered,
        ),
    )
    .unwrap();
    assert_eq!(report.missing, 1, "said out loud, not skipped");
    assert!(report.failed.is_empty(), "and the rest still went");
}

#[test]
fn there_is_nothing_to_hand_over_until_there_is_a_release() {
    let Some(archiver) = archiver() else { return };
    let source = tree();
    let home_dir = TempDir::new("handover-home");
    let out = TempDir::new("handover-out");
    let ship = TempDir::new("handover-ship");
    let (_home, mut reg) = released(&home_dir, &source, out.path());
    let password = nils_release::handover::password(KEY);
    let mut s = settings(
        ship.path(),
        &archiver,
        &password,
        1 << 30,
        Strategy::Ordered,
    );
    s.release = "a cohort nobody released";
    let e = handover::run(&mut reg, &s).unwrap_err().to_string();
    assert!(e.contains("no finished release"), "{e}");
}
