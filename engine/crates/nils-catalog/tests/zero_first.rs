// SPDX-License-Identifier: AGPL-3.0-only

//! A count of nothing is explained first.
//!
//! The zero row explanation and the `emptied_by` warning were the last
//! things a diagnosis worked out: both waited for the whole funnel, and at
//! the size of a real registry the funnel of a session set that reads
//! several stack sets does not end in the time a draft has. So the two
//! things a count of nothing most needs were the two that never came.
//!
//! A diagnosis now counts first the stack sets the answer cannot do
//! without: those its set, or a set that is drawn from or is of, reads
//! through `has` with a least of one or more, and the sets those are
//! drawn from. Their counts are the passes the funnel runs for them, kept
//! and not run again. Where one of them holds no row the explanation and
//! the warning are worked out at once, and are served whether or not the
//! funnel ends in time.
//!
//! The values are planted on each backend's synthetic registry, by the
//! place `i` of a stack among those a stack set sees: `directory_type` is
//! six values in turn, `post_contrast` is `given` on one in twenty and
//! `not_given` on the rest, and `contrast_mix` is `T2like_T1` on the same
//! one in twenty and `PD_T2star` on the rest, so no stack holds
//! `PD_T2star` and `given` together.

mod common;

use common::{Lab, quasi, replant};
use nils_ask::diagnose::{
    Diagnosis, diagnose, diagnose_beside, diagnose_by_stage, diagnose_within, sets_passed,
    time_ends_after,
};
use nils_ask::exec::Bounds;
use nils_ask::parse;
use nils_ask::validate::Code;
use nils_registry::Store;
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_zero_first_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

const TYPES: [&str; 6] = ["dwi", "perf", "func", "fmap", "anat", "misc"];

/// How many stacks hold `given`.
fn plant(l: &mut Lab) -> i64 {
    replant(l, "directory_type", |i| Some(TYPES[i % 6]));
    replant(l, "contrast_mix", |i| {
        Some(if i % 20 == 0 {
            "T2like_T1"
        } else {
            "PD_T2star"
        })
    });
    let contrast = replant(l, "post_contrast", |i| {
        Some(if i % 20 == 0 { "given" } else { "not_given" })
    });
    contrast["given"]
}

fn is(axis: &str, value: &str) -> Value {
    json!(["=", {}, ["axis", {}, axis], value])
}

/// A session set that reads five stack sets through `has`, each drawn
/// from one shared set and of two clauses of its own. With `empty`, the
/// last of them holds two clauses no stack meets together.
fn items(empty: bool, level: &str) -> String {
    items_read(empty, level, json!({"set": "item_e", "min": 1}))
}

/// [`items`], with how the session set reads the last of the five.
fn items_read(empty: bool, level: &str, last_has: Value) -> String {
    let item = |a: Value, b: Value| json!({"grain": "stack", "from": "base", "where": [a, b]});
    let last = if empty {
        item(
            is("contrast_mix", "PD_T2star"),
            is("post_contrast", "given"),
        )
    } else {
        item(
            is("directory_type", "misc"),
            is("post_contrast", "not_given"),
        )
    };
    json!({
        "ast_version": 1,
        "name": "items",
        "sets": {
            "base": {"grain": "stack", "where": [
                ["in", {}, ["axis", {}, "directory_type"], TYPES]
            ]},
            "item_a": item(is("directory_type", "anat"), is("post_contrast", "not_given")),
            "item_b": item(is("directory_type", "dwi"), is("post_contrast", "not_given")),
            "item_c": item(is("directory_type", "func"), is("contrast_mix", "PD_T2star")),
            "item_d": item(is("directory_type", "perf"), is("contrast_mix", "PD_T2star")),
            "item_e": last,
            "visits": {"grain": "session", "has": [
                {"set": "item_a", "min": 1},
                {"set": "item_b", "min": 1},
                {"set": "item_c", "min": 1},
                {"set": "item_d", "min": 1},
                last_has
            ]}
        },
        "out": {"set": "visits", "level": level}
    })
    .to_string()
}

/// A diagnosis under a time that ends at the statement named, and the
/// statements it ran.
fn within(l: &mut Lab, text: &str, ends_at: Option<u32>) -> (Diagnosis, u32) {
    time_ends_after(ends_at);
    let d = diagnose_within(
        &mut l.registry,
        parse(text).unwrap(),
        Vec::new(),
        &l.catalog,
        &quasi(),
        &Scheme::default(),
        bounds(),
        false,
        None,
        Some(600_000),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", l.name));
    (d, time_ends_after(None))
}

/// The diagnosis counted a statement per stage, the way every stage was
/// counted before a set's stages were one pass: what a diagnosis is held
/// to, and one that has no early count.
fn by_stage(l: &mut Lab, text: &str) -> Diagnosis {
    diagnose_by_stage(
        &mut l.registry,
        parse(text).unwrap(),
        Vec::new(),
        &l.catalog,
        &quasi(),
        &Scheme::default(),
        bounds(),
        false,
        None,
        None,
    )
    .unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

fn emptied(d: &Diagnosis) -> Vec<(String, String)> {
    d.warnings
        .iter()
        .filter(|w| w.code == Code::EmptiedBy)
        .map(|w| (w.path.clone(), w.message.clone()))
        .collect()
}

/// The statements a diagnosis of the two documents ran before any set was
/// counted ahead of the funnel: the passes of the six stack sets, the
/// source of the session set and its five `has`, and, for the empty one,
/// the two clauses `emptied_by` leaves out.
const STATEMENTS_WITH_ROWS: u32 = 12;
const STATEMENTS_EMPTY: u32 = 14;

/// The statements that explain the empty document: the six passes and the
/// two clauses left out.
const STATEMENTS_EARLY: u32 = 8;

#[test]
fn a_count_of_nothing_is_explained_inside_a_diagnosis_cut_by_its_time() {
    for mut l in labs() {
        let given = plant(&mut l);
        let text = items(true, "count");
        let said = "no stack of item_e survives where post_contrast = given; everything after it \
                    is empty";
        let warned = vec![(
            "sets.item_e.where[0]".to_string(),
            format!(
                "item_e keeps no stack: with the set's other conditions, contrast_mix = \
                 PD_T2star and post_contrast = given hold together on no stack. Without \
                 contrast_mix = PD_T2star, item_e holds {given} stacks."
            ),
        )];
        // with all the time: the whole diagnosis, as a statement a stage
        // counts it, and no more statements than before
        let (whole, statements) = within(&mut l, &text, None);
        assert_eq!(statements, STATEMENTS_EMPTY, "{}", l.name);
        assert!(whole.unavailable.is_none(), "{}", l.name);
        assert_eq!(whole.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&whole), warned, "{}", l.name);
        let reference = by_stage(&mut l, &text);
        assert_eq!(whole.funnel, reference.funnel, "{}", l.name);
        assert_eq!(whole.drops, reference.drops, "{}", l.name);
        assert_eq!(whole.warnings, reference.warnings, "{}", l.name);
        assert_eq!(whole.zero_rows, reference.zero_rows, "{}", l.name);
        assert_eq!(whole.next, reference.next, "{}", l.name);
        // with the time for the stack sets and the two clauses left out,
        // and none for the session set: the explanation and the warning
        // are served, and the funnel is said to be cut
        let (cut, ran) = within(&mut l, &text, Some(STATEMENTS_EARLY));
        assert_eq!(ran, STATEMENTS_EARLY, "{}", l.name);
        assert!(cut.valid, "{}", l.name);
        assert_eq!(cut.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&cut), warned, "{}", l.name);
        let unavailable = cut
            .unavailable
            .as_ref()
            .unwrap_or_else(|| panic!("{}: nothing is said to be missing", l.name));
        assert_eq!(unavailable.code, "timeout", "{}", l.name);
        assert_eq!(
            unavailable.parts,
            ["funnel", "drops", "ties", "coarse"],
            "{}",
            l.name
        );
        // the funnel keeps the stages counted: those of the stack sets,
        // each as the whole funnel shows it, and none of the session set
        assert!(!cut.funnel.is_empty(), "{}", l.name);
        assert!(cut.funnel.iter().all(|s| s.set != "visits"), "{}", l.name);
        let stacks: Vec<_> = whole.funnel.iter().filter(|s| s.set != "visits").collect();
        assert_eq!(cut.funnel.iter().collect::<Vec<_>>(), stacks, "{}", l.name);
        assert!(cut.drops.is_empty(), "{}", l.name);
        assert!(
            cut.next.iter().any(|n| n.contains("/api/ask/options")),
            "{}: {:?}",
            l.name,
            cut.next
        );
        // with the time for the stack sets alone: the explanation, and no
        // warning, since its counts are the next statements
        let (cut, _) = within(&mut l, &text, Some(STATEMENTS_EARLY - 2));
        assert_eq!(cut.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&cut), [], "{}", l.name);
        assert!(cut.unavailable.is_some(), "{}", l.name);
        // with less than that nothing is known, as before
        let (cut, _) = within(&mut l, &text, Some(STATEMENTS_EARLY - 3));
        assert_eq!(cut.zero_rows, None, "{}", l.name);
        assert_eq!(emptied(&cut), [], "{}", l.name);
        assert!(cut.unavailable.is_some(), "{}", l.name);
        // an empty list says the same as a count of nothing
        let (listed, _) = within(&mut l, &items(true, "record"), Some(STATEMENTS_EARLY));
        assert_eq!(listed.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&listed), warned, "{}", l.name);
    }
}

#[test]
fn a_document_with_rows_pays_no_statement_for_it_and_is_told_nothing_new() {
    for mut l in labs() {
        plant(&mut l);
        let text = items(false, "count");
        let (_, a) = common::ask_of(&mut l, &text);
        assert!(a.rows[0].opt_int(0).unwrap().unwrap() > 0, "{}", l.name);
        let passed = sets_passed();
        let (whole, statements) = within(&mut l, &text, None);
        assert_eq!(statements, STATEMENTS_WITH_ROWS, "{}", l.name);
        // each stack set is counted by one pass, once
        assert_eq!(sets_passed().0 - passed.0, 6, "{}", l.name);
        assert!(whole.unavailable.is_none(), "{}", l.name);
        assert_eq!(whole.zero_rows, None, "{}", l.name);
        assert_eq!(emptied(&whole), [], "{}", l.name);
        let reference = by_stage(&mut l, &text);
        assert_eq!(whole.funnel, reference.funnel, "{}", l.name);
        assert_eq!(whole.drops, reference.drops, "{}", l.name);
        assert_eq!(whole.warnings, reference.warnings, "{}", l.name);
        // cut after the stack sets, it says nothing of a zero
        let (cut, _) = within(&mut l, &text, Some(6));
        assert_eq!(cut.zero_rows, None, "{}", l.name);
        assert_eq!(emptied(&cut), [], "{}", l.name);
        assert!(cut.unavailable.is_some(), "{}", l.name);
    }
}

#[test]
fn a_set_the_answer_can_do_without_explains_nothing_early() {
    for mut l in labs() {
        plant(&mut l);
        // the empty set is read with a most of none: the sessions that hold
        // no such stack are every session, and the answer has rows
        let text = items_read(true, "count", json!({"set": "item_e", "max": 0}));
        let (_, a) = common::ask_of(&mut l, &text);
        assert!(a.rows[0].opt_int(0).unwrap().unwrap() > 0, "{}", l.name);
        let (whole, _) = within(&mut l, &text, None);
        assert_eq!(whole.zero_rows, None, "{}", l.name);
        // the warning of an empty set on the answer's path is as it was
        assert_eq!(emptied(&whole).len(), 1, "{}: {:?}", l.name, whole.warnings);
        let reference = by_stage(&mut l, &text);
        assert_eq!(whole.funnel, reference.funnel, "{}", l.name);
        assert_eq!(whole.warnings, reference.warnings, "{}", l.name);
        // cut after the stack sets: nothing is said, the set is not needed
        let (cut, _) = within(&mut l, &text, Some(6));
        assert_eq!(cut.zero_rows, None, "{}", l.name);
        assert_eq!(emptied(&cut), [], "{}", l.name);
    }
}

#[test]
fn a_least_given_by_a_parameter_and_a_shared_set_that_is_empty_are_explained_too() {
    for mut l in labs() {
        let given = plant(&mut l);
        // the least is a parameter of the document, bound to one
        let mut doc: Value = serde_json::from_str(&items_read(
            true,
            "count",
            json!({"set": "item_e", "min": ["param", {}, "least"]}),
        ))
        .unwrap();
        doc["params"] = json!({"least": {"type": "integer", "value": 1}});
        let text = doc.to_string();
        let (cut, _) = within(&mut l, &text, Some(STATEMENTS_EARLY));
        assert_eq!(
            cut.zero_rows.as_deref(),
            Some(
                "no stack of item_e survives where post_contrast = given; everything after it \
                 is empty"
            ),
            "{}",
            l.name
        );
        assert_eq!(emptied(&cut).len(), 1, "{}", l.name);
        assert!(cut.unavailable.is_some(), "{}", l.name);
        // the set the items are drawn from is the one that holds nothing:
        // it is named, and the item, empty from its source on, adds nothing
        let text = json!({
            "ast_version": 1,
            "name": "shared",
            "sets": {
                "base": {"grain": "stack", "where": [
                    is("contrast_mix", "PD_T2star"),
                    is("post_contrast", "given")
                ]},
                "item": {"grain": "stack", "from": "base", "where": [is("directory_type", "anat")]},
                "visits": {"grain": "session", "has": [{"set": "item", "min": 1}]}
            },
            "out": {"set": "visits", "level": "count"}
        })
        .to_string();
        let said = "no stack of base survives where post_contrast = given; everything after it \
                    is empty";
        let warned = vec![(
            "sets.base.where[0]".to_string(),
            format!(
                "base keeps no stack: contrast_mix = PD_T2star and post_contrast = given hold \
                 together on no stack. Without contrast_mix = PD_T2star, base holds {given} \
                 stacks."
            ),
        )];
        // two passes, the session set's source and its `has`, and the one
        // clause left out
        let (whole, statements) = within(&mut l, &text, None);
        assert_eq!(statements, 5, "{}", l.name);
        assert_eq!(whole.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&whole), warned, "{}", l.name);
        let reference = by_stage(&mut l, &text);
        assert_eq!(whole.funnel, reference.funnel, "{}", l.name);
        assert_eq!(whole.warnings, reference.warnings, "{}", l.name);
        // the two passes and the clause left out are the first three
        let (cut, ran) = within(&mut l, &text, Some(3));
        assert_eq!(ran, 3, "{}", l.name);
        assert_eq!(cut.zero_rows.as_deref(), Some(said), "{}", l.name);
        assert_eq!(emptied(&cut), warned, "{}", l.name);
        assert!(cut.unavailable.is_some(), "{}", l.name);
        assert!(cut.funnel.iter().all(|s| s.set != "visits"), "{}", l.name);
    }
}

#[test]
fn the_three_ways_a_diagnosis_is_counted_explain_a_zero_alike() {
    for mut l in labs() {
        plant(&mut l);
        let scheme = Scheme::default();
        let scope = quasi();
        for empty in [true, false] {
            let text = items(empty, "count");
            let ask = parse(&text).unwrap();
            let mut beside: Vec<Store> = (0..2)
                .map(|_| l.registry.open_ask_reader(None).unwrap())
                .collect();
            let plain = diagnose(
                &mut l.registry,
                ask.clone(),
                Vec::new(),
                &l.catalog,
                &scope,
                &scheme,
                bounds(),
                false,
                None,
            )
            .unwrap();
            let together = diagnose_beside(
                &mut l.registry,
                ask,
                Vec::new(),
                &l.catalog,
                &scope,
                &scheme,
                bounds(),
                false,
                None,
                None,
                &mut beside,
            )
            .unwrap();
            let reference = by_stage(&mut l, &text);
            for d in [&plain, &together] {
                assert_eq!(d.funnel, reference.funnel, "{}: {empty}", l.name);
                assert_eq!(d.drops, reference.drops, "{}: {empty}", l.name);
                assert_eq!(d.warnings, reference.warnings, "{}: {empty}", l.name);
                assert_eq!(d.zero_rows, reference.zero_rows, "{}: {empty}", l.name);
                assert_eq!(d.zero_rows.is_some(), empty, "{}", l.name);
                assert_eq!(emptied(d).len(), usize::from(empty), "{}", l.name);
            }
        }
    }
}
