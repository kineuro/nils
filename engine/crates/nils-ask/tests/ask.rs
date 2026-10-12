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
    // the documents read quasi identifying fields, as the gate runs them
    let scope = Scope {
        classes: [Class::QuasiIdentifying].into_iter().collect(),
        ..Scope::default()
    };
    let plain = Scope::default();
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
        // record 55 K7: at detail plain each validates too, gold C's standard
        // deviation of age among them since the ruling of 2026-10-12: a
        // group's spread is answered for 5 subjects or more, held back below
        let p = prepare(ask.clone(), &Fixture, &plain).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.hash, prepared.hash, "{name}");
        let held: Vec<&str> = p
            .validated
            .held_measures
            .iter()
            .map(String::as_str)
            .collect();
        if *name == "gold-c" {
            assert_eq!(held, ["stddev.age"], "{name}");
            // its mean age is held to the cohort's subjects, its youngest
            // and oldest are shapes
            assert_eq!(p.validated.held.keys().copied().collect::<Vec<_>>(), [4]);
            assert_eq!(
                p.validated.shaped.iter().copied().collect::<Vec<_>>(),
                [5, 6]
            );
        } else {
            assert!(held.is_empty(), "{name}: {held:?}");
        }
        assert!(prepared.validated.held.is_empty() && prepared.validated.held_measures.is_empty());
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
    // what it selects does (record 43): the bound hash, the run cache's key
    // and a saved selection's, keeps the values; with none bound it is the
    // content hash
    use nils_ask::hash::bound_hash;
    assert_ne!(bound_hash(&ask), bound_hash(&other_value));
    assert_ne!(bound_hash(&ask), content_hash(&ask));
    let mut unbound = ask.clone();
    for d in unbound.params.values_mut() {
        d.value = None;
    }
    assert_eq!(bound_hash(&unbound), content_hash(&unbound));
    // the registry's locale is part of the core (Wave 5 section 12.6): the
    // default hashes as before, another timezone or week start is another
    // question
    use nils_ask::hash::{Locale, content_hash_under};
    assert_eq!(
        content_hash(&ask),
        content_hash_under(&ask, &Locale::default())
    );
    let stockholm = Locale {
        timezone: "Europe/Stockholm".into(),
        week_start: "monday".into(),
    };
    let sunday = Locale {
        timezone: "UTC".into(),
        week_start: "sunday".into(),
    };
    assert_ne!(content_hash(&ask), content_hash_under(&ask, &stockholm));
    assert_ne!(content_hash(&ask), content_hash_under(&ask, &sunday));
    assert_ne!(
        content_hash_under(&ask, &stockholm),
        content_hash_under(&ask, &sunday)
    );
    assert!(stockholm.check().is_ok());
    assert!(
        Locale {
            timezone: "Stockholm".into(),
            week_start: "monday".into()
        }
        .check()
        .is_err()
    );
    assert!(
        Locale {
            timezone: "UTC".into(),
            week_start: "tuesday".into()
        }
        .check()
        .is_err()
    );
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
            unsealed: false,
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

/// A group's `by` is a list of clauses and a pick's `by` a list of order
/// terms: repair gives the second a direction and leaves the first alone,
/// so a grouped document a person wrote as the grammar says reaches the
/// parser unchanged (Wave 4c, seen from the assistant's draft tool).
#[test]
fn repair_leaves_a_groups_by_clauses_alone_and_directs_a_picks_by() {
    let text = r#"
ast_version: 1
sets:
  people: {grain: subject}
  each:
    grain: group
    group: {of: people, by: [["field", {}, "code"], ["field", {}, "sex"]]}
    bind:
      n: ["count", {set: people}]
  visits:
    grain: session
    of: people
    pick: {per: subject, n: 1, ties: report, by: [["field", {}, "first"]]}
out: {set: each, level: aggregate, columns: [["field", {}, "code"], ["field", {}, "n"]]}
"#;
    let (ask, repairs) = parse_repaired(text).unwrap();
    let what: Vec<&str> = repairs.iter().map(|r| r.what.as_str()).collect();
    assert!(
        !repairs
            .iter()
            .any(|r| r.path.starts_with("sets.each.group")),
        "a group's by is not an order list: {repairs:?}"
    );
    assert!(
        repairs
            .iter()
            .any(|r| r.path == "sets.visits.pick.by[0]" && r.what.contains("without a direction")),
        "{what:?}"
    );
    let group = ask.sets["each"].group.as_ref().expect("the group");
    assert_eq!(group.by.len(), 2, "{group:?}");
    let pick = ask.sets["visits"].pick.as_ref().expect("the pick");
    assert_eq!(pick.by.len(), 1, "{pick:?}");
    // a relation's by is a list of clauses too: the yardstick's `same` reaches the parser as written
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/yardstick.ask.yml"
    ))
    .unwrap();
    let (_, repairs) = parse_repaired(&text).unwrap();
    assert!(
        !repairs.iter().any(|r| r.path.contains(".same[")),
        "a same relation's by is not an order list: {repairs:?}"
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

/// kineuro/nils#98: YAML is read under the 1.2 core booleans, so a binding
/// named `n` (or `y`, `yes`, `no`, `on`, `off`) is a name in an option, and
/// a name written `true` or `false` is refused in words that say it was
/// read as a boolean, at the clause's path.
#[test]
fn yaml_reads_a_bare_n_as_a_name_and_says_when_a_name_was_read_as_a_boolean() {
    let text = |name: &str| {
        format!(
            r#"
ast_version: 1
sets:
  people: {{grain: subject}}
  by_sex:
    grain: group
    group: {{of: people, by: [["field", {{}}, "sex"]]}}
    bind:
      {name}:     ["count", {{set: people}}]
      share: ["share", {{of: {name}, over: people}}]
out: {{set: by_sex, level: aggregate, columns: [["field", {{}}, "sex"], ["field", {{}}, "share"]]}}
"#
        )
    };
    for name in ["n", "y", "yes", "no", "on", "off", "N", "Yes"] {
        let v = nils_ask::read(&text(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            v["sets"]["by_sex"]["bind"]["share"][1]["of"],
            json!(name),
            "{name}"
        );
        assert!(v["sets"]["by_sex"]["bind"][name].is_array(), "{name}: {v}");
        accepted(v);
    }
    // true and false stay booleans, and the refusal says so
    let v = nils_ask::read(&text("true").replace("true:", "t:")).unwrap();
    assert_eq!(v["sets"]["by_sex"]["bind"]["share"][1]["of"], json!(true));
    let issues = refused(v);
    let hit = issues
        .iter()
        .find(|i| i.path == "sets.by_sex.bind.share")
        .unwrap_or_else(|| panic!("{issues:?}"));
    assert!(
        hit.message.contains("read as the boolean true"),
        "{}",
        hit.message
    );
    assert!(hit.next.contains("quotes"), "{}", hit.next);
    // the same on a measure of out
    let issues = refused(json!({
        "ast_version": 1,
        "sets": {"people": {"grain": "subject"}},
        "out": {"set": "people", "level": "record", "columns": [["field", {}, "sex"]],
                "measures": [{"share": {"of": false, "over": "people"}}]}
    }));
    assert!(
        issues
            .iter()
            .any(|i| i.path == "out.measures[0]" && i.message.contains("read as the boolean false")),
        "{issues:?}"
    );
}

/// Under the 1.2 booleans a `no`, `yes`, `on` or `off` is text, so a
/// boolean option written that way must be refused at its path rather than
/// read as its default (`adjacent: no` would otherwise mean adjacent).
#[test]
fn a_boolean_option_written_as_yes_or_no_is_refused_at_its_path() {
    let yardstick = fixture("yardstick");
    let refused_text = |text: &str| -> Vec<nils_ask::Issue> {
        refused(nils_ask::read(text).unwrap_or_else(|e| panic!("{e}")))
    };
    let says_true_or_false = |issues: &[nils_ask::Issue], path: &str, key: &str| {
        let hit = issues
            .iter()
            .find(|i| i.path == path && i.message.contains(key))
            .unwrap_or_else(|| panic!("{path} {key}: {issues:?}"));
        assert!(
            hit.next.contains("true") && hit.next.contains("false"),
            "{}",
            hit.next
        );
    };
    // change's adjacent
    for word in ["no", "yes", "off", "\"false\"", "0"] {
        let text = yardstick.replace("adjacent: true", &format!("adjacent: {word}"));
        assert_ne!(text, yardstick);
        says_true_or_false(
            &refused_text(&text),
            "sets.converted.bind.transition",
            "adjacent",
        );
    }
    // a comparison's strict
    let text = yardstick.replace(
        r#"[">",  {}, ["field", {}, "first"], ["field", {}, "converted.transition.to_date"]]"#,
        r#"[">",  {strict: yes}, ["field", {}, "first"], ["field", {}, "converted.transition.to_date"]]"#,
    );
    assert_ne!(text, yardstick);
    let issues = refused_text(&text);
    assert!(
        issues
            .iter()
            .any(|i| i.path.starts_with("sets.followups.where") && i.message.contains("strict")),
        "{issues:?}"
    );
    let hit = issues
        .iter()
        .find(|i| i.message.contains("strict"))
        .unwrap();
    assert!(
        hit.next.contains("true") && hit.next.contains("false"),
        "{}",
        hit.next
    );
    // round's key
    let issues = refused(json!({
        "ast_version": 1,
        "sets": {"t": {"grain": "stack",
                        "bind": {"te": ["round", {"key": "yes"}, ["field", {}, "echo_time"], 1]}}},
        "out": {"set": "t", "level": "record", "columns": [["field", {}, "te"]]}
    }));
    says_true_or_false(&issues, "sets.t.bind.te", "key");
    // true and false are still accepted
    accepted(nils_ask::read(&yardstick.replace("adjacent: true", "adjacent: false")).unwrap());
    // a pinned handle's pin
    let e = nils_ask::parse(&yardstick.replacen(
        "grain: subject",
        "grain: subject\n    from: {handle: h1, pin: no}",
        1,
    ))
    .expect_err("pin: no is not a boolean");
    assert!(e.to_string().contains("pin"), "{e}");
}

/// kineuro/nils#101: a list of literals in an argument is a list, not a
/// clause without its options map. Repair leaves an inline list whose first
/// element is not an op of the language as it was written, and still
/// repairs a ref written without its options inside the same clause.
#[test]
fn repair_leaves_an_inline_list_of_literals_a_list() {
    let text = r#"
ast_version: 1
sets:
  scope: {grain: cohort, where: [["in", {}, ["field", {}, "name"], ["ms-cohort-a", "ms-cohort-b"]]]}
  people: {grain: subject, of: scope, where: [["in", ["field", "sex"], ["F", "M"]]]}
out: {set: people, level: count}
"#;
    let (ask, repairs) = parse_repaired(text).unwrap();
    assert!(
        !repairs.iter().any(|r| r.path.starts_with("sets.scope")),
        "an inline list is not a clause: {repairs:?}"
    );
    let clause = &ask.sets["scope"].where_[0];
    assert_eq!(clause.op, "in");
    match &clause.args[1] {
        nils_ask::ast::Arg::List(items) => {
            let texts: Vec<&str> = items.iter().filter_map(|a| a.as_text()).collect();
            assert_eq!(texts, vec!["ms-cohort-a", "ms-cohort-b"]);
        }
        other => panic!("not a list: {other:?}"),
    }
    // the ref inside the second set is still repaired, its list left alone
    let people = &ask.sets["people"].where_[0];
    match (&people.args[0], &people.args[1]) {
        (nils_ask::ast::Arg::Clause(f), nils_ask::ast::Arg::List(items)) => {
            assert_eq!(f.ref_name(), Some("sex"));
            assert_eq!(items.len(), 2);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        repairs
            .iter()
            .any(|r| r.path == "sets.people.where[0][2]" && r.what.contains("options map of field")),
        "{repairs:?}"
    );
    accepted(serde_json::to_value(&ask).unwrap());
}

/// Record 55 K7 (spec §7.1, T13): below detail quasi, a column that reads a
/// quasi identifying field is answered as its shape, whether it projects the
/// field, a binding or a group's key made from it, or a value computed from
/// it; a filter, an order, a comparison and a count read it freely; the
/// series description is technical and never shaped; at detail quasi and
/// above nothing is.
#[test]
fn a_quasi_identifying_column_is_shaped_below_detail_quasi() {
    let plain = Scope::default();
    let quasi = Scope {
        classes: [Class::QuasiIdentifying].into_iter().collect(),
        ..Scope::default()
    };
    let sensitive = Scope {
        classes: [Class::QuasiIdentifying, Class::Sensitive]
            .into_iter()
            .collect(),
        ..Scope::default()
    };
    let doc = json!({
        "ast_version": 1,
        "name": "k7",
        "sets": {
            "people": {
                "grain": "subject",
                "bind": {
                    "born": ["field", {}, "birth_date"],
                    "born_year": ["part", {"unit": "year"}, ["field", {}, "born"]],
                    "n": ["count", {"set": "visits"}],
                    "latest": ["max", {"set": "visits"}, ["field", {}, "first"]]
                },
                "where": [["not_null", {}, ["field", {}, "deceased_at"]]]
            },
            "visits": {"grain": "session"}
        },
        "out": {
            "set": "people",
            "level": "record",
            "columns": [
                ["field", {}, "id"],
                ["field", {}, "code"],
                ["field", {}, "born_year"],
                ["field", {}, "n"],
                ["field", {}, "latest"],
                ["=", {}, ["field", {}, "sex"], "F"],
                ["field", {}, "sex"]
            ],
            "order": [[["field", {}, "birth_date"], "asc"]]
        }
    })
    .to_string();
    let shaped = |scope: &Scope| -> Vec<usize> {
        prepare(parse(&doc).unwrap(), &Fixture, scope)
            .unwrap_or_else(|e| panic!("{e}"))
            .validated
            .shaped
            .into_iter()
            .collect()
    };
    // the code, the year of birth, the latest session's day; not the id, the
    // count, the comparison or the technical sex
    assert_eq!(shaped(&plain), vec![1, 2, 4]);
    assert!(shaped(&quasi).is_empty());
    assert!(shaped(&sensitive).is_empty());

    // a group's key made from a quasi field is shaped, its count is not, and
    // the series description is technical at every detail
    let grouped = json!({
        "ast_version": 1,
        "name": "k7 groups",
        "sets": {
            "s": {"grain": "stack"},
            "g": {"grain": "group", "group": {"of": "s", "by": [["field", {}, "station_name"]]}},
            "d": {"grain": "group", "group": {"of": "s", "by": [["field", {}, "series.series_description"]]}}
        },
        "out": {
            "set": "g",
            "level": "record",
            "columns": [["field", {}, "station_name"], ["field", {}, "_rows"]]
        }
    });
    let prepared = prepare(parse(&grouped.to_string()).unwrap(), &Fixture, &plain).unwrap();
    assert_eq!(
        prepared.validated.shaped.into_iter().collect::<Vec<_>>(),
        vec![0]
    );
    let mut by_description = grouped.clone();
    by_description["out"] = json!({
        "set": "d",
        "level": "record",
        "columns": [["field", {}, "series.series_description"], ["field", {}, "_rows"]]
    });
    let prepared = prepare(
        parse(&by_description.to_string()).unwrap(),
        &Fixture,
        &plain,
    )
    .unwrap();
    assert!(prepared.validated.shaped.is_empty());

    // a measure over a shaped column would read the raw values: refused below
    // quasi, answered at quasi
    let mut measured: Value = serde_json::from_str(&doc).unwrap();
    measured["out"]["measures"] = json!([{"median": {"of": "latest"}}]);
    let text = measured.to_string();
    let refused = prepare(parse(&text).unwrap(), &Fixture, &plain).unwrap_err();
    assert!(refused.to_string().contains("forbidden_field"), "{refused}");
    prepare(parse(&text).unwrap(), &Fixture, &quasi).unwrap();

    // the shape itself, as the values sampler shows it
    assert_eq!(nils_ask::validate::shape("Scanner-01 b"), "Aaaaaaa-99 a");
    assert_eq!(
        nils_ask::validate::shape(&"x".repeat(41)),
        format!("{}~", "a".repeat(40))
    );
}

/// Record 55 K7, review of 2026-10-10: the shapes check traces a value
/// through every level of clauses, so a quasi identifying field wrapped in
/// thirteen coalesces, which it once gave up on, is shaped, and one wrapped
/// deeper than the bound is refused at validate, never answered unshaped.
#[test]
fn a_quasi_column_nested_past_the_bound_is_refused_not_answered_unshaped() {
    let wrapped = |levels: usize| {
        let mut c = json!(["field", {}, "code"]);
        for _ in 0..levels {
            c = json!(["coalesce", {}, c, "none"]);
        }
        json!({
            "ast_version": 1,
            "name": "deep",
            "sets": {"people": {"grain": "subject"}},
            "out": {"set": "people", "level": "record", "columns": [c]}
        })
        .to_string()
    };
    let plain = Scope::default();
    for levels in [10, 13, 40] {
        let within = prepare(parse(&wrapped(levels)).unwrap(), &Fixture, &plain)
            .unwrap_or_else(|e| panic!("{levels}: {e}"));
        assert_eq!(
            within.validated.shaped.into_iter().collect::<Vec<_>>(),
            vec![0],
            "{levels} coalesces"
        );
    }
    let refused = prepare(parse(&wrapped(70)).unwrap(), &Fixture, &plain).unwrap_err();
    let text = refused.to_string();
    assert!(
        text.contains("not_compilable") && text.contains("nest"),
        "{text}"
    );
}

/// The chat sandbox bench of 2026-10-12, as a document: each subject's age
/// at the first visit, and a spread of it in one group, its aggregates as
/// given. Sessions carry the age; the first is picked by its ordinal.
fn first_ages(bind: Value, by: Value, columns: Value) -> Value {
    json!({
        "ast_version": 1,
        "name": "spread of age at the first visit",
        "params": {"all": {"type": "text", "value": "all"}},
        "sets": {
            "people": {"grain": "subject"},
            "visits": {
                "grain": "session",
                "of": "people",
                "bind": {
                    "n": ["ordinal", {}],
                    "age": ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]
                }
            },
            "firsts": {"grain": "session", "from": "visits", "where": [["=", {}, ["field", {}, "n"], 1]]},
            "spread": {"grain": "group", "group": {"of": "firsts", "by": by}, "bind": bind}
        },
        "out": {"set": "spread", "level": "aggregate", "columns": columns}
    })
}

fn issues_at(v: &Value, scope: &Scope) -> Vec<nils_ask::Issue> {
    let ask = serde_json::from_value(v.clone()).expect("a document");
    match prepare(ask, &Fixture, scope) {
        Err(nils_ask::Error::Invalid(issues)) => issues,
        Err(e) => panic!("refused another way: {e}"),
        Ok(p) => panic!("accepted: {}", p.hash),
    }
}

fn quasi_scope() -> Scope {
    Scope {
        classes: [Class::QuasiIdentifying].into_iter().collect(),
        ..Scope::default()
    }
}

/// The chat sandbox bench of 2026-10-12: an aggregate whose column was
/// written as an option (`column:`, `field:`) or left out read nothing, and
/// `min`, `max` and `avg` totalled the rows' own keys, so an answer named the
/// youngest and the oldest age and held row numbers. An aggregate takes one
/// option, `set`, and reads one column, its argument: anything else is
/// refused at every detail with the form to write; `count` and `distinct`
/// still read the rows without a column.
#[test]
fn an_aggregate_reads_its_column_as_its_argument_never_an_option() {
    let by = json!([["param", {}, "all"]]);
    let cols = |names: &[&str]| -> Value {
        names
            .iter()
            .map(|n| json!(["field", {}, n]))
            .collect::<Vec<_>>()
            .into()
    };
    for scope in [Scope::default(), quasi_scope()] {
        // as the bench wrote it: the column as an option
        let bench = first_ages(
            json!({
                "n_subjects": ["count", {"set": "firsts"}],
                "youngest": ["min", {"set": "firsts", "column": ["field", {}, "age"]}],
                "oldest": ["max", {"set": "firsts", "field": "age"}]
            }),
            by.clone(),
            cols(&["n_subjects", "youngest", "oldest"]),
        );
        let issues = issues_at(&bench, &scope);
        let at = |path: &str| {
            issues
                .iter()
                .find(|i| i.path == path)
                .unwrap_or_else(|| panic!("nothing at {path}: {issues:?}"))
        };
        let youngest = at("sets.spread.bind.youngest");
        assert_eq!(youngest.code, Code::UnknownField);
        assert!(youngest.message.contains("column is not one"), "{youngest}");
        assert_eq!(
            youngest.next,
            r#"write ["min", {set: firsts}, ["field",{},"age"]]"#
        );
        let oldest = at("sets.spread.bind.oldest");
        assert_eq!(
            oldest.next,
            r#"write ["max", {set: firsts}, ["field", {}, "age"]]"#
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.path == "sets.spread.bind.n_subjects"),
            "{issues:?}"
        );

        // no column at all, a tuple, a literal: refused; count and distinct
        // without a column: answered
        for (bind, path) in [
            (
                json!({"m": ["max", {"set": "firsts"}]}),
                "sets.spread.bind.m",
            ),
            (
                json!({"m": ["avg", {"set": "firsts"}, ["field", {}, "age"], ["field", {}, "n"]]}),
                "sets.spread.bind.m",
            ),
            (
                json!({"m": ["sum", {"set": "firsts"}, "age"]}),
                "sets.spread.bind.m",
            ),
            (
                json!({"m": ["count", {"set": "firsts", "distinct": true}]}),
                "sets.spread.bind.m",
            ),
        ] {
            let v = first_ages(bind.clone(), by.clone(), cols(&["m"]));
            let issues = issues_at(&v, &scope);
            assert!(
                issues
                    .iter()
                    .any(|i| i.path == path && i.code == Code::UnknownField),
                "{bind}: {issues:?}"
            );
        }
        // a count of the rows is told to drop the option, not to add a column
        let flagged = first_ages(
            json!({"m": ["count", {"set": "firsts", "distinct": true}]}),
            by.clone(),
            cols(&["m"]),
        );
        let issues = issues_at(&flagged, &scope);
        assert_eq!(
            issues[0].next, r#"write ["count", {set: firsts}]"#,
            "{issues:?}"
        );
        let counted = first_ages(
            json!({"rows": ["count", {"set": "firsts"}], "members": ["distinct", {"set": "firsts"}]}),
            by.clone(),
            cols(&["rows", "members"]),
        );
        prepare(serde_json::from_value(counted).unwrap(), &Fixture, &scope)
            .unwrap_or_else(|e| panic!("{e}"));

        // a second aggregate in one clause is checked as the first is
        let both = first_ages(
            json!({"range": ["-", {},
                ["max", {"set": "firsts"}, ["field", {}, "age"]],
                ["min", {"set": "firsts", "column": ["field", {}, "age"]}]]}),
            by.clone(),
            cols(&["range"]),
        );
        let issues = issues_at(&both, &scope);
        assert!(
            issues.iter().any(|i| i.path == "sets.spread.bind.range"
                && i.message.contains("min takes one option")),
            "{issues:?}"
        );
    }
}

/// A clause beside an aggregate is checked in its own set's namespace:
/// validation once stopped at the first aggregate of a clause, so a field
/// after it went unchecked, a sensitive one projected raw at detail quasi.
#[test]
fn a_clause_beside_an_aggregate_is_checked_as_any_other() {
    let stacks = |column: Value| {
        doc(
            json!({"s": {"grain": "stack"}, "t": {"grain": "stack"}}),
            json!({"set": "s", "level": "record", "columns": [column]}),
        )
    };
    let after = |field: &str| json!(["concat", {}, ["count", {"set": "t"}], ["field", {}, field]]);
    // an unknown field
    let issues = issues_at(&stacks(after("no_such_field")), &quasi_scope());
    assert!(
        issues
            .iter()
            .any(|i| i.code == Code::UnknownField && i.path == "out.columns[0]"),
        "{issues:?}"
    );
    // a sensitive field, without the class, at quasi and at plain
    for scope in [quasi_scope(), Scope::default()] {
        let issues = issues_at(&stacks(after("patient_comments")), &scope);
        assert!(
            issues
                .iter()
                .any(|i| i.code == Code::ForbiddenField && i.path == "out.columns[0]"),
            "{issues:?}"
        );
    }
    // with the class, answered
    let sensitive = Scope {
        classes: [Class::QuasiIdentifying, Class::Sensitive]
            .into_iter()
            .collect(),
        ..Scope::default()
    };
    prepare(
        serde_json::from_value(stacks(after("patient_comments"))).unwrap(),
        &Fixture,
        &sensitive,
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Record 55 K7 on aggregates (the chat sandbox bench, 2026-10-12): below
/// detail quasi an aggregate that reads back one person's value of a quasi
/// identifying field is answered as its shape, as the field is: its `min`,
/// `max`, `sum` and `list`, and a group's key made from it. The ruling of
/// 2026-10-12: a group's mean and its count of distinct values are held to
/// the group's subjects (answered for 5 or more with a value, held back
/// below, which only a run can tell), and so is what a binding makes of
/// them alone; a value made of one with a shaped value is a shape. A count
/// of rows and a comparison read no value and are answered. Of the post
/// pass's measures of a group's child field, the spread (`stddev`) is held
/// to the group's subjects too, and the median and the percentiles, one
/// person's value each, are refused with the reason. At detail quasi every
/// one is answered raw.
#[test]
fn an_aggregate_of_a_quasi_field_follows_the_field_below_detail_quasi() {
    let validated = |v: &Value, scope: &Scope| -> nils_ask::Validated {
        prepare(serde_json::from_value(v.clone()).unwrap(), &Fixture, scope)
            .unwrap_or_else(|e| panic!("{e}"))
            .validated
    };
    let shaped = |v: &Value, scope: &Scope| -> Vec<usize> {
        validated(v, scope).shaped.into_iter().collect()
    };
    let held = |v: &Value, scope: &Scope| -> Vec<(usize, Vec<String>)> {
        validated(v, scope)
            .held
            .into_iter()
            .map(|(i, by)| (i, by.into_iter().collect()))
            .collect()
    };
    let by = |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
    let columns = json!([
        ["field", {}, "n_subjects"],
        ["field", {}, "youngest"],
        ["field", {}, "oldest"],
        ["field", {}, "mean_age"],
        ["field", {}, "total"],
        ["field", {}, "ages"],
        ["field", {}, "n_ages"],
        ["field", {}, "range"],
        ["field", {}, "older"],
        ["field", {}, "_rows"],
        ["field", {}, "rounded"],
        ["field", {}, "both"],
        ["field", {}, "off"],
        ["round", {}, ["field", {}, "mean_age"], 1],
        ["share", {"of": "mean_age", "over": "firsts"}]
    ]);
    let bind = json!({
        "n_subjects": ["count", {"set": "firsts"}],
        "youngest": ["min", {"set": "firsts"}, ["field", {}, "age"]],
        "oldest": ["max", {"set": "firsts"}, ["field", {}, "age"]],
        "mean_age": ["avg", {"set": "firsts"}, ["field", {}, "age"]],
        "total": ["sum", {"set": "firsts"}, ["field", {}, "age"]],
        "ages": ["list", {"set": "firsts"}, ["field", {}, "age"]],
        "n_ages": ["distinct", {"set": "firsts"}, ["field", {}, "age"]],
        "range": ["-", {}, ["field", {}, "oldest"], ["field", {}, "youngest"]],
        "older": [">", {}, ["field", {}, "oldest"], 60],
        "rounded": ["round", {}, ["field", {}, "mean_age"], 0],
        "both": ["+", {}, ["field", {}, "mean_age"], ["field", {}, "n_ages"]],
        "off": ["-", {}, ["field", {}, "mean_age"], ["field", {}, "youngest"]]
    });
    // the bench's question: one group of every first visit
    let spread = first_ages(bind.clone(), json!([["param", {}, "all"]]), columns.clone());
    // youngest, oldest, total, the list, the range, and a mean less a
    // youngest: shapes
    assert_eq!(shaped(&spread, &Scope::default()), vec![1, 2, 4, 5, 7, 12]);
    // the mean, the count of distinct ages, what is made of them alone and
    // a share of the mean: held to the group's subjects, each by the
    // bindings that hold it
    assert_eq!(
        held(&spread, &Scope::default()),
        vec![
            (3, by(&["mean_age"])),
            (6, by(&["n_ages"])),
            (10, by(&["mean_age"])),
            (11, by(&["mean_age", "n_ages"])),
            (13, by(&["mean_age"])),
            (14, by(&["mean_age"])),
        ]
    );
    // the counts of rows and subjects and a comparison: answered as they are
    assert!(shaped(&spread, &quasi_scope()).is_empty());
    assert!(held(&spread, &quasi_scope()).is_empty());
    // a group of one, each subject's own: the same, since only a run can
    // tell a group's size
    let mut each = first_ages(
        bind.clone(),
        json!([["field", {}, "subject.id"]]),
        columns.clone(),
    );
    assert_eq!(shaped(&each, &Scope::default()), vec![1, 2, 4, 5, 7, 12]);
    assert_eq!(held(&each, &Scope::default()).len(), 6);
    // a group keyed by the age: its key shaped, its count answered
    let by_age = first_ages(
        json!({"n": ["count", {"set": "firsts"}]}),
        json!([["field", {}, "age"]]),
        json!([
            ["field", {}, "age"],
            ["field", {}, "n"],
            ["field", {}, "_subjects"]
        ]),
    );
    assert_eq!(shaped(&by_age, &Scope::default()), vec![0]);
    assert!(held(&by_age, &Scope::default()).is_empty());
    assert!(shaped(&by_age, &quasi_scope()).is_empty());

    // a scalar measure of the child's age: its spread held to the group's
    // subjects, its median and percentiles refused below quasi; all of them
    // answered at quasi, and of a column that reads no quasi field, or one
    // a group holds, answered at plain
    for (measure, refused, held) in [
        (json!({"median": {"of": "age"}}), true, None),
        (json!({"percentile": {"of": "age", "p": 90}}), true, None),
        (json!({"stddev": {"of": "age"}}), false, Some("stddev.age")),
        (
            json!({"stddev": {"of": "subject.birth_date"}}),
            false,
            Some("stddev.subject.birth_date"),
        ),
        (json!({"median": {"of": "subject.birth_date"}}), true, None),
        (json!({"median": {"of": "n"}}), false, None),
        (json!({"stddev": {"of": "n"}}), false, None),
        (json!({"median": {"of": "n_subjects"}}), false, None),
        (json!({"median": {"of": "mean_age"}}), false, None),
        (json!({"stddev": {"of": "n_ages"}}), false, None),
    ] {
        each["out"]["measures"] = json!([measure.clone()]);
        let ask = || serde_json::from_value(each.clone()).unwrap();
        match prepare(ask(), &Fixture, &Scope::default()) {
            Err(e) if refused => {
                let text = e.to_string();
                assert!(
                    text.contains("forbidden_field at out.measures[0]")
                        && text.contains("record 55 K7")
                        && text.contains("one person's own value"),
                    "{measure}: {text}"
                );
            }
            Ok(p) if !refused => assert_eq!(
                p.validated.held_measures.into_iter().collect::<Vec<_>>(),
                held.map(str::to_string).into_iter().collect::<Vec<_>>(),
                "{measure}"
            ),
            other => panic!("{measure}: {:?}", other.map(|p| p.hash)),
        }
        let quasi =
            prepare(ask(), &Fixture, &quasi_scope()).unwrap_or_else(|e| panic!("{measure}: {e}"));
        assert!(quasi.validated.held_measures.is_empty());
    }
    // a measure of a shaped column is refused; the spread is pointed at the
    // group's child
    each["out"]["measures"] = json!([{"stddev": {"of": "youngest"}}]);
    let issues = issues_at(&each, &Scope::default());
    assert!(
        issues.iter().any(|i| i.path == "out.measures[0]"
            && i.code == Code::ForbiddenField
            && i.next
                .contains("answered for a group of 5 subjects or more")),
        "{issues:?}"
    );
}

/// A mean or a count of distinct values made anywhere but as a group set's
/// own binding reads one subject's rows: a row's aggregate links to its own
/// subject, session or stack. It stays a shape below detail quasi, in a
/// column or bound, and so does a group's mean read through another set
/// (2026-10-12).
#[test]
fn a_mean_outside_its_group_is_one_persons_and_a_shape() {
    let visits = json!({
        "grain": "session",
        "bind": {"age": ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]]}
    });
    let v = doc(
        json!({
            "visits": visits,
            "people": {"grain": "subject", "bind": {
                "bound_mean": ["avg", {"set": "visits"}, ["field", {}, "age"]],
                "visits_n": ["count", {"set": "visits"}]
            }}
        }),
        json!({"set": "people", "level": "record", "columns": [
            ["avg", {"set": "visits"}, ["field", {}, "age"]],
            ["field", {}, "bound_mean"],
            ["distinct", {"set": "visits"}, ["field", {}, "age"]],
            ["field", {}, "visits_n"],
            [">", {}, ["avg", {"set": "visits"}, ["field", {}, "age"]], 60]
        ]}),
    );
    let p = prepare(
        serde_json::from_value(v.clone()).unwrap(),
        &Fixture,
        &Scope::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        p.validated.shaped.into_iter().collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(p.validated.held.is_empty());
    let q = prepare(serde_json::from_value(v).unwrap(), &Fixture, &quasi_scope())
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(q.validated.shaped.is_empty() && q.validated.held.is_empty());

    // a group's mean, read by a set the group is keyed by: one subject's
    let through = doc(
        json!({
            "visits": visits,
            "people": {"grain": "subject"},
            "per": {"grain": "group",
                "group": {"of": "visits", "by": [["field", {}, "subject.id"]]},
                "bind": {"mean_age": ["avg", {"set": "visits"}, ["field", {}, "age"]]}}
        }),
        json!({"set": "people", "level": "record", "columns": [
            ["max", {"set": "per"}, ["field", {}, "mean_age"]]
        ]}),
    );
    let p = prepare(
        serde_json::from_value(through).unwrap(),
        &Fixture,
        &Scope::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.validated.shaped.into_iter().collect::<Vec<_>>(), vec![0]);
    assert!(p.validated.held.is_empty());
}

/// A share divides the name its `of` gives by a count the answer also says,
/// so a share of an age over a set of 38 was the age over 38, answered raw
/// below detail quasi beside the age's own shape (2026-10-12). It carries
/// what it divides, and what it divides is held to the caller's classes as
/// a field is.
#[test]
fn a_share_carries_what_it_divides() {
    let records = |columns: Value| {
        let mut v = first_ages(
            json!({"n_subjects": ["count", {"set": "firsts"}]}),
            json!([["param", {}, "all"]]),
            json!([["field", {}, "n_subjects"]]),
        );
        v["out"] = json!({"set": "firsts", "level": "record", "columns": columns});
        v
    };
    let shares = records(json!([
        ["field", {}, "age"],
        ["share", {"of": "age", "over": "firsts"}],
        ["share", {"of": "n", "over": "firsts"}],
        ["share", {"of": "subject.birth_date", "over": "firsts"}]
    ]));
    let shaped = |scope: &Scope| -> Vec<usize> {
        prepare(
            serde_json::from_value(shares.clone()).unwrap(),
            &Fixture,
            scope,
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .validated
        .shaped
        .into_iter()
        .collect()
    };
    // the age and the shares of it and of the birth date; not the share of
    // the visit's ordinal
    assert_eq!(shaped(&Scope::default()), vec![0, 1, 3]);
    assert!(shaped(&quasi_scope()).is_empty());

    // a sensitive field, divided at detail quasi: refused as it is read
    let stacks = doc(
        json!({"s": {"grain": "stack"}, "t": {"grain": "stack"}}),
        json!({"set": "s", "level": "record", "columns": [
            ["share", {"of": "patient_comments", "over": "t"}]
        ]}),
    );
    let issues = issues_at(&stacks, &quasi_scope());
    assert!(
        issues
            .iter()
            .any(|i| i.code == Code::ForbiddenField && i.path == "out.columns[0]"),
        "{issues:?}"
    );
    // a share of nothing, or of no name of the set
    for of in [
        json!({"over": "firsts"}),
        json!({"of": "nothing", "over": "firsts"}),
    ] {
        let mut opts = of.clone();
        opts["over"] = json!("firsts");
        let v = records(json!([["share", opts]]));
        let issues = issues_at(&v, &Scope::default());
        assert!(
            issues
                .iter()
                .any(|i| i.code == Code::UnknownField && i.path == "out.columns[0]"),
            "{of}: {issues:?}"
        );
    }
}

/// Wave 7a: a value of an axis is named by its identity, an identity it had
/// before a rename, or its label; a row may hold any of them for it, a
/// group's key holds the one the pack stores, and a name that is another
/// value's is never read as this one.
#[test]
fn an_axis_value_is_named_by_any_of_its_names_and_held_in_any_form() {
    use nils_ask::validate::{AxisNames, AxisValueNames};
    let v = |id: &str, label: &str, aliases: &[&str]| AxisValueNames {
        id: id.into(),
        label: label.into(),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    };
    let id_of = |a: &AxisNames, t: &str| a.named(t).map(|x| x.id.clone());
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    // base stores the label: T2*w is what a row holds for T2starw
    let base = AxisNames {
        stores_label: true,
        values: vec![v("T2starw", "T2*w", &[]), v("T2w", "T2w", &[])],
    };
    assert_eq!(id_of(&base, "T2*w").as_deref(), Some("T2starw"));
    assert_eq!(id_of(&base, "T2starw").as_deref(), Some("T2starw"));
    assert_eq!(id_of(&base, "T2"), None);
    assert_eq!(base.held(&base.values[0]), vec!["T2*w", "T2starw"]);
    assert_eq!(base.held(&base.values[1]), vec!["T2w"]);
    assert_eq!(base.synonyms(), vec![pair("T2starw", "T2*w")]);
    // a former identity names its value, and a row may hold it
    let technique = AxisNames {
        stores_label: true,
        values: vec![v("ASL", "ASL", &["ASL-EPI"]), v("3D-TSE", "SPACE", &[])],
    };
    assert_eq!(id_of(&technique, "ASL-EPI").as_deref(), Some("ASL"));
    assert_eq!(technique.held(&technique.values[0]), vec!["ASL", "ASL-EPI"]);
    assert_eq!(
        technique.synonyms(),
        vec![pair("ASL-EPI", "ASL"), pair("3D-TSE", "SPACE")]
    );
    // an axis that stores the identity reads a label as it
    let ids = AxisNames {
        stores_label: false,
        values: vec![v("given", "1", &[]), v("not_given", "0", &[])],
    };
    assert_eq!(ids.held(&ids.values[0]), vec!["given", "1"]);
    assert_eq!(
        ids.synonyms(),
        vec![pair("1", "given"), pair("0", "not_given")]
    );
    // a label that is another value's identity names that value, and is
    // never held for this one
    let clash = AxisNames {
        stores_label: false,
        values: vec![v("a", "b", &[]), v("b", "c", &[])],
    };
    assert_eq!(id_of(&clash, "b").as_deref(), Some("b"));
    assert_eq!(clash.held(&clash.values[0]), vec!["a"]);
    assert_eq!(clash.synonyms(), vec![pair("c", "b")]);
    // a catalog that knows identities alone names a value by its identity
    let fixture = Fixture.axis_names("base").unwrap();
    assert!(!fixture.stores_label);
    assert_eq!(id_of(&fixture, "T1w").as_deref(), Some("T1w"));
    assert!(fixture.synonyms().is_empty());
}

/// 2026-10-11: the chat took the main scans for the pick clause and drafted
/// `pick: {model: main}` and a role written as an op eight times. Each
/// refusal now says that the main scans are `picked` in a where, and the
/// operator itself is accepted on a stack.
#[test]
fn the_main_scans_are_picked_in_a_where_never_the_pick_clause() {
    let text = r#"
ast_version: 1
scheme: default
sets:
  t1: {grain: stack, pick: {per: session, model: main, by: [[["field", {}, "id"], asc]]}}
out: {set: t1, level: count}
"#;
    for e in [
        parse(text).expect_err("model is no key of pick"),
        parse_repaired(text).expect_err("repair adds no key"),
    ] {
        let m = e.to_string();
        assert!(m.contains("unknown field `model`"), "{m}");
        assert!(m.contains(r#"[["picked", {role: t1w}]]"#), "{m}");
    }
    // a role written as an op names the operator, with that role
    let issues = refused(json!({
        "ast_version": 1,
        "sets": {"t": {"grain": "stack", "where": [["t1w", {}]]}},
        "out": {"set": "t", "level": "count"}
    }));
    let i = issues
        .iter()
        .find(|i| i.message == "t1w is not an op of the language")
        .unwrap_or_else(|| panic!("{issues:?}"));
    assert!(i.next.contains(r#"["picked", {role: t1w}]"#), "{}", i.next);
    // an op that is no role keeps the catalog's next step
    let issues = refused(json!({
        "ast_version": 1,
        "sets": {"t": {"grain": "stack", "where": [["frobnicate", {}]]}},
        "out": {"set": "t", "level": "count"}
    }));
    assert!(
        issues.iter().any(|i| i.next.contains("function table")),
        "{issues:?}"
    );
    // the operator is the language's
    accepted(json!({
        "ast_version": 1,
        "sets": {"t": {"grain": "stack", "where": [["picked", {"role": "t1w"}]]}},
        "out": {"set": "t", "level": "count"}
    }));
}

/// The chat sandbox bench of 2026-10-12 bound a `bucket` without its unit:
/// the validator passed it, the compiler refused it, and the draft door,
/// whose diagnosis compiles what it counts, answered a bare 500. A clause
/// the compiler cannot read is refused at validate, at its path, with the
/// reason and the form to write: the arguments its op reads, the unit of
/// `bucket`, `part` and `shift`, a list only as the list of `in`, a
/// literal text for `contains`, whole places for `round`, and the role of
/// `picked`.
#[test]
fn a_clause_the_compiler_cannot_read_is_refused_with_its_form() {
    let visits = |set: Value| {
        doc(
            json!({"people": {"grain": "subject"}, "visits": set}),
            json!({"set": "visits", "level": "count"}),
        )
    };
    let bound =
        |clause: Value| visits(json!({"grain": "session", "of": "people", "bind": {"x": clause}}));
    let kept =
        |clause: Value| visits(json!({"grain": "session", "of": "people", "where": [clause]}));
    let one = |v: Value| -> nils_ask::Issue {
        let issues = refused(v);
        assert_eq!(issues.len(), 1, "{issues:?}");
        issues.into_iter().next().unwrap()
    };
    // as the bench wrote it
    let i = one(bound(json!(["bucket", {}, ["field", {}, "first"]])));
    assert_eq!(i.code, Code::NotCompilable);
    assert_eq!(i.path, "sets.visits.bind.x");
    assert_eq!(i.message, "bucket needs its unit: year, month or day");
    assert_eq!(
        i.next,
        r#"write ["bucket", {unit: month}, ["field",{},"first"]]"#
    );
    for (clause, message, next) in [
        (
            json!(["bucket", {"unit": "week"}, ["field", {}, "first"]]),
            "bucket's unit is year, month or day, and week is not one",
            r#"write ["bucket", {unit: month}, ["field",{},"first"]]"#,
        ),
        // the validator once offered hour, which the compiler never read
        (
            json!(["part", {"unit": "hour"}, ["field", {}, "first"]]),
            "part's unit is year, month, day or dow, and hour is not one",
            r#"write ["part", {unit: year}, ["field",{},"first"]]"#,
        ),
        (
            json!(["part", {}, ["field", {}, "first"]]),
            "part needs its unit: year, month, day or dow",
            r#"write ["part", {unit: year}, ["field",{},"first"]]"#,
        ),
        (
            json!(["shift", {"unit": "week"}, ["field", {}, "first"], 2]),
            "shift's unit is day, month or year, and week is not one",
            r#"write ["shift", {unit: day}, ["field",{},"first"], 2]"#,
        ),
        (
            json!(["bucket", {"unit": "month"}]),
            "bucket reads a date; it has none",
            r#"write ["bucket", {unit: month}, ["field", {}, "<date>"]]"#,
        ),
        (
            json!(["days_between", {}, ["field", {}, "first"]]),
            "days_between reads two dates; it has one",
            r#"write ["days_between", {}, ["field",{},"first"], ["field", {}, "<date>"]]"#,
        ),
        (
            json!(["coalesce", {}]),
            "coalesce reads one value or more; it has none",
            r#"write ["coalesce", {}, ["field", {}, "<field>"]]"#,
        ),
        (
            json!(["round", {}, ["field", {}, "n_studies"], 1.5]),
            "round's places are a literal whole number",
            r#"write ["round", {}, ["field",{},"n_studies"], <places>]"#,
        ),
        (
            json!(["contains", {}, ["field", {}, "label"], 1]),
            "contains reads a literal text after its value",
            r#"write ["contains", {}, ["field",{},"label"], "<text>"]"#,
        ),
    ] {
        let i = one(bound(clause.clone()));
        assert_eq!(i.code, Code::NotCompilable, "{clause}");
        assert_eq!(i.path, "sets.visits.bind.x", "{clause}");
        assert_eq!(i.message, message, "{clause}");
        assert_eq!(i.next, next, "{clause}");
    }
    // in a where, as in a binding
    for (clause, message, next) in [
        (
            json!(["=", {}, ["field", {}, "n_studies"], [1, 2]]),
            "a list is read only as the list of in or not_in, never as an argument of =",
            r#"write ["in", {}, ["field",{},"n_studies"], [1,2]] for one of the list"#,
        ),
        (
            json!(["in", {}, ["field", {}, "n_studies"], 2]),
            "in reads its values as a list, or as a list parameter",
            r#"write ["in", {}, ["field",{},"n_studies"], [<value>, <value>]]"#,
        ),
        (
            json!(["and", {}]),
            "and reads one clause or more; it has none",
            r#"write ["and", {}, <clause>]"#,
        ),
        (
            json!(["=", {}, ["field", {}, "n_studies"]]),
            "= reads two values; it has one",
            r#"write ["=", {}, ["field",{},"n_studies"], <value>]"#,
        ),
    ] {
        let i = one(kept(clause.clone()));
        assert_eq!(i.code, Code::NotCompilable, "{clause}");
        assert_eq!(i.path, "sets.visits.where[0]", "{clause}");
        assert_eq!(i.message, message, "{clause}");
        assert_eq!(i.next, next, "{clause}");
    }
    // picked names its role, as a name
    let stacks = |clause: Value| {
        doc(
            json!({"t": {"grain": "stack", "where": [clause]}}),
            json!({"set": "t", "level": "count"}),
        )
    };
    let i = one(stacks(json!(["picked", {}])));
    assert_eq!(i.message, "picked names the role whose main scans it reads");
    assert_eq!(
        i.next,
        r#"write ["picked", {role: <role>}] in a stack set's where"#
    );
    let i = one(stacks(json!(["picked", {"role": 1}])));
    assert_eq!(
        i.message,
        "picked's role is a role's name, and 1 is not one"
    );
    // the forms named are read
    for clause in [
        json!(["bucket", {"unit": "month"}, ["field", {}, "first"]]),
        json!(["part", {"unit": "dow"}, ["field", {}, "first"]]),
        json!(["shift", {}, ["field", {}, "first"], 30]),
        json!(["round", {}, ["field", {}, "n_studies"], 1]),
        json!(["coalesce", {}, ["field", {}, "label"]]),
        json!(["ordinal", {}]),
    ] {
        accepted(bound(clause));
    }
    accepted(kept(json!(["in", {}, ["field", {}, "n_studies"], [1, 2]])));
    accepted(stacks(json!(["picked", {"role": "t1w"}])));
}

/// 2026-10-12: a `picked` whose role the pack does not pick read no rows at
/// all, so a question about main scans of a role written another way (t2,
/// T1w) answered nothing. It is refused, as a dataset or a cohort `picked`
/// does not know is; a role the pack picks is read, and a role given as a
/// parameter is the parameter's value.
#[test]
fn picked_refuses_a_role_the_pack_does_not_pick() {
    let stacks = |role: Value| {
        doc(
            json!({"t": {"grain": "stack", "where": [["picked", {"role": role}]]}}),
            json!({"set": "t", "level": "count"}),
        )
    };
    for role in ["t2", "T1w", "main"] {
        let issues = refused(stacks(json!(role)));
        assert_eq!(issues.len(), 1, "{role}: {issues:?}");
        let i = &issues[0];
        assert_eq!(i.code, Code::UnknownValue, "{role}");
        assert_eq!(i.path, "sets.t.where[0]", "{role}");
        assert_eq!(i.message, format!("picked reads no role named {role}"));
        assert_eq!(
            i.next,
            r#"write ["picked", {role: <role>}] in a stack set's where"#
        );
    }
    accepted(stacks(json!("t1w")));
    accepted(stacks(json!("flair")));
    let mut by_param = stacks(json!(["param", {}, "role"]));
    by_param["params"] = json!({"role": {"type": "text", "value": "t1w"}});
    accepted(by_param);
}

/// 2026-10-12: a date a document writes as text is a day, `YYYY-MM-DD`, or
/// an instant in UTC, `YYYY-MM-DDTHH:MM:SSZ`, and only a day the calendar
/// has.
#[test]
fn a_written_date_is_a_day_or_an_instant_of_a_day_the_calendar_has() {
    use nils_ask::validate::{WrittenDate, written_date};
    assert_eq!(
        written_date("2020-01-31"),
        Some(WrittenDate::Day("2020-01-31".into()))
    );
    assert_eq!(
        written_date("2024-02-29"),
        Some(WrittenDate::Day("2024-02-29".into()))
    );
    let instant = written_date("2020-01-31T23:59:59Z").unwrap();
    assert_eq!(instant, WrittenDate::Instant("2020-01-31T23:59:59Z".into()));
    assert_eq!(instant.day(), "2020-01-31");
    for bad in [
        "2021-02-29",
        "2100-02-29",
        "2020-02-30",
        "2020-04-31",
        "2020-13-01",
        "2020-00-10",
        "0000-01-01",
        "2020/01/31",
        "2020-1-31",
        "20200131",
        "2020-01-31 10:00:00",
        "2020-01-31T10:00:00",
        "2020-01-31T24:00:00Z",
        "2020-01-31T10:60:00Z",
        "2020-01-31T10:00:00+01:00",
        "2020-01-31T10:00:00.5Z",
        "",
        "soon",
    ] {
        assert_eq!(written_date(bad), None, "{bad}");
    }
}

/// 2026-10-12: Postgres compares no date with text, and `first >=
/// "2020-01-01"` answered 500 there. A text a date field is compared with,
/// or a date function reads, is read as a date, so validate refuses one
/// that writes none, and a number, at the clause, with the forms to write;
/// a date parameter holds a date.
#[test]
fn a_date_compared_with_text_is_written_as_a_date() {
    let sessions = |clause: Value| {
        doc(
            json!({"s": {"grain": "session", "where": [clause]}}),
            json!({"set": "s", "level": "count"}),
        )
    };
    for clause in [
        json!([">=", {}, ["field", {}, "first"], "2020-01-01"]),
        json!(["<", {}, ["field", {}, "first"], "2020-01-01T00:00:00Z"]),
        json!(["<=", {}, "2020-01-01", ["field", {}, "first"]]),
        json!([
            "in",
            {},
            ["field", {}, "first"],
            ["2020-01-01", "2021-06-15T08:30:00Z"]
        ]),
        json!(["=", {}, ["field", {}, "first"], ["field", {}, "last"]]),
        json!([
            "<",
            {},
            ["days_between", {}, ["field", {}, "first"], "2020-01-01"],
            30
        ]),
    ] {
        accepted(sessions(clause));
    }
    let forms = "a date is written YYYY-MM-DD, or YYYY-MM-DDTHH:MM:SSZ in UTC, as 2020-01-31";
    for (clause, written) in [
        (
            json!([">=", {}, ["field", {}, "first"], "2020/01/01"]),
            "2020/01/01",
        ),
        (
            json!([">=", {}, "01.01.2020", ["field", {}, "first"]]),
            "01.01.2020",
        ),
        (json!([">", {}, ["field", {}, "first"], 2020]), "2020"),
        (
            json!([
                "in",
                {},
                ["field", {}, "first"],
                ["2020-01-01", "2020-02-30"]
            ]),
            "2020-02-30",
        ),
        (
            json!([
                "<",
                {},
                ["days_between", {}, ["field", {}, "first"], "last week"],
                30
            ]),
            "last week",
        ),
    ] {
        let issues = refused(sessions(clause.clone()));
        assert_eq!(issues.len(), 1, "{clause}: {issues:?}");
        let i = &issues[0];
        assert_eq!(i.code, Code::UnknownValue, "{clause}");
        assert_eq!(i.path, "sets.s.where[0]", "{clause}");
        assert_eq!(i.message, format!("{written} is not a date"), "{clause}");
        assert_eq!(i.next, forms, "{clause}");
    }
    // a date parameter holds a date
    let mut by_param = sessions(json!([
        ">=",
        {},
        ["field", {}, "first"],
        ["param", {}, "since"]
    ]));
    by_param["params"] = json!({"since": {"type": "date", "value": "2020-01-01"}});
    accepted(by_param.clone());
    by_param["params"]["since"]["value"] = json!("2020-02-30");
    let issues = refused(by_param);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].code, Code::UnknownValue);
    assert_eq!(issues[0].path, "params.since");
    assert_eq!(issues[0].message, "2020-02-30 is not a date");
}
