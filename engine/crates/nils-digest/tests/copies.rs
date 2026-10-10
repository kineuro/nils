// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55, Nima's duplicate policy of 2026-10-10: a file whose subject,
//! study, series and instance UIDs are an instance the registry holds is a
//! location of that instance, `known` when another root read it first and
//! `twice` when its own root holds another file of it, and every root that
//! holds a file of a stack says so in `source_stack`; a file whose instance
//! UID the registry holds under another subject, study or series is held,
//! asked about once per root and pair of subjects, read again by every run,
//! and filed as a copy once a merge of the two subjects answers the
//! question. Every test runs on each backend.

mod common;

use std::fs;

use nils_dicom::synth::TempDir;
use nils_digest::digest;
use nils_registry::linkage::Subkeys;
use nils_registry::merge::{self, Ask};
use nils_registry::review::SAME_INSTANCE_KIND;

use common::*;

/// The source a run's batch read.
fn source_of(reg: &mut nils_registry::Registry, batch: i64) -> i64 {
    one(
        reg,
        &format!("SELECT source_id FROM {{ingest_batch}} WHERE id = {batch}"),
    )
}

fn stack_of(reg: &mut nils_registry::Registry, sop: &str) -> i64 {
    one(
        reg,
        &format!("SELECT stack_id FROM {{instance}} WHERE sop_instance_uid = '{sop}'"),
    )
}

#[test]
fn a_copy_another_root_read_first_is_known_and_its_root_holds_the_stack() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        let first = digest(&settings(&dir), &mut reg).unwrap();
        let first_source = source_of(&mut reg, first.written.unwrap().batch_id);
        // the first root holds a file of every stack it read
        assert_eq!(
            ints(
                &mut reg,
                &format!(
                    "SELECT stack_id FROM {{source_stack}} WHERE source_id = {first_source} ORDER BY stack_id"
                )
            ),
            ints(&mut reg, "SELECT id FROM {stack} ORDER BY id"),
            "{name}"
        );
        let instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.1'",
        );

        // another root holds the same file: subject, study, series and
        // instance all the instance's
        let other = TempDir::new("copies-other");
        other.file(
            "x/IM_0001",
            &fs::read(dir.path().join("sub1/IM_0001")).unwrap(),
        );
        let r = digest(&settings(&other), &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!(
            (w.ingested, w.duplicate, w.known, w.twice, w.same_instance),
            (0, 1, 1, 0, 0),
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        let second = source_of(&mut reg, w.batch_id);
        assert_ne!(second, first_source, "{name}");
        assert_eq!(
            opt_int(
                &mut reg,
                &format!(
                    "SELECT instance_id FROM {{source_file}} WHERE source_id = {second} AND path = 'x/IM_0001'"
                )
            ),
            Some(instance),
            "{name}: the copy is a location of the instance"
        );
        // the second root holds the stack, though the first read it
        let stack = stack_of(&mut reg, "A.1.1");
        assert_eq!(
            ints(
                &mut reg,
                &format!("SELECT stack_id FROM {{source_stack}} WHERE source_id = {second}")
            ),
            [stack],
            "{name}"
        );
        // and every file says when it was first seen
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE first_seen_at IS NULL"
            ),
            0,
            "{name}"
        );

        // read again, nothing changes and nothing is counted twice
        let r = digest(&settings(&other), &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!((w.duplicate, w.known, w.twice), (0, 0, 0), "{name}");
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{source_stack}} WHERE source_id = {second}")
            ),
            1,
            "{name}"
        );
    }
}

#[test]
fn a_second_file_of_an_instance_in_one_root_is_twice() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();
        let bytes = fs::read(dir.path().join("sub1/IM_0001")).unwrap();
        dir.file("sub1/again/IM_0001", &bytes);
        dir.file("sub1/again/IM_0001b", &bytes);
        let r = digest(&s, &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!(
            (w.ingested, w.duplicate, w.known, w.twice),
            (0, 2, 0, 2),
            "{name}"
        );
        // a new instance with two files in one run: its own, and one twice
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        let fresh = mr("A", "A.1", "A.1.7", "P1", &p1);
        dir.file("sub1/new/IM_0007", &fresh);
        dir.file("sub1/new/IM_0007b", &fresh);
        let r = digest(&s, &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!(
            (w.ingested, w.duplicate, w.known, w.twice),
            (1, 1, 0, 1),
            "{name}"
        );
    }
}

#[test]
fn the_same_instance_under_another_subject_is_held_and_asked_until_a_merge() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        let holder = one(
            &mut reg,
            "SELECT se.subject_id FROM {instance} i JOIN {series} se ON se.id = i.series_id \
             WHERE i.sop_instance_uid = 'A.1.1'",
        );

        // the scan's UIDs under another person's identifier
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        let other = TempDir::new("copies-other-subject");
        other.file("y/IM_0001", &mr("A", "A.1", "A.1.1", "P9", &p1));
        let r = digest(&settings(&other), &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!(
            (w.ingested, w.duplicate, w.same_instance),
            (0, 0, 1),
            "{name}"
        );
        let second = source_of(&mut reg, w.batch_id);
        assert_eq!(
            texts(
                &mut reg,
                &format!(
                    "SELECT status FROM {{source_file}} WHERE source_id = {second} AND reason = '{SAME_INSTANCE_KIND}' \
                     AND instance_id IS NULL"
                )
            ),
            ["quarantined"],
            "{name}"
        );
        // no location of the instance, and one question naming both subjects
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{source_stack}} WHERE source_id = {second}")
            ),
            0,
            "{name}"
        );
        let items = rows(
            &mut reg,
            &format!(
                "SELECT ref, evidence, status, members FROM {{review_item}} WHERE kind = '{SAME_INSTANCE_KIND}'"
            ),
        );
        assert_eq!(items.len(), 1, "{name}");
        let reference: serde_json::Value = serde_json::from_str(items[0].text(0).unwrap()).unwrap();
        let evidence: serde_json::Value = serde_json::from_str(items[0].text(1).unwrap()).unwrap();
        assert_eq!(items[0].text(2).unwrap(), "open", "{name}");
        assert_eq!(reference["holder_id"], serde_json::json!(holder), "{name}");
        assert_eq!(evidence["files"], serde_json::json!(1), "{name}");
        assert_eq!(
            evidence["differs"],
            serde_json::json!({"subject": 1}),
            "{name}"
        );
        let alias = reference["subject_id"].as_i64().unwrap();
        assert_ne!(alias, holder, "{name}");

        // a run reads the held file again, holds it again, and asks once
        let r = digest(&settings(&other), &mut reg).unwrap();
        assert_eq!(r.written.unwrap().same_instance, 1, "{name}");
        assert_eq!(
            one(
                &mut reg,
                &format!(
                    "SELECT COUNT(*) FROM {{review_item}} WHERE kind = '{SAME_INSTANCE_KIND}'"
                )
            ),
            1,
            "{name}"
        );

        // the two are one person: after the merge the file is a copy
        let mut linkage = reg.open_linkage().unwrap();
        let keys = Subkeys::derive(&reg.pseudonym_key().unwrap());
        merge::merge(
            reg.store(),
            &mut linkage,
            &keys,
            &Ask {
                canonical: holder,
                alias,
                why: "test",
                actor: "tester",
                job_id: None,
                place_id: None,
            },
        )
        .unwrap();
        let r = digest(&settings(&other), &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!((w.duplicate, w.known, w.same_instance), (1, 1, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                &format!("SELECT status FROM {{review_item}} WHERE kind = '{SAME_INSTANCE_KIND}'")
            ),
            ["superseded"],
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                &format!("SELECT stack_id FROM {{source_stack}} WHERE source_id = {second}")
            ),
            [stack_of(&mut reg, "A.1.1")],
            "{name}"
        );
    }
}

#[test]
fn the_same_instance_uid_in_another_series_is_held() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        // the subject's own identifier, the instance's UID, another series
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        let other = TempDir::new("copies-other-series");
        other.file("z/IM_0001", &mr("A", "A.9", "A.1.1", "P1", &p1));
        let r = digest(&settings(&other), &mut reg).unwrap();
        let w = r.written.unwrap();
        assert_eq!(
            (w.ingested, w.duplicate, w.same_instance),
            (0, 0, 1),
            "{name}"
        );
        let detail = texts(
            &mut reg,
            &format!("SELECT detail FROM {{source_file}} WHERE reason = '{SAME_INSTANCE_KIND}'"),
        );
        assert_eq!(detail.len(), 1, "{name}");
        assert!(
            detail[0].ends_with("|differs:series"),
            "{name}: {}",
            detail[0]
        );
        // no row of it was written: no series, and nothing to sweep
        assert_eq!(
            (w.empty_series_removed, w.empty_stacks_removed),
            (0, 0),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {series} WHERE series_instance_uid = 'A.9'"
            ),
            0,
            "{name}"
        );
    }
}

/// A registry from before filed a copy by its instance UID alone; migration
/// 87 marks it `unchecked` and records no location of it. The next read of
/// its root compares it: a copy of the same subject's instance becomes a
/// location, and a file that names another subject is held and asked about.
#[test]
fn a_copy_filed_before_the_policy_is_compared_on_its_next_read() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap();
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        let other = TempDir::new("copies-before");
        other.file(
            "y/IM_0002",
            &fs::read(dir.path().join("sub1/IM_0002")).unwrap(),
        );
        other.file("y/IM_0001", &mr("A", "A.1", "A.1.1", "P9", &p1));
        let r = digest(&settings(&other), &mut reg).unwrap();
        let second = source_of(&mut reg, r.written.unwrap().batch_id);
        // as a registry from before holds them, migrated
        let instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.1'",
        );
        for sql in [
            format!(
                "UPDATE {{source_file}} SET status = 'duplicate', reason = 'unchecked', detail = NULL, \
                 instance_id = {instance} WHERE path = 'y/IM_0001'"
            ),
            "UPDATE {source_file} SET reason = 'unchecked' WHERE path = 'y/IM_0002'".to_string(),
            format!("DELETE FROM {{source_stack}} WHERE source_id = {second}"),
            format!("DELETE FROM {{review_item}} WHERE kind = '{SAME_INSTANCE_KIND}'"),
        ] {
            let sql = rows_sql(&mut reg, &sql);
            reg.store().execute(&sql, &[]).unwrap();
        }

        let r = digest(&settings(&other), &mut reg).unwrap();
        assert_eq!(r.parsed, 2, "{name}: both read again");
        let w = r.written.unwrap();
        assert_eq!((w.duplicate, w.known, w.same_instance), (1, 1, 1), "{name}");
        assert_eq!(
            ints(
                &mut reg,
                &format!("SELECT stack_id FROM {{source_stack}} WHERE source_id = {second}")
            ),
            [stack_of(&mut reg, "A.1.2")],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE reason = 'unchecked'"
            ),
            0,
            "{name}"
        );
        assert_eq!(
            texts(
                &mut reg,
                &format!("SELECT status FROM {{review_item}} WHERE kind = '{SAME_INSTANCE_KIND}'")
            ),
            ["open"],
            "{name}"
        );
        // and a further run reads the copy no more
        let r = digest(&settings(&other), &mut reg).unwrap();
        assert_eq!(r.parsed, 1, "{name}: only the held file");
    }
}
