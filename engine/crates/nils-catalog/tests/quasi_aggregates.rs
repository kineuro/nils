// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 K7 on aggregates, end to end on each backend (the chat sandbox
//! bench, and the ruling of 2026-10-12). The bench's grouped question, each
//! subject's age at the first visit and its spread over ms-cohort-a, runs on
//! the synthetic registry at detail plain and at detail quasi. Below quasi
//! the youngest and the oldest age are their shapes, in the answer, the
//! handle and its pages, and the answer says which columns those are; the
//! group's mean, its count of distinct ages and its spread are the values
//! detail quasi sees, since the group holds 5 subjects or more, and are held
//! back (null, and named with why) in a group under 5; the counts are
//! answered at both; a group keyed by the age follows the field, and its
//! counts are held below 5 subjects (small counts); the median
//! of the age is refused below quasi; a share of the age is a shape below
//! quasi; and the document as the bench wrote it, its column as an option,
//! is refused at both details with the form to write. The ages by visit
//! number hold both kinds of group at once: the last visit number has 3
//! subjects.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::Lab;
use nils_ask::affordance::{self, Setting};
use nils_ask::exec::{Bounds, HeldBack, render};
use nils_ask::run::{Outcome, Request, RunError, run};
use nils_ask::validate::{MEASURE_K, Scope, held_back_why, shape};
use nils_ask::{Code, handle, parse};
use nils_registry::session::Scheme;
use nils_registry::store::{Cell, Row};

fn labs() -> Vec<Lab> {
    common::labs("nils_quasi_aggregates_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// The sets of the bench's question: ms-cohort-a's subjects, their visits
/// with the age at each, the first visit of each subject.
const SETS: &str = r#"
  scope:  {grain: cohort, where: [["=", {}, ["field", {}, "name"], "ms-cohort-a"]]}
  people: {grain: subject, of: scope}
  visits:
    grain: session
    of: people
    bind:
      n:   ["ordinal", {}]
      age: ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]
  firsts:
    grain: session
    from: visits
    where: [["=", {}, ["field", {}, "n"], 1]]
"#;

/// A document over the bench's sets, with one more set and the answer.
fn document(more: &str, out: &str) -> String {
    format!(
        "ast_version: 1\nname: age at the first visit in ms-cohort-a\nscheme: default\nparams:\n  all: {{type: text, value: all}}\nsets:{SETS}{more}\nout:\n{out}\n"
    )
}

/// The bench's grouped question, written as the language reads it.
fn spread() -> String {
    document(
        r#"  spread:
    grain: group
    group: {of: firsts, by: [["param", {}, "all"]]}
    bind:
      n_subjects: ["count", {set: firsts}]
      youngest:   ["min", {set: firsts}, ["field", {}, "age"]]
      oldest:     ["max", {set: firsts}, ["field", {}, "age"]]
      mean_age:   ["avg", {set: firsts}, ["field", {}, "age"]]
      n_ages:     ["distinct", {set: firsts}, ["field", {}, "age"]]"#,
        r#"  set: spread
  level: aggregate
  columns:
    - ["field", {}, "n_subjects"]
    - ["field", {}, "youngest"]
    - ["field", {}, "oldest"]
    - ["field", {}, "mean_age"]
    - ["field", {}, "n_ages"]"#,
    )
}

fn go(l: &mut Lab, text: &str, scope: &Scope) -> Result<Outcome, RunError> {
    let scheme = Scheme::default();
    run(
        &mut l.registry,
        Request {
            ask: parse(text).unwrap_or_else(|e| panic!("{}: {e}", l.name)),
            names: &l.catalog,
            scope,
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
}

fn ok(l: &mut Lab, text: &str, scope: &Scope) -> Outcome {
    go(l, text, scope).unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

/// A column of an answer, by name.
fn column<'a>(out: &'a Outcome, name: &str) -> Vec<&'a Cell> {
    let i = out
        .answer
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no column {name}: {:?}", out.answer.columns));
    out.answer.rows.iter().map(|r: &Row| &r.0[i]).collect()
}

/// A cell as the answer shows a shape of it.
fn shaped(c: &Cell) -> Cell {
    match c {
        Cell::Null => Cell::Null,
        other => Cell::Text(shape(&render(other))),
    }
}

fn number(c: &Cell) -> f64 {
    match c {
        Cell::Int(i) => *i as f64,
        Cell::Double(d) => *d,
        other => panic!("not a number: {other:?}"),
    }
}

/// The columns an answer names as held back, each with why.
fn held(names: &[&str]) -> Vec<HeldBack> {
    names
        .iter()
        .map(|n| HeldBack {
            column: n.to_string(),
            why: held_back_why(),
        })
        .collect()
}

/// The population spread, as the post pass computes it.
fn spread_of(xs: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt()
}

#[test]
fn an_aggregate_of_a_quasi_field_follows_the_field_on_both_backends() {
    for mut l in labs() {
        let plain = Scope::default();
        let quasi = common::quasi();
        assert_eq!(held_back_why(), "a group under 5 subjects");

        // the ages the spread is made from, read at detail quasi
        let records = document(
            "",
            r#"  set: firsts
  level: record
  columns: [["field", {}, "age"]]"#,
        );
        let ages: Vec<f64> = ok(&mut l, &records, &quasi)
            .answer
            .rows
            .iter()
            .filter(|r| !matches!(r.0[2], Cell::Null))
            .map(|r| number(&r.0[2]))
            .collect();
        assert!(ages.len() > 20, "{}: {ages:?}", l.name);
        let lowest = ages.iter().copied().fold(f64::MAX, f64::min);
        let highest = ages.iter().copied().fold(f64::MIN, f64::max);
        let mean = ages.iter().sum::<f64>() / ages.len() as f64;
        let distinct: BTreeSet<i64> = ages.iter().map(|a| *a as i64).collect();

        // at detail quasi: the values
        let raw = ok(&mut l, &spread(), &quasi);
        assert!(raw.answer.shaped.is_empty(), "{}", l.name);
        assert!(raw.answer.shaped_names().is_empty());
        assert!(raw.answer.held_back.is_empty());
        assert_eq!(number(column(&raw, "youngest")[0]), lowest, "{}", l.name);
        assert_eq!(number(column(&raw, "oldest")[0]), highest, "{}", l.name);
        assert!(
            (number(column(&raw, "mean_age")[0]) - mean).abs() < 1e-6,
            "{}",
            l.name
        );
        assert_eq!(
            number(column(&raw, "n_ages")[0]) as usize,
            distinct.len(),
            "{}",
            l.name
        );

        // at detail plain: the youngest and the oldest as their shapes; the
        // mean and the count of distinct ages as detail quasi sees them,
        // since the group holds 5 subjects or more; the count of subjects
        let at_plain = ok(&mut l, &spread(), &plain);
        assert_eq!(
            at_plain.answer.shaped_names(),
            vec!["youngest", "oldest"],
            "{}",
            l.name
        );
        assert!(at_plain.answer.held_back_named().is_empty(), "{}", l.name);
        for name in ["youngest", "oldest"] {
            let cell = column(&at_plain, name)[0];
            assert_eq!(*cell, shaped(column(&raw, name)[0]), "{}: {name}", l.name);
            assert!(matches!(cell, Cell::Text(t) if t.chars().all(|c| c == '9' || c == '.')));
        }
        for name in ["n_subjects", "mean_age", "n_ages"] {
            assert_eq!(
                column(&at_plain, name),
                column(&raw, name),
                "{}: {name}",
                l.name
            );
        }
        assert!(matches!(column(&at_plain, "mean_age")[0], Cell::Double(_)));
        assert!(matches!(column(&at_plain, "n_ages")[0], Cell::Int(_)));
        // the handle says which, and its pages hold the shapes alone
        let h = handle::get(l.registry.store(), at_plain.handle.id)
            .unwrap()
            .unwrap();
        assert_eq!(h.shaped(), vec!["youngest", "oldest"]);
        assert!(h.held_back().is_empty());
        assert!(
            h.columns.iter().all(
                |c| c.shaped == ["youngest", "oldest"].contains(&c.name.as_str())
                    && c.held_back.is_none()
            ),
            "{:?}",
            h.columns
        );
        let page = handle::page(l.registry.store(), h.id, 0).unwrap().unwrap();
        let at = |name: &str| h.columns.iter().position(|c| c.name == name).unwrap();
        for name in ["youngest", "oldest"] {
            assert_eq!(
                page[0][at(name)],
                handle::cell_json(&shaped(column(&raw, name)[0])),
                "{}: {name}",
                l.name
            );
        }
        assert_eq!(
            page[0][at("mean_age")],
            handle::cell_json(column(&raw, "mean_age")[0])
        );
        // a handle answered raw says none
        let raw_handle = handle::get(l.registry.store(), raw.handle.id)
            .unwrap()
            .unwrap();
        assert!(raw_handle.shaped().is_empty() && raw_handle.held_back().is_empty());
        // the preview says the same
        let scheme = Scheme::default();
        let setting = |scope| Setting {
            names: &l.catalog,
            scope,
            scheme: &scheme,
            principal: "tester",
            bounds: bounds(),
            values_cap: 50,
        };
        let ask = parse(&spread()).unwrap();
        let previewed = affordance::preview(&mut l.registry, &ask, 10, &setting(&plain), None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(previewed.shaped, vec!["youngest", "oldest"]);
        assert!(previewed.held_back.is_empty());
        let previewed = affordance::preview(&mut l.registry, &ask, 10, &setting(&quasi), None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(previewed.shaped.is_empty() && previewed.held_back.is_empty());

        // the spread of the age over the group's child, with its mean: at
        // detail plain the value detail quasi sees, for 5 subjects or more
        let with_spread = spread().replace(
            "    - [\"field\", {}, \"n_ages\"]",
            "    - [\"field\", {}, \"n_ages\"]\n  measures:\n    - {stddev: {of: age}}",
        );
        assert_ne!(with_spread, spread());
        let s_raw = ok(&mut l, &with_spread, &quasi);
        let s_plain = ok(&mut l, &with_spread, &plain);
        assert!(
            (number(column(&s_raw, "stddev.age")[0]) - spread_of(&ages)).abs() < 1e-6,
            "{}",
            l.name
        );
        assert_eq!(
            column(&s_plain, "stddev.age"),
            column(&s_raw, "stddev.age"),
            "{}",
            l.name
        );
        assert!(s_plain.answer.held_back_named().is_empty());

        // a group of one, each subject's own first visit: its mean is its
        // age, its count of distinct ages one, its spread none; each is held
        // back below quasi and named with why, and the count of visits is
        // answered
        let each = document(
            r#"  each:
    grain: group
    group: {of: firsts, by: [["field", {}, "subject.id"]]}
    bind:
      mean_age: ["avg", {set: firsts}, ["field", {}, "age"]]
      n_ages:   ["distinct", {set: firsts}, ["field", {}, "age"]]
      visits:   ["count", {set: firsts}]"#,
            r#"  set: each
  level: aggregate
  columns: [["field", {}, "subject.id"], ["field", {}, "mean_age"], ["field", {}, "n_ages"], ["field", {}, "visits"]]
  measures:
    - {stddev: {of: age}}
  order: [[["field", {}, "subject.id"], asc]]"#,
        );
        let one_raw = ok(&mut l, &each, &quasi);
        let one_plain = ok(&mut l, &each, &plain);
        assert!(one_raw.answer.held_back.is_empty());
        assert_eq!(
            one_plain.answer.held_back_named(),
            held(&["mean_age", "n_ages", "stddev.age"]),
            "{}",
            l.name
        );
        assert!(one_plain.answer.shaped.is_empty());
        assert!(
            column(&one_raw, "mean_age")
                .iter()
                .any(|c| matches!(c, Cell::Double(_))),
            "{}",
            l.name
        );
        for name in ["mean_age", "n_ages", "stddev.age"] {
            assert!(
                column(&one_plain, name)
                    .iter()
                    .all(|c| matches!(c, Cell::Null)),
                "{}: {name}",
                l.name
            );
        }
        assert!(
            column(&one_raw, "visits")
                .iter()
                .all(|c| **c == Cell::Int(1))
        );
        assert_eq!(column(&one_plain, "visits"), column(&one_raw, "visits"));
        // the handle keeps why, on the columns of the answer it stored
        let h = handle::get(l.registry.store(), one_plain.handle.id)
            .unwrap()
            .unwrap();
        assert_eq!(h.held_back(), held(&["mean_age", "n_ages"]), "{}", l.name);
        let flagged = h
            .columns
            .iter()
            .find(|c| c.name == "mean_age")
            .and_then(|c| c.held_back.clone());
        assert_eq!(flagged.as_deref(), Some("a group under 5 subjects"));

        // a group keyed by the age: the key a shape, the counts answered
        // for 5 subjects or more and held back below, with one more beside
        // them (small counts, the ruling of 2026-10-12)
        let by_age = document(
            r#"  by_age:
    grain: group
    group: {of: firsts, by: [["field", {}, "age"]]}
    bind:
      n: ["count", {set: firsts}]"#,
            r#"  set: by_age
  level: aggregate
  columns: [["field", {}, "age"], ["field", {}, "n"]]"#,
        );
        let key_raw = ok(&mut l, &by_age, &quasi);
        let key_plain = ok(&mut l, &by_age, &plain);
        assert_eq!(key_plain.answer.shaped_names(), vec!["age"]);
        assert!(key_raw.answer.shaped_names().is_empty());
        let keys: Vec<Cell> = column(&key_raw, "age").into_iter().map(shaped).collect();
        assert_eq!(
            column(&key_plain, "age")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            keys
        );
        let mut shown = 0;
        for (p, r) in column(&key_plain, "n")
            .into_iter()
            .zip(column(&key_raw, "n"))
        {
            match (p, r) {
                (Cell::Null, _) => {}
                (p, Cell::Int(n)) if *n >= MEASURE_K => {
                    assert_eq!(p, r);
                    shown += 1;
                }
                (p, r) => panic!("{}: {p:?} shown for {r:?}", l.name),
            }
        }
        assert!(shown > 0, "{}", l.name);
        assert_eq!(
            key_plain
                .answer
                .held_back_named()
                .into_iter()
                .map(|h| h.column)
                .collect::<Vec<_>>(),
            vec!["n"]
        );

        // the median of the age per group, which is one person's: refused
        // below quasi with the reason, answered at quasi
        let median = spread().replace(
            "    - [\"field\", {}, \"n_ages\"]",
            "    - [\"field\", {}, \"n_ages\"]\n  measures:\n    - {median: {of: age}}",
        );
        assert_ne!(median, spread());
        match go(&mut l, &median, &plain) {
            Err(RunError::Ask(nils_ask::Error::Invalid(issues))) => assert!(
                issues.iter().any(|i| i.code == Code::ForbiddenField
                    && i.path == "out.measures[0]"
                    && i.message.contains("record 55 K7")),
                "{}: {issues:?}",
                l.name
            ),
            other => panic!("{}: {:?}", l.name, other.map(|o| o.answer.rows)),
        }
        let measured = ok(&mut l, &median, &quasi);
        let mut sorted = ages.clone();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let middle = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        assert_eq!(
            number(column(&measured, "median.age")[0]),
            middle,
            "{}",
            l.name
        );

        // a share of the age divides it by a count the answer also says, so
        // below quasi it is a shape, as the age is
        let shares = document(
            "",
            r#"  set: firsts
  level: record
  columns: [["field", {}, "age"], ["share", {of: age, over: firsts}]]
  order: [[["field", {}, "id"], asc]]"#,
        );
        let s_raw = ok(&mut l, &shares, &quasi);
        let s_plain = ok(&mut l, &shares, &plain);
        assert_eq!(s_plain.answer.shaped_names(), vec!["age", "col1"]);
        assert!(s_raw.answer.shaped_names().is_empty());
        let subjects = number(column(&raw, "n_subjects")[0]);
        for (age, share) in column(&s_raw, "age").iter().zip(column(&s_raw, "col1")) {
            if !matches!(age, Cell::Null) {
                assert!((number(share) * subjects - number(age)).abs() < 1e-6);
            }
        }
        let shapes: Vec<Cell> = column(&s_raw, "col1").into_iter().map(shaped).collect();
        assert_eq!(
            column(&s_plain, "col1")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            shapes,
            "{}",
            l.name
        );

        // the document as the bench wrote it: the column as an option, read
        // as nothing, refused at both details with the form to write
        let bench = spread()
            .replace(
                r#"["min", {set: firsts}, ["field", {}, "age"]]"#,
                r#"["min", {set: firsts, column: ["field", {}, "age"]}]"#,
            )
            .replace(
                r#"["max", {set: firsts}, ["field", {}, "age"]]"#,
                r#"["max", {set: firsts, field: age}]"#,
            );
        for scope in [&plain, &quasi] {
            match go(&mut l, &bench, scope) {
                Err(RunError::Ask(nils_ask::Error::Invalid(issues))) => {
                    let at = |path: &str| {
                        issues
                            .iter()
                            .find(|i| i.path == path)
                            .unwrap_or_else(|| panic!("{}: nothing at {path}: {issues:?}", l.name))
                    };
                    assert_eq!(at("sets.spread.bind.youngest").code, Code::UnknownField);
                    assert_eq!(
                        at("sets.spread.bind.youngest").next,
                        r#"write ["min", {set: firsts}, ["field",{},"age"]]"#
                    );
                    assert_eq!(
                        at("sets.spread.bind.oldest").next,
                        r#"write ["max", {set: firsts}, ["field", {}, "age"]]"#
                    );
                }
                other => panic!("{}: {:?}", l.name, other.map(|o| o.answer.rows)),
            }
        }
    }
}

/// The ages by visit number, from a record read at detail quasi: per visit
/// number, the ages and the subjects that have one.
fn ages_by_visit(l: &mut Lab) -> BTreeMap<i64, (Vec<f64>, BTreeSet<i64>)> {
    let records = document(
        "",
        r#"  set: visits
  level: record
  columns: [["field", {}, "n"], ["field", {}, "age"]]"#,
    );
    let mut out: BTreeMap<i64, (Vec<f64>, BTreeSet<i64>)> = BTreeMap::new();
    for r in ok(l, &records, &common::quasi()).answer.rows {
        let (Cell::Int(n), Cell::Int(subject)) = (&r.0[2], &r.0[1]) else {
            panic!("{}: a visit without its number or subject: {r:?}", l.name);
        };
        if matches!(r.0[3], Cell::Null) {
            continue;
        }
        let (ages, who) = out.entry(*n).or_default();
        ages.push(number(&r.0[3]));
        who.insert(*subject);
    }
    out
}

/// The ruling of 2026-10-12 in one answer: the age at each visit number, by
/// its mean, its youngest, its count of distinct values and its spread. At
/// detail plain a group of 5 subjects or more answers what detail quasi
/// sees, a smaller group holds its mean, count and spread back, the
/// youngest is a shape in every group, and the answer, its handle and its
/// page name the held columns with why. Subjects are counted, never rows.
#[test]
fn a_group_under_five_subjects_holds_its_values_back_on_both_backends() {
    for mut l in labs() {
        let plain = Scope::default();
        let quasi = common::quasi();
        let by_visit = document(
            r#"  by_visit:
    grain: group
    group: {of: visits, by: [["field", {}, "n"]]}
    bind:
      mean_age: ["avg", {set: visits}, ["field", {}, "age"]]
      youngest: ["min", {set: visits}, ["field", {}, "age"]]
      n_ages:   ["distinct", {set: visits}, ["field", {}, "age"]]
      rounded:  ["round", {}, ["field", {}, "mean_age"], 0]"#,
            r#"  set: by_visit
  level: aggregate
  columns:
    - ["field", {}, "n"]
    - ["field", {}, "_subjects"]
    - ["field", {}, "mean_age"]
    - ["field", {}, "youngest"]
    - ["field", {}, "n_ages"]
    - ["field", {}, "rounded"]
  measures:
    - {stddev: {of: age}}
  order: [[["field", {}, "n"], asc]]"#,
        );
        let truth = ages_by_visit(&mut l);
        let small: Vec<i64> = truth
            .iter()
            .filter(|(_, (_, who))| (who.len() as i64) < MEASURE_K)
            .map(|(n, _)| *n)
            .collect();
        assert!(!small.is_empty(), "{}: no visit number under 5", l.name);
        assert!(
            truth.len() > small.len(),
            "{}: no visit number of 5 or more",
            l.name
        );
        let raw = ok(&mut l, &by_visit, &quasi);
        let at_plain = ok(&mut l, &by_visit, &plain);
        assert!(raw.answer.held_back.is_empty() && raw.answer.shaped.is_empty());
        assert_eq!(
            at_plain.answer.shaped_names(),
            vec!["youngest"],
            "{}",
            l.name
        );
        assert_eq!(
            at_plain.answer.held_back_named(),
            held(&["mean_age", "n_ages", "rounded", "stddev.age"]),
            "{}",
            l.name
        );
        let numbers = column(&raw, "n");
        assert_eq!(numbers.len(), truth.len(), "{}", l.name);
        for (row, n) in numbers.iter().enumerate() {
            let n = number(n) as i64;
            let (ages, who) = &truth[&n];
            // detail quasi: the values themselves, the spread from 3 or from 30
            assert!(
                (number(column(&raw, "mean_age")[row])
                    - ages.iter().sum::<f64>() / ages.len() as f64)
                    .abs()
                    < 1e-6,
                "{}: visit {n}",
                l.name
            );
            assert!(
                (number(column(&raw, "stddev.age")[row]) - spread_of(ages)).abs() < 1e-6,
                "{}: visit {n}",
                l.name
            );
            // the count of subjects is a count, answered at plain whatever
            // the size of the group
            assert_eq!(
                column(&at_plain, "_subjects")[row],
                column(&raw, "_subjects")[row]
            );
            assert_eq!(
                *column(&at_plain, "youngest")[row],
                shaped(column(&raw, "youngest")[row]),
                "{}: visit {n}",
                l.name
            );
            for name in ["mean_age", "n_ages", "rounded", "stddev.age"] {
                let got = column(&at_plain, name)[row];
                if (who.len() as i64) < MEASURE_K {
                    assert_eq!(*got, Cell::Null, "{}: visit {n}, {name}", l.name);
                } else {
                    assert_eq!(
                        got,
                        column(&raw, name)[row],
                        "{}: visit {n}, {name}",
                        l.name
                    );
                }
            }
        }
        // the handle and a page of it hold the same and name the columns
        // the stored answer held back; the spread is the run's own column
        let h = handle::get(l.registry.store(), at_plain.handle.id)
            .unwrap()
            .unwrap();
        assert_eq!(h.shaped(), vec!["youngest"]);
        assert_eq!(h.held_back(), held(&["mean_age", "n_ages", "rounded"]));
        let page = handle::page(l.registry.store(), h.id, 0).unwrap().unwrap();
        let at = |name: &str| h.columns.iter().position(|c| c.name == name).unwrap();
        for (row, n) in numbers.iter().enumerate() {
            let held_here = small.contains(&(number(n) as i64));
            assert_eq!(page[row][at("mean_age")].is_null(), held_here, "{}", l.name);
        }
        // the preview names the held columns of the rows it shows
        let scheme = Scheme::default();
        let setting = Setting {
            names: &l.catalog,
            scope: &plain,
            scheme: &scheme,
            principal: "tester",
            bounds: bounds(),
            values_cap: 50,
        };
        let previewed = affordance::preview(
            &mut l.registry,
            &parse(&by_visit).unwrap(),
            10,
            &setting,
            None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(previewed.shaped, vec!["youngest"]);
        assert_eq!(
            previewed.held_back,
            held(&["mean_age", "n_ages", "rounded"])
        );

        // one subject's many visits are one subject: every visit of each
        // subject in one group per subject holds its mean back, however many
        // rows it has
        let per_subject = document(
            r#"  per:
    grain: group
    group: {of: visits, by: [["field", {}, "subject.id"]]}
    bind:
      mean_age: ["avg", {set: visits}, ["field", {}, "age"]]
      rows:     ["count", {set: visits}]"#,
            r#"  set: per
  level: aggregate
  columns: [["field", {}, "mean_age"], ["field", {}, "rows"]]"#,
        );
        let per_plain = ok(&mut l, &per_subject, &plain);
        assert!(
            column(&per_plain, "rows")
                .iter()
                .any(|c| number(c) >= MEASURE_K as f64),
            "{}: no subject with 5 visits",
            l.name
        );
        assert!(
            column(&per_plain, "mean_age")
                .iter()
                .all(|c| matches!(c, Cell::Null)),
            "{}",
            l.name
        );
        assert_eq!(per_plain.answer.held_back_named(), held(&["mean_age"]));
    }
}
