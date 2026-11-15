// SPDX-License-Identifier: AGPL-3.0-only

//! How far apart dates are. Three things a draft of "subjects whose visits
//! are at least so many years apart" met:
//!
//! - a group's value made of two totals of its child, the days between the
//!   latest and the earliest date, was refused: it is now the two totals
//!   bound by themselves and the value made of them, and answers what that
//!   form answers;
//! - "five years" had only days to be said in: `months_between` and
//!   `years_between` count whole calendar months and years, the same on
//!   both backends, and are held here against the calendar;
//! - a difference written with the earlier date first kept nothing and
//!   nothing said why: a diagnosis warns of it, and only of it.

mod common;

use std::collections::BTreeMap;

use common::{Lab, ask_of, quasi};
use nils_ask::diagnose::{Diagnosis, diagnose, diagnose_beside, diagnose_by_stage};
use nils_ask::exec::Bounds;
use nils_ask::validate::Code;
use nils_ask::{parse, prepare};
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_date_gaps_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

fn doc(sets: &Value, out: Value) -> String {
    json!({"ast_version": 1, "name": "how far apart", "sets": sets, "out": out}).to_string()
}

/// The rows of a set, each as its columns by name.
fn listed(
    l: &mut Lab,
    sets: &Value,
    set: &str,
    level: &str,
    columns: &[&str],
) -> Vec<BTreeMap<String, Value>> {
    let cols: Vec<Value> = columns.iter().map(|c| json!(["field", {}, c])).collect();
    let (_, a) = ask_of(
        l,
        &doc(sets, json!({"set": set, "level": level, "columns": cols})),
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

/// The rows a set answers at `level: count`.
fn count(l: &mut Lab, sets: &Value, set: &str) -> i64 {
    let (_, a) = ask_of(l, &doc(sets, json!({"set": set, "level": "count"})));
    a.rows[0].opt_int(0).unwrap().unwrap()
}

/// A day: its year, month and day of the month.
type Day = (i64, i64, i64);

/// A day as written, `YYYY-MM-DD` at the head of the text.
fn ymd(v: &Value) -> (i64, i64, i64) {
    let t = v.as_str().unwrap_or_else(|| panic!("{v} is no date"));
    let n = |a: usize, b: usize| t[a..b].parse::<i64>().unwrap();
    (n(0, 4), n(5, 7), n(8, 10))
}

/// The day's number, counted from 1970-01-01.
fn day_number((y, m, d): (i64, i64, i64)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The whole calendar months from `b` to `a`.
fn months(a: (i64, i64, i64), b: (i64, i64, i64)) -> i64 {
    (a.0 * 12 + a.1) - (b.0 * 12 + b.1) - i64::from(a.2 < b.2)
}

/// The whole calendar years from `b` to `a`.
fn years(a: (i64, i64, i64), b: (i64, i64, i64)) -> i64 {
    a.0 - b.0 - i64::from((a.1, a.2) < (b.1, b.2))
}

/// Every subject's sessions, and the spans of their days a group makes:
/// with the two totals bound by themselves (the form that always worked),
/// and in one clause.
fn spans(bound: i64) -> Value {
    let first = json!(["field", {}, "first"]);
    json!({
        "visits": {"grain": "session"},
        "by_fields": {"grain": "group",
            "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
            "bind": {
                "first_day": ["min", {"set": "visits"}, first],
                "last_day": ["max", {"set": "visits"}, first],
                "span_days": ["days_between", {}, ["field", {}, "last_day"], ["field", {}, "first_day"]]
            },
            "where": [[">=", {}, ["field", {}, "span_days"], bound]]},
        "in_one": {"grain": "group",
            "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
            "bind": {
                "span_days": ["days_between", {}, ["max", {"set": "visits"}, first], ["min", {"set": "visits"}, first]]
            },
            "where": [[">=", {}, ["field", {}, "span_days"], bound]]},
        "in_where": {"grain": "group",
            "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
            "where": [[">=", {},
                ["days_between", {}, ["max", {"set": "visits"}, first], ["min", {"set": "visits"}, first]],
                bound]]}
    })
}

/// The span of each subject's sessions, from the sessions themselves.
fn spans_of(l: &mut Lab) -> BTreeMap<String, (Day, Day)> {
    let sets = json!({"visits": {"grain": "session"}});
    let mut out: BTreeMap<String, (Day, Day)> = BTreeMap::new();
    for r in listed(l, &sets, "visits", "record", &["subject.id", "first"]) {
        let day = ymd(&r["first"]);
        let e = out.entry(r["subject.id"].to_string()).or_insert((day, day));
        e.0 = e.0.min(day);
        e.1 = e.1.max(day);
    }
    out
}

#[test]
fn a_group_binds_the_days_between_two_of_its_totals() {
    for mut l in labs() {
        let truth = spans_of(&mut l);
        let days: BTreeMap<String, i64> = truth
            .iter()
            .map(|(s, (a, b))| (s.clone(), day_number(*b) - day_number(*a)))
            .collect();
        // a bound that keeps some subjects and drops others
        let mut sorted: Vec<i64> = days.values().copied().collect();
        sorted.sort_unstable();
        let bound = sorted[sorted.len() / 2].max(1);
        let kept = days.values().filter(|d| **d >= bound).count();
        assert!(
            kept > 0 && kept < days.len(),
            "{}: {kept} of {} subjects span {bound} days",
            l.name,
            days.len()
        );
        let sets = spans(bound);
        for set in ["by_fields", "in_one"] {
            let rows = listed(
                &mut l,
                &sets,
                set,
                "aggregate",
                &["subject.id", "span_days"],
            );
            assert_eq!(rows.len(), kept, "{}: {set}", l.name);
            for r in &rows {
                assert_eq!(
                    r["span_days"],
                    json!(days[&r["subject.id"].to_string()]),
                    "{}: {set}",
                    l.name
                );
            }
        }
        for set in ["by_fields", "in_one", "in_where"] {
            assert_eq!(count(&mut l, &sets, set), kept as i64, "{}: {set}", l.name);
        }
    }
}

/// The document as it is stored says the totals as bindings of their own,
/// and saying it again changes nothing.
#[test]
fn the_totals_inside_a_value_are_bindings_of_the_stored_document() {
    let l = common::lab(
        "sqlite",
        nils_registry::Backend::Sqlite,
        None,
        "nils_date_gaps_test",
    );
    let text = doc(&spans(30), json!({"set": "in_one", "level": "count"}));
    let p = prepare(parse(&text).unwrap(), &l.catalog, &quasi()).unwrap();
    let names: Vec<&str> = p.ask.sets["in_one"].bind.names().collect();
    assert_eq!(names, ["span_days__max", "span_days__min", "span_days"]);
    let names: Vec<&str> = p.ask.sets["in_where"].bind.names().collect();
    assert_eq!(names, ["where1__max", "where1__min"]);
    let mut again = p.ask.clone();
    nils_ask::sugar::desugar(&mut again).unwrap();
    assert_eq!(again, p.ask);
    // a total of another set is not the group's to make, and says what is
    let other = json!({
        "visits": {"grain": "session"},
        "scans": {"grain": "stack"},
        "g": {"grain": "group",
            "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
            "bind": {"n": ["+", {}, ["count", {"set": "scans"}], 1]}}
    });
    let text = doc(&other, json!({"set": "g", "level": "count"}));
    let refused = prepare(parse(&text).unwrap(), &l.catalog, &quasi())
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    // and it is refused where it was written, not at a binding made for it
    assert!(
        refused.contains("sets.g.bind.n:") || refused.contains("sets.g.bind.n "),
        "{refused}"
    );
    assert!(!refused.is_empty(), "a total of another set is refused");
}

#[test]
fn whole_months_and_years_are_the_calendars_on_both_backends() {
    // from the second date to the first: months, years
    let cases: [(&str, &str, i64, i64); 9] = [
        ("2021-03-01", "2021-01-31", 1, 0),
        ("2021-02-28", "2021-01-31", 0, 0),
        ("2021-01-31", "2021-01-31", 0, 0),
        ("2025-02-28", "2020-02-29", 59, 4),
        ("2025-03-01", "2020-02-29", 60, 5),
        ("2026-06-15", "2021-06-15", 60, 5),
        ("2026-06-14", "2021-06-15", 59, 4),
        // the earlier date first: below nought
        ("2021-06-15", "2026-06-15", -60, -5),
        ("2021-01-31", "2021-03-01", -2, -1),
    ];
    for mut l in labs() {
        let mut bind = serde_json::Map::new();
        for (i, (a, b, ..)) in cases.iter().enumerate() {
            bind.insert(format!("m{i}"), json!(["months_between", {}, a, b]));
            bind.insert(format!("y{i}"), json!(["years_between", {}, a, b]));
        }
        let names: Vec<String> = bind.keys().cloned().collect();
        let sets = json!({"people": {"grain": "subject", "bind": bind}});
        let columns: Vec<&str> = names.iter().map(String::as_str).collect();
        let rows = listed(&mut l, &sets, "people", "record", &columns);
        let r = &rows[0];
        for (i, (a, b, m, y)) in cases.iter().enumerate() {
            assert_eq!(
                r[&format!("m{i}")],
                json!(m),
                "{}: months {b} to {a}",
                l.name
            );
            assert_eq!(
                r[&format!("y{i}")],
                json!(y),
                "{}: years {b} to {a}",
                l.name
            );
        }
        // over the registry's own days: each session after a subject's
        // first, from the session before it
        let first = json!(["field", {}, "first"]);
        let before = json!(["prev", {}, first]);
        let sets = json!({"visits": {"grain": "session", "bind": {
            "before": before,
            "gap_days": ["days_between", {}, first, before],
            "gap_months": ["months_between", {}, first, before],
            "gap_years": ["years_between", {}, first, before]
        }}});
        let rows = listed(
            &mut l,
            &sets,
            "visits",
            "record",
            &["first", "before", "gap_days", "gap_months", "gap_years"],
        );
        let mut later = 0;
        for r in &rows {
            if r["before"].is_null() {
                assert!(r["gap_months"].is_null(), "{}", l.name);
                continue;
            }
            later += 1;
            let (a, b) = (ymd(&r["first"]), ymd(&r["before"]));
            assert_eq!(
                r["gap_days"],
                json!(day_number(a) - day_number(b)),
                "{}",
                l.name
            );
            assert_eq!(r["gap_months"], json!(months(a, b)), "{}: {r:?}", l.name);
            assert_eq!(r["gap_years"], json!(years(a, b)), "{}: {r:?}", l.name);
        }
        assert!(later > 10, "{}: {later} sessions after a first", l.name);
        // "at least so many months apart" keeps what the calendar keeps
        let apart = rows
            .iter()
            .filter(|r| r["gap_months"].as_i64().is_some_and(|m| m >= 6))
            .count();
        let mut filtered = sets.clone();
        filtered["visits"]["where"] = json!([[">=", {}, ["field", {}, "gap_months"], 6]]);
        assert_eq!(
            count(&mut l, &filtered, "visits"),
            apart as i64,
            "{}",
            l.name
        );
    }
}

/// The three ways a diagnosis is counted.
fn diagnosed(l: &mut Lab, text: &str) -> Vec<Diagnosis> {
    let scheme = Scheme::default();
    let ask = parse(text).unwrap();
    let mut beside: Vec<nils_registry::Store> = (0..2)
        .map(|_| l.registry.open_ask_reader(None).unwrap())
        .collect();
    vec![
        diagnose(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &quasi(),
            &scheme,
            bounds(),
            false,
            None,
        )
        .unwrap(),
        diagnose_by_stage(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &quasi(),
            &scheme,
            bounds(),
            false,
            None,
            None,
        )
        .unwrap(),
        diagnose_beside(
            &mut l.registry,
            ask,
            Vec::new(),
            &l.catalog,
            &quasi(),
            &scheme,
            bounds(),
            false,
            None,
            None,
            &mut beside,
        )
        .unwrap(),
    ]
}

/// The warnings of a diagnosis about dates the wrong way round, by path.
fn reversed(d: &Diagnosis) -> Vec<(String, String, String)> {
    d.warnings
        .iter()
        .filter(|w| w.code == Code::ReversedDates)
        .map(|w| (w.path.clone(), w.message.clone(), w.next.clone()))
        .collect()
}

#[test]
fn a_gap_with_the_earlier_date_first_is_warned_of() {
    let first = json!(["field", {}, "first"]);
    let before = json!(["prev", {}, first]);
    // consecutive sessions at least a day apart, three ways
    let visits = |a: &Value, b: &Value, bound: i64| {
        json!({"visits": {"grain": "session",
            "bind": {"before": before, "gap": ["days_between", {}, a, b]},
            "where": [[">=", {}, ["field", {}, "gap"], bound]]}})
    };
    let before_field = json!(["field", {}, "before"]);
    let group = |later: &str, earlier: &str, function: &str| {
        json!({
            "visits": {"grain": "session"},
            "spans": {"grain": "group",
                "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
                "bind": {"span": [function, {},
                    [later, {"set": "visits"}, first], [earlier, {"set": "visits"}, first]]},
                "where": [[">=", {}, ["field", {}, "span"], 1]]}
        })
    };
    for mut l in labs() {
        let out = |set: &str| json!({"set": set, "level": "count"});
        // the right way round keeps rows and warns of nothing
        let right = doc(&visits(&first, &before_field, 1), out("visits"));
        assert!(count(&mut l, &visits(&first, &before_field, 1), "visits") > 0);
        for d in diagnosed(&mut l, &right) {
            assert!(d.valid, "{}", l.name);
            assert_eq!(reversed(&d), [], "{}", l.name);
        }
        // the earlier first keeps none, and says which way round
        let wrong = doc(&visits(&before_field, &first, 1), out("visits"));
        for d in diagnosed(&mut l, &wrong) {
            let w = reversed(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.visits.where[0]", "{}", l.name);
            assert!(
                w[0].1
                    .ends_with("days_between(a, b) is a minus b: put the later date first"),
                "{}: {}",
                l.name,
                w[0].1
            );
            assert_eq!(
                w[0].2, "swap the two dates of the days_between at sets.visits.bind.gap",
                "{}",
                l.name
            );
            assert!(d.zero_rows.is_some(), "{}", l.name);
        }
        // a bound no gap reaches keeps none either way: nothing to warn of
        let far = doc(&visits(&before_field, &first, 1_000_000), out("visits"));
        for d in diagnosed(&mut l, &far) {
            assert_eq!(reversed(&d), [], "{}", l.name);
        }
        // written in the filter itself
        let inline = json!({"visits": {"grain": "session",
            "bind": {"before": before},
            "where": [["<=", {}, 1, ["days_between", {}, before_field, first]]]}});
        for d in diagnosed(&mut l, &doc(&inline, out("visits"))) {
            let w = reversed(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(
                w[0].2, "swap the two dates of the days_between at sets.visits.where[0]",
                "{}",
                l.name
            );
        }
        // a group's span, which the funnel has no stage for
        for function in ["days_between", "months_between", "years_between"] {
            let d = diagnosed(&mut l, &doc(&group("max", "min", function), out("spans")));
            assert!(d.iter().all(|d| reversed(d).is_empty()), "{}", l.name);
            if count(&mut l, &group("max", "min", function), "spans") == 0 {
                // no subject of this registry spans a whole unit
                continue;
            }
            for d in diagnosed(&mut l, &doc(&group("min", "max", function), out("spans"))) {
                let w = reversed(&d);
                assert_eq!(w.len(), 1, "{}: {function}: {:?}", l.name, d.warnings);
                assert_eq!(w[0].0, "sets.spans.where[0]", "{}", l.name);
                assert!(
                    w[0].1.contains(&format!("{function}(a, b) is a minus b")),
                    "{}: {}",
                    l.name,
                    w[0].1
                );
                assert_eq!(
                    w[0].2,
                    format!("swap the two dates of the {function} at sets.spans.bind.span"),
                    "{}",
                    l.name
                );
            }
        }
    }
}

/// The two documents the spec writes (section 5.5), as YAML, with bounds
/// this registry's days reach: first to last, and one session to the next.
#[test]
fn the_documents_of_the_spec_answer_what_the_calendar_says() {
    const FIRST_TO_LAST: &str = r#"
ast_version: 1
sets:
  visits: {grain: session}
  spans:
    grain: group
    group: {of: visits, by: [["field", {}, "subject.id"]]}
    bind:
      span_years: ["years_between", {}, ["max", {set: visits}, ["field", {}, "first"]], ["min", {set: visits}, ["field", {}, "first"]]]
    where:
      - [">=", {}, ["field", {}, "span_years"], 1]
out: {set: spans, level: count}
"#;
    const ONE_TO_THE_NEXT: &str = r#"
ast_version: 1
sets:
  visits:
    grain: session
    bind:
      before: ["prev", {}, ["field", {}, "first"]]
      gap_months: ["months_between", {}, ["field", {}, "first"], ["field", {}, "before"]]
    where:
      - [">=", {}, ["field", {}, "gap_months"], 6]
out: {set: visits, level: count}
"#;
    for mut l in labs() {
        let truth = spans_of(&mut l);
        let spanning = truth.values().filter(|(a, b)| years(*b, *a) >= 1).count();
        assert!(spanning > 0, "{}", l.name);
        let (_, a) = ask_of(&mut l, FIRST_TO_LAST);
        assert_eq!(
            a.rows[0].opt_int(0).unwrap(),
            Some(spanning as i64),
            "{}",
            l.name
        );
        // the sessions of each subject in order, each from the one before
        let sets = json!({"visits": {"grain": "session"}});
        let mut days: BTreeMap<String, Vec<(i64, i64, i64)>> = BTreeMap::new();
        for r in listed(&mut l, &sets, "visits", "record", &["subject.id", "first"]) {
            days.entry(r["subject.id"].to_string())
                .or_default()
                .push(ymd(&r["first"]));
        }
        let apart: usize = days
            .values_mut()
            .map(|d| {
                d.sort_unstable();
                d.windows(2).filter(|w| months(w[1], w[0]) >= 6).count()
            })
            .sum();
        assert!(apart > 0, "{}", l.name);
        let (_, a) = ask_of(&mut l, ONE_TO_THE_NEXT);
        assert_eq!(
            a.rows[0].opt_int(0).unwrap(),
            Some(apart as i64),
            "{}",
            l.name
        );
    }
}
