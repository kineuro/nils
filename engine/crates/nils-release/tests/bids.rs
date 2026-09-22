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
        naming: nils_release::name::Naming::Bids,
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
    assert!(text.starts_with("filename\tacq_time\n"), "{text}");
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
    assert!(
        now.iter().all(|n| n.contains("acq-Spine")),
        "and the new name says so: {now:?}"
    );
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
        assert!(file.contains("acq-Spine"), "the name says it too: {file}");
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
fn the_informative_mode_says_what_the_entities_say_and_the_bids_one_does_not() {
    // Record 37 S7. One question asked of every name: BIDS mode puts the
    // contrast in `ce-` and nowhere else, because a name that said it twice
    // would be a name arguing with itself; informative mode puts every axis
    // in the label as well, for a tree read by people rather than tools.
    let Some(converter) = converter() else { return };
    let source = tree();
    let home_dir = TempDir::new("bids-home");
    let (_home, mut reg) = registry(&home_dir, &source);
    // The axis stores the label and the name is built from the identity.
    decide(&mut reg, "post_contrast", "1");
    let policy = Policy::default();
    let scheme = SessionScheme::default();

    let plain = TempDir::new("bids-out");
    run::run(
        &mut reg,
        &settings(
            plain.path(),
            &policy,
            &scheme,
            Options::default(),
            Some(&converter),
        ),
    )
    .unwrap();
    let told = TempDir::new("bids-told");
    let mut s = settings(
        told.path(),
        &policy,
        &scheme,
        Options::default(),
        Some(&converter),
    );
    s.name = "a cohort read by people";
    s.naming = nils_release::name::Naming::Informative;
    let report = run::run(&mut reg, &s).unwrap();
    assert_eq!(report.naming, "informative");

    let named = |root: &Path| -> Vec<String> {
        files_under(root)
            .into_iter()
            .filter(|f| f.starts_with("sub-") && f.ends_with(".nii.gz"))
            .collect()
    };
    let bids = named(plain.path());
    let informative = named(told.path());
    assert!(!bids.is_empty() && bids.len() == informative.len());
    assert!(
        bids.iter().all(|n| n.contains("_ce-contrast_")),
        "the entity carries it in both: {bids:?}"
    );
    assert!(
        bids.iter().all(|n| !n.contains("CE_ce-contrast")),
        "and only the entity, in BIDS mode: {bids:?}"
    );
    assert!(
        informative.iter().all(|n| n.contains("CE_ce-contrast")),
        "the label says it too, in informative mode: {informative:?}"
    );
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
fn two_acquisitions_that_want_one_name_are_refused_and_a_person_is_asked() {
    // Record 37 S2. The pair differs in what it covers and in nothing a BIDS
    // name can say, so no name is written: a `run-2` there would claim a
    // rescan that never happened, and a validator would pass it. The stacks
    // are in `sourcedata/` under the informative names of §9.1, which are
    // unique, and the question says what differs.
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

    let written = files_under(out.path());
    // No FLAIR in the raw tree, under a `run-` or under anything else ...
    assert!(
        !written
            .iter()
            .any(|f| f.contains("FLAIR") && !f.starts_with("sourcedata/")),
        "{written:?}"
    );
    // ... and both of them under `sourcedata/`, as DICOM, told apart by the
    // informative names, which are unique.
    let source_side: Vec<&String> = written
        .iter()
        .filter(|f| f.starts_with("sourcedata/") && f.contains("FLAIR"))
        .collect();
    assert_eq!(source_side.len(), 10, "{written:?}");
    let places: std::collections::BTreeSet<&str> = source_side
        .iter()
        .filter_map(|f| f.rsplit_once('/').map(|(dir, _)| dir))
        .collect();
    assert_eq!(places.len(), 2, "{source_side:?}");

    // And the question, with what differs in it.
    let asked = |reg: &mut Registry| -> Vec<(serde_json::Value, serde_json::Value)> {
        let store = reg.store();
        let sql = format!(
            "SELECT ref, evidence FROM {} WHERE kind = 'release.shared_name'",
            store.qualified("review_item"),
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| {
                let of = |i: usize| {
                    serde_json::from_str(r.opt_text(i).unwrap().unwrap_or_default())
                        .unwrap_or(serde_json::Value::Null)
                };
                (of(0), of(1))
            })
            .collect()
    };
    let items = asked(&mut reg);
    assert_eq!(items.len(), 1, "one question, and one only");
    let (reference, evidence) = &items[0];
    assert_eq!(evidence["stacks"], 2);
    assert_eq!(evidence["placed"], "sourcedata");
    assert_eq!(
        evidence["differs"],
        serde_json::json!(["the number of images", "what it covers"])
    );
    assert_eq!(
        reference["stack_ids"].as_array().map(Vec::len),
        Some(2),
        "the item names both stacks"
    );

    // And a re-run does not file it again: a release is re-run whenever
    // anything upstream changes, and stacks that say what they said last time
    // raise the same question with the same answer. What recurs is the number
    // in the report.
    let again = run::run(
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
    assert_eq!(again.not_repeats, 2, "{again:?}");
    assert_eq!(asked(&mut reg).len(), 1, "the question is filed once");
}

/// One series of a pair: what a console recorded for it, where its first
/// slice sits, when it was acquired, and the number the scanner gave it.
#[derive(Clone, Copy)]
struct Twin<'a> {
    description: &'a str,
    protocol: &'a str,
    first_slice: f64,
    acquired: Option<&'a str>,
}

fn twin<'a>(description: &'a str, protocol: &'a str) -> Twin<'a> {
    Twin {
        description,
        protocol,
        first_slice: 1.0,
        acquired: None,
    }
}

/// Two series of one session built from the survey's characteristics and from
/// no archive: one protocol step, the same geometry, the same timings, the
/// same image type, written twice (record 37 S4's fixture, and record 38's).
fn twins(one: Twin, two: Twin) -> TempDir {
    let dir = TempDir::new("bids-twins");
    for (n, t) in [("1", one), ("2", two)] {
        for slice in 1..=4 {
            let at = t.first_slice + f64::from(slice - 1);
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
                synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
                synth::text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("0\\0\\{at}")),
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
        .filter(|f| f.ends_with("_T1w.nii.gz"))
        .collect()
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

/// A pair that is not a rescan: no BIDS name for either, nothing numbered,
/// nothing named by text, and one question saying what differs.
fn refused_with(one: Twin, two: Twin, differs: serde_json::Value) {
    let Some(converter) = converter() else { return };
    let source = twins(one, two);
    let home_dir = TempDir::new("bids-home");
    let out = TempDir::new("bids-out");
    let (mut reg, report) = released(&source, &home_dir, &out, &converter);

    assert_eq!(report.shared_names, 1, "{report:?}");
    assert_eq!(report.repeats, 0, "{report:?}");
    assert_eq!(report.not_repeats, 2, "{report:?}");
    let names = anat_names(out.path());
    assert!(names.is_empty(), "neither has a BIDS name: {names:?}");
    let written = files_under(out.path());
    assert!(
        written.iter().all(|f| !f.contains("Text")),
        "no name rests on text: {written:?}"
    );
    assert_eq!(shared_differs(&mut reg), [differs]);
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
fn a_pair_whose_series_description_differs_is_asked_about_and_not_numbered() {
    // Record 38: inside one session a rescan's texts are identical. The text
    // makes no name; it refuses one, and a person decides.
    refused_with(
        twin("t1_mprage_sag", "T1 MPRAGE"),
        twin("t1_mprage_sag_iso", "T1 MPRAGE"),
        serde_json::json!(["the series description"]),
    );
}

#[test]
fn the_counter_a_scanner_welds_on_a_rerun_step_is_a_different_text() {
    // Record 37 folded the counter away; record 38 compares the texts as they
    // were written, less case and whitespace.
    refused_with(
        twin("t1_mprage_sag", "T1 MPRAGE"),
        twin("t1_mprage_sag", "T1 MPRAGE 2"),
        serde_json::json!(["the protocol name"]),
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
    refused_with(upper, lower, serde_json::json!(["where it sits"]));
}

#[test]
fn two_series_made_at_one_moment_are_asked_about() {
    // One acquisition written twice is not a rescan, whatever its UIDs.
    let one = Twin {
        acquired: Some("102000"),
        ..twin("t1_mprage_sag", "T1 MPRAGE")
    };
    refused_with(one, one, serde_json::json!(["acquired at the same moment"]));
}
