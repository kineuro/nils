// SPDX-License-Identifier: AGPL-3.0-only

//! Fixture A (`docs/specs/wave4b-the-ask.md`, §13.3), the part of it slice
//! 5 owns: one statement per ask, executed on both backends inside a read
//! transaction, with agreeing content hashes. The rows that need `near`,
//! the sequences and the derived fields come with slice 6.

use std::collections::BTreeMap;
use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_ask::compile::{Compiled, Context, compile};
use nils_ask::exec::{Answer, Bounds, run};
use nils_ask::validate::Scope;
use nils_ask::{parse, prepare};
use nils_catalog::Catalog;
use nils_dicom::synth::TempDir;
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::table;
use nils_registry::session::Scheme;
use nils_registry::{Backend, Insert, Param, Registry};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_compile_test";

struct Lab {
    name: &'static str,
    registry: Registry,
    catalog: Catalog,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if self._guard.is_some() {
            self.registry
                .store()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .ok();
        }
    }
}

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("compile-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-compile-test-key").unwrap();
    let mut registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: nils_registry::Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    nils_synth::build(
        &mut registry,
        &nils_synth::Plan {
            seed: 11,
            subjects: 48,
        },
    )
    .unwrap();
    // the sessions, under the default scheme
    let scheme = Scheme::default();
    let anchors = nils_session::Anchors::resolve(&mut registry, &scheme, BTreeMap::new()).unwrap();
    nils_session::ensure(&mut registry, &scheme, &anchors, None, false).unwrap();
    // an uploaded list of 600 subjects, already resolved (slice 7 does the
    // resolving; the rows are what the compiler joins)
    {
        let store = registry.store();
        let rows = store
            .insert(
                &Insert::new(
                    table("values_source"),
                    &[
                        "upload_id",
                        "namespace",
                        "digest",
                        "n",
                        "unresolved",
                        "principal",
                        "created_at",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("u-600"),
                    Param::from("patient-id"),
                    Param::from("d"),
                    Param::Int(600),
                    Param::Int(0),
                    Param::from("test"),
                    Param::from("2026-09-07T00:00:00Z"),
                ]],
            )
            .unwrap();
        let source = rows[0].int(0).unwrap();
        let members: Vec<Vec<Param>> = (0..600)
            .map(|i| vec![Param::Int(source), Param::Int(i), Param::Int(1 + (i % 48))])
            .collect();
        store
            .insert(
                &Insert::new(
                    table("values_member"),
                    &["source_id", "position", "subject_id"],
                ),
                &members,
            )
            .unwrap();
    }
    let pack = nils_pack::load(&root().join("packs/mri"), None).unwrap();
    let catalog = Catalog::build(&mut registry, &pack).unwrap();
    Lab {
        name,
        registry,
        catalog,
        _dir: dir,
        _guard: None,
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => {
            let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
            let mut l = lab("postgres", Backend::Postgres, Some(dsn));
            l._guard = Some(guard);
            out.push(l);
        }
        _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped"),
    }
    out
}

fn ask_of(l: &mut Lab, text: &str) -> (Compiled, Answer) {
    let ask = parse(text).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let prepared =
        prepare(ask, &l.catalog, &Scope::default()).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let store = l.registry.store();
    let ctx = Context {
        names: &l.catalog,
        dialect: store.dialect(),
        schema: store.schema().map(str::to_string),
        window_days: 0,
        scheme_digest: Scheme::default().digest(),
        after: None,
        limit: None,
    };
    let compiled = compile(&prepared.ask, &prepared.validated, &ctx)
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let answer = run(
        store,
        &compiled,
        Bounds {
            timeout_ms: 20_000,
            max_rows: 5_000,
            max_bytes: 4 * 1024 * 1024,
        },
    )
    .unwrap_or_else(|e| panic!("{}: {e}\n{}", l.name, compiled.sql));
    (compiled, answer)
}

/// Every fixture row runs on every backend; the answers' hashes agree.
#[test]
fn fixture_a_executes_on_both_backends_with_agreeing_hashes() {
    let rows: Vec<(&str, String)> = vec![
        ("a rounded group key and a projected rounded value", json!({
            "ast_version": 1,
            "sets": {
                "t": {"grain": "stack", "where": [["=", {}, ["axis", {}, "base"], "T1w"]],
                      "bind": {"te": ["round", {"key": true}, ["field", {}, "echo_time"], 1]}},
                "g": {"grain": "group", "group": {"of": "t", "by": [["field", {}, "te"]]},
                      "bind": {"n": ["count", {"set": "t"}], "mean_tr": ["avg", {"set": "t"}, ["field", {}, "repetition_time"]],
                               "shown": ["round", {}, ["field", {}, "mean_tr"], 1]}}
            },
            "out": {"set": "g", "level": "aggregate", "columns": [["field", {}, "te"], ["field", {}, "n"], ["field", {}, "shown"]]}
        }).to_string()),
        ("AVG and COUNT projected over a subject's stacks", json!({
            "ast_version": 1,
            "sets": {
                "p": {"grain": "subject"},
                "t": {"grain": "stack", "of": "p", "where": [["=", {}, ["axis", {}, "disposition"], "acquisition"]]},
                "a": {"grain": "subject", "from": "p",
                      "bind": {"n": ["count", {"set": "t"}], "mean_te": ["avg", {"set": "t"}, ["field", {}, "echo_time"]]},
                      "where": [[">", {}, ["field", {}, "n"], 0]]}
            },
            "out": {"set": "a", "level": "record", "columns": [["field", {}, "code"], ["field", {}, "n"], ["field", {}, "mean_te"]],
                    "order": [[["field", {}, "code"], "asc"]]}
        }).to_string()),
        ("a key list over 500 keys", json!({
            "ast_version": 1,
            "values": {"listed": {"upload": "u-600"}},
            "sets": {"a": {"grain": "subject", "from": "values:listed"}},
            "out": {"set": "a", "level": "count"}
        }).to_string()),
        ("a sorted list", json!({
            "ast_version": 1,
            "sets": {
                "t": {"grain": "stack"},
                "g": {"grain": "group", "group": {"of": "t", "by": [["field", {}, "subject.id"]]},
                      "bind": {"techniques": ["list", {"set": "t"}, ["field", {}, "text_sequence_name"]]}}
            },
            "out": {"set": "g", "level": "aggregate", "columns": [["field", {}, "subject.id"], ["field", {}, "techniques"]], "limit": 20}
        }).to_string()),
        ("contains with a lowercase pattern against uppercase rows", json!({
            "ast_version": 1,
            "sets": {"t": {"grain": "stack", "where": [["contains", {}, ["field", {}, "text_series_description"], "MPRAGE"]]}},
            "out": {"set": "t", "level": "count"}
        }).to_string()),
        ("a pick whose tie falls to the key", json!({
            "ast_version": 1,
            "sets": {
                "s": {"grain": "session"},
                "t": {"grain": "stack", "of": "s", "where": [["=", {}, ["axis", {}, "base"], "T1w"]],
                      "pick": {"per": "session", "by": [[["field", {}, "stack_index"], "asc"]], "ties": "report"}}
            },
            "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"], ["field", {}, "pick.tied"], ["field", {}, "pick.candidates"]],
                    "order": [[["field", {}, "id"], "asc"]], "limit": 50}
        }).to_string()),
        ("a NULL in the field a pick orders by", json!({
            "ast_version": 1,
            "sets": {
                "s": {"grain": "session"},
                "t": {"grain": "stack", "of": "s",
                      "pick": {"per": "session", "by": [[["field", {}, "inversion_time"], "desc"]]}}
            },
            "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"], ["field", {}, "inversion_time"]],
                    "order": [[["field", {}, "inversion_time"], "asc"], [["field", {}, "id"], "asc"]], "limit": 40}
        }).to_string()),
        ("a coarse date under the rows of section 5.3", json!({
            "ast_version": 1,
            "params": {"cut": {"type": "date", "value": "2012-06-15"}},
            "sets": {
                "e": {"grain": "event", "where": [["=", {}, ["field", {}, "kind"], "SP Transition"]]},
                "after_forgiving": {"grain": "event", "from": "e", "where": [[">", {}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "after_strict": {"grain": "event", "from": "e", "where": [[">", {"strict": true}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "on_forgiving": {"grain": "event", "from": "e", "where": [["=", {}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "on_strict": {"grain": "event", "from": "e", "where": [["=", {"strict": true}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "all": {"grain": "event", "from": "e", "bind": {"y": ["part", {"unit": "year"}, ["field", {}, "date"]],
                                                                  "gap": ["days_between", {}, ["field", {}, "date"], ["param", {}, "cut"]]}}
            },
            "keep": ["after_forgiving", "after_strict", "on_forgiving", "on_strict"],
            "out": {"set": "all", "level": "record", "columns": [["field", {}, "date"], ["field", {}, "precision"], ["field", {}, "y"], ["field", {}, "gap"]],
                    "order": [[["field", {}, "date"], "asc"]]}
        }).to_string()),
        ("an ask at the cohort edge with age and the count level", json!({
            "ast_version": 1,
            "params": {"as_of": {"type": "date", "value": "2026-01-01"}, "cohorts": {"type": "list", "value": ["ms-cohort-a"]}},
            "sets": {
                "scope": {"grain": "cohort", "where": [["in", {}, ["field", {}, "name"], ["param", {}, "cohorts"]]]},
                "people": {"grain": "subject", "of": "scope",
                           "bind": {"age": ["age_at", {}, ["field", {}, "birth_date"], ["param", {}, "as_of"]]},
                           "where": [[">=", {}, ["field", {}, "age"], 50]]},
                "visits": {"grain": "session", "of": "people"},
                "active": {"grain": "subject", "from": "people", "has": [{"set": "visits", "min": 2, "as": "n_visits"}]}
            },
            "out": {"set": "active", "level": "count"}
        }).to_string()),
    ];
    let mut hashes: BTreeMap<&str, Vec<(String, String, usize, Vec<String>)>> = BTreeMap::new();
    for mut l in labs() {
        for (what, text) in &rows {
            let (_, answer) = ask_of(&mut l, text);
            assert!(!answer.truncated, "{}: {what}", l.name);
            let hash = answer
                .content_hash
                .clone()
                .expect("a complete answer hashes");
            let rendered: Vec<String> = answer
                .rows
                .iter()
                .take(6)
                .map(|r| {
                    r.0.iter()
                        .map(nils_ask::exec::render)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect();
            hashes.entry(what).or_default().push((
                l.name.to_string(),
                hash,
                answer.rows.len(),
                rendered,
            ));
        }
    }
    for (what, per_backend) in &hashes {
        if per_backend.len() == 2 {
            assert_eq!(
                per_backend[0].1,
                per_backend[1].1,
                "{what}: the two backends disagree ({} vs {} rows)\n{}:\n{}\n{}:\n{}",
                per_backend[0].2,
                per_backend[1].2,
                per_backend[0].0,
                per_backend[0].3.join("\n"),
                per_backend[1].0,
                per_backend[1].3.join("\n")
            );
        }
        assert!(
            per_backend[0].2 > 0,
            "{what}: no rows on {}",
            per_backend[0].0
        );
    }
}

#[test]
fn the_rows_say_what_the_fixture_planted() {
    for mut l in labs() {
        // the key list: 600 rows over 48 subjects resolve to 48 subjects
        let (_, a) = ask_of(
            &mut l,
            &json!({
                "ast_version": 1,
                "values": {"listed": {"upload": "u-600"}},
                "sets": {"a": {"grain": "subject", "from": "values:listed"}},
                "out": {"set": "a", "level": "count"}
            })
            .to_string(),
        );
        assert_eq!(a.columns, vec!["rows", "subjects"]);
        assert_eq!(a.rows[0].int(1).unwrap(), 48, "{}", l.name);
        // contains is case blind on both backends
        let (_, a) = ask_of(&mut l, &json!({
            "ast_version": 1,
            "sets": {"t": {"grain": "stack", "where": [["contains", {}, ["field", {}, "text_series_description"], "MPRAGE"]]}},
            "out": {"set": "t", "level": "count"}
        }).to_string());
        assert!(a.rows[0].int(0).unwrap() > 0, "{}", l.name);
        // the coarse date: a transition known to its year, cut at midyear of
        // the earliest transition's year
        let year: String = {
            let store = l.registry.store();
            let d = store.dialect();
            let date = d.text_of(table("event").column("event_date").unwrap());
            let sql = format!(
                "SELECT MIN({date}) FROM {} WHERE event_date_precision = 'year'",
                store.qualified("event")
            );
            store.query(&sql, &[]).unwrap()[0].text(0).unwrap()[..4].to_string()
        };
        let cut = format!("{year}-06-15");
        let first_day = format!("{year}-01-01");
        let (_, a) = ask_of(&mut l, &json!({
            "ast_version": 1,
            "params": {"cut": {"type": "date", "value": cut}},
            "sets": {
                "e": {"grain": "event", "where": [["=", {}, ["field", {}, "kind"], "SP Transition"]]},
                "f": {"grain": "event", "from": "e", "where": [[">", {}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "s": {"grain": "event", "from": "e", "where": [[">", {"strict": true}, ["field", {}, "date"], ["param", {}, "cut"]]]},
                "forgiving": {"grain": "event", "from": "f", "bind": {"n_strict": ["count", {"set": "s"}]}}
            },
            "out": {"set": "forgiving", "level": "record", "columns": [["field", {}, "date"], ["field", {}, "precision"]], "order": [[["field", {}, "date"], "asc"]]}
        }).to_string());
        let dates: Vec<String> = a
            .rows
            .iter()
            .map(|r| r.text(2).unwrap().to_string())
            .collect();
        // a transition in 2012, known to the year, could be after 15 June 2012
        assert!(
            dates.iter().any(|d| d == &first_day),
            "{}: {dates:?}",
            l.name
        );
        let (_, strict) = ask_of(&mut l, &json!({
            "ast_version": 1,
            "params": {"cut": {"type": "date", "value": cut}},
            "sets": {
                "e": {"grain": "event", "where": [["=", {}, ["field", {}, "kind"], "SP Transition"]]},
                "s": {"grain": "event", "from": "e", "where": [[">", {"strict": true}, ["field", {}, "date"], ["param", {}, "cut"]]]}
            },
            "out": {"set": "s", "level": "record", "columns": [["field", {}, "date"]], "order": [[["field", {}, "date"], "asc"]]}
        }).to_string());
        let strict_dates: Vec<String> = strict
            .rows
            .iter()
            .map(|r| r.text(2).unwrap().to_string())
            .collect();
        // under strict, 2012 is not certainly after the cut
        assert!(
            !strict_dates.iter().any(|d| d == &first_day),
            "{}: {strict_dates:?}",
            l.name
        );
        assert!(strict_dates.len() < dates.len(), "{}", l.name);
        // the pick: every session gets one row, ties reported, never settled by text
        let (pick_sql, p) = ask_of(&mut l, &json!({
            "ast_version": 1,
            "sets": {
                "s": {"grain": "session"},
                "t": {"grain": "stack", "of": "s", "where": [["=", {}, ["axis", {}, "base"], "T1w"]],
                      "pick": {"per": "session", "by": [[["field", {}, "stack_index"], "asc"]], "ties": "report"}}
            },
            "out": {"set": "t", "level": "record", "columns": [["field", {}, "session.id"], ["field", {}, "pick.tied"], ["field", {}, "pick.candidates"]]}
        }).to_string());
        assert!(!p.rows.is_empty(), "{}: no picks\n{}", l.name, pick_sql.sql);
        let sessions: std::collections::BTreeSet<i64> =
            p.rows.iter().map(|r| r.int(2).unwrap()).collect();
        assert_eq!(
            sessions.len(),
            p.rows.len(),
            "{}: one pick per session",
            l.name
        );
        let pairs: Vec<(i64, i64)> = p
            .rows
            .iter()
            .map(|r| (r.int(3).unwrap(), r.int(4).unwrap()))
            .collect();
        assert!(
            pairs.iter().any(|(tied, _)| *tied == 1),
            "{}: a tie was reported; (tied, candidates) = {:?}",
            l.name,
            &pairs[..pairs.len().min(12)]
        );
    }
}

#[test]
fn a_cap_marks_the_answer_truncated_and_a_timeout_stops_it() {
    for mut l in labs() {
        let ask = parse(
            &json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack"}},
                "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"]]}
            })
            .to_string(),
        )
        .unwrap();
        let prepared = prepare(ask, &l.catalog, &Scope::default()).unwrap();
        let store = l.registry.store();
        let ctx = Context {
            names: &l.catalog,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: 0,
            scheme_digest: Scheme::default().digest(),
            after: None,
            limit: None,
        };
        let compiled = compile(&prepared.ask, &prepared.validated, &ctx).unwrap();
        let capped = run(
            store,
            &compiled,
            Bounds {
                timeout_ms: 20_000,
                max_rows: 10,
                max_bytes: 1 << 20,
            },
        )
        .unwrap();
        assert!(capped.truncated, "{}", l.name);
        assert_eq!(capped.rows.len(), 10);
        assert!(capped.content_hash.is_none());
        // keyset paging: the page after the tenth key continues where it left off
        let after = capped.rows[9].int(0).unwrap();
        let ctx2 = Context {
            after: Some(after),
            limit: Some(10),
            ..ctx
        };
        let next = compile(&prepared.ask, &prepared.validated, &ctx2).unwrap();
        let page = run(
            store,
            &next,
            Bounds {
                timeout_ms: 20_000,
                max_rows: 5_000,
                max_bytes: 1 << 20,
            },
        )
        .unwrap();
        assert!(!page.truncated);
        assert_eq!(page.rows.len(), 10);
        assert!(page.rows[0].int(0).unwrap() > after);
        // the store is whole after a capped run
        assert!(store.query("SELECT 1", &[]).is_ok());
    }
}
