// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 K7, small counts at plain (the ruling of 2026-10-12), end to
//! end on each backend. At detail plain a count under 5 subjects of rows a
//! quasi identifying field chose or grouped is held back (null, and named
//! with why), and so is a count beside it, so that a total less the counts
//! shown never says it; counts by technical or clinical fields stay exact,
//! and detail quasi sees every count. The chat bench found it: a plain
//! reader rebuilt ages as counts per age band, one band holding a single
//! subject.
//!
//! The synthetic registry's subjects with an age at their first visit fall
//! into three ten-year bands: 8 from 25 to 34, 34 from 35 to 44, 2 from 45.

mod common;

use std::collections::BTreeMap;

use common::Lab;
use nils_ask::affordance::{self, Setting};
use nils_ask::diagnose;
use nils_ask::exec::{Bounds, HeldBack};
use nils_ask::run::{Outcome, Request, RunError, run};
use nils_ask::validate::{MEASURE_K, Scope, held_count_why};
use nils_ask::{handle, parse};
use nils_registry::session::Scheme;
use nils_registry::store::{Cell, Row};

fn labs() -> Vec<Lab> {
    common::labs("nils_small_counts_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// Every subject, their visits with the age at each and its ten-year band,
/// and each subject's first visit where the age is known.
const SETS: &str = r#"
  people: {grain: subject}
  visits:
    grain: session
    of: people
    bind:
      n:    ["ordinal", {}]
      age:  ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]
      band: ["+", {}, ["*", {}, ["round", {"key": true}, ["/", {}, ["-", {}, ["field", {}, "age"], 9.5], 10], 0], 10], 5]
  firsts:
    grain: session
    from: visits
    where:
      - ["=", {}, ["field", {}, "n"], 1]
      - ["not_null", {}, ["field", {}, "age"]]
"#;

fn document(more: &str, out: &str) -> String {
    format!("ast_version: 1\nname: small counts\nscheme: default\nsets:{SETS}{more}\nout:\n{out}\n")
}

/// The bench's question: the subjects per band, their first sessions, and
/// each band's share of every subject.
fn bands() -> String {
    document(
        r#"  bands:
    grain: group
    group: {of: firsts, by: [["field", {}, "band"]]}
    bind:
      sessions: ["count", {set: firsts}]
      twice:    ["*", {}, ["field", {}, "_subjects"], 2]"#,
        r#"  set: bands
  level: aggregate
  columns:
    - ["field", {}, "band"]
    - ["field", {}, "_subjects"]
    - ["field", {}, "sessions"]
    - ["field", {}, "twice"]
  measures:
    - {share: {of: "_subjects", over: people}}
  order: [[["field", {}, "band"], asc]]"#,
    )
}

/// A count of the first visits at an age of `from` or more.
fn aged_from(from: i64, level: &str) -> String {
    document(
        &format!(
            r#"  older:
    grain: session
    from: firsts
    where: [[">=", {{}}, ["field", {{}}, "age"], {from}]]"#
        ),
        &format!("  set: older\n  level: {level}"),
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

fn column<'a>(out: &'a Outcome, name: &str) -> Vec<&'a Cell> {
    let i = out
        .answer
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no column {name}: {:?}", out.answer.columns));
    out.answer.rows.iter().map(|r: &Row| &r.0[i]).collect()
}

fn ints(cells: Vec<&Cell>) -> Vec<Option<i64>> {
    cells
        .into_iter()
        .map(|c| match c {
            Cell::Int(i) => Some(*i),
            Cell::Null => None,
            other => panic!("not a count: {other:?}"),
        })
        .collect()
}

fn counts_held(names: &[&str]) -> Vec<HeldBack> {
    names
        .iter()
        .map(|n| HeldBack {
            column: n.to_string(),
            why: held_count_why(),
        })
        .collect()
}

fn plain() -> Scope {
    Scope::default()
}

#[test]
fn a_band_under_five_is_held_with_the_smallest_beside_it_on_both_backends() {
    for mut l in labs() {
        assert_eq!(
            held_count_why(),
            "fewer than 5, or held beside a count of fewer than 5 so that it is not found by subtraction"
        );
        // at detail quasi: every count
        let raw = ok(&mut l, &bands(), &common::quasi());
        assert!(raw.answer.held_back.is_empty(), "{}", l.name);
        assert_eq!(
            ints(column(&raw, "_subjects")),
            vec![Some(8), Some(34), Some(2)],
            "{}",
            l.name
        );
        assert_eq!(
            ints(column(&raw, "sessions")),
            vec![Some(8), Some(34), Some(2)]
        );

        // at detail plain: the band of 2 is held back, and the band of 8,
        // the smallest beside it, so that the 44 subjects with an age less
        // the 34 shown leave 10 between two bands; what is made of a count
        // (twice it, its share) is held with it
        let at_plain = ok(&mut l, &bands(), &plain());
        assert_eq!(at_plain.answer.shaped_names(), vec!["band"], "{}", l.name);
        assert_eq!(
            at_plain.answer.held_back_named(),
            counts_held(&["_subjects", "sessions", "twice", "share._subjects"]),
            "{}",
            l.name
        );
        for name in ["_subjects", "sessions"] {
            assert_eq!(
                ints(column(&at_plain, name)),
                vec![None, Some(34), None],
                "{}: {name}",
                l.name
            );
        }
        assert_eq!(ints(column(&at_plain, "twice")), vec![None, Some(68), None]);
        let shares = column(&at_plain, "share._subjects");
        assert!(matches!(shares[0], Cell::Null) && matches!(shares[2], Cell::Null));
        assert_eq!(shares[1], column(&raw, "share._subjects")[1]);

        // the handle holds the nulls and says why, on its pages too
        let h = handle::get(l.registry.store(), at_plain.handle.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            h.held_back(),
            counts_held(&["_subjects", "sessions", "twice"]),
            "{}",
            l.name
        );
        let page = handle::page(l.registry.store(), h.id, 0).unwrap().unwrap();
        let at = h
            .columns
            .iter()
            .position(|c| c.name == "_subjects")
            .unwrap();
        assert_eq!(page[0][at], serde_json::Value::Null);
        assert_eq!(page[1][at], serde_json::json!(34));

        // the preview says the same
        let scheme = Scheme::default();
        let scope = plain();
        let setting = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "tester",
            bounds: bounds(),
            values_cap: 50,
        };
        let previewed = affordance::preview(
            &mut l.registry,
            &parse(&bands()).unwrap(),
            10,
            &setting,
            None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(
            previewed.held_back,
            counts_held(&["_subjects", "sessions", "twice"])
        );

        // a page of one row is held as the whole answer is: the decision is
        // made over every group, never over the rows a page shows
        let scheme = Scheme::default();
        let paged = run(
            &mut l.registry,
            Request {
                ask: parse(&bands()).unwrap(),
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
                after: Some(1),
                limit: Some(1),
                may_project_raw: false,
                purpose: None,
                reader: None,
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(
            ints(column(&paged, "_subjects")),
            vec![Some(34)],
            "{}",
            l.name
        );
    }
}

#[test]
fn a_count_an_age_chose_is_held_under_five_and_answered_from_five() {
    for mut l in labs() {
        let quasi = common::quasi();
        // 2 subjects of 45 or older: a number at quasi, held back at plain
        let few = aged_from(45, "count");
        let raw = ok(&mut l, &few, &quasi);
        assert_eq!(ints(column(&raw, "subjects")), vec![Some(2)], "{}", l.name);
        let held = ok(&mut l, &few, &plain());
        assert_eq!(ints(column(&held, "rows")), vec![None], "{}", l.name);
        assert_eq!(ints(column(&held, "subjects")), vec![None]);
        assert_eq!(
            held.answer.held_back_named(),
            counts_held(&["rows", "subjects"])
        );
        // whether any is: held between none and 5
        let any = ok(&mut l, &aged_from(45, "boolean"), &plain());
        assert_eq!(ints(column(&any, "any")), vec![None], "{}", l.name);
        assert_eq!(
            ints(column(
                &ok(&mut l, &aged_from(45, "boolean"), &quasi),
                "any"
            )),
            vec![Some(1)]
        );
        // 36 subjects of 35 or older: a number at plain
        let many = ok(&mut l, &aged_from(35, "count"), &plain());
        assert_eq!(
            ints(column(&many, "subjects")),
            vec![Some(36)],
            "{}",
            l.name
        );
        assert!(many.answer.held_back.is_empty());
        // none: answered, as a zero says no one
        let none = ok(&mut l, &aged_from(200, "count"), &plain());
        assert_eq!(ints(column(&none, "subjects")), vec![Some(0)], "{}", l.name);
        assert_eq!(
            ints(column(
                &ok(&mut l, &aged_from(200, "boolean"), &plain()),
                "any"
            )),
            vec![Some(0)]
        );
    }
}

#[test]
fn counts_by_technical_or_clinical_fields_stay_exact_at_plain() {
    for mut l in labs() {
        // the visits per visit number: the last number has 3 subjects, and
        // the visit number is no quasi identifying field
        let by_number = document(
            r#"  by_number:
    grain: group
    group: {of: visits, by: [["field", {}, "n"]]}"#,
            r#"  set: by_number
  level: aggregate
  columns: [["field", {}, "n"], ["field", {}, "_subjects"]]
  order: [[["field", {}, "n"], asc]]"#,
        );
        let raw = ok(&mut l, &by_number, &common::quasi());
        let at_plain = ok(&mut l, &by_number, &plain());
        assert_eq!(
            column(&at_plain, "_subjects"),
            column(&raw, "_subjects"),
            "{}",
            l.name
        );
        assert!(
            ints(column(&at_plain, "_subjects"))
                .iter()
                .any(|n| n.is_some_and(|n| n < MEASURE_K)),
            "{}",
            l.name
        );
        assert!(at_plain.answer.held_back.is_empty(), "{}", l.name);
        // a count filtered by the visit number alone
        let thirds = document(
            r#"  thirds:
    grain: session
    from: visits
    where: [["=", {}, ["field", {}, "n"], 6]]"#,
            "  set: thirds\n  level: count",
        );
        let few = ok(&mut l, &thirds, &plain());
        assert_eq!(ints(column(&few, "subjects")), vec![Some(3)], "{}", l.name);
        assert!(few.answer.held_back.is_empty());
    }
}

#[test]
fn a_count_a_group_keeps_of_a_quasi_field_is_held_with_its_complement() {
    for mut l in labs() {
        // the women per cohort: the sex is quasi identifying; one cohort's
        // women leave fewer than 5 others, so its count of women is held,
        // and the other cohort's beside it; the cohort's own count stays
        let women = r#"ast_version: 1
name: women per cohort
scheme: default
sets:
  scope: {grain: cohort}
  people:
    grain: subject
    of: scope
    bind:
      female: ["case", {}, ["=", {}, ["field", {}, "sex"], "F"], 1, 0]
  by_cohort:
    grain: group
    group: {of: people, by: [["field", {}, "cohort.id"]]}
    bind:
      n_female: ["sum", {set: people}, ["field", {}, "female"]]
      women:    ["count", {set: people}, ["case", {}, ["=", {}, ["field", {}, "sex"], "F"], 1]]
out:
  set: by_cohort
  level: aggregate
  columns:
    - ["field", {}, "cohort.id"]
    - ["field", {}, "_subjects"]
    - ["field", {}, "n_female"]
    - ["field", {}, "women"]
  order: [[["field", {}, "cohort.id"], asc]]
"#;
        let raw = ok(&mut l, women, &common::quasi());
        let at_plain = ok(&mut l, women, &plain());
        let subjects = ints(column(&raw, "_subjects"));
        let female = ints(column(&raw, "n_female"));
        assert_eq!(ints(column(&raw, "women")), female, "{}", l.name);
        // some cohort's women or others are under 5, so at least two are held
        let small = subjects.iter().zip(&female).any(|(n, f)| {
            let (n, f) = (n.unwrap(), f.unwrap());
            (1..MEASURE_K).contains(&f) || (1..MEASURE_K).contains(&(n - f))
        });
        assert!(small, "{}: {subjects:?} {female:?}", l.name);
        assert_eq!(column(&at_plain, "_subjects"), column(&raw, "_subjects"));
        let held: Vec<Option<i64>> = ints(column(&at_plain, "n_female"));
        assert!(
            held.iter().filter(|c| c.is_none()).count() >= 2,
            "{}: {held:?}",
            l.name
        );
        assert_eq!(ints(column(&at_plain, "women")), held, "{}", l.name);
        assert_eq!(
            at_plain.answer.held_back_named(),
            counts_held(&["n_female", "women"])
        );
    }
}

#[test]
fn a_group_of_two_keys_holds_no_count_alone_in_a_line() {
    for mut l in labs() {
        // the subjects per visit number and band: in every line of groups
        // (one visit number's bands, one band's visit numbers) a held count
        // is never alone
        let two = document(
            r#"  by_two:
    grain: group
    group: {of: visits, by: [["field", {}, "n"], ["field", {}, "band"]]}"#,
            r#"  set: by_two
  level: aggregate
  columns: [["field", {}, "n"], ["field", {}, "band"], ["field", {}, "_subjects"]]"#,
        );
        let raw = ok(&mut l, &two, &common::quasi());
        let at_plain = ok(&mut l, &two, &plain());
        let numbers: Vec<String> = column(&raw, "n")
            .iter()
            .map(|c| nils_ask::exec::render(c))
            .collect();
        let bands: Vec<String> = column(&raw, "band")
            .iter()
            .map(|c| nils_ask::exec::render(c))
            .collect();
        let raw_n = ints(column(&raw, "_subjects"));
        let held = ints(column(&at_plain, "_subjects"));
        assert_eq!(raw_n.len(), held.len(), "{}", l.name);
        let mut primaries = 0;
        for (i, n) in raw_n.iter().enumerate() {
            let n = n.unwrap();
            if n < MEASURE_K {
                primaries += 1;
                assert_eq!(held[i], None, "{}: row {i}", l.name);
            } else if let Some(shown) = held[i] {
                assert_eq!(shown, n);
            }
        }
        assert!(primaries > 0, "{}", l.name);
        // per line, never exactly one held where the line has another group
        let mut lines: BTreeMap<(usize, String), Vec<Option<i64>>> = BTreeMap::new();
        for i in 0..held.len() {
            lines
                .entry((0, bands[i].clone()))
                .or_default()
                .push(held[i]);
            lines
                .entry((1, numbers[i].clone()))
                .or_default()
                .push(held[i]);
        }
        for (line, cells) in &lines {
            let n_held = cells.iter().filter(|c| c.is_none()).count();
            assert!(
                n_held != 1 || cells.len() == 1,
                "{}: line {line:?} holds one alone: {cells:?}",
                l.name
            );
        }
    }
}

#[test]
fn a_share_over_a_few_subjects_an_age_chose_is_held() {
    for mut l in labs() {
        // the bands of everyone with a known age, each a share of the 2
        // subjects of 45 or older: the denominator would say 2
        let over_few = document(
            r#"  older:
    grain: session
    from: firsts
    where: [[">=", {}, ["field", {}, "age"], 45]]
  by_n:
    grain: group
    group: {of: visits, by: [["field", {}, "n"]]}"#,
            r#"  set: by_n
  level: aggregate
  columns: [["field", {}, "n"], ["field", {}, "_subjects"]]
  measures:
    - {share: {of: "_subjects", over: older}}
  order: [[["field", {}, "n"], asc]]"#,
        );
        let raw = ok(&mut l, &over_few, &common::quasi());
        assert!(
            column(&raw, "share._subjects")
                .iter()
                .all(|c| matches!(c, Cell::Double(_))),
            "{}",
            l.name
        );
        let at_plain = ok(&mut l, &over_few, &plain());
        assert!(
            column(&at_plain, "share._subjects")
                .iter()
                .all(|c| matches!(c, Cell::Null)),
            "{}",
            l.name
        );
        assert_eq!(column(&at_plain, "_subjects"), column(&raw, "_subjects"));
        assert_eq!(
            at_plain.answer.held_back_named(),
            counts_held(&["share._subjects"])
        );
    }
}

#[test]
fn the_funnel_holds_a_stage_an_age_chose_of_few_or_that_dropped_few() {
    for mut l in labs() {
        let scheme = Scheme::default();
        let few = parse(&aged_from(45, "count")).unwrap();
        let diagnosed = |l: &mut Lab, scope: &Scope, keys: bool| {
            diagnose::diagnose(
                &mut l.registry,
                few.clone(),
                Vec::new(),
                &l.catalog,
                scope,
                &scheme,
                bounds(),
                keys,
                None,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", l.name))
        };
        // a set's stages in order: its source, then each clause
        let stages = |d: &diagnose::Diagnosis, set: &str| -> Vec<diagnose::Stage> {
            d.funnel.iter().filter(|s| s.set == set).cloned().collect()
        };
        let raw = diagnosed(&mut l, &common::quasi(), false);
        assert_eq!(stages(&raw, "older")[1].subjects, Some(2), "{}", l.name);
        assert_eq!(stages(&raw, "firsts")[2].subjects, Some(44));
        assert!(raw.funnel.iter().all(|s| s.held_back.is_none()));

        let d = diagnosed(&mut l, &plain(), false);
        // the 2 subjects of 45 or older: fewer than 5
        let older = stages(&d, "older");
        assert_eq!(
            (older[1].rows, older[1].subjects),
            (None, None),
            "{}",
            l.name
        );
        assert_eq!(older[1].held_back, Some(held_count_why()));
        // the subjects with an age: 44, but 4 fewer than the 48 before, and
        // the set that narrows them starts from the same 44
        let firsts = stages(&d, "firsts");
        assert_eq!(firsts[2].subjects, None, "{}", l.name);
        assert_eq!(older[0].subjects, None, "{}", l.name);
        // the first visits, chosen by the visit number: 48 as they are
        assert_eq!(firsts[1].subjects, Some(48));
        assert_eq!(stages(&d, "people")[0].subjects, Some(48));
        // what a held stage lost is held with it
        let by_clause = d.clone().by_clause("older");
        assert!(
            by_clause
                .groups
                .iter()
                .filter(|g| g.set == "older" && g.group == "where")
                .all(|g| g.kept.is_none() && g.lost.is_none()),
            "{}: {:?}",
            l.name,
            by_clause.groups
        );
        // the answer's drop says none of it
        assert!(
            d.drops
                .iter()
                .all(|x| x.before.is_none() && x.after.is_none()),
            "{:?}",
            d.drops
        );
        // with the keys: none for a held stage
        let keyed = diagnosed(&mut l, &plain(), true);
        let older = stages(&keyed, "older");
        assert!(
            older[1].keys.is_empty() && older[1].subjects.is_none(),
            "{}",
            l.name
        );
        assert!(!stages(&keyed, "people")[0].keys.is_empty());
    }
}
