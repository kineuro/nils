// SPDX-License-Identifier: AGPL-3.0-only

//! A release from end to end: what leaves, what does not, and what is recorded
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_pack as _;
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
    registry_on(home_dir, source, Backend::Sqlite, None)
}

/// The schema the Postgres half of these tests owns.
const SCHEMA: &str = "nils_release_test";

/// A registry on one backend, so that the release can be proved against both.
///
/// The date read of the session labels is the reason: Postgres hands a `date`
/// back in a type the store reads only as text, and a select that forgets the
/// cast fails only once a row of that shape exists, which for a release is the
/// first time anyone runs one.
fn registry_on(
    home_dir: &TempDir,
    source: &TempDir,
    backend: Backend,
    dsn: Option<String>,
) -> (Home, Registry) {
    let home = Home::new(home_dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend,
        dsn,
        schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
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

fn drop_schemas(dsn: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop the test schemas");
}

/// The pack every test releases under, loaded once.
fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    })
}

fn settings<'a>(out: &'a Path, policy: &'a Policy, scheme: &'a SessionScheme) -> run::Settings<'a> {
    run::Settings {
        name: "test",
        root: out,
        policy,
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme,
        private: &[],
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout: run::Layout::Descriptive,
        places: nils_release::bids::place::Options::default(),
        converter: None,
        compress: true,
        authors: &[],
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

/// A tree whose files say something about their own pixels, and carry two
/// private blocks: one the allowlist names and one it does not.
fn tree_with(burned: Option<&str>) -> TempDir {
    let dir = TempDir::new("release-pixels");
    let mut e = synth::minimal_mr("A", "A.1", "A.1.1");
    e.extend([
        synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
        synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
        synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        // The block the allowlist names, and the block it does not. In real
        // firmware the second has carried the patient name.
        synth::text(dicom_core::Tag(0x0019, 0x0010), VR::LO, "SIEMENS MR HEADER"),
        synth::text(dicom_core::Tag(0x0019, 0x100C), VR::IS, "1000"),
        synth::text(
            dicom_core::Tag(0x0029, 0x0010),
            VR::LO,
            "SIEMENS CSA HEADER",
        ),
        synth::text(dicom_core::Tag(0x0029, 0x1008), VR::CS, "IMAGE NUM 4"),
        // And an overlay, which is where somebody's arrow and somebody's name
        // end up.
        synth::text(dicom_core::Tag(0x6000, 0x0022), VR::LO, "drawn at a desk"),
    ]);
    if let Some(v) = burned {
        e.push(synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, v));
    }
    dir.file("s/1", &synth::part10(&MetaFields::mr("A.1.1"), &e, true));
    dir
}

fn allowed() -> Vec<nils_pack::private::Allowed> {
    vec![nils_pack::private::Allowed {
        creator: "SIEMENS MR HEADER".into(),
        group: 0x0019,
        element: 0x0C,
        why: "the b value".into(),
    }]
}

#[test]
fn a_stack_the_file_will_not_judge_is_held_and_asked_about() {
    // "No tag" is not "no text". An archive where most stacks are unjudgeable
    // is a fact a release should confront rather than average away, and the
    // engine does not look at pixels to settle it.
    let source = tree_with(None);
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let mut s = settings(out.path(), &policy, &scheme);
    s.on_unknown = nils_release::burned::OnUnknown::Hold;
    let report = run::run(&mut reg, &s).unwrap();

    assert_eq!(report.files, 0, "nothing left");
    assert_eq!(report.unjudged, 1);
    assert_eq!(report.burned_in, 0);
    assert!(files_under(out.path()).is_empty());

    // And a person was asked, so the release can be run again once answered.
    let store = reg.store();
    let rows = store
        .query(
            &format!(
                "SELECT kind FROM {} WHERE kind LIKE 'release.%'",
                store.qualified("review_item")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text(0).unwrap(), "release.unjudged");
}

#[test]
fn a_stack_the_file_says_carries_text_is_never_written() {
    let source = tree_with(Some("YES"));
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let mut s = settings(out.path(), &policy, &scheme);
    // Even told to write what it cannot judge: this one it can judge.
    s.on_unknown = nils_release::burned::OnUnknown::Write;
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.burned_in, 1);
    assert_eq!(report.files, 0);
}

#[test]
fn a_private_block_goes_and_the_one_the_pack_names_stays() {
    // v0 removes 119 named standard tags and touches no private element at
    // all, so every vendor block leaves the building. Siemens CSA headers
    // alone have carried the patient name in shipping firmware.
    let source = tree_with(Some("NO"));
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let keep = allowed();
    let mut s = settings(out.path(), &policy, &scheme);
    s.private = &keep;
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.files, 1);

    let written = files_under(out.path());
    let object = dicom_object::open_file(&written[0]).unwrap();
    let has = |g, e| {
        object
            .element_opt(dicom_core::Tag(g, e))
            .ok()
            .flatten()
            .is_some()
    };
    assert!(has(0x0019, 0x100C), "the b value is named and stays");
    assert!(
        has(0x0019, 0x0010),
        "and so does the creator that reserves it"
    );
    assert!(!has(0x0029, 0x1008), "the CSA block is not named");
    assert!(!has(0x0029, 0x0010), "and its creator names nothing now");
    assert!(!has(0x6000, 0x0022), "an overlay is where an arrow ends up");

    // And the run said which vendors it dropped, without saying what they held.
    assert_eq!(
        report.changes.get("private SIEMENS CSA HEADER removed"),
        Some(&1)
    );
    assert_eq!(
        report.changes.get("(0019,xx0C) SIEMENS MR HEADER kept"),
        Some(&1)
    );
    assert_eq!(report.changes.get("overlay removed"), Some(&1));
}

/// A vendor that exports every echo as its own series, which is what Siemens
/// does and what v0's naming cannot see.
fn two_echo_tree() -> TempDir {
    let dir = TempDir::new("release-echoes");
    for echo in 1..=2u32 {
        let series = format!("A.1.{echo}");
        let sop = format!("A.1.{echo}.1");
        let mut e = synth::minimal_mr("A", &series, &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, "P1"),
            synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
            synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, "ax t2star megre"),
            synth::text(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
            synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
            synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
            synth::text(tags::ECHO_NUMBERS, VR::IS, &echo.to_string()),
            synth::text(
                tags::ECHO_TIME,
                VR::DS,
                &format!("{}", 5.0 * f64::from(echo)),
            ),
            synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
        ]);
        dir.file(
            &format!("s{echo}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

#[test]
fn the_descriptive_layout_names_each_echo_by_its_own_echo_number() {
    // v0 appends an echo suffix only when the series holds more than one
    // stack, and this vendor gives each echo its own series, so every echo of
    // the session builds an identical name and falls through to a counter that
    // does not correspond between magnitude and phase.
    let source = two_echo_tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    // The name is a rendering of the classification axes, so the pipeline runs.
    nils_classify::job::fingerprint(
        &mut reg,
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();
    let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&packs, None).unwrap();
    nils_classify::classify::classify(
        &mut reg,
        &pack,
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(report.files, 2);

    let mut stems: Vec<String> = files_under(out.path())
        .iter()
        .filter_map(|p| {
            p.parent()?
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
        })
        .collect();
    stems.sort();
    assert_eq!(stems.len(), 2);
    // Each carries its own echo, and neither is a bare counter.
    assert!(stems[0].ends_with("_e1"), "{stems:?}");
    assert!(stems[1].ends_with("_e2"), "{stems:?}");
    assert!(stems[0].contains("T2starw"), "{stems:?}");
    // And the tree is subject, session, folder, name.
    let one = &files_under(out.path())[0];
    let parts: Vec<String> = one
        .strip_prefix(out.path())
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    assert_eq!(parts.len(), 5, "{parts:?}");
    assert!(parts[0].starts_with("sub-"), "{parts:?}");
    assert!(parts[1].starts_with("ses-"), "{parts:?}");
    assert_eq!(parts[2], "anat", "{parts:?}");
}

/// The classification a release renders into a name (§9.1), so that a test can
/// change one the way a QC decision does.
fn classified(reg: &mut Registry, source: &TempDir) {
    let _ = source;
    nils_classify::job::fingerprint(
        reg,
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();
    let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&packs, None).unwrap();
    nils_classify::classify::classify(
        reg,
        &pack,
        &nils_classify::Settings::default(),
        &nils_digest::Cancel::new(),
    )
    .unwrap();
}

/// Somebody looks at a stack and says it is a spinal cord, which is one of the
/// changes §8.6 exists for.
fn qc_says_spine(reg: &mut Registry) {
    let store = reg.store();
    let table = store.qualified("classification_axis");
    store
        .execute(
            &format!("DELETE FROM {table} WHERE axis = 'body_part'"),
            &[],
        )
        .unwrap();
    store
        .execute(
            &format!(
                "INSERT INTO {table} (stack_id, axis, value, confidence, tier) \
                 SELECT id, 'body_part', 'spine', 1.0, 'decided' FROM {}",
                store.qualified("stack")
            ),
            &[],
        )
        .unwrap();
}

/// Every file under the root, with when it was last written.
fn stamped(root: &Path) -> std::collections::BTreeMap<String, (std::time::SystemTime, Vec<u8>)> {
    files_under(root)
        .iter()
        .map(|p| {
            let meta = std::fs::metadata(p).unwrap();
            (
                p.strip_prefix(root).unwrap().display().to_string(),
                (meta.modified().unwrap(), std::fs::read(p).unwrap()),
            )
        })
        .collect()
}

#[test]
fn the_first_version_writes_everything_and_says_so() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let first = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

    assert!(first.previous.is_none());
    assert!(
        first.version.starts_with("20") && first.version.ends_with(".1"),
        "{}",
        first.version
    );
    assert_eq!(first.added, 2, "two stacks, both new");
    assert_eq!(first.unchanged, 0);
    assert_eq!(first.written, 2);
    assert_eq!(first.files, 2);
}

#[test]
fn a_re_run_that_changed_nothing_writes_nothing() {
    // The whole point of §8.6. v0 re-exports everything or nothing.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let first = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let before = stamped(out.path());

    let second = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(second.previous.as_deref(), Some(first.version.as_str()));
    assert_ne!(second.version, first.version, "a version still moved");
    assert_eq!(second.unchanged, 2);
    assert_eq!(second.written, 0, "not one file was written again");
    assert_eq!(second.added + second.moved + second.rewritten, 0);

    // And the manifest is still the whole tree, not the part this run touched:
    // a handover of this version has every file in it (§11).
    assert_eq!(second.files, first.files);
    assert_eq!(second.bytes, first.bytes);
    assert_eq!(stamped(out.path()), before, "nothing on disk moved");
}

#[test]
fn a_qc_decision_renames_the_files_rather_than_writing_them_again() {
    // Nima's case: a body part is corrected, and the name of a few thousand
    // files changes while the content of none does. The saving is the whole
    // reason the content digest leaves the place out.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    let before = stamped(out.path());

    qc_says_spine(&mut reg);
    let after = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(after.moved, 2);
    assert_eq!(after.written, 0, "renamed, not rewritten");
    assert_eq!(after.rewritten, 0);

    let now = stamped(out.path());
    assert_eq!(now.len(), before.len());
    let was: Vec<&String> = before.keys().collect();
    let is: Vec<&String> = now.keys().collect();
    assert_ne!(was, is, "the tree is named differently");
    // The same files, moved: same bytes, and the same moment of writing, which
    // is what says they were not written again.
    let mut old: Vec<&(std::time::SystemTime, Vec<u8>)> = before.values().collect();
    let mut new: Vec<&(std::time::SystemTime, Vec<u8>)> = now.values().collect();
    old.sort();
    new.sort();
    assert_eq!(old, new);
}

#[test]
fn a_stack_no_longer_in_the_release_leaves_the_tree() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let first = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(first.files, 2);

    // The pack rules one of them out, which is the ordinary way a stack stops
    // being in a release.
    {
        let store = reg.store();
        let sql = format!(
            "UPDATE {} SET value = 'excluded' WHERE axis = 'disposition' AND stack_id = \
             (SELECT MIN(stack_id) FROM {})",
            store.qualified("classification_axis"),
            store.qualified("classification_axis"),
        );
        store.execute(&sql, &[]).unwrap();
    }
    let second = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(second.removed, 1);
    assert_eq!(second.unchanged, 1);
    assert_eq!(second.written, 0);
    assert_eq!(files_under(out.path()).len(), 1, "and it is off the disk");
    // The directory it was in went with it, rather than being left empty.
    assert_eq!(second.files, 1);
}

#[test]
fn a_release_into_another_root_is_another_tree() {
    // A version compares against a state that describes one directory. Read
    // against a different one, every unchanged file would simply be missing.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let a = TempDir::new("release-a");
    let b = TempDir::new("release-b");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(&mut reg, &settings(a.path(), &policy, &scheme)).unwrap();
    let second = run::run(&mut reg, &settings(b.path(), &policy, &scheme)).unwrap();
    assert!(second.previous.is_none(), "nothing to compare against");
    assert_eq!(second.added, 2);
    assert_eq!(files_under(b.path()).len(), 2);
}

#[test]
fn a_tree_someone_emptied_is_written_again_rather_than_reported_as_moved() {
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    for entry in std::fs::read_dir(out.path()).unwrap().flatten() {
        std::fs::remove_dir_all(entry.path()).unwrap();
    }

    qc_says_spine(&mut reg);
    let after = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(after.moved, 0, "there was nothing to move");
    assert_eq!(after.rewritten, 2);
    assert_eq!(after.written, 2);
    assert_eq!(files_under(out.path()).len(), 2);
}

#[test]
fn a_release_and_its_next_version_run_on_postgres_too() {
    // Every other test here is on SQLite, and the two backends differ in the
    // types they hand back rather than in the SQL they accept, so a select that
    // reads a date or a timestamp raw works until somebody runs a release on
    // Postgres. This is that somebody.
    let Some(dsn) = postgres_dsn() else { return };
    drop_schemas(&dsn);
    let source = tree();
    let home_dir = TempDir::new("release-home-pg");
    let out = TempDir::new("release-out-pg");
    let (_home, mut reg) = registry_on(&home_dir, &source, Backend::Postgres, Some(dsn.clone()));
    classified(&mut reg, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let first = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(first.added, 2);
    assert_eq!(first.files, 2);
    assert!(first.version.ends_with(".1"), "{}", first.version);

    let second = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(second.previous.as_deref(), Some(first.version.as_str()));
    assert_eq!(second.unchanged, 2);
    assert_eq!(second.written, 0);

    qc_says_spine(&mut reg);
    let third = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(third.moved, 2);
    assert_eq!(third.written, 0);
    drop(reg);
    drop_schemas(&dsn);
}

#[test]
fn a_release_takes_an_enumeration_at_three_grains() {
    // §13: a release takes a selection and does not compute one. The grains
    // are the ones a cohort is actually made of, and each is a list a person
    // or a query can hand over.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let all = TempDir::new("release-all");
    let everything = run::run(&mut reg, &settings(all.path(), &policy, &scheme)).unwrap();
    assert_eq!(everything.subjects, 1);
    assert!(everything.stacks >= 2, "{everything:?}");

    // One stack of it, by id.
    let sql = format!("SELECT MIN(id) FROM {}", reg.store().qualified("stack"));
    let one = reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap();
    let out = TempDir::new("release-stack");
    let mut s = settings(out.path(), &policy, &scheme);
    s.selection.stacks = vec![one];
    let picked = run::run(&mut reg, &s).unwrap();
    assert_eq!(picked.stacks, 1);

    // One session of it, by the label the scheme gives it, which is matched
    // after the sessions are derived because a session is never a column.
    let out = TempDir::new("release-session");
    let mut s = settings(out.path(), &policy, &scheme);
    let sql = format!("SELECT code FROM {}", reg.store().qualified("subject"));
    let code = reg.store().query(&sql, &[]).unwrap()[0]
        .text(0)
        .unwrap()
        .to_string();
    s.selection.sessions = vec![(code.clone(), "20220115".to_string())];
    let session = run::run(&mut reg, &s).unwrap();
    assert_eq!(session.subjects, 1);
    assert!(session.stacks < everything.stacks, "one of two sessions");
    assert!(
        files_under(out.path())
            .iter()
            .all(|p| p.to_string_lossy().contains("ses-20220115")),
        "and only that session"
    );

    // A session nobody has is nothing, rather than everything.
    let out = TempDir::new("release-none");
    let mut s = settings(out.path(), &policy, &scheme);
    s.selection.sessions = vec![(code, "M99".to_string())];
    assert_eq!(run::run(&mut reg, &s).unwrap().stacks, 0);
}

#[test]
fn a_release_looks_before_it_writes() {
    // §9.6: a release that discovers a full disk after 400 GB has written 400
    // GB for nothing, and what it reports is the operating system's word for
    // it rather than what to do about it.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    // A root that cannot be made, because a file is in the way.
    let out = TempDir::new("release-out");
    let blocked = out.path().join("a-file");
    std::fs::write(&blocked, b"not a directory").unwrap();
    let e = run::run(&mut reg, &settings(&blocked, &policy, &scheme))
        .unwrap_err()
        .to_string();
    assert!(e.contains("cannot be written into"), "{e}");
}
