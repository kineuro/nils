// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: candidate identity rules probed side by side over a
//! sample, shapes only, nothing written, no path in the answer.

mod common;

use common::mr;
use nils_dicom::synth::TempDir;
use nils_digest::Rule;

/// Three people, two studies each, four files a study; the identifier in
/// the first path segment and a placeholder in the tag.
fn tree() -> TempDir {
    let dir = TempDir::new("probe");
    for (person, studies) in [
        ("AAA111", ["A", "B"]),
        ("BBB222", ["C", "D"]),
        ("CCC333", ["E", "F"]),
    ] {
        for study in studies {
            for i in 1..=4 {
                dir.file(
                    &format!("{person}/{study}/IM_{i:04}"),
                    &mr(
                        study,
                        &format!("{study}.1"),
                        &format!("{study}.1.{i}"),
                        "XXXX",
                        &[],
                    ),
                );
            }
        }
    }
    dir
}

#[test]
fn two_candidates_side_by_side_name_the_placeholder_by_shape_and_leak_nothing() {
    let dir = tree();
    let tag_only =
        Rule::parse("identity:\n  id_type: patient-id\n  from:\n    - field: PatientID\n").unwrap();
    let path_too = Rule::parse(
        "identity:\n  id_type: subject-code\n  code: verbatim\n  from:\n    \
         - field: PatientID\n      pattern: '^(?<id>[A-Z]{3}[0-9]{3})$'\n    \
         - path:\n        segment: 1\n        pattern: '^(?<id>.+)$'\n",
    )
    .unwrap();
    let doc = nils_digest::probe::probe(
        dir.path(),
        1_000,
        &[
            ("tag".to_string(), tag_only),
            ("path".to_string(), path_too),
        ],
        3,
    )
    .unwrap();
    assert_eq!(doc["sample"]["files"], 24, "{doc}");
    assert_eq!(doc["sample"]["parsed"], 24, "{doc}");
    let c = &doc["candidates"];
    assert_eq!(c.as_array().unwrap().len(), 2);

    // The tag alone: one subject, which is the placeholder, and the batch
    // level fact that says so, as a shape.
    assert_eq!(c[0]["label"], "tag");
    assert_eq!(c[0]["subjects"], 1, "{}", c[0]);
    assert_eq!(c[0]["studies"], 6, "{}", c[0]);
    assert_eq!(c[0]["identity_constant"]["constant"], true, "{}", c[0]);
    assert_eq!(c[0]["identity_constant"]["shape"], "AAAA", "{}", c[0]);
    assert_eq!(c[0]["sources"][0]["source"], "PatientID");
    assert_eq!(c[0]["sources"][0]["answered"], 24, "{}", c[0]);
    assert_eq!(c[0]["sources"][0]["shapes"]["AAAA"], 24, "{}", c[0]);

    // With the path: three subjects; the tag declined every file and the
    // folder answered every one.
    assert_eq!(c[1]["label"], "path");
    assert_eq!(c[1]["subjects"], 3, "{}", c[1]);
    assert_eq!(c[1]["fell_back"], 0, "{}", c[1]);
    assert_eq!(c[1]["sources"][0]["unparsed"], 24, "{}", c[1]);
    assert_eq!(c[1]["sources"][1]["source"], "path segment 1");
    assert_eq!(c[1]["sources"][1]["answered"], 24, "{}", c[1]);
    assert_eq!(c[1]["sources"][1]["shapes"]["AAA999"], 24, "{}", c[1]);
    assert!(c[1]["diagnostics"].is_object(), "{}", c[1]);

    // Nothing seeded and no path escapes: not the placeholder, not a code,
    // not a directory.
    let text = doc.to_string();
    for marker in ["XXXX", "AAA111", "BBB222", "CCC333", "IM_0001"] {
        assert!(!text.contains(marker), "{marker} escaped: {text}");
    }
    assert!(
        !text.contains(&dir.path().display().to_string()),
        "the root escaped: {text}"
    );
    // and nothing was written: the tree is as it was
    assert_eq!(nils_dicom::survey::files_under(dir.path(), 100).len(), 24);
}

#[test]
fn a_probe_of_nothing_is_refused_and_a_missing_root_is_named_without_its_path() {
    let dir = tree();
    let e = nils_digest::probe::probe(dir.path(), 10, &[], 1).unwrap_err();
    assert!(e.contains("no candidate"), "{e}");
    let missing = dir.path().join("nowhere");
    let e =
        nils_digest::probe::probe(&missing, 10, &[("d".into(), Rule::default())], 1).unwrap_err();
    assert!(e.contains("cannot be read"), "{e}");
    assert!(!e.contains("nowhere"), "the answer names no path: {e}");
}
