// SPDX-License-Identifier: AGPL-3.0-only

//! `nils digest --reread` on SQLite and, when `NILS_TEST_POSTGRES_DSN` is
//! set, on Postgres: which manufacturers a selection takes, with case and
//! without; that the files are read a series at a time, each page bounded
//! by its series; and that a re-read which did not finish is continued by
//! the next one rather than started over.

mod common;

use std::collections::BTreeSet;

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, TempDir};
use nils_digest::reread::{Pages, Selection, first_sql, page_sql, targets};
use nils_digest::{Settings, digest};
use nils_registry::store::Param;
use nils_registry::{Backend, Registry};

use common::*;

fn maker(m: &str) -> synth::Elem {
    synth::text(tags::MANUFACTURER, VR::LO, m)
}

/// Six studies, one per spelling: GE with a series of five files and one
/// of one, Siemens Healthineers (XA30 and later), `Siemens` (XA11 and XA20),
/// `SIEMENS` (everything older), Philips, and a GE CT.
fn fleet() -> TempDir {
    let dir = TempDir::new("reread");
    let mut n = 0;
    let mut put = |study: &str, series: &str, files: usize, m: &str, ct_: bool| {
        for i in 1..=files {
            n += 1;
            let sop = format!("{series}.{i}");
            let bytes = match ct_ {
                true => ct(study, series, &sop, &format!("P{study}"), &[maker(m)]),
                false => mr(study, series, &sop, &format!("P{study}"), &[maker(m)]),
            };
            dir.file(&format!("{study}/{series}/IM_{n:04}"), &bytes);
        }
    };
    put("1", "1.1", 5, "GE MEDICAL SYSTEMS", false);
    put("1", "1.2", 1, "GE MEDICAL SYSTEMS", false);
    put("2", "2.1", 2, "Siemens Healthineers", false);
    put("3", "3.1", 2, "Siemens", false);
    put("4", "4.1", 3, "SIEMENS", false);
    put("5", "5.1", 1, "Philips", false);
    put("6", "6.1", 1, "GE MEDICAL SYSTEMS", true);
    dir
}

/// The GE, Siemens Healthineers and Siemens XA selection production runs.
fn planned() -> (Vec<String>, Vec<String>) {
    (
        vec!["GE MEDICAL SYSTEMS".into(), "Siemens Healthineers".into()],
        vec!["Siemens".into()],
    )
}

fn series_uids(reg: &mut Registry, ids: &[i64]) -> Vec<String> {
    let mut out: Vec<String> = ids
        .iter()
        .map(|id| {
            texts(
                reg,
                &format!("SELECT series_instance_uid FROM {{series}} WHERE id = {id}"),
            )[0]
            .clone()
        })
        .collect();
    out.sort();
    out
}

fn reread(dir: &TempDir, any: &[String], exact: &[String]) -> Settings {
    let mut s = settings(dir);
    s.reread = any.to_vec();
    s.reread_exact = exact.to_vec();
    s
}

#[test]
fn a_selection_takes_the_spellings_it_names_and_no_other() {
    for lab in labs() {
        let name = lab.name;
        let dir = fleet();
        let mut reg = lab.open();
        let first = digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(first.parsed, 15, "{name}");

        let mut resolve = |any: &[&str], exact: &[&str]| {
            let any: Vec<String> = any.iter().map(|s| s.to_string()).collect();
            let exact: Vec<String> = exact.iter().map(|s| s.to_string()).collect();
            let ids = targets(reg.store(), &Selection::new(&any, &exact)).unwrap();
            series_uids(&mut reg, &ids)
        };
        // without case, `siemens` is the XA spelling and the older fleet
        assert_eq!(resolve(&["siemens"], &[]), ["3.1", "4.1"], "{name}");
        // with case, only the XA spelling
        assert_eq!(resolve(&[], &["Siemens"]), ["3.1"], "{name}");
        assert_eq!(
            resolve(&[], &[" Siemens "]),
            ["3.1"],
            "{name}: outer spaces aside"
        );
        assert!(resolve(&[], &["siemens"]).is_empty(), "{name}");
        // the CT is GE's and not MR
        assert_eq!(
            resolve(
                &["GE MEDICAL SYSTEMS", "Siemens Healthineers"],
                &["Siemens"]
            ),
            ["1.1", "1.2", "2.1", "3.1"],
            "{name}"
        );

        // the planned re-read reads those four series' ten files and leaves
        // every other file's row as the first run left it
        let (any, exact) = planned();
        let report =
            digest(&reread(&dir, &any, &exact), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 10, "{name}");
        assert_eq!(report.unchanged, 0, "{name}");
        let batch = report.written.as_ref().unwrap().batch_id;
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{source_file}} WHERE batch_id = {batch}")
            ),
            10,
            "{name}"
        );
        let config = texts(
            &mut reg,
            &format!("SELECT config FROM {{ingest_batch}} WHERE id = {batch}"),
        );
        let config: serde_json::Value = serde_json::from_str(&config[0]).unwrap();
        assert_eq!(
            config["reread_exact"],
            serde_json::json!(["Siemens"]),
            "{name}"
        );
    }
}

#[test]
fn the_files_are_paged_a_series_at_a_time_each_page_bounded() {
    for lab in labs() {
        let name = lab.name;
        let dir = fleet();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source = one(&mut reg, "SELECT MIN(id) FROM {source}");
        let (any, exact) = planned();
        let series = targets(reg.store(), &Selection::new(&any, &exact)).unwrap();
        assert_eq!(series.len(), 4, "{name}");

        // pages of two: the five-file series is three pages, the others one
        let mut pages = Pages::new(reg.store(), source, series.clone(), 2);
        let mut sizes = Vec::new();
        let mut seen = Vec::new();
        while let Some(page) = pages.next(reg.store()).unwrap() {
            sizes.push(page.len());
            seen.extend(page.iter().map(|f| f.id));
        }
        // (the series' order is their ids', which the parsers' race decides)
        sizes.sort();
        assert_eq!(sizes, [1, 1, 2, 2, 2, 2], "{name}");
        let distinct: BTreeSet<i64> = seen.iter().copied().collect();
        assert_eq!(distinct.len(), seen.len(), "{name}: no file twice");
        let expected: BTreeSet<i64> = ints(
            &mut reg,
            "SELECT f.id FROM {source_file} f JOIN {instance} i ON i.source_file_id = f.id \
             JOIN {series} s ON s.id = i.series_id \
             WHERE s.series_instance_uid IN ('1.1', '1.2', '2.1', '3.1')",
        )
        .into_iter()
        .collect();
        assert_eq!(distinct, expected, "{name}");

        // Every page is an index lookup of one series joined by primary key:
        // on SQLite the plan searches both tables and scans neither.
        if reg.store().backend() == Backend::Sqlite {
            let sql = format!("EXPLAIN QUERY PLAN {}", page_sql(reg.store()));
            let plan: Vec<String> = reg
                .store()
                .query(
                    &sql,
                    &[
                        Param::Int(series[0]),
                        Param::Int(source),
                        Param::Int(0),
                        Param::Int(2),
                    ],
                )
                .unwrap()
                .iter()
                .map(|r| r.text(3).unwrap().to_string())
                .collect();
            let text = plan.join("\n");
            assert!(
                plan.iter()
                    .any(|l| l.contains("instance") && l.contains("series_id")),
                "{text}"
            );
            assert!(
                plan.iter()
                    .any(|l| l.starts_with("SEARCH f USING INTEGER PRIMARY KEY (rowid=?)")),
                "{text}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|l| l.starts_with("SCAN i") || l.starts_with("SCAN f")),
                "{text}"
            );
        }
    }
}

#[test]
fn a_reread_that_did_not_finish_is_continued_and_a_finished_one_is_not() {
    for lab in labs() {
        let name = lab.name;
        let dir = fleet();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (any, exact) = planned();
        let s = reread(&dir, &any, &exact);

        // A re-read that stopped after four of its ten files: its batch
        // ended cancelled, and the writer had set the four files' rows to it.
        let first = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let stopped = first.written.as_ref().unwrap().batch_id;
        let first_ingest = one(&mut reg, "SELECT MIN(id) FROM {ingest_batch}");
        rows(
            &mut reg,
            &format!("UPDATE {{ingest_batch}} SET state = 'cancelled' WHERE id = {stopped}"),
        );
        let later: Vec<i64> = ints(
            &mut reg,
            &format!("SELECT id FROM {{source_file}} WHERE batch_id = {stopped} ORDER BY id"),
        );
        assert_eq!(later.len(), 10, "{name}");
        for id in &later[4..] {
            rows(
                &mut reg,
                &format!("UPDATE {{source_file}} SET batch_id = {first_ingest} WHERE id = {id}"),
            );
        }

        // the next one reads the six it had not, and counts the four
        let next = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(next.parsed, 6, "{name}");
        assert_eq!(next.unchanged, 4, "{name}");
        assert_eq!(next.seen, 10, "{name}");

        // that one finished, so the one after it starts over
        let again = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(again.parsed, 10, "{name}");
        assert_eq!(again.unchanged, 0, "{name}");

        // an unfinished one with --restart beside it is started over too
        let last = again.written.as_ref().unwrap().batch_id;
        rows(
            &mut reg,
            &format!("UPDATE {{ingest_batch}} SET state = 'failed' WHERE id = {last}"),
        );
        let mut restart = s.clone();
        restart.restart = true;
        let over = digest(&restart, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(over.parsed, 10, "{name}");

        // and an ordinary digest in between ends the chain
        let over_id = over.written.as_ref().unwrap().batch_id;
        rows(
            &mut reg,
            &format!("UPDATE {{ingest_batch}} SET state = 'cancelled' WHERE id = {over_id}"),
        );
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(after.parsed, 10, "{name}");
        assert_eq!(after.unchanged, 0, "{name}");
    }
}

#[test]
fn every_mr_series_is_taken_and_one_file_of_each_is_read_a_chunk_at_a_time() {
    for lab in labs() {
        let name = lab.name;
        let dir = fleet();
        let mut reg = lab.open();
        digest(&settings(&dir), &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source = one(&mut reg, "SELECT MIN(id) FROM {source}");

        // every MR series, whatever its manufacturer; never the CT
        let every = Selection::new(&[], &[]).every(true);
        assert!(!every.is_empty());
        let series = targets(reg.store(), &every).unwrap();
        assert_eq!(
            series_uids(&mut reg, &series),
            ["1.1", "1.2", "2.1", "3.1", "4.1", "5.1"],
            "{name}"
        );
        // a missing column narrows it: the fleet's files write no
        // SamplesPerPixel, so every series lacks it until it is given one
        let missing = every.clone().missing(true);
        assert_eq!(targets(reg.store(), &missing).unwrap(), series, "{name}");
        rows(&mut reg, "UPDATE {series} SET samples_per_pixel = 1");
        assert!(targets(reg.store(), &missing).unwrap().is_empty(), "{name}");
        rows(
            &mut reg,
            "UPDATE {series} SET samples_per_pixel = NULL WHERE series_instance_uid IN ('1.1', '4.1')",
        );
        let narrowed = targets(reg.store(), &missing).unwrap();
        assert_eq!(series_uids(&mut reg, &narrowed), ["1.1", "4.1"], "{name}");

        // one file of each, two series a page: the five-file series gives
        // its lowest file and no other
        let mut pages = Pages::one_each(reg.store(), source, series.clone(), 4);
        let mut sizes = Vec::new();
        let mut seen = Vec::new();
        while let Some(page) = pages.next(reg.store()).unwrap() {
            sizes.push(page.len());
            seen.extend(page.iter().map(|f| f.id));
        }
        assert_eq!(sizes, [4, 2], "{name}");
        let expected: Vec<i64> = ints(
            &mut reg,
            "SELECT MIN(f.id) FROM {source_file} f JOIN {instance} i ON i.source_file_id = f.id \
             JOIN {series} s ON s.id = i.series_id WHERE s.modality = 'MR' \
             GROUP BY s.id ORDER BY 1",
        );
        seen.sort();
        assert_eq!(seen, expected, "{name}");

        // a series whose lowest file is no longer ingested gives its next
        let low = one(
            &mut reg,
            "SELECT MIN(f.id) FROM {source_file} f JOIN {instance} i ON i.source_file_id = f.id \
             JOIN {series} s ON s.id = i.series_id WHERE s.series_instance_uid = '1.1'",
        );
        rows(
            &mut reg,
            &format!("UPDATE {{source_file}} SET status = 'gone' WHERE id = {low}"),
        );
        let mut pages = Pages::one_each(reg.store(), source, series.clone(), 10);
        let page = pages.next(reg.store()).unwrap().unwrap();
        assert_eq!(page.len(), 6, "{name}");
        assert!(page.iter().all(|f| f.id != low), "{name}");
        assert!(pages.next(reg.store()).unwrap().is_none(), "{name}");
        // another source's run reads none of them
        let mut pages = Pages::one_each(reg.store(), source + 1000, series.clone(), 10);
        assert!(pages.next(reg.store()).unwrap().is_none(), "{name}");

        // and a chunk is one aggregate over the series' instances: on
        // SQLite the plan searches instance by its series and source_file by
        // its key, and scans neither
        if reg.store().backend() == Backend::Sqlite {
            let sql = format!("EXPLAIN QUERY PLAN {}", first_sql(reg.store(), 2));
            let plan: Vec<String> = reg
                .store()
                .query(&sql, &[Param::Int(series[0]), Param::Int(series[1])])
                .unwrap()
                .iter()
                .map(|r| r.text(3).unwrap().to_string())
                .collect();
            let text = plan.join("\n");
            assert!(
                !plan
                    .iter()
                    .any(|l| l.starts_with("SCAN i") || l.starts_with("SCAN f")),
                "{text}"
            );
        }

        // the run itself: every MR series, one file each
        let mut s = settings(&dir);
        s.reread_every = true;
        s.reread_one = true;
        let report = digest(&s, &mut reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.parsed, 6, "{name}");
    }
}
