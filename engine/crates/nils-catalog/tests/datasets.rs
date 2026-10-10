// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a: the ask's `dataset` field. A stack is of the dataset whose
//! digest first read it (an active source place that is a dataset, not a
//! root); a subject or a session is of every dataset one of its stacks is
//! of. Two datasets are planted on each backend's synthetic registry, a
//! third of the subjects in each and a third in none, one subject with a
//! stack of both, and every answer is held against the registry read
//! directly: the T1w stacks of one dataset counted, the stacks grouped by
//! dataset, the subject of both found in each and grouped under each, its
//! session likewise, the field in the catalog, the value sampler, the
//! preview, a handle read again by dataset, a misspelt name refused, and a
//! root and a retired place never a dataset.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use common::{Lab, quasi, refresh, run_ask};
use nils_ask::affordance::{self, Setting};
use nils_ask::exec::{Answer, Bounds};
use nils_ask::handle::cell_json;
use nils_ask::run::{Request, run};
use nils_ask::validate::{Class, Code, Scope};
use nils_ask::{Error as AskError, parse, prepare};
use nils_dicom::synth::TempDir;
use nils_registry::place::{self, New, Role};
use nils_registry::schema::table;
use nils_registry::session::Scheme;
use nils_registry::{Insert, Param, Store};
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_dataset_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

const BIG: &str = "study-big";
const SMALL: &str = "study-small";

/// What was planted: the folders, the two datasets' places, and each
/// dataset's batch.
struct Planted {
    _dir: TempDir,
    small_place: i64,
    big_batch: i64,
    small_batch: i64,
    /// The subject with stacks of both datasets, and the session that holds
    /// a stack of each.
    shared: i64,
    shared_session: i64,
}

fn add_place(store: &mut Store, name: &str, path: &Path, dataset: Value) -> i64 {
    place::add(
        store,
        &New {
            name,
            role: Role::Source,
            path: &path.display().to_string(),
            guarantees: json!({}),
            probed: Value::Null,
            handling: Value::Null,
            dataset,
        },
    )
    .unwrap()
}

/// The stacks a stack set sees (none the pack excluded), each as (stack,
/// subject, session under the default scheme, first batch, T1w).
fn seen(store: &mut Store) -> Vec<(i64, i64, Option<i64>, i64, bool)> {
    let q = |t: &str| store.qualified(t);
    let sql = format!(
        "SELECT st.id, se.subject_id, sc.id, st.first_batch_id, \
         CASE WHEN EXISTS (SELECT 1 FROM {ax} b WHERE b.stack_id = st.id AND b.axis = 'base' AND b.value = 'T1w') THEN 1 ELSE 0 END \
         FROM {stack} st JOIN {series} se ON se.id = st.series_id JOIN {study} sy ON sy.id = se.study_id \
         JOIN {subject} su ON su.id = se.subject_id JOIN {fp} f ON f.stack_id = st.id \
         LEFT JOIN {scs} scs ON scs.study_id = sy.id AND scs.window_days = 0 \
         LEFT JOIN {sc} sc ON sc.id = scs.session_id AND sc.subject_id = se.subject_id \
         WHERE NOT EXISTS (SELECT 1 FROM {ax} x WHERE x.stack_id = st.id AND x.axis = 'disposition' AND x.value = 'excluded') \
         ORDER BY st.id",
        ax = q("classification_axis"),
        stack = q("stack"),
        series = q("series"),
        study = q("study"),
        subject = q("subject"),
        fp = q("stack_fingerprint"),
        scs = q("session_cache_study"),
        sc = q("session_cache"),
    );
    store
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.int(0).unwrap(),
                r.int(1).unwrap(),
                r.opt_int(2).unwrap(),
                r.int(3).unwrap(),
                r.int(4).unwrap() == 1,
            )
        })
        .collect()
}

/// Two datasets under a root: the stacks of every subject whose id is 1
/// modulo 3 are the first's, 2 modulo 3 the second's, the rest no
/// dataset's; then one stack of a subject of the first, from a study that
/// keeps another of its stacks there, becomes the second's.
fn plant(l: &mut Lab) -> Planted {
    let dir = TempDir::new("ask-datasets");
    let tree = |name: &str| {
        let p = dir.path().join(name).join("derivatives/dcm-anon");
        std::fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    };
    let (big_tree, small_tree) = (tree(BIG), tree(SMALL));
    let store = l.registry.store();
    // the root the two were added under, which is no dataset of its own
    add_place(store, "archive", dir.path(), json!({"kind": "root"}));
    add_place(store, BIG, &dir.path().join(BIG), Value::Null);
    let small_place = add_place(store, SMALL, &dir.path().join(SMALL), Value::Null);
    let now = "2026-10-09T08:00:00Z";
    let mut batch_of = |root: &Path| -> i64 {
        let root = root.display().to_string();
        let source = store
            .insert(
                &Insert::new(
                    table("source"),
                    &["root", "root_canonical", "first_seen_at"],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(root.as_str()),
                    Param::from(root.as_str()),
                    Param::from(now),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        store
            .insert(
                &Insert::new(
                    table("ingest_batch"),
                    &[
                        "source_id",
                        "name",
                        "config",
                        "started_at",
                        "finished_at",
                        "state",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::Int(source),
                    Param::from("read"),
                    Param::from("{}"),
                    Param::from(now),
                    Param::from(now),
                    Param::from("done"),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap()
    };
    let (big_batch, small_batch) = (batch_of(&big_tree), batch_of(&small_tree));
    let all = seen(store);
    let (mut big, mut small): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
    for (stack, subject, _, _, _) in &all {
        match subject % 3 {
            1 => big.push(*stack),
            2 => small.push(*stack),
            _ => {}
        }
    }
    // a session of the first dataset's that holds two stacks seen: its
    // last moves to the second
    let mut by_session: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
    for (stack, subject, session, _, _) in &all {
        if let Some(s) = session
            && subject % 3 == 1
        {
            by_session.entry(*s).or_default().push((*stack, *subject));
        }
    }
    let (shared_session, stacks) = by_session
        .iter()
        .find(|(_, v)| v.len() >= 2)
        .expect("a session of two stacks");
    let (moved, shared) = *stacks.last().unwrap();
    big.retain(|s| *s != moved);
    small.push(moved);
    let list = |ids: &[i64]| {
        ids.iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (batch, ids) in [(big_batch, &big), (small_batch, &small)] {
        store
            .execute(
                &format!(
                    "UPDATE {} SET first_batch_id = {batch} WHERE id IN ({})",
                    store.qualified("stack"),
                    list(ids)
                ),
                &[],
            )
            .unwrap();
    }
    // contrast given on every other T1w stack, which the synthetic
    // registry never says
    let given: Vec<Vec<Param>> = all
        .iter()
        .filter(|(stack, _, _, _, t1w)| *t1w && stack % 2 == 0)
        .map(|(stack, _, _, _, _)| {
            vec![
                Param::Int(*stack),
                Param::from("post_contrast"),
                Param::from("given"),
                Param::Double(1.0),
                Param::from("exclusive"),
            ]
        })
        .collect();
    store
        .insert(
            &Insert::new(
                table("classification_axis"),
                &["stack_id", "axis", "value", "confidence", "tier"],
            ),
            &given,
        )
        .unwrap();
    let shared_session = *shared_session;
    refresh(l);
    Planted {
        _dir: dir,
        small_place,
        big_batch,
        small_batch,
        shared,
        shared_session,
    }
}

fn answer(l: &mut Lab, doc: Value) -> Answer {
    let ask = parse(&doc.to_string()).unwrap_or_else(|e| panic!("{}: {e}", l.name));
    run_ask(l, ask).1
}

/// A count answer's rows and subjects.
fn counted(a: &Answer) -> (i64, i64) {
    let row = &a.rows[0].0;
    let n = |i: usize| cell_json(&row[i]).as_i64().unwrap();
    (n(0), n(1))
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

/// A group answer as key to (rows, subjects).
fn grouped(a: &Answer, key: &str) -> BTreeMap<Option<String>, (i64, i64)> {
    let keys = column(a, key);
    let rows = column(a, "_rows");
    let subjects = column(a, "_subjects");
    keys.iter()
        .zip(rows.iter().zip(subjects.iter()))
        .map(|(k, (r, s))| {
            (
                k.as_str().map(str::to_string),
                (r.as_i64().unwrap(), s.as_i64().unwrap()),
            )
        })
        .collect()
}

/// Where the planted stacks are: per dataset (none for no dataset's), the
/// stacks seen, their subjects and their sessions.
struct Truth {
    stacks: BTreeMap<Option<&'static str>, BTreeSet<i64>>,
    t1w: BTreeMap<Option<&'static str>, (i64, BTreeSet<i64>)>,
    subjects: BTreeMap<Option<&'static str>, BTreeSet<i64>>,
    sessions: BTreeMap<Option<&'static str>, BTreeSet<i64>>,
}

fn truth(l: &mut Lab, p: &Planted) -> Truth {
    let mut t = Truth {
        stacks: BTreeMap::new(),
        t1w: BTreeMap::new(),
        subjects: BTreeMap::new(),
        sessions: BTreeMap::new(),
    };
    for (stack, subject, session, batch, t1w) in seen(l.registry.store()) {
        let ds = if batch == p.big_batch {
            Some(BIG)
        } else if batch == p.small_batch {
            Some(SMALL)
        } else {
            None
        };
        t.stacks.entry(ds).or_default().insert(stack);
        t.subjects.entry(ds).or_default().insert(subject);
        if let Some(s) = session {
            t.sessions.entry(ds).or_default().insert(s);
        }
        if t1w {
            let e = t.t1w.entry(ds).or_default();
            e.0 += 1;
            e.1.insert(subject);
        }
    }
    t
}

fn all_subjects(l: &mut Lab) -> BTreeSet<i64> {
    let sql = format!(
        "SELECT id FROM {} WHERE merged_into IS NULL",
        l.registry.store().qualified("subject")
    );
    l.registry
        .store()
        .query(&sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

#[test]
fn the_t1w_stacks_of_one_dataset_among_two_are_counted_and_grouped_by_dataset() {
    let mut hashes: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for mut l in labs() {
        let p = plant(&mut l);
        let t = truth(&mut l, &p);
        assert!(
            t.t1w[&Some(BIG)].0 > 0 && t.t1w[&Some(SMALL)].0 > 0,
            "{}",
            l.name
        );
        // the T1w stacks of each dataset, rows and subjects
        for ds in [BIG, SMALL] {
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "name": "T1w stacks of one dataset",
                    "sets": {"t1": {"grain": "stack", "where": [
                        ["=", {}, ["field", {}, "dataset"], ds],
                        ["=", {}, ["axis", {}, "base"], "T1w"]
                    ]}},
                    "out": {"set": "t1", "level": "count"}
                }),
            );
            let (n, subjects) = &t.t1w[&Some(ds)];
            assert_eq!(counted(&a), (*n, subjects.len() as i64), "{}: {ds}", l.name);
            hashes
                .entry(l.name)
                .or_default()
                .push((format!("t1w {ds}"), a.content_hash.clone().unwrap()));
        }
        // the question as a person or the assistant writes it, in YAML:
        // T1 with contrast in one dataset
        let example = [
            "ast_version: 1",
            "name: T1 with contrast in study-big",
            "scheme: default",
            "sets:",
            "  t1c:",
            "    grain: stack",
            "    where:",
            "      - [\"=\", {}, [\"field\", {}, \"dataset\"], \"study-big\"]",
            "      - [\"=\", {}, [\"axis\", {}, \"base\"], \"T1w\"]",
            "      - [\"=\", {}, [\"axis\", {}, \"post_contrast\"], \"given\"]",
            "out: {set: t1c, level: count}",
        ]
        .join("\n");
        let ask = parse(&example).unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let (_, a) = run_ask(&mut l, ask);
        let store = l.registry.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {stack} st JOIN {fp} f ON f.stack_id = st.id \
             WHERE st.first_batch_id = {big} \
             AND EXISTS (SELECT 1 FROM {ax} b WHERE b.stack_id = st.id AND b.axis = 'base' AND b.value = 'T1w') \
             AND EXISTS (SELECT 1 FROM {ax} c WHERE c.stack_id = st.id AND c.axis = 'post_contrast' AND c.value = 'given') \
             AND NOT EXISTS (SELECT 1 FROM {ax} x WHERE x.stack_id = st.id AND x.axis = 'disposition' AND x.value = 'excluded')",
            stack = store.qualified("stack"),
            fp = store.qualified("stack_fingerprint"),
            ax = store.qualified("classification_axis"),
            big = p.big_batch,
        );
        let want = store.query(&sql, &[]).unwrap()[0].int(0).unwrap();
        assert!(want > 0, "{}: nothing planted for the example", l.name);
        assert_eq!(counted(&a).0, want, "{}: the example", l.name);
        // the stacks grouped by dataset, a stack no dataset holds under null
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "name": "stacks by dataset",
                "sets": {
                    "t": {"grain": "stack"},
                    "g": {"grain": "group", "group": {"of": "t", "by": [["field", {}, "dataset"]]}}
                },
                "out": {"set": "g", "level": "aggregate", "columns": [
                    ["field", {}, "dataset"], ["field", {}, "_rows"], ["field", {}, "_subjects"]
                ]}
            }),
        );
        let want: BTreeMap<Option<String>, (i64, i64)> = [None, Some(BIG), Some(SMALL)]
            .into_iter()
            .map(|ds| {
                (
                    ds.map(str::to_string),
                    (t.stacks[&ds].len() as i64, t.subjects[&ds].len() as i64),
                )
            })
            .collect();
        assert_eq!(grouped(&a, "dataset"), want, "{}", l.name);
        hashes
            .entry(l.name)
            .or_default()
            .push(("grouped".into(), a.content_hash.clone().unwrap()));
        // the stack's own dataset beside its id, and in and <> on it
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["in", {}, ["field", {}, "dataset"], [BIG, SMALL]]]}},
                "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"], ["field", {}, "dataset"]],
                        "order": [[["field", {}, "id"], "asc"]]}
            }),
        );
        let mut want: Vec<(i64, String)> = Vec::new();
        for ds in [BIG, SMALL] {
            want.extend(t.stacks[&Some(ds)].iter().map(|s| (*s, ds.to_string())));
        }
        want.sort();
        let got: Vec<(i64, String)> = column(&a, "id")
            .iter()
            .zip(column(&a, "dataset"))
            .map(|(i, d)| (i.as_i64().unwrap(), d.as_str().unwrap().to_string()))
            .collect();
        assert_eq!(got, want, "{}", l.name);
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["<>", {}, ["field", {}, "dataset"], BIG]]}},
                "out": {"set": "t", "level": "count"}
            }),
        );
        assert_eq!(
            counted(&a).0,
            t.stacks[&Some(SMALL)].len() as i64,
            "{}: <> leaves out a stack of no dataset, as any null",
            l.name
        );
    }
    let mut it = hashes.values();
    if let (Some(a), Some(b)) = (it.next(), it.next()) {
        assert_eq!(a, b, "the backends' answers agree");
    }
}

#[test]
fn a_subject_with_stacks_of_two_datasets_is_of_each() {
    let mut hashes: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for mut l in labs() {
        let p = plant(&mut l);
        let t = truth(&mut l, &p);
        let everyone = all_subjects(&mut l);
        assert!(t.subjects[&Some(BIG)].contains(&p.shared), "{}", l.name);
        assert!(t.subjects[&Some(SMALL)].contains(&p.shared), "{}", l.name);
        let people = |clause: Value| {
            json!({
                "ast_version": 1,
                "sets": {"people": {"grain": "subject", "where": [clause]}},
                "out": {"set": "people", "level": "record", "columns": [["field", {}, "id"]],
                        "order": [[["field", {}, "id"], "asc"]]}
            })
        };
        // = asks whether the name is among the subject's datasets, and so
        // do has and in
        for ds in [BIG, SMALL] {
            let want = &t.subjects[&Some(ds)];
            for clause in [
                json!(["=", {}, ["field", {}, "dataset"], ds]),
                json!(["has", {}, ["field", {}, "dataset"], ds]),
                json!(["in", {}, ["field", {}, "dataset"], [ds]]),
            ] {
                let a = answer(&mut l, people(clause.clone()));
                assert_eq!(&ids(&a, "id"), want, "{}: {clause}", l.name);
            }
            // <> and not_in: the subjects of no stack of it, one of no
            // dataset at all among them
            let rest: BTreeSet<i64> = everyone.difference(want).copied().collect();
            for clause in [
                json!(["<>", {}, ["field", {}, "dataset"], ds]),
                json!(["not_in", {}, ["field", {}, "dataset"], [ds]]),
            ] {
                let a = answer(&mut l, people(clause.clone()));
                assert_eq!(ids(&a, "id"), rest, "{}: {clause}", l.name);
            }
        }
        // the subject's datasets as one sorted list, none for a subject of
        // no dataset
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {"people": {"grain": "subject"}},
                "out": {"set": "people", "level": "record", "columns": [["field", {}, "id"], ["field", {}, "dataset"]],
                        "order": [[["field", {}, "id"], "asc"]]}
            }),
        );
        let listed: BTreeMap<i64, Value> = column(&a, "id")
            .iter()
            .map(|v| v.as_i64().unwrap())
            .zip(column(&a, "dataset"))
            .collect();
        assert_eq!(
            listed[&p.shared],
            json!(format!("{BIG}, {SMALL}")),
            "{}",
            l.name
        );
        for (subject, shown) in &listed {
            let want = match (
                t.subjects[&Some(BIG)].contains(subject),
                t.subjects[&Some(SMALL)].contains(subject),
            ) {
                (true, true) => json!(format!("{BIG}, {SMALL}")),
                (true, false) => json!(BIG),
                (false, true) => json!(SMALL),
                (false, false) => Value::Null,
            };
            assert_eq!(shown, &want, "{}: subject {subject}", l.name);
        }
        hashes
            .entry(l.name)
            .or_default()
            .push(("listed".into(), a.content_hash.clone().unwrap()));
        // a group of subjects by dataset counts the shared one under each
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {
                    "people": {"grain": "subject"},
                    "g": {"grain": "group", "group": {"of": "people", "by": [["field", {}, "dataset"]]}}
                },
                "out": {"set": "g", "level": "aggregate", "columns": [
                    ["field", {}, "dataset"], ["field", {}, "_rows"], ["field", {}, "_subjects"]
                ]}
            }),
        );
        let none: BTreeSet<i64> = everyone
            .iter()
            .filter(|s| {
                !t.subjects[&Some(BIG)].contains(s) && !t.subjects[&Some(SMALL)].contains(s)
            })
            .copied()
            .collect();
        let n = |s: &BTreeSet<i64>| (s.len() as i64, s.len() as i64);
        let want: BTreeMap<Option<String>, (i64, i64)> = BTreeMap::from([
            (None, n(&none)),
            (Some(BIG.to_string()), n(&t.subjects[&Some(BIG)])),
            (Some(SMALL.to_string()), n(&t.subjects[&Some(SMALL)])),
        ]);
        assert_eq!(grouped(&a, "dataset"), want, "{}", l.name);
        hashes
            .entry(l.name)
            .or_default()
            .push(("subjects grouped".into(), a.content_hash.clone().unwrap()));
        // the sessions likewise: the shared session is of both
        for ds in [BIG, SMALL] {
            let a = answer(
                &mut l,
                json!({
                    "ast_version": 1,
                    "sets": {"visits": {"grain": "session", "where": [["=", {}, ["field", {}, "dataset"], ds]]}},
                    "out": {"set": "visits", "level": "record", "columns": [["field", {}, "id"]]}
                }),
            );
            let got = ids(&a, "id");
            assert_eq!(&got, &t.sessions[&Some(ds)], "{}: {ds}", l.name);
            assert!(got.contains(&p.shared_session), "{}: {ds}", l.name);
        }
        // a stack set reads its subject's datasets: every stack of the
        // shared subject is of a subject of the second dataset
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["=", {}, ["field", {}, "subject.dataset"], SMALL]]}},
                "out": {"set": "t", "level": "count"}
            }),
        );
        let shared_stacks = seen(l.registry.store())
            .iter()
            .filter(|(_, subject, _, _, _)| t.subjects[&Some(SMALL)].contains(subject))
            .count() as i64;
        assert_eq!(
            counted(&a),
            (shared_stacks, t.subjects[&Some(SMALL)].len() as i64),
            "{}",
            l.name
        );
        // the visits of a dataset through the stacks, as has reads them,
        // agree with the session's own field
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {
                    "scans": {"grain": "stack", "where": [["=", {}, ["field", {}, "dataset"], SMALL]]},
                    "visits": {"grain": "session", "has": [{"set": "scans", "min": 1}]}
                },
                "out": {"set": "visits", "level": "count"}
            }),
        );
        assert_eq!(
            counted(&a).0,
            t.sessions[&Some(SMALL)].len() as i64,
            "{}",
            l.name
        );
    }
    let mut it = hashes.values();
    if let (Some(a), Some(b)) = (it.next(), it.next()) {
        assert_eq!(a, b, "the backends' answers agree");
    }
}

#[test]
fn the_catalog_serves_the_field_and_its_values_and_refuses_a_name_it_does_not_hold() {
    for mut l in labs() {
        let p = plant(&mut l);
        let t = truth(&mut l, &p);
        // the field on three levels, technical, with a line that says what
        // it is; the root is no dataset
        for level in ["stack", "session", "subject"] {
            let f = &l.catalog.fields[&(level.to_string(), "dataset".to_string())];
            assert_eq!(f.class, Class::Technical, "{level}");
            assert_eq!(f.type_, "text", "{level}");
            assert!(!f.dated && !f.federated, "{level}");
            assert!(
                f.description.contains("dataset"),
                "{level}: {}",
                f.description
            );
            let plain = Scope::default();
            assert!(l.catalog.may_project_raw(f, &plain), "{level}");
            assert!(
                l.catalog
                    .fields_of(level, &plain)
                    .iter()
                    .any(|g| g.path == "dataset"),
                "{level}"
            );
        }
        let doc = l.catalog.document(&Scope::default());
        assert_eq!(doc["datasets"], json!([BIG, SMALL]), "{}", l.name);
        // the value sampler, at plain: the names with their counts
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
        let sample = affordance::values(&mut l.registry, "stack", "dataset", &s, 50, None).unwrap();
        assert_eq!(sample.kind, "values", "{}", l.name);
        let items: BTreeMap<String, i64> = sample.items.into_iter().collect();
        assert_eq!(items[BIG], t.stacks[&Some(BIG)].len() as i64, "{}", l.name);
        assert_eq!(
            items[SMALL],
            t.stacks[&Some(SMALL)].len() as i64,
            "{}",
            l.name
        );
        let sample =
            affordance::values(&mut l.registry, "subject", "dataset", &s, 50, None).unwrap();
        let items: BTreeMap<String, i64> = sample.items.into_iter().collect();
        assert_eq!(
            items[BIG],
            t.subjects[&Some(BIG)].len() as i64,
            "{}",
            l.name
        );
        assert_eq!(
            items[SMALL],
            t.subjects[&Some(SMALL)].len() as i64,
            "{}",
            l.name
        );
        // the preview of a count, at plain
        let ask = parse(
            &json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["=", {}, ["field", {}, "dataset"], SMALL]]}},
                "out": {"set": "t", "level": "count"}
            })
            .to_string(),
        )
        .unwrap();
        let shown = affordance::preview(&mut l.registry, &ask, 10, &s, None).unwrap();
        assert_eq!(
            shown.rows[0][0],
            json!(t.stacks[&Some(SMALL)].len()),
            "{}",
            l.name
        );
        let described = affordance::describe(&ask, &s).unwrap();
        assert!(
            described
                .conventions
                .iter()
                .any(|c| c.contains("dataset whose digest first read it")),
            "{}: {:?}",
            l.name,
            described.conventions
        );
        // the subjects of a dataset drafted as the assistant drafts them:
        // stored, and the funnel keeps the dataset's subjects
        let text = [
            "ast_version: 1",
            "name: subjects in study-small",
            "scheme: default",
            "sets:",
            "  people:",
            "    grain: subject",
            "    where:",
            "      - [\"=\", {}, [\"field\", {}, \"dataset\"], \"study-small\"]",
            "out: {set: people, level: count}",
        ]
        .join("\n");
        let drafted = affordance::draft(&mut l.registry, &text, &s, None).unwrap();
        assert!(
            drafted.diagnosis.valid && drafted.document.is_some(),
            "{}: {:?}",
            l.name,
            drafted.diagnosis.issues
        );
        let kept = drafted
            .diagnosis
            .funnel
            .iter()
            .rfind(|st| st.set == "people")
            .unwrap_or_else(|| panic!("{}: {:?}", l.name, drafted.diagnosis.funnel));
        assert_eq!(
            kept.subjects,
            t.subjects[&Some(SMALL)].len() as i64,
            "{}",
            l.name
        );
        // a name the registry does not hold is refused with the ones it does
        let misspelt = parse(
            &json!({
                "ast_version": 1,
                "sets": {"t": {"grain": "stack", "where": [["=", {}, ["field", {}, "dataset"], "study-bgi"]]}},
                "out": {"set": "t", "level": "count"}
            })
            .to_string(),
        )
        .unwrap();
        match prepare(misspelt, &l.catalog, &quasi()) {
            Err(AskError::Invalid(issues)) => {
                let i = issues
                    .iter()
                    .find(|i| i.code == Code::UnknownValue)
                    .unwrap_or_else(|| panic!("{}: {issues:?}", l.name));
                assert!(
                    i.message.contains("study-bgi") && i.message.contains("study-big, study-small"),
                    "{}: {}",
                    i.message,
                    l.name
                );
            }
            other => panic!("{}: a misspelt dataset was taken: {other:?}", l.name),
        }
        // a handle read again by dataset: the stacks of both, kept, then
        // grouped from the handle
        let scheme = Scheme::default();
        let both = parse(
            &json!({
                "ast_version": 1,
                "name": "the stacks of both datasets",
                "sets": {"t": {"grain": "stack", "where": [["in", {}, ["field", {}, "dataset"], [BIG, SMALL]]]}},
                "out": {"set": "t", "level": "record", "columns": [["field", {}, "id"]]}
            })
            .to_string(),
        )
        .unwrap();
        let out = run(
            &mut l.registry,
            Request {
                ask: both,
                names: &l.catalog,
                scope: &quasi(),
                principal: "tester",
                node: "lab",
                pack_version: Some("0.0.0-test"),
                scheme: &scheme,
                bounds: bounds(),
                page_rows: 50,
                name: Some("both datasets"),
                keep: true,
                after: None,
                limit: None,
                may_project_raw: false,
                purpose: None,
                reader: None,
            },
        )
        .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        let h = out.handle.id;
        assert_eq!(
            out.handle.row_count as usize,
            t.stacks[&Some(BIG)].len() + t.stacks[&Some(SMALL)].len(),
            "{}",
            l.name
        );
        refresh(&mut l);
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {
                    "t": {"grain": "stack", "from": format!("handle:{h}")},
                    "g": {"grain": "group", "group": {"of": "t", "by": [["field", {}, "dataset"]]}}
                },
                "out": {"set": "g", "level": "aggregate", "columns": [
                    ["field", {}, "dataset"], ["field", {}, "_rows"], ["field", {}, "_subjects"]
                ]}
            }),
        );
        let want: BTreeMap<Option<String>, (i64, i64)> = [BIG, SMALL]
            .into_iter()
            .map(|ds| {
                (
                    Some(ds.to_string()),
                    (
                        t.stacks[&Some(ds)].len() as i64,
                        t.subjects[&Some(ds)].len() as i64,
                    ),
                )
            })
            .collect();
        assert_eq!(grouped(&a, "dataset"), want, "{}", l.name);
        // a retired place is no dataset: its stacks are of none, and its
        // name is refused, once the catalog reads the places again
        place::retire(l.registry.store(), p.small_place).unwrap();
        l.catalog.refresh_datasets(l.registry.store()).unwrap();
        assert_eq!(
            l.catalog.document(&Scope::default())["datasets"],
            json!([BIG]),
            "{}",
            l.name
        );
        let a = answer(
            &mut l,
            json!({
                "ast_version": 1,
                "sets": {
                    "t": {"grain": "stack"},
                    "g": {"grain": "group", "group": {"of": "t", "by": [["field", {}, "dataset"]]}}
                },
                "out": {"set": "g", "level": "aggregate", "columns": [
                    ["field", {}, "dataset"], ["field", {}, "_rows"]
                ]}
            }),
        );
        let got = grouped_rows(&a);
        assert_eq!(
            got,
            BTreeMap::from([
                (
                    None,
                    (t.stacks[&None].len() + t.stacks[&Some(SMALL)].len()) as i64
                ),
                (Some(BIG.to_string()), t.stacks[&Some(BIG)].len() as i64),
            ]),
            "{}",
            l.name
        );
    }
}

/// A group answer as key to rows, for one without `_subjects`.
fn grouped_rows(a: &Answer) -> BTreeMap<Option<String>, i64> {
    column(a, "dataset")
        .iter()
        .zip(column(a, "_rows"))
        .map(|(k, r)| (k.as_str().map(str::to_string), r.as_i64().unwrap()))
        .collect()
}

#[test]
fn a_root_of_the_registry_two_datasets_hold_is_the_deeper_ones() {
    for mut l in labs() {
        let dir = TempDir::new("ask-datasets-nested");
        let outer = dir.path().join("outer");
        let inner = outer.join("derivatives/dcm-anon");
        std::fs::create_dir_all(&inner).unwrap();
        let store = l.registry.store();
        add_place(store, "outer", &outer, Value::Null);
        // a legacy place names a dataset's pseudonymised tree itself
        add_place(store, "inner", &inner, json!({"kind": "legacy"}));
        let root = inner.canonicalize().unwrap().join("sub-1");
        let source = store
            .insert(
                &Insert::new(
                    table("source"),
                    &["root", "root_canonical", "first_seen_at"],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(root.display().to_string().as_str()),
                    Param::from(root.display().to_string().as_str()),
                    Param::from("2026-10-09T08:00:00Z"),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        let datasets = place::dataset_sources(store).unwrap();
        let of = |name: &str| {
            datasets
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.sources.clone())
                .unwrap_or_default()
        };
        assert_eq!(of("inner"), vec![source], "{}", l.name);
        assert!(!of("outer").contains(&source), "{}", l.name);
    }
}
