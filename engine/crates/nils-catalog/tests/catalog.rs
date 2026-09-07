// SPDX-License-Identifier: AGPL-3.0-only

//! The catalog (`docs/specs/wave4b-the-ask.md`, §9, §15 slice 4), built
//! from the synthetic registry and the MR pack: the four fixtures of the
//! ask validate against it, a level's listing pages inside the byte budget,
//! a sensitive kind is absent without the class and refused at validate,
//! birth date is usable by a reader and projected raw only with the class,
//! and curation survives a rebuild.

use std::collections::BTreeSet;
use std::path::Path;

use nils_ask::validate::{Class, Names, Scope};
use nils_ask::{Code, parse, prepare};
use nils_catalog::{Catalog, Curation, PAGE_BYTES, curate};
use nils_dicom::synth::TempDir;
use nils_registry::clinical::{self, Vocabulary};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Registry};

fn registry() -> (Registry, TempDir) {
    let dir = TempDir::new("catalog-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-catalog-test-key").unwrap();
    let mut registry = home
        .init(&InitOptions {
            backend: Backend::Sqlite,
            dsn: None,
            schema: None,
            scheme: nils_registry::Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    nils_synth::build(
        &mut registry,
        &nils_synth::Plan {
            seed: 5,
            subjects: 30,
        },
    )
    .unwrap();
    // the group's whole vocabulary on top of the synthetic one, for the
    // sensitive kind it carries
    let yaml = std::fs::read_to_string(root().join("packs/clinical/vocabulary.yml")).unwrap();
    clinical::load(registry.store(), &Vocabulary::parse(&yaml).unwrap()).unwrap();
    (registry, dir)
}

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn pack() -> nils_pack::Pack {
    nils_pack::load(&root().join("packs/mri"), None).unwrap()
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(root().join(format!("engine/crates/nils-ask/fixtures/{name}.ask.yml")))
        .unwrap()
}

#[test]
fn the_four_fixtures_validate_against_the_real_catalog() {
    let (mut registry, _dir) = registry();
    let catalog = Catalog::build(&mut registry, &pack()).unwrap();
    for name in ["yardstick", "gold-a", "gold-b", "gold-c"] {
        let ask = parse(&fixture(name)).unwrap();
        let prepared =
            prepare(ask, &catalog, &Scope::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            prepared
                .validated
                .warnings
                .iter()
                .all(|w| w.code == Code::NotReleasable),
            "{name}: {:?}",
            prepared.validated.warnings
        );
    }
    // what the catalog says about the pack and the registry
    assert!(catalog.level("loose") && catalog.level("exact") && catalog.level("strict"));
    assert!(!catalog.level("fuzzy"));
    assert!(
        catalog
            .axis_values("technique")
            .unwrap()
            .iter()
            .any(|v| v == "MPRAGE")
    );
    assert_eq!(catalog.kind("SP Transition").unwrap().precision, "year");
    assert!(
        catalog
            .cohorts
            .iter()
            .any(|c| c.name == "ms-cohort-a" && c.members > 0)
    );
    assert!(
        catalog
            .schemes
            .iter()
            .any(|s| s.name == "default" && s.digest.len() == 32)
    );
    assert!(catalog.roles.contains(&"t1w".to_string()) || !catalog.roles.is_empty());
    assert_eq!(catalog.schema_digest, nils_ask::schema::digest());
    let loose = catalog.levels.iter().find(|l| l.name == "loose").unwrap();
    assert!(loose.exact.iter().any(|f| f == "technique"));
    assert!(loose.ignored.iter().any(|f| f == "inversion_time"));
    assert_eq!(loose.rounded["slice_thickness"], 0.5);
}

#[test]
fn a_level_pages_inside_the_byte_budget_with_a_working_next() {
    let (mut registry, _dir) = registry();
    let catalog = Catalog::build(&mut registry, &pack()).unwrap();
    let scope = Scope::default();
    let all: Vec<String> = catalog
        .fields_of("stack", &scope)
        .iter()
        .map(|f| f.path.clone())
        .collect();
    assert!(all.len() > 40, "{}", all.len());
    let mut after: Option<String> = None;
    let mut walked: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        let page = catalog.page("stack", &scope, after.as_deref(), PAGE_BYTES);
        pages += 1;
        let rendered = serde_json::to_string(&page.fields).unwrap();
        assert!(
            rendered.len() <= PAGE_BYTES,
            "page {pages} is {} bytes",
            rendered.len()
        );
        assert!(!page.fields.is_empty());
        assert_eq!(page.total, all.len());
        walked.extend(
            page.fields
                .iter()
                .map(|f| f["path"].as_str().unwrap().to_string()),
        );
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert!(
        pages >= 2,
        "one page held everything; the budget is not being exercised"
    );
    assert_eq!(walked, all, "the pages walk every field once, in order");
    // the whole document renders, and every level is in it
    let doc = catalog.document(&scope);
    assert_eq!(
        doc["levels"].as_array().unwrap().len(),
        nils_ask::validate::levels().len()
    );
    assert!(doc["caps"]["sync_timeout_ms"].as_u64().unwrap() > 0);
}

#[test]
fn a_sensitive_kind_is_absent_without_the_class_and_refused_at_validate() {
    let (mut registry, _dir) = registry();
    let catalog = Catalog::build(&mut registry, &pack()).unwrap();
    let reader = Scope::default();
    let names: Vec<&str> = catalog
        .kinds_for(&reader)
        .iter()
        .map(|k| k.name.as_str())
        .collect();
    assert!(names.contains(&"EDSS"));
    assert!(!names.contains(&"Pregnancy Delivery"), "{names:?}");
    let holder = Scope {
        federated: false,
        classes: BTreeSet::from([Class::Sensitive]),
    };
    assert!(
        catalog
            .kinds_for(&holder)
            .iter()
            .any(|k| k.name == "Pregnancy Delivery")
    );
    let ask = parse(
        r#"{"ast_version": 1, "sets": {"d": {"grain": "event", "where": [["=", {}, ["field", {}, "kind"], "Pregnancy Delivery"]]}}, "out": {"set": "d", "level": "count"}}"#,
    )
    .unwrap();
    match prepare(ask.clone(), &catalog, &reader) {
        Err(nils_ask::Error::Invalid(issues)) => {
            assert!(
                issues.iter().any(|i| i.code == Code::ForbiddenField),
                "{issues:?}"
            );
        }
        other => panic!("accepted: {other:?}"),
    }
    prepare(ask, &catalog, &holder).unwrap();
    // an unknown kind is an unknown value
    let ask = parse(
        r#"{"ast_version": 1, "sets": {"d": {"grain": "event", "where": [["=", {}, ["field", {}, "kind"], "Shoe Size"]]}}, "out": {"set": "d", "level": "count"}}"#,
    )
    .unwrap();
    match prepare(ask, &catalog, &reader) {
        Err(nils_ask::Error::Invalid(issues)) => {
            assert!(issues.iter().any(|i| i.code == Code::UnknownValue))
        }
        other => panic!("accepted: {other:?}"),
    }
}

#[test]
fn birth_date_is_usable_by_a_reader_and_projected_raw_only_with_the_class() {
    let (mut registry, _dir) = registry();
    let catalog = Catalog::build(&mut registry, &pack()).unwrap();
    let reader = Scope::default();
    let f = catalog.fields[&("subject".to_string(), "birth_date".to_string())].clone();
    assert_eq!(f.class, Class::QuasiIdentifying);
    assert!(f.dated);
    assert!(catalog.visible(&f, &reader));
    assert!(!catalog.may_project_raw(&f, &reader));
    let holder = Scope {
        federated: false,
        classes: BTreeSet::from([Class::QuasiIdentifying]),
    };
    assert!(catalog.may_project_raw(&f, &holder));
    let ask = parse(
        r#"{"ast_version": 1, "params": {"as_of": {"type": "date", "value": "2026-01-01"}}, "sets": {"a": {"grain": "subject", "bind": {"age": ["age_at", {}, ["field", {}, "birth_date"], ["param", {}, "as_of"]]}, "where": [[">=", {}, ["field", {}, "age"], 40]]}}, "out": {"set": "a", "level": "count"}}"#,
    )
    .unwrap();
    prepare(ask.clone(), &catalog, &reader).unwrap();
    // and never under a federated run
    let federated = Scope {
        federated: true,
        classes: BTreeSet::new(),
    };
    match prepare(ask, &catalog, &federated) {
        Err(nils_ask::Error::Invalid(issues)) => {
            assert!(issues.iter().any(|i| i.code == Code::FederatedScope))
        }
        other => panic!("accepted: {other:?}"),
    }
    // an identifier has no record at all
    assert!(catalog.field("subject", "patient_name").is_none());
}

#[test]
fn curation_is_keyed_by_path_and_survives_a_rebuild() {
    let (mut registry, _dir) = registry();
    let before = Catalog::build(&mut registry, &pack()).unwrap();
    let te = before.fields[&("stack".to_string(), "echo_time".to_string())].clone();
    assert!(!te.curated);
    curate(
        registry.store(),
        "stack.echo_time",
        &Curation {
            description: Some("TE, the echo time in milliseconds".into()),
            caveats: Some("multi-echo stacks carry the first echo".into()),
            ai_context: Some("prefer inversion_time to tell an MPRAGE apart".into()),
            visibility: None,
            class: None,
        },
        "someone@node",
    )
    .unwrap();
    let after = Catalog::build(&mut registry, &pack()).unwrap();
    let te = after.fields[&("stack".to_string(), "echo_time".to_string())].clone();
    assert!(te.curated);
    assert!(te.description.starts_with("TE,"));
    assert!(te.caveats.is_some() && te.ai_context.is_some());
    // a second curation of the same path replaces, never duplicates
    curate(
        registry.store(),
        "stack.echo_time",
        &Curation {
            description: Some("the echo time".into()),
            ..Curation::default()
        },
        "someone@node",
    )
    .unwrap();
    let again = Catalog::build(&mut registry, &pack()).unwrap();
    assert_eq!(
        again.fields[&("stack".to_string(), "echo_time".to_string())].description,
        "the echo time"
    );
    let rows = {
        let store = registry.store();
        let sql = format!(
            "SELECT COUNT(*) FROM {}",
            store.qualified("catalog_curation")
        );
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    };
    assert_eq!(rows, 1);
}
