// SPDX-License-Identifier: AGPL-3.0-only

//! A release from end to end: what leaves, what does not, and what is recorded
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::session::{Naming, Scheme as SessionScheme};
use nils_registry::{Backend, Registry, Scheme};
use nils_release::policy::{Policy, Uids};
use nils_release::run::{self, Selection};
use nils_release::{dates, tags as categories};

const KEY: &[u8] = b"a release test key of some length";

/// Two studies of one person, six months apart, each with an identifier in it.
fn tree() -> TempDir {
    let dir = TempDir::new("release");
    for (n, day) in [("A", "20220115"), ("B", "20220715")] {
        let study = format!("{n}.1");
        let series = format!("{n}.1.1");
        let sop = format!("{n}.1.1.1");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
            synth::text(tags::PATIENT_NAME, VR::PN, "SVENSSON^ANNA"),
            synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19800101"),
            synth::text(tags::INSTITUTION_NAME, VR::LO, "Karolinska"),
            synth::text(tags::STUDY_DATE, VR::DA, day),
            synth::text(tags::STUDY_TIME, VR::TM, "031415"),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
            synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
            synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("{n}/1"),
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
    (home, reg)
}

fn settings<'a>(out: &'a Path, policy: &'a Policy, scheme: &'a SessionScheme) -> run::Settings<'a> {
    run::Settings {
        name: "test",
        root: out,
        policy,
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme,
        actor: "a test",
        key: KEY,
        pack: "mri",
        pack_version: "0.1.0",
    }
}

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
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn what_leaves_carries_no_identifier_and_says_what_was_done_to_it() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

    assert_eq!(report.files, 2);
    assert_eq!(report.subjects, 1);
    let written = files_under(out.path());
    assert_eq!(written.len(), 2);

    for path in &written {
        let bytes = std::fs::read(path).unwrap();
        // A part 10 file, whatever it was read from.
        assert_eq!(&bytes[128..132], b"DICM");
        for gone in [
            &b"SVENSSON"[..],
            &b"Karolinska"[..],
            &b"19800101-1234"[..],
            &b"031415"[..],
        ] {
            assert!(
                bytes.windows(gone.len()).all(|w| w != gone),
                "{} still holds {}",
                path.display(),
                String::from_utf8_lossy(gone)
            );
        }
    }

    // The age the birth date allowed, which the archive had and v0's output
    // does not: v0 removes the birth date without ever computing one.
    let ages = report
        .changes
        .iter()
        .filter(|(k, _)| k.starts_with("(0010,1010)"))
        .map(|(_, n)| *n)
        .sum::<i64>();
    assert_eq!(ages, 2, "an age was written for each");

    // And the run said what it did, without saying what any value was.
    let rendered = format!("{:?}", report.changes);
    assert!(!rendered.contains("SVENSSON"), "{rendered}");
    assert!(rendered.contains("(0010,0010) removed"), "{rendered}");
}

#[test]
fn the_same_release_twice_writes_the_same_bytes() {
    // Which is what makes two releases of overlapping selections agree, and
    // what the keyed deterministic remapping of §8.2 is for.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let first = TempDir::new("release-a");
    let second = TempDir::new("release-b");
    run::run(&mut reg, &settings(first.path(), &policy, &scheme)).unwrap();
    run::run(&mut reg, &settings(second.path(), &policy, &scheme)).unwrap();

    let a = files_under(first.path());
    let b = files_under(second.path());
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(
            x.strip_prefix(first.path()),
            y.strip_prefix(second.path()),
            "the same tree"
        );
        assert_eq!(
            std::fs::read(x).unwrap(),
            std::fs::read(y).unwrap(),
            "the same bytes"
        );
    }
}

#[test]
fn a_shift_moves_the_dates_and_keeps_the_interval() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy {
        dates: dates::Policy::Shift,
        ..Policy::default()
    };
    // §4.3: a scheme that labels by the date would put the date back in the
    // path, so a shifted release uses another.
    let scheme = SessionScheme {
        naming: Naming::Ordinal,
        ..SessionScheme::default()
    };
    run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

    let mut days: Vec<String> = Vec::new();
    for path in files_under(out.path()) {
        let object = dicom_object::open_file(&path).unwrap();
        days.push(
            object
                .element(tags::STUDY_DATE)
                .unwrap()
                .value()
                .to_str()
                .unwrap()
                .trim()
                .to_string(),
        );
    }
    days.sort();
    assert_eq!(days.len(), 2);
    assert!(!days.contains(&"20220115".to_string()), "{days:?}");
    let a = nils_registry::day::Day::parse(&days[0]).unwrap();
    let b = nils_registry::day::Day::parse(&days[1]).unwrap();
    assert_eq!(a.days_to(b), 181, "the interval is what survives");

    // And the offset is kept with the identifiers, because it is the thing
    // that undoes the policy.
    let mut linkage = reg.open_linkage().unwrap();
    let rows = linkage
        .query(
            &format!(
                "SELECT offset_days FROM {}",
                linkage.qualified("date_shift")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn the_two_halves_of_4_3_are_refused_rather_than_warned_about() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    // A shift with the UIDs kept: the true date leaves in the UID.
    let kept_uids = Policy {
        dates: dates::Policy::Shift,
        uids: Uids::Preserve,
        ..Policy::default()
    };
    let ordinal = SessionScheme {
        naming: Naming::Ordinal,
        ..SessionScheme::default()
    };
    let e = run::run(&mut reg, &settings(out.path(), &kept_uids, &ordinal))
        .unwrap_err()
        .to_string();
    assert!(e.contains("decorative"), "{e}");

    // And a shift with a date-named session: the tree carries what the files
    // no longer do.
    let shifted = Policy {
        dates: dates::Policy::Shift,
        ..Policy::default()
    };
    let by_date = SessionScheme::default();
    let e = run::run(&mut reg, &settings(out.path(), &shifted, &by_date))
        .unwrap_err()
        .to_string();
    assert!(e.contains("labels by the date"), "{e}");

    // Neither wrote anything.
    assert!(files_under(out.path()).is_empty());
}
