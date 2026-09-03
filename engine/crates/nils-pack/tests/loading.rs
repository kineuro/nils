// SPDX-License-Identifier: AGPL-3.0-only

//! What a pack that is wrong does: it does not load, and it says where.
//!
//! A pack is written by someone who writes vocabulary, so every refusal here
//! is checked for naming the file, the line and the path, not just failing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

/// A pack directory built for one test and removed after it.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-pack-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(p.join("corpus")).unwrap();
        Dir(p)
    }

    fn file(&self, name: &str, body: &str) -> &Dir {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const MANIFEST: &str = "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
";

const PARSERS: &str = "\
parsers:
  image_type:
    field: image_type
    case: upper
    tokenize: {split: '[\\\\/\\s]+'}
    predicates:
      is_original: {token: ORIGINAL}
";

const FLAGS: &str = "\
flags:
  is_original: image_type.is_original
";

const CASES: &str = "\
cases:
  - name: an original image is original
    stack: {image_type: 'ORIGINAL\\PRIMARY'}
    flags: {is_original: true}
";

fn good() -> Dir {
    let d = Dir::new();
    d.file("pack.yml", MANIFEST)
        .file("parsers.yml", PARSERS)
        .file("flags.yml", FLAGS)
        .file("corpus/cases.yml", CASES);
    d
}

fn refusal(d: &Dir) -> String {
    match nils_pack::load(d.path(), None) {
        Ok(_) => panic!("the pack loaded and should not have"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_pack_that_is_right_loads() {
    let d = good();
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.id(), "t@1.0.0");
    assert_eq!(pack.cases, 1);
}

#[test]
fn a_flag_naming_something_that_is_not_there_is_refused_with_the_line() {
    let d = good();
    d.file(
        "flags.yml",
        "flags:\n  is_original: image_type.is_original\n  is_odd: image_type.no_such_predicate\n",
    );
    let e = refusal(&d);
    assert!(e.contains("flags.yml:3:"), "{e}");
    assert!(e.contains("flags.is_odd"), "{e}");
    assert!(e.contains("no predicate no_such_predicate"), "{e}");
}

#[test]
fn a_flag_naming_a_parser_that_is_not_there_says_which() {
    let d = good();
    d.file(
        "flags.yml",
        "flags:\n  is_odd: no_such_parser.is_original\n",
    );
    let e = refusal(&d);
    assert!(e.contains("no parser named no_such_parser"), "{e}");
}

#[test]
fn a_field_that_is_not_a_field_is_refused() {
    let d = good();
    d.file(
        "flags.yml",
        "flags:\n  is_odd: {field: no_such_field, gt: 0}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("no field named no_such_field"), "{e}");
}

#[test]
fn a_flag_that_depends_on_itself_is_refused_by_name() {
    let d = good();
    d.file("flags.yml", "flags:\n  a: {any: [b]}\n  b: {any: [a]}\n");
    let e = refusal(&d);
    assert!(e.contains("depends on itself"), "{e}");
    assert!(e.contains("flags.a") || e.contains("flags.b"), "{e}");
}

#[test]
fn a_condition_with_no_meaning_is_refused_rather_than_ignored() {
    let d = good();
    d.file("flags.yml", "flags:\n  is_odd: {wobble: 3}\n");
    let e = refusal(&d);
    assert!(e.contains("no condition has the keys"), "{e}");
}

#[test]
fn a_pattern_that_will_not_compile_is_refused_before_it_runs() {
    let d = good();
    d.file(
        "flags.yml",
        "flags:\n  is_odd: {text: text_all, matches: '([unclosed'}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("is not a regular expression"), "{e}");
}

#[test]
fn a_contract_the_engine_does_not_implement_is_refused() {
    let d = good();
    d.file("pack.yml", &MANIFEST.replace("contract: 1", "contract: 99"));
    let e = refusal(&d);
    assert!(e.contains("contract 99"), "{e}");
    assert!(e.contains("half-understood"), "{e}");
}

#[test]
fn a_version_that_is_not_semantic_is_refused() {
    let d = good();
    d.file(
        "pack.yml",
        &MANIFEST.replace("version: 1.0.0", "version: '1.0'"),
    );
    let e = refusal(&d);
    assert!(e.contains("is not a version"), "{e}");
}

#[test]
fn a_pack_with_no_corpus_does_not_load() {
    let d = Dir::new();
    d.file("pack.yml", MANIFEST)
        .file("parsers.yml", PARSERS)
        .file("flags.yml", FLAGS);
    let e = refusal(&d);
    assert!(e.contains("this one has none"), "{e}");
}

#[test]
fn a_pack_whose_corpus_fails_does_not_load() {
    let d = good();
    d.file(
        "corpus/cases.yml",
        "cases:\n  - name: a wrong claim\n    stack: {image_type: 'DERIVED\\PRIMARY'}\n    flags: {is_original: true}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("do not hold, so it does not load"), "{e}");
    assert!(e.contains("a wrong claim: is_original is false"), "{e}");
}

#[test]
fn a_case_asserting_a_flag_the_pack_has_not_got_is_refused() {
    let d = good();
    d.file(
        "corpus/cases.yml",
        "cases:\n  - name: c\n    stack: {image_type: 'ORIGINAL'}\n    flags: {is_purple: true}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("no flag named is_purple"), "{e}");
}

#[test]
fn a_case_naming_a_field_that_is_not_one_is_refused() {
    let d = good();
    d.file(
        "corpus/cases.yml",
        "cases:\n  - name: c\n    stack: {wobble: 1}\n    flags: {is_original: false}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("no field named wobble"), "{e}");
}

#[test]
fn a_parser_declared_twice_is_refused() {
    let d = good();
    d.file(
        "pack.yml",
        &MANIFEST.replace(
            "parsers: [parsers.yml]",
            "parsers: [parsers.yml, again.yml]",
        ),
    )
    .file("again.yml", PARSERS);
    let e = refusal(&d);
    assert!(e.contains("is declared twice"), "{e}");
}

#[test]
fn a_review_threshold_for_an_axis_that_does_not_exist_is_refused() {
    let d = good();
    d.file(
        "pack.yml",
        &format!("{MANIFEST}review:\n  low_confidence: {{technique: 0.8}}\n"),
    );
    let e = refusal(&d);
    assert!(e.contains("no axis named technique"), "{e}");
    assert!(e.contains("pack.yml"), "{e}");
}

#[test]
fn a_pack_that_declares_no_review_asks_about_nothing() {
    let d = good();
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.review.below("technique"), 0.0);
    assert!(!pack.review.asks_when_missing("technique"));
}

#[test]
fn a_pack_may_say_that_a_stack_is_nobody_s_question() {
    let d = good();
    d.file(
        "pack.yml",
        &format!("{MANIFEST}review:\n  silent_when: is_original\n"),
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    let mut original = nils_pack::Stack::new();
    original
        .set(
            "image_type",
            nils_pack::stack::Value::Text(Some("ORIGINAL\\PRIMARY")),
        )
        .unwrap();
    assert!(
        nils_pack::Evaluated::new(&pack, &original)
            .classify()
            .silent,
        "the pack rules this stack out of the queue"
    );
    let mut derived = nils_pack::Stack::new();
    derived
        .set(
            "image_type",
            nils_pack::stack::Value::Text(Some("DERIVED\\SECONDARY")),
        )
        .unwrap();
    assert!(!nils_pack::Evaluated::new(&pack, &derived).classify().silent);
}
