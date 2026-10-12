// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 K7 on aggregates, end to end on each backend (the chat sandbox
//! bench, 2026-10-12). The bench's grouped question, each subject's age at
//! the first visit and its spread over ms-cohort-a, runs on the synthetic
//! registry at detail plain and at detail quasi: below quasi the youngest,
//! the oldest and the mean age are their shapes, in the answer, the handle
//! and its pages, and the answer says which columns those are; the counts
//! are answered at both; a group of one and a group keyed by the age follow
//! the same rule; the median of the age is refused below quasi; a share of
//! the age, the age over a count the answer says, is a shape below quasi;
//! and the document as the bench wrote it, its column as an option, is
//! refused at both details with the form to write.

mod common;

use std::collections::BTreeSet;

use common::Lab;
use nils_ask::affordance::{self, Setting};
use nils_ask::exec::{Bounds, render};
use nils_ask::run::{Outcome, Request, RunError, run};
use nils_ask::validate::{Scope, shape};
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

#[test]
fn an_aggregate_of_a_quasi_field_follows_the_field_on_both_backends() {
    for mut l in labs() {
        let plain = Scope::default();
        let quasi = common::quasi();

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

        // at detail plain: the youngest, the oldest and the mean as their
        // shapes; the count of subjects and of distinct ages as numbers
        let held = ok(&mut l, &spread(), &plain);
        assert_eq!(
            held.answer.shaped_names(),
            vec!["youngest", "oldest", "mean_age"],
            "{}",
            l.name
        );
        for name in ["youngest", "oldest", "mean_age"] {
            let at_plain = column(&held, name)[0];
            assert_eq!(
                *at_plain,
                shaped(column(&raw, name)[0]),
                "{}: {name}",
                l.name
            );
            assert!(matches!(at_plain, Cell::Text(t) if t.chars().all(|c| c == '9' || c == '.')));
        }
        for name in ["n_subjects", "n_ages"] {
            assert_eq!(
                column(&held, name),
                column(&raw, name),
                "{}: {name}",
                l.name
            );
            assert!(matches!(column(&held, name)[0], Cell::Int(_)), "{}", l.name);
        }
        // the handle says which, and its pages hold the shapes alone
        let h = handle::get(l.registry.store(), held.handle.id)
            .unwrap()
            .unwrap();
        assert_eq!(h.shaped(), vec!["youngest", "oldest", "mean_age"]);
        assert!(
            h.columns
                .iter()
                .all(|c| c.shaped == ["youngest", "oldest", "mean_age"].contains(&c.name.as_str())),
            "{:?}",
            h.columns
        );
        let page = handle::page(l.registry.store(), h.id, 0).unwrap().unwrap();
        let at = |name: &str| h.columns.iter().position(|c| c.name == name).unwrap();
        for name in ["youngest", "oldest", "mean_age"] {
            assert_eq!(
                page[0][at(name)],
                handle::cell_json(&shaped(column(&raw, name)[0])),
                "{}: {name}",
                l.name
            );
        }
        // a handle answered raw says none
        let raw_handle = handle::get(l.registry.store(), raw.handle.id)
            .unwrap()
            .unwrap();
        assert!(raw_handle.shaped().is_empty());
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
        assert_eq!(previewed.shaped, vec!["youngest", "oldest", "mean_age"]);
        let previewed = affordance::preview(&mut l.registry, &ask, 10, &setting(&quasi), None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(previewed.shaped.is_empty());

        // a group of one, each subject's own first visit: its mean is its
        // age, a shape below quasi whatever the size of the group
        let each = document(
            r#"  each:
    grain: group
    group: {of: firsts, by: [["field", {}, "subject.id"]]}
    bind:
      mean_age: ["avg", {set: firsts}, ["field", {}, "age"]]
      visits:   ["count", {set: firsts}]"#,
            r#"  set: each
  level: aggregate
  columns: [["field", {}, "subject.id"], ["field", {}, "mean_age"], ["field", {}, "visits"]]
  order: [[["field", {}, "subject.id"], asc]]"#,
        );
        let one_raw = ok(&mut l, &each, &quasi);
        let one_plain = ok(&mut l, &each, &plain);
        assert_eq!(one_plain.answer.shaped_names(), vec!["mean_age"]);
        assert!(
            column(&one_raw, "visits")
                .iter()
                .all(|c| **c == Cell::Int(1))
        );
        let shapes: Vec<Cell> = column(&one_raw, "mean_age")
            .into_iter()
            .map(shaped)
            .collect();
        assert_eq!(
            column(&one_plain, "mean_age")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            shapes,
            "{}",
            l.name
        );
        assert_eq!(column(&one_plain, "visits"), column(&one_raw, "visits"));

        // a group keyed by the age: the key a shape, the counts answered
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
        assert_eq!(column(&key_plain, "n"), column(&key_raw, "n"));

        // the median of the age per group, which the post pass reads from
        // the child's rows: refused below quasi with the reason, answered
        // at quasi
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
