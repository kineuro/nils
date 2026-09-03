// SPDX-License-Identifier: AGPL-3.0-only

//! Identity (§7): one identifier is one subject across studies, batches and
//! runs; the linkage store holds the identifier sealed and gives it back to
//! `reveal` with an audit row; a file without the field falls back to its
//! study; a rule reads another field through a pattern; a subject an import
//! created keeps its code; a blake2b-8 registry reproduces the v0 codes; two
//! identifiers on one code stop the job with a review item. Every test runs
//! on each backend.

mod common;

use std::collections::BTreeMap;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{DigestError, Rule, digest};
use nils_registry::linkage::{self, ImportRow, Subkeys};
use nils_registry::{Registry, Row, Scheme, Store, pseudonym};

use common::*;

/// A query on the linkage store with `{table}` placeholders.
fn lrows(store: &mut Store, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "identity",
        "id_type",
        "read_audit",
        "linkage",
        "linkage_meta",
    ] {
        text = text.replace(&format!("{{{t}}}"), &store.qualified(t));
    }
    store.query(&text, &[]).unwrap()
}

fn lone(store: &mut Store, sql: &str) -> i64 {
    lrows(store, sql)[0].int(0).unwrap()
}

fn ltexts(store: &mut Store, sql: &str) -> Vec<String> {
    lrows(store, sql)
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect()
}

/// The identifiers of a subject, decrypted, as `(type, value, source)`.
fn revealed(reg: &mut Registry, store: &mut Store, subject: i64) -> Vec<(String, String, String)> {
    let keys = Subkeys::derive(&reg.pseudonym_key().unwrap());
    linkage::reveal(store, &keys, subject, "test", None)
        .unwrap()
        .into_iter()
        .map(|r| (r.id_type, r.value, r.source))
        .collect()
}

fn identity(id_type: &str, value: &str, source: &str) -> (String, String, String) {
    (id_type.to_string(), value.to_string(), source.to_string())
}

fn subject_of_study(reg: &mut Registry, study: &str) -> i64 {
    one(
        reg,
        &format!("SELECT subject_id FROM {{study}} WHERE study_instance_uid = '{study}'"),
    )
}

fn code_of(reg: &mut Registry, subject: i64) -> String {
    texts(
        reg,
        &format!("SELECT code FROM {{subject}} WHERE id = {subject}"),
    )
    .remove(0)
}

/// The error a failed digest ends with, in words.
fn failure(name: &str, result: Result<nils_digest::Report, DigestError>) -> String {
    match result {
        Ok(_) => panic!("{name}: the digest did not fail"),
        Err(DigestError::Registry(e)) => e.to_string(),
        Err(e) => panic!("{name}: {e}"),
    }
}

#[test]
fn one_identifier_is_one_subject_across_studies_and_runs() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.subjects_created, 2, "{name}");
        assert_eq!(w.subjects_matched, 0, "{name}");
        assert_eq!(w.identities_attached, 0, "{name}");
        assert!(report.diagnostics.is_empty(), "{name}");
        // one identity per subject, from the digest, in the first batch
        assert_eq!(
            lone(&mut store, "SELECT COUNT(*) FROM {identity}"),
            2,
            "{name}"
        );
        assert_eq!(
            ltexts(&mut store, "SELECT DISTINCT source FROM {identity}"),
            ["dicom"],
            "{name}"
        );
        assert_eq!(
            lone(
                &mut store,
                "SELECT COUNT(*) FROM {identity} WHERE first_batch_id = 1"
            ),
            2,
            "{name}"
        );
        assert_eq!(
            ltexts(
                &mut store,
                "SELECT DISTINCT t.name FROM {identity} i JOIN {id_type} t ON t.id = i.id_type_id"
            ),
            ["patient-id"],
            "{name}"
        );
        // the registry holds the code and its digest, never the identifier
        let sub_a = subject_of_study(&mut reg, "A");
        let sub_b = subject_of_study(&mut reg, "B");
        assert_ne!(sub_a, sub_b, "{name}");
        let codes = texts(&mut reg, "SELECT code FROM {subject} ORDER BY id");
        assert_eq!(codes.len(), 2, "{name}");
        assert!(codes.iter().all(|c| c != "P1" && c != "P2"), "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {subject} WHERE code_digest IS NULL"
            ),
            0,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {subject} WHERE first_batch_id = 1"
            ),
            2,
            "{name}"
        );
        // the code is the scheme's code of the identifier under the key
        assert_eq!(
            code_of(&mut reg, sub_a),
            pseudonym::code(Scheme::DEFAULT, KEY, "P1", 12).code,
            "{name}"
        );
        // the identifier comes back from the linkage store, and the read is audited
        assert_eq!(
            revealed(&mut reg, &mut store, sub_a),
            [identity("patient-id", "P1", "dicom")],
            "{name}"
        );
        assert_eq!(
            revealed(&mut reg, &mut store, sub_b),
            [identity("patient-id", "P2", "dicom")],
            "{name}"
        );
        assert_eq!(
            lone(&mut store, "SELECT COUNT(*) FROM {read_audit}"),
            2,
            "{name}"
        );

        // a third study of P1 in a later run lands on the same subject
        let p1 = [birth("19800101"), sex("M"), description("Spine")];
        dir.file("sub1/IM_0004", &mr("C", "C.1", "C.1.1", "P1", &p1));
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = again.written.clone().unwrap();
        assert_eq!(w.subjects_created, 0, "{name}");
        assert_eq!(w.subjects_matched, 1, "{name}");
        assert_eq!(w.identities_attached, 0, "{name}");
        assert_eq!(w.studies_created, 1, "{name}");
        assert_eq!(subject_of_study(&mut reg, "C"), sub_a, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 2, "{name}");
        assert_eq!(
            lone(&mut store, "SELECT COUNT(*) FROM {identity}"),
            2,
            "{name}"
        );
        // the linkage store is this registry's
        assert_eq!(
            ltexts(
                &mut store,
                "SELECT value FROM {linkage_meta} WHERE key = 'registry_id'"
            ),
            [reg.meta().registry_id.clone()],
            "{name}"
        );
    }
}

#[test]
fn a_file_without_the_field_is_filed_under_its_study() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("identity-fallback");
        // no PatientID at all, and one with only spaces
        let bare = synth::minimal_mr("F", "F.1", "F.1.1");
        dir.file(
            "a/IM_0001",
            &synth::part10(&MetaFields::mr("F.1.1"), &bare, true),
        );
        let mut blank = synth::minimal_mr("G", "G.1", "G.1.1");
        blank.push(synth::text(tags::PATIENT_ID, VR::LO, "  "));
        dir.file(
            "b/IM_0001",
            &synth::part10(&MetaFields::mr("G.1.1"), &blank, true),
        );
        dir.file("c/IM_0001", &mr("H", "H.1", "H.1.1", "P9", &[]));
        let s = settings(&dir);
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.subjects_created, 3, "{name}");
        let fallback: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.kind == "identity_fallback")
            .collect();
        assert_eq!(fallback.len(), 1, "{name}: {:?}", report.diagnostics);
        assert_eq!(fallback[0].count, 2, "{name}");
        assert!(
            fallback[0].samples.iter().all(|s| s.contains("PatientID")),
            "{name}: {:?}",
            fallback[0].samples
        );
        // the fallback subjects carry the study's identity, the other its own
        let sub_f = subject_of_study(&mut reg, "F");
        let sub_g = subject_of_study(&mut reg, "G");
        let sub_h = subject_of_study(&mut reg, "H");
        assert_eq!(
            revealed(&mut reg, &mut store, sub_f),
            [identity("study-instance-uid", "F", "dicom")],
            "{name}"
        );
        assert_eq!(
            revealed(&mut reg, &mut store, sub_g),
            [identity("study-instance-uid", "G", "dicom")],
            "{name}"
        );
        assert_eq!(
            revealed(&mut reg, &mut store, sub_h),
            [identity("patient-id", "P9", "dicom")],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT CAST(SUM(count) AS BIGINT) FROM {diagnostic} WHERE kind = 'identity_fallback'"
            ),
            2,
            "{name}"
        );
        // the same files again read nothing and create nothing
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(again.written.unwrap().subjects_created, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 3, "{name}");
    }
}

#[test]
fn a_rule_reads_another_field_through_its_pattern() {
    let rule = Rule::parse(
        "identity:\n  id_type: patient-id\n  from:\n    - field: PatientComments\n      pattern: 'id=(?<id>[0-9]+)'\n    - field: PatientID\n",
    )
    .unwrap();
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("identity-rule");
        let comment = |s: &str| synth::text(tags::PATIENT_COMMENTS, VR::LT, s);
        // the comment names the subject; the PatientID is another one
        dir.file(
            "a/IM_0001",
            &mr("A", "A.1", "A.1.1", "P1", &[comment("id=42 (moved)")]),
        );
        // no id in the comment: the PatientID is read, and it is the same subject
        dir.file(
            "b/IM_0001",
            &mr("B", "B.1", "B.1.1", "42", &[comment("nothing here")]),
        );
        // no comment at all
        dir.file("c/IM_0001", &mr("C", "C.1", "C.1.1", "P3", &[]));
        let mut s = settings(&dir);
        s.identity = rule.clone();
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.subjects_created, 2, "{name}");
        let sub_a = subject_of_study(&mut reg, "A");
        assert_eq!(subject_of_study(&mut reg, "B"), sub_a, "{name}");
        assert_eq!(
            revealed(&mut reg, &mut store, sub_a),
            [identity("patient-id", "42", "dicom")],
            "{name}"
        );
        // the comment that did not match is a diagnostic with the shape, not the text
        let unparsed: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.kind == "identity_unparsed")
            .collect();
        assert_eq!(unparsed.len(), 1, "{name}: {:?}", report.diagnostics);
        assert_eq!(unparsed[0].count, 1, "{name}");
        assert!(
            unparsed[0].samples[0].starts_with("PatientComments="),
            "{name}: {:?}",
            unparsed[0].samples
        );
        assert!(
            !unparsed[0].samples[0].contains("nothing"),
            "{name}: {:?}",
            unparsed[0].samples
        );
        // the job records the rule it ran under
        let config = texts(&mut reg, "SELECT CAST(args AS TEXT) FROM {job} ORDER BY id");
        let config: serde_json::Value = serde_json::from_str(&config[0]).unwrap();
        assert_eq!(
            config["identity"]["from"][0]["field"], "PatientComments",
            "{name}"
        );
        assert_eq!(
            config["identity"]["from"][0]["pattern"], "id=(?<id>[0-9]+)",
            "{name}"
        );
        assert_eq!(
            config["identity"]["from"][1]["field"], "PatientID",
            "{name}"
        );
    }
}

#[test]
fn a_subject_an_import_created_keeps_its_code() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();
        let keys = Subkeys::derive(&reg.pseudonym_key().unwrap());

        // the codes of a legacy registry, filed as they were
        let rows = [
            ImportRow {
                line: 2,
                identifier: "P1".to_string(),
                code: "legacy-0001".to_string(),
            },
            ImportRow {
                line: 3,
                identifier: "P7".to_string(),
                code: "legacy-0007".to_string(),
            },
        ];
        let imported =
            linkage::import(reg.store(), &mut store, &keys, "patient-id", &rows).unwrap();
        assert_eq!(
            (
                imported.rows,
                imported.subjects_created,
                imported.identities_added,
                imported.unchanged
            ),
            (2, 2, 2, 0),
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT code FROM {subject} ORDER BY code"),
            ["legacy-0001", "legacy-0007"],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {subject} WHERE code_digest IS NULL"
            ),
            2,
            "{name}"
        );

        // the digest finds P1 by its identity and creates only P2
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.subjects_created, 1, "{name}");
        assert_eq!(w.subjects_matched, 1, "{name}");
        assert_eq!(w.identities_attached, 0, "{name}");
        let sub_a = subject_of_study(&mut reg, "A");
        assert_eq!(code_of(&mut reg, sub_a), "legacy-0001", "{name}");
        // the digest filled the fields the import did not have
        assert_eq!(
            texts(
                &mut reg,
                &format!("SELECT sex FROM {{subject}} WHERE id = {sub_a}")
            ),
            ["M"],
            "{name}"
        );
        assert_eq!(
            revealed(&mut reg, &mut store, sub_a),
            [identity("patient-id", "P1", "csv")],
            "{name}"
        );
        // the same import again changes nothing
        let again = linkage::import(reg.store(), &mut store, &keys, "patient-id", &rows).unwrap();
        assert_eq!(
            (
                again.subjects_created,
                again.identities_added,
                again.unchanged
            ),
            (0, 0, 2),
            "{name}"
        );
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 3, "{name}");
    }
}

#[test]
fn a_blake2b_8_registry_reproduces_the_v0_codes() {
    // the fixture of §7.1: PID-0001 under nils-fixture-key
    for lab in labs_keyed(Scheme::Blake2b8, 16, b"nils-fixture-key") {
        let name = lab.name;
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();
        let dir = TempDir::new("identity-v0");
        dir.file("a/IM_0001", &mr("A", "A.1", "A.1.1", "PID-0001", &[]));
        dir.file("a/IM_0002", &mr("A", "A.2", "A.2.1", "PID-0001", &[]));
        dir.file("b/IM_0001", &mr("B", "B.1", "B.1.1", " PID-0001 ", &[]));
        dir.file("c/IM_0001", &mr("C", "C.1", "C.1.1", "PID-0002", &[]));
        let mut s = settings(&dir);
        // one file per batch: the second and third meet the subject through the store
        s.batch_rows = 1;
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let w = report.written.clone().unwrap();
        assert_eq!(w.subjects_created, 2, "{name}");
        // every returning identifier of one study tree is one subject
        let sub_a = subject_of_study(&mut reg, "A");
        assert_eq!(subject_of_study(&mut reg, "B"), sub_a, "{name}");
        assert_ne!(subject_of_study(&mut reg, "C"), sub_a, "{name}");
        assert_eq!(code_of(&mut reg, sub_a), "771c4326c89c082c", "{name}");
        assert_eq!(
            revealed(&mut reg, &mut store, sub_a),
            [identity("patient-id", "PID-0001", "dicom")],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {subject} WHERE code_digest IS NULL"
            ),
            0,
            "{name}"
        );
    }
}

#[test]
fn two_identifiers_on_one_code_stop_the_job_with_a_review_item() {
    // one character of display: identifiers that share it are easy to find
    let mut by_code: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in 0..256 {
        let id = format!("P{i}");
        let code = pseudonym::code(Scheme::DEFAULT, KEY, &id, 1).code;
        by_code.entry(code).or_default().push(id);
    }
    let mut pairs = by_code.values().filter(|ids| ids.len() >= 2);
    let (a, b) = {
        let ids = pairs.next().expect("two identifiers with one display code");
        (ids[0].clone(), ids[1].clone())
    };
    let (c, d) = {
        let ids = pairs.next().expect("a second pair");
        (ids[0].clone(), ids[1].clone())
    };
    for lab in labs_with(Scheme::DEFAULT, 1) {
        let name = lab.name;
        let dir = TempDir::new("identity-collision");
        dir.file("a/IM_0001", &mr("A", "A.1", "A.1.1", &a, &[]));
        let mut s = settings(&dir);
        s.batch_rows = 100;
        let mut reg = lab.open();

        // the first subject lands
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");

        // the second identifier derives the same display code: the job stops
        dir.file("b/IM_0001", &mr("B", "B.1", "B.1.1", &b, &[]));
        let err = failure(name, digest(&s, &mut reg));
        assert!(
            err.contains("identity collision under patient-id"),
            "{name}: {err}"
        );
        assert!(err.contains("review item 1 is open"), "{name}: {err}");
        assert!(!err.contains(&a) && !err.contains(&b), "{name}: {err}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {study}"), 1, "{name}");
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {job} ORDER BY id"),
            ["done", "failed"],
            "{name}"
        );
        assert!(
            texts(&mut reg, "SELECT error FROM {job} WHERE id = 2")[0]
                .contains("identity collision"),
            "{name}"
        );
        let item = rows(
            &mut reg,
            "SELECT kind, scope, status, CAST(ref AS TEXT), CAST(evidence AS TEXT) FROM {review_item}",
        );
        assert_eq!(item.len(), 1, "{name}");
        assert_eq!(item[0].text(0).unwrap(), "identity.collision", "{name}");
        assert_eq!(item[0].text(1).unwrap(), "subject", "{name}");
        assert_eq!(item[0].text(2).unwrap(), "open", "{name}");
        let reference: serde_json::Value = serde_json::from_str(item[0].text(3).unwrap()).unwrap();
        assert_eq!(reference["subject_id"], 1, "{name}");
        assert_eq!(reference["code"].as_str().unwrap().len(), 1, "{name}");
        let text = item[0].text(4).unwrap();
        let evidence: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(evidence["id_type"], "patient-id", "{name}");
        assert_eq!(evidence["reason"], "display-code", "{name}");
        assert_eq!(evidence["scheme"], "blake2b-32", "{name}");
        assert_eq!(evidence["display_length"], 1, "{name}");
        assert_eq!(evidence["batch_id"], 2, "{name}");
        assert!(!text.contains(&a) && !text.contains(&b), "{name}: {text}");

        // two more in one batch: refused before either lands
        let dir2 = TempDir::new("identity-collision-batch");
        dir2.file("c/IM_0001", &mr("C", "C.1", "C.1.1", &c, &[]));
        dir2.file("d/IM_0001", &mr("D", "D.1", "D.1.1", &d, &[]));
        let mut s2 = settings(&dir2);
        // one parser, one batch: each parser batches its own files
        s2.workers = 1;
        s2.batch_rows = 100;
        let err = failure(name, digest(&s2, &mut reg));
        assert!(
            err.contains("two identifiers of this batch"),
            "{name}: {err}"
        );
        assert!(!err.contains(&c) && !err.contains(&d), "{name}: {err}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE status = 'open'"
            ),
            2,
            "{name}"
        );
        let text = texts(
            &mut reg,
            "SELECT CAST(evidence AS TEXT) FROM {review_item} WHERE id = 2",
        );
        let evidence: serde_json::Value = serde_json::from_str(&text[0]).unwrap();
        assert_eq!(evidence["reason"], "batch", "{name}");
        assert_eq!(evidence["batch_id"], 3, "{name}");
    }
}

#[test]
fn a_person_may_hold_several_identifiers_of_one_type() {
    // A personnummer is not for life: a temporary number becomes a permanent
    // one when residency is granted, someone may carry several temporary ones
    // before that, and the permanent one itself changes when a legal sex
    // change does (the number says which). However many they are, they are one
    // person: the import files them all on the one subject, in one file or in
    // several, and a digest of any of them lands there (§7.4).
    for lab in labs() {
        let name = lab.name;
        let numbers = ["spare-01", "spare-02", "main-01"];
        let dir = TempDir::new("identity-spare");
        for (i, number) in numbers.iter().enumerate() {
            let study = format!("S{i}");
            dir.file(
                &format!("{study}/IM_0001"),
                &mr(
                    &study,
                    &format!("{study}.1"),
                    &format!("{study}.1.1"),
                    number,
                    &[],
                ),
            );
        }
        // the number the sex change brought, digested after the others
        dir.file("S9/IM_0001", &mr("S9", "S9.1", "S9.1.1", "main-02", &[]));
        let s = settings(&dir);
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();
        let keys = Subkeys::derive(&reg.pseudonym_key().unwrap());
        let row = |line: usize, identifier: &str| ImportRow {
            line,
            identifier: identifier.to_string(),
            code: "one-person".to_string(),
        };
        let rows: Vec<ImportRow> = numbers
            .iter()
            .enumerate()
            .map(|(i, n)| row(i + 2, n))
            .collect();
        let imported =
            linkage::import(reg.store(), &mut store, &keys, "patient-id", &rows).unwrap();
        assert_eq!(
            (
                imported.subjects_created,
                imported.identities_added,
                imported.second_identifiers
            ),
            (1, 3, 2),
            "{name}"
        );
        // a later file adds a fourth to the subject the first import created
        let again = linkage::import(
            reg.store(),
            &mut store,
            &keys,
            "patient-id",
            &[row(2, "main-02")],
        )
        .unwrap();
        assert_eq!(
            (
                again.subjects_created,
                again.identities_added,
                again.second_identifiers
            ),
            (0, 1, 1),
            "{name}"
        );
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.written.unwrap().subjects_created, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");
        let subject = subject_of_study(&mut reg, "S0");
        for study in ["S1", "S2", "S9"] {
            assert_eq!(
                subject_of_study(&mut reg, study),
                subject,
                "{name}: {study}"
            );
        }
        let mut held = revealed(&mut reg, &mut store, subject);
        held.sort();
        assert_eq!(
            held,
            [
                identity("patient-id", "main-01", "csv"),
                identity("patient-id", "main-02", "csv"),
                identity("patient-id", "spare-01", "csv"),
                identity("patient-id", "spare-02", "csv"),
            ],
            "{name}"
        );
    }
}

#[test]
fn a_verbatim_rule_files_the_code_the_files_carry() {
    // Data pseudonymized before it reaches us: the anonymizer wrote the
    // subject code into PatientID, so the digest files it as the code
    // instead of deriving one (§7.3). A value that is not shaped like a code
    // is not one, and the file falls back to its study UID.
    let rule = Rule::parse(
        "identity:\n  id_type: subject-code\n  from:\n    - field: PatientID\n      pattern: '^(?<id>[0-9a-f]{16})$'\n  code: verbatim\n",
    )
    .unwrap();
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("identity-verbatim");
        dir.file(
            "a/IM_0001",
            &mr("A", "A.1", "A.1.1", "771c4326c89c082c", &[]),
        );
        dir.file(
            "a/IM_0002",
            &mr("A", "A.2", "A.2.1", "771c4326c89c082c", &[]),
        );
        // a personnummer where a code was expected: no subject code of ours
        dir.file("b/IM_0001", &mr("B", "B.1", "B.1.1", "19800101-1234", &[]));
        let mut s = settings(&dir);
        s.identity = rule.clone();
        let mut reg = lab.open();
        let mut store = reg.open_linkage().unwrap();
        linkage::add_id_type(
            &mut store,
            "subject-code",
            Some("the code the anonymizer wrote"),
        )
        .unwrap();
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            report.written.clone().unwrap().subjects_created,
            2,
            "{name}"
        );
        // the code is the value, not a hash of it
        let codes = texts(&mut reg, "SELECT code FROM {subject} ORDER BY code");
        assert!(
            codes.contains(&"771c4326c89c082c".to_string()),
            "{name}: {codes:?}"
        );
        assert_eq!(codes.len(), 2, "{name}: {codes:?}");
        let subject = subject_of_study(&mut reg, "A");
        assert_eq!(
            revealed(&mut reg, &mut store, subject),
            [identity("subject-code", "771c4326c89c082c", "dicom")],
            "{name}"
        );
        // the file whose PatientID is no code fell back to its study UID
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.kind == "identity_fallback")
                .count(),
            1,
            "{name}"
        );
        let other = subject_of_study(&mut reg, "B");
        assert_eq!(
            revealed(&mut reg, &mut store, other),
            [identity("study-instance-uid", "B", "dicom")],
            "{name}"
        );
    }
}
