// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a: an axis value in a document names the value, whichever of its
//! names the document writes. A pack may store a value's label rather than
//! its identity (`stores: label`): the rules write `1` for post_contrast's
//! `given` and `T2*w` for base's `T2starw`, and a person's decision may hold
//! the identity. Both forms are planted on each backend's synthetic
//! registry, and every answer is held against the rows as planted: `=`,
//! `<>`, `in`, `not_in` and `has` by identity and by label, the visits with
//! contrast found through `has`, a group keyed by such an axis counting each
//! value once in the form rows store, a group's `where` on that key, the
//! catalog's listing, its value sampler, and a draft written the way the
//! assistant writes one.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Lab, quasi, run_ask};
use nils_ask::affordance::{self, Setting};
use nils_ask::exec::{Answer, Bounds};
use nils_ask::handle::cell_json;
use nils_ask::validate::{Code, Scope};
use nils_ask::{Error as AskError, parse, prepare};
use nils_registry::schema::table;
use nils_registry::session::Scheme;
use nils_registry::{Insert, Param, Store};
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_axis_names_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// One stack as a stack set sees it (none the pack excluded): its subject,
/// its session under the default scheme, its base and its technique.
struct Seen {
    stack: i64,
    subject: i64,
    session: Option<i64>,
    base: Option<String>,
    technique: Option<String>,
}

fn seen(store: &mut Store) -> Vec<Seen> {
    let q = |t: &str| store.qualified(t);
    let sql = format!(
        "SELECT st.id, se.subject_id, sc.id, \
         (SELECT b.value FROM {ax} b WHERE b.stack_id = st.id AND b.axis = 'base'), \
         (SELECT t.value FROM {ax} t WHERE t.stack_id = st.id AND t.axis = 'technique') \
         FROM {stack} st JOIN {series} se ON se.id = st.series_id JOIN {study} sy ON sy.id = se.study_id \
         JOIN {fp} f ON f.stack_id = st.id \
         LEFT JOIN {scs} scs ON scs.study_id = sy.id AND scs.window_days = 0 \
         LEFT JOIN {sc} sc ON sc.id = scs.session_id AND sc.subject_id = se.subject_id \
         WHERE NOT EXISTS (SELECT 1 FROM {ax} x WHERE x.stack_id = st.id AND x.axis = 'disposition' AND x.value = 'excluded') \
         ORDER BY st.id",
        ax = q("classification_axis"),
        stack = q("stack"),
        series = q("series"),
        study = q("study"),
        fp = q("stack_fingerprint"),
        scs = q("session_cache_study"),
        sc = q("session_cache"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| Seen {
            stack: r.int(0).unwrap(),
            subject: r.int(1).unwrap(),
            session: r.opt_int(2).unwrap(),
            base: r.opt_text(3).unwrap().map(str::to_string),
            technique: r.opt_text(4).unwrap().map(str::to_string),
        })
        .collect()
}

/// A row as the rules write it, or as a person's decision holds it.
fn put(store: &mut Store, stack: i64, axis: &str, value: &str, tier: &str) {
    store
        .insert(
            &Insert::new(
                table("classification_axis"),
                &["stack_id", "axis", "value", "confidence", "tier"],
            ),
            &[vec![
                Param::Int(stack),
                Param::from(axis),
                Param::from(value),
                Param::Double(0.9),
                Param::from(tier),
            ]],
        )
        .unwrap();
}

/// A single-valued axis's row rewritten to another value.
fn set(store: &mut Store, stack: i64, axis: &str, value: &str) {
    let sql = format!(
        "UPDATE {} SET value = '{value}' WHERE stack_id = {stack} AND axis = '{axis}'",
        store.qualified("classification_axis")
    );
    store.execute(&sql, &[]).unwrap();
}

/// What was planted, by stack.
struct Planted {
    /// post_contrast `given`: `1` as the rules store it, and `given` once,
    /// as a person's decision holds it.
    given: BTreeSet<i64>,
    /// post_contrast `not_given`: `0`, and `not_given` once.
    not_given: BTreeSet<i64>,
    /// base `T2starw`, on stacks that said T2w: `T2*w` twice, `T2starw` once.
    t2star: BTreeSet<i64>,
    /// technique `3D-TSE`, on stacks that said TSE: `SPACE`, and `3D-TSE`.
    space: BTreeSet<i64>,
    /// modifier `WaterExcitation`, beside a stack's own: `WaterExc`, and
    /// `WaterExcitation`.
    water: BTreeSet<i64>,
    /// The subject whose first three visits each hold a T1w stack given
    /// contrast, and those visits.
    subject: i64,
    visits: BTreeSet<i64>,
    /// Every stack a stack set sees, and every subject.
    stacks: BTreeSet<i64>,
    subjects: BTreeSet<i64>,
}

fn all_subjects(store: &mut Store) -> BTreeSet<i64> {
    let sql = format!(
        "SELECT id FROM {} WHERE merged_into IS NULL",
        store.qualified("subject")
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

fn plant(l: &mut Lab) -> Planted {
    let rows = seen(l.registry.store());
    // the subject with the most visits that hold a T1w stack
    let mut t1w: BTreeMap<i64, BTreeMap<i64, i64>> = BTreeMap::new();
    for r in &rows {
        if let (Some(s), Some("T1w")) = (r.session, r.base.as_deref()) {
            t1w.entry(r.subject)
                .or_default()
                .entry(s)
                .or_insert(r.stack);
        }
    }
    let (&subject, visits) = t1w
        .iter()
        .max_by_key(|(id, v)| (v.len(), std::cmp::Reverse(**id)))
        .unwrap();
    assert!(visits.len() >= 3, "{}: {visits:?}", l.name);
    let visits: Vec<(i64, i64)> = visits.iter().take(3).map(|(s, t)| (*s, *t)).collect();
    let store = l.registry.store();
    let mut given = BTreeSet::new();
    for (i, (_, stack)) in visits.iter().enumerate() {
        if i == 1 {
            put(store, *stack, "post_contrast", "given", "decision");
        } else {
            put(store, *stack, "post_contrast", "1", "exclusive");
        }
        given.insert(*stack);
    }
    // two other subjects' T1w stacks not given contrast
    let others: Vec<i64> = t1w
        .iter()
        .filter(|(s, _)| **s != subject)
        .take(2)
        .map(|(_, v)| *v.values().next().unwrap())
        .collect();
    put(store, others[0], "post_contrast", "0", "keywords");
    put(store, others[1], "post_contrast", "not_given", "decision");
    let not_given: BTreeSet<i64> = others.iter().copied().collect();
    // three T2w stacks of three other subjects T2*w, one of them written as
    // its identity; then two TSE stacks 3D-TSE and two MPRAGE stacks with
    // water excitation, each once by its label and once by its identity
    let by_subject = |want: &dyn Fn(&Seen) -> bool, skip: &BTreeSet<i64>, n: usize| {
        let mut out: Vec<i64> = Vec::new();
        let mut used: BTreeSet<i64> = BTreeSet::new();
        for r in &rows {
            if want(r) && !skip.contains(&r.subject) && used.insert(r.subject) {
                out.push(r.stack);
                if out.len() == n {
                    break;
                }
            }
        }
        out
    };
    let none = BTreeSet::from([subject]);
    let t2 = by_subject(&|r| r.base.as_deref() == Some("T2w"), &none, 3);
    set(store, t2[0], "base", "T2*w");
    set(store, t2[1], "base", "T2*w");
    set(store, t2[2], "base", "T2starw");
    let mut skip = none.clone();
    for r in &rows {
        if t2.contains(&r.stack) {
            skip.insert(r.subject);
        }
    }
    let tse = by_subject(&|r| r.technique.as_deref() == Some("TSE"), &skip, 2);
    set(store, tse[0], "technique", "SPACE");
    set(store, tse[1], "technique", "3D-TSE");
    let mprage = by_subject(&|r| r.technique.as_deref() == Some("MPRAGE"), &skip, 2);
    put(store, mprage[0], "modifier", "WaterExc", "exclusive");
    put(store, mprage[1], "modifier", "WaterExcitation", "decision");
    assert_eq!((t2.len(), tse.len(), mprage.len()), (3, 2, 2), "{}", l.name);
    Planted {
        given,
        not_given,
        t2star: t2.into_iter().collect(),
        space: tse.into_iter().collect(),
        water: mprage.into_iter().collect(),
        subject,
        visits: visits.iter().map(|(s, _)| *s).collect(),
        stacks: rows.iter().map(|r| r.stack).collect(),
        subjects: all_subjects(store),
    }
}

fn answer(l: &mut Lab, doc: Value) -> Answer {
    let ask = parse(&doc.to_string()).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    run_ask(l, ask).1
}

/// One column of an answer, by name.
fn column(a: &Answer, name: &str) -> Vec<Value> {
    let i = a
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no column {name} in {:?}", a.columns));
    a.rows.iter().map(|r| cell_json(&r.0[i])).collect()
}

fn ids(a: &Answer, name: &str) -> BTreeSet<i64> {
    column(a, name)
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect()
}

/// The stacks one clause keeps.
fn stacks(l: &mut Lab, clause: &Value) -> BTreeSet<i64> {
    let a = answer(
        l,
        json!({
            "ast_version": 1,
            "sets": {"t": {"grain": "stack", "where": [clause]}},
            "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"]]}
        }),
    );
    ids(&a, "id")
}

/// A group answer as key to rows.
fn grouped(a: &Answer, key: &str) -> BTreeMap<Option<String>, i64> {
    column(a, key)
        .iter()
        .zip(column(a, "_rows"))
        .map(|(k, n)| (k.as_str().map(str::to_string), n.as_i64().unwrap()))
        .collect()
}

fn minus(a: &BTreeSet<i64>, b: &BTreeSet<i64>) -> BTreeSet<i64> {
    a.difference(b).copied().collect()
}

#[test]
fn a_value_is_found_by_its_identity_or_its_label_whichever_form_a_row_holds() {
    for mut l in labs() {
        let p = plant(&mut l);
        let axis = |name: &str| json!(["axis", {}, name]);
        let cases: Vec<(&str, &str, &str, &BTreeSet<i64>)> = vec![
            ("=", "post_contrast", "given", &p.given),
            ("=", "post_contrast", "1", &p.given),
            ("=", "post_contrast", "not_given", &p.not_given),
            ("=", "post_contrast", "0", &p.not_given),
            ("=", "base", "T2starw", &p.t2star),
            ("=", "base", "T2*w", &p.t2star),
            ("=", "technique", "3D-TSE", &p.space),
            ("=", "technique", "SPACE", &p.space),
            ("has", "modifier", "WaterExcitation", &p.water),
            ("has", "modifier", "WaterExc", &p.water),
        ];
        for (op, name, value, want) in cases {
            let clause = json!([op, {}, axis(name), value]);
            assert_eq!(&stacks(&mut l, &clause), want, "{}: {clause}", l.name);
        }
        // <> keeps every stack that does not hold the value, in either form
        for value in ["given", "1"] {
            let clause = json!(["<>", {}, axis("post_contrast"), value]);
            assert_eq!(
                stacks(&mut l, &clause),
                minus(&p.stacks, &p.given),
                "{}: {clause}",
                l.name
            );
        }
        for value in ["T2starw", "T2*w"] {
            let clause = json!(["<>", {}, axis("base"), value]);
            assert_eq!(
                stacks(&mut l, &clause),
                minus(&p.stacks, &p.t2star),
                "{}: {clause}",
                l.name
            );
        }
        // a number is read as its digits, and a parameter as its value
        let clause = json!(["=", {}, axis("post_contrast"), 1]);
        assert_eq!(stacks(&mut l, &clause), p.given, "{}: {clause}", l.name);
        for value in ["given", "1"] {
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "params": {"contrast": {"type": "text", "value": value}},
                    "sets": {"t": {"grain": "stack", "where": [
                        ["=", {}, axis("post_contrast"), ["param", {}, "contrast"]]
                    ]}},
                    "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"]]}
                }),
            );
            assert_eq!(ids(&a, "id"), p.given, "{}: {value}", l.name);
        }
        // what was right before is unchanged: a value whose identity is its
        // label, read directly
        let t1w: BTreeSet<i64> = seen(l.registry.store())
            .iter()
            .filter(|r| r.base.as_deref() == Some("T1w"))
            .map(|r| r.stack)
            .collect();
        assert!(!t1w.is_empty(), "{}", l.name);
        assert_eq!(
            stacks(&mut l, &json!(["=", {}, axis("base"), "T1w"])),
            t1w,
            "{}",
            l.name
        );
        // a name that is no value's is refused, with the axis named
        let refused = parse(
            &json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["=", {}, axis("post_contrast"), "givn"]]}},
                "out": {"set": "t", "level": "count"}
            })
            .to_string(),
        )
        .unwrap();
        match prepare(refused, &l.catalog, &quasi()) {
            Err(AskError::Invalid(issues)) => {
                let i = issues
                    .iter()
                    .find(|i| i.code == Code::UnknownValue)
                    .unwrap_or_else(|| panic!("{}: {issues:?}", l.name));
                assert!(
                    i.message.contains("givn") && i.message.contains("post_contrast"),
                    "{}: {}",
                    l.name,
                    i.message
                );
            }
            other => panic!("{}: a misspelt value was taken: {other:?}", l.name),
        }
    }
}

#[test]
fn in_and_not_in_list_the_values_of_an_axis_by_either_name() {
    for mut l in labs() {
        let p = plant(&mut l);
        let both: BTreeSet<i64> = p.given.union(&p.not_given).copied().collect();
        for list in [
            json!(["given", "not_given"]),
            json!(["1", "0"]),
            json!(["given", "0"]),
        ] {
            let clause = json!(["in", {}, ["axis", {}, "post_contrast"], list]);
            assert_eq!(stacks(&mut l, &clause), both, "{}: {clause}", l.name);
            let clause = json!(["not_in", {}, ["axis", {}, "post_contrast"], list]);
            assert_eq!(
                stacks(&mut l, &clause),
                minus(&p.stacks, &both),
                "{}: {clause}",
                l.name
            );
        }
        let t1w: BTreeSet<i64> = seen(l.registry.store())
            .iter()
            .filter(|r| r.base.as_deref() == Some("T1w"))
            .map(|r| r.stack)
            .collect();
        let want: BTreeSet<i64> = t1w.union(&p.t2star).copied().collect();
        for list in [json!(["T2starw", "T1w"]), json!(["T2*w", "T1w"])] {
            let clause = json!(["in", {}, ["axis", {}, "base"], list]);
            assert_eq!(stacks(&mut l, &clause), want, "{}: {clause}", l.name);
        }
        // a list parameter binds the same way
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "params": {"bases": {"type": "list", "value": ["T2*w"]}},
                "sets": {"t": {"grain": "stack", "where": [["in", {}, ["axis", {}, "base"], ["param", {}, "bases"]]]}},
                "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"]]}
            }),
        );
        assert_eq!(ids(&a, "id"), p.t2star, "{}", l.name);
    }
}

#[test]
fn the_visits_with_contrast_are_found_whichever_name_the_question_uses() {
    let mut hashes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for mut l in labs() {
        let p = plant(&mut l);
        for value in ["given", "1"] {
            // one subject's visits with contrast, as the dataset viewer
            // finds them
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "sets": {
                        "scans": {"grain": "stack", "where": [
                            ["=", {}, ["axis", {}, "post_contrast"], value],
                            ["=", {}, ["field", {}, "subject.id"], p.subject]
                        ]},
                        "visits": {"grain": "session", "has": [{"set": "scans", "min": 1}]}
                    },
                    "out": {"set": "visits", "level": "record", "columns": [["field", {}, "id"]],
                            "order": [[["field", {}, "id"], "asc"]]}
                }),
            );
            assert_eq!(ids(&a, "id"), p.visits, "{}: {value}", l.name);
            hashes
                .entry(l.name)
                .or_default()
                .push(a.content_hash.clone().unwrap());
            // and the subjects with any scan given contrast
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "sets": {
                        "scans": {"grain": "stack", "where": [["=", {}, ["axis", {}, "post_contrast"], value]]},
                        "people": {"grain": "subject", "has": [{"set": "scans", "min": 1}]}
                    },
                    "out": {"set": "people", "level": "record", "columns": [["field", {}, "id"]]}
                }),
            );
            assert_eq!(ids(&a, "id"), BTreeSet::from([p.subject]), "{}", l.name);
            // and those with none: every other subject
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "sets": {
                        "scans": {"grain": "stack", "where": [["=", {}, ["axis", {}, "post_contrast"], value]]},
                        "people": {"grain": "subject", "has": [{"set": "scans", "max": 0}]}
                    },
                    "out": {"set": "people", "level": "record", "columns": [["field", {}, "id"]]}
                }),
            );
            assert_eq!(
                ids(&a, "id"),
                minus(&p.subjects, &BTreeSet::from([p.subject])),
                "{}",
                l.name
            );
        }
    }
    let mut it = hashes.values();
    if let (Some(a), Some(b)) = (it.next(), it.next()) {
        assert_eq!(a, b, "the backends' answers agree");
    }
}

#[test]
fn a_group_keyed_by_an_axis_counts_each_value_once_in_the_form_rows_store() {
    let mut hashes: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for mut l in labs() {
        let p = plant(&mut l);
        let group = |by: Value, where_: Option<Value>| {
            let key = by[2].clone();
            let mut g = json!({"grain": "group", "group": {"of": "t", "by": [by]}});
            if let Some(w) = where_ {
                g["where"] = json!([w]);
            }
            json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack"}, "g": g},
                "out": {"set": "g", "level": "aggregate", "columns": [
                    ["field", {}, key], ["field", {}, "_rows"], ["field", {}, "_subjects"]
                ]}
            })
        };
        // post_contrast: one key per value, whichever form its rows hold
        let a = answer(&mut l, group(json!(["axis", {}, "post_contrast"]), None));
        let rest = (p.stacks.len() - p.given.len() - p.not_given.len()) as i64;
        let want = BTreeMap::from([
            (Some("1".to_string()), p.given.len() as i64),
            (Some("0".to_string()), p.not_given.len() as i64),
            (None, rest),
        ]);
        assert_eq!(grouped(&a, "post_contrast"), want, "{}", l.name);
        hashes
            .entry(l.name)
            .or_default()
            .push(a.content_hash.clone().unwrap());
        // base: T2*w once
        let a = answer(&mut l, group(json!(["axis", {}, "base"]), None));
        let by = grouped(&a, "base");
        assert_eq!(
            by.get(&Some("T2*w".to_string())),
            Some(&(p.t2star.len() as i64)),
            "{}: {by:?}",
            l.name
        );
        assert!(
            !by.contains_key(&Some("T2starw".to_string())),
            "{}: {by:?}",
            l.name
        );
        // a value of several, each: WaterExc once
        let a = answer(
            &mut l,
            group(json!(["axis", {"each": true}, "modifier"]), None),
        );
        let by = grouped(&a, "modifier");
        assert_eq!(
            by.get(&Some("WaterExc".to_string())),
            Some(&(p.water.len() as i64)),
            "{}: {by:?}",
            l.name
        );
        assert!(
            !by.contains_key(&Some("WaterExcitation".to_string())),
            "{}: {by:?}",
            l.name
        );
        // the group's where on its key reads a value by either name
        for value in ["given", "1"] {
            let a = answer(
                &mut l,
                group(
                    json!(["axis", {}, "post_contrast"]),
                    Some(json!(["=", {}, ["field", {}, "post_contrast"], value])),
                ),
            );
            assert_eq!(
                grouped(&a, "post_contrast"),
                BTreeMap::from([(Some("1".to_string()), p.given.len() as i64)]),
                "{}: {value}",
                l.name
            );
        }
        let a = answer(
            &mut l,
            group(
                json!(["axis", {}, "post_contrast"]),
                Some(json!([
                    "in",
                    {},
                    ["field", {}, "post_contrast"],
                    ["given", "not_given"]
                ])),
            ),
        );
        assert_eq!(
            grouped(&a, "post_contrast"),
            BTreeMap::from([
                (Some("1".to_string()), p.given.len() as i64),
                (Some("0".to_string()), p.not_given.len() as i64),
            ]),
            "{}",
            l.name
        );
        hashes
            .entry(l.name)
            .or_default()
            .push(a.content_hash.clone().unwrap());
    }
    let mut it = hashes.values();
    if let (Some(a), Some(b)) = (it.next(), it.next()) {
        assert_eq!(a, b, "the backends' answers agree");
    }
}

#[test]
fn the_catalog_says_what_rows_store_and_its_sampler_and_a_draft_agree() {
    for mut l in labs() {
        let p = plant(&mut l);
        // the listing: both names of every value, and which one rows store
        let doc = l.catalog.document(&Scope::default());
        let axes = doc["axes"].as_array().unwrap();
        let of = |name: &str| {
            axes.iter()
                .find(|a| a["name"] == name)
                .unwrap_or_else(|| panic!("no axis {name}"))
                .clone()
        };
        let pc = of("post_contrast");
        assert_eq!(pc["stores"], "label", "{}: {pc}", l.name);
        assert!(
            pc["values"]
                .as_array()
                .unwrap()
                .contains(&json!({"id": "given", "label": "1"})),
            "{}: {pc}",
            l.name
        );
        assert_eq!(of("disposition")["stores"], "id", "{}", l.name);
        // the value sampler: each value once, in the form rows store
        let scope = Scope::default();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let sample = affordance::values(&mut l.registry, "stack", "post_contrast", &s, 50, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert_eq!(sample.kind, "values", "{}", l.name);
        let items: BTreeMap<String, i64> = sample.items.iter().cloned().collect();
        assert_eq!(
            items.get("1"),
            Some(&(p.given.len() as i64)),
            "{}: {items:?}",
            l.name
        );
        assert_eq!(
            items.get("0"),
            Some(&(p.not_given.len() as i64)),
            "{}: {items:?}",
            l.name
        );
        assert!(
            !items.contains_key("given") && !items.contains_key("not_given"),
            "{}: {items:?}",
            l.name
        );
        assert_eq!(sample.distinct, items.len() as i64, "{}: {items:?}", l.name);
        let sample = affordance::values(&mut l.registry, "stack", "base", &s, 50, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let items: BTreeMap<String, i64> = sample.items.iter().cloned().collect();
        assert_eq!(
            items.get("T2*w"),
            Some(&(p.t2star.len() as i64)),
            "{}: {items:?}",
            l.name
        );
        assert!(!items.contains_key("T2starw"), "{}: {items:?}", l.name);
        // a draft as the assistant writes one, by identity and by label, the
        // label quoted or left a number
        for value in ["\"given\"", "\"1\"", "1"] {
            let text = [
                "ast_version: 1",
                "name: scans given contrast",
                "scheme: default",
                "sets:",
                "  scans:",
                "    grain: stack",
                "    where:",
                &format!("      - [\"=\", {{}}, [\"axis\", {{}}, \"post_contrast\"], {value}]"),
                "out: {set: scans, level: count}",
            ]
            .join("\n");
            let drafted = affordance::draft(&mut l.registry, &text, &s, None).unwrap();
            assert!(
                drafted.diagnosis.valid && drafted.document.is_some(),
                "{}: {value}: {:?}",
                l.name,
                drafted.diagnosis.issues
            );
            let kept = drafted
                .diagnosis
                .funnel
                .iter()
                .rfind(|st| st.set == "scans")
                .unwrap_or_else(|| panic!("{}: {:?}", l.name, drafted.diagnosis.funnel));
            assert_eq!(kept.rows, p.given.len() as i64, "{}: {value}", l.name);
        }
    }
}
