// SPDX-License-Identifier: AGPL-3.0-only

//! How an acquisition family is filed, served and held.
//!
//! A pack's `families` files are served whole by the catalog, beside the
//! axis that says which of its values a question counts by default. The
//! pack's loader holds a file to what a pack knows by itself; what only
//! the ask can say is held where the catalog is built, and by `nils pack
//! validate`: that every clause parses and its op is the language's, that
//! a field or a derived field it reads is one the catalog lists, that the
//! sets of an example read as the sets of a document, and that `raw`
//! names a text field of a stack.
//!
//! The family is invented, written in the shipped pack's own axes, in a
//! copy of the pack: the pack ships none from here.

mod common;

use std::path::Path;

use nils_ask::validate::{Names, Scope};
use nils_catalog::Catalog;
use nils_dicom::synth::TempDir;
use serde_json::json;

const FAMILY: &str = "\
family: invented-pair
label: 'An invented pair'
terms: ['invented pair', 'pair']
summary: 'A magnitude and a phase of one acquisition, for a test.'
products:
  - id: magnitude
    what: 'The magnitude image.'
    acquired: true
    where:
      - ['=', {}, ['axis', {}, 'base'], 'T2starw']
      - ['=', {}, ['axis', {}, 'disposition'], 'acquisition']
      - ['not', {}, ['has', {}, ['axis', {}, 'construct'], 'Phase']]
      - ['>', {}, ['field', {}, 'echo_time'], 10]
    note: 'Makers differ.'
  - id: phase
    what: 'The phase image.'
    acquired: false
    where:
      - ['has', {}, ['axis', {}, 'construct'], 'Phase']
      - ['=', {}, ['derived', {}, 'acquisition_type'], '3D']
ask:
  - words: 'the magnitude and the phase of each visit'
    sets: |
      magnitude:
        grain: stack
        of: visits
        where:
          - ['=', {}, ['axis', {}, 'base'], 'T2starw']
          - ['not', {}, ['has', {}, ['axis', {}, 'construct'], 'Phase']]
      phase:
        grain: stack
        of: visits
        where:
          - ['has', {}, ['axis', {}, 'construct'], 'Phase']
      visits:
        grain: session
        has:
          - {set: magnitude, min: 1, as: n}
        where:
          - ['>', {}, ['field', {}, 'n'], 0]
          - ['not_null', {}, ['field', {}, 'first']]
    note: 'The same acquisition is read as the same session.'
raw:
  - field: text_series_description
    what: 'The maker words for the pair.'
";

/// A copy of the shipped pack with the family files given.
fn pack_with(files: &[(&str, &str)]) -> TempDir {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    let dir = TempDir::new("pack-families");
    copy(&common::root().join("packs/mri"), dir.path());
    // the families the pack ships are put aside: the test's own are all
    // this copy holds
    let _ = std::fs::remove_dir_all(dir.path().join("families"));
    std::fs::create_dir_all(dir.path().join("families")).unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join("families").join(name), text).unwrap();
    }
    dir
}

#[test]
fn the_catalog_serves_the_families_and_the_default_kinds_of_scan() {
    let other = FAMILY
        .replace("family: invented-pair", "family: a-map")
        .replace("An invented pair", "A map");
    let dir = pack_with(&[("invented-pair.yml", FAMILY), ("a-map.yml", &other)]);
    let pack = nils_pack::load(dir.path(), None).unwrap_or_else(|e| panic!("{e}"));
    nils_catalog::families::held_alone(&pack).unwrap_or_else(|e| panic!("{e}"));
    for mut l in common::labs("nils_families_test") {
        let catalog = Catalog::build(&mut l.registry, &pack).unwrap();
        let doc = catalog.document(&Scope::default());
        // each file whole, in the order of the files' names
        let families = doc["families"].as_array().unwrap();
        assert_eq!(families.len(), 2, "{}", l.name);
        assert_eq!(families[0]["family"], "a-map", "{}", l.name);
        let f = &families[1];
        assert_eq!(
            f.as_object().unwrap().keys().collect::<Vec<_>>(),
            [
                "family", "label", "terms", "summary", "products", "ask", "raw"
            ],
            "{}",
            l.name
        );
        assert_eq!(f["family"], "invented-pair", "{}", l.name);
        assert_eq!(f["label"], "An invented pair", "{}", l.name);
        assert_eq!(f["terms"], json!(["invented pair", "pair"]), "{}", l.name);
        assert_eq!(
            f["products"][0],
            json!({
                "id": "magnitude",
                "what": "The magnitude image.",
                "acquired": true,
                "where": [
                    ["=", {}, ["axis", {}, "base"], "T2starw"],
                    ["=", {}, ["axis", {}, "disposition"], "acquisition"],
                    ["not", {}, ["has", {}, ["axis", {}, "construct"], "Phase"]],
                    [">", {}, ["field", {}, "echo_time"], 10]
                ],
                "note": "Makers differ."
            }),
            "{}",
            l.name
        );
        assert_eq!(f["products"][1]["acquired"], false, "{}", l.name);
        assert!(f["products"][1].get("note").is_none(), "{}", l.name);
        assert_eq!(
            f["ask"][0]["words"], "the magnitude and the phase of each visit",
            "{}",
            l.name
        );
        assert!(
            f["ask"][0]["sets"]
                .as_str()
                .is_some_and(|t| t.starts_with("magnitude:\n  grain: stack\n")),
            "{}: {}",
            l.name,
            f["ask"][0]["sets"]
        );
        assert_eq!(
            f["raw"],
            json!([{"field": "text_series_description", "what": "The maker words for the pair."}]),
            "{}",
            l.name
        );
        // the axis that says which of its values a question counts
        let axis = |name: &str| {
            doc["axes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["name"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(
            axis("disposition")["ask_default"],
            json!(["acquisition", "scanner_derived"]),
            "{}",
            l.name
        );
        // and the clause that keeps them, as a reading adds it
        assert_eq!(
            axis("disposition")["ask_default_clause"],
            json!(["in", {"unset": true}, ["axis", {}, "disposition"], ["acquisition", "scanner_derived"]]),
            "{}",
            l.name
        );
        assert!(axis("technique").get("ask_default").is_none(), "{}", l.name);
        assert!(
            axis("technique").get("ask_default_clause").is_none(),
            "{}",
            l.name
        );
        assert!(axis("disposition").get("default").is_none(), "{}", l.name);
        assert_eq!(
            catalog.axis_kin("disposition").ask_default,
            ["acquisition", "scanner_derived"],
            "{}",
            l.name
        );
        // a pack with no family file: the key is absent
        let bare = pack_with(&[]);
        let bare = nils_pack::load(bare.path(), None).unwrap_or_else(|e| panic!("{e}"));
        let none = Catalog::build(&mut l.registry, &bare)
            .unwrap()
            .document(&Scope::default());
        assert!(none.get("families").is_none(), "{}", l.name);
        assert_eq!(
            none["axes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["name"] == "disposition")
                .unwrap()["ask_default"],
            json!(["acquisition", "scanner_derived"]),
            "{}",
            l.name
        );
    }
}

#[test]
fn a_family_is_held_to_the_language_and_the_catalog_s_fields() {
    let labs = common::labs("nils_families_held_test");
    let mut l = labs.into_iter().next().unwrap();
    let mut refused = |from: &str, to: &str, path: &str, why: &str| {
        let text = FAMILY.replace(from, to);
        assert_ne!(text, FAMILY, "{from}");
        let dir = pack_with(&[("invented-pair.yml", &text)]);
        // the pack's own loader takes it: it knows no field and no op
        let pack = nils_pack::load(dir.path(), None).unwrap_or_else(|e| panic!("{from}: {e}"));
        let e = nils_catalog::families::held_alone(&pack)
            .expect_err(from)
            .to_string();
        assert!(e.contains("invented-pair.yml"), "{from}: {e}");
        assert!(e.contains(path), "{from}: {e}");
        assert!(e.contains(why), "{from}: {e}");
        // and no catalog is built of such a pack
        let built = Catalog::build(&mut l.registry, &pack)
            .expect_err(from)
            .to_string();
        assert!(built.contains(why), "{from}: {built}");
    };
    // a field of a stack the catalog lists
    refused(
        "['field', {}, 'echo_time']",
        "['field', {}, 'echo_colour']",
        "products[0].where[3]",
        "echo_colour is no field of a stack the catalog lists",
    );
    // a derived field the catalog lists
    refused(
        "['derived', {}, 'acquisition_type']",
        "['derived', {}, 'acquisition_mood']",
        "products[1].where[1]",
        "acquisition_mood is no derived field the catalog lists",
    );
    // an op of the language
    refused(
        "['>', {}, ['field', {}, 'echo_time'], 10]",
        "['above', {}, ['field', {}, 'echo_time'], 10]",
        "products[0].where[3]",
        "above is no op of the language",
    );
    // the sets of an example read as the sets of a document
    refused(
        "        grain: session\n",
        "        grain: session\n        colour: red\n",
        "ask[0].sets",
        "the sets do not read as the sets of a document",
    );
    refused(
        "['not_null', {}, ['field', {}, 'first']]",
        "['not_null', {}, ['field', {}, 'nowhere']]",
        "ask[0].sets.visits.where[1]",
        "nowhere is no field of a session the catalog lists",
    );
    // raw names a text field of a stack
    refused(
        "  - field: text_series_description",
        "  - field: echo_time",
        "raw[0].field",
        "raw names a text field",
    );
    refused(
        "  - field: text_series_description",
        "  - field: text_series_colour",
        "raw[0].field",
        "text_series_colour is no field of a stack the catalog lists",
    );
}

/// The families the pack ships, held where a bad one must fail: the
/// catalog is built over them, every clause of every product validates as
/// a clause of a stack set, and the sets of every worked example read as
/// the sets of a document and validate as one, with a session set for
/// each set an example reads and leaves to the question around it.
#[test]
fn the_shipped_pack_s_families_hold_and_validate_as_questions() {
    use nils_ask::ast::Set;
    use std::collections::BTreeMap;
    let pack =
        nils_pack::load(&common::root().join("packs/mri"), None).unwrap_or_else(|e| panic!("{e}"));
    assert!(!pack.families.is_empty());
    nils_catalog::families::held_alone(&pack).unwrap_or_else(|e| panic!("{e}"));
    for mut l in common::labs("nils_families_shipped_test") {
        // a catalog is built of the shipped pack, its families with it
        let catalog = Catalog::build(&mut l.registry, &pack).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(catalog.families.len(), pack.families.len(), "{}", l.name);
        let served = catalog.document(&Scope::default());
        assert_eq!(
            served["families"].as_array().map(Vec::len),
            Some(pack.families.len()),
            "{}",
            l.name
        );
        let scope = common::quasi();
        for f in &pack.families {
            for p in &f.products {
                let doc = json!({
                    "ast_version": 1,
                    "sets": {"product": {"grain": "stack", "where": p.where_}},
                    "out": {"set": "product", "level": "count"}
                })
                .to_string();
                let ask = nils_ask::parse(&doc)
                    .unwrap_or_else(|e| panic!("{}: {}.{}: {e}", l.name, f.family, p.id));
                nils_ask::prepare(ask, &catalog, &scope)
                    .unwrap_or_else(|e| panic!("{}: {}.{}: {e}", l.name, f.family, p.id));
            }
            for (i, e) in f.ask.iter().enumerate() {
                let at = format!("{}: {}.ask[{i}]", l.name, f.family);
                let value = nils_ask::read(&e.sets).unwrap_or_else(|why| panic!("{at}: {why}"));
                let sets: BTreeMap<String, Set> = serde_json::from_value(value.clone())
                    .unwrap_or_else(|why| panic!("{at}: {why}"));
                assert!(!sets.is_empty(), "{at}");
                // the sets an example reads and does not write are the
                // question's around it: the visits, as sessions
                let mut whole = value.as_object().unwrap().clone();
                let mut read: Vec<String> = Vec::new();
                for set in sets.values() {
                    read.extend(set.reads().into_iter().map(str::to_string));
                }
                for name in read {
                    if !whole.contains_key(&name) {
                        whole.insert(name, json!({"grain": "session"}));
                    }
                }
                // the answer is a set nothing else reads
                let out = sets
                    .keys()
                    .find(|n| !sets.values().any(|s| s.reads().contains(&n.as_str())))
                    .unwrap_or_else(|| panic!("{at}: every set is read by another"));
                let doc = json!({
                    "ast_version": 1,
                    "sets": whole,
                    "out": {"set": out, "level": "count"}
                })
                .to_string();
                let ask = nils_ask::parse(&doc).unwrap_or_else(|why| panic!("{at}: {why}"));
                nils_ask::prepare(ask, &catalog, &scope)
                    .unwrap_or_else(|why| panic!("{at}: {why}"));
            }
        }
    }
}
