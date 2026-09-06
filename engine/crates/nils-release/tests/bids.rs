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
        categories: categories::Category::every(),
        selection: Selection::default(),
        scheme,
        private: &pack().release,
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout: Layout::Bids,
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
fn the_tree_carries_the_clinical_layer_under_the_policy() {
    // Wave 4a §7.4. participants.tsv carries the sex and the age at the
    // first session; each sessions.tsv the age at the session and the
    // nearest observation of each kind the release names, with its distance
    // in days and, under a policy that keeps or shifts dates, its date.
    // Under `year` the date is not written at all, and under `shift` it
    // moves with the subject's offset, so the interval to the scan holds.
    use nils_registry::clinical::{self, Vocabulary};
    use nils_registry::schema;
    use nils_registry::store::{Insert, Param};
    use nils_release::dates;
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
    // A date-labelled scheme is refused with shifted dates (Wave 3 §4.3), so
    // the shifted and year trees label their sessions by ordinal.
    let by_date = SessionScheme::default();
    let ordinal = SessionScheme {
        naming: nils_registry::session::Naming::Ordinal,
        ..SessionScheme::default()
    };
    let kinds = ["EDSS".to_string(), "Relapse".to_string()];
    for policy_dates in [
        dates::Policy::Keep,
        dates::Policy::Shift,
        dates::Policy::Year,
    ] {
        let scheme = match policy_dates {
            dates::Policy::Keep => &by_date,
            _ => &ordinal,
        };
        let out = TempDir::new("bids-clinical");
        let policy = Policy {
            dates: policy_dates,
            ..Policy::default()
        };
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
        assert_eq!(cell("sex"), Some("F"), "{policy_dates:?}: {participants}");
        assert_eq!(cell("age"), Some("42"), "{policy_dates:?}: {participants}");
        let sessions = written
            .iter()
            .find(|f| f.ends_with("_sessions.tsv"))
            .unwrap_or_else(|| panic!("{policy_dates:?}: {written:?}"));
        let text = std::fs::read_to_string(out.path().join(sessions)).unwrap();
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let values: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let cell = |name: &str| -> Option<&str> {
            header.iter().position(|h| *h == name).map(|i| values[i])
        };
        assert_eq!(cell("age"), Some("42"), "{policy_dates:?}: {text}");
        assert_eq!(cell("edss"), Some("3.5"), "{policy_dates:?}: {text}");
        assert_eq!(cell("edss_days"), Some("-5"), "{policy_dates:?}: {text}");
        assert_eq!(cell("relapse"), Some("yes"), "{policy_dates:?}: {text}");
        assert_eq!(
            cell("relapse_days"),
            Some("-365"),
            "{policy_dates:?}: {text}"
        );
        match policy_dates {
            dates::Policy::Keep => {
                assert_eq!(cell("edss_date"), Some("2022-01-10"), "{text}");
                assert_eq!(cell("relapse_date"), Some("2021-01-15"), "{text}");
            }
            dates::Policy::Shift => {
                // The scan moved by the offset, and so did the observation.
                let scan = cell("acq_time").unwrap();
                let scan_day = nils_registry::day::Day::parse(&scan[..10]).unwrap();
                let edss_day = nils_registry::day::Day::parse(cell("edss_date").unwrap()).unwrap();
                assert_eq!(edss_day.to_days() - scan_day.to_days(), -5, "{text}");
                assert_ne!(cell("edss_date"), Some("2022-01-10"), "shifted: {text}");
            }
            dates::Policy::Year => {
                assert_eq!(cell("edss_date"), None, "no date under year: {text}");
                assert_eq!(cell("relapse_date"), None, "{text}");
            }
        }
        // The sensitive kind is not a column, and naming it is refused.
        assert!(
            !header.iter().any(|h| h.starts_with("delivery")),
            "{policy_dates:?}: {text}"
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
