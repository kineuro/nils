// SPDX-License-Identifier: AGPL-3.0-only

//! What a scheme says of a session (`label`, `months`, `nominal`,
//! `flagged`, `reason`) and what its studies say of it (`has_primary`) are
//! fields of a session that no column of the session cache holds. The
//! catalog named the cache for each, so a question that read one validated,
//! compiled and failed where it ran (`no such column: sc.label`). Each is
//! read here every way a field can be, on both backends, against what the
//! tables hold; the labels of another scheme never reach an answer; and a
//! draft of such a question diagnoses, previews and samples.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use common::{Lab, ask_of, quasi};
use nils_ask::affordance::{self, Setting};
use nils_ask::compile::{Context, compile};
use nils_ask::exec::Bounds;
use nils_ask::validate::Scope;
use nils_ask::{parse, prepare};
use nils_registry::Param;
use nils_registry::schema::Type;
use nils_registry::session::Scheme;
use nils_registry::store::Cell;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_session_fields_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// One statement's rows, on the registry's own tables.
fn rows(l: &mut Lab, sql: &str, params: &[Param]) -> Vec<nils_registry::store::Row> {
    let store = l.registry.store();
    let mut text = sql.to_string();
    for t in [
        "session_label",
        "session_cache_study",
        "session_cache",
        "study",
    ] {
        text = text.replace(&format!("{{{t}}}"), &store.qualified(t));
    }
    let d = store.dialect();
    for (i, p) in params.iter().enumerate() {
        let ty = match p {
            Param::Int(_) => Type::Int,
            _ => Type::Text,
        };
        text = text.replace(&format!("${}", i + 1), &d.param(i + 1, ty));
    }
    store
        .query(&text, params)
        .unwrap_or_else(|e| panic!("{}: {e}\n{text}", l.name))
}

fn one(l: &mut Lab, sql: &str, params: &[Param]) -> i64 {
    rows(l, sql, params)[0].int(0).unwrap()
}

/// The column of an answer, by its name.
fn column(answer: &nils_ask::exec::Answer, name: &str) -> Vec<Value> {
    let at = answer
        .columns
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no column {name}: {:?}", answer.columns));
    answer
        .rows
        .iter()
        .map(|r| nils_ask::handle::cell_json(&r.0[at]))
        .collect()
}

fn texts(values: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = values
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    out.sort();
    out
}

fn sum(values: &[Value]) -> i64 {
    values.iter().filter_map(Value::as_i64).sum()
}

fn doc(sets: Value, out: Value) -> String {
    json!({"ast_version": 1, "name": "what a scheme says of a session", "sets": sets, "out": out})
        .to_string()
}

fn count(l: &mut Lab, sets: Value, set: &str) -> i64 {
    let (_, answer) = ask_of(l, &doc(sets, json!({"set": set, "level": "count"})));
    match answer.rows[0].0[0] {
        Cell::Int(n) => n,
        ref other => panic!("{}: a count of {set}: {other:?}", l.name),
    }
}

fn by(of: &str, key: &str) -> Value {
    json!({"grain": "group", "group": {"of": of, "by": [["field", {}, key]]}})
}

fn grouped(key: &str) -> Value {
    json!({"set": "g", "level": "aggregate", "columns": [["field", {}, key], ["field", {}, "_rows"]]})
}

/// What the test plants: one session of the default scheme given months, a
/// cadence point, a flag and its reason, and a row of another scheme for
/// the same session, which no answer under the default scheme may read.
struct Planted {
    session: i64,
    label: String,
    /// Every label of the default scheme, sorted.
    labels: Vec<String>,
    sessions: i64,
}

fn plant(l: &mut Lab) -> Planted {
    let digest = Scheme::default().digest();
    let found = rows(
        l,
        "SELECT l.session_id, l.label FROM {session_label} l WHERE l.scheme_digest = $1 \
         AND l.label IS NOT NULL ORDER BY l.session_id LIMIT 1",
        &[Param::from(digest.as_str())],
    );
    let session = found[0].int(0).unwrap();
    let label = found[0].text(1).unwrap().to_string();
    let store = l.registry.store();
    let d = store.dialect();
    store
        .execute(
            &format!(
                "UPDATE {} SET months = 12, nominal = 12, flagged = 1, reason = 'collision' \
                 WHERE session_id = {} AND scheme_digest = {}",
                store.qualified("session_label"),
                d.param(1, Type::Int),
                d.param(2, Type::Text)
            ),
            &[Param::Int(session), Param::from(digest.as_str())],
        )
        .unwrap();
    // every session, under a scheme the documents here do not read
    store
        .execute(
            &format!(
                "INSERT INTO {} (session_id, scheme_digest, label, months, flagged, reason) \
                 SELECT id, 'another-scheme', 'OTHER', 99, 1, 'no_anchor' FROM {}",
                store.qualified("session_label"),
                store.qualified("session_cache")
            ),
            &[],
        )
        .unwrap();
    let labels = rows(
        l,
        "SELECT l.label FROM {session_label} l WHERE l.scheme_digest = $1 AND l.label IS NOT NULL \
         ORDER BY l.label",
        &[Param::from(digest.as_str())],
    )
    .iter()
    .map(|r| r.text(0).unwrap().to_string())
    .collect();
    let sessions = one(l, "SELECT COUNT(*) FROM {session_cache}", &[]);
    Planted {
        session,
        label,
        labels,
        sessions,
    }
}

fn distinct(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// On a session set itself: as a group's key, in `where`, in the answer's
/// columns and its order, in a pick's `by`, and from a set drawn `from` it.
#[test]
fn a_session_set_reads_what_its_scheme_says_of_it() {
    for mut l in labs() {
        let p = plant(&mut l);
        assert!(p.labels.len() > 100 && p.sessions >= p.labels.len() as i64);
        // a group's key: one row a label, every session under one
        let (compiled, answer) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session"}, "g": by("v", "label")}),
                grouped("label"),
            ),
        );
        assert_eq!(
            texts(&column(&answer, "label")),
            distinct(&p.labels),
            "{}",
            l.name
        );
        assert_eq!(sum(&column(&answer, "_rows")), p.sessions, "{}", l.name);
        // the labels are a scheme's: the join is by its digest, once
        assert_eq!(
            compiled.sql.matches("sl.scheme_digest = ").count(),
            1,
            "{}: {}",
            l.name,
            compiled.sql
        );
        assert!(!texts(&column(&answer, "label")).contains(&"OTHER".to_string()));
        // the answer's columns, every field, in the order of the label
        let (_, answer) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session"}}),
                json!({"set": "v", "level": "record",
                    "columns": [["field", {}, "label"], ["field", {}, "months"],
                        ["field", {}, "nominal"], ["field", {}, "flagged"], ["field", {}, "reason"]],
                    "order": [[["field", {}, "label"], "asc"]]}),
            ),
        );
        assert_eq!(answer.rows.len() as i64, p.sessions, "{}", l.name);
        let labels: Vec<String> = column(&answer, "label")
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert_eq!(labels, p.labels, "{}: in the label's order", l.name);
        let key = column(&answer, "_key");
        let at = key
            .iter()
            .position(|k| k.as_i64() == Some(p.session))
            .unwrap();
        let cell = |name: &str| column(&answer, name)[at].clone();
        assert_eq!(cell("label"), json!(p.label), "{}", l.name);
        assert_eq!(cell("months"), json!(12), "{}", l.name);
        assert_eq!(cell("nominal"), json!(12), "{}", l.name);
        assert_eq!(cell("flagged"), json!(1), "{}", l.name);
        assert_eq!(cell("reason"), json!("collision"), "{}", l.name);
        assert_eq!(sum(&column(&answer, "flagged")), 1, "{}", l.name);
        // where: by the label, by the months and by the flag
        for clause in [
            json!(["=", {}, ["field", {}, "label"], p.label]),
            json!(["=", {}, ["field", {}, "months"], 12]),
            json!(["=", {}, ["field", {}, "nominal"], 12]),
            json!(["=", {}, ["field", {}, "flagged"], 1]),
            json!(["=", {}, ["field", {}, "reason"], "collision"]),
        ] {
            let expected = if clause[2][2] == "label" {
                p.labels.iter().filter(|x| **x == p.label).count() as i64
            } else {
                1
            };
            assert_eq!(
                count(
                    &mut l,
                    json!({"v": {"grain": "session", "where": [clause.clone()]}}),
                    "v"
                ),
                expected,
                "{}: {clause}",
                l.name
            );
        }
        // a pick's by: one session a subject, the one of its first label
        let subjects = one(
            &mut l,
            "SELECT COUNT(DISTINCT subject_id) FROM {session_cache}",
            &[],
        );
        let (_, answer) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session",
                    "pick": {"per": "subject", "by": [[["field", {}, "label"], "asc"]]}}}),
                json!({"set": "v", "level": "record", "columns": [["field", {}, "label"]]}),
            ),
        );
        assert_eq!(answer.rows.len() as i64, subjects, "{}", l.name);
        let firsts: Vec<String> = rows(
            &mut l,
            "SELECT MIN(l.label) FROM {session_cache} c JOIN {session_label} l ON l.session_id = c.id \
             AND l.scheme_digest = $1 GROUP BY c.subject_id ORDER BY 1",
            &[Param::from(Scheme::default().digest().as_str())],
        )
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
        assert_eq!(texts(&column(&answer, "label")), firsts, "{}", l.name);
        // a set drawn from another reads it as its source does
        assert_eq!(
            count(
                &mut l,
                json!({"v": {"grain": "session"},
                    "w": {"grain": "session", "from": "v",
                        "where": [["not_null", {}, ["field", {}, "label"]]]}}),
                "w"
            ),
            p.labels.len() as i64,
            "{}",
            l.name
        );
        // a session set drawn of a subject set, which starts from the subject
        assert_eq!(
            count(
                &mut l,
                json!({"people": {"grain": "subject"},
                    "v": {"grain": "session", "of": "people",
                        "where": [["not_null", {}, ["field", {}, "label"]]]}}),
                "v"
            ),
            p.labels.len() as i64,
            "{}",
            l.name
        );
        // and a set that reads none of them is the statement it was
        let (compiled, _) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session"}}),
                json!({"set": "v", "level": "count"}),
            ),
        );
        assert!(!compiled.sql.contains("session_label"), "{}", compiled.sql);
    }
}

/// From a set below the session: as `session.<field>` on a stack and an
/// instance set, through the session set a stack set is drawn `of`, as a
/// group's key and in `where`.
#[test]
fn a_stack_and_an_instance_set_read_their_sessions_label() {
    for mut l in labs() {
        let p = plant(&mut l);
        let stacks = count(&mut l, json!({"s": {"grain": "stack"}}), "s");
        assert!(stacks > 0, "{}", l.name);
        // the carried level, as a group's key: every stack under a label
        let (compiled, carried) = ask_of(
            &mut l,
            &doc(
                json!({"s": {"grain": "stack"}, "g": by("s", "session.label")}),
                grouped("session.label"),
            ),
        );
        assert_eq!(
            sum(&column(&carried, "_rows")),
            stacks,
            "{}: every stack once",
            l.name
        );
        let labels = texts(&column(&carried, "session.label"));
        assert!(
            !labels.is_empty() && labels.iter().all(|x| p.labels.contains(x)),
            "{}",
            l.name
        );
        assert_eq!(compiled.sql.matches("sl.scheme_digest = ").count(), 1);
        // through the session set it is drawn of: the same table
        let (_, through) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session"}, "s": {"grain": "stack", "of": "v"},
                    "g": by("s", "v.label")}),
                grouped("v.label"),
            ),
        );
        let table = |a: &nils_ask::exec::Answer, key: &str| -> BTreeMap<String, i64> {
            column(a, key)
                .iter()
                .zip(column(a, "_rows"))
                .map(|(k, n)| (k.as_str().unwrap_or("").to_string(), n.as_i64().unwrap()))
                .collect()
        };
        let by_label = table(&carried, "session.label");
        // a stack of no session is under no label in the first and absent
        // from the second
        let mut labelled = by_label.clone();
        labelled.remove("");
        let mut of_visits = table(&through, "v.label");
        of_visits.remove("");
        assert_eq!(labelled, of_visits, "{}", l.name);
        // where, on the carried level and through the ancestor
        let under = by_label[&p.label];
        for sets in [
            json!({"s": {"grain": "stack",
                "where": [["=", {}, ["field", {}, "session.label"], p.label]]}}),
            json!({"v": {"grain": "session"}, "s": {"grain": "stack", "of": "v",
                "where": [["=", {}, ["field", {}, "v.label"], p.label]]}}),
        ] {
            assert_eq!(
                count(&mut l, sets.clone(), "s"),
                under,
                "{}: {sets}",
                l.name
            );
        }
        // a stack set drawn of a subject set, which starts from the subject
        assert_eq!(
            count(
                &mut l,
                json!({"people": {"grain": "subject"},
                    "s": {"grain": "stack", "of": "people",
                        "where": [["=", {}, ["field", {}, "session.label"], p.label]]}}),
                "s"
            ),
            under,
            "{}",
            l.name
        );
        // the one session planted with months and a flag: its stacks
        let of_session = count(
            &mut l,
            json!({"s": {"grain": "stack",
                "where": [["=", {}, ["field", {}, "session.id"], p.session]]}}),
            "s",
        );
        assert!(of_session >= 1, "{}", l.name);
        for sets in [
            json!({"s": {"grain": "stack",
                "where": [["=", {}, ["field", {}, "session.months"], 12]]}}),
            json!({"s": {"grain": "stack",
                "where": [["=", {}, ["field", {}, "session.reason"], "collision"]]}}),
            json!({"v": {"grain": "session", "where": [["=", {}, ["field", {}, "flagged"], 1]]},
                "s": {"grain": "stack", "of": "v"}}),
            json!({"v": {"grain": "session"}, "s": {"grain": "stack", "of": "v",
                "where": [["=", {}, ["field", {}, "v.nominal"], 12]]}}),
        ] {
            assert_eq!(
                count(&mut l, sets.clone(), "s"),
                of_session,
                "{}: {sets}",
                l.name
            );
        }
        // an instance set reads it by the same join
        let instances = count(&mut l, json!({"i": {"grain": "instance"}}), "i");
        let (compiled, answer) = ask_of(
            &mut l,
            &doc(
                json!({"i": {"grain": "instance"}, "g": by("i", "session.label")}),
                grouped("session.label"),
            ),
        );
        assert_eq!(sum(&column(&answer, "_rows")), instances, "{}", l.name);
        assert_eq!(compiled.sql.matches("sl.scheme_digest = ").count(), 1);
        assert_eq!(
            count(
                &mut l,
                json!({"i": {"grain": "instance",
                    "where": [["not_null", {}, ["field", {}, "session.label"]]]}}),
                "i"
            ),
            sum(&column(&answer, "_rows"))
                - column(&answer, "session.label")
                    .iter()
                    .zip(column(&answer, "_rows"))
                    .filter(|(k, _)| k.is_null())
                    .map(|(_, n)| n.as_i64().unwrap())
                    .sum::<i64>(),
            "{}",
            l.name
        );
    }
}

/// Through another set: an attached partner, a near partner, and an
/// aggregate over a set of sessions.
#[test]
fn a_partner_and_an_aggregate_read_a_sessions_label() {
    for mut l in labs() {
        let p = plant(&mut l);
        let digest = Scheme::default().digest();
        // attach: each subject's first session, by its label
        let (compiled, answer) = ask_of(
            &mut l,
            &doc(
                json!({
                    "v": {"grain": "session",
                        "pick": {"per": "subject", "by": [[["field", {}, "first"], "asc"]]}},
                    "people": {"grain": "subject", "attach": [{"as": "opening", "set": "v"}]}
                }),
                json!({"set": "people", "level": "record",
                    "columns": [["field", {}, "opening.label"], ["field", {}, "opening.months"]]}),
            ),
        );
        let firsts: Vec<String> = rows(
            &mut l,
            "SELECT l.label FROM {session_cache} c JOIN {session_label} l ON l.session_id = c.id \
             AND l.scheme_digest = $1 WHERE c.first = (SELECT MIN(c2.first) FROM {session_cache} c2 \
             WHERE c2.subject_id = c.subject_id) ORDER BY 1",
            &[Param::from(digest.as_str())],
        )
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
        assert_eq!(
            texts(&column(&answer, "opening.label")),
            firsts,
            "{}",
            l.name
        );
        assert_eq!(compiled.sql.matches("sl.scheme_digest = ").count(), 1);
        // near: the session nearest an event, by its label
        let (compiled, answer) = ask_of(
            &mut l,
            &doc(
                json!({
                    "v": {"grain": "session"},
                    "scores": {"grain": "event",
                        "where": [["=", {}, ["field", {}, "kind"], "EDSS"]],
                        "near": [{"as": "visit", "set": "v",
                            "window": {"from": -36600, "to": 36600, "unit": "day"},
                            "policy": "nearest", "tie": "earlier"}]}
                }),
                json!({"set": "scores", "level": "record",
                    "columns": [["field", {}, "visit.label"]]}),
            ),
        );
        let near = texts(&column(&answer, "visit.label"));
        assert!(
            !near.is_empty() && near.iter().all(|x| p.labels.contains(x)),
            "{}: {} rows",
            l.name,
            answer.rows.len()
        );
        assert_eq!(compiled.sql.matches("sl.scheme_digest = ").count(), 1);
        // an aggregate over a set of sessions, read by a subject set
        let (_, answer) = ask_of(
            &mut l,
            &doc(
                json!({
                    "v": {"grain": "session"},
                    "people": {"grain": "subject", "bind": {
                        "labels": ["distinct", {"set": "v"}, ["field", {}, "label"]],
                        "longest": ["max", {"set": "v"}, ["field", {}, "months"]],
                        "flags": ["sum", {"set": "v"}, ["field", {}, "flagged"]]
                    }}
                }),
                json!({"set": "people", "level": "record", "columns": [
                    ["field", {}, "labels"], ["field", {}, "longest"], ["field", {}, "flags"]]}),
            ),
        );
        let per_subject = one(
            &mut l,
            "SELECT COUNT(*) FROM (SELECT DISTINCT c.subject_id, l.label FROM {session_cache} c \
             JOIN {session_label} l ON l.session_id = c.id AND l.scheme_digest = $1 \
             WHERE l.label IS NOT NULL) q",
            &[Param::from(digest.as_str())],
        );
        assert_eq!(sum(&column(&answer, "labels")), per_subject, "{}", l.name);
        assert_eq!(sum(&column(&answer, "longest")), 12, "{}", l.name);
        assert_eq!(sum(&column(&answer, "flags")), 1, "{}", l.name);
    }
}

/// `has_primary` is what the session's studies say together, folded as the
/// session resolver folds it: a study that holds one makes it true, else a
/// study that has not said leaves it unknown, else it is false.
#[test]
fn a_session_says_whether_a_study_of_it_holds_a_primary() {
    for mut l in labs() {
        // a study that says no and one that has not said, so every answer
        // has a session of it
        let studies: Vec<i64> = rows(&mut l, "SELECT id FROM {study} ORDER BY id LIMIT 2", &[])
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        for (study, says) in studies.iter().zip(["0", "NULL"]) {
            let store = l.registry.store();
            let d = store.dialect();
            store
                .execute(
                    &format!(
                        "UPDATE {} SET has_original_primary = {says} WHERE id = {}",
                        store.qualified("study"),
                        d.param(1, Type::Int)
                    ),
                    &[Param::Int(*study)],
                )
                .unwrap();
        }
        let mut said: HashMap<i64, Vec<Option<bool>>> = HashMap::new();
        for r in rows(
            &mut l,
            "SELECT cs.session_id, y.has_original_primary FROM {session_cache_study} cs \
             JOIN {study} y ON y.id = cs.study_id",
            &[],
        ) {
            said.entry(r.int(0).unwrap())
                .or_default()
                .push(r.opt_int(1).unwrap().map(|v| v != 0));
        }
        let mut expected: BTreeMap<String, i64> = BTreeMap::new();
        for says in said.values() {
            let folded = if says.contains(&Some(true)) {
                "1"
            } else if says.contains(&None) {
                "null"
            } else {
                "0"
            };
            *expected.entry(folded.to_string()).or_insert(0) += 1;
        }
        assert!(expected.len() >= 2, "{}: {expected:?}", l.name);
        let table = |a: &nils_ask::exec::Answer, key: &str| -> BTreeMap<String, i64> {
            column(a, key)
                .iter()
                .zip(column(a, "_rows"))
                .map(|(k, n)| (k.to_string(), n.as_i64().unwrap()))
                .collect()
        };
        let (_, answer) = ask_of(
            &mut l,
            &doc(
                json!({"v": {"grain": "session"}, "g": by("v", "has_primary")}),
                grouped("has_primary"),
            ),
        );
        assert_eq!(table(&answer, "has_primary"), expected, "{}", l.name);
        // in where, and from a stack set through the carried level
        assert_eq!(
            count(
                &mut l,
                json!({"v": {"grain": "session",
                    "where": [["=", {}, ["field", {}, "has_primary"], 1]]}}),
                "v"
            ),
            expected.get("1").copied().unwrap_or(0),
            "{}",
            l.name
        );
        let stacks = count(&mut l, json!({"s": {"grain": "stack"}}), "s");
        let (_, answer) = ask_of(
            &mut l,
            &doc(
                json!({"s": {"grain": "stack"}, "g": by("s", "session.has_primary")}),
                grouped("session.has_primary"),
            ),
        );
        assert_eq!(sum(&column(&answer, "_rows")), stacks, "{}", l.name);
    }
}

/// A draft of a question that reads a label diagnoses with its funnel
/// counted, is stored, previews and is sampled; and the label, which is the
/// session's day under a scheme that names by the day, is answered as its
/// shape below detail quasi, as the day is.
#[test]
fn a_draft_that_reads_a_label_diagnoses_previews_and_is_held_to_the_detail() {
    let text = r#"
ast_version: 1
name: scans of the sessions a scheme labelled, by label
sets:
  visits:
    grain: session
    where: [["not_null", {}, ["field", {}, "label"]]]
  scans:
    grain: stack
    of: visits
    where: [["not_null", {}, ["field", {}, "session.label"]]]
  by_label:
    grain: group
    group: {of: scans, by: [["field", {}, "visits.label"]]}
out:
  set: by_label
  level: aggregate
  columns: [["field", {}, "visits.label"], ["field", {}, "_rows"]]
  order: [[["field", {}, "visits.label"], asc]]
"#;
    for mut l in labs() {
        let p = plant(&mut l);
        let scheme = Scheme::default();
        for (scope, raw) in [(quasi(), true), (Scope::default(), false)] {
            let s = Setting {
                names: &l.catalog,
                scope: &scope,
                scheme: &scheme,
                principal: "author",
                bounds: bounds(),
                values_cap: 50,
            };
            let drafted = affordance::draft(&mut l.registry, text, &s, None)
                .unwrap_or_else(|e| panic!("{}: {e}", l.name));
            let d = &drafted.diagnosis;
            assert!(d.valid, "{}: {:?}", l.name, d.issues);
            assert!(drafted.document.is_some(), "{}", l.name);
            assert!(d.unavailable.is_none(), "{}: {:?}", l.name, d.unavailable);
            // the funnel counts the sets that read the label
            for set in ["visits", "scans"] {
                assert!(
                    d.funnel.iter().any(|st| st.set == set),
                    "{}: no stage of {set}: {:?}",
                    l.name,
                    d.funnel
                );
            }
            if raw {
                let visits = d
                    .funnel
                    .iter()
                    .rev()
                    .find(|st| st.set == "visits")
                    .and_then(|st| st.rows);
                assert_eq!(visits, Some(p.labels.len() as i64), "{}", l.name);
            }
            // the preview answers rows
            let ask = parse(text).unwrap();
            let preview = affordance::preview(&mut l.registry, &ask, 5, &s, None)
                .unwrap_or_else(|e| panic!("{}: {e}", l.name));
            assert_eq!(preview.rows.len(), 5, "{}", l.name);
            let at = preview
                .columns
                .iter()
                .position(|c| c == "visits.label")
                .unwrap();
            let shown: Vec<&str> = preview.rows.iter().filter_map(|r| r[at].as_str()).collect();
            if raw {
                assert!(preview.shaped.is_empty(), "{}", l.name);
                assert!(
                    shown.iter().all(|x| p.labels.iter().any(|y| y == x)),
                    "{}: {shown:?}",
                    l.name
                );
            } else {
                // a label is a day under the default scheme: its shape
                assert_eq!(preview.shaped, ["visits.label"], "{}", l.name);
                assert!(
                    shown.iter().all(|x| !p.labels.iter().any(|y| y == x)),
                    "{}: {shown:?}",
                    l.name
                );
            }
            // the sampler: values at quasi, shapes below
            let sample = affordance::values(&mut l.registry, "session", "label", &s, 50, None)
                .unwrap_or_else(|e| panic!("{}: {e}", l.name));
            assert_eq!(sample.kind, "shapes", "{}", l.name);
            assert!(!sample.items.is_empty(), "{}", l.name);
            let sample = affordance::values(&mut l.registry, "session", "months", &s, 50, None)
                .unwrap_or_else(|e| panic!("{}: {e}", l.name));
            assert_eq!(sample.kind, "values", "{}", l.name);
        }
        // the statement of a diagnosis's count reads the labels by the
        // scheme's digest too
        let prepared = prepare(parse(text).unwrap(), &l.catalog, &quasi()).unwrap();
        let store = l.registry.store();
        let ctx = Context {
            names: &l.catalog,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: scheme.window_days,
            scheme_digest: scheme.digest(),
            after: None,
            limit: None,
            withhold_sealed: false,
        };
        let compiled = compile(&prepared.ask, &prepared.validated, &ctx).unwrap();
        assert_eq!(
            compiled.sql.matches("sl.scheme_digest = ").count(),
            2,
            "{}: one join a set that reads a label: {}",
            l.name,
            compiled.sql
        );
    }
}
