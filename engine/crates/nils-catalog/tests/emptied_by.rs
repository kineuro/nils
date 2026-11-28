// SPDX-License-Identifier: AGPL-3.0-only

//! A zero that says why.
//!
//! A count of nothing over a session set that reads several stack sets
//! answered `zero_rows: null`: the diagnosis looked for the answer set's
//! last stage among the stages sorted by their names, where `source` comes
//! after every `has`, and so read the rows the set started from. It reads
//! the last stage the funnel counted now, and a count of nothing names the
//! step that kept none as an empty list does.
//!
//! And where a stack set came to nothing because two of its clauses hold
//! together on no stack, one of them often written in a set it is drawn
//! from and not on the item itself, the diagnosis says which
//! (`emptied_by`): the set is counted with each other value clause left
//! out, one at a time, and where exactly one of them leaves rows, that
//! clause is warned of at its own path, with the clause the set came to
//! nothing at and the rows the set holds without it.
//!
//! The values are planted on each backend's synthetic registry, by the
//! place `i` of a stack among those a stack set sees:
//!
//! - `contrast_mix`: `T2like_T1` on one stack in twenty (`i % 20 == 0`),
//!   `PD_T2star` on the rest;
//! - `post_contrast`: `given` on the same one in twenty, `not_given` on the
//!   rest, so no stack holds `PD_T2star` and `given` together;
//! - `directory_type`: six values in turn, `anat` on `i % 6 == 4`;
//! - `base_basis`: `physics` on `i % 20 == 1`, which no `given` stack is.

mod common;

use common::{Lab, quasi, replant};
use nils_ask::diagnose::{
    Diagnosis, EMPTIED_CLAUSES, diagnose, diagnose_beside, diagnose_by_stage, time_ends_after,
};
use nils_ask::exec::Bounds;
use nils_ask::parse;
use nils_ask::validate::{Code, Scope};
use nils_registry::Store;
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_emptied_by_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// How many stacks a stack set sees, and how many of them hold `given` and
/// `anat` together.
fn plant(l: &mut Lab) -> (i64, i64) {
    const TYPES: [&str; 6] = ["dwi", "perf", "func", "fmap", "anat", "misc"];
    let stacks = common::stacks_seen(l).len();
    replant(l, "contrast_mix", |i| {
        Some(if i % 20 == 0 {
            "T2like_T1"
        } else {
            "PD_T2star"
        })
    });
    replant(l, "post_contrast", |i| {
        Some(if i % 20 == 0 { "given" } else { "not_given" })
    });
    replant(l, "directory_type", |i| Some(TYPES[i % 6]));
    replant(l, "base_basis", |i| (i % 20 == 1).then_some("physics"));
    let both = (0..stacks).filter(|i| i % 20 == 0 && i % 6 == 4).count();
    assert!(both >= 5, "{}: {both} of {stacks}", l.name);
    (stacks as i64, both as i64)
}

fn doc(sets: Value, out: &str, level: &str) -> String {
    json!({"ast_version": 1, "name": "emptied", "sets": sets, "out": {"set": out, "level": level}})
        .to_string()
}

fn is(axis: &str, value: &str) -> Value {
    json!(["=", {}, ["axis", {}, axis], value])
}

/// The three ways a diagnosis is counted, under one scope.
fn diagnosed(l: &mut Lab, text: &str, scope: &Scope) -> Vec<Diagnosis> {
    let scheme = Scheme::default();
    let ask = parse(text).unwrap();
    let mut beside: Vec<Store> = (0..2)
        .map(|_| l.registry.open_ask_reader(None).unwrap())
        .collect();
    vec![
        diagnose(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            scope,
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
            scope,
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
            scope,
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

/// The warnings of a diagnosis about a set two clauses emptied: path,
/// message, next.
fn emptied(d: &Diagnosis) -> Vec<(String, String, String)> {
    d.warnings
        .iter()
        .filter(|w| w.code == Code::EmptiedBy)
        .map(|w| (w.path.clone(), w.message.clone(), w.next.clone()))
        .collect()
}

/// The rows a document answers at `level: count`.
fn count(l: &mut Lab, text: &str) -> i64 {
    let (_, a) = common::ask_of(l, text);
    a.rows[0].opt_int(0).unwrap().unwrap()
}

#[test]
fn a_count_of_nothing_names_the_step_that_kept_none() {
    for mut l in labs() {
        plant(&mut l);
        // a session set that reads three stack sets, one of them empty
        let sets = json!({
            "plain": {"grain": "stack", "where": [is("contrast_mix", "PD_T2star")]},
            "given": {"grain": "stack", "where": [is("post_contrast", "given")]},
            "none": {"grain": "stack", "where": [
                is("contrast_mix", "PD_T2star"),
                is("post_contrast", "given")
            ]},
            "visits": {"grain": "session", "has": [
                {"set": "plain", "min": 1},
                {"set": "given", "min": 1},
                {"set": "none", "min": 1}
            ]}
        });
        let counted = doc(sets.clone(), "visits", "count");
        assert_eq!(count(&mut l, &counted), 0, "{}", l.name);
        let said = "no stack of none survives where post_contrast = given; everything after it \
                    is empty";
        for level in ["count", "record"] {
            let text = doc(sets.clone(), "visits", level);
            for d in diagnosed(&mut l, &text, &quasi()) {
                assert!(d.valid, "{}", l.name);
                assert_eq!(d.zero_rows.as_deref(), Some(said), "{} at {level}", l.name);
                assert!(
                    d.next.iter().any(|n| n.contains("/api/ask/options")),
                    "{}: {:?}",
                    l.name,
                    d.next
                );
            }
        }
        // the session set itself comes to nothing at one of its `has`
        let sets = json!({
            "plain": {"grain": "stack", "where": [is("contrast_mix", "PD_T2star")]},
            "given": {"grain": "stack", "where": [is("post_contrast", "given")]},
            "visits": {"grain": "session", "has": [
                {"set": "plain", "min": 1},
                {"set": "given", "min": 100000}
            ]}
        });
        let text = doc(sets, "visits", "count");
        assert_eq!(count(&mut l, &text), 0, "{}", l.name);
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert_eq!(
                d.zero_rows.as_deref(),
                Some("no session of visits survives has given; everything after it is empty"),
                "{}",
                l.name
            );
        }
        // an answer that holds rows says nothing of a zero
        let sets = json!({
            "plain": {"grain": "stack", "where": [is("contrast_mix", "PD_T2star")]},
            "visits": {"grain": "session", "has": [{"set": "plain", "min": 1}]}
        });
        let text = doc(sets, "visits", "count");
        assert!(count(&mut l, &text) > 0, "{}", l.name);
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert_eq!(d.zero_rows, None, "{}", l.name);
        }
    }
}

#[test]
fn a_clause_of_a_shared_set_that_empties_a_set_drawn_from_it_is_named() {
    for mut l in labs() {
        let (_, both) = plant(&mut l);
        // the item's own value holds on no stack the shared set keeps: its
        // `PD_T2star` and the item's `given` are never on one stack
        let sets = json!({
            "base": {"grain": "stack", "where": [
                is("contrast_mix", "PD_T2star"),
                is("directory_type", "anat")
            ]},
            "item": {"grain": "stack", "from": "base", "where": [is("post_contrast", "given")]},
            "visits": {"grain": "session", "has": [{"set": "item", "min": 1}]}
        });
        let text = doc(sets, "visits", "count");
        assert_eq!(count(&mut l, &text), 0, "{}", l.name);
        // what the item holds without the shared clause, counted by itself
        let without = doc(
            json!({"item": {"grain": "stack", "where": [
                is("directory_type", "anat"),
                is("post_contrast", "given")
            ]}}),
            "item",
            "count",
        );
        assert_eq!(count(&mut l, &without), both, "{}", l.name);
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert!(d.valid, "{}", l.name);
            assert_eq!(
                d.zero_rows.as_deref(),
                Some(
                    "no stack of item survives where post_contrast = given; everything after it \
                     is empty"
                ),
                "{}",
                l.name
            );
            let w = emptied(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.base.where[0]", "{}", l.name);
            assert_eq!(
                w[0].1,
                format!(
                    "item keeps no stack: with the set's other conditions, contrast_mix = \
                     PD_T2star (written in base, which item is drawn from) and post_contrast = \
                     given hold together on no stack. Without contrast_mix = PD_T2star, item \
                     holds {both} stacks."
                ),
                "{}",
                l.name
            );
            assert_eq!(
                w[0].2,
                "where the question's words did not put contrast_mix = PD_T2star on item, draft \
                 item on its own instead of from: base, with the clauses it needs and without \
                 that one; where they did, no stack holds both and the answer stands",
                "{}",
                l.name
            );
        }
    }
}

#[test]
fn a_clause_of_the_set_s_own_that_empties_it_is_named() {
    for mut l in labs() {
        let (stacks, _) = plant(&mut l);
        let given = (0..stacks).filter(|i| i % 20 == 0).count();
        let sets = json!({
            "item": {"grain": "stack", "where": [
                is("contrast_mix", "PD_T2star"),
                is("post_contrast", "given")
            ]}
        });
        let text = doc(sets, "item", "count");
        for d in diagnosed(&mut l, &text, &quasi()) {
            let w = emptied(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.item.where[0]", "{}", l.name);
            assert_eq!(
                w[0].1,
                format!(
                    "item keeps no stack: contrast_mix = PD_T2star and post_contrast = given \
                     hold together on no stack. Without contrast_mix = PD_T2star, item holds \
                     {given} stacks."
                ),
                "{}",
                l.name
            );
            assert_eq!(
                w[0].2,
                "where the question's words did not put contrast_mix = PD_T2star on item, leave \
                 sets.item.where[0] out; where they did, no stack holds both and the answer \
                 stands",
                "{}",
                l.name
            );
        }
    }
}

#[test]
fn nothing_is_named_where_no_one_clause_or_more_than_one_frees_the_set() {
    for mut l in labs() {
        plant(&mut l);
        let quiet = |l: &mut Lab, sets: Value, out: &str, why: &str| {
            let text = doc(sets, out, "count");
            assert_eq!(count(l, &text), 0, "{}: {why}", l.name);
            for d in diagnosed(l, &text, &quasi()) {
                assert!(d.valid, "{}: {why}", l.name);
                assert!(d.zero_rows.is_some(), "{}: {why}", l.name);
                assert_eq!(emptied(&d), [], "{}: {why}", l.name);
            }
        };
        // two clauses of the shared set each hold on no stack the item's
        // value is on: leaving either out alone frees nothing
        quiet(
            &mut l,
            json!({
                "base": {"grain": "stack", "where": [
                    is("contrast_mix", "PD_T2star"),
                    is("base_basis", "physics")
                ]},
                "item": {"grain": "stack", "from": "base", "where": [is("post_contrast", "given")]}
            }),
            "item",
            "no clause frees it",
        );
        // the set is empty before any clause of its own: the set it is
        // drawn from is the one that came to nothing, and that is the set
        // looked at; there it is the two that are named
        let text = doc(
            json!({
                "base": {"grain": "stack", "where": [
                    is("contrast_mix", "PD_T2star"),
                    is("post_contrast", "given")
                ]},
                "item": {"grain": "stack", "from": "base", "where": [is("directory_type", "anat")]}
            }),
            "item",
            "count",
        );
        for d in diagnosed(&mut l, &text, &quasi()) {
            let w = emptied(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.base.where[0]", "{}", l.name);
            assert!(w[0].1.starts_with("base keeps no stack: "), "{}", l.name);
        }
        // a set that came to nothing at a clause no stack meets at all
        quiet(
            &mut l,
            json!({"item": {"grain": "stack", "where": [
                is("directory_type", "anat"),
                is("contrast_mix", "intermediate_PD_T2")
            ]}}),
            "item",
            "a value no stack holds",
        );
    }
}

#[test]
fn more_than_one_clause_that_frees_the_set_names_none() {
    for mut l in labs() {
        plant(&mut l);
        // three conditions no stack meets together, each two of which some
        // stack does: `given` (one in twenty), `anat` (one in six), and a
        // value planted on the stacks that hold exactly one of the two
        replant(&mut l, "base_basis", |i| {
            ((i % 20 == 0) != (i % 6 == 4)).then_some("physics")
        });
        let text = doc(
            json!({"item": {"grain": "stack", "where": [
                is("post_contrast", "given"),
                is("base_basis", "physics"),
                is("directory_type", "anat")
            ]}}),
            "item",
            "count",
        );
        assert_eq!(count(&mut l, &text), 0, "{}", l.name);
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert_eq!(
                d.zero_rows.as_deref(),
                Some(
                    "no stack of item survives where directory_type = anat; everything after it \
                     is empty"
                ),
                "{}",
                l.name
            );
            assert_eq!(emptied(&d), [], "{}: {:?}", l.name, d.warnings);
        }
    }
}

#[test]
fn the_clauses_left_out_are_a_few_and_none_once_the_time_is_spent() {
    for mut l in labs() {
        plant(&mut l);
        let scheme = Scheme::default();
        let scope = quasi();
        // more value clauses in the shared set than are left out one by
        // one: the one that frees the item is the last of them
        let mut clauses: Vec<Value> = (0..EMPTIED_CLAUSES)
            .map(|_| is("directory_type", "anat"))
            .collect();
        clauses.push(is("contrast_mix", "PD_T2star"));
        let text = doc(
            json!({
                "base": {"grain": "stack", "where": clauses},
                "item": {"grain": "stack", "from": "base", "where": [is("post_contrast", "given")]}
            }),
            "item",
            "count",
        );
        for d in diagnosed(&mut l, &text, &scope) {
            assert!(d.zero_rows.is_some(), "{}", l.name);
            assert_eq!(emptied(&d), [], "{}: {:?}", l.name, d.warnings);
        }
        // a diagnosis under a time: the statements it runs without the
        // warning's own, then one fewer than it needs
        let text = doc(
            json!({
                "base": {"grain": "stack", "where": [is("contrast_mix", "PD_T2star")]},
                "item": {"grain": "stack", "from": "base", "where": [is("post_contrast", "given")]}
            }),
            "item",
            "count",
        );
        let mut run = |ends_at: Option<u32>| {
            time_ends_after(ends_at);
            let d = nils_ask::diagnose::diagnose_within(
                &mut l.registry,
                parse(&text).unwrap(),
                Vec::new(),
                &l.catalog,
                &scope,
                &scheme,
                bounds(),
                false,
                None,
                Some(60_000),
            )
            .unwrap();
            (d, time_ends_after(None))
        };
        let (whole, statements) = run(None);
        assert_eq!(emptied(&whole).len(), 1, "{}: {:?}", l.name, whole.warnings);
        assert!(whole.unavailable.is_none(), "{}", l.name);
        // the last statement is the warning's: without it the counts are
        // whole, nothing is warned of, and nothing is said to be missing
        let (cut, _) = run(Some(statements - 1));
        assert_eq!(emptied(&cut), [], "{}", l.name);
        assert!(cut.unavailable.is_none(), "{}", l.name);
        assert_eq!(cut.zero_rows, whole.zero_rows, "{}", l.name);
        assert_eq!(cut.funnel, whole.funnel, "{}", l.name);
    }
}
