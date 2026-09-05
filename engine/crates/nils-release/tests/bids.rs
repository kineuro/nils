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
        private: &pack().private,
        on_unknown: nils_release::burned::OnUnknown::Write,
        actor: "a test",
        key: KEY,
        pack: pack(),
        layout: Layout::Bids,
        places,
        converter,
        compress: true,
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
