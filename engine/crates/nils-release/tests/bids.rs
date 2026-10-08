// SPDX-License-Identifier: AGPL-3.0-only

//! The BIDS layout from end to end
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.2 to §9.6).
//!
//! What is proved here is the tree: the names the standard admits, where the
//! rest went, the files that make it a dataset, and that a re-run of it pays
//! only for what changed. The conversion needs `dcm2niix`, which is a
//! prerequisite of a deployment rather than of a checkout, so the tests that
//! need one say so and stop when it is absent.

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::bids::place::{Localizers, Options, Synthetic};
use nils_release::policy::Policy;
use nils_release::run::{self, Layout, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a bids test key of some length!!";

/// One session of one person: a T1w, a FLAIR and a localizer.
fn tree() -> TempDir {
    let dir = TempDir::new("bids");
    let series = [
        ("1", "t1_mprage_sag", "MPRAGE"),
        ("2", "t2_flair_tra", "FLAIR"),
        ("3", "localizer", "LOC"),
    ];
    for (n, description, protocol) in series {
        for slice in 1..=4 {
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr(&format!("1.2.3.{n}"), &format!("1.2.3.{n}.0"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, "031415"),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                synth::text(tags::PROTOCOL_NAME, VR::LO, protocol),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                // Geometry and pixels, because a converter reads them and
                // every other reader we have stops before them.
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

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    })
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

/// The converter, or nothing and a word about why.
fn converter() -> Option<nils_release::bids::convert::Converter> {
    match nils_release::bids::convert::Converter::find(Path::new("dcm2niix")) {
        Ok(c) => Some(c),
        Err(_) => {
            eprintln!("dcm2niix is not installed; the conversion half is skipped");
            None
        }
    }
}

fn settings<'a>(
    out: &'a Path,
    policy: &'a Policy,
    scheme: &'a SessionScheme,
    places: Options,
    converter: Option<&'a nils_release::bids::convert::Converter>,
) -> run::Settings<'a> {
    run::Settings {
        name: "a cohort",
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
        layout: Layout::Bids,
        naming: nils_release::name::Naming::Full,
        places,
        converter,
        compress: true,
        observations: &[],
        authors: &[],
    }
}

fn files_under(root: &Path) -> Vec<String> {
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
            } else if let Ok(rest) = p.strip_prefix(root) {
                out.push(rest.display().to_string());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_bids_release_needs_a_converter_and_says_so_before_it_starts() {
    // §9.6. A converter is not a thing to discover halfway through an archive,
    // and v0 discovers it per stack, in a worker process, as N identical
    // failures.
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let e = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, Options::default(), None),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("§9.6"), "{e}");
    assert!(
        files_under(out.path()).is_empty(),
        "and nothing was written"
    );
}

#[test]
fn the_tree_is_a_dataset_and_not_only_a_pile_of_named_files() {
    // §9.5. v0 writes none of these, which is why its tree is not a dataset
    // rather than an invalid one.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    let written = files_under(out.path());
    for required in ["dataset_description.json", "participants.tsv", "README"] {
        assert!(written.contains(&required.to_string()), "{written:?}");
    }
    let description: serde_json::Value = serde_json::from_slice(
        &std::fs::read(out.path().join("dataset_description.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(description["BIDSVersion"], "1.11.1");
    assert_eq!(
        description["GeneratedBy"][0]["DatasetVersion"],
        report.version
    );
    // §9.6: a tree says which converter made it.
    assert!(
        description["GeneratedBy"][0]["Container"]["Converter"]
            .as_str()
            .unwrap()
            .contains("dcm2nii"),
        "{description}"
    );

    // §9.4: the date is in the standard's own column, so anything joining on
    // it reads a column rather than parsing a directory name.
    let scans: Vec<String> = written
        .iter()
        .filter(|f| f.ends_with("_scans.tsv"))
        .cloned()
        .collect();
    assert_eq!(scans.len(), 1, "{written:?}");
    let text = std::fs::read_to_string(out.path().join(&scans[0])).unwrap();
    assert!(
        text.starts_with("filename\tacq_time\tnils_name\n"),
        "{text}"
    );
    assert!(text.contains("2022-01-15T03:14:15"), "{text}");
}

#[test]
fn what_the_standard_admits_gets_the_standards_name() {
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    let names: Vec<String> = files_under(out.path())
        .into_iter()
        .filter(|f| f.ends_with(".nii.gz"))
        .collect();
    assert!(
        names.iter().any(|n| n.contains("_T1w.nii.gz")),
        "a T1w by its suffix: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("_FLAIR.nii.gz")),
        "and a FLAIR: {names:?}"
    );
    assert!(
        names.iter().all(|n| n.starts_with("sub-")),
        "every name is the standard's: {names:?}"
    );
    // §9.3: the localizer went to `sourcedata/`, as DICOM, by default.
    assert_eq!(
        report.placements.get("localizers").map(String::as_str),
        Some("sourcedata")
    );
    assert!(
        files_under(out.path())
            .iter()
            .any(|f| f.starts_with("sourcedata/") && f.ends_with(".dcm")),
        "the localizer is in sourcedata as DICOM"
    );
}

#[test]
fn a_localizer_goes_where_the_release_said() {
    // §9.3 and Nima's own point: BIDS has no word for a localizer, and which
    // answer is right depends on who the dataset is for. A release records the
    // one it took, because a tree that does not say where it put its
    // localizers is a tree whose absence of localizers means nothing.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    for (choice, expected) in [
        (Localizers::Datatype, "localizer/"),
        (Localizers::Anat, "_localizer."),
    ] {
        let out = TempDir::new("bids-loc");
        let places = Options {
            localizers: choice,
            synthetic: Synthetic::Anat,
        };
        let report = run::run(
            &mut reg,
            &settings(out.path(), &policy, &scheme, places, Some(&converter)),
        )
        .unwrap();
        let written = files_under(out.path());
        assert!(
            written.iter().any(|f| f.contains(expected)),
            "{choice:?} put nothing at {expected}: {written:?}"
        );
        // And it says so, in the tree, where the standard does not know it.
        assert!(
            written.contains(&".bidsignore".to_string()),
            "{choice:?} needs a .bidsignore line: {written:?}"
        );
        assert_eq!(
            report.placements.get("localizers").map(String::as_str),
            Some(choice.name())
        );
    }

    // And dropped is nowhere, reported rather than silent.
    let out = TempDir::new("bids-drop");
    let places = Options {
        localizers: Localizers::Drop,
        synthetic: Synthetic::Anat,
    };
    let report = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, places, Some(&converter)),
    )
    .unwrap();
    assert!(report.routes.get("nowhere").copied().unwrap_or(0) > 0);
    assert!(
        !files_under(out.path())
            .iter()
            .any(|f| f.contains("localizer")),
        "nothing of it is in the tree"
    );
}

#[test]
fn a_re_run_of_a_bids_tree_writes_nothing_either() {
    // §8.6 is layout-independent, which it has to be: the place is a prefix of
    // every file of a stack, and in BIDS that prefix is a filename stem rather
    // than a directory.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(
        out.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );

    let first = run::run(&mut reg, &s).unwrap();
    assert!(first.added > 0);
    let before: Vec<(String, std::time::SystemTime)> = files_under(out.path())
        .into_iter()
        .map(|f| {
            let when = std::fs::metadata(out.path().join(&f))
                .unwrap()
                .modified()
                .unwrap();
            (f, when)
        })
        .collect();

    let second = run::run(&mut reg, &s).unwrap();
    assert_eq!(second.added, 0);
    assert_eq!(second.rewritten, 0);
    assert_eq!(second.written, 0, "not one file was written again");
    assert_eq!(second.unchanged, first.added);
    // The dataset files are written every version, because they name the
    // version; the images are not touched at all.
    let after: Vec<(String, std::time::SystemTime)> = files_under(out.path())
        .into_iter()
        .map(|f| {
            let when = std::fs::metadata(out.path().join(&f))
                .unwrap()
                .modified()
                .unwrap();
            (f, when)
        })
        .collect();
    for ((was, when), (is, now)) in before.iter().zip(&after) {
        assert_eq!(was, is);
        if was.ends_with(".nii.gz") || was.ends_with(".dcm") {
            assert_eq!(when, now, "{was} was written again");
        }
    }
}

#[test]
fn a_qc_decision_renames_a_bids_file_rather_than_writing_it_again() {
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(
        out.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );
    run::run(&mut reg, &s).unwrap();
    let before: Vec<String> = files_under(out.path())
        .into_iter()
        .filter(|f| f.ends_with(".nii.gz"))
        .collect();

    // Somebody looks at the stacks and says they are spinal cord, which
    // changes the `acq-` label of every name and the content of none.
    {
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
    let after = run::run(&mut reg, &s).unwrap();
    assert!(after.moved > 0, "{after:?}");
    assert_eq!(after.written, 0, "renamed, not rewritten");
    let now: Vec<String> = files_under(out.path())
        .into_iter()
        .filter(|f| f.ends_with(".nii.gz"))
        .collect();
    assert_eq!(now.len(), before.len());
    assert_ne!(now, before, "the tree is named differently");
    // v0's prefix for the spine (record 55 C4), and the FLAIR is said once,
    // by its suffix.
    assert!(
        now.iter().all(|n| n.contains("acq-SCAx")),
        "and the new name says so: {now:?}"
    );
    assert!(now.iter().all(|n| !acq_of(n).contains("FLAIR")), "{now:?}");
}

#[test]
fn a_conversion_the_converter_refuses_is_planned_as_refused_next_time() {
    // Found by walking a cohort through its life: a stack the converter will
    // not convert falls back to `sourcedata/` as DICOM, and every re-run used
    // to plan the raw route again, fail again, and rewrite it. That is the
    // incremental promise broken for exactly the stacks that cost the most.
    //
    // Nothing the answer depends on has changed between runs, and the
    // converter is in the content digest, so the fallback is planned rather
    // than retried. An upgraded converter changes the digest and it is tried
    // again, which is what an upgrade means.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let s = settings(
        out.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );
    let first = run::run(&mut reg, &s).unwrap();
    let second = run::run(&mut reg, &s).unwrap();
    assert_eq!(second.written, 0, "the first re-run is already free");
    assert_eq!(second.rewritten, 0);
    assert_eq!(second.moved, 0, "and nothing moved either");
    assert_eq!(second.unchanged, first.added + first.rewritten);
}

#[test]
fn the_tree_carries_the_clinical_layer_with_its_real_dates() {
    // Wave 4a §7.4. participants.tsv carries the sex and the age at the
    // first session; each sessions.tsv the age at the session and the
    // nearest observation of each kind the release names, with its distance
    // in days and its date, which is the real date (record 38 S3).
    use nils_registry::clinical::{self, Vocabulary};
    use nils_registry::schema;
    use nils_registry::store::{Insert, Param};
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    {
        let store = reg.store();
        let v = Vocabulary::parse(
            "vocabulary:\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric, primary: true}\n    - {name: Relapse, category: event}\n    - {name: Delivery, category: event, primary: true, sensitive: true}\n",
        )
        .unwrap();
        clinical::load(store, &v).unwrap();
        let edss = clinical::kind_named(store, "EDSS").unwrap().unwrap().id;
        let relapse = clinical::kind_named(store, "Relapse").unwrap().unwrap().id;
        let subject = store
            .query(
                &format!(
                    "SELECT id FROM {} ORDER BY id LIMIT 1",
                    store.qualified("subject")
                ),
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        store
            .execute(
                &format!(
                    "UPDATE {} SET birth_date = '1980-01-01', sex = 'F' WHERE id = {subject}",
                    store.qualified("subject"),
                ),
                &[],
            )
            .unwrap();
        // EDSS 3.5 five days before the scan (2022-01-15), 4.0 six weeks
        // after: the nearest is the earlier one. A relapse a year before.
        for (kind, date, number) in [
            (edss, "2022-01-10", Some(3.5)),
            (edss, "2022-02-26", Some(4.0)),
            (relapse, "2021-01-15", None),
        ] {
            store
                .insert(
                    &Insert::new(
                        schema::table("event"),
                        &[
                            "subject_id",
                            "observation_type_id",
                            "event_date",
                            "number",
                            "created_at",
                        ],
                    ),
                    &[vec![
                        Param::Int(subject),
                        Param::Int(kind),
                        Param::from(date),
                        number.map_or(Param::Null, Param::Double),
                        Param::from("2026-09-06T00:00:00Z"),
                    ]],
                )
                .unwrap();
        }
    }
    // Record 38 S3: the date is the date, whatever labels the session. A
    // tree whose sessions are numbered carries the same real dates in its
    // clinical columns as one labelled by the date.
    let by_date = SessionScheme::default();
    let ordinal = SessionScheme {
        naming: nils_registry::session::Naming::Ordinal,
        ..SessionScheme::default()
    };
    let kinds = ["EDSS".to_string(), "Relapse".to_string()];
    for (label, scheme) in [("date", &by_date), ("ordinal", &ordinal)] {
        let out = TempDir::new("bids-clinical");
        let policy = Policy::default();
        let mut settings = settings(
            out.path(),
            &policy,
            scheme,
            Options::default(),
            Some(&converter),
        );
        settings.observations = &kinds;
        let report = run::run(&mut reg, &settings).unwrap();
        let written = files_under(out.path());
        let participants = std::fs::read_to_string(out.path().join("participants.tsv")).unwrap();
        let mut lines = participants.lines();
        let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let values: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let cell = |name: &str| -> Option<&str> {
            header.iter().position(|h| *h == name).map(|i| values[i])
        };
        assert_eq!(cell("sex"), Some("F"), "{label}: {participants}");
        assert_eq!(cell("age"), Some("42"), "{label}: {participants}");
        let sessions = written
            .iter()
            .find(|f| f.ends_with("_sessions.tsv"))
            .unwrap_or_else(|| panic!("{label}: {written:?}"));
        let text = std::fs::read_to_string(out.path().join(sessions)).unwrap();
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let values: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let cell = |name: &str| -> Option<&str> {
            header.iter().position(|h| *h == name).map(|i| values[i])
        };
        assert_eq!(cell("age"), Some("42"), "{label}: {text}");
        assert_eq!(cell("edss"), Some("3.5"), "{label}: {text}");
        assert_eq!(cell("edss_days"), Some("-5"), "{label}: {text}");
        assert_eq!(cell("relapse"), Some("yes"), "{label}: {text}");
        assert_eq!(cell("relapse_days"), Some("-365"), "{label}: {text}");
        assert_eq!(cell("edss_date"), Some("2022-01-10"), "{label}: {text}");
        assert_eq!(cell("relapse_date"), Some("2021-01-15"), "{label}: {text}");
        // The sensitive kind is not a column, and naming it is refused.
        assert!(
            !header.iter().any(|h| h.starts_with("delivery")),
            "{label}: {text}"
        );
        assert_eq!(
            report.clinical.get("sex"),
            Some(&1),
            "{:?}",
            report.clinical
        );
        assert_eq!(
            report.clinical.get("nearest EDSS"),
            Some(&1),
            "{:?}",
            report.clinical
        );
    }
}

#[test]
fn a_sensitive_kind_is_refused_by_name_and_left_out_by_default() {
    // Wave 4a §7.4: the pack marks a kind sensitive, and no release writes
    // it. Named, the release refuses before it plans anything, whatever the
    // layout; unnamed, the default list of primary kinds leaves it out (the
    // tree half of that is in the test above).
    use nils_registry::clinical::{self, Vocabulary};
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-sensitive");
    let (_home, mut reg) = registry(&home_dir, &source);
    let v = Vocabulary::parse(
        "vocabulary:\n  observation_types:\n    - {name: Delivery, category: event, primary: true, sensitive: true}\n",
    )
    .unwrap();
    clinical::load(reg.store(), &v).unwrap();
    assert!(
        clinical::kind_named(reg.store(), "delivery")
            .unwrap()
            .unwrap()
            .sensitive
    );
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let kinds = ["Delivery".to_string()];
    let mut settings = settings(out.path(), &policy, &scheme, Options::default(), None);
    settings.layout = Layout::Descriptive;
    settings.observations = &kinds;
    let e = run::run(&mut reg, &settings).unwrap_err().to_string();
    assert!(e.contains("sensitive"), "{e}");
    assert!(files_under(out.path()).is_empty(), "nothing was written");
    let unknown = ["Nope".to_string()];
    settings.observations = &unknown;
    let e = run::run(&mut reg, &settings).unwrap_err().to_string();
    assert!(e.contains("names no observation kind"), "{e}");
}

/// Put one value of one axis on every stack, as a person deciding would.
fn decide(reg: &mut Registry, axis: &str, value: &str) {
    let store = reg.store();
    let table = store.qualified("classification_axis");
    store
        .execute(&format!("DELETE FROM {table} WHERE axis = '{axis}'"), &[])
        .unwrap();
    store
        .execute(
            &format!(
                "INSERT INTO {table} (stack_id, axis, value, confidence, tier) \
                 SELECT id, '{axis}', '{value}', 1.0, 'decided' FROM {}",
                store.qualified("stack")
            ),
            &[],
        )
        .unwrap();
}

#[test]
fn the_body_part_is_in_the_name_and_in_the_sidecar() {
    // Record 37 S6. The two are not in conflict and the study says why:
    // `BodyPart` is where a reader looks the fact up, and `acq-` is what stops
    // a brain and a spine acquisition of one session overwriting each other,
    // which is 22 pairs of an archive.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    decide(&mut reg, "body_part", "spine");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    let sidecars: Vec<String> = files_under(out.path())
        .into_iter()
        .filter(|f| f.ends_with(".json") && f.starts_with("sub-"))
        .collect();
    assert!(!sidecars.is_empty(), "the converter writes a sidecar");
    for file in &sidecars {
        assert!(file.contains("acq-SCAx"), "the name says it too: {file}");
        let text = std::fs::read_to_string(out.path().join(file)).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["BodyPart"], serde_json::Value::from("spine"), "{file}");
    }
}

#[test]
fn an_axis_the_pack_declares_reaches_a_name_without_the_engine_learning_it() {
    // Record 37 S6, and the fault it fixes: the axes `acq-` could read were
    // hard-coded, so the quality axis of S5, which says what a file claims is
    // wrong with its own image, could not reach a filename at all. Two stacks
    // that agree on everything and disagree on whether the image is whole are
    // not interchangeable.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    decide(&mut reg, "quality", "Distorted");
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    let names: Vec<String> = files_under(out.path())
        .into_iter()
        .filter(|f| f.starts_with("sub-") && f.ends_with(".nii.gz"))
        .collect();
    assert!(!names.is_empty());
    assert!(
        names.iter().all(|n| n.contains("Distorted")),
        "the pack declared it and the name carries it: {names:?}"
    );
}

#[test]
fn the_full_style_spells_the_slots_and_the_minimal_one_only_what_separates() {
    // Record 55 C4, ruled 2026-10-08: both styles, as a release's option.
    // The full style spells v0's slots in `acq-` and the contrast in `ce-`
    // only; the minimal style writes no `acq-` where nothing shares a name.
    // Both carry everything in the sidecar's `NILS` object and the
    // descriptive name in `scans.tsv`.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    // The axis stores the label and the name is built from the identity.
    decide(&mut reg, "post_contrast", "1");
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let full = TempDir::new("bids-out");
    let report = run::run(
        &mut reg,
        &settings(
            full.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();
    assert_eq!(report.naming, "full");
    let minimal = TempDir::new("bids-minimal");
    let mut s = settings(
        minimal.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );
    s.name = "a cohort, minimal";
    s.naming = nils_release::name::Naming::Minimal;
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.naming, "minimal");

    let named = |root: &Path| -> Vec<String> {
        files_under(root)
            .into_iter()
            .filter(|f| f.starts_with("sub-") && f.ends_with(".nii.gz"))
            .collect()
    };
    let long = named(full.path());
    let short = named(minimal.path());
    assert!(!long.is_empty() && long.len() == short.len());
    assert!(
        long.iter()
            .all(|n| n.contains("_ce-contrast_") && n.contains("_acq-")),
        "{long:?}"
    );
    assert!(
        long.iter().all(|n| !acq_of(n).contains("CE")),
        "the contrast is said once, by its entity: {long:?}"
    );
    assert!(
        short
            .iter()
            .all(|n| n.contains("_ce-contrast_") && !n.contains("_acq-")),
        "nothing shares a name, so no acq-: {short:?}"
    );

    // The sidecar's `NILS` object, in both.
    for root in [full.path(), minimal.path()] {
        let sidecars: Vec<String> = files_under(root)
            .into_iter()
            .filter(|f| f.starts_with("sub-") && f.ends_with(".json"))
            .collect();
        assert!(!sidecars.is_empty());
        for file in &sidecars {
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(root.join(file)).unwrap()).unwrap();
            let card = &doc["NILS"];
            assert!(
                card["DescriptiveName"]
                    .as_str()
                    .is_some_and(|n| n.contains("_CE")),
                "{file}: {card}"
            );
            assert_eq!(
                card["Axes"]["post_contrast"]["values"],
                serde_json::json!(["given"]),
                "{file}"
            );
            assert!(
                card["Axes"]["post_contrast"]["tier"].is_string(),
                "{file}: {card}"
            );
            assert!(card["Axes"]["base"]["values"].is_array(), "{file}: {card}");
            // What this fixture's files state; the timings are the card's
            // unit test's.
            for key in [
                "SliceThicknessMm",
                "Matrix",
                "VoxelSizeMm",
                "FieldOfViewMm",
                "NumberOfImages",
                "AcquisitionType",
                "Manufacturer",
            ] {
                assert!(
                    !card["Acquisition"][key].is_null(),
                    "{file}: {key} in {card}"
                );
            }
            // Never the station, which names a place.
            assert!(card["Acquisition"].get("StationName").is_none());
        }
        // And the descriptive name beside every file in `scans.tsv`.
        let scans: Vec<String> = files_under(root)
            .into_iter()
            .filter(|f| f.ends_with("_scans.tsv"))
            .collect();
        assert!(!scans.is_empty());
        for f in scans {
            let text = std::fs::read_to_string(root.join(&f)).unwrap();
            assert!(
                text.starts_with("filename\tacq_time\tnils_name\n"),
                "{text}"
            );
            assert!(
                text.lines()
                    .skip(1)
                    .all(|l| l.split('\t').nth(2).is_some_and(|n| !n.is_empty())),
                "{text}"
            );
        }
        assert!(root.join("scans.json").is_file(), "the column is described");
    }
}

/// One session with two names more than one stack wants (record 37, S2).
///
/// Two MPRAGE series that agree on everything a fingerprint holds, which is a
/// site that scanned one protocol twice; and two FLAIR series that agree on
/// every axis and differ in what they cover, which is the commonest residual
/// in the archive. Both pairs build one BIDS name each. Nothing here is read
/// from a corpus: the shapes are the ones the 2026-09-19 studies described.
fn colliding() -> TempDir {
    let dir = TempDir::new("bids-collide");
    let series = [
        ("1", "t1_mprage_sag", "T1 MPRAGE", "3D", 4),
        ("2", "t1_mprage_sag", "T1 MPRAGE", "3D", 4),
        ("3", "t2_flair_tra", "T2 FLAIR", "2D", 4),
        ("4", "t2_flair_tra", "T2 FLAIR", "2D", 6),
    ];
    for (n, description, protocol, acquisition, slices) in series {
        for slice in 1..=slices {
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr("1.2.3.0", &format!("1.2.3.{n}.0"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, "031415"),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                synth::text(tags::PROTOCOL_NAME, VR::LO, protocol),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, acquisition),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::BODY_PART_EXAMINED, VR::CS, "BRAIN"),
                synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                synth::text(tags::SERIES_NUMBER, VR::IS, n),
                synth::text(tags::ECHO_TIME, VR::DS, "3"),
                synth::text(tags::REPETITION_TIME, VR::DS, "2000"),
                synth::text(tags::FLIP_ANGLE, VR::DS, "9"),
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
                // S1 reads the coverage from here, so the fixture says where
                // its slices are the way a scanner does.
                synth::text(tags::SLICE_LOCATION, VR::DS, &slice.to_string()),
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

#[test]
fn a_site_that_scanned_one_protocol_twice_still_gets_run_indices() {
    // Record 37 S2. The case `run-` exists for is not lost by the test that
    // stops it being written everywhere: two series that agree on every fact
    // the fingerprint holds are one acquisition made again, and that is what
    // the entity says.
    let Some(converter) = converter() else { return };
    let source = colliding();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    let written = files_under(out.path());
    // The image itself, not its sidecar, which carries the same stem.
    let runs: Vec<&String> = written
        .iter()
        .filter(|f| f.contains("_run-") && f.ends_with(".nii.gz"))
        .collect();
    assert_eq!(runs.len(), 2, "{written:?}");
    assert!(
        runs.iter()
            .all(|f| f.contains("MPRAGE") && f.ends_with("_T1w.nii.gz")),
        "{runs:?}"
    );
    assert!(runs.iter().any(|f| f.contains("_run-1_")), "{runs:?}");
    assert!(runs.iter().any(|f| f.contains("_run-2_")), "{runs:?}");
    assert_eq!(report.repeats, 2, "{report:?}");
}

#[test]
fn two_acquisitions_that_want_one_name_are_both_named_by_what_differs() {
    // Wave 7a §8.1. The pair differs in what it covers, so neither is refused
    // its name and neither is a run: each says how many slices it has,
    // inside `acq-`, and nobody is asked, because the engine saw what
    // differs and said it. Record 37 S2 refused them both before.
    let Some(converter) = converter() else { return };
    let source = colliding();
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();

    // Two names were shared, one by a repeat and one by two acquisitions.
    assert_eq!(report.shared_names, 2, "{report:?}");
    assert_eq!(report.repeats, 2, "{report:?}");
    assert_eq!(report.not_repeats, 2, "{report:?}");
    assert_eq!(report.numbered, 0, "{report:?}");

    let written = files_under(out.path());
    assert!(
        !written
            .iter()
            .any(|f| f.contains("FLAIR") && f.starts_with("sourcedata/")),
        "nothing is refused its name: {written:?}"
    );
    let flair: Vec<&String> = written
        .iter()
        .filter(|f| f.contains("FLAIR") && f.ends_with(".nii.gz"))
        .collect();
    assert_eq!(flair.len(), 2, "{written:?}");
    assert!(flair.iter().all(|f| !f.contains("_run-")), "{flair:?}");
    assert!(
        flair.iter().any(|f| f.contains("4sl_FLAIR"))
            && flair.iter().any(|f| f.contains("6sl_FLAIR")),
        "the slice count, in acq-: {flair:?}"
    );

    // The release's record lists both, with the property and the values.
    let mut decided: Vec<(String, String)> = report
        .decided
        .iter()
        .flat_map(|d| {
            d.marks
                .iter()
                .map(|m| (m.property.clone(), m.value.clone()))
        })
        .collect();
    decided.sort();
    assert_eq!(
        decided,
        [
            ("Slices".to_string(), "4".to_string()),
            ("Slices".to_string(), "6".to_string())
        ],
        "{:?}",
        report.decided
    );
    assert!(report.decided.iter().all(|d| d.name.contains("_FLAIR")));
    assert!(
        shared_differs(&mut reg).is_empty(),
        "nobody is asked about a difference the name says"
    );
}

/// One series of a pair: what a console recorded for it, where its first
/// slice sits, when it was acquired, and the number the scanner gave it.
#[derive(Clone, Copy)]
struct Twin<'a> {
    description: &'a str,
    protocol: &'a str,
    first_slice: f64,
    acquired: Option<&'a str>,
    /// Sagittal slices, and how far down the spine the station sits: the
    /// slice location is the position along the normal, left to right, and
    /// this moves the stack in the plane of its slices, which it never sees.
    sagittal_at: Option<f64>,
    /// The slice thickness, in millimetres.
    thickness: &'a str,
}

fn twin<'a>(description: &'a str, protocol: &'a str) -> Twin<'a> {
    Twin {
        description,
        protocol,
        first_slice: 1.0,
        acquired: None,
        sagittal_at: None,
        thickness: "1.0",
    }
}

/// Two series of one session built from the survey's characteristics and from
/// no archive: one protocol step, the same geometry, the same timings, the
/// same image type, written twice (record 37 S4's fixture, and record 38's).
fn twins(one: Twin, two: Twin) -> TempDir {
    scans(&[one, two])
}

/// Series of one session, numbered from 1 in the order given.
fn scans(series: &[Twin]) -> TempDir {
    let dir = TempDir::new("bids-twins");
    for (i, t) in series.iter().enumerate() {
        let n = (i + 1).to_string();
        let n = n.as_str();
        for slice in 1..=4 {
            let at = t.first_slice + f64::from(slice - 1);
            let (orientation, position) = match t.sagittal_at {
                None => ("1\\0\\0\\0\\1\\0", format!("0\\0\\{at}")),
                Some(z) => ("0\\1\\0\\0\\0\\-1", format!("{at}\\0\\{z}")),
            };
            let sop = format!("1.2.3.{n}.{slice}");
            let mut e = synth::minimal_mr(&format!("1.2.3.{n}"), &format!("1.2.3.{n}.0"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "19800101-1234"),
                synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
                synth::text(tags::SERIES_TIME, VR::TM, "031415"),
                synth::text(tags::SERIES_NUMBER, VR::IS, n),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, t.description),
                synth::text(tags::PROTOCOL_NAME, VR::LO, t.protocol),
                synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                synth::text(tags::ECHO_TIME, VR::DS, "3"),
                synth::text(tags::REPETITION_TIME, VR::DS, "2000"),
                synth::text(tags::FLIP_ANGLE, VR::DS, "9"),
                synth::us(tags::ROWS, 16),
                synth::us(tags::COLUMNS, 16),
                synth::us(tags::BITS_ALLOCATED, 16),
                synth::us(tags::BITS_STORED, 12),
                synth::us(tags::HIGH_BIT, 11),
                synth::us(tags::PIXEL_REPRESENTATION, 0),
                synth::us(tags::SAMPLES_PER_PIXEL, 1),
                synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                synth::text(tags::PIXEL_SPACING, VR::DS, "1.0\\1.0"),
                synth::text(tags::SLICE_THICKNESS, VR::DS, t.thickness),
                synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, orientation),
                synth::text(tags::IMAGE_POSITION_PATIENT, VR::DS, &position),
                synth::text(tags::SLICE_LOCATION, VR::DS, &at.to_string()),
                synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                synth::bytes(tags::PIXEL_DATA, VR::OW, vec![0x40u8; 16 * 16 * 2]),
            ]);
            if let Some(time) = t.acquired {
                e.push(synth::text(tags::ACQUISITION_TIME, VR::TM, time));
            }
            dir.file(
                &format!("{n}/{slice}"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
    }
    dir
}

/// The names of the released anatomical images, sorted.
fn anat_names(root: &Path) -> Vec<String> {
    files_under(root)
        .into_iter()
        .filter(|f| f.contains("/anat/") && f.ends_with(".nii.gz"))
        .collect()
}

/// The `acq-` label of a name, or nothing.
fn acq_of(name: &str) -> &str {
    name.split("_acq-")
        .nth(1)
        .and_then(|rest| rest.split('_').next())
        .unwrap_or("")
}

/// What a name says before its suffix.
fn before_suffix(name: &str) -> &str {
    name.rsplit_once('_').map(|(head, _)| head).unwrap_or(name)
}

fn review_kinds(reg: &mut Registry) -> Vec<String> {
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

/// Release one source tree into a BIDS tree, and hand back what happened.
fn released(
    source: &TempDir,
    home_dir: &TempDir,
    out: &TempDir,
    converter: &nils_release::bids::convert::Converter,
) -> (Registry, run::Report) {
    let (_home, mut reg) = registry(home_dir, source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let report = run::run(
        &mut reg,
        &settings(
            out.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(converter),
        ),
    )
    .unwrap();
    (reg, report)
}

/// What each open `release.shared_name` question says differs.
fn shared_differs(reg: &mut Registry) -> Vec<serde_json::Value> {
    let store = reg.store();
    let sql = format!(
        "SELECT evidence FROM {} WHERE kind = 'release.shared_name' ORDER BY id",
        store.qualified("review_item"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            let evidence: serde_json::Value =
                serde_json::from_str(r.text(0).unwrap()).unwrap_or_default();
            evidence["differs"].clone()
        })
        .collect()
}

/// A pair that is not a rescan and that nothing a name may spell separates:
/// both keep a BIDS name, told apart by a plain number inside `acq-` that is
/// never a run, nothing named by text, and a question only when `asked`.
fn numbered_with(one: Twin, two: Twin, differs: serde_json::Value, asked: bool) {
    let Some(converter) = converter() else { return };
    let source = twins(one, two);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (mut reg, report) = released(&source, &home_dir, &out, &converter);

    assert_eq!(report.shared_names, 1, "{report:?}");
    assert_eq!(report.repeats, 0, "{report:?}");
    assert_eq!(report.not_repeats, 2, "{report:?}");
    assert_eq!(report.numbered, 2, "{report:?}");
    let names = anat_names(out.path());
    assert_eq!(names.len(), 2, "both have a BIDS name: {names:?}");
    assert!(
        names.iter().any(|n| before_suffix(n).ends_with('1'))
            && names.iter().any(|n| before_suffix(n).ends_with('2')),
        "the number, in acq-: {names:?}"
    );
    assert!(names.iter().all(|n| !n.contains("_run-")), "{names:?}");
    let written = files_under(out.path());
    assert!(
        written.iter().all(|f| !f.starts_with("sourcedata/")),
        "nothing is refused: {written:?}"
    );
    assert!(
        written.iter().all(|f| !f.contains("Text")),
        "no name rests on text: {written:?}"
    );
    assert!(report.decided.iter().all(|d| {
        d.marks.len() == 1
            && d.marks[0].property == "number"
            && d.differs
                == differs
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
    }));
    match asked {
        true => assert_eq!(shared_differs(&mut reg), [differs]),
        false => assert!(shared_differs(&mut reg).is_empty()),
    }
    assert!(
        !review_kinds(&mut reg).contains(&"release.named_by_text".to_string()),
        "and no text question is raised"
    );
}

#[test]
fn one_protocol_measured_twice_is_numbered_in_the_order_it_was_made() {
    // Record 38 S2: the case `run-` exists for. Identical in everything, each
    // at its own moment, and series 2 was made first, so it is `run-1`.
    let Some(converter) = converter() else { return };
    let first = Twin {
        acquired: Some("102000"),
        ..twin("t1_mprage_sag", "T1 MPRAGE")
    };
    let earlier = Twin {
        acquired: Some("100500"),
        ..first
    };
    let source = twins(first, earlier);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (mut reg, report) = released(&source, &home_dir, &out, &converter);

    let names = anat_names(out.path());
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|n| n.contains("_run-1_")) && names.iter().any(|n| n.contains("_run-2_")),
        "the standard's answer: {names:?}"
    );
    assert!(names.iter().all(|n| !n.contains("Text")), "{names:?}");
    assert_eq!(report.repeats, 2, "{report:?}");
    assert!(shared_differs(&mut reg).is_empty());

    // `run-1` is the one acquired at 10:05, which is series 2.
    let sidecar = |run: &str| -> serde_json::Value {
        let name = names.iter().find(|n| n.contains(run)).unwrap();
        let json = out.path().join(name.replace(".nii.gz", ".json"));
        serde_json::from_str(&std::fs::read_to_string(json).unwrap()).unwrap()
    };
    assert_eq!(sidecar("_run-1_")["SeriesNumber"], 2, "{names:?}");
    assert_eq!(sidecar("_run-2_")["SeriesNumber"], 1, "{names:?}");
}

#[test]
fn a_pair_whose_series_description_differs_is_numbered_and_never_named_by_it() {
    // Record 38: inside one session a rescan's texts are identical. The text
    // makes no name; it refuses one, and a person decides.
    numbered_with(
        twin("t1_mprage_sag", "T1 MPRAGE"),
        twin("t1_mprage_sag_iso", "T1 MPRAGE"),
        serde_json::json!(["the series description"]),
        false,
    );
}

#[test]
fn the_counter_a_scanner_welds_on_a_rerun_step_is_a_different_text() {
    // Record 37 folded the counter away; record 38 compares the texts as they
    // were written, less case and whitespace.
    numbered_with(
        twin("t1_mprage_sag", "T1 MPRAGE"),
        twin("t1_mprage_sag", "T1 MPRAGE 2"),
        serde_json::json!(["the protocol name"]),
        false,
    );
}

#[test]
fn two_stations_that_differ_only_in_position_are_two_acquisitions() {
    // Record 38 S2's proof: every parameter, the slice count and the extent
    // agree, and the second station sits 200 mm further along.
    let upper = twin("t1_mprage_sag", "T1 MPRAGE");
    let lower = Twin {
        first_slice: -199.0,
        ..upper
    };
    numbered_with(upper, lower, serde_json::json!(["where it sits"]), false);
}

#[test]
fn two_series_made_at_one_moment_are_numbered_and_asked_about() {
    // One acquisition written twice is not a rescan, whatever its UIDs.
    let one = Twin {
        acquired: Some("102000"),
        ..twin("t1_mprage_sag", "T1 MPRAGE")
    };
    // The one case the engine sees nothing in: numbered, and asked about.
    numbered_with(
        one,
        one,
        serde_json::json!(["acquired at the same moment"]),
        true,
    );
}

#[test]
fn two_sagittal_stations_that_share_every_slice_location_are_two_acquisitions() {
    // The stations of a sagittal spine sit one above the other, in the plane
    // of their slices: the slice locations agree, and the images' positions
    // are 200 mm apart.
    let upper = Twin {
        sagittal_at: Some(0.0),
        ..twin("t2_tse_sag_spine", "T2 TSE SAG")
    };
    let lower = Twin {
        sagittal_at: Some(-200.0),
        ..upper
    };
    numbered_with(upper, lower, serde_json::json!(["where it sits"]), false);
}

/// Release one source tree into a BIDS tree under the minimal naming style.
fn released_minimal(
    source: &TempDir,
    home_dir: &TempDir,
    out: &TempDir,
    converter: &nils_release::bids::convert::Converter,
) -> (Registry, run::Report) {
    let (_home, mut reg) = registry(home_dir, source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let mut s = settings(
        out.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(converter),
    );
    s.naming = nils_release::name::Naming::Minimal;
    let report = run::run(&mut reg, &s).unwrap();
    (reg, report)
}

#[test]
fn a_difference_that_is_no_axis_is_named_by_its_property_and_value() {
    // Wave 7a §8.1, Nima's example: two scans of one protocol step, one cut
    // at 1 mm and one at 3 mm. Same name before; refused before; now each
    // says its thickness in its own slot of `acq-`, after the pack's, in
    // both naming modes (record 55 C4), and in its own `_` slot in the
    // descriptive layout.
    let Some(converter) = converter() else { return };
    let thin = Twin {
        acquired: Some("100500"),
        ..twin("t1_mprage_sag", "T1 MPRAGE")
    };
    let thick = Twin {
        acquired: Some("102000"),
        thickness: "3.0",
        ..thin
    };

    let source = twins(thin, thick);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (mut reg, report) = released(&source, &home_dir, &out, &converter);
    let names = anat_names(out.path());
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("MPRAGE1mm_T1w.nii.gz"))
            && names.iter().any(|n| n.ends_with("MPRAGE3mm_T1w.nii.gz")),
        "{names:?}"
    );
    assert!(names.iter().all(|n| !n.contains("_run-")), "{names:?}");
    assert_eq!(report.numbered, 0, "{report:?}");
    assert!(shared_differs(&mut reg).is_empty());
    let marks: Vec<String> = report
        .decided
        .iter()
        .flat_map(|d| {
            d.marks
                .iter()
                .map(|m| format!("{}={}", m.property, m.value))
        })
        .collect();
    assert_eq!(
        marks,
        ["SliceThickness=1", "SliceThickness=3"],
        "{report:?}"
    );

    let source = twins(thin, thick);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_reg, _) = released_minimal(&source, &home_dir, &out, &converter);
    let names = anat_names(out.path());
    assert!(
        names.iter().any(|n| n.ends_with("_acq-1mm_T1w.nii.gz"))
            && names.iter().any(|n| n.ends_with("_acq-3mm_T1w.nii.gz")),
        "the minimal style: the one slot that differs: {names:?}"
    );
    assert!(
        !out.path().join(".bidsignore").exists(),
        "nothing outside the standard to ignore"
    );

    // The descriptive layout: a slot of its own, `_1mm`, `_3mm`.
    let source = twins(thin, thick);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let mut s = settings(
        out.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );
    s.layout = Layout::Descriptive;
    run::run(&mut reg, &s).unwrap();
    let dirs: std::collections::BTreeSet<String> = files_under(out.path())
        .iter()
        .filter_map(|f| {
            f.rsplit_once('/')
                .map(|(d, _)| d.rsplit('/').next().unwrap_or(d).to_string())
        })
        .filter(|d| d.contains("T1w"))
        .collect();
    assert!(
        dirs.iter().any(|d| d.ends_with("_1mm")) && dirs.iter().any(|d| d.ends_with("_3mm")),
        "{dirs:?}"
    );
}

#[test]
fn the_informative_fallback_is_a_plain_number_and_never_a_run() {
    // Wave 7a §8.1: two stations of one spine prescription that nothing a
    // name may spell separates take the plain number, alone in `acq-` in the
    // minimal style.
    let Some(converter) = converter() else { return };
    let upper = twin("t1_mprage_sag", "T1 MPRAGE");
    let lower = Twin {
        first_slice: -199.0,
        ..upper
    };
    let source = twins(upper, lower);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (_reg, report) = released_minimal(&source, &home_dir, &out, &converter);
    let names = anat_names(out.path());
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names.iter().any(|n| before_suffix(n).ends_with("_acq-1"))
            && names.iter().any(|n| before_suffix(n).ends_with("_acq-2")),
        "{names:?}"
    );
    assert!(names.iter().all(|n| !n.contains("_run-")), "{names:?}");
    assert_eq!(report.numbered, 2, "{report:?}");
}

#[test]
fn two_rescans_beside_a_different_scan_stay_runs_and_the_third_says_why() {
    // Wave 7a §8.1, rule 1 inside rule 3: three series want one name; two
    // are one acquisition made again and the third is cut thicker. The
    // thickness goes on first, and the two that still share a name are
    // measured again and are runs.
    let Some(converter) = converter() else { return };
    let first = Twin {
        acquired: Some("100500"),
        ..twin("t1_mprage_sag", "T1 MPRAGE")
    };
    let again = Twin {
        acquired: Some("101500"),
        ..first
    };
    let thick = Twin {
        acquired: Some("103000"),
        thickness: "3.0",
        ..first
    };
    let source = scans(&[first, again, thick]);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (mut reg, report) = released(&source, &home_dir, &out, &converter);
    let names = anat_names(out.path());
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(
        names.iter().any(|n| n.ends_with("1mm_run-1_T1w.nii.gz"))
            && names.iter().any(|n| n.ends_with("1mm_run-2_T1w.nii.gz"))
            && names.iter().any(|n| n.ends_with("3mm_T1w.nii.gz")),
        "{names:?}"
    );
    assert_eq!(report.shared_names, 1, "{report:?}");
    assert_eq!(report.repeats, 2, "{report:?}");
    assert_eq!(report.numbered, 0, "{report:?}");
    assert!(shared_differs(&mut reg).is_empty());
}
