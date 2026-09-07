// SPDX-License-Identifier: AGPL-3.0-only

//! Slice 7 (`docs/specs/wave4b-the-ask.md`, §8, §14.1, bars 5 and 6):
//! a run leaves a handle with pages and kept sets; a named handle needs
//! its ask; an upload resolves through the linkage store and leaks nothing
//! into the registry, the handle store or the explain output; a selection
//! saves, inlines and refuses a cohort's name; promotion opens intervals
//! and the cohort answers; a drifted handle is re-evaluated and an expired
//! one says so; the measures ride on the answer.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Lab, fixture, refresh, run_ask, set_param};
use nils_ask::ast::{Ask, Grain};
use nils_ask::exec::Bounds;
use nils_ask::handle::{self, HandleError, Provenance, Spec};
use nils_ask::run::{Request, RunError, explain, run};
use nils_ask::validate::{Class, Scope};
use nils_ask::{parse, promote, selection, values};
use nils_registry::linkage::{self, NewIdentity, Subkeys};
use nils_registry::session::Scheme;
use nils_registry::store::Cell;
use nils_registry::time::now_iso;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_handle_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// Run an ask through the pipeline with the test's defaults.
fn go(l: &mut Lab, ask: Ask, name: Option<&str>, keep: bool) -> nils_ask::run::Outcome {
    let scope = Scope::default();
    let scheme = Scheme::default();
    run(
        &mut l.registry,
        Request {
            ask,
            names: &l.catalog,
            scope: &scope,
            principal: "tester",
            node: "lab",
            pack_version: Some("0.0.0-test"),
            scheme: &scheme,
            bounds: bounds(),
            page_rows: 5,
            name,
            keep,
            after: None,
            limit: None,
            may_project_raw: false,
            purpose: None,
            reader: None,
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

fn text_cells(rows: &[nils_registry::store::Row], i: usize) -> Vec<String> {
    rows.iter()
        .map(|r| r.text(i).unwrap().to_string())
        .collect()
}

#[test]
fn a_run_leaves_a_handle_with_pages_and_kept_sets() {
    let mut hashes: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for mut l in labs() {
        let ask = fixture("yardstick");
        let (_, direct) = run_ask(&mut l, ask.clone());
        let out = go(&mut l, ask, Some("converters"), true);
        let h = &out.handle;
        assert_eq!(h.name.as_deref(), Some("converters"), "{}", l.name);
        assert_eq!(h.grain, Grain::Subject);
        assert_eq!(h.row_count, 14, "{}", l.name);
        assert!(!h.truncated);
        assert_eq!(h.content_hash, direct.content_hash, "{}", l.name);
        assert_eq!(
            h.ask_hash().as_deref(),
            Some(out.hash.as_str()),
            "{}",
            l.name
        );
        assert_eq!(h.principal, "tester");
        assert_eq!(h.node, "lab");
        assert_eq!(h.pack_version.as_deref(), Some("0.0.0-test"));
        assert_eq!(h.scheme_digest, Some(Scheme::default().digest()));
        assert_eq!(h.disclosure, "local");
        assert_eq!(h.columns.len(), 8, "{}: {:?}", l.name, h.columns);
        assert_eq!(h.columns[2].name, "code");
        assert_eq!(h.columns[2].type_, "text");
        assert_eq!(h.columns[5].name, "n_good");
        assert_eq!(h.columns[5].type_, "integer");
        // the pages: fourteen rows at five a page
        let store = l.registry.store();
        assert_eq!(handle::page_count(store, h.id).unwrap(), 3, "{}", l.name);
        let last = handle::page(store, h.id, 2).unwrap().unwrap();
        assert_eq!(last.len(), 4, "{}", l.name);
        assert!(handle::page(store, h.id, 3).unwrap().is_none());
        let first = handle::page(store, h.id, 0).unwrap().unwrap();
        assert_eq!(first[0].len(), 8);
        assert!(first[0][2].is_string(), "{}: {:?}", l.name, first[0]);
        // the keys, in the answer's order
        let keys = handle::keys(store, h.id).unwrap();
        assert_eq!(keys.len(), 14);
        assert!(
            keys.iter().all(|(k, s)| Some(*k) == *s),
            "{}: a subject handle keys its subjects",
            l.name
        );
        let read = handle::get(store, h.id).unwrap().unwrap();
        assert!(
            read.last_read_at.is_some(),
            "{}: a page read is recorded",
            l.name
        );
        // the kept sets, each a handle of keys named under the answer's
        let kept: BTreeMap<String, Grain> = out
            .kept
            .iter()
            .map(|k| (k.name.clone().unwrap(), k.grain))
            .collect();
        assert_eq!(
            kept,
            BTreeMap::from([
                ("converters/converted".to_string(), Grain::Subject),
                ("converters/followups".to_string(), Grain::Session),
                ("converters/good".to_string(), Grain::Session),
            ]),
            "{}",
            l.name
        );
        let good = out
            .kept
            .iter()
            .find(|k| k.name.as_deref() == Some("converters/good"))
            .unwrap();
        assert_eq!(good.row_count, 66, "{}", l.name);
        assert_eq!(good.columns.len(), 2);
        assert_eq!(handle::list(store, false).unwrap().len(), 4, "{}", l.name);
        hashes
            .entry("yardstick")
            .or_default()
            .push((l.name.to_string(), h.content_hash.clone().unwrap()));
    }
    for (_, per) in hashes {
        if per.len() == 2 {
            assert_eq!(per[0].1, per[1].1, "the two backends disagree");
        }
    }
}

#[test]
fn a_named_handle_needs_its_ask_and_a_withdrawn_one_keeps_its_record() {
    for mut l in labs() {
        let mut c = fixture("gold-c");
        set_param(&mut c, "cohorts", json!(["ms-cohort-a", "ms-cohort-b"]));
        let out = go(&mut l, c, None, false);
        let store = l.registry.store();
        let answer = out.answer.clone();
        let spec = Spec {
            name: Some("unbacked"),
            grain: Grain::Group,
            ask: None,
            params: Value::Null,
            selection_versions: Value::Null,
            values_unresolved: Value::Null,
            provenance: Provenance {
                principal: "tester",
                node: "lab",
                pack_version: None,
                epoch: 1,
                scheme_digest: None,
                disclosure: "local",
                suppression: Value::Null,
            },
            page_rows: 5,
        };
        let err = handle::save(store, &spec, &answer).unwrap_err();
        assert!(
            matches!(err, HandleError::NamedWithoutAsk),
            "{}: {err}",
            l.name
        );
        // unnamed, the same save is fine
        let spec = Spec { name: None, ..spec };
        let h = handle::save(store, &spec, &answer).unwrap();
        assert!(h.ask.is_none());
        // withdrawn: the rows go, the record stays, a second withdrawal is refused
        handle::withdraw(store, h.id, "tester", "a test").unwrap();
        let w = handle::get(store, h.id).unwrap().unwrap();
        assert_eq!(w.withdrawn_by.as_deref(), Some("tester"));
        assert!(!w.has_rows());
        assert_eq!(handle::page_count(store, h.id).unwrap(), 0);
        assert!(matches!(
            handle::withdraw(store, h.id, "tester", "again").unwrap_err(),
            HandleError::Withdrawn(_)
        ));
        assert!(
            handle::list(store, false)
                .unwrap()
                .iter()
                .all(|x| x.id != h.id)
        );
        assert!(
            handle::list(store, true)
                .unwrap()
                .iter()
                .any(|x| x.id == h.id)
        );
    }
}

/// Every text cell of every table of the registry (never the linkage
/// store), for the leak grep of bar 5.
fn registry_text(l: &mut Lab) -> String {
    let store = l.registry.store();
    let tables: Vec<String> = if l.name == "sqlite" {
        store
            .query("SELECT name FROM sqlite_master WHERE type = 'table'", &[])
            .unwrap()
            .iter()
            .map(|r| r.text(0).unwrap().to_string())
            .collect()
    } else {
        store
            .query(
                &format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = '{}'",
                    l.schema
                ),
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.text(0).unwrap().to_string())
            .collect()
    };
    assert!(
        tables.iter().any(|t| t == "handle_page"),
        "{}: {tables:?}",
        l.name
    );
    let mut out = String::new();
    for t in &tables {
        let rows = store
            .query(&format!("SELECT * FROM {}", store.qualified(t)), &[])
            .unwrap();
        for r in rows {
            for c in &r.0 {
                if let Cell::Text(s) = c {
                    out.push_str(s);
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[test]
fn an_upload_resolves_through_the_linkage_store_and_leaks_nothing() {
    for mut l in labs() {
        // five subjects get a patient id in the linkage store
        let key = l.registry.pseudonym_key().unwrap();
        let keys = Subkeys::derive(&key);
        let mut lk = l.registry.open_linkage().unwrap();
        // the seeded type
        let t = linkage::id_type_id(&mut lk, "patient-id").unwrap().unwrap();
        let rows: Vec<NewIdentity> = (1..=5)
            .map(|i| {
                let v = format!("PID-000{i}");
                NewIdentity {
                    subject_id: i,
                    id_type_id: t,
                    lookup: keys.lookup("patient-id", &v),
                    ciphertext: keys.seal(&v),
                    source: "manual",
                    first_batch_id: None,
                }
            })
            .collect();
        linkage::insert_identities(&mut lk, &rows).unwrap();
        drop(lk);
        // an upload of six, one unknown
        let listed: Vec<String> = (1..=5)
            .map(|i| format!("PID-000{i}"))
            .chain(["PID-9999".to_string()])
            .collect();
        let up = values::upload(&mut l.registry, "patient-id", &listed, "tester").unwrap();
        assert_eq!(up.n, 6);
        assert_eq!(up.unresolved, vec![5]);
        assert_eq!(up.subjects, 5);
        assert!(matches!(
            values::upload(&mut l.registry, "no-such", &listed, "tester").unwrap_err(),
            values::ValuesError::UnknownNamespace(_)
        ));
        refresh(&mut l);
        let ask = parse(&json!({
            "ast_version": 1,
            "values": {"listed": {"upload": up.upload_id}},
            "sets": {"a": {"grain": "subject", "from": "values:listed"}},
            "out": {"set": "a", "level": "record", "columns": [["field", {}, "code"]], "order": [[["field", {}, "code"], "asc"]]}
        }).to_string()).unwrap();
        let out = go(&mut l, ask.clone(), Some("listed"), false);
        assert_eq!(out.answer.rows.len(), 5, "{}", l.name);
        assert_eq!(
            out.handle.values_unresolved[&up.upload_id]["unresolved"],
            json!(1),
            "{}",
            l.name
        );
        assert_eq!(
            out.handle.values_unresolved[&up.upload_id]["sample_positions"],
            json!([5])
        );
        // saved as a selection, read back through it
        let saved = selection::save(
            &mut l.registry,
            "listed-five",
            &out.handle.ask.clone().unwrap(),
            &out.hash,
            "tester",
            Some("the five"),
            None,
        )
        .unwrap();
        assert_eq!(saved.version, 1);
        refresh(&mut l);
        let through = parse(&json!({
            "ast_version": 1,
            "sets": {"s": {"grain": "subject", "from": "selection:listed-five"}},
            "out": {"set": "s", "level": "record", "columns": [["field", {}, "code"]], "order": [[["field", {}, "code"], "asc"]]}
        }).to_string()).unwrap();
        let via = go(&mut l, through.clone(), None, false);
        assert_eq!(via.inlined.len(), 1, "{}", l.name);
        assert_eq!(via.inlined[0].version, 1);
        assert_eq!(
            text_cells(&via.answer.rows, 2),
            text_cells(&out.answer.rows, 2),
            "{}",
            l.name
        );
        assert_eq!(
            via.handle.selection_versions,
            json!([{"set": "s", "selection": "listed-five", "version": 1}]),
            "{}",
            l.name
        );
        // explain shows both texts and no identifier
        let scope = Scope::default();
        let scheme = Scheme::default();
        let ex = explain(
            &mut l.registry,
            through.clone(),
            &l.catalog,
            &scope,
            &scheme,
        )
        .unwrap();
        assert!(
            ex.sqlite.contains("values_member") && ex.postgres.contains("values_member"),
            "{}",
            l.name
        );
        assert!(
            !ex.sqlite.contains("PID-") && !ex.postgres.contains("PID-"),
            "{}",
            l.name
        );
        // the identifiers: refused without the role, projected with it, audited, never paged
        let mut ident = ask.clone();
        ident.out.identifiers = vec!["patient-id".to_string()];
        let err = go_raw(&mut l, ident.clone(), false).unwrap_err();
        assert!(matches!(err, RunError::Forbidden(_)), "{}: {err}", l.name);
        let raw = go_raw(&mut l, ident, true).unwrap();
        assert_eq!(
            raw.answer.columns.last().map(String::as_str),
            Some("patient-id")
        );
        let ids = text_cells(&raw.answer.rows, raw.answer.columns.len() - 1);
        assert_eq!(
            ids,
            (1..=5).map(|i| format!("PID-000{i}")).collect::<Vec<_>>(),
            "{}",
            l.name
        );
        assert_eq!(raw.identifiers, vec!["patient-id".to_string()]);
        let store = l.registry.store();
        let audit = store
            .query(
                &format!(
                    "SELECT principal, columns, rows FROM {}",
                    store.qualified("handle_read_audit")
                ),
                &[],
            )
            .unwrap();
        assert_eq!(audit.len(), 1, "{}", l.name);
        assert_eq!(audit[0].text(1).unwrap(), "[\"patient-id\"]");
        assert_eq!(audit[0].int(2).unwrap(), 5);
        let page = handle::page(store, raw.handle.id, 0).unwrap().unwrap();
        assert_eq!(
            page[0].len(),
            3,
            "{}: the page holds the answer before the identifiers",
            l.name
        );
        // the leak grep: no identifier anywhere in the registry
        let text = registry_text(&mut l);
        assert!(
            text.contains("listed-five"),
            "{}: the grep reads the tables",
            l.name
        );
        assert!(
            !text.contains("PID-"),
            "{}: an identifier leaked into the registry",
            l.name
        );
    }
}

fn go_raw(l: &mut Lab, ask: Ask, may: bool) -> Result<nils_ask::run::Outcome, RunError> {
    let scope = Scope {
        federated: false,
        classes: BTreeSet::from([Class::QuasiIdentifying]),
    };
    let scheme = Scheme::default();
    run(
        &mut l.registry,
        Request {
            ask,
            names: &l.catalog,
            scope: &scope,
            principal: "reader",
            node: "lab",
            pack_version: None,
            scheme: &scheme,
            bounds: bounds(),
            page_rows: 5,
            name: None,
            keep: false,
            after: None,
            limit: None,
            may_project_raw: may,
            purpose: Some("a test"),
            reader: None,
        },
    )
}

#[test]
fn promotion_opens_intervals_and_the_cohort_answers() {
    for mut l in labs() {
        let ask = fixture("yardstick");
        let out = go(&mut l, ask.clone(), None, false);
        let before = l.registry.meta().epoch;
        // a selection under the cohort's future name, then the promotion links them
        let saved = selection::save(
            &mut l.registry,
            "converters",
            &out.handle.ask.clone().unwrap(),
            &out.hash,
            "tester",
            None,
            None,
        )
        .unwrap();
        assert_eq!(saved.version, 1);
        assert!(
            l.registry.meta().epoch > before,
            "{}: saving a selection advances the epoch",
            l.name
        );
        let no = promote::promote(
            &mut l.registry,
            out.handle.id,
            "converters",
            "tester",
            None,
            false,
        )
        .unwrap_err();
        assert!(
            matches!(no, promote::PromoteError::NoSuchCohort(_)),
            "{}: {no}",
            l.name
        );
        let p = promote::promote(
            &mut l.registry,
            out.handle.id,
            "converters",
            "tester",
            Some("the study's converters"),
            true,
        )
        .unwrap();
        assert!(p.created);
        assert_eq!((p.added, p.already), (14, 0), "{}", l.name);
        assert_eq!(p.selection, Some(("converters".to_string(), 1)));
        assert!(!p.source_moved);
        assert_eq!(p.ask_hash.as_deref(), Some(out.hash.as_str()));
        let again = promote::promote(
            &mut l.registry,
            out.handle.id,
            "converters",
            "tester",
            None,
            false,
        )
        .unwrap();
        assert_eq!((again.added, again.already), (0, 14), "{}", l.name);
        // the intervals carry their provenance
        let store = l.registry.store();
        let rows = store
            .query(
                &format!(
                    "SELECT source, handle_id, epoch, ask_hash, selection_version, reason FROM {} WHERE source = 'promotion' ORDER BY subject_id",
                    store.qualified("cohort_member")
                ),
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 14, "{}", l.name);
        assert_eq!(rows[0].int(1).unwrap(), out.handle.id);
        assert_eq!(rows[0].int(2).unwrap(), out.handle.epoch);
        assert_eq!(rows[0].text(3).unwrap(), out.hash);
        assert_eq!(rows[0].int(4).unwrap(), 1);
        assert_eq!(rows[0].text(5).unwrap(), "the study's converters");
        // the selection now belongs to the cohort, so its name may be saved again
        let linked = selection::get(store, "converters", None).unwrap().unwrap();
        assert_eq!(linked.cohort_id, Some(p.cohort_id));
        // a cohort's name is refused to any other selection
        let err = selection::save(
            &mut l.registry,
            "ms-cohort-a",
            &ask,
            "x",
            "tester",
            None,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, selection::SelectionError::NameIsACohort(_)),
            "{}: {err}",
            l.name
        );
        // the handle is pinned by the cohort
        assert_eq!(
            handle::pinned_by(l.registry.store(), out.handle.id).unwrap(),
            vec!["cohort converters".to_string()]
        );
        // the cohort answers through the fact, open intervals only
        refresh(&mut l);
        let (_, a) = run_ask(&mut l, parse(&json!({
            "ast_version": 1,
            "sets": {
                "c": {"grain": "cohort", "where": [["=", {}, ["field", {}, "name"], "converters"]]},
                "p": {"grain": "subject", "of": "c"}
            },
            "out": {"set": "p", "level": "count"}
        }).to_string()).unwrap());
        assert_eq!(a.rows[0].int(1).unwrap(), 14, "{}", l.name);
        // the source moved: a second version of the selection with another
        // structure (a parameter's value is not part of the hash), and the
        // tool says so
        let mut edited = ask.clone();
        edited.sets.get_mut("converted").unwrap().bind.0[0]
            .1
            .opts
            .insert("adjacent".into(), Value::Bool(false));
        let out2 = go(&mut l, edited, None, false);
        let v2 = selection::save(
            &mut l.registry,
            "converters",
            &out2.handle.ask.clone().unwrap(),
            &out2.hash,
            "tester",
            Some("looser"),
            None,
        )
        .unwrap();
        assert_eq!(v2.version, 2);
        let moved = promote::promote(
            &mut l.registry,
            out.handle.id,
            "converters",
            "tester",
            None,
            false,
        )
        .unwrap();
        assert!(moved.source_moved, "{}", l.name);
        assert_eq!(
            selection::list(l.registry.store())
                .unwrap()
                .iter()
                .filter(|s| s.name == "converters")
                .count(),
            1
        );
    }
}

#[test]
fn a_drifted_handle_is_re_evaluated_and_an_expired_one_says_so() {
    for mut l in labs() {
        let out = go(&mut l, fixture("yardstick"), None, false);
        let id = out.handle.id;
        refresh(&mut l);
        let reads = |pin: bool| {
            parse(&json!({
                "ast_version": 1,
                "sets": {"a": {"grain": "subject", "from": if pin { json!({"handle": id.to_string(), "pin": true}) } else { json!(format!("handle:{id}")) }}},
                "out": {"set": "a", "level": "count"}
            }).to_string()).unwrap()
        };
        // the same epoch: the stored keys join, no drift
        let same = go(&mut l, reads(false), None, false);
        assert!(same.drift.is_empty(), "{}", l.name);
        assert_eq!(same.answer.rows[0].int(1).unwrap(), 14);
        // the epoch moves: a judgement changing act
        selection::save(
            &mut l.registry,
            "bump",
            &out.handle.ask.clone().unwrap(),
            &out.hash,
            "tester",
            None,
            None,
        )
        .unwrap();
        let drifted = go(&mut l, reads(false), None, false);
        assert_eq!(drifted.drift.len(), 1, "{}", l.name);
        let d = &drifted.drift[0];
        assert_eq!(
            (d.handle, d.added, d.removed, d.expired),
            (id, 0, 0, false),
            "{}",
            l.name
        );
        assert!(d.epoch_now > d.epoch_then);
        assert_ne!(d.replaced_by, id);
        assert_eq!(drifted.answer.rows[0].int(1).unwrap(), 14);
        let pinned = go(&mut l, reads(true), None, false);
        assert!(
            pinned.drift.is_empty(),
            "{}: a pinned handle joins its stored keys",
            l.name
        );
        // expiry: unread for ninety days, nothing names it
        let store = l.registry.store();
        let pruned = handle::prune(store, &now_iso(), 0).unwrap();
        assert!(pruned.dropped.contains(&id), "{}: {pruned:?}", l.name);
        assert!(pruned.pinned.is_empty());
        assert!(!handle::get(store, id).unwrap().unwrap().has_rows());
        let expired = go(&mut l, reads(false), None, false);
        assert_eq!(expired.drift.len(), 1);
        assert!(expired.drift[0].expired, "{}", l.name);
        assert_eq!(expired.answer.rows[0].int(1).unwrap(), 14);
        let scope = Scope::default();
        let scheme = Scheme::default();
        let err = run(
            &mut l.registry,
            Request {
                ask: reads(true),
                names: &l.catalog,
                scope: &scope,
                principal: "tester",
                node: "lab",
                pack_version: None,
                scheme: &scheme,
                bounds: bounds(),
                page_rows: 5,
                name: None,
                keep: false,
                after: None,
                limit: None,
                may_project_raw: false,
                purpose: None,
                reader: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, RunError::Handle(HandleError::Expired(_))),
            "{}: {err}",
            l.name
        );
        // a prune spares what a cohort names
        let fresh = go(&mut l, fixture("yardstick"), None, false);
        promote::promote(
            &mut l.registry,
            fresh.handle.id,
            "kept",
            "tester",
            None,
            true,
        )
        .unwrap();
        let pruned = handle::prune(l.registry.store(), &now_iso(), 0).unwrap();
        assert!(
            pruned
                .pinned
                .iter()
                .any(|(h, why)| *h == fresh.handle.id && why == &vec!["cohort kept".to_string()]),
            "{}: {pruned:?}",
            l.name
        );
    }
}

#[test]
fn the_measures_ride_on_the_answer() {
    let mut seen: Vec<(String, Vec<String>, BTreeMap<String, f64>)> = Vec::new();
    for mut l in labs() {
        let mut ask = fixture("gold-c");
        set_param(&mut ask, "cohorts", json!(["ms-cohort-a", "ms-cohort-b"]));
        let out = go(&mut l, ask, None, false);
        // the share is a column; the stddev of a child's binding is one
        // per group, so it is a column as well and no scalar
        assert_eq!(
            out.measured.columns,
            vec!["stddev.age".to_string(), "share._subjects".to_string()],
            "{}",
            l.name
        );
        assert!(
            out.measured.scalars.is_empty(),
            "{}: {:?}",
            l.name,
            out.measured.scalars
        );
        let n = out.answer.columns.len();
        assert_eq!(
            &out.answer.columns[n - 2..],
            ["stddev.age", "share._subjects"]
        );
        let shares: Vec<String> = out
            .answer
            .rows
            .iter()
            .map(|r| {
                format!(
                    "{} {}",
                    nils_ask::exec::render(&r.0[n - 2]),
                    nils_ask::exec::render(&r.0[n - 1])
                )
            })
            .collect();
        let mut total = 0.0;
        for r in &out.answer.rows {
            let s = r.double(n - 1).unwrap();
            assert!((0.0..=1.0).contains(&s), "{}: {s}", l.name);
            total += s;
            let sd = r.double(n - 2).unwrap();
            assert!(sd > 0.0 && sd < 30.0, "{}: {sd}", l.name);
        }
        assert!(
            total > 1.0,
            "{}: the two cohorts overlap, so the shares exceed one",
            l.name
        );
        // the handle stores the answer before the post pass
        assert_eq!(out.handle.columns.len(), 9, "{}", l.name);
        seen.push((l.name.to_string(), shares, out.measured.scalars.clone()));
    }
    if seen.len() == 2 {
        assert_eq!(seen[0].1, seen[1].1, "the shares differ between backends");
        assert_eq!(seen[0].2, seen[1].2, "the scalars differ between backends");
    }
}
