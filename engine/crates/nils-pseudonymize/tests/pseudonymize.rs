// SPDX-License-Identifier: AGPL-3.0-only

//! The pseudonymiser over a synthetic identified dataset, into a registry
//! on SQLite: the copies written under the code with the identifiers gone
//! and the pixels untouched, the layout from facts, the files held for want
//! of a map with their shape and sealed identifier and the review item,
//! the resume of a second run, a changed file written again, the held rows
//! read again once coded anyway, a refused file, the report, the batch and
//! the job, a dry run, and a run asked to stop before it began.

use std::path::{Path, PathBuf};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::Cancel;
use nils_pack::private::Allowed;
use nils_pseudonymize::{Cancelled, Report, Settings, pseudonymize, pseudonymize_with};
use nils_registry::home::{Home, InitOptions};
use nils_registry::linkage::{self, ImportRow, Subkeys};
use nils_registry::place::{self, New, Role};
use nils_registry::review;
use nils_registry::{Backend, Registry, Scheme};
use serde_json::{Value, json};

const KEY: &[u8] = b"nils-pseudonymize-fixture-key";

/// The three people of the tree: the first two mapped, the third not.
const MAPPED: [(&str, &str); 2] = [("199001011234", "subj0001a"), ("198502023456", "subj0002b")];
const UNMAPPED: &str = "197003034567";

struct Lab {
    home: Home,
    _dir: TempDir,
}

fn lab() -> Lab {
    let dir = TempDir::new("pseudonymize-home");
    let home = Home::new(dir.path());
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
    Lab { home, _dir: dir }
}

fn pixels(seed: u32) -> Vec<u8> {
    (0..6000u32)
        .map(|i| ((i * 31 + seed) % 253) as u8)
        .collect()
}

/// One original: identified through and through, with a private block
/// the pack keeps one element of, and a series description.
fn original(patient: &str, study: u32, series: u32, instance: u32, description: &str) -> Vec<u8> {
    let study_uid = format!("1.2.826.0.1.3680043.8.498.{patient}.{study}");
    let series_uid = format!("{study_uid}.{series}");
    let sop = format!("{series_uid}.{instance}");
    let mut e = synth::minimal_mr(&study_uid, &series_uid, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
    e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
    e.push(synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19900101"));
    e.push(synth::text(tags::PATIENT_SEX, VR::CS, "F"));
    e.push(synth::text(tags::PATIENT_WEIGHT, VR::DS, "62"));
    e.push(synth::text(
        tags::STUDY_DATE,
        VR::DA,
        &format!("2024010{study}"),
    ));
    e.push(synth::text(tags::STUDY_TIME, VR::TM, "101500"));
    e.push(synth::text(
        tags::SERIES_NUMBER,
        VR::IS,
        &series.to_string(),
    ));
    e.push(synth::text(
        tags::INSTANCE_NUMBER,
        VR::IS,
        &instance.to_string(),
    ));
    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, description));
    e.push(synth::text(
        tags::INSTITUTION_NAME,
        VR::LO,
        "Somewhere General",
    ));
    e.push(synth::text(tags::STATION_NAME, VR::SH, "MR1"));
    e.push(synth::text(tags::DEVICE_SERIAL_NUMBER, VR::LO, "SN-0001"));
    e.push(synth::text(
        tags::REFERRING_PHYSICIAN_NAME,
        VR::PN,
        "Ref^Dr",
    ));
    e.push(synth::text(Tag(0x0019, 0x0010), VR::LO, "A VENDOR"));
    e.push(synth::text(Tag(0x0019, 0x100C), VR::IS, "1000"));
    e.push(synth::text(Tag(0x0019, 0x1099), VR::LO, "the operator"));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        pixels(study * 100 + series * 10 + instance),
    ));
    synth::part10(&MetaFields::mr(&sop), &e, true)
}

/// A dataset folder with its originals: two studies for the first person,
/// one each for the others, two series of two files, and a file that is
/// no DICOM.
fn dataset() -> TempDir {
    let dir = TempDir::new("pseudonymize-ds");
    let originals = Path::new("derivatives/dcm-original");
    let mut n = 0;
    for (p, (patient, _)) in MAPPED.iter().enumerate() {
        for study in 1..=(2 - p as u32) {
            for series in 1..=2 {
                for instance in 1..=2 {
                    n += 1;
                    dir.file(
                        &originals
                            .join(format!("p{p}/s{study}/IM_{n:04}"))
                            .display()
                            .to_string(),
                        &original(patient, study, series, instance, "t1_mprage"),
                    );
                }
            }
        }
    }
    for instance in 1..=3 {
        dir.file(
            &originals
                .join(format!("p2/IM_{instance:04}"))
                .display()
                .to_string(),
            &original(UNMAPPED, 1, 1, instance, "t2_tse"),
        );
    }
    dir.file(
        &originals.join("p0/notes.txt").display().to_string(),
        b"not a dicom file at all",
    );
    std::fs::create_dir_all(dir.path().join("derivatives/dcm-anon")).unwrap();
    dir
}

fn declare(registry: &mut Registry, dir: &Path, tags: Value) -> place::Place {
    let store = registry.store();
    let id = place::add(
        store,
        &New {
            name: "scans",
            role: Role::Source,
            path: dir.to_str().unwrap(),
            guarantees: json!({}),
            probed: Value::Null,
            handling: Value::Null,
            dataset: json!({
                "arrives": "identified",
                "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
                "identity": null,
                "unmapped": "hold",
                "tags": tags,
            }),
        },
    )
    .unwrap();
    place::show(store, id).unwrap().unwrap()
}

fn import_map(registry: &mut Registry) {
    let mut linkage = registry.open_linkage().unwrap();
    let keys = Subkeys::derive(KEY);
    let rows: Vec<ImportRow> = MAPPED
        .iter()
        .enumerate()
        .map(|(i, (identifier, code))| ImportRow {
            line: i + 2,
            identifier: identifier.to_string(),
            code: code.to_string(),
        })
        .collect();
    linkage::import(registry.store(), &mut linkage, &keys, "patient-id", &rows).unwrap();
}

fn settings(place: &place::Place) -> Settings {
    let mut s = Settings::for_dataset(place).unwrap();
    s.name = "scans-test".into();
    s.workers = 3;
    s.walk_threads = 2;
    s.batch_rows = 2;
    s.private = vec![Allowed {
        creator: "A VENDOR".into(),
        group: 0x0019,
        element: 0x0C,
        why: "a test".into(),
    }];
    s.pack = Some("test".into());
    s
}

fn text(ds: &dicom_object::InMemDicomObject, tag: Tag) -> Option<String> {
    let e = ds.get(tag)?;
    let s = e.value().to_str().ok()?;
    let s = s.trim_matches(['\0', ' ']);
    (!s.is_empty()).then(|| s.to_string())
}

fn outputs(anon: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![anon.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn rows(registry: &mut Registry, sql: &str) -> Vec<nils_registry::Row> {
    registry.store().query(sql, &[]).unwrap()
}

fn one(registry: &mut Registry, sql: &str) -> i64 {
    rows(registry, sql)[0].int(0).unwrap()
}

fn files_of(report: &Report) -> (u64, u64, u64, u64, u64) {
    let f = &report.files;
    (f.seen, f.written, f.unchanged, f.held, f.refused)
}

#[test]
fn a_dataset_is_pseudonymised_held_resumed_and_the_held_coded_anyway() {
    let lab = lab();
    let dir = dataset();
    let originals = dir.path().join("derivatives/dcm-original");
    let anon = dir.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    import_map(&mut registry);
    let place = declare(
        &mut registry,
        dir.path(),
        json!({"keep_demographics": true, "remove": ["0018,1000"], "keep": ["0008,1010"]}),
    );
    let s = settings(&place);
    assert_eq!(s.originals, originals);
    assert_eq!(s.anon, anon);

    // a dry run walks and resolves and writes nothing
    let mut dry = s.clone();
    dry.dry_run = true;
    let report = pseudonymize(&dry, &mut registry).unwrap();
    assert!(report.dry_run);
    assert_eq!(files_of(&report), (16, 12, 0, 3, 1));
    assert_eq!(report.subjects.seen, 2);
    assert_eq!(report.subjects.new, 0);
    assert!(outputs(&anon).is_empty());
    assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM pseudonym_file"), 0);
    assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM job"), 0);

    // the run
    let start = std::time::Instant::now();
    let report = pseudonymize(&s, &mut registry).unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "pseudonymized {} files in {elapsed:.3} s: {:.0} files/s",
        report.files.seen,
        report.files.seen as f64 / elapsed
    );
    assert_eq!(files_of(&report), (16, 12, 0, 3, 1), "{report}");
    assert_eq!(report.subjects.seen, 2);
    assert_eq!(report.subjects.new, 0);
    assert_eq!(report.subjects.provisional, 0);
    assert_eq!(
        report.tags_removed.get("(0010,0010)"),
        Some(&12),
        "the name, every file"
    );
    assert_eq!(
        report.tags_removed.get("(0010,0030)"),
        Some(&12),
        "the birth date"
    );
    assert_eq!(
        report.tags_removed.get("(0018,1000)"),
        Some(&12),
        "the dataset's remove list"
    );
    assert_eq!(
        report.tags_removed.get("(0008,0080)"),
        Some(&12),
        "the institution"
    );
    assert_eq!(
        report.tags_removed.get("(0008,1010)"),
        None,
        "the dataset's keep list wins"
    );
    assert_eq!(
        report.tags_removed.get("(0010,0040)"),
        None,
        "the demographics are kept"
    );
    assert_eq!(
        report.private_removed, 12,
        "one private element per file goes"
    );
    assert_eq!(report.refused_by.get("not_dicom"), Some(&1));
    assert_eq!(report.held_by_shape.get("999999999999"), Some(&3));
    assert!(report.cancelled.is_none());
    let batch_id = report.batch_id.unwrap();
    let job_id = report.job_id.unwrap();
    let rendered = format!("{report}");
    assert!(
        rendered.contains("16 seen   12 written   0 unchanged   3 held   1 refused"),
        "{rendered}"
    );
    assert!(
        !rendered.contains(UNMAPPED) && !rendered.contains("Doe"),
        "{rendered}"
    );

    // the outputs: the layout from facts, twelve of them
    let written = outputs(&anon);
    assert_eq!(written.len(), 12, "{written:?}");
    for path in &written {
        let rel = path.strip_prefix(&anon).unwrap();
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(parts.len(), 4, "{rel:?}");
        assert!(
            parts[0] == "subj0001a" || parts[0] == "subj0002b",
            "{rel:?}"
        );
        assert_eq!(parts[1].len(), 8 + 1 + 8, "{rel:?}");
        assert!(parts[1].starts_with("2024010"), "{rel:?}");
        assert_eq!(parts[2].len(), 3);
        assert!(parts[3].ends_with(".dcm") && parts[3].len() == 9, "{rel:?}");
        assert!(!path.with_extension("dcm.part").exists());
    }
    // one of them against its source: the code in, the identifiers out,
    // the dates and UIDs kept, the pixels the same bytes
    let sample = written
        .iter()
        .find(|p| p.to_string_lossy().contains("subj0002b"))
        .unwrap();
    let read = nils_dicom::read(sample).unwrap();
    let ds = &read.dataset;
    assert_eq!(text(ds, tags::PATIENT_ID).as_deref(), Some("subj0002b"));
    assert_eq!(text(ds, tags::PATIENT_NAME), None);
    assert_eq!(text(ds, tags::PATIENT_BIRTH_DATE), None);
    assert_eq!(text(ds, tags::PATIENT_AGE).as_deref(), Some("034Y"));
    assert_eq!(text(ds, tags::PATIENT_SEX).as_deref(), Some("F"));
    assert_eq!(text(ds, tags::PATIENT_WEIGHT).as_deref(), Some("62"));
    assert_eq!(text(ds, tags::STUDY_DATE).as_deref(), Some("20240101"));
    assert_eq!(text(ds, tags::STUDY_TIME).as_deref(), Some("101500"));
    assert_eq!(text(ds, tags::INSTITUTION_NAME), None);
    assert_eq!(text(ds, tags::REFERRING_PHYSICIAN_NAME), None);
    assert_eq!(text(ds, tags::STATION_NAME).as_deref(), Some("MR1"));
    assert_eq!(text(ds, tags::DEVICE_SERIAL_NUMBER), None);
    assert_eq!(
        text(ds, tags::SERIES_DESCRIPTION).as_deref(),
        Some("t1_mprage")
    );
    assert_eq!(text(ds, Tag(0x0019, 0x100C)).as_deref(), Some("1000"));
    assert_eq!(text(ds, Tag(0x0019, 0x0010)).as_deref(), Some("A VENDOR"));
    assert_eq!(text(ds, Tag(0x0019, 0x1099)), None);
    let sop = text(ds, tags::SOP_INSTANCE_UID).unwrap();
    assert!(sop.starts_with("1.2.826.0.1.3680043.8.498.198502023456.1."));
    assert_eq!(
        text(ds, tags::STUDY_INSTANCE_UID).as_deref(),
        Some("1.2.826.0.1.3680043.8.498.198502023456.1")
    );
    let instance: u32 = text(ds, tags::INSTANCE_NUMBER).unwrap().parse().unwrap();
    let series: u32 = text(ds, tags::SERIES_NUMBER).unwrap().parse().unwrap();
    let out_bytes = std::fs::read(sample).unwrap();
    assert!(out_bytes.ends_with(&pixels(100 + series * 10 + instance)));
    let whole = dicom_object::OpenFileOptions::new()
        .open_file(sample)
        .unwrap();
    assert_eq!(
        whole
            .element(tags::PIXEL_DATA)
            .unwrap()
            .value()
            .to_bytes()
            .unwrap()
            .len(),
        6000
    );
    // the digest recorded is the digest of what was written
    let row = &rows(
        &mut registry,
        &format!(
            "SELECT digest, out_size FROM pseudonym_file WHERE out_path = '{}'",
            sample.strip_prefix(&anon).unwrap().display()
        ),
    )[0];
    use blake2::Digest as _;
    assert_eq!(
        row.text(0).unwrap(),
        hex::encode(blake2::Blake2s256::digest(&out_bytes))
    );
    assert_eq!(row.int(1).unwrap(), out_bytes.len() as i64);

    // the rows: twelve written, three held, one refused; a held row keeps
    // the shape, the keyed lookup and the sealed identifier, never the
    // identifier
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'written'"
        ),
        12
    );
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'refused'"
        ),
        1
    );
    let held = rows(
        &mut registry,
        "SELECT path, dir, shape, lookup, sealed, id_type, out_path, code_anyway FROM pseudonym_file WHERE state = 'held' ORDER BY path",
    );
    assert_eq!(held.len(), 3);
    let keys = Subkeys::derive(KEY);
    for r in &held {
        assert!(r.text(0).unwrap().starts_with("p2/IM_"));
        assert_eq!(r.text(1).unwrap(), "p2");
        assert_eq!(r.text(2).unwrap(), "999999999999");
        assert_eq!(r.bytes(3).unwrap(), keys.lookup("patient-id", UNMAPPED));
        assert_eq!(keys.open(r.bytes(4).unwrap()).unwrap(), UNMAPPED);
        assert_eq!(r.text(5).unwrap(), "patient-id");
        assert!(r.opt_text(6).unwrap().is_none());
        assert_eq!(r.int(7).unwrap(), 0);
    }
    let dump = rows(
        &mut registry,
        "SELECT path, shape, id_type, out_path, digest FROM pseudonym_file",
    )
    .iter()
    .map(|r| format!("{r:?}"))
    .collect::<String>();
    assert!(
        !dump.contains(UNMAPPED) && !dump.contains("1990010112"),
        "{dump}"
    );

    // the review item: one per dataset and shape, the files as evidence
    let items = rows(
        &mut registry,
        "SELECT kind, status, group_key, evidence FROM review_item ORDER BY id",
    );
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].text(0).unwrap(), review::UNMAPPED_KIND);
    assert_eq!(items[0].text(1).unwrap(), "open");
    assert_eq!(
        items[0].text(2).unwrap(),
        format!("place:{}|shape:999999999999", place.id)
    );
    let evidence: Value = serde_json::from_str(items[0].text(3).unwrap()).unwrap();
    assert_eq!(evidence["files"], 3);
    assert!(evidence["first_seen"].is_string());

    // the batch and the job
    let batch = &rows(
        &mut registry,
        &format!(
            "SELECT kind, state, name, counts, epoch_after, source_id FROM ingest_batch WHERE id = {batch_id}"
        ),
    )[0];
    assert_eq!(batch.text(0).unwrap(), "pseudonymize");
    assert_eq!(batch.text(1).unwrap(), "done");
    assert_eq!(batch.text(2).unwrap(), "scans-test");
    let counts: Value = serde_json::from_str(batch.text(3).unwrap()).unwrap();
    assert_eq!(counts["files"]["written"], 12);
    assert_eq!(counts["held_by_shape"]["999999999999"], 3);
    assert!(batch.opt_int(4).unwrap().is_some());
    let source_root = rows(
        &mut registry,
        &format!(
            "SELECT root FROM source WHERE id = {}",
            batch.int(5).unwrap()
        ),
    )[0]
    .text(0)
    .unwrap()
    .to_string();
    assert_eq!(
        Path::new(&source_root),
        anon,
        "the source is the pseudonymised tree"
    );
    let job = &rows(
        &mut registry,
        &format!("SELECT kind, state, result, name FROM job WHERE id = {job_id}"),
    )[0];
    assert_eq!(job.text(0).unwrap(), "pseudonymize");
    assert_eq!(job.text(1).unwrap(), "done");
    let result: Value = serde_json::from_str(job.text(2).unwrap()).unwrap();
    assert_eq!(result["files"]["held"], 3);
    assert_eq!(result["subjects"]["seen"], 2);
    assert!(result["seconds"].as_f64().unwrap() >= 0.0);

    // a second run: everything unchanged, nothing written again
    let before: Vec<(PathBuf, std::time::SystemTime)> = written
        .iter()
        .map(|p| (p.clone(), std::fs::metadata(p).unwrap().modified().unwrap()))
        .collect();
    let again = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&again), (16, 0, 12, 3, 1), "{again}");
    assert_eq!(again.subjects.seen, 0);
    for (p, modified) in &before {
        assert_eq!(&std::fs::metadata(p).unwrap().modified().unwrap(), modified);
    }
    assert_eq!(
        one(
            &mut registry,
            &format!(
                "SELECT COUNT(*) FROM pseudonym_file WHERE batch_id = {}",
                again.batch_id.unwrap()
            )
        ),
        16,
        "every row touched by the run"
    );
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM review_item WHERE status = 'open'"
        ),
        1,
        "still one open item"
    );

    // a changed source is written again, now without the demographics the
    // dataset stopped keeping; its old output stands where it was, since
    // the facts did not move
    let changed = originals.join("p0/s1/IM_0001");
    std::fs::write(&changed, original(MAPPED[0].0, 1, 1, 1, "t1_mprage repeat")).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::open(&changed)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let place = place::set_dataset(
        registry.store(),
        place.id,
        &json!({
            "arrives": "identified",
            "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
            "identity": null,
            "unmapped": "hold",
            "tags": {"keep_demographics": false, "remove": ["0018,1000"], "keep": ["0008,1010"]},
        }),
    )
    .unwrap();
    let s = settings(&place);
    let third = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&third), (16, 1, 11, 3, 1), "{third}");
    assert_eq!(outputs(&anon).len(), 12);
    let out_path = rows(
        &mut registry,
        "SELECT out_path FROM pseudonym_file WHERE path = 'p0/s1/IM_0001'",
    )[0]
    .text(0)
    .unwrap()
    .to_string();
    let rewritten = nils_dicom::read(&anon.join(&out_path)).unwrap();
    assert_eq!(
        text(&rewritten.dataset, tags::SERIES_DESCRIPTION).as_deref(),
        Some("t1_mprage repeat")
    );
    assert_eq!(text(&rewritten.dataset, tags::PATIENT_SEX), None);
    assert_eq!(
        text(&rewritten.dataset, tags::PATIENT_ID).as_deref(),
        Some("subj0001a")
    );

    // coded anyway: the held rows are read again, their subject made and
    // marked provisional, the question closed and a new one opened
    registry
        .store()
        .execute(
            "UPDATE pseudonym_file SET code_anyway = 1 WHERE state = 'held'",
            &[],
        )
        .unwrap();
    let mut held_only = s.clone();
    held_only.held = true;
    let fourth = pseudonymize(&held_only, &mut registry).unwrap();
    assert_eq!(files_of(&fourth), (3, 3, 0, 0, 0), "{fourth}");
    assert!(fourth.held_only);
    assert_eq!(fourth.subjects.new, 1);
    assert_eq!(fourth.subjects.seen, 1);
    assert_eq!(fourth.subjects.provisional, 1);
    assert_eq!(outputs(&anon).len(), 15);
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'held'"
        ),
        0
    );
    let subject = &rows(
        &mut registry,
        "SELECT code, provisional, first_batch_id FROM subject WHERE provisional = 1",
    )[0];
    let code = subject.text(0).unwrap().to_string();
    assert_eq!(code.len(), 12);
    assert_eq!(subject.int(1).unwrap(), 1);
    assert_eq!(subject.opt_int(2).unwrap(), fourth.batch_id);
    assert!(anon.join(&code).is_dir());
    let coded = &rows(
        &mut registry,
        "SELECT state, out_path, code_anyway, shape, sealed FROM pseudonym_file WHERE path = 'p2/IM_0001'",
    )[0];
    assert_eq!(coded.text(0).unwrap(), "written");
    assert!(coded.text(1).unwrap().starts_with(&format!("{code}/")));
    assert_eq!(coded.int(2).unwrap(), 0);
    assert!(coded.opt_text(3).unwrap().is_none());
    assert!(coded.opt_bytes(4).unwrap().is_none());
    // the identity was filed: a fifth run finds the subject by its lookup
    let mut linkage = registry.open_linkage().unwrap();
    let found = linkage::identities_by_lookup(&mut linkage, &[keys.lookup("patient-id", UNMAPPED)])
        .unwrap();
    assert_eq!(found.len(), 1);
    let items = rows(
        &mut registry,
        "SELECT kind, status, evidence FROM review_item ORDER BY id",
    );
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].text(0).unwrap(), review::UNMAPPED_KIND);
    assert_eq!(items[0].text(1).unwrap(), review::RESOLVED);
    assert_eq!(items[1].text(0).unwrap(), review::PROVISIONAL_KIND);
    assert_eq!(items[1].text(1).unwrap(), "open");
    let evidence: Value = serde_json::from_str(items[1].text(2).unwrap()).unwrap();
    assert_eq!(evidence["files"], 3);
    // the epoch moved for the subject made
    let batches = rows(
        &mut registry,
        "SELECT epoch_after FROM ingest_batch ORDER BY id",
    );
    let epochs: Vec<i64> = batches.iter().map(|r| r.int(0).unwrap()).collect();
    assert!(epochs[3] > epochs[2], "{epochs:?}");
    assert_eq!(epochs[0], epochs[1], "{epochs:?}");

    // a fifth run over the whole tree: everything unchanged, no question
    let fifth = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&fifth), (16, 0, 15, 0, 1), "{fifth}");
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM review_item WHERE status = 'open'"
        ),
        1,
        "the provisional question stays"
    );
}

/// Lab 26, defect 5: the rule reads the third person's identifier as a
/// patient id and holds her files; a map naming that value as a study id,
/// with a code of its own, releases the files all the same, says under
/// which type, and the next `--held` run writes them under that code.
#[test]
fn a_map_naming_a_held_value_under_another_type_releases_it_for_the_held_run() {
    use nils_registry::identity_map::{self, Column, Map, Role, Row};
    let lab = lab();
    let dir = dataset();
    let anon = dir.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    import_map(&mut registry);
    let place = declare(&mut registry, dir.path(), json!({}));
    let s = settings(&place);
    let report = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&report), (16, 12, 0, 3, 1), "{report}");
    let keys = Subkeys::derive(KEY);
    let mut linkage = registry.open_linkage().unwrap();
    let columns = vec![
        Column {
            header: "study".into(),
            role: Role::parse("identifier:study-id").unwrap(),
        },
        Column {
            header: "code".into(),
            role: Role::Code,
        },
    ];
    let rows = vec![Row {
        line: 2,
        cells: vec![UNMAPPED.to_string(), "subj0003c".to_string()],
    }];
    let map = Map {
        columns: &columns,
        rows: &rows,
        dry_run: false,
        make_types: true,
        place_id: Some(place.id),
        actor: "tester@lab",
        job_id: None,
    };
    let filed = identity_map::import(registry.store(), &mut linkage, &keys, None, &map).unwrap();
    assert!(filed.written(), "{filed}");
    assert_eq!(filed.held_released, 3, "{filed}");
    assert_eq!(filed.held_released_by.len(), 1);
    assert_eq!(filed.held_released_by[0].id_type, "study-id");
    assert_eq!(filed.held_released_by[0].held_as, "patient-id");
    assert_eq!(filed.held_released_by[0].files, 3);
    drop(linkage);
    // the held run finds the subject under the lookup the map filed
    let mut held_only = s.clone();
    held_only.held = true;
    let run = pseudonymize(&held_only, &mut registry).unwrap();
    assert_eq!(files_of(&run), (3, 3, 0, 0, 0), "{run}");
    assert_eq!(run.subjects.seen, 1);
    assert_eq!(run.subjects.new, 0);
    assert_eq!(run.subjects.provisional, 0);
    assert!(anon.join("subj0003c").is_dir());
    assert_eq!(outputs(&anon).len(), 15);
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'held'"
        ),
        0
    );
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'written' AND out_path LIKE 'subj0003c/%'"
        ),
        3
    );
    // the unmapped question is answered, and no subject was made
    assert_eq!(
        one(
            &mut registry,
            &format!(
                "SELECT COUNT(*) FROM review_item WHERE kind = '{}' AND status = 'open'",
                review::UNMAPPED_KIND
            )
        ),
        0
    );
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM subject WHERE provisional = 1"
        ),
        0
    );
}

/// Lab 26d, finding 2, and the cure a refused purge names. A run records
/// what the original hashed to beside what the copy hashed to, since a
/// purge proves by content what it destroys. Two files cannot be proved: one
/// whose row was written before that digest was recorded, which is every row
/// of an install from before, and one changed in place under a modification
/// time put back to the recorded value, which a size and a time cannot see.
/// The run the refusal names is the one that mends both, so it has to be a
/// run that notices both.
#[test]
fn a_run_records_what_the_original_hashed_to_and_writes_again_what_it_cannot_prove() {
    use blake2::Digest as _;
    let lab = lab();
    let dir = dataset();
    let originals = dir.path().join("derivatives/dcm-original");
    let anon = dir.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    import_map(&mut registry);
    let place = declare(&mut registry, dir.path(), json!({}));
    let s = settings(&place);
    let report = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&report), (16, 12, 0, 3, 1), "{report}");

    // every copy carries the digest of the original it was made from, which
    // is that file's own bytes and never the copy's
    let written = rows(
        &mut registry,
        "SELECT path, original_digest, digest FROM pseudonym_file WHERE state = 'written' ORDER BY path",
    );
    assert_eq!(written.len(), 12);
    for r in &written {
        let rel = r.text(0).unwrap().to_string();
        let recorded = r.opt_text(1).unwrap().expect("the original's digest");
        let source = std::fs::read(originals.join(&rel)).unwrap();
        assert_eq!(
            recorded,
            hex::encode(blake2::Blake2s256::digest(&source)),
            "{rel}"
        );
        assert_ne!(recorded, r.text(2).unwrap(), "the copy is another file");
    }
    // the held and the refused rows carry none: no copy was made of them,
    // and a purge is refused for as long as either is there
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE original_digest IS NULL"
        ),
        4,
        "the three held and the one the reader refused"
    );

    // the first cure: rows from before the digest was recorded are read
    // again, and this run records what their originals hash to
    registry
        .store()
        .execute("UPDATE pseudonym_file SET original_digest = NULL", &[])
        .unwrap();
    let again = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&again), (16, 12, 0, 3, 1), "{again}");
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'written' AND original_digest IS NOT NULL"
        ),
        12,
        "the run a refused purge names is the run that records them"
    );

    // the second cure: an original changed in place, to other bytes of
    // exactly its length, with its modification time put back to the one
    // the row recorded. The size and the time say nothing happened
    let rel = "p0/s1/IM_0001";
    let path = originals.join(rel);
    let row = |registry: &mut Registry| -> (String, String) {
        let r = &rows(
            registry,
            &format!("SELECT original_digest, out_path FROM pseudonym_file WHERE path = '{rel}'"),
        )[0];
        (
            r.opt_text(0).unwrap().unwrap_or_default().to_string(),
            r.text(1).unwrap().to_string(),
        )
    };
    let (before, out_path) = row(&mut registry);
    let was = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let n = bytes.len();
    for b in &mut bytes[n - 64..] {
        *b ^= 0xFF;
    }
    std::fs::write(&path, &bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(was))
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, n);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), was);

    let third = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(
        files_of(&third),
        (16, 1, 11, 3, 1),
        "the run notices by content what the two numbers hide: {third}"
    );
    let (after, _) = row(&mut registry);
    assert_ne!(after, before);
    assert_eq!(after, hex::encode(blake2::Blake2s256::digest(&bytes)));
    assert!(
        std::fs::read(anon.join(&out_path))
            .unwrap()
            .ends_with(&bytes[n - 64..]),
        "the copy holds the bytes the original holds now"
    );
}

/// v0's copy of an original: the same UIDs, the code in `PatientID`, no
/// name and no birth date, as `dcm-raw` holds it.
fn v0_copy(code: &str, patient: &str, study: u32, series: u32, instance: u32) -> Vec<u8> {
    let study_uid = format!("1.2.826.0.1.3680043.8.498.{patient}.{study}");
    let series_uid = format!("{study_uid}.{series}");
    let sop = format!("{series_uid}.{instance}");
    let mut e = synth::minimal_mr(&study_uid, &series_uid, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, code));
    e.push(synth::text(tags::PATIENT_SEX, VR::CS, "F"));
    e.push(synth::text(
        tags::STUDY_DATE,
        VR::DA,
        &format!("2024010{study}"),
    ));
    e.push(synth::text(
        tags::SERIES_NUMBER,
        VR::IS,
        &series.to_string(),
    ));
    e.push(synth::text(
        tags::INSTANCE_NUMBER,
        VR::IS,
        &instance.to_string(),
    ));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        pixels(study * 100 + series * 10 + instance),
    ));
    synth::part10(&MetaFields::mr(&sop), &e, true)
}

/// v0's 16 hex characters for a person, as `blake2b(..., digest_size=8)`
/// writes them: any sixteen will do here, since the map files them.
const V0_CODES: [&str; 2] = ["771c4326c89c082c", "0a1b2c3d4e5f6071"];

/// A v0 cohort folder after its declaration: the originals of two people,
/// ten files each, and the pseudonymised tree holding v0's copy of every
/// one of them under v0's code.
fn v0_dataset() -> TempDir {
    let dir = TempDir::new("pseudonymize-v0");
    let originals = Path::new("derivatives/dcm-original");
    let anon = Path::new("derivatives/dcm-anon");
    for (p, (patient, _)) in MAPPED.iter().enumerate() {
        let code = V0_CODES[p];
        let mut n = 0;
        for series in 1..=2 {
            for instance in 1..=5 {
                n += 1;
                dir.file(
                    &originals
                        .join(format!("sub-{p}/ses-1/IM_{n:04}"))
                        .display()
                        .to_string(),
                    &original(patient, 1, series, instance, "t1_mprage"),
                );
                dir.file(
                    &anon
                        .join(format!("{code}/ses-1/IM_{n:04}"))
                        .display()
                        .to_string(),
                    &v0_copy(code, patient, 1, series, instance),
                );
            }
        }
    }
    dir
}

/// Lab 26, defect 7: a v0 folder's tree holds every original already, and
/// the pseudonymiser wrote each one again beside v0's copy. Once a digest
/// has read the tree, an original whose SOP instance the tree holds is
/// left as it is, recorded as unchanged against the tree's own file, and
/// the next run finds it so; a new original is written as before.
#[test]
fn an_original_the_tree_already_holds_is_not_written_again() {
    let lab = lab();
    let dir = v0_dataset();
    let anon = dir.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    // v0's map: the numbers to v0's codes
    {
        let mut linkage = registry.open_linkage().unwrap();
        let keys = Subkeys::derive(KEY);
        let rows: Vec<ImportRow> = MAPPED
            .iter()
            .enumerate()
            .map(|(i, (identifier, _))| ImportRow {
                line: i + 2,
                identifier: identifier.to_string(),
                code: V0_CODES[i].to_string(),
            })
            .collect();
        linkage::import(registry.store(), &mut linkage, &keys, "patient-id", &rows).unwrap();
    }
    let place = declare(&mut registry, dir.path(), json!({}));
    let s = settings(&place);
    assert_eq!(outputs(&anon).len(), 20);

    // before any digest, the registry knows nothing of the tree: a dry run
    // would write everything again, which is what bring-in's digest-first
    // step is for
    let mut dry = s.clone();
    dry.dry_run = true;
    let report = pseudonymize(&dry, &mut registry).unwrap();
    assert_eq!(files_of(&report), (20, 20, 0, 0, 0), "{report}");
    assert_eq!(report.in_tree, 0);

    // the tree digested under v0's code shape, read verbatim
    let mut d = nils_digest::Settings::new(anon.clone());
    d.name = "v0".into();
    d.workers = 2;
    d.identity = nils_digest::Rule::parse(
        "identity:\n  id_type: subject-code\n  from:\n    - field: PatientID\n      pattern: '^(?<id>[0-9a-f]{16})$'\n  code: verbatim\n",
    )
    .unwrap();
    let digested = nils_digest::digest(&d, &mut registry).unwrap();
    assert_eq!(digested.parsed, 20, "{digested}");
    assert_eq!(digested.subjects, 2, "{digested}");

    // the run: every original is in the tree already, nothing written
    let report = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&report), (20, 0, 20, 0, 0), "{report}");
    assert_eq!(report.in_tree, 20);
    assert_eq!(report.subjects.seen, 0);
    assert!(
        report
            .to_string()
            .contains("20 original(s) the tree held already"),
        "{report}"
    );
    assert_eq!(outputs(&anon).len(), 20, "the tree holds 20 files, not 40");
    let rows_now = rows(
        &mut registry,
        "SELECT state, out_path, out_size, digest, written_at FROM pseudonym_file ORDER BY path",
    );
    assert_eq!(rows_now.len(), 20);
    for r in &rows_now {
        assert_eq!(r.text(0).unwrap(), "unchanged");
        let out = r.text(1).unwrap();
        assert!(
            out.starts_with(V0_CODES[0]) || out.starts_with(V0_CODES[1]),
            "{out}"
        );
        assert!(anon.join(out).is_file(), "{out}");
        assert_eq!(
            r.int(2).unwrap() as u64,
            std::fs::metadata(anon.join(out)).unwrap().len()
        );
        assert!(r.opt_text(3).unwrap().is_none());
        assert!(r.opt_text(4).unwrap().is_none());
    }
    // the next run finds the records and checks the tree's files
    let again = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&again), (20, 0, 20, 0, 0), "{again}");
    assert_eq!(again.in_tree, 0);
    assert_eq!(outputs(&anon).len(), 20);

    // a new original the tree does not hold is written as before, under
    // the code the map filed
    dir.file(
        "derivatives/dcm-original/sub-0/ses-1/IM_0011",
        &original(MAPPED[0].0, 1, 3, 1, "t2_tse"),
    );
    let third = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&third), (21, 1, 20, 0, 0), "{third}");
    assert_eq!(third.in_tree, 0);
    assert_eq!(outputs(&anon).len(), 21);
    assert_eq!(
        one(
            &mut registry,
            &format!(
                "SELECT COUNT(*) FROM pseudonym_file WHERE state = 'written' AND out_path LIKE '{}/%'",
                V0_CODES[0]
            )
        ),
        1
    );
}

/// Lab 26c, finding 3: a copy corrupted after it was written is no longer
/// what the pseudonymiser recorded, and a purge refuses on it. The advice
/// such a refusal gives is to pseudonymise the dataset again, so the run
/// must write that copy again: the original has not changed, its size and
/// modification time are what they were, and only the copy's digest can
/// tell. It used to see the size alone and leave the corruption standing,
/// which left the person no way out.
#[test]
fn a_copy_that_is_no_longer_what_was_recorded_is_written_again() {
    let lab = lab();
    let dir = dataset();
    let anon = dir.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    import_map(&mut registry);
    let place = declare(&mut registry, dir.path(), json!({}));
    let s = settings(&place);

    let first = pseudonymize(&s, &mut registry).unwrap();
    let written = first.files.written;
    assert!(written > 0, "{first}");
    // a second run leaves every copy alone
    let again = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!((again.files.written, again.files.unchanged), (0, written));

    // one copy changed in place, to other bytes of exactly its length: the
    // original is untouched, so its size and modification time still match
    // the row and only the digest can tell
    let copy = outputs(&anon)[0].clone();
    let recorded = std::fs::read(&copy).unwrap();
    let mut broken = recorded.clone();
    let n = broken.len();
    for b in &mut broken[n - 32..] {
        *b ^= 0xFF;
    }
    std::fs::write(&copy, &broken).unwrap();
    assert_eq!(std::fs::metadata(&copy).unwrap().len() as usize, n);

    let third = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(
        (third.files.written, third.files.unchanged),
        (1, written - 1),
        "the copy that no longer verifies is written again: {third}"
    );
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        recorded,
        "and what stands is what was recorded"
    );
    // the row still points at that copy, with the digest of what is there
    let row = rows(
        &mut registry,
        "SELECT state, digest FROM pseudonym_file WHERE out_path IS NOT NULL AND state = 'written' ORDER BY path",
    );
    assert_eq!(row.len() as u64, written);

    // a copy that is gone is written again the same way
    std::fs::remove_file(&copy).unwrap();
    let fourth = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(
        (fourth.files.written, fourth.files.unchanged),
        (1, written - 1)
    );
    assert!(copy.is_file());
}

#[test]
fn a_run_asked_to_stop_before_it_began_writes_nothing_and_ends_cancelled() {
    let lab = lab();
    let dir = dataset();
    let mut registry = lab.home.open().unwrap();
    import_map(&mut registry);
    let place = declare(&mut registry, dir.path(), json!({}));
    let s = settings(&place);
    let cancel = Cancel::new();
    cancel.request();
    let report = pseudonymize_with(&s, &mut registry, &cancel).unwrap();
    assert_eq!(report.cancelled, Some(Cancelled::Stopped));
    assert_eq!(report.files.seen, 0, "{report}");
    assert!(outputs(&dir.path().join("derivatives/dcm-anon")).is_empty());
    let batch = &rows(
        &mut registry,
        "SELECT state FROM ingest_batch ORDER BY id DESC LIMIT 1",
    )[0];
    assert_eq!(batch.text(0).unwrap(), "cancelled");
    let job = &rows(
        &mut registry,
        "SELECT state FROM job ORDER BY id DESC LIMIT 1",
    )[0];
    assert_eq!(job.text(0).unwrap(), "cancelled");
    // and the run after it does the work
    let report = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(report.files.written, 12, "{report}");
}

#[test]
fn a_dataset_that_codes_unmapped_identifiers_makes_provisional_subjects() {
    let lab = lab();
    let dir = dataset();
    let mut registry = lab.home.open().unwrap();
    let mut place = declare(&mut registry, dir.path(), json!({}));
    place.dataset["unmapped"] = json!("code");
    let s = settings(&place);
    let report = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&report), (16, 15, 0, 0, 1), "{report}");
    assert_eq!(report.subjects.new, 3);
    assert_eq!(report.subjects.seen, 3);
    assert_eq!(report.subjects.provisional, 3);
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM subject WHERE provisional = 1"
        ),
        3
    );
    assert_eq!(
        one(
            &mut registry,
            &format!(
                "SELECT COUNT(*) FROM review_item WHERE kind = '{}'",
                review::PROVISIONAL_KIND
            )
        ),
        3
    );
    assert_eq!(
        one(
            &mut registry,
            &format!(
                "SELECT COUNT(*) FROM review_item WHERE kind = '{}'",
                review::UNMAPPED_KIND
            )
        ),
        0
    );
    // the codes are the registry's own derivation, so a map filed later
    // under the same key names the same subjects
    let codes: Vec<String> = rows(&mut registry, "SELECT code FROM subject ORDER BY code")
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
    let expected = nils_registry::pseudonym::code(Scheme::DEFAULT, KEY, UNMAPPED, 12).code;
    assert!(codes.contains(&expected), "{codes:?}");
    let again = pseudonymize(&s, &mut registry).unwrap();
    assert_eq!(files_of(&again), (16, 0, 15, 0, 1), "{again}");
    assert_eq!(again.subjects.new, 0);
}
