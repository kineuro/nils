// SPDX-License-Identifier: AGPL-3.0-only

//! The digest over a synthetic tree, into a registry on SQLite and, when
//! `NILS_TEST_POSTGRES_DSN` is set, on Postgres: the rows a first run writes
//! and what a second run leaves alone (§5.2), duplicates (§5.3), changed and
//! gone files, the retry of quarantine, a restart, the disagreements the
//! writer raises as diagnostics (§9.1), and the job that holds the registry
//! (§10). Every test runs on each backend and expects the same numbers.

mod common;

use std::fs;
use std::time::Duration;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, TempDir};
use nils_digest::{DigestError, Report, digest};
use nils_registry::schema::table;
use nils_registry::store::Cell;
use nils_registry::time::{iso_of, now_iso, now_secs};
use nils_registry::{Insert, Param, Registry};

use common::*;

/// The counts a report carries, without the timings, the root, and the
/// number of batches (how the parsers split the files is a matter of timing).
fn shape(report: &Report) -> serde_json::Value {
    let mut v = serde_json::to_value(report).unwrap();
    let o = v.as_object_mut().unwrap();
    for k in [
        "elapsed_s",
        "files_per_s",
        "mb_per_s",
        "peak_rss_bytes",
        "setup",
        "root",
    ] {
        o.remove(k);
    }
    if let Some(w) = o.get_mut("written").and_then(|w| w.as_object_mut()) {
        w.remove("writes");
        w.remove("epoch");
    }
    v
}

#[test]
fn a_first_run_writes_the_rows_and_a_second_leaves_them_alone() {
    let mut shapes = Vec::new();
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();

        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.seen, 6, "{name}");
        assert_eq!(report.parsed, 5, "{name}");
        assert_eq!(report.quarantined, 1, "{name}");
        assert_eq!(report.unchanged, 0, "{name}");
        assert_eq!(report.subjects, 2, "{name}");
        assert_eq!(report.studies, 2, "{name}");
        assert_eq!(report.series, 4, "{name}");
        assert!(
            report.diagnostics.is_empty(),
            "{name}: {:?}",
            report.diagnostics
        );
        let w = report.written.clone().unwrap();
        assert_eq!(w.batch_id, 1, "{name}");
        // the epoch moves once per batch written (§4.2)
        assert!(w.writes >= 1, "{name}");
        assert_eq!(w.epoch, w.writes as i64, "{name}");
        let epoch = w.epoch;
        assert_eq!(w.ingested, 5, "{name}");
        assert_eq!(w.duplicate, 0, "{name}");
        assert_eq!(w.changed, 0, "{name}");
        assert_eq!(w.quarantine_kept, 0, "{name}");
        assert_eq!(w.gone, 0, "{name}");
        assert_eq!(w.subjects_created, 2, "{name}");
        assert_eq!(w.studies_created, 2, "{name}");
        assert_eq!(w.series_created, 4, "{name}");

        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {study}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 4, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_mr}"),
            2,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_ct}"),
            1,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {series_pet}"),
            1,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {source_file}"),
            6,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {diagnostic}"),
            0,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT n_instances FROM {series} ORDER BY series_instance_uid"
            ),
            [2, 1, 1, 1],
            "{name}"
        );
        // every instance points at its file and every ingested file at its instance
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {instance} i JOIN {source_file} f ON f.id = i.source_file_id AND f.instance_id = i.id"
            ),
            5,
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT status FROM {source_file} ORDER BY path"),
            [
                "quarantined",
                "ingested",
                "ingested",
                "ingested",
                "ingested",
                "ingested"
            ],
            "{name}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT reason FROM {source_file} WHERE status = 'quarantined'"
            ),
            ["not_dicom"],
            "{name}"
        );
        // the subject rows carry the catalogue values and the pseudonym, not the identifier
        assert_eq!(
            texts(&mut reg, "SELECT sex FROM {subject} ORDER BY sex"),
            ["F", "M"],
            "{name}"
        );
        let codes = texts(&mut reg, "SELECT code FROM {subject}");
        assert!(
            codes
                .iter()
                .all(|c| c.len() == 12 && c != "P1" && c != "P2"),
            "{name}: {codes:?}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT study_description FROM {study} ORDER BY study_description"
            ),
            ["Brain", "Chest"],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {ingest_batch}"),
            ["done"],
            "{name}"
        );
        assert_eq!(
            ints(&mut reg, "SELECT epoch_after FROM {ingest_batch}"),
            [epoch],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {job}"),
            ["done"],
            "{name}"
        );
        reg.refresh_meta().unwrap();
        assert_eq!(reg.meta().epoch, epoch, "{name}");

        // the second run reads nothing and moves every row to the new batch
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(again.seen, 6, "{name}");
        assert_eq!(again.parsed, 0, "{name}");
        assert_eq!(again.quarantined, 0, "{name}");
        assert_eq!(again.unchanged, 6, "{name}");
        let w = again.written.clone().unwrap();
        assert_eq!(w.batch_id, 2, "{name}");
        // unchanged files are rows too: their batches move the epoch
        assert!(w.writes >= 1, "{name}");
        assert_eq!(w.epoch, epoch + w.writes as i64, "{name}");
        assert_eq!(w.ingested, 0, "{name}");
        assert_eq!(w.quarantine_kept, 1, "{name}");
        assert_eq!(w.subjects_created, 0, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE batch_id = 2"
            ),
            6,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT epoch_after FROM {ingest_batch} ORDER BY id"
            ),
            [epoch, w.epoch],
            "{name}"
        );
        assert_eq!(
            texts(&mut reg, "SELECT state FROM {job} ORDER BY id"),
            ["done", "done"],
            "{name}"
        );
        shapes.push((name, shape(&report), shape(&again)));
    }
    // both backends report the same numbers
    for pair in shapes.windows(2) {
        assert_eq!(pair[0].1, pair[1].1, "{} vs {}", pair[0].0, pair[1].0);
        assert_eq!(pair[0].2, pair[1].2, "{} vs {}", pair[0].0, pair[1].0);
    }
}

#[test]
fn duplicates_changed_and_gone_files_are_filed_as_the_spec_says() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();
        let original = dir.path().join("sub1/IM_0001");
        let original_bytes = fs::read(&original).unwrap();
        let original_mtime = fs::metadata(&original).unwrap().modified().unwrap();
        let first_instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.1'",
        );

        // the same instance under a second path is a duplicate (§5.3)
        let copy = dir.file("sub1/copy/IM_0001", &original_bytes);
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.changed), (0, 1, 0), "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["duplicate"],
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT instance_id FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            Some(first_instance),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            2,
            "{name}"
        );

        // a file rewritten with another SOP instance: file_changed, the old
        // instance loses its file, the new one is ingested
        let changed = dir.path().join("sub1/IM_0002");
        let old_mtime = fs::metadata(&changed).unwrap().modified().unwrap();
        let p1 = [birth("19800101"), sex("M"), description("Brain")];
        fs::write(&changed, mr("A", "A.1", "A.1.9", "P1", &p1)).unwrap();
        fs::File::options()
            .write(true)
            .open(&changed)
            .unwrap()
            .set_modified(old_mtime + Duration::from_secs(10))
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 1, "{name}");
        let sample = r
            .diagnostics
            .iter()
            .find(|d| d.kind == "file_changed")
            .map(|d| d.samples.clone())
            .unwrap();
        assert_eq!(sample, ["new_sop"], "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.changed), (1, 0, 1), "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            6,
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.2'"
            ),
            None,
            "{name}: the replaced instance has no file"
        );
        let new_instance = one(
            &mut reg,
            "SELECT id FROM {instance} WHERE sop_instance_uid = 'A.1.9'",
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT instance_id FROM {source_file} WHERE path = 'sub1/IM_0002'"
            ),
            Some(new_instance),
            "{name}"
        );
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.9'"
            ),
            Some(one(
                &mut reg,
                "SELECT id FROM {source_file} WHERE path = 'sub1/IM_0002'"
            )),
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            3,
            "{name}"
        );

        // the duplicate goes: gone; back as it was: a duplicate again
        let copy_mtime = fs::metadata(&copy).unwrap().modified().unwrap();
        fs::remove_file(&copy).unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (6, 0, 6), "{name}");
        assert_eq!(r.written.as_ref().unwrap().gone, 1, "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["gone"],
            "{name}"
        );
        dir.file("sub1/copy/IM_0001", &original_bytes);
        fs::File::options()
            .write(true)
            .open(&copy)
            .unwrap()
            .set_modified(copy_mtime)
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 0, "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.gone), (0, 1, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/copy/IM_0001'"
            ),
            ["duplicate"],
            "{name}"
        );

        // the instance's own file goes and comes back: its own again, not a
        // duplicate of itself
        fs::remove_file(&original).unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().gone, 1, "{name}");
        assert_eq!(
            opt_int(
                &mut reg,
                "SELECT source_file_id FROM {instance} WHERE sop_instance_uid = 'A.1.1'"
            ),
            Some(one(
                &mut reg,
                "SELECT id FROM {source_file} WHERE path = 'sub1/IM_0001'"
            )),
            "{name}: a gone file keeps its instance"
        );
        dir.file("sub1/IM_0001", &original_bytes);
        fs::File::options()
            .write(true)
            .open(&original)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed, r.unchanged), (7, 1, 6), "{name}");
        assert_eq!(r.kind("file_changed"), 0, "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.duplicate, w.gone), (1, 0, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'sub1/IM_0001'"
            ),
            ["ingested"],
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            6,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            3,
            "{name}"
        );
    }
}

#[test]
fn the_quarantine_is_kept_until_retried_or_the_file_changes() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();

        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.quarantined, r.unchanged), (0, 6), "{name}");
        assert_eq!(r.written.as_ref().unwrap().quarantine_kept, 1, "{name}");

        s.retry_quarantine = true;
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.quarantined, r.unchanged), (1, 5), "{name}");
        assert_eq!(r.class("not_dicom"), 1, "{name}");
        assert_eq!(r.written.as_ref().unwrap().quarantine_kept, 0, "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'bad/readme.txt'"
            ),
            ["quarantined"],
            "{name}"
        );
        s.retry_quarantine = false;

        // the file becomes a DICOM file: read again without the flag
        dir.file("bad/readme.txt", &pet("B", "B.2", "B.2.2", "P2"));
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.parsed, r.quarantined, r.unchanged), (1, 0, 5), "{name}");
        let w = r.written.clone().unwrap();
        assert_eq!((w.ingested, w.quarantine_kept), (1, 0), "{name}");
        assert_eq!(
            texts(
                &mut reg,
                "SELECT status FROM {source_file} WHERE path = 'bad/readme.txt'"
            ),
            ["ingested"],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'B.2'"
            ),
            2,
            "{name}"
        );
    }
}

#[test]
fn a_restart_reads_everything_and_agrees_with_what_it_wrote() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap();

        s.restart = true;
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(
            (r.seen, r.parsed, r.quarantined, r.unchanged),
            (6, 5, 1, 0),
            "{name}"
        );
        // the rows read back hash as the rows written: no disagreement on either backend
        assert!(r.diagnostics.is_empty(), "{name}: {:?}", r.diagnostics);
        let w = r.written.clone().unwrap();
        assert_eq!(w.ingested, 5, "{name}");
        assert_eq!(w.duplicate, 0, "{name}");
        assert_eq!(w.changed, 0, "{name}");
        assert_eq!(
            w.subjects_created + w.studies_created + w.series_created,
            0,
            "{name}"
        );
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT n_instances FROM {series} ORDER BY series_instance_uid"
            ),
            [2, 1, 1, 1],
            "{name}: a restart does not count instances twice"
        );
        assert_eq!(
            texts(&mut reg, "SELECT status FROM {source_file} ORDER BY path"),
            [
                "quarantined",
                "ingested",
                "ingested",
                "ingested",
                "ingested",
                "ingested"
            ],
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE batch_id = 2"
            ),
            6,
            "{name}"
        );
    }
}

#[test]
fn disagreements_are_diagnostics_not_errors() {
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("digest-disagree");
        // each disagreement is between two files, so the count is one
        // whichever file the walk brings first; a null (a/2's birth date,
        // a/3's description, c/1's both) is no value and disagrees with nothing
        // one study described two ways
        dir.file(
            "a/1",
            &mr(
                "A",
                "A.1",
                "A.1.1",
                "P1",
                &[birth("19800101"), description("Brain")],
            ),
        );
        dir.file(
            "a/2",
            &mr("A", "A.2", "A.2.1", "P1", &[description("Head")]),
        );
        // one subject born twice
        dir.file("a/3", &mr("A", "A.3", "A.3.1", "P1", &[birth("19800102")]));
        // one series under two studies
        dir.file("c/1", &mr("C", "A.1", "C.1.1", "P1", &[]));
        let s = settings(&dir);
        let mut reg = lab.open();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!((r.seen, r.parsed), (4, 4), "{name}");
        assert_eq!(r.written.as_ref().unwrap().ingested, 4, "{name}");
        assert_eq!(
            r.kind("field_disagreement"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        assert_eq!(
            r.kind("subject_field_disagreement"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        assert_eq!(
            r.kind("series_multi_study"),
            1,
            "{name}: {:?}",
            r.diagnostics
        );
        let samples = |kind: &str| {
            r.diagnostics
                .iter()
                .find(|d| d.kind == kind)
                .map(|d| d.samples.clone())
                .unwrap_or_default()
        };
        let field = samples("field_disagreement");
        assert!(
            field == ["study.study_description=Aaaaa"] || field == ["study.study_description=Aaaa"],
            "{name}: {field:?}"
        );
        assert_eq!(
            samples("subject_field_disagreement"),
            ["subject.birth_date=9999-99-99"],
            "{name}"
        );
        assert_eq!(samples("series_multi_study"), ["series.study_id"], "{name}");
        // one row each: the study has one description, the subject one birth
        // date, the series one study
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 1, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {study}"), 2, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 3, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            4,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT n_instances FROM {series} WHERE series_instance_uid = 'A.1'"
            ),
            2,
            "{name}"
        );
        // the row is decided, not raced: the smaller value in the canonical
        // text order stands, whichever file the walk brought first (§9.1)
        let described = texts(
            &mut reg,
            "SELECT study_description FROM {study} WHERE study_instance_uid = 'A'",
        );
        assert_eq!(described, ["Brain"], "{name}");
        let born = texts(&mut reg, "SELECT CAST(birth_date AS TEXT) FROM {subject}");
        assert_eq!(born, ["1980-01-01"], "{name}");
        // the diagnostics are rows of the batch
        assert_eq!(
            texts(
                &mut reg,
                "SELECT kind FROM {diagnostic} WHERE batch_id = 1 ORDER BY kind"
            ),
            [
                "field_disagreement",
                "series_multi_study",
                "subject_field_disagreement"
            ],
            "{name}"
        );
        assert_eq!(
            ints(
                &mut reg,
                "SELECT count FROM {diagnostic} WHERE batch_id = 1 ORDER BY kind"
            ),
            [1, 1, 1],
            "{name}"
        );
    }
}

#[test]
fn a_fresh_job_holds_the_registry_and_a_stale_one_is_taken_over() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let job = table("job");
        let insert = Insert::new(
            job,
            &[
                "kind",
                "name",
                "args",
                "state",
                "pid",
                "host",
                "started_at",
                "heartbeat_at",
            ],
        )
        .returning(&["id"]);
        let now = now_iso();
        let inserted = reg
            .store()
            .insert(
                &insert,
                &[vec![
                    Param::from("digest"),
                    Param::from("other"),
                    Param::from("{}"),
                    Param::from("running"),
                    Param::from(4242_i64),
                    Param::from("elsewhere"),
                    Param::from(now.as_str()),
                    Param::from(now.as_str()),
                ]],
            )
            .unwrap();
        let other = inserted[0].int(0).unwrap();

        let err = digest(&s, &mut reg).unwrap_err();
        assert!(
            matches!(err, DigestError::Busy { job_id, .. } if job_id == other),
            "{name}: {err}"
        );
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {job}"), 1, "{name}");

        // a heartbeat two minutes old: the job is marked failed and the run goes on
        reg.store()
            .update_by_id(
                job,
                &[("heartbeat_at", Param::from(iso_of(now_secs() - 120)))],
                "id",
                other,
            )
            .unwrap();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().ingested, 5, "{name}");
        let states = rows(&mut reg, "SELECT state, error FROM {job} ORDER BY id");
        assert_eq!(states.len(), 2, "{name}");
        assert_eq!(states[0].text(0).unwrap(), "failed", "{name}");
        assert!(
            states[0]
                .text(1)
                .unwrap()
                .starts_with("stale: no heartbeat since "),
            "{name}: {:?}",
            states[0].get(1)
        );
        assert_eq!(states[1].text(0).unwrap(), "done", "{name}");
        assert!(matches!(states[1].get(1), Cell::Null), "{name}");
    }
}

#[test]
fn a_job_of_this_host_whose_process_is_gone_is_taken_over_at_once() {
    if !cfg!(unix) {
        return;
    }
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let host = texts(&mut reg, "SELECT host FROM {job} WHERE id = 1")[0].clone();

        // a process that has ended: its pid is free
        let child = std::process::Command::new("true")
            .spawn()
            .and_then(|mut c| c.wait().map(|_| c.id()))
            .expect("a child that ends");
        let running = |reg: &mut nils_registry::Registry, pid: u32| -> i64 {
            let now = now_iso();
            let inserted = reg
                .store()
                .insert(
                    &Insert::new(
                        table("job"),
                        &[
                            "kind",
                            "name",
                            "args",
                            "state",
                            "pid",
                            "host",
                            "started_at",
                            "heartbeat_at",
                        ],
                    )
                    .returning(&["id"]),
                    &[vec![
                        Param::from("digest"),
                        Param::from("other"),
                        Param::from("{}"),
                        Param::from("running"),
                        Param::from(i64::from(pid)),
                        Param::from(host.as_str()),
                        Param::from(now.as_str()),
                        Param::from(now.as_str()),
                    ]],
                )
                .unwrap();
            inserted[0].int(0).unwrap()
        };
        let dead = running(&mut reg, child);
        let r = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.unchanged, 6, "{name}");
        let error = texts(
            &mut reg,
            &format!("SELECT error FROM {{job}} WHERE id = {dead}"),
        );
        let expected = format!("process {child} is gone; no heartbeat since ");
        assert!(error[0].starts_with(&expected), "{name}: {}", error[0]);
        assert!(error[0].ends_with('Z'), "{name}: {}", error[0]);

        // this process is alive: its fresh job holds the registry
        let alive = running(&mut reg, std::process::id());
        let err = digest(&s, &mut reg).unwrap_err();
        assert!(
            matches!(err, DigestError::Busy { job_id, .. } if job_id == alive),
            "{name}: {err}"
        );
    }
}

#[test]
fn a_failed_batch_has_its_last_second_read_again_and_its_identities_repaired() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        let first = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            first.written.as_ref().unwrap().subjects_created,
            2,
            "{name}"
        );

        // As if the run had ended between the registry's commit and the
        // linkage store's (§9.3): the subjects stand, their identity rows
        // do not, and the batch ends failed with its last second marked.
        let mut linkage = reg.open_linkage().unwrap();
        let identity = linkage.qualified("identity");
        assert_eq!(
            linkage
                .query(&format!("SELECT COUNT(*) FROM {identity}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap(),
            2,
            "{name}"
        );
        linkage
            .execute(&format!("DELETE FROM {identity}"), &[])
            .unwrap();
        drop(linkage);
        let mark = rows_sql(
            &mut reg,
            "UPDATE {ingest_batch} SET state = 'failed', reparse_from = (SELECT MAX(f.seen_at) FROM {source_file} AS f WHERE f.batch_id = ingest_batch.id) WHERE id = 1",
        );
        let marked = reg.store().execute(&mark, &[]).unwrap();
        assert_eq!(marked, 1, "{name}");

        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        // every file of the batch was recorded in its last second
        assert_eq!(again.unchanged, 0, "{name}");
        assert_eq!(again.parsed, 5, "{name}");
        let w = again.written.as_ref().unwrap();
        assert_eq!(w.subjects_created, 0, "{name}");
        // the linkage store has not met the identifiers; the subjects are
        // found by their codes and the identities attached (§7.4 step 5)
        assert_eq!(w.subjects_matched, 0, "{name}");
        assert_eq!(w.identities_attached, 2, "{name}");
        assert_eq!(w.ingested, 5, "{name}");
        assert_eq!(w.changed, 0, "{name}");
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {subject}"), 2, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {instance}"),
            5,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {source_file} WHERE batch_id = 2 AND status = 'ingested'"
            ),
            5,
            "{name}"
        );
        let mut linkage = reg.open_linkage().unwrap();
        assert_eq!(
            linkage
                .query(&format!("SELECT COUNT(*) FROM {identity}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap(),
            2,
            "{name}"
        );

        // a third run leaves everything alone: the rows moved to batch 2
        let third = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(third.unchanged, 6, "{name}");
        assert_eq!(third.parsed, 0, "{name}");
    }
}

#[test]
fn a_batch_files_one_review_item_per_quarantine_class() {
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let s = settings(&dir);
        let mut reg = lab.open();
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let items = rows(
            &mut reg,
            "SELECT kind, scope, status, CAST(ref AS TEXT), CAST(evidence AS TEXT) FROM {review_item} ORDER BY id",
        );
        assert_eq!(items.len(), 1, "{name}");
        assert_eq!(items[0].text(0).unwrap(), "ingest.quarantine", "{name}");
        assert_eq!(items[0].text(1).unwrap(), "batch", "{name}");
        assert_eq!(items[0].text(2).unwrap(), "open", "{name}");
        let reference: serde_json::Value = serde_json::from_str(items[0].text(3).unwrap()).unwrap();
        assert_eq!(reference["batch_id"], 1, "{name}");
        assert_eq!(reference["class"], "not_dicom", "{name}");
        let evidence: serde_json::Value = serde_json::from_str(items[0].text(4).unwrap()).unwrap();
        assert_eq!(evidence, serde_json::json!({ "count": 1 }), "{name}");

        // a run that keeps the quarantine files nothing new
        digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {review_item}"),
            1,
            "{name}"
        );

        // a retry that quarantines again files a new item for its batch
        let mut retry = settings(&dir);
        retry.retry_quarantine = true;
        digest(&retry, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let refs = texts(
            &mut reg,
            "SELECT CAST(ref AS TEXT) FROM {review_item} ORDER BY id",
        );
        assert_eq!(refs.len(), 2, "{name}");
        let reference: serde_json::Value = serde_json::from_str(&refs[1]).unwrap();
        assert_eq!(reference["batch_id"], 3, "{name}");
    }
}

#[test]
fn a_disagreed_field_is_decided_by_value_not_by_order() {
    // Two series that carry the same three sequence names and body parts in
    // opposite file order, so the writer meets them in opposite orders, and a
    // third series that a later run adds a file to, when the row is no longer
    // in the writer's cache and is read back. Every one of them ends with the
    // smaller value in the canonical text order (§9.1).
    let variants = [("ep_b1000", "HEAD"), ("tse2d1_9", "NECK"), ("*fl3d1", "BRAIN")];
    let file = |series: &str, i: usize, sop: &str, (sequence, part): (&str, &str)| {
        (
            format!("{series}/IM_{i:04}"),
            mr(
                "A",
                series,
                sop,
                "P1",
                &[
                    synth::text(tags::BODY_PART_EXAMINED, VR::CS, part),
                    synth::text(tags::SEQUENCE_NAME, VR::SH, sequence),
                ],
            ),
        )
    };
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("digest-decided");
        for (i, v) in variants.iter().enumerate() {
            let (path, bytes) = file("A.1", i, &format!("A.1.{i}"), *v);
            dir.file(&path, &bytes);
            let (path, bytes) = file("A.2", i, &format!("A.2.{i}"), variants[2 - i]);
            dir.file(&path, &bytes);
        }
        // the third series starts with the middle value alone
        let (path, bytes) = file("A.3", 0, "A.3.0", variants[1]);
        dir.file(&path, &bytes);
        let mut s = settings(&dir);
        // one file per batch, so the walk order is the writer's order
        s.workers = 1;
        s.walk_threads = 1;
        s.batch_rows = 1;
        let mut reg = lab.open();
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().ingested, 7, "{name}");
        let decided = |reg: &mut Registry, uid: &str| {
            texts(
                reg,
                &format!(
                    "SELECT sequence_name || ' ' || body_part_examined FROM {{series}} \
                     WHERE series_instance_uid = '{uid}'"
                ),
            )
        };
        // the two orders decide the same row
        assert_eq!(decided(&mut reg, "A.1"), ["*fl3d1 BRAIN"], "{name}");
        assert_eq!(decided(&mut reg, "A.2"), ["*fl3d1 BRAIN"], "{name}");
        // a later run, with nothing of the row in its cache, still decides it:
        // the smaller value replaces the stored one, a larger one does not
        let (path, bytes) = file("A.3", 1, "A.3.1", variants[2]);
        dir.file(&path, &bytes);
        let r = digest(&s, &mut reg).unwrap();
        assert_eq!(r.written.as_ref().unwrap().ingested, 1, "{name}");
        assert_eq!(decided(&mut reg, "A.3"), ["*fl3d1 BRAIN"], "{name}");
        let (path, bytes) = file("A.3", 2, "A.3.2", variants[0]);
        dir.file(&path, &bytes);
        digest(&s, &mut reg).unwrap();
        assert_eq!(
            decided(&mut reg, "A.3"),
            ["*fl3d1 BRAIN"],
            "{name}: a larger value replaced the row"
        );
        // every file after the first of a series disagrees on both fields
        assert_eq!(one(&mut reg, "SELECT COUNT(*) FROM {series}"), 3, "{name}");
    }
}
