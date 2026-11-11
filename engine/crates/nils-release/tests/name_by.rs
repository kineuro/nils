// SPDX-License-Identifier: AGPL-3.0-only

//! A release that names its subjects by an ID type, from end to end.
//!
//! What is proved here is what a recipient gets and what the registry
//! keeps: the name where the code stood in every path and every file, the
//! code nowhere, the pairing kept for what comes back, and that a plan
//! which cannot name everybody stops before a file is written.

mod common;

use std::path::Path;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::digest;
use nils_registry::home::{Home, InitOptions};
use nils_registry::linkage::{self, NewIdentity, Subkeys};
use nils_registry::session::Scheme as SessionScheme;
use nils_registry::store::Param;
use nils_registry::{Backend, Registry, Scheme};
use nils_release::handover::archive::Archiver;
use nils_release::handover::plan::Strategy;
use nils_release::handover::run as handover;
use nils_release::name_by::Choice;
use nils_release::policy::Policy;
use nils_release::run::{self, Selection};
use nils_release::tags as categories;

const KEY: &[u8] = b"a naming test key of some length";

/// `n` people, one study each.
fn people(n: usize) -> TempDir {
    let dir = TempDir::new("names");
    for i in 1..=n {
        let study = format!("1.2.9.{i}");
        let sop = format!("{study}.1.1");
        let mut e = synth::minimal_mr(&study, &format!("{study}.1"), &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, &format!("P-00{i}")),
            synth::text(tags::STUDY_DATE, VR::DA, "20220115"),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
            synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
            synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("{i}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

fn registry(home_dir: &TempDir, source: &TempDir) -> (Home, Registry) {
    registry_on(home_dir, source, Backend::Sqlite, None)
}

/// The schema the Postgres half of these tests owns.
const SCHEMA: &str = "nils_release_names_test";

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
        scheme: Scheme::Blake2b32,
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

fn pack() -> &'static nils_pack::pack::Pack {
    static PACK: std::sync::OnceLock<nils_pack::pack::Pack> = std::sync::OnceLock::new();
    PACK.get_or_init(|| {
        let dir = common::shipped_mri();
        nils_pack::load(&dir, None).expect("the MRI pack loads")
    })
}

fn settings<'a>(
    out: &'a Path,
    policy: &'a Policy,
    scheme: &'a SessionScheme,
    name_by: &'a [String],
    name_choices: &'a [Choice],
) -> run::Settings<'a> {
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
        naming: nils_release::name::Naming::Full,
        places: nils_release::bids::place::Options::default(),
        converter: None,
        compress: true,
        observations: &[],
        authors: &[],
        workers: 0,
        name_by,
        name_choices,
    }
}

fn by(types: &[&str]) -> Vec<String> {
    types.iter().map(|t| t.to_string()).collect()
}

/// The subjects as `(id, code)`, in the order `people` made them.
fn subjects(reg: &mut Registry) -> Vec<(i64, String)> {
    let store = reg.store();
    let sql = format!(
        "SELECT su.id, su.code FROM {} su JOIN {} st ON st.subject_id = su.id \
         ORDER BY st.study_instance_uid",
        store.qualified("subject"),
        store.qualified("study"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| (r.int(0).unwrap(), r.text(1).unwrap().to_string()))
        .collect()
}

/// File a kept value of an ID type on a subject, making the type on first use.
fn file(reg: &mut Registry, subject: i64, id_type: &str, value: &str) {
    let keys = Subkeys::derive(&reg.pseudonym_key().unwrap());
    let mut link = reg.open_linkage().unwrap();
    let known = linkage::id_types(&mut link).unwrap();
    let t = match known.into_iter().find(|t| t.name == id_type) {
        Some(t) => t,
        None => linkage::add_id_type(&mut link, id_type, None).unwrap(),
    };
    linkage::insert_identities(
        &mut link,
        &[NewIdentity {
            subject_id: subject,
            id_type_id: t.id,
            lookup: keys.lookup(id_type, value),
            ciphertext: keys.seal(value),
            source: "csv",
            first_batch_id: None,
        }],
    )
    .unwrap();
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
            } else {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// The `sub-` folders at the top of a tree.
fn subject_dirs(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("sub-"))
        .collect();
    out.sort();
    out
}

/// Whether any path or any file under the root spells `word`.
fn spelled_under(root: &Path, word: &str) -> Vec<String> {
    let mut out = Vec::new();
    for f in files_under(root) {
        if f.contains(word) {
            out.push(format!("the path {f}"));
        }
        let bytes = std::fs::read(root.join(&f)).unwrap();
        if bytes.windows(word.len()).any(|w| w == word.as_bytes()) {
            out.push(format!("the bytes of {f}"));
        }
    }
    out
}

fn count(reg: &mut Registry, table: &str) -> i64 {
    let store = reg.store();
    let sql = format!("SELECT count(*) FROM {}", store.qualified(table));
    store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

/// What the registry kept of a version's names: `(subject, name, type, chosen)`.
fn kept(reg: &mut Registry, release: i64) -> Vec<(i64, String, Option<String>, bool)> {
    let store = reg.store();
    let sql = format!(
        "SELECT subject_id, name, id_type, chosen FROM {} WHERE release_id = {} ORDER BY subject_id",
        store.qualified("release_name"),
        store.dialect().param(1, nils_registry::schema::Type::Int),
    );
    store
        .query(&sql, &[Param::Int(release)])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.int(0).unwrap(),
                r.text(1).unwrap().to_string(),
                r.opt_text(2).unwrap().map(str::to_string),
                r.int(3).unwrap() != 0,
            )
        })
        .collect()
}

/// An expectation of [`kept`], in its order: by subject, whatever order the
/// digest met them in.
fn by_subject(
    mut rows: Vec<(i64, String, Option<String>, bool)>,
) -> Vec<(i64, String, Option<String>, bool)> {
    rows.sort();
    rows
}

fn refusal(result: Result<run::Report, run::Error>) -> String {
    match result {
        Err(run::Error::Refused(words)) => words,
        Err(other) => panic!("refused in other words: {other}"),
        Ok(report) => panic!("the release ran: version {}", report.version),
    }
}

#[test]
fn the_first_type_that_holds_a_value_names_and_the_code_is_last() {
    let source = people(3);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    assert_eq!(who.len(), 3);
    // The first holds both types, the second only the later one, the third
    // neither.
    file(&mut reg, who[0].0, "study-id", "ST_001");
    file(&mut reg, who[0].0, "clinic-id", "C-1");
    file(&mut reg, who[1].0, "clinic-id", "C-2");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id", "clinic-id"]);
    let report = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(report.subjects, 3);
    assert_eq!(report.files, 3);

    // The folders: the cleaned value of the first type that holds one, and
    // the code only for the subject no type names.
    let mut expected = vec![
        "sub-ST001".to_string(),
        "sub-C2".to_string(),
        format!("sub-{}", who[2].1),
    ];
    expected.sort();
    assert_eq!(subject_dirs(out.path()), expected);

    // The counts say the same, and say no value.
    let names = report.names.clone().expect("the report says how it named");
    assert_eq!(
        names,
        serde_json::json!({
            "by": ["study-id", "clinic-id"],
            "subjects": 3,
            "types": [
                {"type": "study-id", "subjects": 1},
                {"type": "clinic-id", "subjects": 1},
            ],
            "code": 1,
            "chosen": 0,
            "passed_over": 0,
        })
    );

    // The code of a subject an ID named is nowhere in what leaves: not in a
    // path, not in a file. The name is in the file, where the code stood.
    for (_, code) in &who[..2] {
        let found = spelled_under(out.path(), code);
        assert!(found.is_empty(), "the code {code} is in {found:?}");
    }
    let first = spelled_under(out.path(), "ST001");
    assert!(
        first.iter().any(|f| f.starts_with("the bytes of")),
        "the name is the released file's PatientID: {first:?}"
    );
    // A subject that kept its code is written as it always was.
    let third = spelled_under(out.path(), &who[2].1);
    assert!(
        third.iter().any(|f| f.starts_with("the bytes of")),
        "{third:?}"
    );

    // The registry keeps the pairing, so what comes back under a name is
    // linked to its subject.
    assert_eq!(
        kept(&mut reg, report.release_id),
        by_subject(vec![
            (
                who[0].0,
                "ST001".to_string(),
                Some("study-id".to_string()),
                false
            ),
            (
                who[1].0,
                "C2".to_string(),
                Some("clinic-id".to_string()),
                false
            ),
            (who[2].0, who[2].1.clone(), None, false),
        ])
    );

    // The release row says what it named by, and the audit trail has a row
    // with the types and the counts and nothing else.
    let store = reg.store();
    let row = store
        .query(
            &format!("SELECT name_by FROM {}", store.qualified("release")),
            &[],
        )
        .unwrap();
    let stored: serde_json::Value = serde_json::from_str(row[0].text(0).unwrap()).unwrap();
    assert_eq!(stored, serde_json::json!(["study-id", "clinic-id"]));
    let audit = store
        .query(
            &format!(
                "SELECT scope, details FROM {} WHERE action = 'release.names'",
                store.qualified("audit")
            ),
            &[],
        )
        .unwrap();
    assert_eq!(audit.len(), 1, "one row for the release");
    let scope = audit[0].text(0).unwrap().to_string();
    let details = audit[0].text(1).unwrap().to_string();
    assert!(scope.contains(&report.version), "{scope}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&details).unwrap(),
        names
    );
    for value in ["ST_001", "ST001", "C-1", "C-2", "C2"] {
        assert!(!details.contains(value), "{details}");
        assert!(!scope.contains(value), "{scope}");
    }

    // The same release again changes nothing: the names are part of what a
    // stack's bytes are decided by, and they did not change.
    let again = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(again.unchanged, 3);
    assert_eq!(again.written, 0);
}

#[test]
fn several_values_stop_the_plan_and_the_choice_is_kept_for_the_next_version() {
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "ST_001");
    file(&mut reg, who[0].0, "study-id", "ST_009");
    // The same value in another spelling is one value, not a second.
    file(&mut reg, who[1].0, "study-id", "ST_002");
    file(&mut reg, who[1].0, "study-id", "ST-002");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id"]);

    // The rehearsal lists the subject with its values, and writes nothing.
    let plan = run::name_plan(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap()
    .expect("a plan, since the release names by a type");
    assert!(!plan.ready());
    assert_eq!(plan.needs_choice.len(), 1);
    assert_eq!(plan.needs_choice[0].subject, who[0].1);
    assert_eq!(plan.needs_choice[0].values, ["ST_001", "ST_009"]);
    let shown = plan.doc(true);
    assert_eq!(shown["ready"], false);
    assert_eq!(shown["values"], "shown");
    assert_eq!(
        shown["needs_choice"],
        serde_json::json!([{
            "subject": who[0].1,
            "type": "study-id",
            "count": 2,
            "values": ["ST_001", "ST_009"],
        }])
    );
    // A caller who may not read identifiers gets the count and the code.
    let withheld = plan.doc(false);
    assert_eq!(withheld["values"], "withheld");
    assert_eq!(
        withheld["needs_choice"],
        serde_json::json!([{"subject": who[0].1, "type": "study-id", "count": 2}])
    );
    assert!(!withheld.to_string().contains("ST_00"), "{withheld}");

    // The release stops there, before a row or a file, and its words name
    // the subject by its code and spell no value.
    let words = refusal(run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    ));
    assert!(words.contains(&who[0].1), "{words}");
    assert!(words.contains("Nothing was written."), "{words}");
    assert!(
        !words.contains("ST_00") && !words.contains("ST00"),
        "{words}"
    );
    assert_eq!(count(&mut reg, "release"), 0);
    assert_eq!(count(&mut reg, "release_name"), 0);
    assert!(files_under(out.path()).is_empty());

    // A choice that is none of the values is refused, by the subject.
    let wrong = [Choice {
        subject: who[0].1.clone(),
        value: "ST_005".to_string(),
    }];
    let words = refusal(run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &wrong),
    ));
    assert!(words.contains(&who[0].1), "{words}");
    assert!(!words.contains("ST_005"), "{words}");
    assert_eq!(count(&mut reg, "release"), 0);

    // With the choice the release runs, and says one name was chosen.
    let choice = [Choice {
        subject: who[0].1.clone(),
        value: "ST_009".to_string(),
    }];
    let first = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &choice),
    )
    .unwrap();
    assert_eq!(subject_dirs(out.path()), ["sub-ST002", "sub-ST009"]);
    assert_eq!(first.names.as_ref().unwrap()["chosen"], 1);
    assert!(kept(&mut reg, first.release_id).contains(&(
        who[0].0,
        "ST009".to_string(),
        Some("study-id".to_string()),
        true
    )));

    // The next version is not asked again: the choice was kept with the
    // release, and nothing is written for it.
    let second = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(second.previous.as_deref(), Some(first.version.as_str()));
    assert_eq!(second.unchanged, 2);
    assert_eq!(second.written, 0);
    assert_eq!(second.names.as_ref().unwrap()["chosen"], 1);
    assert_eq!(subject_dirs(out.path()), ["sub-ST002", "sub-ST009"]);
    assert!(
        kept(&mut reg, second.release_id)
            .iter()
            .any(|k| k.0 == who[0].0 && k.1 == "ST009" && k.3),
        "still a choice"
    );

    // Another choice names the subject again: its files are written under
    // the new name and the old folder leaves the tree.
    let other = [Choice {
        subject: who[0].1.clone(),
        value: "ST001".to_string(),
    }];
    let third = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &other),
    )
    .unwrap();
    assert_eq!(third.unchanged, 1);
    assert_eq!(subject_dirs(out.path()), ["sub-ST001", "sub-ST002"]);
    let left = spelled_under(out.path(), "ST009");
    assert!(left.is_empty(), "{left:?}");

    // A release of another name has no choice of its own to read.
    let elsewhere = TempDir::new("names-elsewhere");
    let mut other_release = settings(elsewhere.path(), &policy, &scheme, &types, &[]);
    other_release.name = "another";
    let words = refusal(run::run(&mut reg, &other_release));
    assert!(words.contains(&who[0].1), "{words}");
}

#[test]
fn a_value_is_cleaned_and_a_shared_name_is_refused_with_the_list() {
    let source = people(4);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    // Two subjects whose values differ only in what cleaning takes away and
    // in case, a third whose value is nothing once cleaned, and a fourth
    // whose value is the third's code.
    file(&mut reg, who[0].0, "study-id", "AB_01");
    file(&mut reg, who[1].0, "study-id", "ab-01");
    file(&mut reg, who[2].0, "study-id", "_-_");
    let third_code = who[2].1.clone();
    file(&mut reg, who[3].0, "study-id", &third_code);

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id"]);
    let plan = run::name_plan(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap()
    .unwrap();
    assert!(!plan.ready());
    assert!(plan.needs_choice.is_empty());
    assert_eq!(plan.code, 1, "an empty name is no name: the code");
    assert_eq!(plan.collisions.len(), 2, "{:?}", plan.collisions);
    let doc = plan.doc(false);
    let listed = doc["collisions"].to_string();
    for (_, code) in &who {
        assert!(listed.contains(code.as_str()), "{listed}");
    }
    // Where values are withheld a collision of names from IDs spells no
    // name; with them shown it does.
    assert!(
        !listed.contains("AB01") && !listed.contains("ab01"),
        "{listed}"
    );
    assert!(
        plan.doc(true)["collisions"]
            .to_string()
            .to_ascii_lowercase()
            .contains("ab01")
    );

    let words = refusal(run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    ));
    for (_, code) in &who {
        assert!(words.contains(code.as_str()), "{words}");
    }
    assert!(words.contains("2 names would be shared"), "{words}");
    assert!(words.contains("Nothing was written."), "{words}");
    assert!(
        !words.contains("AB01") && !words.contains("AB_01"),
        "{words}"
    );
    assert_eq!(count(&mut reg, "release"), 0);
    assert!(files_under(out.path()).is_empty());
}

#[test]
fn a_cleaned_value_names_and_an_empty_one_falls_to_the_next_type() {
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "MM-NO1_001");
    file(&mut reg, who[1].0, "study-id", "--");
    file(&mut reg, who[1].0, "clinic-id", "c 7");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id", "clinic-id"]);
    let report = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(subject_dirs(out.path()), ["sub-MMNO1001", "sub-c7"]);
    assert_eq!(report.names.as_ref().unwrap()["code"], 0);
}

#[test]
fn a_type_that_keeps_no_value_is_refused_by_name() {
    let source = people(1);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "ST_001");
    {
        // A type nobody holds a value of.
        let mut link = reg.open_linkage().unwrap();
        linkage::add_id_type(&mut link, "empty-id", None).unwrap();
    }

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let cases: [(&[&str], &[&str]); 5] = [
        (
            &["personnummer"],
            &["name by personnummer", "a personnummer is never kept"],
        ),
        // Whatever else the list holds, and wherever it stands in it.
        (
            &["study-id", "personnummer"],
            &["name by personnummer", "a personnummer is never kept"],
        ),
        (
            &["empty-id"],
            &["name by empty-id", "no subject holds a kept value"],
        ),
        (
            &["no-such-id"],
            &["name by no-such-id", "no ID type is named so", "study-id"],
        ),
        (&["study-id", "study-id"], &["in the list twice"]),
    ];
    for (types, said) in cases {
        let types = by(types);
        let words = refusal(run::run(
            &mut reg,
            &settings(out.path(), &policy, &scheme, &types, &[]),
        ));
        for part in said {
            assert!(words.contains(part), "{types:?}: {words}");
        }
        assert!(words.ends_with("Nothing was written."), "{words}");
        // The rehearsal refuses in the same words.
        match run::name_plan(
            &mut reg,
            &settings(out.path(), &policy, &scheme, &types, &[]),
        ) {
            Err(run::Error::Refused(plan_words)) => assert_eq!(plan_words, words),
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("a plan for {types:?}"),
        }
    }
    // A choice without a type to name by is refused too.
    let choice = [Choice {
        subject: who[0].1.clone(),
        value: "ST_001".to_string(),
    }];
    let words = refusal(run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &[], &choice),
    ));
    assert!(words.contains("names by no ID type"), "{words}");

    assert_eq!(count(&mut reg, "release"), 0);
    assert!(files_under(out.path()).is_empty());
}

#[test]
fn a_value_shaped_like_a_personnummer_never_names() {
    // The store keeps no such value under any type, and a release does not
    // rest on that: a value of that shape met here is passed over, counted,
    // and the subject goes on to the next type and then to its code.
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    // A made-up number with a valid check digit, as the engine's own tests
    // of the shape use.
    file(&mut reg, who[0].0, "study-id", "19850101-2382");
    file(&mut reg, who[1].0, "study-id", "ST_002");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id"]);
    let report = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    let mut expected = vec![format!("sub-{}", who[0].1), "sub-ST002".to_string()];
    expected.sort();
    assert_eq!(subject_dirs(out.path()), expected);
    let names = report.names.unwrap();
    assert_eq!(names["passed_over"], 1);
    assert_eq!(names["code"], 1);
    let found = spelled_under(out.path(), "198501012382");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_release_that_names_by_nothing_is_the_release_it_was() {
    // The fixture and the settings of every release before a release could
    // name by anything: nothing about it may change. It keeps no names, its
    // row says none, its report says none, and no audit row is added.
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    // An ID the release is not asked to name by changes nothing either.
    file(&mut reg, who[0].0, "study-id", "ST_001");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let plain = run::run(&mut reg, &settings(out.path(), &policy, &scheme, &[], &[])).unwrap();
    assert!(plain.names.is_none());
    let rendered = serde_json::to_value(&plain).unwrap();
    assert!(rendered.get("names").is_none(), "{rendered}");
    let mut expected: Vec<String> = who.iter().map(|(_, c)| format!("sub-{c}")).collect();
    expected.sort();
    assert_eq!(subject_dirs(out.path()), expected);
    assert_eq!(count(&mut reg, "release_name"), 0);
    let store = reg.store();
    let row = store
        .query(
            &format!("SELECT name_by FROM {}", store.qualified("release")),
            &[],
        )
        .unwrap();
    assert_eq!(row[0].opt_text(0).unwrap(), None);
    let audit = store
        .query(
            &format!(
                "SELECT id FROM {} WHERE action = 'release.names'",
                store.qualified("audit")
            ),
            &[],
        )
        .unwrap();
    assert!(audit.is_empty());
    assert!(
        run::name_plan(&mut reg, &settings(out.path(), &policy, &scheme, &[], &[]))
            .unwrap()
            .is_none()
    );

    // A subject that falls back to its code under a list is that same
    // release, byte for byte and digest for digest: the second subject
    // here holds no study-id, so its tree and its content identity under
    // the list are what they are without one.
    let named_out = TempDir::new("names-named");
    let types = by(&["study-id"]);
    let mut named_settings = settings(named_out.path(), &policy, &scheme, &types, &[]);
    named_settings.name = "named";
    let named = run::run(&mut reg, &named_settings).unwrap();
    let folder = format!("sub-{}", who[1].1);
    let under = |root: &Path| -> Vec<(String, Vec<u8>)> {
        files_under(&root.join(&folder))
            .into_iter()
            .map(|f| {
                let bytes = std::fs::read(root.join(&folder).join(&f)).unwrap();
                (f, bytes)
            })
            .collect()
    };
    let before = under(out.path());
    assert!(!before.is_empty());
    assert_eq!(
        before,
        under(named_out.path()),
        "the same tree, the same bytes"
    );
    // What the registry keeps of each stack a version wrote: the digest of
    // everything that decides its bytes, and where it went.
    let contents = |reg: &mut Registry, release: i64| -> Vec<(String, String)> {
        let store = reg.store();
        let sql = format!(
            "SELECT content, dir FROM {} WHERE release_id = {} ORDER BY stack_id",
            store.qualified("release_stack"),
            store.dialect().param(1, nils_registry::schema::Type::Int),
        );
        store
            .query(&sql, &[Param::Int(release)])
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r.text(0).unwrap().to_string(),
                    r.text(1).unwrap().to_string(),
                )
            })
            .collect()
    };
    let was = contents(&mut reg, plain.release_id);
    let now = contents(&mut reg, named.release_id);
    assert_eq!(was.len(), 2);
    assert_eq!(now.len(), 2);
    let of = |rows: &[(String, String)], folder: &str| -> (String, String) {
        rows.iter()
            .find(|(_, dir)| dir.starts_with(folder))
            .cloned()
            .unwrap_or_else(|| panic!("no stack under {folder} in {rows:?}"))
    };
    assert_eq!(
        of(&was, &folder),
        of(&now, &folder),
        "the subject that kept its code"
    );
    assert_ne!(
        of(&was, &format!("sub-{}", who[0].1)).0,
        of(&now, "sub-ST001").0,
        "the subject an ID named is other bytes"
    );

    // And the release that names by nothing, run again after all that,
    // finds nothing to do.
    let again = run::run(&mut reg, &settings(out.path(), &policy, &scheme, &[], &[])).unwrap();
    assert_eq!(again.unchanged, 2);
    assert_eq!(again.written, 0);
}

#[test]
fn naming_by_a_type_after_all_writes_the_subject_under_its_name() {
    // A release made by code and then asked to name by a type is another
    // tree for the subjects the type names: the folder of the code leaves
    // and nothing of it stays behind.
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "ST_001");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    run::run(&mut reg, &settings(out.path(), &policy, &scheme, &[], &[])).unwrap();
    let types = by(&["study-id"]);
    let second = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(second.unchanged, 1, "the subject that keeps its code");
    let mut expected = vec!["sub-ST001".to_string(), format!("sub-{}", who[1].1)];
    expected.sort();
    assert_eq!(subject_dirs(out.path()), expected);
    let found = spelled_under(out.path(), &who[0].1);
    assert!(found.is_empty(), "{found:?}");

    // And back: the name leaves with the list.
    let third = run::run(&mut reg, &settings(out.path(), &policy, &scheme, &[], &[])).unwrap();
    assert_eq!(third.unchanged, 1);
    let found = spelled_under(out.path(), "ST001");
    assert!(found.is_empty(), "{found:?}");
}

/// A stand-in for 7-Zip that writes the list of what it was asked to pack
/// as the archive, so a test can read what a recipient would unpack without
/// the archiver being installed.
#[cfg(unix)]
fn listing_archiver(dir: &Path) -> Archiver {
    use std::os::unix::fs::PermissionsExt as _;
    let fake = dir.join("7z");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ $# -eq 0 ]; then echo '7-Zip stand-in'; exit 0; fi\n\
         [ \"$1\" = a ] || exit 0\n\
         shift\n\
         for arg in \"$@\"; do\n\
         case \"$arg\" in\n\
         -i@*) list=${arg#-i@};;\n\
         -*) ;;\n\
         *) archive=$arg;;\n\
         esac\n\
         done\n\
         cat \"$list\" > \"$archive\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    Archiver::find(&fake).expect("the stand-in answers")
}

#[cfg(unix)]
#[test]
fn a_handover_of_a_named_release_carries_the_name_and_never_the_code() {
    let source = people(2);
    let home_dir = TempDir::new("names-home");
    let out = TempDir::new("names-out");
    let ship = TempDir::new("names-ship");
    let tools = TempDir::new("names-tools");
    let (_home, mut reg) = registry(&home_dir, &source);
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "ST_001");
    file(&mut reg, who[1].0, "study-id", "ST_002");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id"]);
    run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();

    let archiver = listing_archiver(tools.path());
    let report = handover::run(
        &mut reg,
        &handover::Settings {
            release: "test",
            out: ship.path(),
            archiver: &archiver,
            protection: handover::Protection::Made {
                key_name: "k",
                key: KEY,
            },
            cap: 1 << 30,
            strategy: Strategy::Ordered,
            level: 1,
            par2: None,
            verify: true,
            actor: "a test",
        },
    )
    .unwrap();
    assert_eq!(report.archives, 1);
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    // What is shipped: the archives by their names, what they hold, and the
    // manifest. None of it spells a subject code; the archive is named by
    // the names.
    let shipped = files_under(ship.path());
    assert!(
        shipped.iter().any(|f| f.contains("sub-ST001-to-ST002")),
        "{shipped:?}"
    );
    for (_, code) in &who {
        let found = spelled_under(ship.path(), code);
        assert!(found.is_empty(), "the code {code} is in {found:?}");
    }

    // The registry's own record of the set links each name to its subject.
    let store = reg.store();
    let rows = store
        .query(
            &format!(
                "SELECT code, subject_id FROM {} ORDER BY code",
                store.qualified("handover_subject")
            ),
            &[],
        )
        .unwrap();
    let linked: Vec<(String, Option<i64>)> = rows
        .iter()
        .map(|r| (r.text(0).unwrap().to_string(), r.opt_int(1).unwrap()))
        .collect();
    assert_eq!(
        linked,
        vec![
            ("ST001".to_string(), Some(who[0].0)),
            ("ST002".to_string(), Some(who[1].0)),
        ]
    );
}

#[test]
fn a_named_release_runs_on_postgres_too() {
    let Some(dsn) = postgres_dsn() else { return };
    drop_schemas(&dsn);
    let source = people(2);
    let home_dir = TempDir::new("names-home-pg");
    let out = TempDir::new("names-out-pg");
    let (_home, mut reg) = registry_on(&home_dir, &source, Backend::Postgres, Some(dsn.clone()));
    let who = subjects(&mut reg);
    file(&mut reg, who[0].0, "study-id", "ST_001");
    file(&mut reg, who[0].0, "study-id", "ST_009");

    let policy = Policy::default();
    let scheme = SessionScheme::default();
    let types = by(&["study-id"]);
    let words = refusal(run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    ));
    assert!(words.contains(&who[0].1), "{words}");
    let choice = [Choice {
        subject: who[0].1.clone(),
        value: "ST_009".to_string(),
    }];
    let first = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &choice),
    )
    .unwrap();
    let mut expected = vec!["sub-ST009".to_string(), format!("sub-{}", who[1].1)];
    expected.sort();
    assert_eq!(subject_dirs(out.path()), expected);
    assert_eq!(
        kept(&mut reg, first.release_id),
        by_subject(vec![
            (
                who[0].0,
                "ST009".to_string(),
                Some("study-id".to_string()),
                true
            ),
            (who[1].0, who[1].1.clone(), None, false),
        ])
    );
    let second = run::run(
        &mut reg,
        &settings(out.path(), &policy, &scheme, &types, &[]),
    )
    .unwrap();
    assert_eq!(second.unchanged, 2);
    assert_eq!(second.written, 0);
    drop(reg);
    drop_schemas(&dsn);
}
