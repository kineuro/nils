// SPDX-License-Identifier: AGPL-3.0-only

//! The funnel counted in one pass per set answers what a statement per
//! stage answers. A diagnosis counts every stage of every set; the stages
//! from a set's last relation through its `where` clauses are now read in
//! one statement (`Counter::pass`), and `diagnose_by_stage` still counts
//! each by a statement of its own. Here every document of the gate and the
//! four fixtures of the ask are diagnosed both ways on the synthetic
//! registry, on each backend, as a caller who reads quasi identifying
//! fields raw and as one at detail plain, where counts of few subjects are
//! held back: the two diagnoses are the same document, stage for stage,
//! with their held counts, drops, ties and coarse dates, and a document
//! one refuses the other refuses in the same words.
//!
//! Sets of the same rows (no source, relation or binding of their own, one
//! grain) are counted by one pass together; the second test holds
//! documents of two and three such sets to the same comparison, with the
//! answer's set among them, where the pass counts the unknown fields of
//! the drops as well.

mod common;

use common::{Lab, quasi, root};
use nils_ask::diagnose::{diagnose, diagnose_by_stage, sets_passed};
use nils_ask::exec::Bounds;
use nils_ask::parse;
use nils_ask::validate::Scope;
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_funnel_pass_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 60_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// Every document the repository keeps: the gate's and the ask's own.
fn documents() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["gate/fixtures", "engine/crates/nils-ask/fixtures"] {
        let mut files: Vec<_> = std::fs::read_dir(root().join(dir))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                let name = p.to_string_lossy();
                name.ends_with(".ask.yml") || name.ends_with(".ask.json")
            })
            .collect();
        files.sort();
        for f in files {
            out.push((
                f.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&f).unwrap(),
            ));
        }
    }
    out
}

/// The diagnosis of a document both ways, as JSON; a refusal as its words.
fn both_ways(l: &mut Lab, ask: &nils_ask::ast::Ask, scope: &Scope) -> [Value; 2] {
    let scheme = Scheme::default();
    [true, false].map(|in_one_pass| {
        let d = if in_one_pass {
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
        } else {
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
        };
        match d {
            Ok(d) => serde_json::to_value(&d).unwrap(),
            Err(e) => json!({"error": e.to_string()}),
        }
    })
}

/// The sets of a funnel that have a `where` stage.
fn sets_with_where(diagnosis: &Value) -> std::collections::BTreeSet<String> {
    diagnosis["funnel"]
        .as_array()
        .map(|f| {
            f.iter()
                .filter(|s| s["stage"].as_str().is_some_and(|l| l.starts_with("where ")))
                .filter_map(|s| s["set"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_funnel_in_one_pass_is_the_funnel_a_stage_at_a_time() {
    let documents = documents();
    assert!(documents.len() >= 30, "{}", documents.len());
    for mut l in labs() {
        let name = l.name;
        let mut counted = 0;
        let mut with_where = 0;
        let mut together = 0;
        let passed = sets_passed();
        for (file, text) in &documents {
            // a document the language does not read is no document to count
            let Ok(ask) = parse(text) else { continue };
            for (detail, scope) in [("quasi", quasi()), ("plain", Scope::default())] {
                let both = both_ways(&mut l, &ask, &scope);
                assert_eq!(both[0], both[1], "{name}: {file} at detail {detail}");
                counted += 1;
                let sets = sets_with_where(&both[0]);
                with_where += usize::from(!sets.is_empty());
                together += usize::from(sets.len() > 1);
            }
        }
        // the comparison is of funnels that have `where` stages to pass over
        assert!(counted >= 60, "{name}: {counted}");
        assert!(with_where >= 20, "{name}: {with_where}");
        assert!(together >= 4, "{name}: {together}");
        // and a pass counted them: sets by the score, some of them together
        let now = sets_passed();
        assert!(now.0 - passed.0 >= 20, "{name}: {passed:?} {now:?}");
        assert!(now.1 - passed.1 >= 8, "{name}: {passed:?} {now:?}");
    }
}

/// Documents whose sets one pass counts together.
const TOGETHER: [(&str, &str); 4] = [
    (
        "two sets of subjects, and their union",
        r#"
ast_version: 1
name: women or men
sets:
  women: {grain: subject, where: [["=", {}, ["field", {}, "sex"], "F"]]}
  men:   {grain: subject, where: [["=", {}, ["field", {}, "sex"], "M"]]}
  both:  {grain: subject, algebra: {op: union, sets: [women, men]}}
out: {set: both, level: count}
"#,
    ),
    (
        "the answer's set among two",
        r#"
ast_version: 1
name: women, beside men
sets:
  women: {grain: subject, where: [["=", {}, ["field", {}, "sex"], "F"]]}
  men:   {grain: subject, where: [["=", {}, ["field", {}, "sex"], "M"]]}
out: {set: women, level: count}
"#,
    ),
    (
        "three sets of stacks of several clauses",
        r#"
ast_version: 1
name: stacks by their axes
sets:
  t1:
    grain: stack
    where:
      - ["=", {}, ["axis", {}, "base"], "T1w"]
      - ["=", {}, ["axis", {}, "post_contrast"], "given"]
  flair:
    grain: stack
    where:
      - ["has", {}, ["axis", {}, "modifier"], "FLAIR"]
      - ["not", {}, ["=", {}, ["derived", {}, "acquisition_type"], "2D"]]
      - ["<=", {}, ["derived", {}, "voxel"], 1.5]
  t2:
    grain: stack
    where:
      - ["=", {}, ["axis", {}, "base"], "T2w"]
  any: {grain: stack, algebra: {op: union, sets: [t1, flair, t2]}}
out: {set: any, level: count}
"#,
    ),
    (
        "the answer's set of stacks among two, its fields unknown on some",
        r#"
ast_version: 1
name: fine stacks, beside FLAIR
sets:
  fine:
    grain: stack
    where:
      - ["=", {}, ["axis", {}, "base"], "T1w"]
      - ["<=", {}, ["derived", {}, "voxel"], 1.5]
      - ["=", {}, ["axis", {}, "post_contrast"], "given"]
  flair:
    grain: stack
    where:
      - ["has", {}, ["axis", {}, "modifier"], "FLAIR"]
out: {set: fine, level: count}
"#,
    ),
];

#[test]
fn sets_of_the_same_rows_counted_together_answer_the_same() {
    for mut l in labs() {
        let name = l.name;
        for (what, text) in TOGETHER {
            let ask = parse(text).unwrap_or_else(|e| panic!("{name}: {what}: {e}"));
            // the sets one pass counts together: every set but a union
            let sets = ask.sets.values().filter(|s| s.algebra.is_none()).count();
            for (detail, scope) in [("quasi", quasi()), ("plain", Scope::default())] {
                let passed = sets_passed();
                let both = both_ways(&mut l, &ask, &scope);
                assert_eq!(both[0], both[1], "{name}: {what} at detail {detail}");
                assert_eq!(
                    sets_passed().1 - passed.1,
                    sets,
                    "{name}: {what} at detail {detail}"
                );
                // the document is one whose stages are counted, of two sets
                // or more
                assert_eq!(both[0]["valid"], json!(true), "{name}: {what}: {}", both[0]);
                assert!(
                    sets_with_where(&both[0]).len() > 1,
                    "{name}: {what} at detail {detail}: {}",
                    both[0]
                );
            }
        }
    }
}
