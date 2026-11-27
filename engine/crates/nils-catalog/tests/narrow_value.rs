// SPDX-License-Identifier: AGPL-3.0-only

//! A filter on one value of an axis that throws away what the value's
//! siblings hold. "All brain scans" written as `body_part = brain` keeps
//! the stacks that end above the neck and drops the many labelled brain
//! and neck; the answer is a small share of what was meant, and a
//! diagnosis said nothing. It now warns (`narrow_value`), names the
//! siblings with their counts and, where the pack declares a coarse form
//! of the axis, the wider choice.
//!
//! A clause is measured against its set: the rows the set holds with that
//! one clause left out, by their value. So the warning is the same wherever
//! in the list the clause stands, and a clause the set's other conditions
//! already answer warns of nothing.
//!
//! The values are planted on each backend's synthetic registry, and every
//! count the warning says is held against the rows as planted: the warning
//! in a stack set a session set reads through `has`, on each of the three
//! ways a diagnosis is counted; none where the filter kept a large share,
//! where the rest holds no sibling, where the siblings are other things,
//! and on an axis the pack declares no coarse form of, however its counts
//! fall; none where a count is held back; none once the time for the
//! counts is spent; and the drops of the sets the answer reads through.

mod common;

use std::collections::BTreeMap;

use common::{Lab, quasi};
use nils_ask::diagnose::{
    Diagnosis, NARROW_CLAUSES, diagnose, diagnose_beside, diagnose_by_stage, diagnose_within,
};
use nils_ask::exec::Bounds;
use nils_ask::parse;
use nils_ask::validate::{Code, Scope};
use nils_registry::schema::table;
use nils_registry::session::Scheme;
use nils_registry::{Insert, Param, Store};
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_narrow_value_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// The stacks a stack set sees (none the pack excluded), each with its
/// subject, in the order of their ids.
fn seen(store: &mut Store) -> Vec<(i64, i64)> {
    let q = |t: &str| store.qualified(t);
    let sql = format!(
        "SELECT st.id, se.subject_id FROM {stack} st JOIN {series} se ON se.id = st.series_id \
         JOIN {fp} f ON f.stack_id = st.id \
         WHERE NOT EXISTS (SELECT 1 FROM {ax} x WHERE x.stack_id = st.id \
         AND x.axis = 'disposition' AND x.value = 'excluded') ORDER BY st.id",
        ax = q("classification_axis"),
        stack = q("stack"),
        series = q("series"),
        fp = q("stack_fingerprint"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
        .collect()
}

/// The axes this file plants: whatever the synthetic registry said on them
/// is taken away first.
const PLANTED: [&str; 6] = [
    "body_part",
    "body_region",
    "contrast_mix",
    "base_basis",
    "directory_type",
    "post_contrast",
];

/// How many stacks hold each planted value, by axis and value as a row
/// stores it.
type Counts = BTreeMap<(&'static str, &'static str), i64>;

/// Plant the six axes over every stack a stack set sees, by the stack's
/// place `i` among them:
///
/// - `body_part`: brain on one stack in twenty, spine on one, none on one,
///   brain-neck on the other seventeen, with the region the rules fold
///   each into. Where `few_brains`, the brains are the stacks of the first
///   two subjects alone, so a count of them is of fewer than five people;
/// - `contrast_mix`, which no coarse form is declared around: one value on
///   one in twenty, another on the rest;
/// - `base_basis`: one value on one in twenty, no value on the rest;
/// - `directory_type`: six of its values in turn;
/// - `post_contrast`, a yes or no: given (`1`) on one in twenty.
fn plant(l: &mut Lab, few_brains: bool) -> (Counts, i64) {
    plant_few(l, few_brains.then_some("brain"))
}

/// [`plant`], with the value the stacks of the first two subjects alone
/// hold, where one is given: `brain` as `plant` plants it, or `spine`,
/// when every other stack holds a brain on one in twenty, none on one and
/// brain-neck on the rest, so that the sibling a count of which is of
/// fewer than five people is the spine.
fn plant_few(l: &mut Lab, few: Option<&'static str>) -> (Counts, i64) {
    let few_brains = few == Some("brain");
    let store = l.registry.store();
    let stacks = seen(store);
    let listed = PLANTED
        .iter()
        .map(|a| format!("'{a}'"))
        .collect::<Vec<_>>()
        .join(", ");
    store
        .execute(
            &format!(
                "DELETE FROM {} WHERE axis IN ({listed})",
                store.qualified("classification_axis")
            ),
            &[],
        )
        .unwrap();
    let mut subjects: Vec<i64> = stacks.iter().map(|s| s.1).collect();
    subjects.sort_unstable();
    subjects.dedup();
    let first_two = &subjects[..2];
    const TYPES: [&str; 6] = ["dwi", "perf", "func", "fmap", "anat", "misc"];
    let mut counts = Counts::new();
    let mut rows: Vec<Vec<Param>> = Vec::new();
    for (i, (stack, subject)) in stacks.iter().enumerate() {
        let part = if few_brains {
            match (first_two.contains(subject), i % 20) {
                (true, _) => Some("brain"),
                (false, 18) => Some("spine"),
                (false, 19) => None,
                (false, _) => Some("brain-neck"),
            }
        } else if few == Some("spine") {
            match (first_two.contains(subject), i % 20) {
                (true, _) => Some("spine"),
                (false, 0) => Some("brain"),
                (false, 19) => None,
                (false, _) => Some("brain-neck"),
            }
        } else {
            match i % 20 {
                0 => Some("brain"),
                18 => Some("spine"),
                19 => None,
                _ => Some("brain-neck"),
            }
        };
        let region = part.map(|p| if p == "spine" { "spine" } else { "head" });
        let twentieth = i % 20 == 0;
        let planted: [(&'static str, Option<&'static str>); 6] = [
            ("body_part", part),
            ("body_region", region),
            (
                "contrast_mix",
                Some(if twentieth { "T2like_T1" } else { "PD_T2star" }),
            ),
            ("base_basis", twentieth.then_some("physics")),
            ("directory_type", Some(TYPES[i % 6])),
            ("post_contrast", Some(if twentieth { "1" } else { "0" })),
        ];
        for (axis, value) in planted {
            let Some(value) = value else { continue };
            *counts.entry((axis, value)).or_default() += 1;
            rows.push(vec![
                Param::Int(*stack),
                Param::from(axis),
                Param::from(value),
                Param::Double(0.9),
                Param::from("stated"),
            ]);
        }
    }
    for chunk in rows.chunks(500) {
        store
            .insert(
                &Insert::new(
                    table("classification_axis"),
                    &["stack_id", "axis", "value", "confidence", "tier"],
                ),
                chunk,
            )
            .unwrap();
    }
    let ids: Vec<i64> = stacks.iter().map(|s| s.0).collect();
    nils_registry::stack_axis::refresh(store, &ids).unwrap();
    (counts, stacks.len() as i64)
}

fn doc(sets: Value, out: &str) -> String {
    json!({"ast_version": 1, "name": "narrow", "sets": sets, "out": {"set": out, "level": "count"}})
        .to_string()
}

/// A session set that reads a stack set through `has`, the stack set
/// filtered by `clauses`.
fn visits_with(clauses: Value) -> String {
    doc(
        json!({
            "scans": {"grain": "stack", "where": clauses},
            "visits": {"grain": "session", "has": [{"set": "scans", "min": 1}]}
        }),
        "visits",
    )
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

/// The warnings of a diagnosis about a narrow value: path, message, next.
fn narrow(d: &Diagnosis) -> Vec<(String, String, String)> {
    d.warnings
        .iter()
        .filter(|w| w.code == Code::NarrowValue)
        .map(|w| (w.path.clone(), w.message.clone(), w.next.clone()))
        .collect()
}

/// A count as the warning writes it.
fn n(count: i64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// The rows a set of a document answers at `level: count`.
fn count(l: &mut Lab, text: &str) -> i64 {
    let (_, a) = common::ask_of(l, text);
    a.rows[0].opt_int(0).unwrap().unwrap()
}

#[test]
fn a_fine_value_that_left_most_rows_with_its_sibling_is_warned_of_with_the_wider_choice() {
    for mut l in labs() {
        let (counts, stacks) = plant(&mut l, false);
        let brain = counts[&("body_part", "brain")];
        let brain_neck = counts[&("body_part", "brain-neck")];
        assert!(brain >= 5 && brain_neck > 10 * brain, "{}", l.name);
        let text = visits_with(json!([is("body_part", "brain")]));
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert!(d.valid, "{}", l.name);
            let w = narrow(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.scans.where[0]", "{}", l.name);
            assert_eq!(
                w[0].1,
                format!(
                    "scans: body_part = brain keeps {} of {} stacks; brain-neck holds {} of the rest. \
                     If any head of body_region is meant, write body_region = head, which covers \
                     brain and brain-neck.",
                    n(brain),
                    n(stacks),
                    n(brain_neck)
                ),
                "{}",
                l.name
            );
            assert_eq!(
                w[0].2,
                "to keep them all, write [\"=\",{},[\"axis\",{},\"body_region\"],\"head\"] at \
                 sets.scans.where[0]; where only brain is meant, change nothing",
                "{}",
                l.name
            );
            // the drops name the clause of the set the answer reads through
            let drop = d
                .drops
                .iter()
                .find(|x| x.set == "scans")
                .unwrap_or_else(|| panic!("{}: {:?}", l.name, d.drops));
            assert_eq!(drop.clause, "where body_part = brain", "{}", l.name);
            assert_eq!(
                (drop.before, drop.after, drop.nulls),
                (Some(stacks), Some(brain), Some(0)),
                "{}",
                l.name
            );
        }
        // the wider choice keeps what it says it covers
        let wider = doc(
            json!({"scans": {"grain": "stack", "where": [is("body_region", "head")]}}),
            "scans",
        );
        assert_eq!(count(&mut l, &wider), brain + brain_neck, "{}", l.name);
        // and warns of nothing itself: the coarse axis has no wider form
        for d in diagnosed(&mut l, &wider, &quasi()) {
            assert_eq!(narrow(&d), [], "{}", l.name);
        }
        // `in` and `has` with the value are the same filter
        for clause in [
            json!(["in", {}, ["axis", {}, "body_part"], ["brain"]]),
            json!(["has", {}, ["axis", {}, "body_part"], "brain"]),
        ] {
            let text = visits_with(json!([clause]));
            for d in diagnosed(&mut l, &text, &quasi()) {
                let w = narrow(&d);
                assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
                assert!(
                    w[0].1.contains("write body_region = head"),
                    "{}: {}",
                    l.name,
                    w[0].1
                );
            }
        }
        // a document is valid with the warning, and without the counts
        // nothing is said: at validation the warning does not exist
        let prepared = nils_ask::prepare(parse(&text).unwrap(), &l.catalog, &quasi()).unwrap();
        assert!(
            prepared
                .validated
                .warnings
                .iter()
                .all(|w| w.code != Code::NarrowValue)
        );
    }
}

#[test]
fn a_value_that_kept_a_large_share_or_has_no_such_sibling_is_left_alone() {
    for mut l in labs() {
        let (counts, _) = plant(&mut l, false);
        let quiet = |l: &mut Lab, clauses: Value, why: &str| {
            let text = visits_with(clauses);
            for d in diagnosed(l, &text, &quasi()) {
                assert!(d.valid, "{}: {why}", l.name);
                assert_eq!(narrow(&d), [], "{}: {why}", l.name);
            }
        };
        // the value most stacks hold
        quiet(
            &mut l,
            json!([is("body_part", "brain-neck")]),
            "a large share kept",
        );
        // both halves of the head named
        quiet(
            &mut l,
            json!([["in", {}, ["axis", {}, "body_part"], ["brain", "brain-neck"]]]),
            "both values kept",
        );
        // a spine in a registry of heads: few are kept, and the many
        // dropped are under another region, which is another thing
        assert!(counts[&("body_part", "spine")] * 5 < counts[&("body_part", "brain-neck")]);
        quiet(
            &mut l,
            json!([is("body_part", "spine")]),
            "siblings under another coarse value",
        );
        // the coarse axis itself, with nothing wider
        quiet(&mut l, json!([is("body_region", "spine")]), "a coarse axis");
        // axes the pack declares no coarse form of, however their counts
        // fall: few kept, and the rest hold no value on the axis at all
        quiet(
            &mut l,
            json!([is("base_basis", "physics")]),
            "no sibling holds the rest",
        );
        // few kept, and the rest spread over five values
        quiet(
            &mut l,
            json!([is("directory_type", "dwi")]),
            "siblings spread thin",
        );
        // a yes or no: the other value is the opposite
        quiet(&mut l, json!([is("post_contrast", "given")]), "a yes or no");
        // a value that is excluded, not kept
        quiet(
            &mut l,
            json!([["<>", {}, ["axis", {}, "body_part"], "brain-neck"]]),
            "an exclusion",
        );
    }
}

#[test]
fn an_axis_the_pack_declares_no_coarse_form_of_is_left_alone() {
    for mut l in labs() {
        let (counts, _) = plant(&mut l, false);
        // one value on a twentieth of the stacks and another on all the
        // rest: by the counts alone this is the brain beside the brain and
        // neck, and nothing says the two could be mistaken for each other
        let kept = counts[&("contrast_mix", "T2like_T1")];
        let other = counts[&("contrast_mix", "PD_T2star")];
        assert!(other > 10 * kept, "{}", l.name);
        let text = visits_with(json!([is("contrast_mix", "T2like_T1")]));
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert!(d.valid, "{}", l.name);
            assert_eq!(narrow(&d), [], "{}", l.name);
            // its drop is counted all the same
            let drop = d.drops.iter().find(|x| x.set == "scans").unwrap();
            assert_eq!(drop.after, Some(kept), "{}", l.name);
        }
    }
}

#[test]
fn the_warning_counts_siblings_for_a_few_clauses_of_a_document_at_most() {
    for mut l in labs() {
        plant(&mut l, false);
        // five stack sets with the narrow filter, each read by the answer
        let mut sets = serde_json::Map::new();
        let mut has = Vec::new();
        for i in 0..5 {
            sets.insert(
                format!("scans{i}"),
                json!({"grain": "stack", "where": [is("body_part", "brain")]}),
            );
            has.push(json!({"set": format!("scans{i}"), "min": 1}));
        }
        sets.insert("visits".into(), json!({"grain": "session", "has": has}));
        let text = doc(Value::Object(sets), "visits");
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert!(d.valid, "{}", l.name);
            assert_eq!(narrow(&d).len(), NARROW_CLAUSES, "{}", l.name);
            // every one of the five is among the drops all the same
            assert_eq!(
                d.drops
                    .iter()
                    .filter(|x| x.set.starts_with("scans"))
                    .count(),
                5,
                "{}: {:?}",
                l.name,
                d.drops
            );
        }
    }
}

#[test]
fn a_count_held_back_warns_of_nothing() {
    for mut l in labs() {
        let (counts, stacks) = plant(&mut l, true);
        let brain = counts[&("body_part", "brain")];
        assert!(brain > 0 && brain * 5 < stacks, "{}", l.name);
        // the stacks of a day or later, which is every stack, then the
        // brains: the stage is one a quasi identifying field chose
        let text = visits_with(json!([
            [">=", {}, ["field", {}, "day"], "1900-01-01"],
            is("body_part", "brain")
        ]));
        // at detail quasi the counts are shown, and the warning with them
        for d in diagnosed(&mut l, &text, &quasi()) {
            let w = narrow(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert_eq!(w[0].0, "sets.scans.where[1]", "{}", l.name);
            assert!(
                w[0].1
                    .contains(&format!("keeps {} of {} stacks", n(brain), n(stacks))),
                "{}: {}",
                l.name,
                w[0].1
            );
        }
        // at plain the brains are two people's, their count is held back
        // in the funnel, and the warning that would say it is not given
        for d in diagnosed(&mut l, &text, &Scope::default()) {
            assert!(d.valid, "{}", l.name);
            let stage = d
                .funnel
                .iter()
                .find(|s| s.set == "scans" && s.stage == "where body_part = brain")
                .unwrap_or_else(|| panic!("{}: {:?}", l.name, d.funnel));
            assert_eq!((stage.rows, stage.subjects), (None, None), "{}", l.name);
            assert_eq!(narrow(&d), [], "{}", l.name);
            assert!(
                d.warnings
                    .iter()
                    .all(|w| !w.message.contains(&n(brain)) || w.code != Code::NarrowValue),
                "{}",
                l.name
            );
        }
        // without the quasi identifying clause nothing is held at plain,
        // and the warning says what the funnel says
        let open = visits_with(json!([is("body_part", "brain")]));
        for d in diagnosed(&mut l, &open, &Scope::default()) {
            let stage = d
                .funnel
                .iter()
                .find(|s| s.set == "scans" && s.stage == "where body_part = brain")
                .unwrap();
            assert_eq!(stage.rows, Some(brain), "{}", l.name);
            assert_eq!(narrow(&d).len(), 1, "{}: {:?}", l.name, d.warnings);
        }
    }
}

/// A diagnosis under a time, with the clock a test holds: the time ends at
/// the statement named. Answers the diagnosis and how many statements ran.
fn within(l: &mut Lab, text: &str, ends_at: Option<u32>) -> (Diagnosis, u32) {
    nils_ask::diagnose::time_ends_after(ends_at);
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
    let ran = nils_ask::diagnose::time_ends_after(None);
    (d, ran)
}

/// The warning's statement is the last a diagnosis runs, and it is a hint.
/// Where the time for the counts ended before it, at whatever statement,
/// it is not run and nothing is warned of, and the diagnosis still says
/// what it left uncounted. Where the time ends in the warning's own
/// statement, nothing is warned of and nothing is said to be uncounted,
/// since every count was read.
#[test]
fn nothing_is_warned_of_once_the_time_for_the_counts_is_spent() {
    for mut l in labs() {
        plant(&mut l, false);
        let text = visits_with(json!([is("body_part", "brain")]));
        // time enough: the warning, nothing left uncounted, and the number
        // of statements it took, the warning's own the last of them
        let (whole, statements) = within(&mut l, &text, None);
        assert!(whole.unavailable.is_none(), "{}", l.name);
        assert_eq!(narrow(&whole).len(), 1, "{}", l.name);
        assert!(statements >= 3, "{}: {statements}", l.name);
        let funnel =
            |d: &Diagnosis| -> Vec<Option<i64>> { d.funnel.iter().map(|s| s.rows).collect() };

        // no time at all: valid, nothing counted, nothing warned of
        let (none, ran) = within(&mut l, &text, Some(0));
        assert!(none.valid, "{}", l.name);
        assert!(none.unavailable.is_some(), "{}", l.name);
        assert_eq!(narrow(&none), [], "{}", l.name);
        assert_eq!(ran, 0, "{}", l.name);

        // the time ends at each statement before the warning's in turn:
        // the stages counted until then stand, the warning's statement is
        // not run (no statement more than the time allowed), nothing is
        // warned of, and the diagnosis says its counts are not whole
        let mut counted_the_narrow_stage = false;
        for at in 1..statements - 1 {
            let (d, ran) = within(&mut l, &text, Some(at));
            assert_eq!(ran, at, "{}: the time ended at statement {at}", l.name);
            assert!(d.valid, "{}", l.name);
            assert!(
                d.unavailable.is_some(),
                "{}: at {at} the counts are said to be whole",
                l.name
            );
            assert_eq!(narrow(&d), [], "{}: warned at {at}", l.name);
            counted_the_narrow_stage |= d
                .funnel
                .iter()
                .any(|s| s.set == "scans" && s.stage.starts_with("where") && s.rows.is_some());
        }
        assert!(
            counted_the_narrow_stage,
            "{}: at no point were the counts the warning reads there with the time spent",
            l.name
        );

        // the time ends in the warning's own statement: every count of the
        // diagnosis is there as with time enough, nothing is said to be
        // uncounted, and nothing is warned of
        let (last, ran) = within(&mut l, &text, Some(statements - 1));
        assert_eq!(ran, statements - 1, "{}", l.name);
        assert!(last.unavailable.is_none(), "{}", l.name);
        assert_eq!(funnel(&last), funnel(&whole), "{}", l.name);
        assert_eq!(last.drops, whole.drops, "{}", l.name);
        assert_eq!(narrow(&last), [], "{}", l.name);
    }
}

/// A group answer with a count held back gives no warning. At plain, over
/// a stage a quasi identifying field chose, the group statement holds a
/// value of fewer than five people and one more beside it; the funnel
/// shows the kept count all the same, and a message with the counts shown
/// next to it would say by subtraction what the answer kept apart.
#[test]
fn a_sibling_whose_count_is_held_back_warns_of_nothing() {
    for mut l in labs() {
        let (counts, stacks) = plant_few(&mut l, Some("spine"));
        let brain = counts[&("body_part", "brain")];
        let spine = counts[&("body_part", "spine")];
        assert!(brain > 0 && brain * 5 < stacks && spine > 0, "{}", l.name);
        let text = visits_with(json!([
            [">=", {}, ["field", {}, "day"], "1900-01-01"],
            is("body_part", "brain")
        ]));
        // at detail quasi every count is shown, and the warning is given
        // (the spine is under another coarse value, and is never named)
        for d in diagnosed(&mut l, &text, &quasi()) {
            let w = narrow(&d);
            assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
            assert!(
                w[0].1.contains("brain-neck holds"),
                "{}: {}",
                l.name,
                w[0].1
            );
        }
        // at plain the kept count is of many people and the funnel shows
        // it; the spines are two people's, the group answer holds them,
        // and no warning is made of the counts it does show
        for d in diagnosed(&mut l, &text, &Scope::default()) {
            assert!(d.valid, "{}", l.name);
            let stage = d
                .funnel
                .iter()
                .find(|s| s.set == "scans" && s.stage == "where body_part = brain")
                .unwrap_or_else(|| panic!("{}: {:?}", l.name, d.funnel));
            assert_eq!(
                stage.rows,
                Some(brain),
                "{}: the kept count is shown",
                l.name
            );
            assert_eq!(narrow(&d), [], "{}: {:?}", l.name, d.warnings);
        }
    }
}

/// Where the question asks for keys, a stage's rows are the keys listed,
/// cut at the cap of a list, and nothing is warned of from them.
#[test]
fn nothing_is_warned_of_where_the_keys_are_asked_for() {
    for mut l in labs() {
        let (counts, stacks) = plant(&mut l, false);
        let brain = counts[&("body_part", "brain")];
        let text = visits_with(json!([is("body_part", "brain")]));
        let scheme = Scheme::default();
        // a cap of a list below the stacks there are, as on a registry
        // larger than a list may be
        let cap = (brain as u64 * 2).max(4);
        assert!((cap as i64) < stacks, "{}", l.name);
        let with = |l: &mut Lab, keys: bool| {
            diagnose(
                &mut l.registry,
                parse(&text).unwrap(),
                Vec::new(),
                &l.catalog,
                &quasi(),
                &scheme,
                Bounds {
                    max_rows: cap,
                    ..bounds()
                },
                keys,
                None,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", l.name))
        };
        let counted = with(&mut l, false);
        let w = narrow(&counted);
        assert_eq!(w.len(), 1, "{}: {:?}", l.name, counted.warnings);
        assert!(
            w[0].1
                .contains(&format!("keeps {} of {} stacks", n(brain), n(stacks))),
            "{}: {}",
            l.name,
            w[0].1
        );
        let keyed = with(&mut l, true);
        assert!(keyed.valid, "{}", l.name);
        assert!(
            keyed.funnel.iter().any(|s| !s.keys.is_empty()),
            "{}: the keys were asked for",
            l.name
        );
        assert_eq!(narrow(&keyed), [], "{}: {:?}", l.name, keyed.warnings);
    }
}

/// A set that already keeps the coarse value, in a clause of its own or of
/// the set it is drawn from, and then narrows to a value that coarse value
/// covers, narrows on purpose: nothing is warned of, and the clause the
/// warning would suggest is one the set has.
#[test]
fn a_fine_value_beside_its_own_coarse_value_is_left_alone() {
    for mut l in labs() {
        plant(&mut l, false);
        // the plain clause warns: the control
        let alone = doc(
            json!({"scans": {"grain": "stack", "where": [is("body_part", "brain")]}}),
            "scans",
        );
        for d in diagnosed(&mut l, &alone, &quasi()) {
            assert_eq!(narrow(&d).len(), 1, "{}", l.name);
        }
        let deliberate = [
            // the coarse clause first, then the fine one
            json!({"scans": {"grain": "stack", "where": [is("body_region", "head"), is("body_part", "brain")]}}),
            // the fine one first: the wider clause is still one the set has
            json!({"scans": {"grain": "stack", "where": [is("body_part", "brain"), is("body_region", "head")]}}),
            // the coarse clause in the set it is drawn from
            json!({
                "heads": {"grain": "stack", "where": [is("body_region", "head")]},
                "scans": {"grain": "stack", "from": "heads", "where": [is("body_part", "brain")]}
            }),
            // and among several coarse values that cover it
            json!({"scans": {"grain": "stack", "where": [
                ["in", {}, ["axis", {}, "body_region"], ["head", "spine"]],
                is("body_part", "brain")
            ]}}),
        ];
        for sets in deliberate {
            let text = doc(sets.clone(), "scans");
            for d in diagnosed(&mut l, &text, &quasi()) {
                assert!(d.valid, "{}: {sets}", l.name);
                assert_eq!(narrow(&d), [], "{}: {sets}", l.name);
                assert!(
                    d.warnings.iter().all(|w| !w.next.contains("body_region")),
                    "{}: {sets}",
                    l.name
                );
            }
        }
        // a coarse clause that does not cover the value kept is no reason
        // to keep quiet: the spines hold no brain, and nothing is kept to
        // warn of either
        let other = doc(
            json!({"scans": {"grain": "stack", "where": [is("body_region", "spine"), is("body_part", "brain")]}}),
            "scans",
        );
        for d in diagnosed(&mut l, &other, &quasi()) {
            assert_eq!(narrow(&d), [], "{}", l.name);
        }
    }
}

/// A clause is measured against its set, not against the rows before it
/// in the list: the same set with the value clause first, in the middle
/// and last gives the same warning with the same numbers, the rows the
/// set holds without the clause and what the clause keeps of them, each
/// held here against a count of its own.
#[test]
fn the_warning_is_the_same_wherever_the_clause_stands() {
    for mut l in labs() {
        plant(&mut l, false);
        let anat = is("directory_type", "anat");
        let day = json!([">=", {}, ["field", {}, "day"], "1900-01-01"]);
        let brain = is("body_part", "brain");
        let stack_count = |l: &mut Lab, clauses: Value| {
            count(
                l,
                &doc(
                    json!({"scans": {"grain": "stack", "where": clauses}}),
                    "scans",
                ),
            )
        };
        let all = stack_count(&mut l, json!([anat, day]));
        let kept = stack_count(&mut l, json!([anat, day, brain]));
        let neck = stack_count(&mut l, json!([anat, day, is("body_part", "brain-neck")]));
        assert!(kept > 0 && kept * 5 < all && neck >= 3 * kept, "{}", l.name);
        let expected = format!(
            "scans: with the set's other conditions, body_part = brain keeps {} of {} stacks; \
             brain-neck holds {} of the rest. If any head of body_region is meant, write \
             body_region = head, which covers brain and brain-neck.",
            n(kept),
            n(all),
            n(neck)
        );
        for (at, clauses) in [
            (0, json!([brain, anat, day])),
            (1, json!([anat, brain, day])),
            (2, json!([anat, day, brain])),
        ] {
            let text = visits_with(clauses);
            for d in diagnosed(&mut l, &text, &quasi()) {
                let w = narrow(&d);
                assert_eq!(w.len(), 1, "{}: at {at}: {:?}", l.name, d.warnings);
                assert_eq!(w[0].0, format!("sets.scans.where[{at}]"), "{}", l.name);
                assert_eq!(w[0].1, expected, "{}: at {at}", l.name);
                assert!(
                    w[0].2.contains(&format!("at sets.scans.where[{at}]")),
                    "{}: {}",
                    l.name,
                    w[0].2
                );
            }
        }
    }
}

/// A clause that keeps little of the rows before it, where the set's
/// other conditions already narrow to the value it keeps: without the
/// clause the set holds the same rows, so nothing was thrown away and
/// nothing is warned of, whichever of the two is written first. And the
/// other way round: a clause that keeps every row before it, in a set
/// whose other conditions leave it nothing, is warned about.
#[test]
fn a_clause_the_other_conditions_already_answer_is_left_alone() {
    for mut l in labs() {
        let (counts, stacks) = plant(&mut l, false);
        let brain = counts[&("body_part", "brain")];
        // as planted, a contrast agent was given on the brains and on no
        // other stack, and the contrast mix of a brain is one no other has
        assert_eq!(counts[&("post_contrast", "1")], brain, "{}", l.name);
        assert!(brain * 5 < stacks, "{}: few at its own place", l.name);
        let given = is("post_contrast", "given");
        let part = is("body_part", "brain");
        for clauses in [json!([part, given]), json!([given, part])] {
            let text = visits_with(clauses.clone());
            for d in diagnosed(&mut l, &text, &quasi()) {
                assert!(d.valid, "{}", l.name);
                assert_eq!(narrow(&d), [], "{}: {clauses}", l.name);
            }
        }
        // the brains hold one contrast mix and every other stack another:
        // after the mix of the brains the clause keeps every row before it,
        // and beside the mix of the others it keeps none of the many the
        // set would hold, most of them a brain and neck
        let others = is("contrast_mix", "PD_T2star");
        let without = count(
            &mut l,
            &doc(
                json!({"scans": {"grain": "stack", "where": [others]}}),
                "scans",
            ),
        );
        for (at, clauses) in [(0, json!([part, others])), (1, json!([others, part]))] {
            let text = visits_with(clauses);
            for d in diagnosed(&mut l, &text, &quasi()) {
                let w = narrow(&d);
                assert_eq!(w.len(), 1, "{}: {:?}", l.name, d.warnings);
                assert_eq!(w[0].0, format!("sets.scans.where[{at}]"), "{}", l.name);
                assert!(
                    w[0].1.contains(&format!(
                        "with the set's other conditions, body_part = brain keeps 0 of {} stacks",
                        n(without)
                    )),
                    "{}: {}",
                    l.name,
                    w[0].1
                );
            }
        }
    }
}

/// The clauses measured are those of the sets the answer reads its rows
/// through, before any other set's, and among them those of the sets that
/// hold the fewest rows.
#[test]
fn the_clauses_measured_are_the_answer_s_own_sets_and_the_smallest_first() {
    for mut l in labs() {
        plant(&mut l, false);
        let brain = is("body_part", "brain");
        let of =
            |kind: &str| json!({"grain": "stack", "where": [brain, is("directory_type", kind)]});
        // four sets the answer reads, the first of them the largest, and
        // one it does not read, declared before them all
        let sets = json!({
            "aside": of("misc"),
            "every": {"grain": "stack", "where": [brain]},
            "scans_a": of("anat"),
            "scans_d": of("dwi"),
            "scans_f": of("func"),
            "visits": {"grain": "session", "has": [
                {"set": "every", "min": 1},
                {"set": "scans_a", "min": 1},
                {"set": "scans_d", "min": 1},
                {"set": "scans_f", "min": 1}
            ]}
        });
        let text = doc(sets, "visits");
        for d in diagnosed(&mut l, &text, &quasi()) {
            assert!(d.valid, "{}", l.name);
            let mut warned: Vec<String> = narrow(&d).into_iter().map(|w| w.0).collect();
            warned.sort();
            assert_eq!(
                warned,
                [
                    "sets.scans_a.where[0]",
                    "sets.scans_d.where[0]",
                    "sets.scans_f.where[0]",
                ],
                "{}: {:?}",
                l.name,
                d.warnings
            );
        }
    }
}
