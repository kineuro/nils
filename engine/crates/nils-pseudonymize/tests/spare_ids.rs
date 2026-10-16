// SPDX-License-Identifier: AGPL-3.0-only

//! One person under several identifying IDs that are the same everywhere (a
//! main number and spare ones), and a v0 tree beside the originals: what a
//! map imported before the pseudonymiser does, in each shape a site may
//! write it, what happens when the map comes too late, names another code,
//! leaves the main number out or holds a value that is no number, and how a
//! v0 tree is read under each declaration of what PatientID holds.
//!
//! The numbers are the tax agency's published test numbers, which nobody
//! holds, one made-up coordination number, and one with a wrong check digit;
//! the key is made up.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_pseudonymize::{Report, Settings, pseudonymize};
use nils_registry::home::{Home, InitOptions};
use nils_registry::identity_map::{self, Column, Derive, Map, Role, Row};
use nils_registry::linkage::{self, Subkeys};
use nils_registry::place::{self, New, Role as PlaceRole};
use nils_registry::{Backend, Registry, Scheme};
use serde_json::{Value, json};

const KEY: &[u8] = b"test-reg-key-not-real";

/// The person's main number, the one their code is made from.
const MAIN: &str = "198501012382";
/// Two spare numbers of the same person.
const SPARE: &str = "192501019216";
const SPARE2: &str = "189001619809";
/// A second person.
const OTHER: &str = "201501012395";
/// A spare that is no personnummer: the main number with a wrong check digit.
const NOT_A_NUMBER: &str = "198501012383";

/// v0's site IDs, four letters and four digits, as v0's tree holds them.
const SITE_MAIN: &str = "ABCD1234";
const SITE_OTHER: &str = "EFGH5678";

fn code_of(number: &str) -> String {
    nils_registry::pseudonym::code(Scheme::Blake2b8, KEY, number, 12).code
}

struct Lab {
    home: Home,
    _dir: TempDir,
}

fn lab() -> Lab {
    let dir = TempDir::new("spare-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend: Backend::Sqlite,
        dsn: None,
        schema: None,
        scheme: Scheme::Blake2b8,
        key: "k".to_string(),
        display_length: 12,
        session_scheme: None,
    })
    .unwrap();
    let registry = home.open().unwrap();
    let mut store = registry.open_linkage().unwrap();
    linkage::ensure_id_type(&mut store, "personnummer", None).unwrap();
    Lab { home, _dir: dir }
}

fn pixels(seed: u32) -> Vec<u8> {
    (0..4000u32)
        .map(|i| ((i * 31 + seed) % 253) as u8)
        .collect()
}

fn uids(person: &str, study: u32, series: u32, instance: u32) -> (String, String, String) {
    let study_uid = format!("1.2.826.0.1.3680043.8.498.{person}.{study}");
    let series_uid = format!("{study_uid}.{series}");
    let sop = format!("{series_uid}.{instance}");
    (study_uid, series_uid, sop)
}

/// One original, its number in PatientID; `person` keeps the UIDs of one
/// person's studies apart from another's whatever number the file carries.
fn original(person: &str, number: &str, study: u32, instance: u32) -> Vec<u8> {
    let (study_uid, series_uid, sop) = uids(person, study, 1, instance);
    let mut e = synth::minimal_mr(&study_uid, &series_uid, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, number));
    e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
    e.push(synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19850101"));
    e.push(synth::text(
        tags::STUDY_DATE,
        VR::DA,
        &format!("2024010{study}"),
    ));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
    e.push(synth::text(
        tags::INSTANCE_NUMBER,
        VR::IS,
        &instance.to_string(),
    ));
    e.push(synth::text(tags::SERIES_DESCRIPTION, VR::LO, "t1_mprage"));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        pixels(study * 10 + instance),
    ));
    synth::part10(&MetaFields::mr(&sop), &e, true)
}

/// v0's copy of an original: the same UIDs, the site ID in PatientID, no
/// name and no birth date.
fn v0_copy(person: &str, site: &str, study: u32, instance: u32) -> Vec<u8> {
    let (study_uid, series_uid, sop) = uids(person, study, 1, instance);
    let mut e = synth::minimal_mr(&study_uid, &series_uid, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, site));
    e.push(synth::text(
        tags::STUDY_DATE,
        VR::DA,
        &format!("2024010{study}"),
    ));
    e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "1"));
    e.push(synth::text(
        tags::INSTANCE_NUMBER,
        VR::IS,
        &instance.to_string(),
    ));
    e.push(synth::bytes(
        tags::PIXEL_DATA,
        VR::OW,
        pixels(study * 10 + instance),
    ));
    synth::part10(&MetaFields::mr(&sop), &e, true)
}

/// The originals: person "a" under the main number (study 1, two files),
/// a spare (study 2, two files) and a second spare (study 3, one file);
/// person "b" under their own number (study 1, two files). Seven files.
const FILES: [(&str, &str, &str, u32, u32); 7] = [
    ("a", MAIN, SITE_MAIN, 1, 1),
    ("a", MAIN, SITE_MAIN, 1, 2),
    ("a", SPARE, SITE_MAIN, 2, 1),
    ("a", SPARE, SITE_MAIN, 2, 2),
    ("a", SPARE2, SITE_MAIN, 3, 1),
    ("b", OTHER, SITE_OTHER, 1, 1),
    ("b", OTHER, SITE_OTHER, 1, 2),
];

/// A dataset folder: its originals, and an empty pseudonymised tree, or
/// v0's tree holding a copy of every original but those `v0_lacks` names.
fn dataset(v0: bool, v0_lacks: &[usize]) -> TempDir {
    let dir = TempDir::new("spare-ds");
    for (n, (person, number, site, study, instance)) in FILES.iter().enumerate() {
        dir.file(
            &format!("derivatives/dcm-original/{person}/s{study}/IM_{:04}", n + 1),
            &original(person, number, *study, *instance),
        );
        if v0 && !v0_lacks.contains(&n) {
            dir.file(
                &format!("derivatives/dcm-anon/{site}/ses-{study}/IM_{:04}", n + 1),
                &v0_copy(person, site, *study, *instance),
            );
        }
    }
    std::fs::create_dir_all(dir.path().join("derivatives/dcm-anon")).unwrap();
    dir
}

/// The dataset declared as the desk's Add a dataset declares a folder whose
/// originals hold an ID that is the same everywhere: identified, the rule
/// PatientID as personnummer, unmapped files held; and what PatientID gets
/// and what names each copy's folder, as The IDs sets them.
fn declare(
    registry: &mut Registry,
    dir: &Path,
    patient_id: &str,
    copy_folder: &str,
) -> place::Place {
    let store = registry.store();
    let id = place::add(
        store,
        &New {
            name: "lab",
            role: PlaceRole::Source,
            path: dir.to_str().unwrap(),
            guarantees: json!({}),
            probed: Value::Null,
            handling: Value::Null,
            dataset: json!({
                "arrives": "identified",
                "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
                "identity": {"id_type": "personnummer", "from": [{"field": "PatientID"}]},
                "unmapped": "hold",
                "patient_id": patient_id,
                "copy_folder": copy_folder,
            }),
        },
    )
    .unwrap();
    place::show(store, id).unwrap().unwrap()
}

fn settings(place: &place::Place) -> Settings {
    let mut s = Settings::for_dataset(place).unwrap();
    s.name = "lab-run".into();
    s.workers = 2;
    s.walk_threads = 1;
    s.batch_rows = 3;
    s
}

/// A map imported as the Pseudonyms page and `nils linkage import` import it.
fn import(
    registry: &mut Registry,
    columns: &[(&str, Role)],
    rows: &[&[&str]],
    make_types: bool,
) -> identity_map::Report {
    let mut store = registry.open_linkage().unwrap();
    let keys = Subkeys::derive(KEY);
    let columns: Vec<Column> = columns
        .iter()
        .map(|(header, role)| Column {
            header: header.to_string(),
            role: role.clone(),
        })
        .collect();
    let rows: Vec<Row> = rows
        .iter()
        .enumerate()
        .map(|(i, cells)| Row {
            line: i + 2,
            cells: cells.iter().map(|c| c.to_string()).collect(),
        })
        .collect();
    identity_map::import(
        registry.store(),
        &mut store,
        &keys,
        Some(&Derive {
            scheme: Scheme::Blake2b8,
            key: KEY,
            display_length: 12,
        }),
        &Map {
            columns: &columns,
            rows: &rows,
            dry_run: false,
            make_types,
            place_id: None,
            actor: "tester@lab",
            job_id: None,
        },
    )
    .unwrap()
}

/// A map's columns and its rows.
type MapShape<'a> = (Vec<(&'a str, Role)>, Vec<Vec<&'a str>>);

fn pn() -> Role {
    Role::Identifier("personnummer".into())
}

fn query(registry: &mut Registry, sql: &str) -> Vec<nils_registry::Row> {
    registry.store().query(sql, &[]).unwrap()
}

fn one(registry: &mut Registry, sql: &str) -> i64 {
    query(registry, sql)[0].int(0).unwrap()
}

/// The codes of the subjects not merged into another.
fn subjects(registry: &mut Registry) -> BTreeSet<String> {
    query(
        registry,
        "SELECT code FROM subject WHERE merged_into IS NULL",
    )
    .iter()
    .map(|r| r.text(0).unwrap().to_string())
    .collect()
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

fn patient_id(path: &Path) -> String {
    let ds = dicom_object::open_file(path).unwrap();
    let e = ds.get(tags::PATIENT_ID).unwrap();
    e.value()
        .to_str()
        .unwrap()
        .trim_matches(['\0', ' '])
        .to_string()
}

/// The pseudonymised tree by its top folders: how many files each holds and
/// the PatientIDs they carry.
fn tree(anon: &Path) -> BTreeMap<String, (usize, BTreeSet<String>)> {
    let mut out: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for p in outputs(anon) {
        let top = p
            .strip_prefix(anon)
            .unwrap()
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        let e = out.entry(top).or_default();
        e.0 += 1;
        e.1.insert(patient_id(&p));
    }
    out
}

fn folders(entries: &[(&str, usize, &[&str])]) -> BTreeMap<String, (usize, BTreeSet<String>)> {
    entries
        .iter()
        .map(|(top, n, ids)| {
            (
                top.to_string(),
                (*n, ids.iter().map(|s| s.to_string()).collect()),
            )
        })
        .collect()
}

fn files_of(report: &Report) -> (u64, u64, u64, u64, u64) {
    let f = &report.files;
    (f.seen, f.written, f.unchanged, f.held, f.refused)
}

/// The subject each original's row was written under, by the original's
/// number, as the registry recorded it.
fn subject_by_file(registry: &mut Registry) -> BTreeMap<String, BTreeSet<String>> {
    let rows = query(
        registry,
        "SELECT p.path, s.code FROM pseudonym_file p JOIN subject s ON s.id = p.subject_id \
         WHERE p.state = 'written' ORDER BY p.path",
    );
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in &rows {
        let path = r.text(0).unwrap();
        let n: usize = path[path.len() - 4..].parse().unwrap();
        out.entry(FILES[n - 1].1.to_string())
            .or_default()
            .insert(r.text(1).unwrap().to_string());
    }
    out
}

/// A digest of the pseudonymised tree under the rule the engine stores for
/// an identified dataset (`nils::dataset::stored_rule` and `unmapped_of`):
/// the code verbatim under `subject-code`, or the value of the declared id
/// type through the linkage store; a value the rule cannot read held, and
/// an identifier no subject holds held.
fn digest_anon(registry: &mut Registry, anon: &Path, declared: &str) -> nils_digest::Report {
    let mut d = nils_digest::Settings::new(anon.to_path_buf());
    d.name = "lab-read".into();
    d.workers = 2;
    d.walk_threads = 1;
    let mut rule = match declared.strip_prefix("id-type:") {
        None => {
            let mut rule = nils_digest::Rule::parse(
                "identity:\n  id_type: subject-code\n  from:\n    - field: PatientID\n      pattern: '^(?<id>[0-9a-hjkmnp-tv-z]+)$'\n  code: verbatim\n",
            )
            .unwrap();
            rule.own_codes = true;
            rule
        }
        Some(name) => nils_digest::Rule::parse(&format!(
            "identity:\n  id_type: {name}\n  from:\n    - field: PatientID\n"
        ))
        .unwrap(),
    };
    rule.holds_unread = true;
    d.identity = rule;
    d.unmapped = nils_digest::Unmapped::Hold;
    nils_digest::digest(&d, registry).unwrap()
}

/// A map of number to subject code, every number on its own row and each
/// spare carrying the main number's code, imported before the run: every file
/// of a spare number lands on the main subject, under the main number's code.
/// The map makes the subjects; the run makes none.
#[test]
fn a_map_of_number_and_code_puts_every_spare_number_on_the_main_subject() {
    let lab = lab();
    let data = dataset(false, &[]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    let r = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
        false,
    );
    assert!(r.written(), "{r}");
    assert_eq!(r.rows, 4);
    assert_eq!((r.subjects.named, r.subjects.new), (2, 2), "{r}");
    assert_eq!((r.identifiers.filed, r.identifiers.new), (4, 4), "{r}");
    // two numbers joined a subject that names one already: the spares
    assert_eq!(r.further, 2, "{r}");
    assert!(r.merges.is_empty() && r.conflicts.is_empty());

    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (7, 7, 0, 0, 0), "{report}");
    assert_eq!(report.subjects.new, 0, "{report}");
    assert_eq!(report.subjects.seen, 2, "{report}");

    // two subjects, the main number's code and the other's; none of a spare
    assert_eq!(subjects(&mut registry), [a.clone(), b.clone()].into());
    assert!(!subjects(&mut registry).contains(&code_of(SPARE)));
    assert!(!subjects(&mut registry).contains(&code_of(SPARE2)));
    // the five files of the three numbers under the one code, PatientID too
    assert_eq!(
        tree(&anon),
        folders(&[
            (a.as_str(), 5, &[a.as_str()]),
            (b.as_str(), 2, &[b.as_str()])
        ])
    );
    assert_eq!(
        subject_by_file(&mut registry),
        [
            (MAIN.to_string(), [a.clone()].into()),
            (SPARE.to_string(), [a.clone()].into()),
            (SPARE2.to_string(), [a.clone()].into()),
            (OTHER.to_string(), [b.clone()].into()),
        ]
        .into()
    );
    // the linkage store: three numbers on the main subject, kept as keyed
    // lookups only (a personnummer's ciphertext is empty)
    let mut store = registry.open_linkage().unwrap();
    let held = store
        .query(
            "SELECT COUNT(*), SUM(LENGTH(ciphertext)) FROM identity i JOIN id_type t ON t.id = i.id_type_id \
             WHERE t.name = 'personnummer'",
            &[],
        )
        .unwrap();
    assert_eq!(held[0].int(0).unwrap(), 4);
    assert_eq!(held[0].opt_int(1).unwrap().unwrap_or(0), 0);
}

/// The same map in the other shapes the importer takes: the main number as
/// the canonical column and the spares as identifier columns of the same
/// type on its row, or several identifier columns of the type beside a code
/// column. Every shape gives the same subjects and the same tree.
#[test]
fn the_other_shapes_of_the_map_give_the_same_subjects_and_tree() {
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    let expected = folders(&[
        (a.as_str(), 5, &[a.as_str()]),
        (b.as_str(), 2, &[b.as_str()]),
    ]);
    let shapes: Vec<MapShape> = vec![
        (
            vec![
                ("main", Role::Canonical("personnummer".into())),
                ("spare", pn()),
                ("spare2", pn()),
            ],
            vec![vec![MAIN, SPARE, SPARE2], vec![OTHER, "", ""]],
        ),
        (
            vec![
                ("pn1", pn()),
                ("pn2", pn()),
                ("pn3", pn()),
                ("code", Role::Code),
            ],
            vec![vec![MAIN, SPARE, SPARE2, &a], vec![OTHER, "", "", &b]],
        ),
    ];
    for (columns, rows) in shapes {
        let lab = lab();
        let data = dataset(false, &[]);
        let anon = data.path().join("derivatives/dcm-anon");
        let mut registry = lab.home.open().unwrap();
        let rows: Vec<&[&str]> = rows.iter().map(Vec::as_slice).collect();
        let r = import(&mut registry, &columns, &rows, false);
        assert!(r.written(), "{r}");
        assert_eq!(
            (r.subjects.named, r.identifiers.new, r.further),
            (2, 4, 2),
            "{r}"
        );
        let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (7, 7, 0, 0, 0), "{report}");
        assert_eq!(report.subjects.new, 0, "{report}");
        assert_eq!(subjects(&mut registry), [a.clone(), b.clone()].into());
        assert_eq!(tree(&anon), expected);
    }
}

/// A spare number the generator coded before the map came: the run made a
/// subject of it. Number to code then refuses the row (the number maps to
/// another subject) and writes nothing; the canonical shape merges the
/// spare's subject into the main one. The registry then reads one person,
/// but the copies written before keep the spare's code in PatientID and in
/// their folder: the tree is not rewritten.
#[test]
fn a_spare_number_coded_before_its_map_is_refused_or_merged_and_its_copies_stay() {
    let lab = lab();
    let data = dataset(false, &[]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b, s, s2) = (
        code_of(MAIN),
        code_of(OTHER),
        code_of(SPARE),
        code_of(SPARE2),
    );
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    let first = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&first), (7, 7, 0, 0, 0), "{first}");
    assert_eq!(first.subjects.new, 4, "one subject per number: {first}");
    assert_eq!(
        subjects(&mut registry),
        [a.clone(), b.clone(), s.clone(), s2.clone()].into()
    );

    // number to code: refused, nothing written
    let refused = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
        false,
    );
    assert!(!refused.written(), "{refused}");
    let whys: Vec<String> = refused.conflicts.iter().map(|c| c.to_string()).collect();
    assert_eq!(whys.len(), 2, "{whys:?}");
    assert!(
        whys.iter().all(|w| w.contains("already maps to subject")),
        "{whys:?}"
    );
    assert_eq!(subjects(&mut registry).len(), 4);

    // the canonical shape: the two spares' subjects merged into the main one
    let merged = import(
        &mut registry,
        &[
            ("main", Role::Canonical("personnummer".into())),
            ("spare", pn()),
            ("spare2", pn()),
        ],
        &[&[MAIN, SPARE, SPARE2]],
        false,
    );
    assert!(merged.written(), "{merged}");
    assert_eq!(merged.merges.len(), 2, "{merged}");
    assert_eq!(subjects(&mut registry), [a.clone(), b.clone()].into());
    // every row of the spares now names the main subject
    assert_eq!(
        subject_by_file(&mut registry)
            .into_iter()
            .filter(|(n, _)| n != OTHER)
            .flat_map(|(_, c)| c)
            .collect::<BTreeSet<_>>(),
        [a.clone()].into()
    );
    // the copies are not rewritten: four folders, the spares' codes kept
    let again = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&again), (7, 0, 7, 0, 0), "{again}");
    assert_eq!(
        tree(&anon),
        folders(&[
            (a.as_str(), 2, &[a.as_str()]),
            (b.as_str(), 2, &[b.as_str()]),
            (s.as_str(), 2, &[s.as_str()]),
            (s2.as_str(), 1, &[s2.as_str()]),
        ])
    );
    // a digest of the tree reads the spares' codes through the merge: one
    // person in the registry, two subjects in all
    let read = digest_anon(&mut registry, &anon, "subject-code");
    assert_eq!(read.parsed, 7, "{read}");
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(DISTINCT subject_id) FROM series"
        ),
        2
    );
    let main_id = one(
        &mut registry,
        &format!("SELECT id FROM subject WHERE code = '{a}'"),
    );
    assert_eq!(
        one(
            &mut registry,
            &format!("SELECT COUNT(*) FROM study WHERE subject_id = {main_id}")
        ),
        3
    );
}

/// A map whose code for the main number is not the generator's: the map's
/// code stands, for the main number and its spares, and no subject has the
/// generator's code. Where the generator's subject was made first, the map
/// is refused instead.
#[test]
fn a_map_code_that_is_not_the_generators_stands_unless_the_generator_came_first() {
    let lab = lab();
    let data = dataset(false, &[]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let given = "00112233445566ff";
    let b = code_of(OTHER);
    let r = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[
            &[MAIN, given],
            &[SPARE, given],
            &[SPARE2, given],
            &[OTHER, &b],
        ],
        false,
    );
    assert!(r.written(), "{r}");
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (7, 7, 0, 0, 0), "{report}");
    assert_eq!(
        subjects(&mut registry),
        [given.to_string(), b.clone()].into()
    );
    assert_eq!(
        tree(&anon),
        folders(&[(given, 5, &[given]), (b.as_str(), 2, &[b.as_str()])])
    );

    // the generator first: the main number is its subject's, and the map
    // naming another code for it is refused
    let second = self::lab();
    let data = dataset(false, &[]);
    let mut registry = second.home.open().unwrap();
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    pseudonymize(&settings(&place), &mut registry).unwrap();
    let late = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, given]],
        false,
    );
    assert!(!late.written(), "{late}");
    assert!(
        late.conflicts[0]
            .to_string()
            .contains("already maps to subject")
    );
}

/// A map that names the spare with the main number's code and leaves the
/// main number out: the main number's files derive that code, find its
/// subject holding another personnummer, and the run ends on a collision
/// with a review item open. The map must name the main number too.
#[test]
fn a_map_without_the_main_number_ends_the_run_on_a_collision() {
    let lab = lab();
    let data = dataset(false, &[]);
    let mut registry = lab.home.open().unwrap();
    let a = code_of(MAIN);
    let r = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[SPARE, &a]],
        false,
    );
    assert!(r.written(), "{r}");
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    let failed = pseudonymize(&settings(&place), &mut registry).unwrap_err();
    let why = failed.to_string();
    assert!(
        why.contains("identity collision under personnummer"),
        "{why}"
    );
    assert!(why.contains("already holds another identifier"), "{why}");
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(*) FROM review_item WHERE kind = 'identity.collision' AND status = 'open'"
        ),
        1
    );
}

/// A spare that is no personnummer (a wrong check digit, as a reserve number
/// may have): the number map refuses its row and writes nothing, and the run
/// holds its files as a reserve number's, under the number's keyed lookup and
/// by its shape, nothing of it kept. A map row naming it under
/// `reserve-number` beside the subject code releases them, and the next run
/// of the held files writes them under the main subject.
#[test]
fn a_spare_that_is_no_personnummer_is_refused_by_the_map_and_held_by_the_run() {
    let lab = lab();
    let data = dataset(false, &[]);
    data.file(
        "derivatives/dcm-original/a/s4/IM_0099",
        &original("a", NOT_A_NUMBER, 4, 1),
    );
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    let refused = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[
            &[MAIN, &a],
            &[SPARE, &a],
            &[SPARE2, &a],
            &[NOT_A_NUMBER, &a],
            &[OTHER, &b],
        ],
        false,
    );
    assert!(!refused.written(), "{refused}");
    assert_eq!(refused.conflicts.len(), 1, "{refused}");
    assert_eq!(refused.conflicts[0].row, 5);
    assert!(
        refused.conflicts[0].to_string().contains("check digit"),
        "{refused}"
    );
    // without that row the map stands
    let r = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
        false,
    );
    assert!(r.written(), "{r}");
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (8, 7, 0, 1, 0), "{report}");
    let held = query(
        &mut registry,
        "SELECT id_type, shape FROM pseudonym_file WHERE state = 'held'",
    );
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].text(0).unwrap(), "reserve-number");
    // held, and shown, by the shape of the number
    assert_eq!(held[0].text(1).unwrap(), "999999999999");

    // the number mapped as a reserve number to the subject code releases it
    let released = import(
        &mut registry,
        &[
            ("reserve", Role::Identifier("reserve-number".into())),
            ("code", Role::Code),
        ],
        &[&[NOT_A_NUMBER, &a]],
        false,
    );
    assert!(released.written(), "{released}");
    assert_eq!(released.held_released, 1, "{released}");
    let mut held_run = settings(&place);
    held_run.held = true;
    let report = pseudonymize(&held_run, &mut registry).unwrap();
    assert_eq!(files_of(&report).1, 1, "{report}");
    assert_eq!(
        tree(&anon),
        folders(&[
            (a.as_str(), 6, &[a.as_str()]),
            (b.as_str(), 2, &[b.as_str()])
        ])
    );
    assert_eq!(subjects(&mut registry), [a.clone(), b.clone()].into());
}

/// A v0 cohort folder under `patient_id: subject-code` (the desk's default for
/// an identified dataset): v0's tree beside the originals holds the site ID
/// in PatientID. Bring-in digests the tree first; the declaration reads
/// codes, so every v0 file is held and none is filed. The pseudonymiser then
/// finds no original in the tree and writes every one again, under the
/// codes, beside v0's folders: the tree holds two copies of every file and
/// the registry reads only the new ones.
#[test]
fn a_v0_tree_read_as_subject_codes_is_held_whole_and_every_original_is_written_again() {
    let lab = lab();
    let data = dataset(true, &[]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    let r = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
        false,
    );
    assert!(r.written(), "{r}");
    let place = declare(&mut registry, data.path(), "subject-code", "subject-code");
    assert_eq!(outputs(&anon).len(), 7);

    // bring-in's first step: the tree digested by its declaration
    let first = digest_anon(&mut registry, &anon, "subject-code");
    assert_eq!(first.parsed, 7, "{first}");
    let w = first.written.as_ref().unwrap();
    assert_eq!((w.ingested, w.held), (0, 7), "{first}");
    assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM instance"), 0);

    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (7, 7, 0, 0, 0), "{report}");
    assert_eq!(report.in_tree, 0, "{report}");
    assert_eq!(
        tree(&anon),
        folders(&[
            (SITE_MAIN, 5, &[SITE_MAIN]),
            (SITE_OTHER, 2, &[SITE_OTHER]),
            (a.as_str(), 5, &[a.as_str()]),
            (b.as_str(), 2, &[b.as_str()]),
        ])
    );
    // the digest after it: the new copies filed, v0's held again
    let second = digest_anon(&mut registry, &anon, "subject-code");
    let w = second.written.as_ref().unwrap();
    assert_eq!((w.ingested, w.held), (7, 7), "{second}");
    assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM instance"), 7);
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(DISTINCT subject_id) FROM series"
        ),
        2
    );
}

/// A v0 cohort folder under `patient_id: id-type:site-id` with each copy's
/// folder named by it, the site ID's
/// type and the map of site ID to subject code imported with the number map
/// first: bring-in's first digest files every v0 file under its subject, the
/// pseudonymiser leaves every original the tree holds as it is, and writes
/// only what v0's tree lacks, a spare number's file here, into the main
/// subject's site-ID folder with the site ID in PatientID.
#[test]
fn a_v0_tree_read_by_its_site_id_is_kept_and_only_what_it_lacks_is_written() {
    let lab = lab();
    // v0's tree lacks the spare number's second file
    let data = dataset(true, &[3]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    let numbers = import(
        &mut registry,
        &[("pn", pn()), ("code", Role::Code)],
        &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
        false,
    );
    assert!(numbers.written(), "{numbers}");
    let sites = import(
        &mut registry,
        &[
            ("site", Role::Identifier("site-id".into())),
            ("code", Role::Code),
        ],
        &[&[SITE_MAIN, &a], &[SITE_OTHER, &b]],
        true,
    );
    assert!(sites.written(), "{sites}");
    assert_eq!(sites.identifiers.types_new, 1, "{sites}");
    assert_eq!(sites.subjects.known, 2, "{sites}");
    let place = declare(&mut registry, data.path(), "id-type:site-id", "id-type");
    assert_eq!(outputs(&anon).len(), 6);

    let first = digest_anon(&mut registry, &anon, "id-type:site-id");
    let w = first.written.as_ref().unwrap();
    assert_eq!(
        (w.ingested, w.held, w.subjects_created),
        (6, 0, 0),
        "{first}"
    );
    assert_eq!(w.subjects_matched, 2, "{first}");

    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (7, 1, 6, 0, 0), "{report}");
    assert_eq!(report.in_tree, 6, "{report}");
    assert_eq!(report.subjects.new, 0, "{report}");
    assert_eq!(
        tree(&anon),
        folders(&[(SITE_MAIN, 5, &[SITE_MAIN]), (SITE_OTHER, 2, &[SITE_OTHER])])
    );
    // the one written is the spare's, on the main subject
    assert_eq!(
        subject_by_file(&mut registry),
        [(SPARE.to_string(), [a.clone()].into())].into()
    );
    let second = digest_anon(&mut registry, &anon, "id-type:site-id");
    let w = second.written.as_ref().unwrap();
    assert_eq!((w.ingested, w.held), (1, 0), "{second}");
    assert_eq!(one(&mut registry, "SELECT COUNT(*) FROM instance"), 7);
    assert_eq!(subjects(&mut registry), [a.clone(), b.clone()].into());
    assert_eq!(
        one(
            &mut registry,
            "SELECT COUNT(DISTINCT subject_id) FROM series"
        ),
        2
    );
}

/// The same declaration with v0's tree moved out of the dataset before it
/// is added: nothing to digest first, every original written under its
/// subject's site ID, the spare's files with the main subject's.
#[test]
fn without_v0_s_tree_every_original_is_written_under_its_subject_s_site_id() {
    let lab = lab();
    let data = dataset(false, &[]);
    let anon = data.path().join("derivatives/dcm-anon");
    let mut registry = lab.home.open().unwrap();
    let (a, b) = (code_of(MAIN), code_of(OTHER));
    assert!(
        import(
            &mut registry,
            &[("pn", pn()), ("code", Role::Code)],
            &[&[MAIN, &a], &[SPARE, &a], &[SPARE2, &a], &[OTHER, &b]],
            false,
        )
        .written()
    );
    assert!(
        import(
            &mut registry,
            &[
                ("site", Role::Identifier("site-id".into())),
                ("code", Role::Code)
            ],
            &[&[SITE_MAIN, &a], &[SITE_OTHER, &b]],
            true,
        )
        .written()
    );
    let place = declare(&mut registry, data.path(), "id-type:site-id", "id-type");
    let report = pseudonymize(&settings(&place), &mut registry).unwrap();
    assert_eq!(files_of(&report), (7, 7, 0, 0, 0), "{report}");
    assert_eq!(report.subjects.new, 0, "{report}");
    assert_eq!(
        tree(&anon),
        folders(&[(SITE_MAIN, 5, &[SITE_MAIN]), (SITE_OTHER, 2, &[SITE_OTHER])])
    );
    let read = digest_anon(&mut registry, &anon, "id-type:site-id");
    let w = read.written.as_ref().unwrap();
    assert_eq!(
        (w.ingested, w.held, w.subjects_created),
        (7, 0, 0),
        "{read}"
    );
    assert_eq!(subjects(&mut registry), [a, b].into());
}
