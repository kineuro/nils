// SPDX-License-Identifier: AGPL-3.0-only

//! A statement slower than the time it is given (2026-11-13), on both
//! backends: an answer's time stops it and a job's bounds run it whole; a
//! job's answer is not cut at an answer's row cap; a draft whose counts run
//! out of time is still valid, stored and answered by its handle, with what
//! was not counted said and the job path named; and a diagnosis's time is
//! for all its counts together.

mod common;

use std::time::{Duration, Instant};

use common::Lab;
use nils_ask::affordance::{self, AffordanceError, Setting};
use nils_ask::diagnose::{self, PARTS};
use nils_ask::document;
use nils_ask::exec::{Bounds, ExecError};
use nils_ask::parse;
use nils_ask::run::{self, Request, RunError};
use nils_ask::validate::Scope;
use nils_catalog::Catalog;
use nils_registry::dialect::Dialect;
use nils_registry::session::Scheme;

fn labs() -> Vec<Lab> {
    common::labs("nils_slow_test")
}

/// The caps of the bounded path, with the time a test names.
fn answer_bounds(timeout_ms: u64) -> Bounds {
    Bounds {
        timeout_ms,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// A job's bounds: no row or byte cap, and far more time than the slow
/// statement takes.
fn job_bounds() -> Bounds {
    Bounds {
        timeout_ms: 10 * 60 * 1000,
        max_rows: u64::MAX,
        max_bytes: u64::MAX,
    }
}

/// Every read of the clinical events made slow, on a registry of 48
/// subjects where each statement answers in milliseconds: the table is put
/// aside under another name and a view takes its place that works for a
/// while before its first row, a count to some millions on SQLite, which
/// has no sleep, and a sleep on Postgres. A statement that reads events is
/// then far slower than the 200 to 300 ms the tests give it, and one that
/// reads none is as fast as before.
fn slow_events(l: &mut Lab) {
    let store = l.registry.store();
    let event = store.qualified("event");
    let kept = store.qualified("event_kept");
    let work = match store.dialect() {
        Dialect::Sqlite => {
            "(WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 6000000) \
             SELECT count(*) FROM c) > 0"
        }
        Dialect::Postgres => "(SELECT 1 FROM pg_sleep(1.5)) = 1",
    };
    store
        .batch(&format!(
            "ALTER TABLE {event} RENAME TO event_kept; \
             CREATE VIEW {event} AS SELECT e.* FROM {kept} e WHERE {work}"
        ))
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
}

/// How many clinical events the registry holds.
const EVENTS: &str = r#"{"ast_version": 1, "sets": {"e": {"grain": "event"}},
    "out": {"set": "e", "level": "count"}}"#;

/// The events of the subjects: a set that reads no event, then one that
/// does.
const PEOPLE_THEN_EVENTS: &str = r#"{"ast_version": 1, "sets": {
        "people": {"grain": "subject"},
        "e": {"grain": "event", "of": "people"}},
    "out": {"set": "e", "level": "count"}}"#;

/// Every stack, one row each.
const STACKS: &str = r#"{"ast_version": 1, "sets": {"s": {"grain": "stack"}},
    "out": {"set": "s", "level": "record"}}"#;

fn request<'a>(
    catalog: &'a Catalog,
    text: &str,
    scope: &'a Scope,
    scheme: &'a Scheme,
    bounds: Bounds,
) -> Request<'a> {
    Request {
        ask: parse(text).unwrap(),
        names: catalog,
        scope,
        principal: "author",
        node: "test",
        pack_version: None,
        scheme,
        bounds,
        page_rows: 200,
        name: None,
        keep: false,
        after: None,
        limit: None,
        may_project_raw: false,
        purpose: None,
        reader: None,
    }
}

#[test]
fn an_answer_s_time_stops_a_slow_statement_and_a_job_s_bounds_run_it_whole() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        // a job's answer is whole where an answer's row cap cuts it: a cut
        // answer has no hash, and is not exported, pinned or promoted
        let capped = Bounds {
            max_rows: 3,
            ..answer_bounds(20_000)
        };
        let cut = run::run(
            &mut l.registry,
            request(&l.catalog, STACKS, &scope, &scheme, capped),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(cut.answer.truncated, "{}", l.name);
        assert_eq!(cut.answer.rows.len(), 3, "{}", l.name);
        assert!(cut.handle.content_hash.is_none(), "{}", l.name);
        let whole = run::run(
            &mut l.registry,
            request(&l.catalog, STACKS, &scope, &scheme, job_bounds()),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(!whole.answer.truncated, "{}", l.name);
        assert!(whole.answer.rows.len() > 3, "{}", l.name);
        assert!(whole.handle.content_hash.is_some(), "{}", l.name);

        slow_events(&mut l);
        // an answer's time, which the statement runs past: it is stopped,
        // and the words say so without sending a job to be queued as a job
        let stopped = run::run(
            &mut l.registry,
            request(&l.catalog, EVENTS, &scope, &scheme, answer_bounds(200)),
        );
        let Err(RunError::Exec(ExecError::Timeout(200))) = stopped else {
            panic!("{}: {:?}", l.name, stopped.map(|o| o.answer.rows));
        };
        let words = ExecError::Timeout(200).to_string();
        assert!(words.contains("ran past 200 ms"), "{words}");
        assert!(!words.contains("queue it as a job"), "{words}");
        // the same statement under a job's bounds answers
        let answered = run::run(
            &mut l.registry,
            request(&l.catalog, EVENTS, &scope, &scheme, job_bounds()),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(answered.answer.rows.len(), 1, "{}", l.name);
        assert!(
            answered.answer.rows[0].int(0).unwrap() > 0,
            "{}: the registry holds events",
            l.name
        );
        assert!(answered.handle.content_hash.is_some(), "{}", l.name);
    }
}

#[test]
fn a_draft_whose_counts_run_out_of_time_is_valid_stored_and_says_so() {
    for mut l in labs() {
        slow_events(&mut l);
        let scope = Scope::default();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: answer_bounds(20_000),
            values_cap: 50,
        };
        let started = Instant::now();
        let drafted =
            affordance::draft_within(&mut l.registry, PEOPLE_THEN_EVENTS, &s, None, Some(300))
                .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        // within the draft's time and a little, never a statement's own
        // time once for each stage
        assert!(
            started.elapsed() < Duration::from_millis(s.bounds.timeout_ms),
            "{}: {:?}",
            l.name,
            started.elapsed()
        );
        // valid, with its cost class, stored and answered by its handle
        let d = &drafted.diagnosis;
        assert!(d.valid && d.issues.is_empty(), "{}: {:?}", l.name, d.issues);
        assert_eq!(d.cost.class, "small", "{}", l.name);
        let id = drafted
            .document
            .unwrap_or_else(|| panic!("{}: a valid draft is stored", l.name));
        assert!(drafted.hash.is_some(), "{}", l.name);
        let stored = document::get(l.registry.store(), id).unwrap().unwrap();
        assert_eq!(Some(&stored.hash), drafted.hash.as_ref(), "{}", l.name);
        // what was not counted, why, and the next call by the document's id
        let u = d
            .unavailable
            .as_ref()
            .unwrap_or_else(|| panic!("{}: the counts ran out of time", l.name));
        assert_eq!(u.code, "timeout", "{}", l.name);
        assert_eq!(u.budget_ms, 300, "{}", l.name);
        assert_eq!(u.parts, PARTS, "{}", l.name);
        assert!(u.why.contains("ran past 300 ms"), "{}", u.why);
        assert_eq!(
            d.next,
            vec![diagnose::job_next(Some(id))],
            "{}: the job path, by the document's id",
            l.name
        );
        assert!(
            d.next[0].contains(&format!("document_id {id}")),
            "{:?}",
            d.next
        );
        // the funnel keeps the stages counted before the time ran out, and
        // nothing after: the subjects were counted, the events were not
        assert!(
            d.funnel
                .iter()
                .any(|st| st.set == "people" && st.rows == Some(48)),
            "{}: {:?}",
            l.name,
            d.funnel
        );
        assert!(
            d.funnel.iter().all(|st| st.set != "e"),
            "{}: {:?}",
            l.name,
            d.funnel
        );
        assert!(
            d.drops.is_empty() && d.ties.is_empty() && d.coarse.is_empty(),
            "{}: a part cut by the time is left empty",
            l.name
        );
        assert!(d.zero_rows.is_none(), "{}: {:?}", l.name, d.zero_rows);
        // in the answer as a caller reads it
        let v = serde_json::to_value(&drafted).unwrap();
        assert_eq!(v["document"], id, "{v}");
        assert_eq!(v["diagnosis"]["valid"], true, "{v}");
        assert_eq!(v["diagnosis"]["unavailable"]["code"], "timeout", "{v}");

        // a document that is wrong is still refused, at once and unstored:
        // slow and wrong are never mistaken for one another
        let wrong = r#"{"ast_version": 1, "sets": {"e": {"grain": "event", "from": "nowhere"}},
            "out": {"set": "e", "level": "count"}}"#;
        let refused = affordance::draft_within(&mut l.registry, wrong, &s, None, Some(300))
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(!refused.diagnosis.valid, "{}", l.name);
        assert!(!refused.diagnosis.issues.is_empty(), "{}", l.name);
        assert!(refused.diagnosis.unavailable.is_none(), "{}", l.name);
        assert!(refused.document.is_none() && refused.hash.is_none());

        // a document that reads no event is counted whole in the same time
        let quick = r#"{"ast_version": 1, "sets": {"people": {"grain": "subject"}},
            "out": {"set": "people", "level": "count"}}"#;
        let counted = affordance::draft_within(&mut l.registry, quick, &s, None, Some(20_000))
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(counted.diagnosis.unavailable.is_none(), "{}", l.name);
        assert!(counted.diagnosis.next.is_empty(), "{}", l.name);
        assert!(counted.document.is_some(), "{}", l.name);
        assert_eq!(
            serde_json::to_value(&counted).unwrap()["diagnosis"].get("unavailable"),
            None,
            "{}: absent where everything was counted",
            l.name
        );

        // without a time for the whole, a statement past its own time is the
        // error it was, for the callers that have nothing to answer without
        // the count
        let quickly = Setting {
            bounds: answer_bounds(200),
            ..s
        };
        let failed = affordance::draft(&mut l.registry, EVENTS, &quickly, None);
        assert!(
            matches!(
                failed,
                Err(AffordanceError::Run(RunError::Exec(ExecError::Timeout(
                    200
                ))))
            ),
            "{}: {:?}",
            l.name,
            failed.map(|d| d.document)
        );
    }
}

#[test]
fn a_diagnosis_s_time_is_for_all_its_counts_together() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        let ask = parse(PEOPLE_THEN_EVENTS).unwrap();
        // counted whole where the time is enough, as without one
        let whole = diagnose::diagnose_within(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            answer_bounds(20_000),
            false,
            None,
            Some(20_000),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let plain = diagnose::diagnose(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            answer_bounds(20_000),
            false,
            None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(whole.unavailable.is_none(), "{}", l.name);
        assert_eq!(whole.funnel, plain.funnel, "{}", l.name);
        assert!(
            whole.funnel.iter().any(|st| st.set == "e"),
            "{}: {:?}",
            l.name,
            whole.funnel
        );
        // no time at all: nothing is counted and nothing fails, the
        // document is valid and every part is named as not counted
        let none = diagnose::diagnose_within(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            answer_bounds(20_000),
            false,
            None,
            Some(0),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(none.valid, "{}", l.name);
        assert!(none.funnel.is_empty(), "{}: {:?}", l.name, none.funnel);
        assert_eq!(none.cost, plain.cost, "{}", l.name);
        let u = none.unavailable.as_ref().unwrap();
        assert_eq!((u.code.as_str(), u.budget_ms), ("timeout", 0), "{}", l.name);
        assert_eq!(u.parts, PARTS, "{}", l.name);
        assert_eq!(none.next, vec![diagnose::job_next(None)], "{}", l.name);
        // and the same diagnosis of a stored document names it
        assert_eq!(
            none.of_stored(7).next,
            vec![diagnose::job_next(Some(7))],
            "{}",
            l.name
        );
        // each statement runs under what is left of the time, so the whole
        // is stopped once, not once for each stage
        slow_events(&mut l);
        let started = Instant::now();
        let cut = diagnose::diagnose_within(
            &mut l.registry,
            ask,
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            answer_bounds(20_000),
            false,
            None,
            Some(250),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{}: {:?}",
            l.name,
            started.elapsed()
        );
        assert!(cut.valid, "{}", l.name);
        assert_eq!(
            cut.funnel
                .iter()
                .map(|st| st.set.as_str())
                .collect::<Vec<_>>(),
            vec!["people"],
            "{}",
            l.name
        );
        assert_eq!(cut.unavailable.unwrap().parts, PARTS, "{}", l.name);
    }
}
