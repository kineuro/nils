// SPDX-License-Identifier: AGPL-3.0-only

//! The cohort edge and set algebra (`docs/specs/wave4b-the-ask.md`, rule
//! 4): an algebra keeps its left operand's, a union the common ones, and
//! the cohort edge exposes the cohort key where a subject set names a
//! cohort set in `of`. A union's rows come from sets that need not be drawn
//! of the same cohort set, or of any, so `cohort.id` is no field of a
//! union. It was passed, compiled to a column the union does not project,
//! and failed where it ran (`no such column: o.cohort_k`). It is refused
//! here by the validator with what to write, on both backends; what it
//! says to write runs; and an intersect and an except answer as they did.

mod common;

use std::collections::BTreeMap;

use common::{Lab, ask_of, quasi};
use nils_ask::affordance::{self, Setting};
use nils_ask::exec::Bounds;
use nils_ask::{Code, Issue, parse, prepare};
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_union_edge_test")
}

fn doc(sets: Value, out: Value) -> String {
    json!({"ast_version": 1, "name": "the cohort of a union", "sets": sets, "out": out}).to_string()
}

/// Members of every cohort, everyone, and one algebra of the two.
fn algebra(op: &str, operands: [&str; 2]) -> Value {
    json!({
        "cohorts": {"grain": "cohort"},
        "members": {"grain": "subject", "of": "cohorts"},
        "everyone": {"grain": "subject"},
        "nobody": {"grain": "subject", "where": [["=", {}, ["field", {}, "id"], -1]]},
        "both": {"grain": "subject", "algebra": {"op": op, "sets": operands}}
    })
}

fn listed(set: &str) -> Value {
    json!({"set": set, "level": "record", "columns": [["field", {}, "cohort.id"]]})
}

fn refused(l: &Lab, text: &str) -> Vec<Issue> {
    match prepare(parse(text).unwrap(), &l.catalog, &quasi()) {
        Err(nils_ask::Error::Invalid(issues)) => issues,
        Err(e) => panic!("{}: refused another way: {e}", l.name),
        Ok(_) => panic!("{}: accepted: {text}", l.name),
    }
}

const WORDS: &str = "is one or keeps the rows of one: a union's rows come from sets that need not be drawn \
     of the same cohort set, or of any. Bind it on every operand \
     (bind: {cohort: [\"field\", {}, \"cohort.id\"]}) and read the binding, which a union keeps where \
     all its operands have it; or use intersect, which keeps its left operand's rows and their cohort";

fn of_a_union(issues: &[Issue], set: &str) -> Vec<String> {
    issues
        .iter()
        .map(|i| {
            assert_eq!(i.code, Code::UnknownField, "{issues:?}");
            assert_eq!(
                i.message,
                format!("cohort.id is not carried by a union, and {set} {WORDS}"),
                "{issues:?}"
            );
            assert!(!i.next.is_empty(), "{issues:?}");
            i.path.clone()
        })
        .collect()
}

/// The pairs of an answer that lists `cohort.id`: each subject with its
/// cohort, and how often.
fn pairs(answer: &nils_ask::exec::Answer, column: &str) -> BTreeMap<(i64, String), usize> {
    let at = answer.columns.iter().position(|c| c == column).unwrap();
    let subject = answer.columns.iter().position(|c| c == "_subject").unwrap();
    let mut out = BTreeMap::new();
    for r in &answer.rows {
        let s = nils_ask::handle::cell_json(&r.0[subject]).as_i64().unwrap();
        let c = nils_ask::handle::cell_json(&r.0[at]).to_string();
        *out.entry((s, c)).or_insert(0) += 1;
    }
    out
}

#[test]
fn a_union_carries_no_cohort_edge_and_says_what_to_write() {
    for mut l in labs() {
        // every reader of it on a union is refused, in the one answer
        let mut sets = algebra("union", ["members", "everyone"]);
        sets["g"] =
            json!({"grain": "group", "group": {"of": "both", "by": [["field", {}, "cohort.id"]]}});
        sets["both"]["where"] = json!([["not_null", {}, ["field", {}, "cohort.id"]]]);
        let issues = refused(
            &l,
            &doc(
                sets,
                json!({"set": "both", "level": "record",
                    "columns": [["field", {}, "cohort.id"]],
                    "order": [[["field", {}, "cohort.id"], "asc"]]}),
            ),
        );
        let mut paths = of_a_union(&issues, "both");
        paths.sort();
        assert_eq!(
            paths,
            [
                "out.columns[0]",
                "out.order[0]",
                "sets.both.where[0]",
                "sets.g.group.by[0]"
            ],
            "{}",
            l.name
        );
        // whichever operand is the left one, and where neither has the edge
        for operands in [["everyone", "members"], ["everyone", "nobody"]] {
            let issues = refused(&l, &doc(algebra("union", operands), listed("both")));
            assert_eq!(
                of_a_union(&issues, "both"),
                ["out.columns[0]"],
                "{}",
                l.name
            );
        }
        // an intersect or an except that keeps a union's rows has none either
        for op in ["intersect", "except"] {
            let mut sets = algebra("union", ["members", "everyone"]);
            sets["kept"] =
                json!({"grain": "subject", "algebra": {"op": op, "sets": ["both", "nobody"]}});
            let issues = refused(&l, &doc(sets, listed("kept")));
            assert_eq!(
                of_a_union(&issues, "kept"),
                ["out.columns[0]"],
                "{}",
                l.name
            );
        }
        // the draft door's path: refused before anything is counted or stored
        let scope = quasi();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: Bounds {
                timeout_ms: 20_000,
                max_rows: 5_000,
                max_bytes: 4 * 1024 * 1024,
            },
            values_cap: 50,
        };
        let text = doc(algebra("union", ["members", "everyone"]), listed("both"));
        let drafted = affordance::draft(&mut l.registry, &text, &s, None)
            .unwrap_or_else(|e| panic!("{}: {e}", l.name));
        assert!(!drafted.diagnosis.valid && drafted.document.is_none());
        assert_eq!(
            of_a_union(&drafted.diagnosis.issues, "both"),
            ["out.columns[0]"],
            "{}",
            l.name
        );
        assert!(drafted.diagnosis.funnel.is_empty(), "{}", l.name);

        // what the members of every cohort are, each under each of its cohorts
        let (_, members) = ask_of(
            &mut l,
            &doc(algebra("union", ["members", "everyone"]), listed("members")),
        );
        let memberships = pairs(&members, "cohort.id");
        assert!(memberships.len() > 2, "{}", l.name);

        // what the refusal says to write, first: the binding on every
        // operand, which the union keeps. Two halves of the members by sex
        // are the members again, each under its cohort.
        let half = |sex: &str| {
            json!({"grain": "subject", "of": "cohorts",
                "where": [["=", {}, ["field", {}, "sex"], sex]],
                "bind": {"cohort": ["field", {}, "cohort.id"]}})
        };
        let (_, halves) = ask_of(
            &mut l,
            &doc(
                json!({
                    "cohorts": {"grain": "cohort"},
                    "women": half("F"),
                    "men": half("M"),
                    "both": {"grain": "subject", "algebra": {"op": "union", "sets": ["women", "men"]}}
                }),
                json!({"set": "both", "level": "record", "columns": [["field", {}, "cohort"]]}),
            ),
        );
        let (_, sexed) = ask_of(
            &mut l,
            &doc(
                json!({
                    "cohorts": {"grain": "cohort"},
                    "members": {"grain": "subject", "of": "cohorts",
                        "where": [["in", {}, ["field", {}, "sex"], ["F", "M"]]]}
                }),
                listed("members"),
            ),
        );
        assert!(!halves.rows.is_empty(), "{}", l.name);
        assert_eq!(
            pairs(&halves, "cohort"),
            pairs(&sexed, "cohort.id"),
            "{}",
            l.name
        );
        // second: an intersect keeps the left operand's rows and their
        // cohort, as it did
        let (_, kept) = ask_of(
            &mut l,
            &doc(
                algebra("intersect", ["members", "everyone"]),
                listed("both"),
            ),
        );
        assert_eq!(pairs(&kept, "cohort.id"), memberships, "{}", l.name);
        // and so does an except
        let (_, kept) = ask_of(
            &mut l,
            &doc(algebra("except", ["members", "nobody"]), listed("both")),
        );
        assert_eq!(pairs(&kept, "cohort.id"), memberships, "{}", l.name);
        let (_, none) = ask_of(
            &mut l,
            &doc(algebra("except", ["members", "everyone"]), listed("both")),
        );
        assert!(none.rows.is_empty(), "{}", l.name);
        // a union that reads no cohort is the union it was: every subject once
        let (_, everyone) = ask_of(
            &mut l,
            &doc(
                algebra("union", ["members", "everyone"]),
                json!({"set": "everyone", "level": "count"}),
            ),
        );
        let (_, both) = ask_of(
            &mut l,
            &doc(
                algebra("union", ["members", "everyone"]),
                json!({"set": "both", "level": "count"}),
            ),
        );
        assert_eq!(both.rows[0].0, everyone.rows[0].0, "{}", l.name);
    }
}
