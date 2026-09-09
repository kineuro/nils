// SPDX-License-Identifier: AGPL-3.0-only

//! The ask (`docs/specs/wave4b-the-ask.md`, §4, §15 slice 3): the four
//! fixtures of the appendices parse, desugar idempotently, validate against
//! a catalog fixture and hash stably; every validate-time code of the
//! taxonomy has a document that produces it with a path and a next call;
//! repair does what it says and no more; both schemas exist.

use std::collections::{BTreeMap, BTreeSet};

use nils_ask::ast::{Grain, canonical_json};
use nils_ask::validate::{Class, DerivedInfo, FieldInfo, KindInfo, Names, pin_selections};
use nils_ask::{Code, Scope, content_hash, desugar, parse, parse_repaired, prepare};
use serde_json::{Value, json};

/// The catalog of the synthetic registry, as validation sees it.
struct Fixture;

fn field(class: Class, dated: bool) -> FieldInfo {
    FieldInfo {
        class,
        dated,
        federated: class != Class::QuasiIdentifying,
    }
}

impl Names for Fixture {
    fn field(&self, level: &str, path: &str) -> Option<FieldInfo> {
        use Class::*;
        let f = |class, dated| Some(field(class, dated));
        match (level, path) {
            ("cohort", "id" | "name" | "owner") => f(Technical, false),
            ("subject", "id" | "sex") => f(Technical, false),
            ("subject", "code") => f(QuasiIdentifying, false),
            ("subject", "birth_date" | "deceased_at") => f(QuasiIdentifying, true),
            ("subject", "patient_name") => f(Identifying, false),
            ("session", "id" | "n_studies") => f(Technical, false),
            ("session", "first" | "last") => f(QuasiIdentifying, true),
            ("session", "label") => f(Technical, false),
            ("study", "id") => f(Technical, false),
            ("study", "study_date") => f(QuasiIdentifying, true),
            ("series", "id" | "series_description") => f(Technical, false),
            (
                "stack",
                "id" | "n_instances" | "echo_time" | "repetition_time" | "inversion_time",
            ) => f(Technical, false),
            ("stack", "station_name") => f(QuasiIdentifying, false),
            ("stack", "patient_comments") => f(Sensitive, false),
            ("instance", "id") => f(Technical, false),
            ("event", "id" | "kind" | "number" | "value" | "unit") => f(Clinical, false),
            ("event", "date" | "event_date") => f(QuasiIdentifying, true),
            ("event", "precision") => f(Technical, false),
            _ => None,
        }
    }

    fn axis_values(&self, axis: &str) -> Option<Vec<String>> {
        let v: &[&str] = match axis {
            "base" => &["T1w", "T2w", "PDw", "DWI", "SWI"],
            "technique" => &["MPRAGE", "MP2RAGE", "TSE", "GRE", "EPI"],
            "modifier" => &["FLAIR", "STIR", "DIR", "FatSat"],
            "construct" => &["MPR", "ADC", "FA"],
            "disposition" => &[
                "acquisition",
                "scanner_derived",
                "reformat",
                "working_scan",
                "scout",
            ],
            "body_part" => &["Brain", "Spine"],
            _ => return None,
        };
        Some(v.iter().map(|s| s.to_string()).collect())
    }

    fn kind(&self, name: &str) -> Option<KindInfo> {
        match name {
            "EDSS" | "SDMT" | "Diagnosis" => Some(KindInfo {
                precision: "day".into(),
                sensitive: false,
            }),
            "SP Transition" => Some(KindInfo {
                precision: "year".into(),
                sensitive: false,
            }),
            "Pregnancy Delivery" => Some(KindInfo {
                precision: "day".into(),
                sensitive: true,
            }),
            _ => None,
        }
    }

    fn level(&self, name: &str) -> bool {
        matches!(name, "exact" | "strict" | "loose")
    }

    fn role(&self, name: &str) -> Option<Grain> {
        matches!(name, "t1w" | "flair").then_some(Grain::Stack)
    }

    fn scheme(&self, name: &str) -> Option<String> {
        matches!(name, "visits" | "quarterly").then(|| format!("digest-of-{name}"))
    }

    fn cohort(&self, name: &str) -> bool {
        matches!(name, "cohort_a" | "cohort_b" | "cohort_c")
    }

    fn selection(&self, name: &str) -> Option<u64> {
        matches!(name, "converters").then_some(7)
    }

    fn handle(&self, id: &str) -> Option<Grain> {
        matches!(id, "h1").then_some(Grain::Subject)
    }

    fn upload(&self, id: &str) -> bool {
        id == "u1"
    }

    fn derived(&self, name: &str) -> Option<DerivedInfo> {
        let (grain, params): (Grain, &[&str]) = match name {
            "acquisition_type" | "field_strength" | "resolution" | "voxel_min" | "voxel_max" => {
                (Grain::Stack, &[])
            }
            "voxel" => (Grain::Stack, &["third"]),
            "signature" => (Grain::Stack, &["level"]),
            "study_day" => (Grain::Stack, &[]),
            "course" => (Grain::Subject, &["disease"]),
            _ => return None,
        };
        Some(DerivedInfo {
            grain,
            params: params.iter().map(|p| p.to_string()).collect(),
        })
    }
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/fixtures/{name}.ask.yml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

const FIXTURES: &[&str] = &["yardstick", "gold-a", "gold-b", "gold-c"];

#[test]
fn the_four_fixtures_parse_desugar_once_validate_and_hash() {
    let scope = Scope::default();
    let mut hashes = BTreeSet::new();
    for name in FIXTURES {
        let ask = parse(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut once = ask.clone();
        desugar(&mut once).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut twice = once.clone();
        desugar(&mut twice).unwrap();
        assert_eq!(
            once.canonical(),
            twice.canonical(),
            "{name}: desugar is not idempotent"
        );
        // the sugar is gone, the hidden sets are there
        assert!(
            once.sets
                .values()
                .all(|s| s.same.is_empty() && s.every.is_empty() && s.pairs.is_none()),
            "{name}"
        );
        let prepared =
            prepare(ask.clone(), &Fixture, &scope).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(prepared.hash.len(), 64, "{name}");
        assert!(
            prepared
                .validated
                .warnings
                .iter()
                .all(|w| w.code == Code::NotReleasable),
            "{name}: {:?}",
            prepared.validated.warnings
        );
        assert!(
            hashes.insert(prepared.hash.clone()),
            "{name}: two fixtures with one hash"
        );
        // the hash is the same whatever the writer's key order and spacing
        let json = serde_json::to_string_pretty(&prepared.ask).unwrap();
        let reparsed = parse(&json).unwrap();
        assert_eq!(content_hash(&reparsed), prepared.hash, "{name}");
    }
}

#[test]
fn the_yardstick_desugars_into_the_named_sets_of_the_spec() {
    let mut ask = parse(&fixture("yardstick")).unwrap();
    desugar(&mut ask).unwrap();
    // the structural window is inlined, the scalar parameters stay refs
    let good = &ask.sets["good"];
    assert!(
        matches!(good.near[0].window, nils_ask::ast::WindowSpec::Literal(ref w) if w.unit == nils_ask::ast::Unit::Month && w.from == Some(-6))
    );
    let answer = &ask.sets["answer"];
    assert!(matches!(
        &answer.has[0].min,
        Some(nils_ask::ast::IntSpec::Param(_))
    ));
    // the level is inlined into the signature
    let t1 = &ask.sets["t1"];
    assert_eq!(t1.bind.get("sig").unwrap().opts["level"], json!("loose"));
    // same became a hidden group and two bindings
    let group = &ask.sets["answer__same_comparable"];
    assert_eq!(group.grain, Grain::Group);
    assert_eq!(group.group.as_ref().unwrap().of, "good");
    assert_eq!(group.group.as_ref().unwrap().by.len(), 3);
    assert!(answer.bind.get("comparable.largest").is_some());
    assert!(answer.bind.get("comparable.groups").is_some());
    assert_eq!(answer.where_.len(), 1);
    // and the two readings of the hash differ by exactly the reading
    let strict = {
        let mut a = ask.clone();
        a.sets.get_mut("good").unwrap().near[0].strict = true;
        a
    };
    assert_ne!(content_hash(&ask), content_hash(&strict));
    // a parameter's value does not change the hash; its declaration does
    let mut other_value = ask.clone();
    other_value.params.get_mut("age_from").unwrap().value = Some(json!(45));
    assert_eq!(content_hash(&ask), content_hash(&other_value));
    let mut other_decl = ask.clone();
    other_decl.params.remove("age_from");
    assert_ne!(content_hash(&ask), content_hash(&other_decl));
}

fn doc(sets: Value, out: Value) -> Value {
    json!({ "ast_version": 1, "sets": sets, "out": out })
}

fn refused(v: Value) -> Vec<nils_ask::Issue> {
    let ask = serde_json::from_value(v).expect("a document");
    match prepare(ask, &Fixture, &Scope::default()) {
        Err(nils_ask::Error::Invalid(issues)) => issues,
        Err(e) => panic!("refused another way: {e}"),
        Ok(p) => panic!("accepted: {}", p.hash),
    }
}

fn accepted(v: Value) -> nils_ask::Prepared {
    let ask = serde_json::from_value(v).expect("a document");
    prepare(ask, &Fixture, &Scope::default()).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn every_validate_time_code_has_a_document_that_produces_it_with_a_path_and_a_next() {
    let subjects = json!({"grain": "subject"});
    let mut seen: BTreeMap<Code, (String, String)> = BTreeMap::new();
    let mut expect = |code: Code, v: Value, path_prefix: &str| {
        let issues = refused(v);
        let hit = issues
            .iter()
            .find(|i| i.code == code)
            .unwrap_or_else(|| panic!("{}: got {issues:?}", code.name()));
        assert!(
            hit.path.starts_with(path_prefix),
            "{}: path {} does not start with {path_prefix}",
            code.name(),
            hit.path
        );
        assert!(!hit.next.is_empty(), "{}: no next", code.name());
        seen.insert(code, (hit.path.clone(), hit.next.clone()));
    };
    expect(
        Code::UnknownSet,
        doc(
            json!({"a": {"grain": "session", "of": "nobody"}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.of",
    );
    expect(
        Code::UnknownField,
        doc(
            json!({"a": {"grain": "subject", "where": [["=", {}, ["field", {}, "shoe_size"], 1]]}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.where[0]",
    );
    expect(
        Code::AmbiguousPath,
        doc(
            json!({"a": {"grain": "subject", "bind": {"code": ["concat", {}, "x"]}}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.bind.code",
    );
    expect(
        Code::GrainMismatch,
        doc(
            json!({"p": subjects.clone(), "a": {"grain": "session", "from": "p"}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.from",
    );
    expect(
        Code::Cycle,
        doc(
            json!({"a": {"grain": "subject", "from": "b"}, "b": {"grain": "subject", "from": "a"}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.",
    );
    expect(
        Code::NotDated,
        doc(
            json!({"p": subjects.clone(), "e": {"grain": "event"}, "a": {"grain": "subject", "from": "p", "near": [{"as": "x", "set": "e", "window": {"from": -1, "to": 1, "unit": "day"}, "policy": "nearest"}]}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.near[0]",
    );
    expect(
        Code::NotFunctional,
        doc(
            json!({"s": {"grain": "session"}, "t": {"grain": "stack", "of": "s"}, "a": {"grain": "session", "from": "s", "attach": [{"as": "t", "set": "t"}]}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.attach[0]",
    );
    expect(
        Code::AmbiguousParent,
        doc(
            json!({"p": subjects.clone(), "q": subjects.clone(), "a": {"grain": "subject", "of": "p", "algebra": {"op": "union", "sets": ["p", "q"]}}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.from",
    );
    expect(
        Code::UnknownValue,
        doc(
            json!({"t": {"grain": "stack", "where": [["=", {}, ["axis", {}, "base"], "T1"]]}}),
            json!({"set": "t", "level": "count"}),
        ),
        "sets.t.where[0]",
    );
    expect(
        Code::UnknownLevel,
        doc(
            json!({"t": {"grain": "stack", "bind": {"sig": ["derived", {"level": "fuzzy"}, "signature"]}}}),
            json!({"set": "t", "level": "count"}),
        ),
        "sets.t.bind.sig",
    );
    expect(
        Code::ForbiddenField,
        doc(
            json!({"a": {"grain": "subject", "where": [["not_null", {}, ["field", {}, "patient_name"]]]}}),
            json!({"set": "a", "level": "count"}),
        ),
        "sets.a.where[0]",
    );
    expect(
        Code::SchemeMismatch,
        json!({"ast_version": 1, "scheme": "study", "sets": {"a": subjects.clone()}, "out": {"set": "a", "level": "count"}}),
        "scheme",
    );
    expect(
        Code::MissingOrder,
        doc(
            json!({"s": {"grain": "session"}, "t": {"grain": "stack", "of": "s", "pick": {"per": "session", "by": []}}}),
            json!({"set": "t", "level": "count"}),
        ),
        "sets.t.pick.by",
    );
    // the federated scope
    {
        let ask = serde_json::from_value(doc(json!({"a": {"grain": "subject", "where": [["not_null", {}, ["field", {}, "birth_date"]]]}}), json!({"set": "a", "level": "count"}))).unwrap();
        let scope = Scope {
            federated: true,
            classes: BTreeSet::new(),
        };
        match prepare(ask, &Fixture, &scope) {
            Err(nils_ask::Error::Invalid(issues)) => {
                let hit = issues
                    .iter()
                    .find(|i| i.code == Code::FederatedScope)
                    .expect("federated_scope");
                assert!(hit.path.starts_with("sets.a.where[0]"));
                seen.insert(Code::FederatedScope, (hit.path.clone(), hit.next.clone()));
            }
            other => panic!("{other:?}"),
        }
    }
    // the two warnings leave the document valid
    {
        let p = accepted(doc(
            json!({"a": {"grain": "subject", "from": "selection:converters@3"}}),
            json!({"set": "a", "level": "count"}),
        ));
        let w = p
            .validated
            .warnings
            .iter()
            .find(|i| i.code == Code::SelectionOutdated)
            .expect("selection_outdated");
        assert_eq!(w.path, "sets.a.from");
        seen.insert(Code::SelectionOutdated, (w.path.clone(), w.next.clone()));
        let p = accepted(doc(
            json!({
                "p": {"grain": "subject", "bind": {"x": ["concat", {}, "a"]}},
                "q": subjects.clone(),
                "u": {"grain": "subject", "algebra": {"op": "union", "sets": ["p", "q"]}}
            }),
            json!({"set": "u", "level": "count"}),
        ));
        let w = p
            .validated
            .warnings
            .iter()
            .find(|i| i.code == Code::BindingDropped)
            .expect("binding_dropped");
        assert_eq!(w.path, "sets.u.algebra.sets");
        seen.insert(Code::BindingDropped, (w.path.clone(), w.next.clone()));
        let p = accepted(
            json!({"ast_version": 1, "sets": {"e": {"grain": "event"}}, "keep": ["e"], "out": {"set": "e", "level": "count"}}),
        );
        let w = p
            .validated
            .warnings
            .iter()
            .find(|i| i.code == Code::NotReleasable)
            .expect("not_releasable");
        assert_eq!(w.path, "keep[0]");
        seen.insert(Code::NotReleasable, (w.path.clone(), w.next.clone()));
    }
    // what validate cannot produce: the run time codes of later slices
    let run_time = [Code::Truncated, Code::StaleOptions];
    for c in [
        Code::UnknownSet,
        Code::UnknownField,
        Code::AmbiguousPath,
        Code::GrainMismatch,
        Code::Cycle,
        Code::NotDated,
        Code::NotFunctional,
        Code::AmbiguousParent,
        Code::UnknownValue,
        Code::UnknownLevel,
        Code::ForbiddenField,
        Code::FederatedScope,
        Code::SchemeMismatch,
        Code::SelectionOutdated,
        Code::BindingDropped,
        Code::MissingOrder,
        Code::NotReleasable,
    ] {
        assert!(seen.contains_key(&c), "{} has no fixture", c.name());
    }
    assert_eq!(seen.len() + run_time.len(), 19);
}

#[test]
fn a_bare_selection_is_pinned_at_validate_and_the_pin_is_in_the_hash() {
    let mut ask: nils_ask::Ask = serde_json::from_value(doc(
        json!({"a": {"grain": "subject", "from": "selection:converters"}}),
        json!({"set": "a", "level": "count"}),
    ))
    .unwrap();
    let before = content_hash(&ask);
    let pinned = pin_selections(&mut ask, &Fixture).unwrap();
    assert_eq!(pinned, vec![("a".to_string(), "converters".to_string(), 7)]);
    assert_eq!(
        ask.sets["a"].from,
        Some(nils_ask::Src::Selection {
            name: "converters".into(),
            version: Some(7)
        })
    );
    assert_ne!(before, content_hash(&ask));
    let p = prepare(ask, &Fixture, &Scope::default()).unwrap();
    assert!(p.validated.warnings.is_empty());
    assert!(p.pinned.is_empty(), "already pinned");
}

#[test]
fn repair_inserts_the_options_map_wraps_a_lone_clause_and_maps_aliases_and_no_more() {
    let text = r#"
ast_version: 1
sets:
  a:
    grain: subject
    where: ["==", ["field", "sex"], "F"]
    bind:
      age: ["age_at", ["field", {}, "birth_date"], ["param", {}, "as_of"]]
  e:
    grain: event
    near: {as: x, set: e, window: {from: -1, to: 1, unit: days}, policy: nearest}
params:
  as_of: {type: date, value: "2026-01-01"}
out: {set: a, level: count, order: [["field", {}, "code"]]}
"#;
    let (ask, repairs) = parse_repaired(text).unwrap();
    let what: Vec<&str> = repairs.iter().map(|r| r.what.as_str()).collect();
    assert!(
        what.iter().any(|w| w.contains("wrapped a lone clause")),
        "{what:?}"
    );
    assert!(
        what.iter().any(|w| w.contains("missing options map")),
        "{what:?}"
    );
    assert!(what.iter().any(|w| w == &"== is written ="), "{what:?}");
    assert!(what.iter().any(|w| w == &"days is written day"), "{what:?}");
    assert!(
        what.iter().any(|w| w.contains("without a direction")),
        "{what:?}"
    );
    assert!(what.iter().any(|w| w.contains("lone relation")), "{what:?}");
    assert_eq!(ask.sets["a"].where_[0].op, "=");
    assert_eq!(ask.sets["a"].where_[0].args.len(), 2);
    // the value "F" was not touched, and a strict parse of the same text is refused
    assert_eq!(ask.sets["a"].where_[0].args[1].as_text(), Some("F"));
    assert!(parse(text).is_err());
    // repairs report where they happened
    assert!(
        repairs.iter().any(|r| r.path == "sets.a.where"),
        "{repairs:?}"
    );
}

#[test]
fn an_older_version_is_upgraded_and_a_newer_one_refused() {
    let mut old: nils_ask::Ask = serde_json::from_value(json!({"ast_version": 0, "sets": {"a": {"grain": "subject"}}, "out": {"set": "a", "level": "count"}})).unwrap();
    desugar(&mut old).unwrap();
    assert_eq!(old.ast_version, 1);
    let mut newer: nils_ask::Ask = serde_json::from_value(json!({"ast_version": 2, "sets": {"a": {"grain": "subject"}}, "out": {"set": "a", "level": "count"}})).unwrap();
    assert!(desugar(&mut newer).is_err());
    // an unknown key is refused at parse
    assert!(parse(r#"{"ast_version": 1, "sets": {"a": {"grain": "subject", "colour": 1}}, "out": {"set": "a", "level": "count"}}"#).is_err());
}

#[test]
fn the_pipeline_sugar_is_a_chain_and_the_every_sugar_is_three_clauses() {
    let p = accepted(json!({
        "ast_version": 1,
        "pipeline": [
            {"grain": "subject", "where": [["not_null", {}, ["field", {}, "birth_date"]]]},
            {"name": "women", "grain": "subject", "where": [["=", {}, ["field", {}, "sex"], "F"]]}
        ],
        "out": {"set": "", "level": "count"}
    }));
    assert_eq!(p.ask.out.set, "women");
    assert_eq!(
        p.ask.sets["women"].from,
        Some(nils_ask::Src::Set("p1".into()))
    );
    let p = accepted(json!({
        "ast_version": 1,
        "sets": {
            "people": {"grain": "subject"},
            "visits": {"grain": "session", "of": "people"},
            "good": {"grain": "session", "from": "visits", "where": [["not_null", {}, ["field", {}, "label"]]]},
            "all_good": {"grain": "subject", "from": "people", "every": [{"of": "visits", "in": "good"}]}
        },
        "out": {"set": "all_good", "level": "count"}
    }));
    let s = &p.ask.sets["all_good"];
    assert!(p.ask.sets.contains_key("all_good__not_good"));
    assert_eq!(s.has.len(), 2);
    assert_eq!(s.has[0].set, "all_good__not_good");
    assert_eq!(s.has[1].set, "visits");
}

#[test]
fn both_schemas_exist_and_the_tightened_one_closes_every_struct() {
    let generated = nils_ask::schema::generated();
    assert!(generated["$defs"]["Clause"].is_object());
    assert!(generated["$defs"]["Set"]["properties"]["grain"].is_object());
    let tight = nils_ask::schema::tightened();
    assert_eq!(tight["additionalProperties"], json!(false));
    assert_eq!(tight["$defs"]["Set"]["additionalProperties"], json!(false));
    assert!(
        tight["$defs"]["Clause"]["prefixItems"][0]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "change")
    );
    assert_eq!(nils_ask::schema::digest().len(), 32);
    // every reference resolves inside the document: a definition named and
    // never registered is what a grammar backend refuses the whole schema
    // for (Wave 4c C0 found `Arg` missing)
    for (which, schema) in [("generated", &generated), ("tightened", &tight)] {
        let mut refs = BTreeSet::new();
        collect_refs(schema, &mut refs);
        assert!(
            refs.contains("#/$defs/Arg"),
            "{which}: the clause's arguments are a reference"
        );
        for r in refs {
            let name = r
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("{which}: {r} is not a local reference"));
            assert!(
                schema["$defs"][name].is_object(),
                "{which}: {r} names no definition"
            );
        }
    }
    // every fixture only uses ops the tightened schema lists
    for name in FIXTURES {
        let mut ask = parse(&fixture(name)).unwrap();
        desugar(&mut ask).unwrap();
        let text = canonical_json(&serde_json::to_value(&ask).unwrap());
        let v: Value = serde_json::from_str(&text).unwrap();
        let mut ops = BTreeSet::new();
        collect_ops(&v, &mut ops);
        for op in ops {
            assert!(
                nils_ask::schema::OPS.contains(&op.as_str()),
                "{name}: {op} is not in the op table"
            );
        }
    }
}

fn collect_ops(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Array(items) => {
            if items.len() >= 2 && items[0].is_string() && items[1].is_object() {
                out.insert(items[0].as_str().unwrap().to_string());
            }
            for i in items {
                collect_ops(i, out);
            }
        }
        Value::Object(m) => {
            for (_, c) in m {
                collect_ops(c, out);
            }
        }
        _ => {}
    }
}

#[test]
fn the_yaml_rendering_round_trips() {
    for name in FIXTURES {
        let ask = parse(&fixture(name)).unwrap();
        let yaml = nils_ask::to_yaml(&ask).unwrap();
        let back = parse(&yaml).unwrap_or_else(|e| panic!("{name}: {e}\n{yaml}"));
        assert_eq!(ask, back, "{name}");
    }
}

fn collect_refs(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(r) = m.get("$ref").and_then(Value::as_str) {
                out.insert(r.to_string());
            }
            for x in m.values() {
                collect_refs(x, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                collect_refs(x, out);
            }
        }
        _ => {}
    }
}
