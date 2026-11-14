// SPDX-License-Identifier: AGPL-3.0-only

//! The count of a group set answers its groups and the subjects under
//! them. `subjects` was the number of groups, so a set of seven groups that
//! five subjects made read as seven subjects. It is the distinct subjects
//! of the rows the groups that survive hold: one subject of two groups is
//! one, a group the set's own `where` drops takes its subjects with it
//! unless another group holds them, a group of a group is read down to the
//! rows with a subject, and rows with no subject (cohorts) have none, which
//! is null and never the number of groups. Each is held here against what
//! the groups' own rows say, on both backends, with what is held back at
//! plain.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Lab, ask_of, quasi};
use nils_ask::affordance::{self, Setting};
use nils_ask::compile::{Context, compile};
use nils_ask::exec::{Answer, Bounds, run};
use nils_ask::validate::Scope;
use nils_ask::{parse, prepare};
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_group_subjects_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

fn doc(sets: &Value, out: Value) -> String {
    json!({"ast_version": 1, "name": "the subjects under the groups", "sets": sets, "out": out})
        .to_string()
}

/// An answer under a scope of the caller's.
fn answered(l: &mut Lab, text: &str, scope: &Scope) -> Answer {
    let prepared = prepare(parse(text).unwrap(), &l.catalog, scope)
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
    let store = l.registry.store();
    let ctx = Context {
        names: &l.catalog,
        dialect: store.dialect(),
        schema: store.schema().map(str::to_string),
        window_days: 0,
        scheme_digest: Scheme::default().digest(),
        after: None,
        limit: None,
        withhold_sealed: false,
    };
    let compiled = compile(&prepared.ask, &prepared.validated, &ctx)
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
    run(store, &compiled, bounds()).unwrap_or_else(|e| panic!("{}: {e}\n{}", l.name, compiled.sql))
}

/// The count of a set: its rows and its subjects, as answered.
fn count(l: &mut Lab, sets: &Value, set: &str, scope: &Scope) -> (Value, Value, Answer) {
    let a = answered(l, &doc(sets, json!({"set": set, "level": "count"})), scope);
    assert_eq!(a.columns, ["rows", "subjects"], "{}", l.name);
    assert_eq!(a.rows.len(), 1, "{}", l.name);
    let cell = |i: usize| nils_ask::handle::cell_json(&a.rows[0].0[i]);
    (cell(0), cell(1), a)
}

/// The rows of a group set, each as its columns by name.
fn listed(l: &mut Lab, sets: &Value, set: &str, columns: &[&str]) -> Vec<BTreeMap<String, Value>> {
    let cols: Vec<Value> = columns.iter().map(|c| json!(["field", {}, c])).collect();
    let (_, a) = ask_of(
        l,
        &doc(
            sets,
            json!({"set": set, "level": "aggregate", "columns": cols}),
        ),
    );
    a.rows
        .iter()
        .map(|r| {
            a.columns
                .iter()
                .cloned()
                .zip(r.0.iter().map(nils_ask::handle::cell_json))
                .collect()
        })
        .collect()
}

fn distinct(rows: &[&BTreeMap<String, Value>], column: &str) -> usize {
    rows.iter()
        .map(|r| r[column].to_string())
        .collect::<BTreeSet<_>>()
        .len()
}

/// Stacks, and their groups by subject and session: a subject of several
/// sessions makes several groups.
fn cells(wheres: Value) -> Value {
    json!({
        "scans": {"grain": "stack"},
        "cells": {"grain": "group",
            "group": {"of": "scans", "by": [["field", {}, "subject.id"], ["field", {}, "session.id"]]},
            "where": wheres}
    })
}

/// The shape that was seen: more groups than subjects, each subject once.
#[test]
fn the_subjects_of_a_group_set_are_counted_once_each() {
    for mut l in labs() {
        let sets = cells(json!([]));
        let rows = listed(&mut l, &sets, "cells", &["subject.id", "_rows"]);
        let all: Vec<&BTreeMap<String, Value>> = rows.iter().collect();
        let subjects = distinct(&all, "subject.id");
        assert!(
            rows.len() > subjects && subjects > 5,
            "{}: {} groups of {subjects} subjects",
            l.name,
            rows.len()
        );
        let (n, s, a) = count(&mut l, &sets, "cells", &quasi());
        assert_eq!(n, json!(rows.len()), "{}: the groups", l.name);
        assert_eq!(s, json!(subjects), "{}: their subjects, each once", l.name);
        assert!(a.held_back.is_empty(), "{}", l.name);
        // the stacks' own count says the same subjects
        let (_, of_stacks, _) = count(&mut l, &sets, "scans", &quasi());
        assert_eq!(s, of_stacks, "{}", l.name);
        // each group still says its own subjects, as it did
        let per_group = listed(&mut l, &sets, "cells", &["_subjects"]);
        assert!(
            per_group.iter().all(|r| r["_subjects"] == json!(1)),
            "{}",
            l.name
        );

        // a where that drops groups drops their subjects, unless another
        // group that survives holds them
        let sizes: BTreeSet<i64> = rows.iter().filter_map(|r| r["_rows"].as_i64()).collect();
        let mut tried = 0;
        for least in sizes {
            let kept: Vec<&BTreeMap<String, Value>> = rows
                .iter()
                .filter(|r| r["_rows"].as_i64().is_some_and(|x| x >= least))
                .collect();
            let of_kept = distinct(&kept, "subject.id");
            let (n, s, _) = count(
                &mut l,
                &cells(json!([[">=", {}, ["field", {}, "_rows"], least]])),
                "cells",
                &quasi(),
            );
            assert_eq!(n, json!(kept.len()), "{}: at least {least}", l.name);
            assert_eq!(s, json!(of_kept), "{}: at least {least}", l.name);
            if of_kept < subjects && kept.len() > of_kept {
                // some subjects left with their groups, and a subject kept
                // is still of several groups
                tried += 1;
            }
        }
        assert!(
            tried > 0,
            "{}: no where drops some subjects and keeps one of two groups",
            l.name
        );

        // keys that name no subject: the groups by the year of the visit,
        // the years of fewer subjects dropped
        let year = json!({"year": ["bucket", {"unit": "year"}, ["field", {}, "session.first"]]});
        let by_year = |wheres: Value| {
            json!({
                "scans": {"grain": "stack", "bind": year,
                    "where": [["not_null", {}, ["field", {}, "year"]]]},
                "years": {"grain": "group",
                    "group": {"of": "scans", "by": [["field", {}, "year"]]},
                    "where": wheres}
            })
        };
        let years = listed(&mut l, &by_year(json!([])), "years", &["year", "_subjects"]);
        let mut held: Vec<i64> = years
            .iter()
            .filter_map(|r| r["_subjects"].as_i64())
            .collect();
        held.sort_unstable();
        assert!(
            held.len() >= 4 && held[0] < held[held.len() - 1],
            "{held:?}"
        );
        let least = held[held.len() / 2].max(held[0] + 1);
        let names: Vec<Value> = years
            .iter()
            .filter(|r| r["_subjects"].as_i64().is_some_and(|x| x >= least))
            .map(|r| r["year"].clone())
            .collect();
        assert!(!names.is_empty() && names.len() < years.len(), "{}", l.name);
        let (n, s, _) = count(
            &mut l,
            &by_year(json!([[">=", {}, ["field", {}, "_subjects"], least]])),
            "years",
            &quasi(),
        );
        assert_eq!(n, json!(names.len()), "{}", l.name);
        // the subjects of the stacks of those years, as a stack set counts
        let (_, of_those, _) = count(
            &mut l,
            &json!({"scans": {"grain": "stack", "bind": year,
                "where": [["in", {}, ["field", {}, "year"], names]]}}),
            "scans",
            &quasi(),
        );
        assert_eq!(s, of_those, "{}", l.name);
        // a subject seen in two of those years is one subject: fewer than
        // the years' own counts together, and no more than every year holds
        let together: i64 = held.iter().filter(|x| **x >= least).sum();
        let (_, of_all, _) = count(&mut l, &by_year(json!([])), "years", &quasi());
        assert!(
            s.as_i64() < Some(together) && s.as_i64() <= of_all.as_i64(),
            "{}: {s} of {together}, {of_all} in all",
            l.name
        );
    }
}

/// A group of a group is read down to the rows with a subject, through the
/// `where` of each.
#[test]
fn a_group_of_a_group_counts_the_subjects_under_what_survives_of_both() {
    for mut l in labs() {
        let inner = cells(json!([[">=", {}, ["field", {}, "_rows"], 2]]));
        let rows = listed(&mut l, &inner, "cells", &["subject.id", "_rows"]);
        assert!(rows.len() > 10, "{}", l.name);
        // by subject, the subjects with two such groups or more
        let mut per_subject: BTreeMap<String, usize> = BTreeMap::new();
        for r in &rows {
            *per_subject.entry(r["subject.id"].to_string()).or_insert(0) += 1;
        }
        let twice = per_subject.values().filter(|n| **n >= 2).count();
        assert!(twice > 0 && twice < per_subject.len(), "{}", l.name);
        let mut sets = inner.clone();
        sets["repeats"] = json!({"grain": "group",
            "group": {"of": "cells", "by": [["field", {}, "subject.id"]]},
            "bind": {"n": ["count", {"set": "cells"}]},
            "where": [[">=", {}, ["field", {}, "n"], 2]]});
        let (n, s, _) = count(&mut l, &sets, "repeats", &quasi());
        assert_eq!((n, s), (json!(twice), json!(twice)), "{}", l.name);
        // by a key that names no subject: the groups by how many stacks
        // they hold, the sizes that two groups or more have
        let mut by_size: BTreeMap<i64, Vec<&BTreeMap<String, Value>>> = BTreeMap::new();
        for r in &rows {
            by_size
                .entry(r["_rows"].as_i64().unwrap())
                .or_default()
                .push(r);
        }
        let kept: Vec<&Vec<&BTreeMap<String, Value>>> =
            by_size.values().filter(|g| g.len() >= 2).collect();
        let under: Vec<&BTreeMap<String, Value>> =
            kept.iter().flat_map(|g| g.iter().copied()).collect();
        assert!(!kept.is_empty(), "{}", l.name);
        let mut sets = inner.clone();
        sets["sizes"] = json!({"grain": "group",
            "group": {"of": "cells", "by": [["field", {}, "_rows"]]},
            "where": [[">=", {}, ["field", {}, "_rows"], 2]]});
        let (n, s, _) = count(&mut l, &sets, "sizes", &quasi());
        assert_eq!(n, json!(kept.len()), "{}: the sizes", l.name);
        assert_eq!(
            s,
            json!(distinct(&under, "subject.id")),
            "{}: the subjects of the groups of those sizes",
            l.name
        );
        // and three deep
        sets["once"] = json!({"grain": "group",
            "group": {"of": "sizes", "by": [["field", {}, "_rows"]]}});
        let (_, deeper, _) = count(&mut l, &sets, "once", &quasi());
        assert_eq!(
            deeper, s,
            "{}: a group over the sizes holds their subjects",
            l.name
        );
    }
}

/// Rows with no subject have none to count: null, never the groups.
#[test]
fn a_group_over_rows_with_no_subject_counts_none() {
    for mut l in labs() {
        let sets = json!({
            "cohorts": {"grain": "cohort"},
            "names": {"grain": "group", "group": {"of": "cohorts", "by": [["field", {}, "name"]]}},
            "sizes": {"grain": "group", "group": {"of": "names", "by": [["field", {}, "_rows"]]}}
        });
        let (cohorts, _, _) = count(&mut l, &sets, "cohorts", &quasi());
        assert!(cohorts.as_i64().is_some_and(|n| n >= 2), "{}", l.name);
        for scope in [quasi(), Scope::default()] {
            let (n, s, a) = count(&mut l, &sets, "names", &scope);
            assert_eq!(n, cohorts, "{}: a group a cohort", l.name);
            assert_eq!(s, Value::Null, "{}: no subject is counted", l.name);
            // and a number that is not there is not a number held back
            assert!(a.held_back.is_empty(), "{}: {:?}", l.name, a.held_back);
            let (n, s, _) = count(&mut l, &sets, "sizes", &scope);
            assert_eq!((n, s), (json!(1), Value::Null), "{}", l.name);
        }
    }
}

/// Record 55 K7, small counts at plain: the count of a group set that a
/// quasi identifying field keyed is held where its groups hold 1 to 4
/// subjects, the subjects this count answers, and shown for none and for 5
/// and more.
#[test]
fn the_count_of_a_group_set_is_held_by_the_subjects_under_its_groups() {
    for mut l in labs() {
        let years = |wheres: Value| {
            json!({
                "people": {"grain": "subject",
                    "bind": {"born": ["bucket", {"unit": "year"}, ["field", {}, "birth_date"]]}},
                "years": {"grain": "group",
                    "group": {"of": "people", "by": [["field", {}, "born"]]},
                    "where": wheres}
            })
        };
        let rows = listed(&mut l, &years(json!([])), "years", &["born", "_subjects"]);
        let all: i64 = rows.iter().filter_map(|r| r["_subjects"].as_i64()).sum();
        assert!(all >= 5 && rows.len() >= 5, "{}", l.name);
        let few = rows
            .iter()
            .find(|r| {
                r["_subjects"].as_i64().is_some_and(|n| (1..5).contains(&n))
                    && r["born"].is_string()
            })
            .unwrap_or_else(|| panic!("{}: no year of 1 to 4 subjects: {rows:?}", l.name));
        let (year, in_year) = (few["born"].clone(), few["_subjects"].clone());
        let one_year = years(json!([["=", {}, ["field", {}, "born"], year]]));
        let no_year = years(json!([["=", {}, ["field", {}, "born"], "1000"]]));
        let plain = Scope::default();

        // every year: 5 subjects and more, shown at plain as at quasi
        for scope in [quasi(), plain.clone()] {
            let (n, s, a) = count(&mut l, &years(json!([])), "years", &scope);
            assert_eq!((n, s), (json!(rows.len()), json!(all)), "{}", l.name);
            assert!(a.held_back.is_empty(), "{}", l.name);
        }
        // one year of 1 to 4 subjects: answered at quasi
        let (n, s, a) = count(&mut l, &one_year, "years", &quasi());
        assert_eq!((n, s), (json!(1), in_year), "{}", l.name);
        assert!(a.held_back.is_empty(), "{}", l.name);
        // and held at plain, the groups and the subjects both, though the
        // set the groups are of holds many
        let (n, s, a) = count(&mut l, &one_year, "years", &plain);
        assert_eq!((n, s), (Value::Null, Value::Null), "{}", l.name);
        assert_eq!(a.held_back, [0, 1], "{}", l.name);
        assert_eq!(a.held_counts, [0, 1], "{}", l.name);
        // no group: none is a number the rule shows
        let (n, s, a) = count(&mut l, &no_year, "years", &plain);
        assert_eq!((n, s), (json!(0), json!(0)), "{}", l.name);
        assert!(a.held_back.is_empty(), "{}", l.name);
    }
}

/// The same numbers at every reading of the count: a preview, a draft's
/// diagnosis, which counts no stage of a group, and the engine's own count
/// of the rows alone, which counts no subject.
#[test]
fn a_preview_and_a_draft_read_the_same_count() {
    for mut l in labs() {
        let sets = cells(json!([[">=", {}, ["field", {}, "_rows"], 2]]));
        let (n, s, _) = count(&mut l, &sets, "cells", &quasi());
        assert!(
            n.as_i64() > s.as_i64() && s.as_i64() > Some(0),
            "{}",
            l.name
        );
        let text = doc(&sets, json!({"set": "cells", "level": "count"}));
        let scope = quasi();
        let scheme = Scheme::default();
        let setting = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let ask = parse(&text).unwrap();
        let preview = affordance::preview(&mut l.registry, &ask, 5, &setting, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(preview.columns, ["rows", "subjects"], "{}", l.name);
        assert_eq!(preview.rows, [vec![n.clone(), s.clone()]], "{}", l.name);
        let drafted = affordance::draft(&mut l.registry, &text, &setting, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(
            drafted.diagnosis.valid && drafted.document.is_some(),
            "{}: {:?}",
            l.name,
            drafted.diagnosis.issues
        );
        assert!(
            drafted.diagnosis.funnel.iter().all(|st| st.set != "cells"),
            "{}: a group set has no stage",
            l.name
        );
        let rows = affordance::count_rows(&mut l.registry, &ask, &setting, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(json!(rows), n, "{}", l.name);
        // the count of the rows alone reads no member of a group
        let prepared = prepare(ask, &l.catalog, &scope).unwrap();
        let store = l.registry.store();
        let ctx = Context {
            names: &l.catalog,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: 0,
            scheme_digest: scheme.digest(),
            after: None,
            limit: None,
            withhold_sealed: false,
        };
        let whole = compile(&prepared.ask, &prepared.validated, &ctx).unwrap();
        let alone = compile(
            &prepared.ask,
            &prepared.validated.clone().counting_rows(),
            &ctx,
        )
        .unwrap();
        assert!(whole.sql.contains(" AS km"), "{}: {}", l.name, whole.sql);
        assert!(!alone.sql.contains(" AS km"), "{}: {}", l.name, alone.sql);
        let a = run(store, &alone, bounds()).unwrap();
        assert_eq!(
            a.rows[0]
                .0
                .iter()
                .map(nils_ask::handle::cell_json)
                .collect::<Vec<_>>(),
            [n, Value::Null],
            "{}",
            l.name
        );
    }
}

/// A share is the value over the distinct subjects of the set it names
/// (`docs/specs/wave4b-the-ask.md`, the post pass). Over a group set that
/// is the subjects under its groups; it was the number of groups.
#[test]
fn a_share_over_a_group_set_divides_by_its_subjects() {
    for mut l in labs() {
        let mut sets = cells(json!([]));
        sets["each"] = json!({"grain": "group",
            "group": {"of": "scans", "by": [["field", {}, "subject.id"]]}});
        let (groups, subjects, _) = count(&mut l, &sets, "cells", &quasi());
        let (groups, subjects) = (groups.as_i64().unwrap(), subjects.as_i64().unwrap());
        assert!(groups > subjects && subjects > 0, "{}", l.name);
        let text = json!({
            "ast_version": 1,
            "name": "each subject's share of the subjects seen",
            "sets": sets,
            "out": {"set": "each", "level": "aggregate",
                "columns": [["field", {}, "_subjects"]],
                "measures": [{"share": {"of": "_subjects", "over": "cells"}}]}
        })
        .to_string();
        let scope = quasi();
        let scheme = Scheme::default();
        let out = nils_ask::run::run(
            &mut l.registry,
            nils_ask::run::Request {
                ask: parse(&text).unwrap(),
                names: &l.catalog,
                scope: &scope,
                principal: "tester",
                node: "lab",
                pack_version: Some("0.0.0-test"),
                scheme: &scheme,
                bounds: bounds(),
                page_rows: 50,
                name: None,
                keep: false,
                after: None,
                limit: None,
                may_project_raw: false,
                purpose: None,
                reader: None,
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let at = out
            .answer
            .columns
            .iter()
            .position(|c| c == "share._subjects")
            .unwrap_or_else(|| panic!("{:?}", out.answer.columns));
        assert_eq!(out.answer.rows.len() as i64, subjects, "{}", l.name);
        for r in &out.answer.rows {
            let share = nils_ask::handle::cell_json(&r.0[at]).as_f64().unwrap();
            assert!(
                (share - 1.0 / subjects as f64).abs() < 1e-9,
                "{}: {share}, one of {subjects} subjects in {groups} groups",
                l.name
            );
        }
    }
}
