// SPDX-License-Identifier: AGPL-3.0-only

//! The pack's own shape (record 41, S3): the values no rule reaches, and the
//! clauses that only restate another axis.

use std::fs;
use std::path::{Path, PathBuf};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nils-shape-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).expect("a directory");
        Dir(path)
    }

    fn file(&self, name: &str, body: &str) -> &Dir {
        let at = self.0.join(name);
        fs::create_dir_all(at.parent().expect("a parent")).expect("a directory");
        fs::write(at, body).expect("a file");
        self
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A small pack with one of each thing planted: a value no rule names, a
/// value only a rule gated on that one names, a value its exclusion group
/// always beats, an implication and a clause that looks like one and is not.
fn planted() -> Dir {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "\
pack: planted
version: 0.1.0
contract: 1
modality: XR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/purpose.yml, axes/plane.yml, axes/grade.yml]
rules: [rules/tilt.yml, rules/grading.yml]
order: [purpose, plane, tilt, grading]
",
    );
    d.file(
        "parsers.yml",
        "\
parsers:
  image_type:
    field: image_type
    case: upper
    tokenize: {split: '[\\\\\\\\/\\\\s]+'}
    predicates:
      is_survey: {token: SURVEY}
      is_final: {token: FINAL}
",
    );
    d.file(
        "flags.yml",
        "\
flags:
  is_survey: image_type.is_survey
  is_final: image_type.is_final
",
    );
    // `archived` is planted: nothing names it.
    d.file(
        "axes/purpose.yml",
        "\
axis: purpose
kind: single
default: unstated
values:
  'diagnosis':
    keywords: [diagnostic, report]
  'survey':
    detection:
      exclusive: is_survey
  'archived': {}
",
    );
    // `skew` is planted: the one rule that writes it writes `flat` beside
    // it, and `flat` wins their group.
    d.file(
        "axes/plane.yml",
        "\
axis: plane
kind: multi
order: [flat, oblique]
values:
  'flat': {keywords: [flat, ap, pa], group: G, priority: 1}
  'oblique': {keywords: [oblique, angled], group: G, priority: 2}
  'skew': {group: G, priority: 3}
",
    );
    d.file(
        "axes/grade.yml",
        "\
axis: grade
kind: single
values:
  'low': {}
  'high': {}
  'mid': {}
",
    );
    d.file(
        "rules/tilt.yml",
        "\
rule_set: tilt
decides: [plane]
adds: [plane]
order: [both]
rules:
  both:
    clauses:
      - {flag: is_final, tier: exclusive}
    set: {plane: [flat, skew]}
    confidence: 0.9
",
    );
    // `high` is planted: only a purpose nothing reaches leads to it. The
    // first rule restates purpose; the third reads a flag beside it.
    d.file(
        "rules/grading.yml",
        "\
rule_set: grading
decides: [grade]
order: [survey_is_low, archived_is_high, final_diagnosis_is_mid]
rules:
  survey_is_low:
    clauses:
      - {when: {axis: purpose, is: survey}, cite: survey, source: purpose}
    set: {grade: low}
    confidence: 0.9
  archived_is_high:
    clauses:
      - {when: {axis: purpose, is: archived}, cite: archived, source: purpose}
    set: {grade: high}
    confidence: 0.9
  final_diagnosis_is_mid:
    clauses:
      - {when: {all: [is_final, {axis: purpose, is: diagnosis}]}, cite: diagnosis, source: purpose}
    set: {grade: mid}
    confidence: 0.9
",
    );
    d.file(
        "corpus/cases.yml",
        "\
cases:
  - name: a survey image is graded low, because it is a survey
    stack:
      image_type: 'ORIGINAL\\\\SURVEY'
      text_all: 'ap survey'
    axes:
      purpose: survey
      plane: flat
      grade: low
",
    );
    d
}

#[test]
fn a_planted_unreachable_value_is_named_with_its_reason() {
    let d = planted();
    let pack = nils_pack::load(d.path(), None).expect("the planted pack loads");
    let shape = pack.shape();
    let unreached: Vec<(String, String, String)> = shape
        .axes
        .iter()
        .flat_map(|a| {
            a.unreachable
                .iter()
                .map(|u| (a.axis.clone(), u.value.clone(), u.why.clone()))
        })
        .collect();
    let want = |a: &str, v: &str, w: &str| (a.to_string(), v.to_string(), w.to_string());
    assert_eq!(
        unreached,
        [
            want("purpose", "archived", "no_rule"),
            want("plane", "skew", "exclusion_group"),
            want("grade", "high", "no_valid_combination"),
        ],
        "{shape}"
    );
    let grade = shape.axes.iter().find(|a| a.axis == "grade").unwrap();
    assert_eq!(grade.unreachable[0].rules, ["grading/archived_is_high"]);
    assert_eq!(grade.reached, 2);
    let plane = shape.axes.iter().find(|a| a.axis == "plane").unwrap();
    assert_eq!(plane.unreachable[0].by.as_deref(), Some("flat"));
    assert_eq!(shape.counts(), (9, 6));
}

#[test]
fn a_clause_that_reads_only_another_axis_is_an_implication() {
    let d = planted();
    let pack = nils_pack::load(d.path(), None).expect("the planted pack loads");
    let shape = pack.shape();
    let named: Vec<(&str, &str, usize)> = shape
        .implications
        .iter()
        .map(|i| (i.rule_set.as_str(), i.rule.as_str(), i.clause))
        .collect();
    // The third rule's clause reads purpose too, and a flag beside it, so
    // its vote carries evidence of its own.
    assert_eq!(
        named,
        [
            ("grading", "survey_is_low", 0),
            ("grading", "archived_is_high", 0)
        ]
    );
    assert_eq!(shape.implications[0].reads, ["purpose"]);
    assert_eq!(shape.implications[0].writes, ["grade=low"]);

    // and the same answer is on the rule, for whoever records a vote
    let grading = pack.rule_sets.iter().find(|s| s.name == "grading").unwrap();
    let flags: Vec<bool> = grading.rules.iter().map(|r| r.restates(0)).collect();
    assert_eq!(flags, [true, true, false]);
    let purpose = pack.rule_sets.iter().find(|s| s.name == "purpose").unwrap();
    assert!(purpose.rules.iter().all(|r| !r.restates(0)));

    // the short form says all of it in a few lines
    let text = shape.to_string();
    assert!(text.starts_with("planted@0.1.0: 6 of 9 values reached by a rule, 3 unreachable, 2 clauses restate another axis"), "{text}");
    assert!(
        text.contains("unreachable  grade.high  grading/archived_is_high can never fire"),
        "{text}"
    );
}

#[test]
fn a_rule_that_reads_an_axis_decided_after_it_does_not_reach() {
    // The same pack with grading run before purpose: at that point purpose
    // holds nothing, so survey_is_low cannot fire either.
    let d = planted();
    d.file(
        "pack.yml",
        "\
pack: planted
version: 0.1.0
contract: 1
modality: XR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/purpose.yml, axes/plane.yml, axes/grade.yml]
rules: [rules/tilt.yml, rules/grading.yml]
order: [grading, purpose, plane, tilt]
",
    );
    // and its case no longer claims a grade, which it would not get
    d.file(
        "corpus/cases.yml",
        "\
cases:
  - name: a survey image is a survey
    stack:
      image_type: 'ORIGINAL\\\\SURVEY'
      text_all: 'ap survey'
    axes:
      purpose: survey
",
    );
    let pack = nils_pack::load(d.path(), None).expect("the pack loads");
    let shape = pack.shape();
    let grade = shape.axes.iter().find(|a| a.axis == "grade").unwrap();
    assert_eq!(grade.reached, 0, "{shape}");
}

#[test]
fn the_mri_pack_has_a_shape() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&dir, None).expect("the MRI pack loads");
    let shape = pack.shape();
    let (values, reached) = shape.counts();
    assert!(reached <= values && reached > values * 9 / 10, "{shape}");
    // What it said when S3 landed: two values nothing writes, and v0's
    // marker for no answer is one of them.
    let base = shape.axes.iter().find(|a| a.axis == "base").unwrap();
    assert!(
        base.unreachable
            .iter()
            .any(|u| u.value == "Unknown" && u.why == "no_rule"),
        "{shape}"
    );
    // The technique-locked base contrasts are the implications the record
    // names first.
    assert!(
        shape.implications.iter().any(|i| i.rule_set == "base"
            && i.rule == "technique:MPRAGE"
            && i.writes == ["base=T1w"])
    );
    assert!(shape.implications.iter().all(|i| !i.reads.is_empty()));
}
