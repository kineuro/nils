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
    registry_in(home_dir, source, backend, dsn, SCHEMA)
}

/// The same, in a schema of the test's own, so two tests on Postgres do not
/// drop each other's tables.
fn registry_in(
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
    drop_schemas_named(dsn, SCHEMA);
}

fn drop_schemas_named(dsn: &str, schema: &str) {
    let mut store = nils_registry::Store::connect_postgres(dsn, schema).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
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
        naming: nils_release::name::Naming::Informative,
        places: nils_release::bids::place::Options::default(),
        converter: None,
        compress: true,
        observations: &[],
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

    // The row records the scheme that named the sessions, which here is the
    // one the run asked for, and says nothing about why they were named that
    // way: §4.3 numbered nothing, because nothing moved the dates.
    assert!(report.session_naming.is_none(), "{report:?}");
    let store = reg.store();
    let sql = format!(
        "SELECT session_scheme, session_naming FROM {} ORDER BY id DESC",
        store.qualified("release")
    );
    let stored = store.query(&sql, &[]).unwrap();
    let named: serde_json::Value = serde_json::from_str(stored[0].text(0).unwrap()).unwrap();
    assert_eq!(named["naming"], "date", "{named}");
    assert_eq!(stored[0].opt_text(1).unwrap(), None);
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

/// One study of one person, under its own UIDs, for a second dataset.
fn tree_of(prefix: &str, patient: &str, day: &str) -> TempDir {
    let dir = TempDir::new("release-dataset");
    let study = format!("{prefix}.1");
    let series = format!("{prefix}.1.1");
    let sop = format!("{prefix}.1.1.1");
    let mut e = synth::minimal_mr(&study, &series, &sop);
    e.extend([
        synth::text(tags::PATIENT_ID, VR::LO, patient),
        synth::text(tags::PATIENT_NAME, VR::PN, "PERSSON^BO"),
        synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19750301"),
        synth::text(tags::STUDY_DATE, VR::DA, day),
        synth::text(tags::STUDY_TIME, VR::TM, "101010"),
        synth::text(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
        synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
    ]);
    dir.file("s/1", &synth::part10(&MetaFields::mr(&sop), &e, true));
    dir
}

/// A source place on a folder, with the leaving policy it declares
/// (record 26 section 13): the folder itself is its pseudonymised tree.
fn dataset(reg: &mut Registry, name: &str, path: &Path, on_release: serde_json::Value) -> i64 {
    nils_registry::place::add(
        reg.store(),
        &nils_registry::place::New {
            name,
            role: nils_registry::place::Role::Source,
            path: path.to_str().unwrap(),
            guarantees: serde_json::json!({}),
            probed: serde_json::json!({}),
            handling: serde_json::json!({"on_release": on_release}),
            dataset: serde_json::Value::Null,
        },
    )
    .unwrap()
}

/// The study date and the SOP instance UID of each file written, by path.
fn written_dates_and_uids(root: &Path) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for path in files_under(root) {
        let object = dicom_object::open_file(&path).unwrap();
        let text = |tag| {
            object
                .element(tag)
                .unwrap()
                .value()
                .to_str()
                .unwrap()
                .trim()
                .to_string()
        };
        out.push((
            path.strip_prefix(root).unwrap().display().to_string(),
            text(tags::STUDY_DATE),
            text(tags::SOP_INSTANCE_UID),
        ));
    }
    out
}

#[test]
fn a_release_spanning_two_datasets_leaves_each_under_its_own_policy() {
    // Record 26 section 13: without --dates and --uids the policy of each
    // file comes from the dataset holding it; the row records them all.
    let a = tree_of("A", "19750301-1111", "20220115");
    let b = tree_of("B", "19750301-2222", "20220115");
    let home_dir = TempDir::new("release-home");
    let (_home, mut reg) = registry(&home_dir, &a);
    let mut s = nils_digest::Settings::new(b.path());
    s.name = "b".into();
    digest(&s, &mut reg).unwrap();
    dataset(
        &mut reg,
        "ds-shifted",
        a.path(),
        serde_json::json!({"dates": "shift", "uids": "remap"}),
    );
    dataset(
        &mut reg,
        "ds-kept",
        b.path(),
        serde_json::json!({"dates": "keep", "uids": "preserve"}),
    );
    let ordinal = SessionScheme {
        naming: Naming::Ordinal,
        ..SessionScheme::default()
    };
    let policy = Policy::default();
    let out = TempDir::new("release-out");
    let by_dataset = run::Settings {
        policy_from: nils_release::policy::Source::Datasets,
        ..settings(out.path(), &policy, &ordinal)
    };
    let report = run::run(&mut reg, &by_dataset).unwrap();
    assert_eq!(report.files, 2, "{report:?}");
    // the row says what each dataset's files left under, and where from
    let mut rows: Vec<(String, String, String, String)> = report
        .policies
        .iter()
        .map(|p| {
            (
                p["dataset"].as_str().unwrap_or("").to_string(),
                p["dates"].as_str().unwrap().to_string(),
                p["uids"].as_str().unwrap().to_string(),
                p["from"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (
                "ds-kept".into(),
                "keep".into(),
                "preserve".into(),
                "dataset".into()
            ),
            (
                "ds-shifted".into(),
                "shift".into(),
                "remap".into(),
                "dataset".into()
            ),
        ]
    );
    assert!(
        report.policy.contains("ds-shifted: dates shift"),
        "{}",
        report.policy
    );
    // and each file left under its own: the kept dataset's file keeps its
    // date and its UID, the shifted dataset's has neither
    let files = written_dates_and_uids(out.path());
    assert_eq!(files.len(), 2, "{files:?}");
    let kept = files.iter().find(|(_, _, uid)| uid == "B.1.1.1");
    assert!(
        kept.is_some(),
        "the preserved UID names the file: {files:?}"
    );
    assert_eq!(kept.unwrap().1, "20220115");
    let shifted = files.iter().find(|(_, _, uid)| uid != "B.1.1.1").unwrap();
    assert_ne!(shifted.1, "20220115", "{files:?}");
    assert_ne!(shifted.2, "A.1.1.1", "{files:?}");
    let store = reg.store();
    let sql = format!(
        "SELECT policies, policy FROM {} ORDER BY id DESC",
        store.qualified("release")
    );
    let stored = store.query(&sql, &[]).unwrap();
    let policies: serde_json::Value = serde_json::from_str(stored[0].text(0).unwrap()).unwrap();
    assert_eq!(policies.as_array().unwrap().len(), 2, "{policies}");
    let own: serde_json::Value = serde_json::from_str(stored[0].text(1).unwrap()).unwrap();
    assert_eq!(own["from"], "datasets", "{own}");

    // the flags given override every dataset's, and the row says so
    let flagged_out = TempDir::new("release-out-flags");
    let flagged = run::Settings {
        name: "flagged",
        policy_from: nils_release::policy::Source::Flags,
        ..settings(flagged_out.path(), &policy, &ordinal)
    };
    let report = run::run(&mut reg, &flagged).unwrap();
    assert!(
        report
            .policies
            .iter()
            .all(|p| p["from"] == "flags" && p["dates"] == "keep" && p["uids"] == "remap"),
        "{:?}",
        report.policies
    );
    let files = written_dates_and_uids(flagged_out.path());
    assert!(
        files.iter().all(|(_, day, _)| day == "20220115"),
        "{files:?}"
    );
    assert!(
        files.iter().all(|(_, _, uid)| uid != "B.1.1.1"),
        "{files:?}"
    );

    // a dataset as the selection releases its files and no other's
    let one_out = TempDir::new("release-out-one");
    let one = run::Settings {
        name: "one",
        policy_from: nils_release::policy::Source::Datasets,
        selection: Selection {
            datasets: vec!["ds-kept".to_string()],
            ..Selection::default()
        },
        ..settings(one_out.path(), &policy, &ordinal)
    };
    let report = run::run(&mut reg, &one).unwrap();
    assert_eq!(report.files, 1, "{report:?}");
    assert_eq!(report.policies.len(), 1, "{:?}", report.policies);
    assert_eq!(report.policies[0]["dataset"], "ds-kept");

    // a dataset whose declared policy would shift dates and preserve UIDs is
    // refused by name, as the run's own is; the declaration door refuses
    // it too, so it is written past the door here
    let t = nils_registry::schema::table("place");
    let store = reg.store();
    let sql = format!(
        "UPDATE {} SET handling = {} WHERE name = 'ds-kept'",
        store.qualified("place"),
        store.dialect().param(1, t.column("handling").unwrap().ty)
    );
    store
        .execute(
            &sql,
            &[nils_registry::Param::from(
                serde_json::json!({"on_release": {"dates": "shift", "uids": "preserve"}})
                    .to_string(),
            )],
        )
        .unwrap();
    let refused_out = TempDir::new("release-out-refused");
    let refused = run::Settings {
        name: "refused",
        policy_from: nils_release::policy::Source::Datasets,
        ..settings(refused_out.path(), &policy, &ordinal)
    };
    let e = run::run(&mut reg, &refused).unwrap_err().to_string();
    assert!(
        e.contains("dataset ds-kept") && e.contains("decorative"),
        "{e}"
    );
    assert!(files_under(refused_out.path()).is_empty());
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

    // And a shift with a date-named session, both halves asked for by the
    // run's own flags: the tree would carry the date the files no longer do,
    // and a warning on a run that produced a tree is read after the tree
    // exists. The refusal names the case that is resolved instead, a dataset
    // declaring the policy of its own.
    let shifted = Policy {
        dates: dates::Policy::Shift,
        ..Policy::default()
    };
    let by_date = SessionScheme::default();
    let e = run::run(&mut reg, &settings(out.path(), &shifted, &by_date))
        .unwrap_err()
        .to_string();
    assert!(e.contains("labels by the date"), "{e}");
    assert!(e.contains("numbered in date order"), "{e}");

    // Neither wrote anything.
    assert!(files_under(out.path()).is_empty());
}

#[test]
fn a_dataset_that_declares_moved_dates_numbers_its_sessions_rather_than_refusing() {
    // §4.3 with record 26 section 13: nobody gave --dates or --uids, so the
    // shift is the dataset's own standing rule rather than an instruction of
    // this run's. A declared policy that could never be released would be a
    // declaration nobody could use, and an ordinal label leaks no date, which
    // is what §4.3 protects: here the labels give way rather than the release.
    let source = tree();
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    dataset(
        &mut reg,
        "ds-shifted",
        source.path(),
        serde_json::json!({"dates": "shift", "uids": "remap"}),
    );

    let by_date = SessionScheme::default();
    let defaults = Policy::default();
    let declared = run::Settings {
        policy_from: nils_release::policy::Source::Datasets,
        ..settings(out.path(), &defaults, &by_date)
    };
    let report = run::run(&mut reg, &declared).unwrap();
    assert_eq!(report.files, 2, "{report:?}");
    // the report says why, and names whose declaration it was
    let why = report.session_naming.clone().unwrap_or_default();
    assert!(why.contains("ds-shifted"), "{why}");
    assert!(why.contains("numbered in date order"), "{why}");
    let written = files_under(out.path());
    assert!(!written.is_empty());
    for path in &written {
        let text = path.display().to_string();
        assert!(
            !text.contains("20220115") && !text.contains("20220715"),
            "{text}"
        );
        assert!(text.contains("ses-01") || text.contains("ses-02"), "{text}");
    }
    // and the row records the scheme that named them, not the one asked for
    let store = reg.store();
    let sql = format!(
        "SELECT session_scheme FROM {} ORDER BY id DESC",
        store.qualified("release")
    );
    let stored = store.query(&sql, &[]).unwrap();
    let scheme: serde_json::Value = serde_json::from_str(stored[0].text(0).unwrap()).unwrap();
    assert_eq!(scheme["naming"], "ordinal", "{scheme}");
    // and the sentence with it, so that why they were numbered hangs off the
    // release itself and not off the job that happened to make it
    let sql = format!(
        "SELECT session_naming FROM {} ORDER BY id DESC",
        store.qualified("release")
    );
    let stored = store.query(&sql, &[]).unwrap();
    assert_eq!(stored[0].text(0).unwrap(), why, "{why}");
}

/// A tree whose files say something about their own pixels, and carry two
/// private blocks: one the allowlist names and one it does not.
fn tree_with(burned: Option<&str>) -> TempDir {
    tree_saying(burned, None)
}

/// The same, with an image type of the test's own, for the stacks whose
/// pixels are a photograph of a screen and say so there rather than in
/// `BurnedInAnnotation`.
fn tree_saying(burned: Option<&str>, image_type: Option<&str>) -> TempDir {
    let dir = TempDir::new("release-pixels");
    let mut e = synth::minimal_mr("A", "A.1", "A.1.1");
    if let Some(v) = image_type {
        e.push(synth::text(tags::IMAGE_TYPE, VR::CS, v));
    }
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

/// The review items a release filed, by kind, in the order they were filed.
fn release_items(reg: &mut Registry) -> Vec<String> {
    let store = reg.store();
    let sql = format!(
        "SELECT kind FROM {} WHERE kind LIKE 'release.%' ORDER BY id",
        store.qualified("review_item")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

/// What the release's own row says it held and could not judge, and under
/// which setting.
fn row_counts(reg: &mut Registry, release: i64) -> (i64, i64, String) {
    let store = reg.store();
    let policy = nils_registry::schema::table("release")
        .column("policy")
        .expect("release.policy is a column");
    let sql = format!(
        "SELECT burned_in, unjudged, {} FROM {} WHERE id = {}",
        store.dialect().text_of(policy),
        store.qualified("release"),
        release
    );
    let rows = store.query(&sql, &[]).unwrap();
    let policy: serde_json::Value = serde_json::from_str(rows[0].text(2).unwrap()).unwrap();
    (
        rows[0].int(0).unwrap(),
        rows[0].int(1).unwrap(),
        policy["on_unknown"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[test]
fn a_stack_the_file_will_not_judge_is_released_and_counted() {
    // §8.4 as ratified: where the tag is absent the release says how many
    // stacks it could not judge. It does not keep them: the tag is absent on
    // most of the series of a real archive, so a default that held on it held
    // three quarters of what was selected and most subjects released nothing.
    let source = tree_with(None);
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(out.path(), &policy, &scheme);
    let report = run::run(&mut reg, &s).unwrap();

    assert_eq!(report.unjudged, 1, "counted");
    assert_eq!(report.burned_in, 0);
    assert_eq!(report.stacks, 1, "and written");
    assert!(!files_under(out.path()).is_empty());
    // Nobody is asked about a stack nothing is being done to.
    assert!(release_items(&mut reg).is_empty());
}

#[test]
fn the_report_and_the_release_row_carry_the_count_it_could_not_judge() {
    // One number, in the report a person reads and in the row the tree's own
    // record keeps, beside the setting that says what was done with them.
    let source = tree_with(None);
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(out.path(), &policy, &scheme);
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.unjudged, 1);
    assert_eq!(report.on_unknown, "write");
    assert_eq!(
        row_counts(&mut reg, report.release_id),
        (0, 1, "write".into())
    );

    // And under the strict setting the same number reads the other way, which
    // is why the row says which was asked for.
    let out = TempDir::new("release-out");
    let mut s = settings(out.path(), &policy, &scheme);
    s.on_unknown = nils_release::burned::OnUnknown::Hold;
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.on_unknown, "hold");
    assert_eq!(
        row_counts(&mut reg, report.release_id),
        (0, 1, "hold".into())
    );
}

#[test]
fn the_strict_setting_still_holds_every_stack_the_file_will_not_judge() {
    // A site that will not let an unjudged stack leave before somebody has
    // looked at it asks for that, and gets exactly what the default gave
    // before: nothing written, and a question per held stack.
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
    assert_eq!(release_items(&mut reg), ["release.unjudged"]);
}

#[test]
fn a_stack_the_file_says_carries_text_is_never_written() {
    let source = tree_with(Some("YES"));
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    // The default, which writes what it cannot judge: this one it can judge.
    let s = settings(out.path(), &policy, &scheme);
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.burned_in, 1);
    assert_eq!(report.unjudged, 0);
    assert_eq!(report.files, 0);
    assert_eq!(release_items(&mut reg), ["release.burned_in"]);
    assert_eq!(
        row_counts(&mut reg, report.release_id),
        (1, 0, "write".into())
    );
}

#[test]
fn a_stack_whose_image_type_says_screenshot_is_held_whatever_the_tag_says() {
    // A photograph of a screen is a photograph of a screen, and firmware that
    // writes the token frequently writes `BurnedInAnnotation NO` by rote.
    let source = tree_saying(Some("NO"), Some("DERIVED\\SECONDARY\\SCREENSHOT"));
    let home_dir = TempDir::new("release-home");
    let out = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(out.path(), &policy, &scheme);
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.burned_in, 1);
    assert_eq!(report.unjudged, 0);
    assert_eq!(report.files, 0);
    assert!(files_under(out.path()).is_empty());
    assert_eq!(release_items(&mut reg), ["release.burned_in"]);
}

#[test]
fn a_held_stack_is_asked_about_once_and_not_again_on_the_next_release() {
    // A release is re-run whenever anything upstream of it changes, and the
    // file says the same thing every time: two releases of one selection once
    // filed two queues of identical questions. What recurs is the count.
    let source = tree_with(Some("YES"));
    let home_dir = TempDir::new("release-home");
    let first = TempDir::new("release-out");
    let (_home, mut reg) = registry(&home_dir, &source);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(first.path(), &policy, &scheme);
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.burned_in, 1);
    assert_eq!(release_items(&mut reg), ["release.burned_in"]);

    let second = TempDir::new("release-out");
    let s = settings(second.path(), &policy, &scheme);
    let again = run::run(&mut reg, &s).unwrap();
    assert_eq!(again.burned_in, 1, "the count is filed again");
    assert_eq!(
        release_items(&mut reg),
        ["release.burned_in"],
        "the question is not"
    );
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

#[test]
fn a_role_is_matched_by_equality_against_a_row_per_value() {
    // Wave 4a §6.1, fault 4: a stack that is both a t1w and a flair holds two
    // role rows, and `--role flair` finds it by equality, where a joined
    // string needed four patterns and a role named `t1` would have matched
    // `t1w` by accident.
    let source = tree();
    let home_dir = TempDir::new("release-home-roles");
    let out = TempDir::new("release-out-roles");
    let (_home, mut reg) = registry(&home_dir, &source);
    classified(&mut reg, &source);
    let one = {
        let store = reg.store();
        let table = store.qualified("classification_axis");
        store
            .execute(&format!("DELETE FROM {table} WHERE axis = 'role'"), &[])
            .unwrap();
        let first = store
            .query(
                &format!("SELECT MIN(id) FROM {}", store.qualified("stack")),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        for role in ["t1w", "flair"] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {table} (stack_id, axis, value, confidence, tier) \
                         VALUES ({first}, 'role', '{role}', 1.0, 'decided')"
                    ),
                    &[],
                )
                .unwrap();
        }
        first
    };
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let mut s = settings(out.path(), &policy, &scheme);
    s.selection.roles = vec!["flair".to_string()];
    let flair = run::run(&mut reg, &s).unwrap();
    assert_eq!(flair.stacks, 1, "{flair:?}");
    let mut s = settings(out.path(), &policy, &scheme);
    s.selection.roles = vec!["t1".to_string()];
    let t1 = run::run(&mut reg, &s).unwrap();
    assert_eq!(t1.stacks, 0, "a role is a value, not a prefix: {t1:?}");
    let _ = one;
}

/// Wave 4a §8: the selection resolves at the four grains, on both backends,
/// and `preview` counts what it reaches.
#[test]
fn a_selection_resolves_at_the_four_grains_and_is_counted_before_it_leaves() {
    use nils_registry::schema;
    use nils_registry::store::{Insert, Param};
    use nils_release::select::{self, How, Item};
    const OWN: &str = "nils_select_test";
    let mut backends: Vec<(Backend, Option<String>)> = vec![(Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        drop_schemas_named(&dsn, OWN);
        backends.push((Backend::Postgres, Some(dsn)));
    }
    for (backend, dsn) in backends {
        let name = format!("{backend:?}");
        let source = tree();
        let home_dir = TempDir::new("select-home");
        let (_home, mut reg) = registry_in(&home_dir, &source, backend, dsn, OWN);
        // A cohort with the one subject in it, and an axis value on one stack.
        let (code, stack) = {
            let store = reg.store();
            let row = &store
                .query(
                    &format!(
                        "SELECT su.id, su.code, MIN(k.id) FROM {} su JOIN {} se ON se.subject_id = su.id \
                         JOIN {} k ON k.series_id = se.id GROUP BY su.id, su.code",
                        store.qualified("subject"),
                        store.qualified("series"),
                        store.qualified("stack")
                    ),
                    &[],
                )
                .unwrap()[0];
            let (subject, code, stack) = (
                row.int(0).unwrap(),
                row.text(1).unwrap().to_string(),
                row.int(2).unwrap(),
            );
            let cohort = store
                .insert(
                    &Insert::new(schema::table("cohort"), &["name", "owner", "created_at"])
                        .returning(&["id"]),
                    &[vec![
                        Param::from("MS-2026"),
                        Param::from("the group"),
                        Param::from("2026-09-06T00:00:00Z"),
                    ]],
                )
                .unwrap()[0]
                .int(0)
                .unwrap();
            store
                .insert(
                    &Insert::new(
                        schema::table("cohort_member"),
                        &["cohort_id", "subject_id", "joined_at"],
                    ),
                    &[vec![
                        Param::Int(cohort),
                        Param::Int(subject),
                        Param::from("2026-09-06T00:00:00Z"),
                    ]],
                )
                .unwrap();
            store
                .insert(
                    &Insert::new(
                        schema::table("classification_axis"),
                        &["stack_id", "axis", "value", "confidence", "tier"],
                    ),
                    &[vec![
                        Param::Int(stack),
                        Param::from("base"),
                        Param::from("T1w"),
                        Param::Double(1.0),
                        Param::from("certain"),
                    ]],
                )
                .unwrap();
            (code, stack)
        };
        let items = vec![
            Item::Cohort("ms-2026".into()),
            Item::Subject(code.clone()),
            Item::Subject("19800101-1234".into()),
            Item::Subject("nobody".into()),
            Item::Session("19800101-1234".into(), "20220115".into()),
            Item::Stack(stack),
            Item::Axis("base".into(), "T1w".into()),
            Item::Axis("base".into(), "T2w".into()),
            Item::Axis("nonsense".into(), "x".into()),
        ];
        let resolved = select::resolve(&mut reg, &items, Some(pack())).unwrap();
        let how = |item: &Item| -> Option<How> {
            resolved
                .items
                .iter()
                .find(|(i, _)| i == item)
                .map(|(_, h)| h.clone())
        };
        assert_eq!(how(&items[0]), Some(How::Cohort { members: 1 }), "{name}");
        assert_eq!(how(&items[1]), Some(How::Code), "{name}");
        // The patient id the digest filed as an identifier resolves to the
        // same subject, and says which type it was.
        assert_eq!(
            how(&items[2]),
            Some(How::Identifier {
                id_type: "patient-id".into(),
                code: code.clone()
            }),
            "{name}"
        );
        assert_eq!(how(&items[3]), None, "{name}: unknown");
        assert!(
            matches!(how(&items[4]), Some(How::Session(inner)) if matches!(*inner, How::Identifier { .. })),
            "{name}: {:?}",
            how(&items[4])
        );
        assert_eq!(how(&items[5]), Some(How::Stack), "{name}");
        assert_eq!(how(&items[6]), Some(How::Axis { stacks: 1 }), "{name}");
        assert_eq!(how(&items[7]), Some(How::Axis { stacks: 0 }), "{name}");
        assert_eq!(
            how(&items[8]),
            None,
            "{name}: the pack decides no such axis"
        );
        let unresolved: Vec<String> = resolved
            .unresolved
            .iter()
            .map(|(i, _)| i.describe())
            .collect();
        assert_eq!(unresolved, vec!["nobody", "nonsense=x"], "{name}");
        // The enumeration: one subject, once, whichever way it was named.
        assert_eq!(resolved.selection.subjects, vec![code.clone()], "{name}");
        assert_eq!(resolved.selection.cohorts, vec!["ms-2026"], "{name}");
        assert_eq!(
            resolved.selection.sessions,
            vec![(code.clone(), "20220115".to_string())],
            "{name}"
        );
        assert_eq!(resolved.selection.stacks, vec![stack], "{name}");
        assert_eq!(resolved.selection.axes.len(), 2, "{name}");

        // Counted before it leaves: the whole registry, then the one stack
        // the axis value names.
        let whole = run::preview(reg.store(), &Selection::default()).unwrap();
        assert_eq!(whole.subjects, 1, "{name}");
        assert!(whole.stacks > 1, "{name}: {whole:?}");
        let axis = Selection {
            axes: vec![("base".into(), "T1w".into())],
            ..Selection::default()
        };
        let one = run::preview(reg.store(), &axis).unwrap();
        assert_eq!(one.stacks, 1, "{name}: {one:?}");
        assert!(one.files >= 1 && one.bytes > 0, "{name}: {one:?}");
        let two = Selection {
            axes: vec![("base".into(), "T1w".into()), ("base".into(), "T2w".into())],
            ..Selection::default()
        };
        assert_eq!(
            run::preview(reg.store(), &two).unwrap().stacks,
            1,
            "{name}: two values of one axis are alternatives"
        );
        let none = Selection {
            axes: vec![
                ("base".into(), "T1w".into()),
                ("technique".into(), "FLAIR".into()),
            ],
            ..Selection::default()
        };
        assert_eq!(
            run::preview(reg.store(), &none).unwrap().stacks,
            0,
            "{name}: two axes both have to hold"
        );
    }
}

/// Wave 4a §12, bar 9: a file removed from the tree since the last version
/// is not carried forward on the state's word. The stack is written again,
/// and the report says how many were.
#[test]
fn a_stack_whose_files_left_the_tree_is_written_again_and_not_carried() {
    let source = tree();
    let home_dir = TempDir::new("restore-home");
    let out = TempDir::new("restore-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let first = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert!(first.added > 0, "{first:?}");
    assert_eq!(first.restored, 0);
    // Somebody removes one stack's directory from the tree.
    let mut dirs: Vec<std::path::PathBuf> = walkdir_like(out.path())
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| p.parent().unwrap().to_path_buf())
        .collect();
    dirs.sort();
    dirs.dedup();
    let gone = dirs.first().expect("a stack directory").clone();
    std::fs::remove_dir_all(&gone).unwrap();
    assert!(!gone.exists());
    // The re-run writes it again and says so; the rest is unchanged.
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(again.restored, 1, "{again:?}");
    assert_eq!(again.rewritten, 1, "{again:?}");
    assert_eq!(again.added, 0, "{again:?}");
    assert_eq!(again.unchanged, first.added - 1, "{again:?}");
    assert!(gone.is_dir(), "the stack is back on the disk");
    // And a third run carries everything.
    let third = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(third.restored, 0, "{third:?}");
    assert_eq!(third.unchanged, first.added, "{third:?}");
}

fn walkdir_like(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out
}

/// Every direct identifier of `tags::NEVER_LEAVES`, with the VR it is written
/// under. The two lists are held against each other below, because a fixture
/// that quietly stopped writing one would prove the release removed it.
const IDENTIFIERS: &[((u16, u16), VR)] = &[
    ((0x0008, 0x0050), VR::SH),
    ((0x0008, 0x0080), VR::LO),
    ((0x0008, 0x0081), VR::ST),
    ((0x0008, 0x0090), VR::PN),
    ((0x0008, 0x1010), VR::SH),
    ((0x0008, 0x1050), VR::PN),
    ((0x0008, 0x1070), VR::PN),
    ((0x0010, 0x0010), VR::PN),
    ((0x0010, 0x0030), VR::DA),
    ((0x0010, 0x1000), VR::LO),
    ((0x0010, 0x1001), VR::PN),
    ((0x0010, 0x1005), VR::PN),
    ((0x0010, 0x1040), VR::LO),
    ((0x0010, 0x2154), VR::SH),
    ((0x0010, 0x4000), VR::LT),
    ((0x0012, 0x0040), VR::LO),
    ((0x0018, 0x1000), VR::LO),
    ((0x0020, 0x0010), VR::SH),
    ((0x0032, 0x1032), VR::PN),
    ((0x0038, 0x0010), VR::LO),
    ((0x0038, 0x0300), VR::LO),
    ((0x0038, 0x0400), VR::LO),
    ((0x0040, 0x0242), VR::SH),
    ((0x0040, 0x2008), VR::PN),
    ((0x0040, 0x2010), VR::SH),
    ((0x0040, 0x2016), VR::LO),
    ((0x0040, 0x2017), VR::LO),
    ((0x0040, 0xA123), VR::PN),
];

/// What each is written with: a marker of its own, so that a value found in
/// the output says which element it came from, and none of them is a word
/// any other part of the file uses. Nothing here is from an archive.
fn identifier_value(i: usize, vr: VR) -> String {
    match vr {
        // A date has to parse. Its category removes it outright, before the
        // date policy of §8.3 ever reads the file, so it is checked by its
        // absence rather than by its bytes.
        VR::DA => "19800101".to_string(),
        _ => format!("NOTLEAVING{i:02}"),
    }
}

fn identifiers() -> Vec<(dicom_core::Tag, VR, String)> {
    IDENTIFIERS
        .iter()
        .enumerate()
        .map(|(i, ((g, e), vr))| (dicom_core::Tag(*g, *e), *vr, identifier_value(i, *vr)))
        .collect()
}

/// Two studies of one person, each file carrying every direct identifier.
fn tree_of_identifiers() -> TempDir {
    let dir = TempDir::new("release-identifiers");
    for (n, day) in [("A", "20220115"), ("B", "20220715")] {
        let study = format!("{n}.1");
        let series = format!("{n}.1.1");
        let sop = format!("{n}.1.1.1");
        let mut e = synth::minimal_mr(&study, &series, &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, "a-code-of-some-length"),
            synth::text(tags::STUDY_DATE, VR::DA, day),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
            synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
            synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        e.extend(
            identifiers()
                .into_iter()
                .map(|(tag, vr, value)| synth::text(tag, vr, &value)),
        );
        dir.file(
            &format!("{n}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

/// Record 35, S1: a release removes every direct identifier and can prove it.
///
/// This is the test that would have caught finding 1. The accession number
/// and the device serial number were in none of the removal categories, so a
/// release wrote both through verbatim on every file it had ever written, and
/// its change list never mentioned either: nothing in the output said the
/// elements existed, so nobody reading a report could tell. The list of what
/// may never survive is written out in the crate rather than derived from the
/// categories, this fixture is held against that list, and each element is
/// looked for in the release's own output.
#[test]
fn no_direct_identifier_survives_a_release_and_the_report_names_each() {
    // The fixture and the list say the same thing, or the proof is about
    // whatever the fixture happened to write.
    let mut carried: Vec<dicom_core::Tag> = identifiers().iter().map(|(t, _, _)| *t).collect();
    let mut listed: Vec<dicom_core::Tag> = categories::NEVER_LEAVES
        .iter()
        .map(|(g, e)| dicom_core::Tag(*g, *e))
        .collect();
    carried.sort_unstable();
    listed.sort_unstable();
    assert_eq!(carried, listed, "the fixture and the list have drifted");

    let source = tree_of_identifiers();
    // And the files really carry them: an element the fixture never wrote is
    // an element the release cannot be shown to have removed.
    for path in files_under(source.path()) {
        let object = dicom_object::open_file(&path).unwrap();
        for (tag, _, _) in identifiers() {
            assert!(
                object.element_opt(tag).ok().flatten().is_some(),
                "({:04X},{:04X}) is not in the file the release is asked to clean",
                tag.group(),
                tag.element()
            );
        }
    }

    let home_dir = TempDir::new("identifiers-home");
    let out = TempDir::new("identifiers-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();
    assert_eq!(report.files, 2);

    let written = files_under(out.path());
    assert_eq!(written.len(), 2);
    for path in &written {
        let object = dicom_object::open_file(path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        for (tag, vr, value) in identifiers() {
            assert!(
                object.element_opt(tag).ok().flatten().is_none(),
                "({:04X},{:04X}) survived the release, in {}",
                tag.group(),
                tag.element(),
                path.display()
            );
            if vr == VR::DA {
                continue;
            }
            assert!(
                bytes.windows(value.len()).all(|w| w != value.as_bytes()),
                "the value of ({:04X},{:04X}) is still in the bytes of {}",
                tag.group(),
                tag.element(),
                path.display()
            );
        }
    }

    // And the report names each of them, so a reader sees the element was
    // handled rather than absent by luck. Two files, so twice each.
    for (tag, _, _) in identifiers() {
        let named = format!("({:04X},{:04X}) removed", tag.group(), tag.element());
        assert_eq!(
            report.changes.get(&named),
            Some(&2),
            "{named} is not in the change list: {:?}",
            report.changes
        );
    }

    // The row says which categories were applied, the one that holds the
    // accession number among them: "de-identified" is not a property a file
    // carries without saying under what rule.
    let store = reg.store();
    let sql = format!(
        "SELECT categories FROM {} ORDER BY id DESC",
        store.qualified("release")
    );
    let stored = store.query(&sql, &[]).unwrap();
    let applied = stored[0].text(0).unwrap().to_string();
    assert_eq!(
        applied, "patient,trial,provider,institution,times,ids",
        "the release did not record what it applied"
    );
}

/// Record 35, the seam between S1 and S3: one release writes both answers.
///
/// S1 gave the release a sixth category and made the row say which categories
/// it applied; S3 gave the same row the two counts and the report the line
/// that reads them. They are written by different statements, the insert and
/// the close, and a release that reported one and lost the other would be
/// telling half of what it did. The fixture has no burned-in tag at all, so
/// both stacks are unjudged and every direct identifier is in the file.
#[test]
fn the_release_row_says_what_it_removed_and_what_it_could_not_judge() {
    let source = tree_of_identifiers();
    let home_dir = TempDir::new("seam-home");
    let out = TempDir::new("seam-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

    // S3: nothing said either way about the pixels, so both were written and
    // counted, and the report carries the setting that reads the count.
    assert_eq!((report.burned_in, report.unjudged), (0, 2));
    assert_eq!(report.on_unknown, "write");
    assert_eq!(report.files, 2);

    // S1: and the same run removed the accession number and named it.
    for tag in [
        dicom_core::Tag(0x0008, 0x0050),
        dicom_core::Tag(0x0018, 0x1000),
    ] {
        let named = format!("({:04X},{:04X}) removed", tag.group(), tag.element());
        assert_eq!(report.changes.get(&named), Some(&2), "{named}");
    }

    // One row, carrying both: the counts the close wrote and the categories
    // the insert wrote, with `ids` among them.
    assert_eq!(
        row_counts(&mut reg, report.release_id),
        (0, 2, "write".into())
    );
    let store = reg.store();
    let sql = format!(
        "SELECT categories FROM {} WHERE id = {}",
        store.qualified("release"),
        report.release_id
    );
    let rows = store.query(&sql, &[]).unwrap();
    assert_eq!(
        rows[0].text(0).unwrap(),
        "patient,trial,provider,institution,times,ids"
    );
}

// ------------------------------------------- record 35 finding 1: identity
//
// A series carries the subject its own files named; a study carries the
// subject the study's files named. An archive that re-linked a series leaves
// the two disagreeing, and then a subject owns series and no study at all.
// The session layer refuses to build a session for such a subject and files
// `identity.no_study`. The release used to read the label off the study
// anyway, which borrowed a session from whoever owned the study, and wrote a
// disowned subject into `ses-` directories no scheme produced.

/// Give the second study's series to a subject of its own, which is the shape
/// the re-run found: a subject with series and no study. Answers its code.
fn disown_a_series(reg: &mut Registry) -> String {
    use nils_registry::schema::table;
    use nils_registry::store::{Insert, Param};
    let store = reg.store();
    let code = "orphaned";
    let orphan = store
        .insert(
            &Insert::new(table("subject"), &["code", "created_at"]).returning(&["id"]),
            &[vec![Param::from(code), Param::from("2026-09-19T00:00:00Z")]],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    // The later of the two studies, so the earlier one still makes a session
    // for the subject that keeps it.
    let sql = format!(
        "SELECT se.id FROM {} se JOIN {} st ON st.id = se.study_id ORDER BY st.id DESC",
        store.qualified("series"),
        store.qualified("study")
    );
    let series = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
    store
        .execute(
            &format!(
                "UPDATE {} SET subject_id = {orphan} WHERE id = {series}",
                store.qualified("series")
            ),
            &[],
        )
        .unwrap();
    code.to_string()
}

/// Every `sub-*/ses-*` pair a tree holds, once, in order.
fn session_dirs(root: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = files_under(root)
        .iter()
        .filter_map(|p| {
            let parts: Vec<String> = p
                .strip_prefix(root)
                .ok()?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let subject = parts.iter().find(|c| c.starts_with("sub-"))?;
            let session = parts.iter().find(|c| c.starts_with("ses-"))?;
            Some((subject.clone(), session.clone()))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every `sub-*/ses-*` pair the scheme produced, from the cache.
fn derived_sessions(reg: &mut Registry, scheme: &SessionScheme) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = nils_session::sessions_of(reg.store(), scheme, None)
        .unwrap()
        .iter()
        .map(|c| (format!("sub-{}", c.code), format!("ses-{}", c.name())))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn a_subject_with_series_and_no_study_is_not_written_into_a_tree_as_a_session() {
    let source = tree();
    let home_dir = TempDir::new("disowned-home");
    let out = TempDir::new("disowned-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let code = disown_a_series(&mut reg);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

    // The session layer says the subject is on no timeline.
    let anchors =
        nils_session::Anchors::resolve(&mut reg, &scheme, std::collections::BTreeMap::new())
            .unwrap();
    let rebuilt = nils_session::ensure(&mut reg, &scheme, &anchors, None, true).unwrap();
    assert_eq!(rebuilt.without_a_study, 1, "{rebuilt:?}");
    let sessions = nils_session::sessions_of(reg.store(), &scheme, Some(&code)).unwrap();
    assert!(sessions.is_empty(), "{sessions:?}");

    // And the release says the same thing rather than inventing a session:
    // the subject is refused, named, and counted as left out.
    assert_eq!(report.without_a_session.get(&code), Some(&1), "{report:?}");
    assert_eq!(report.left_out, 1, "{report:?}");
    assert_eq!(report.subjects, 1, "{report:?}");
    assert_eq!(report.stacks, 1, "{report:?}");
    let written: Vec<String> = files_under(out.path())
        .iter()
        .map(|p| p.strip_prefix(out.path()).unwrap().display().to_string())
        .collect();
    assert!(
        written.iter().all(|p| !p.contains(&code)),
        "the disowned subject is in the tree: {written:?}"
    );
    assert!(
        written.iter().all(|p| !p.contains("ses-unknown")),
        "a session nobody derived: {written:?}"
    );

    // Never a silent drop: the row says which stack and why.
    let store = reg.store();
    let sql = format!(
        "SELECT kind, why FROM {} WHERE release_id = {}",
        store.qualified("release_absent"),
        report.release_id
    );
    let rows = store.query(&sql, &[]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text(0).unwrap(), "no_session");
    assert!(
        rows[0]
            .text(1)
            .unwrap()
            .contains("owns series and no study"),
        "{}",
        rows[0].text(1).unwrap()
    );
}

#[test]
fn the_trees_session_directories_are_exactly_the_sessions_the_scheme_produced() {
    for disowned in [false, true] {
        let source = tree();
        let home_dir = TempDir::new("dirs-home");
        let out = TempDir::new("dirs-out");
        let (_home, mut reg) = registry(&home_dir, &source);
        if disowned {
            disown_a_series(&mut reg);
        }
        let policy = Policy::default();
        let scheme = SessionScheme::default();
        run::run(&mut reg, &settings(out.path(), &policy, &scheme)).unwrap();

        let written = session_dirs(out.path());
        let derived = derived_sessions(&mut reg, &scheme);
        // The scheme produces two sessions of the one subject that owns a
        // study either way: a series that changed hands is still filed under
        // the study that declared it, so the timeline does not move.
        assert_eq!(derived.len(), 2, "disowned {disowned}: {derived:?}");
        // Every directory in the tree is one of them, and under the subject
        // whose session it is. A release writes no session of its own.
        for pair in &written {
            assert!(
                derived.contains(pair),
                "disowned {disowned}: {pair:?} is in no scheme, of {derived:?}"
            );
        }
        match disowned {
            // Nothing refused, so the tree holds all of them.
            false => assert_eq!(written, derived, "{written:?}"),
            // The disowned subject's stack is refused, so its study's session
            // has nothing to write and the other one stands alone.
            true => assert_eq!(written.len(), 1, "{written:?}"),
        }
    }
}

#[test]
fn the_ask_and_select_agree_on_how_many_subjects_a_selection_holds() {
    for disowned in [false, true] {
        let source = tree();
        let home_dir = TempDir::new("agree-home");
        let (_home, mut reg) = registry(&home_dir, &source);
        if disowned {
            disown_a_series(&mut reg);
        }
        // The fingerprints carry the subject, so they are built after the
        // series changed hands, as a digest of that archive would have.
        classified(&mut reg, &source);

        // What the selection reaches, which is what `nils select` prints.
        let reached = run::preview(reg.store(), &Selection::default())
            .unwrap()
            .subjects;
        assert_eq!(reached, if disowned { 2 } else { 1 }, "{disowned}");

        // And what an ask over every stack holds.
        let catalog = nils_catalog::Catalog::build(&mut reg, pack()).unwrap();
        let text = serde_json::json!({
            "ast_version": 1,
            "sets": {"every": {"grain": "stack"}},
            "out": {"set": "every", "level": "count"}
        })
        .to_string();
        let prepared = nils_ask::prepare(
            nils_ask::parse(&text).unwrap(),
            &catalog,
            &nils_ask::validate::Scope::default(),
        )
        .unwrap();
        let store = reg.store();
        let ctx = nils_ask::compile::Context {
            names: &catalog,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: 0,
            scheme_digest: SessionScheme::default().digest(),
            after: None,
            limit: None,
        };
        let compiled =
            nils_ask::compile::compile(&prepared.ask, &prepared.validated, &ctx).unwrap();
        let answer = nils_ask::exec::run(
            store,
            &compiled,
            nils_ask::exec::Bounds {
                timeout_ms: 20_000,
                max_rows: 5_000,
                max_bytes: 4 * 1024 * 1024,
            },
        )
        .unwrap();
        assert_eq!(answer.columns, vec!["rows", "subjects"]);
        assert_eq!(
            answer.rows[0].int(1).unwrap(),
            reached,
            "disowned {disowned}: an ask and a selection count one archive"
        );
    }
}
