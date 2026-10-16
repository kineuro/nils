// SPDX-License-Identifier: AGPL-3.0-only

//! A hospital's reserve number beside an ID that is the same everywhere
//! (Nima, 2026-10-16): a dataset whose rule reads PatientID as a
//! personnummer reads a PatientID that is none as a reserve number. Found
//! under `reserve-number`, the file is that subject's and is written like any
//! other; not found, it is held under the number's keyed lookup until a map
//! names the number, and the next run writes it. No code is ever made from a
//! reserve number, whatever the dataset says of an identifier no map names
//! and whoever asks; the registry keeps none of it but its keyed lookup. A
//! file an earlier engine held under its study's UID is released by a map of
//! that UID as it always was. On both backends.
//!
//! The personnummer are the tax agency's published test numbers, which
//! nobody holds; the reserve numbers are made up, twelve digits that name no
//! date; the key is made up.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_pseudonymize::{Report, Settings, pseudonymize};
use nils_registry::home::{Home, InitOptions};
use nils_registry::identity_map::{self, Column, Derive, Map, Role, Row};
use nils_registry::linkage::Subkeys;
use nils_registry::place::{self, New, Role as PlaceRole};
use nils_registry::schema::Type;
use nils_registry::{Backend, Param, Registry, Scheme, Store};
use serde_json::{Value, json};

const KEY: &[u8] = b"test-reg-key-not-real";

/// A person's main number, and a second person's.
const MAIN: &str = "198501012382";
const OTHER: &str = "201501012395";
/// The first person's reserve number, as their spare, written as a hospital
/// writes it, and in its one written form.
const SPARE: &str = "19991324-1234";
const SPARE_FORM: &str = "199913241234";
/// The reserve number of a person with no personnummer, written with
/// spaces, and in its one written form.
const ALONE: &str = " 19991325 5678";
const ALONE_FORM: &str = "199913255678";
/// A reserve number no map names until later.
const LATER: &str = "199913269012";
/// The codes a map gives the person known only by a reserve number and the
/// one named later: a site's own, given as they are.
const ALONE_CODE: &str = "00000000000000c1";
const LATER_CODE: &str = "00000000000000d1";

/// The originals: person `a` under the main number (study 1, two files) and
/// the spare reserve number (study 2, two files); person `b` under their own
/// number (one file); person `c` known only by a reserve number (two files);
/// person `d` under a reserve number no map names yet (one file).
const FILES: [(&str, &str, u32, u32); 8] = [
    ("a", MAIN, 1, 1),
    ("a", MAIN, 1, 2),
    ("a", SPARE, 2, 1),
    ("a", SPARE, 2, 2),
    ("b", OTHER, 1, 1),
    ("c", ALONE, 1, 1),
    ("c", ALONE, 1, 2),
    ("d", LATER, 1, 1),
];

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_reserve_ids_test";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
}

fn drop_schemas(dsn: &str) {
    if let Ok(mut store) = Store::connect_postgres(dsn, "public") {
        let _ = store.batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ));
    }
}

struct Lab {
    name: &'static str,
    home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self._guard.is_some()
            && let Some(dsn) = postgres_dsn()
        {
            drop_schemas(&dsn);
        }
    }
}

/// A registry of the subject code generator's scheme under the made-up key,
/// on one backend.
fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let guard =
        (backend == Backend::Postgres).then(|| POSTGRES.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(dsn) = &dsn {
        drop_schemas(dsn);
    }
    let dir = TempDir::new("reserve-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", KEY).unwrap();
    home.init(&InitOptions {
        backend,
        dsn,
        schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
        scheme: Scheme::Blake2b8,
        key: "k".to_string(),
        display_length: 12,
        session_scheme: None,
    })
    .unwrap();
    Lab {
        name,
        home,
        _dir: dir,
        _guard: guard,
    }
}

/// SQLite always, and Postgres where CI or the bench gives one.
fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    match postgres_dsn() {
        Some(dsn) => out.push(lab("postgres", Backend::Postgres, Some(dsn))),
        None => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
    out
}

fn code_of(number: &str) -> String {
    nils_registry::pseudonym::code(Scheme::Blake2b8, KEY, number, 12).code
}

fn study_uid(person: &str, study: u32) -> String {
    let n = person.bytes().next().unwrap_or(b'a') as u32;
    format!("1.2.826.0.1.3680043.8.498.{n}.{study}")
}

fn original(person: &str, patient_id: &str, study: u32, instance: u32) -> Vec<u8> {
    let study_uid = study_uid(person, study);
    let series_uid = format!("{study_uid}.1");
    let sop = format!("{series_uid}.{instance}");
    let mut e = synth::minimal_mr(&study_uid, &series_uid, &sop);
    e.push(synth::text(tags::PATIENT_ID, VR::LO, patient_id));
    e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
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
        (0..2000u32)
            .map(|i| ((i * 7 + study * 13 + instance) % 251) as u8)
            .collect(),
    ));
    synth::part10(&MetaFields::mr(&sop), &e, true)
}

/// The original at `FILES[n]`, under its person's folder.
fn write_original(dir: &TempDir, n: usize) {
    let (person, patient_id, study, instance) = FILES[n];
    dir.file(
        &format!("derivatives/dcm-original/{person}/s{study}/IM_{:04}", n + 1),
        &original(person, patient_id, study, instance),
    );
}

/// A dataset folder holding the originals and an empty pseudonymised tree.
fn dataset() -> TempDir {
    let dir = TempDir::new("reserve-ds");
    for n in 0..FILES.len() {
        write_original(&dir, n);
    }
    std::fs::create_dir_all(dir.path().join("derivatives/dcm-anon")).unwrap();
    dir
}

/// The dataset declared as the desk's Add a dataset declares a folder whose
/// originals hold an ID that is the same everywhere: identified, the rule
/// PatientID as personnummer; what an identifier no map names does, what
/// PatientID gets and what names each copy's folder as given.
fn declare(
    registry: &mut Registry,
    dir: &Path,
    unmapped: &str,
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
                "unmapped": unmapped,
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

/// A map as the Pseudonyms page and `nils linkage import` file it.
fn import(
    registry: &mut Registry,
    columns: &[(&str, &str)],
    rows: &[&[&str]],
    dry_run: bool,
) -> identity_map::Report {
    let mut linkage = registry.open_linkage().unwrap();
    let keys = Subkeys::derive(KEY);
    let columns: Vec<Column> = columns
        .iter()
        .map(|(header, role)| Column {
            header: header.to_string(),
            role: Role::parse(role).unwrap(),
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
        &mut linkage,
        &keys,
        Some(&Derive {
            scheme: Scheme::Blake2b8,
            key: KEY,
            display_length: 12,
        }),
        &Map {
            columns: &columns,
            rows: &rows,
            dry_run,
            make_types: false,
            place_id: None,
            actor: "tester@lab",
            job_id: None,
        },
    )
    .unwrap()
}

fn one(store: &mut Store, sql: &str) -> i64 {
    store.query(sql, &[]).unwrap()[0].int(0).unwrap()
}

/// The codes of the registry's subjects.
fn subjects(registry: &mut Registry) -> BTreeSet<String> {
    registry
        .store()
        .query("SELECT code FROM subject WHERE merged_into IS NULL", &[])
        .unwrap()
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
    ds.get(tags::PATIENT_ID)
        .unwrap()
        .value()
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

fn folders(entries: &[(&str, usize, &str)]) -> BTreeMap<String, (usize, BTreeSet<String>)> {
    entries
        .iter()
        .map(|(top, n, id)| (top.to_string(), (*n, BTreeSet::from([id.to_string()]))))
        .collect()
}

/// (seen, written, unchanged, held, refused)
fn files_of(report: &Report) -> (u64, u64, u64, u64, u64) {
    let f = &report.files;
    (f.seen, f.written, f.unchanged, f.held, f.refused)
}

/// The held rows: their type, their shape, whether anything is sealed, and
/// their lookup.
fn held(registry: &mut Registry) -> Vec<(String, String, bool, Vec<u8>)> {
    registry
        .store()
        .query(
            "SELECT id_type, shape, sealed IS NOT NULL, lookup FROM pseudonym_file \
             WHERE state = 'held' ORDER BY path",
            &[],
        )
        .unwrap()
        .iter()
        .map(|r| {
            let sealed = match r.get(2) {
                nils_registry::store::Cell::Bool(b) => *b,
                nils_registry::store::Cell::Int(n) => *n != 0,
                _ => true,
            };
            (
                r.text(0).unwrap().to_string(),
                r.text(1).unwrap().to_string(),
                sealed,
                r.bytes(3).unwrap().to_vec(),
            )
        })
        .collect()
}

/// How many identities of a type the linkage store holds, and how many of
/// them keep anything but their lookup.
fn identities(registry: &mut Registry, id_type: &str) -> (i64, i64) {
    let mut linkage = registry.open_linkage().unwrap();
    let of = format!(
        "SELECT COUNT(*) FROM identity i JOIN id_type t ON t.id = i.id_type_id WHERE t.name = '{id_type}'"
    );
    (
        one(&mut linkage, &of),
        one(&mut linkage, &format!("{of} AND length(i.ciphertext) > 0")),
    )
}

/// No written form of any number is in any file of the home: both stores,
/// their journals, the key store. Only where the stores are files.
fn nothing_kept_in(home: &Path, numbers: &[&str]) {
    let mut forms: Vec<String> = Vec::new();
    for n in numbers {
        let compact: String = n.chars().filter(|c| c.is_ascii_digit()).collect();
        forms.push(compact.clone());
        forms.push(n.trim().to_string());
        forms.push(format!("{}-{}", &compact[..8], &compact[8..]));
    }
    let mut queue = vec![home.to_path_buf()];
    let mut read = 0;
    while let Some(d) = queue.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                queue.push(p);
                continue;
            }
            let bytes = std::fs::read(&p).unwrap();
            read += 1;
            for form in &forms {
                assert!(
                    !bytes.windows(form.len()).any(|w| w == form.as_bytes()),
                    "a form of a number is in {}",
                    p.display()
                );
            }
        }
    }
    assert!(read >= 2, "the stores were read");
}

/// A map that names a person's spare reserve number beside their main
/// number's code, and the reserve number of a person who has no
/// personnummer beside the code their site gave them, filed before the
/// first run: every file of the first person lands on one subject under the
/// main number's code, the second person's under theirs, and the run makes
/// no subject but the second personnummer's. A reserve number no map names
/// is held under its own keyed lookup, nothing sealed and its subject never
/// made; a map row naming it releases it, and the next run writes it. The
/// registry keeps no reserve number but its lookup.
#[test]
fn a_reserve_number_is_found_through_its_map_and_held_until_one_names_it() {
    for lab in labs() {
        let name = lab.name;
        let data = dataset();
        let anon = data.path().join("derivatives/dcm-anon");
        let mut registry = lab.home.open().unwrap();
        let (a, b) = (code_of(MAIN), code_of(OTHER));
        let columns = [("reserve", "identifier:reserve-number"), ("code", "code")];
        // written another way than PatientID holds it: one written form
        let r = import(
            &mut registry,
            &columns,
            &[&[SPARE_FORM, &a], &["19991325-5678", ALONE_CODE]],
            false,
        );
        assert!(r.written(), "{name}: {r}");
        assert_eq!(
            (r.rows, r.subjects.new, r.identifiers.new),
            (2, 2, 2),
            "{name}: {r}"
        );
        let place = declare(
            &mut registry,
            data.path(),
            "hold",
            "subject-code",
            "subject-code",
        );
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 7, 0, 1, 0), "{name}: {report}");
        assert_eq!(report.subjects.new, 1, "{name}: only the second number's");
        assert_eq!(
            tree(&anon),
            folders(&[
                (a.as_str(), 4, a.as_str()),
                (b.as_str(), 1, b.as_str()),
                (ALONE_CODE, 2, ALONE_CODE),
            ]),
            "{name}"
        );
        // no subject was made from a reserve number, nor from a study
        assert_eq!(
            subjects(&mut registry),
            BTreeSet::from([a.clone(), b.clone(), ALONE_CODE.to_string()]),
            "{name}"
        );
        // the number no map names: held under its own keyed lookup, by its
        // shape, nothing sealed
        let keys = Subkeys::derive(KEY);
        assert_eq!(
            held(&mut registry),
            [(
                "reserve-number".to_string(),
                "999999999999".to_string(),
                false,
                keys.lookup("reserve-number", LATER)
            )],
            "{name}"
        );
        assert_eq!(
            identities(&mut registry, "reserve-number"),
            (2, 0),
            "{name}"
        );
        assert_eq!(identities(&mut registry, "personnummer"), (2, 0), "{name}");

        // a map row names it: released, and the next run writes it
        let rehearsed = import(&mut registry, &columns, &[&[LATER, LATER_CODE]], true);
        assert_eq!(rehearsed.held_released, 1, "{name}: {rehearsed}");
        assert_eq!(
            identities(&mut registry, "reserve-number"),
            (2, 0),
            "{name}"
        );
        let r = import(&mut registry, &columns, &[&[LATER, LATER_CODE]], false);
        assert!(r.written(), "{name}: {r}");
        assert_eq!(r.held_released, 1, "{name}: {r}");
        assert_eq!(
            r.held_released_by,
            [identity_map::Released {
                id_type: "reserve-number".into(),
                held_as: "reserve-number".into(),
                files: 1
            }],
            "{name}"
        );
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 1, 7, 0, 0), "{name}: {report}");
        assert_eq!(report.subjects.new, 0, "{name}: {report}");
        assert_eq!(
            tree(&anon),
            folders(&[
                (a.as_str(), 4, a.as_str()),
                (b.as_str(), 1, b.as_str()),
                (ALONE_CODE, 2, ALONE_CODE),
                (LATER_CODE, 1, LATER_CODE),
            ]),
            "{name}"
        );
        assert!(held(&mut registry).is_empty(), "{name}");
        assert_eq!(
            identities(&mut registry, "reserve-number"),
            (3, 0),
            "{name}"
        );
        let codes_by_number: BTreeMap<&str, BTreeSet<String>> = {
            let rows = registry
                .store()
                .query(
                    "SELECT p.path, s.code FROM pseudonym_file p JOIN subject s ON s.id = p.subject_id \
                     WHERE p.state = 'written' ORDER BY p.path",
                    &[],
                )
                .unwrap();
            let mut out: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
            for r in &rows {
                let path = r.text(0).unwrap();
                let n: usize = path[path.len() - 4..].parse().unwrap();
                out.entry(FILES[n - 1].1)
                    .or_default()
                    .insert(r.text(1).unwrap().to_string());
            }
            out
        };
        assert_eq!(
            codes_by_number,
            BTreeMap::from([
                (MAIN, BTreeSet::from([a.clone()])),
                (SPARE, BTreeSet::from([a.clone()])),
                (OTHER, BTreeSet::from([b.clone()])),
                (ALONE, BTreeSet::from([ALONE_CODE.to_string()])),
                (LATER, BTreeSet::from([LATER_CODE.to_string()])),
            ]),
            "{name}"
        );
        drop(registry);
        if name == "sqlite" {
            nothing_kept_in(lab.home.dir(), &[MAIN, OTHER, SPARE, ALONE, LATER]);
        }
    }
}

/// No code is ever made from a reserve number: not where the dataset codes
/// an identifier no map names, not in a rehearsal's count, and not when a
/// person asks for its files to be coded anyway. A valid personnummer beside
/// it is coded by the generator as it always was.
#[test]
fn a_reserve_number_is_never_coded_whoever_asks() {
    for lab in labs() {
        let name = lab.name;
        let data = dataset();
        let anon = data.path().join("derivatives/dcm-anon");
        let mut registry = lab.home.open().unwrap();
        let place = declare(
            &mut registry,
            data.path(),
            "code",
            "subject-code",
            "subject-code",
        );
        let mut dry = settings(&place);
        dry.dry_run = true;
        let report = pseudonymize(&dry, &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 3, 0, 5, 0), "{name}: {report}");
        assert_eq!(report.subjects.new, 2, "{name}: the two numbers'");
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 3, 0, 5, 0), "{name}: {report}");
        assert_eq!(report.subjects.new, 2, "{name}: {report}");
        let (a, b) = (code_of(MAIN), code_of(OTHER));
        assert_eq!(
            subjects(&mut registry),
            BTreeSet::from([a.clone(), b.clone()]),
            "{name}"
        );
        let keys = Subkeys::derive(KEY);
        let lookups: BTreeSet<Vec<u8>> = held(&mut registry)
            .into_iter()
            .map(|(id_type, shape, sealed, lookup)| {
                assert_eq!(
                    (id_type.as_str(), shape.as_str(), sealed),
                    ("reserve-number", "999999999999", false),
                    "{name}"
                );
                lookup
            })
            .collect();
        assert_eq!(
            lookups,
            [SPARE, ALONE, LATER]
                .iter()
                .map(|n| keys.lookup("reserve-number", n))
                .collect(),
            "{name}: one lookup a number, however many files"
        );
        // however the number is written
        assert_eq!(
            keys.lookup("reserve-number", ALONE),
            keys.lookup("reserve-number", ALONE_FORM)
        );
        // a person asks for them to be coded anyway: the run still writes
        // none and makes no subject
        registry
            .store()
            .execute(
                "UPDATE pseudonym_file SET code_anyway = 1 WHERE state = 'held'",
                &[],
            )
            .unwrap();
        let mut anyway = settings(&place);
        anyway.held = true;
        let report = pseudonymize(&anyway, &mut registry).unwrap();
        assert_eq!(files_of(&report).1, 0, "{name}: {report}");
        assert_eq!(files_of(&report).3, 5, "{name}: {report}");
        assert_eq!(report.subjects.new, 0, "{name}: {report}");
        assert_eq!(subjects(&mut registry).len(), 2, "{name}");
        assert_eq!(
            tree(&anon),
            folders(&[(a.as_str(), 2, a.as_str()), (b.as_str(), 1, b.as_str())]),
            "{name}"
        );
        assert_eq!(
            identities(&mut registry, "reserve-number"),
            (0, 0),
            "{name}"
        );
    }
}

/// A site's maps for a dataset whose copies carry its own ID (PatientID and
/// each copy's folder the person's `site-id`): the site ID map already in,
/// and the reserve rows as three columns, the reserve number, the person's
/// code and their site ID, which the import checks against the subject the
/// code names. Every file of a person lands in their site ID's folder with it
/// in PatientID, a spare reserve number's beside the main number's, and the
/// run makes no subject.
#[test]
fn reserve_rows_beside_a_site_id_map_write_every_file_under_the_person_s_site_id() {
    for lab in labs() {
        let name = lab.name;
        let data = dataset();
        let anon = data.path().join("derivatives/dcm-anon");
        let mut registry = lab.home.open().unwrap();
        {
            let mut linkage = registry.open_linkage().unwrap();
            nils_registry::linkage::add_id_type(&mut linkage, "site-id", None).unwrap();
        }
        let (a, b) = (code_of(MAIN), code_of(OTHER));
        let site = import(
            &mut registry,
            &[("site", "identifier:site-id"), ("code", "code")],
            &[
                &["ABCD1234", &a],
                &["EFGH5678", &b],
                &["IJKL9012", ALONE_CODE],
                &["MNOP3456", LATER_CODE],
            ],
            false,
        );
        assert!(site.written(), "{name}: {site}");
        let columns = [
            ("reserve", "identifier:reserve-number"),
            ("code", "code"),
            ("site", "identifier:site-id"),
        ];
        // a row whose site ID is another person's is refused, whole
        let wrong = import(&mut registry, &columns, &[&[SPARE, &a, "EFGH5678"]], true);
        assert_eq!(wrong.conflicts.len(), 1, "{name}: {wrong}");
        let r = import(
            &mut registry,
            &columns,
            &[
                &[SPARE, &a, "ABCD1234"],
                &[ALONE, ALONE_CODE, "IJKL9012"],
                &[LATER, LATER_CODE, "MNOP3456"],
            ],
            false,
        );
        assert!(r.written(), "{name}: {r}");
        assert_eq!(
            (
                r.subjects.named,
                r.subjects.known,
                r.subjects.new,
                r.identifiers.known,
                r.identifiers.new
            ),
            (3, 3, 0, 3, 3),
            "{name}: {r}"
        );
        let place = declare(
            &mut registry,
            data.path(),
            "hold",
            "id-type:site-id",
            "id-type",
        );
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 8, 0, 0, 0), "{name}: {report}");
        assert_eq!(report.subjects.new, 0, "{name}: {report}");
        assert_eq!(
            tree(&anon),
            folders(&[
                ("ABCD1234", 4, "ABCD1234"),
                ("EFGH5678", 1, "EFGH5678"),
                ("IJKL9012", 2, "IJKL9012"),
                ("MNOP3456", 1, "MNOP3456"),
            ]),
            "{name}"
        );
        assert_eq!(subjects(&mut registry).len(), 4, "{name}");
        assert_eq!(
            identities(&mut registry, "reserve-number"),
            (3, 0),
            "{name}"
        );
    }
}

/// A file an engine from before reserve numbers held under its study's UID
/// stays releasable as it was: a map of that UID releases it and the run of
/// the held files writes it under the code the map gives; and a file of that
/// study the engine meets later is found by the study the map named.
#[test]
fn a_file_held_under_its_study_before_reserve_numbers_is_released_by_a_map_of_the_study() {
    for lab in labs() {
        let name = lab.name;
        let data = dataset();
        let anon = data.path().join("derivatives/dcm-anon");
        let mut registry = lab.home.open().unwrap();
        let place = declare(
            &mut registry,
            data.path(),
            "hold",
            "subject-code",
            "subject-code",
        );
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report), (8, 3, 0, 5, 0), "{name}: {report}");
        // the row of `d`'s file as an engine from before held it: under the
        // study's UID, by its shape, the UID sealed
        let keys = Subkeys::derive(KEY);
        let uid = study_uid("d", 1);
        {
            let store = registry.store();
            let d = store.dialect();
            let sql = format!(
                "UPDATE pseudonym_file SET id_type = 'study-instance-uid', lookup = {}, sealed = {}, shape = {} \
                 WHERE state = 'held' AND path = 'd/s1/IM_0008'",
                d.param(1, Type::Bytes),
                d.param(2, Type::Bytes),
                d.param(3, Type::Text)
            );
            let n = store
                .execute(
                    &sql,
                    &[
                        Param::Bytes(keys.lookup("study-instance-uid", &uid)),
                        Param::Bytes(keys.seal(&uid)),
                        Param::from(nils_dicom::diagnostic::shape(&uid)),
                    ],
                )
                .unwrap();
            assert_eq!(n, 1, "{name}");
        }
        let r = import(
            &mut registry,
            &[("study", "identifier:study-instance-uid"), ("code", "code")],
            &[&[&uid, LATER_CODE]],
            false,
        );
        assert!(r.written(), "{name}: {r}");
        assert_eq!(r.held_released, 1, "{name}: {r}");
        let mut again = settings(&place);
        again.held = true;
        let report = pseudonymize(&again, &mut registry).unwrap();
        assert_eq!(files_of(&report).1, 1, "{name}: {report}");
        // a file of that study met later is the same subject's
        data.file(
            "derivatives/dcm-original/d/s1/IM_0009",
            &original("d", LATER, 1, 2),
        );
        let report = pseudonymize(&settings(&place), &mut registry).unwrap();
        assert_eq!(files_of(&report).1, 1, "{name}: {report}");
        assert_eq!(report.subjects.new, 0, "{name}: {report}");
        assert_eq!(
            tree(&anon).get(LATER_CODE),
            Some(&(2, BTreeSet::from([LATER_CODE.to_string()]))),
            "{name}"
        );
        // the others still wait for their numbers' map
        assert_eq!(files_of(&report).3, 4, "{name}: {report}");
    }
}
