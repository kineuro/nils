// SPDX-License-Identifier: AGPL-3.0-only

//! What a pack that is wrong does: it does not load, and it says where.
//!
//! A pack is written by someone who writes vocabulary, so every refusal here
//! is checked for naming the file, the line and the path, not just failing.

mod common;

use common::Dir;

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
    let d = Dir::new("pack-test");
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
    let d = Dir::new("pack-test");
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

// ---------------------------------------------------------------------------
// The disposition phase (Wave 3 §7)

/// The good pack, plus an axis decided after the passes and one decided with
/// the rest.
fn phased() -> Dir {
    let d = good();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/disposition.yml]
rules: [rules/kind.yml, rules/disposition.yml]
order: [kind, disposition]
",
    )
    .file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues: {a: {}, b: {}}\n",
    )
    .file(
        "axes/disposition.yml",
        "axis: disposition\nphase: disposition\nkind: single\nvalues: {keep: {}, drop: {}}\n",
    )
    .file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a}
",
    )
    .file(
        "rules/disposition.yml",
        "\
rule_set: disposition
decides: [disposition]
tiers: {stated: 0.9}
order: [from_kind]
rules:
  from_kind:
    clauses: [{when: {axis: kind, is: a}, cite: a, source: kind}]
    set: {disposition: keep}
",
    );
    d
}

#[test]
fn an_axis_may_be_decided_after_the_passes() {
    let d = phased();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let kind = pack.axis_index("kind").unwrap();
    let disposition = pack.axis_index("disposition").unwrap();
    assert_eq!(pack.axes[kind].phase, nils_pack::rules::AxisPhase::Class);
    assert_eq!(
        pack.axes[disposition].phase,
        nils_pack::rules::AxisPhase::Disposition
    );

    // The two phases decide their own axes and leave the other's alone.
    let mut stack = nils_pack::stack::Stack::new();
    stack
        .set(
            "image_type",
            nils_pack::stack::Value::Text(Some("ORIGINAL\\PRIMARY")),
        )
        .unwrap();
    let e = nils_pack::eval::Evaluated::new(&pack, &stack);
    let verdict = e.classify();
    assert_eq!(verdict.stored("kind"), "a");
    assert_eq!(verdict.stored("disposition"), "", "not this phase's to say");

    let mut seed = vec![Vec::new(); pack.axes.len()];
    seed[kind] = vec!["a".to_string()];
    let after = e.dispose(&seed);
    assert_eq!(after.stored("disposition"), "keep");
    assert_eq!(after.stored("kind"), "", "already decided, and not again");
}

#[test]
fn a_rule_that_runs_before_the_passes_may_not_read_what_runs_after_them() {
    // Nothing stops it at run time: an axis nothing has decided reads as
    // empty, so the rule takes the wrong branch and says nothing about it.
    let d = phased();
    d.file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    clauses: [{when: {axis: disposition, is: keep}, cite: keep, source: disposition}]
    set: {kind: a}
",
    );
    let e = refusal(&d);
    assert!(e.contains("decided after the passes"), "{e}");
    assert!(e.contains("disposition"), "{e}");
}

#[test]
fn a_rule_set_decides_one_phase() {
    let d = phased();
    d.file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind, disposition]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a, disposition: keep}
",
    );
    let e = refusal(&d);
    assert!(e.contains("a rule set decides one phase"), "{e}");
}

// ---------------------------------------------------------------------------
// `redecides` (MRI pack 0.9.0): a route entered on an axis may decide it again
// for the stacks whose output is the acquisition itself.

/// The phased pack, plus an axis `part` and a route entered on `kind: a` that
/// says a primary image is of kind `b`.
fn rerouted(redecides: &str) -> Dir {
    let d = phased();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/part.yml, axes/disposition.yml]
rules: [rules/kind.yml, rules/route.yml, rules/disposition.yml]
order: [kind, route, disposition]
",
    )
    .file(
        "parsers.yml",
        "\
parsers:
  image_type:
    field: image_type
    case: upper
    tokenize: {split: '[\\\\/\\s]+'}
    predicates:
      is_original: {token: ORIGINAL}
      is_primary: {token: PRIMARY}
",
    )
    .file(
        "flags.yml",
        "flags:\n  is_original: image_type.is_original\n  is_primary: image_type.is_primary\n",
    )
    .file(
        "axes/part.yml",
        "axis: part\nkind: single\nvalues: {one: {}, two: {}}\n",
    )
    .file(
        "rules/route.yml",
        &format!(
            "\
rule_set: route
enter_when: {{axis: kind, is: a}}
decides: [kind, part]
{redecides}
tiers: {{stated: 0.9}}
order: [primary, otherwise]
rules:
  primary:
    clauses: [{{flag: is_primary, tier: stated}}]
    set: {{kind: b, part: one}}
  otherwise:
    clauses: [{{when: true, cite: a, source: kind}}]
    set: {{part: two}}
"
        ),
    );
    d
}

fn stack(image_type: &str) -> nils_pack::stack::Stack {
    let mut stack = nils_pack::stack::Stack::new();
    stack
        .set(
            "image_type",
            nils_pack::stack::Value::Text(Some(image_type)),
        )
        .unwrap();
    stack
}

#[test]
fn a_route_may_decide_again_the_axis_it_was_entered_on() {
    let d = rerouted("redecides: [kind]");
    let pack = nils_pack::load(d.path(), None).unwrap();

    // The route's rule for the acquisition's own image replaces the kind the
    // earlier set decided, and the rest of its answer stands beside it.
    let primary = stack("ORIGINAL\\PRIMARY");
    let v = nils_pack::eval::Evaluated::new(&pack, &primary).classify();
    assert_eq!(v.stored("kind"), "b");
    assert_eq!(v.stored("part"), "one");

    // A rule that writes nothing there leaves the earlier answer alone.
    let secondary = stack("ORIGINAL\\SECONDARY");
    let v = nils_pack::eval::Evaluated::new(&pack, &secondary).classify();
    assert_eq!(v.stored("kind"), "a");
    assert_eq!(v.stored("part"), "two");

    // A person's answer is never moved.
    let e = nils_pack::eval::Evaluated::new(&pack, &primary);
    let kind = pack.axis_index("kind").unwrap();
    let mut pins = vec![None; pack.axes.len()];
    pins[kind] = Some(vec!["a".to_string()]);
    let v = e.with_pins(&pins, &|_, _| false);
    assert_eq!(v.stored("kind"), "a");
    assert_eq!(v.stored("part"), "one");
}

#[test]
fn without_redecides_a_route_leaves_an_earlier_answer_alone() {
    let d = rerouted("");
    let pack = nils_pack::load(d.path(), None).unwrap();
    let primary = stack("ORIGINAL\\PRIMARY");
    let v = nils_pack::eval::Evaluated::new(&pack, &primary).classify();
    assert_eq!(
        v.stored("kind"),
        "a",
        "an axis a set decided is not decided again"
    );
    assert_eq!(v.stored("part"), "one");
}

#[test]
fn a_set_redecides_only_a_single_valued_axis_it_decides() {
    let d = rerouted("redecides: [disposition]");
    let e = refusal(&d);
    assert!(e.contains("redecides"), "{e}");
    assert!(e.contains("not in `decides`"), "{e}");
}

/// Record 41: a longhand rule's `requires` was read by nothing, so the rule
/// fired where its author said it must not. It gates the rule now, as it
/// gates a value of an axis file.
#[test]
fn a_longhand_rule_is_gated_by_its_requires() {
    let d = phased();
    d.file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [gated, otherwise]
rules:
  gated:
    requires: {not: is_original}
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a}
  otherwise:
    clauses: [{when: true, cite: anything, source: none}]
    set: {kind: b}
",
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    let mut stack = nils_pack::stack::Stack::new();
    stack
        .set(
            "image_type",
            nils_pack::stack::Value::Text(Some("ORIGINAL\\PRIMARY")),
        )
        .unwrap();
    let verdict = nils_pack::eval::Evaluated::new(&pack, &stack).classify();
    assert_eq!(
        verdict.stored("kind"),
        "b",
        "the clause held and the gate did not"
    );
}

#[test]
fn a_key_a_longhand_rule_does_not_have_is_refused_rather_than_ignored() {
    let d = phased();
    d.file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    require: {not: is_original}
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a}
",
    );
    let e = refusal(&d);
    assert!(e.contains("rules.original.require"), "{e}");
    assert!(e.contains("is not a key of a rule"), "{e}");
    assert!(e.contains("kind.yml"), "{e}");
}

#[test]
fn a_longhand_rule_may_not_require_what_is_decided_after_the_passes() {
    let d = phased();
    d.file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    requires: {axis: disposition, is: keep}
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a}
",
    );
    let e = refusal(&d);
    assert!(e.contains("decided after the passes"), "{e}");
}

const PRIVATE: &str = "\
private:
  coverage: [a test vendor]
  ingest:
    - creator: A VENDOR
      group: 0x0019
      element: 0x0C
      name: vendor_b_value
      why: a test
  release:
    - creator: A VENDOR
      group: 0x0019
      element: 0x0C
      why: a test
";

/// The manifest of `good()` with a private file and a dictionary beside it.
fn with_private() -> Dir {
    let d = good();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
private: [private.yml]
dictionary: [dictionary.tsv]
",
    )
    .file("private.yml", PRIVATE)
    .file("dictionary.tsv", "A VENDOR\t0019\t0C\tIS\t1\tB value\n");
    d
}

#[test]
fn an_ingested_element_is_a_field_with_the_dictionary_s_vr_and_name() {
    // Wave 4a section 5.2: the two lists, and the dictionary naming the entry.
    let d = with_private();
    let pack = nils_pack::load(d.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.ingest.len(), 1);
    assert_eq!(pack.ingest[0].name, "vendor_b_value");
    assert_eq!(pack.ingest[0].vr.as_deref(), Some("IS"));
    assert_eq!(pack.ingest[0].dictionary_name.as_deref(), Some("B value"));
    assert_eq!(pack.ingest[0].address(), "0019xx0C A VENDOR");
    assert_eq!(pack.release.len(), 1);
    assert_eq!(pack.private_coverage, vec!["a test vendor".to_string()]);
    assert_eq!(pack.dictionary.len(), 1);
}

#[test]
fn a_flag_and_a_case_may_name_an_ingested_element() {
    // The whole point: a rule reads the vendor's parameter the way it reads
    // a standard field, and a corpus case sets it by the pack's name.
    let d = with_private();
    d.file(
        "flags.yml",
        "\
flags:
  is_original: image_type.is_original
  is_diffusion: {field: vendor_b_value, gt: 0}
",
    )
    .file(
        "corpus/cases.yml",
        "\
cases:
  - name: a b value above zero is diffusion
    stack: {image_type: 'ORIGINAL\\\\PRIMARY', vendor_b_value: '1000'}
    flags: {is_diffusion: true}
  - name: and none is not
    stack: {image_type: 'ORIGINAL\\\\PRIMARY'}
    flags: {is_diffusion: false}
",
    );
    let pack = nils_pack::load(d.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.cases, 2);
}

#[test]
fn the_old_keep_list_is_refused_by_its_new_name() {
    let d = with_private();
    d.file(
        "private.yml",
        "private:\n  keep:\n    - {creator: A VENDOR, group: 0x0019, element: 0x0C, why: old}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("private.keep"), "{e}");
    assert!(e.contains("release"), "{e}");
}

#[test]
fn an_ingested_name_that_is_already_a_field_is_refused() {
    // Shadowing the fingerprint's own field would make a rule read the
    // vendor's value where every other pack reads the standard's.
    let d = with_private();
    d.file(
        "private.yml",
        "private:\n  ingest:\n    - {creator: A VENDOR, group: 0x0019, element: 0x0C, name: echo_time, why: no}\n",
    );
    let e = refusal(&d);
    assert!(
        e.contains("echo_time is a field of the fingerprint already"),
        "{e}"
    );
    d.file(
        "private.yml",
        "private:\n  ingest:\n    - {creator: A VENDOR, group: 0x0019, element: 0x0C, name: twice, why: a}\n    - {creator: A VENDOR, group: 0x0019, element: 0x0D, name: twice, why: b}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("twice is declared twice"), "{e}");
    d.file(
        "private.yml",
        "private:\n  ingest:\n    - {creator: A VENDOR, group: 0x0019, element: 0x0C, name: 'Not A Name', why: a}\n",
    );
    let e = refusal(&d);
    assert!(e.contains("is not a field name"), "{e}");
}

#[test]
fn a_dictionary_line_that_is_wrong_is_refused_with_its_number() {
    let d = with_private();
    d.file("dictionary.tsv", "A VENDOR\t0019\t0C\tIS\n");
    let e = refusal(&d);
    assert!(e.contains("dictionary.tsv"), "{e}");
    assert!(e.contains("line 1"), "{e}");
}

/// Record 48 (pack contract 6): a pack with two class axes and a file of
/// exclusions or hints, written as `body`.
fn crossed(key: &str, body: &str) -> Dir {
    let d = phased();
    d.file(
        "pack.yml",
        &format!(
            "\
pack: t
version: 1.0.0
contract: 6
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/family.yml, axes/disposition.yml]
rules: [rules/kind.yml, rules/disposition.yml]
order: [kind, disposition]
{key}: [{key}.yml]
"
        ),
    )
    .file(
        "axes/family.yml",
        "axis: family\nkind: single\nvalues: {x: {family: SE}, y: {family: SE}, z: {family: GRE}, w: {aliases: [old-w]}}\n",
    )
    .file(&format!("{key}.yml"), body);
    d
}

#[test]
fn an_exclusion_between_axes_loads_with_its_family_named_value_by_value() {
    let d = crossed(
        "excludes",
        "excludes:\n  - {id: a-not-se, when: {axis: kind, is: a}, excludes: {axis: family, family: SE}, why: because, sources: [S1]}\n",
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.excludes.len(), 1);
    let every: Vec<usize> = (0..pack.axes.len()).collect();
    let (x, _) = nils_pack::legal::cross(&pack, &every);
    assert_eq!(x[0]["values"], serde_json::json!(["x", "y"]));
    assert_eq!(x[0]["when"], serde_json::json!({"axis": "kind", "is": "a"}));
    // an alias names the value it became
    let f = &pack.axes[pack.axis_index("family").unwrap()];
    assert_eq!(f.value_index("old-w"), f.value_index("w"));
}

#[test]
fn an_exclusion_that_reads_the_file_or_names_nothing_is_refused() {
    let e = refusal(&crossed(
        "excludes",
        "excludes:\n  - {id: bad, when: is_original, excludes: {axis: family, is: x}, why: w}\n",
    ));
    assert!(e.contains("axis values"), "{e}");
    let e = refusal(&crossed(
        "excludes",
        "excludes:\n  - {id: bad, when: {axis: kind, is: a}, excludes: {axis: family, family: EPI}, why: w}\n",
    ));
    assert!(e.contains("no value of family is of the family EPI"), "{e}");
    let e = refusal(&crossed(
        "excludes",
        "excludes:\n  - {id: bad, when: {axis: kind, is: a}, excludes: {axis: family, is: q}, why: w}\n",
    ));
    assert!(e.contains("q is not a value of the family axis"), "{e}");
    let e = refusal(&crossed(
        "excludes",
        "excludes:\n  - {id: one, when: {axis: kind, is: a}, excludes: {axis: family, is: x}, why: w}\n  - {id: one, when: {axis: kind, is: b}, excludes: {axis: family, is: y}, why: w}\n",
    ));
    assert!(e.contains("declared twice"), "{e}");
}

#[test]
fn a_hint_names_one_value_and_may_cite_what_contradicts_it() {
    let d = crossed(
        "hints",
        "hints:\n  - {id: a-usually-z, when: {axis: kind, is: a}, suggest: {axis: family, value: z}, why: usually, sources: [S1], counter: [S2]}\n",
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.hints[0].counter, ["S2"]);
    let every: Vec<usize> = (0..pack.axes.len()).collect();
    let (_, h) = nils_pack::legal::cross(&pack, &every);
    assert_eq!(h[0]["value"], "z");
    assert_eq!(h[0]["why"], "usually");
    let e = refusal(&crossed(
        "hints",
        "hints:\n  - {id: h, when: {axis: kind, is: a}, suggest: {axis: family, value: z}, why: w, excludes: {axis: family, is: x}}\n",
    ));
    assert!(
        e.contains("excludes is not a key of an entry of hints"),
        "{e}"
    );
}

/// MRI pack 0.10.0: a normalized text may leave one vendor's value of a field
/// out, because GE writes the exam's protocol into `ProtocolName`. The
/// corpus of the pack below is what says it works, for both vendors, and a
/// key the entry does not know is refused.
#[test]
fn a_normalized_text_leaves_out_the_field_a_vendor_fills_with_something_else() {
    let d = good();
    d.file(
        "pack.yml",
        "pack: t\nversion: 1.0.0\ncontract: 1\nmodality: MR\nparsers: [parsers.yml]\nflags: [flags.yml]\nnormalize: [normalize.yml]\n",
    )
    .file(
        "normalize.yml",
        "normalize: series_text\nfrom:\n  - text_series_description\n  - {field: text_protocol_name, unless_manufacturer: [GE]}\n",
    )
    .file(
        "flags.yml",
        "flags:\n  is_original: image_type.is_original\n  says_fmri: {text: series_text, substring: fmri}\n",
    )
    .file(
        "corpus/cases.yml",
        "cases:\n  - name: a GE exam protocol says nothing about the series\n    stack: {manufacturer: GE MEDICAL SYSTEMS, text_series_description: 'Sag T1', text_protocol_name: 'fmri exam'}\n    flags: {says_fmri: false}\n  - name: another vendor's protocol name is the series' own\n    stack: {manufacturer: SIEMENS, text_series_description: 'Sag T1', text_protocol_name: 'fmri exam'}\n    flags: {says_fmri: true}\n  - name: and the series' own words count for GE\n    stack: {manufacturer: GE MEDICAL SYSTEMS, text_series_description: 'rs fmri'}\n    flags: {says_fmri: true}\n",
    );
    let pack = nils_pack::load(d.path(), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(pack.cases, 3);

    d.file(
        "normalize.yml",
        "normalize: series_text\nfrom:\n  - {field: text_protocol_name, unless_vendor: [GE]}\n",
    );
    let why = refusal(&d);
    assert!(why.contains("unless_vendor is not a key here"), "{why}");
}

/// The good pack, plus an axis of three kinds and its coarse form of two
/// sizes, folded by a rule set: `a` and `b` are small, `c` is large.
fn coarse(size_axis: &str, size_rules: &str) -> Dir {
    let d = good();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/size.yml]
rules: [rules/kind.yml, rules/size.yml]
order: [kind, size]
",
    )
    .file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues: {a: {}, b: {aliases: [beta]}, c: {}}\n",
    )
    .file("axes/size.yml", size_axis)
    .file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {stated: 0.9}
order: [original]
rules:
  original:
    clauses: [{flag: is_original, tier: stated}]
    set: {kind: a}
",
    )
    .file("rules/size.yml", size_rules);
    d
}

const SIZE_AXIS: &str = "\
axis: size
kind: single
coarse_of: kind
values:
  small: {covers: [a, beta]}
  large: {covers: [c]}
";

const SIZE_RULES: &str = "\
rule_set: size
decides: [size]
tiers: {stated: 0.9}
order: [small, large]
rules:
  small:
    clauses: [{when: {any: [{axis: kind, is: a}, {axis: kind, is: b}]}, cite: small, source: kind}]
    set: {size: small}
  large:
    clauses: [{when: {axis: kind, is: c}, cite: large, source: kind}]
    set: {size: large}
";

#[test]
fn an_axis_may_say_it_is_the_coarse_form_of_another() {
    let d = coarse(SIZE_AXIS, SIZE_RULES);
    let pack = nils_pack::load(d.path(), None).unwrap();
    let size = &pack.axes[pack.axis_index("size").unwrap()];
    assert_eq!(size.coarse_of.as_deref(), Some("kind"));
    // a covered value named by an identity it had before is the value
    assert_eq!(size.values[0].covers, ["a", "b"]);
    assert_eq!(size.values[1].covers, ["c"]);
    let kind = &pack.axes[pack.axis_index("kind").unwrap()];
    assert_eq!(kind.coarse_of, None);
    assert!(kind.values.iter().all(|v| v.covers.is_empty()));
    // what a catalog serves: the coarse forms of the fine axis
    let of = nils_pack::coarse::of(&pack.axes, "kind");
    assert_eq!(of.len(), 1);
    assert_eq!(of[0].0, "size");
    assert_eq!(of[0].1[0].0, "small");
    assert!(nils_pack::coarse::of(&pack.axes, "size").is_empty());
}

/// What a sort reads of a coarse form: the coarse value that covers a
/// value of the finer axis, whether one covers another, and the pairs that
/// do not agree, each by any name of a value and as a row stores it.
#[test]
fn a_coarse_form_folds_and_covers_by_any_name_of_a_value() {
    let d = coarse(SIZE_AXIS, SIZE_RULES);
    let pack = nils_pack::load(d.path(), None).unwrap();
    let forms = nils_pack::coarse::forms(&pack);
    assert_eq!(forms.len(), 1);
    let f = forms[0];
    assert_eq!(
        (f.coarse.name.as_str(), f.fine.name.as_str()),
        ("size", "kind")
    );
    assert_eq!(f.fold("a"), Some(Some("small")));
    assert_eq!(f.fold("beta"), Some(Some("small")), "a name b had before");
    assert_eq!(f.fold(" c "), Some(Some("large")));
    assert_eq!(f.fold(""), None, "an axis that holds nothing");
    assert_eq!(f.fold("zebra"), None, "no value of the finer axis");
    assert_eq!(f.covers("small", "b"), Some(true));
    assert_eq!(f.covers("large", "a"), Some(false));
    assert_eq!(f.covers("large", ""), None);
    assert_eq!(f.covers("", "a"), None);
    assert_eq!(f.covers("huge", "a"), None);
    let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        f.apart(),
        [
            ("small".to_string(), words(&["c"])),
            ("large".to_string(), words(&["a", "b"])),
        ]
    );

    // a finer value no coarse value covers folds to nothing
    let d = coarse(&SIZE_AXIS.replace("[a, beta]", "[a]"), SIZE_RULES_A);
    let pack = nils_pack::load(d.path(), None).unwrap();
    let f = nils_pack::coarse::forms(&pack)[0];
    assert_eq!(f.fold("b"), Some(None));
    assert_eq!(f.covers("small", "b"), Some(false));

    // an axis that holds several values is no form a sort holds a stack to
    let d = coarse(
        &SIZE_AXIS.replace("kind: single", "kind: multi"),
        SIZE_RULES,
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert!(nils_pack::coarse::forms(&pack).is_empty());
    // and a pack that declares none has none
    let pack = nils_pack::load(good().path(), None).unwrap();
    assert!(nils_pack::coarse::forms(&pack).is_empty());
}

/// The size rules where `b` is covered by no size: only `a` is small.
const SIZE_RULES_A: &str = "\
rule_set: size
decides: [size]
tiers: {stated: 0.9}
order: [small, large]
rules:
  small:
    clauses: [{when: {axis: kind, is: a}, cite: small, source: kind}]
    set: {size: small}
  large:
    clauses: [{when: {axis: kind, is: c}, cite: large, source: kind}]
    set: {size: large}
";

#[test]
fn the_shipped_pack_says_the_body_region_is_the_body_part_s_coarse_form() {
    let pack = nils_pack::load(&common::shipped_mri(), None).unwrap();
    let region = &pack.axes[pack.axis_index("body_region").unwrap()];
    assert_eq!(region.coarse_of.as_deref(), Some("body_part"));
    let covers = |id: &str| -> Vec<&str> {
        region
            .values
            .iter()
            .find(|v| v.id == id)
            .unwrap()
            .covers
            .iter()
            .map(String::as_str)
            .collect()
    };
    assert_eq!(covers("head"), ["brain", "brain-neck"]);
    assert_eq!(covers("spine"), ["spine", "neck"]);
    assert_eq!(covers("chest"), ["chest"]);
    assert_eq!(covers("other"), ["other"]);
    // the word names the region, and no longer the fine value beside it
    let part = &pack.axes[pack.axis_index("body_part").unwrap()];
    let brain = part.values.iter().find(|v| v.id == "brain").unwrap();
    assert!(
        !brain.terms.iter().any(|t| t == "head"),
        "{:?}",
        brain.terms
    );
    let head = region.values.iter().find(|v| v.id == "head").unwrap();
    assert!(head.terms.iter().any(|t| t == "head"));
    assert!(head.description.as_deref().unwrap().contains("brain scan"));
}

#[test]
fn a_coarse_form_of_an_axis_that_is_not_declared_before_it_is_refused() {
    let d = coarse(
        &SIZE_AXIS.replace("coarse_of: kind", "coarse_of: sort"),
        SIZE_RULES,
    );
    let e = refusal(&d);
    assert!(e.contains("size.yml:3:"), "{e}");
    assert!(e.contains("coarse_of"), "{e}");
    assert!(
        e.contains("no axis named sort is declared before size"),
        "{e}"
    );

    let d = coarse(
        &SIZE_AXIS.replace("coarse_of: kind", "coarse_of: size"),
        SIZE_RULES,
    );
    let e = refusal(&d);
    assert!(
        e.contains("size cannot be the coarse form of itself"),
        "{e}"
    );
}

#[test]
fn a_covered_value_that_is_no_value_of_the_finer_axis_is_refused() {
    let d = coarse(&SIZE_AXIS.replace("[c]", "[c, d]"), SIZE_RULES);
    let e = refusal(&d);
    assert!(e.contains("size.yml"), "{e}");
    assert!(e.contains("values.large.covers"), "{e}");
    assert!(e.contains("d is no value of kind"), "{e}");
}

#[test]
fn a_value_covered_twice_is_refused() {
    let d = coarse(&SIZE_AXIS.replace("[c]", "[c, b]"), SIZE_RULES);
    let e = refusal(&d);
    assert!(e.contains("values.large.covers"), "{e}");
    assert!(e.contains("b is covered by small already"), "{e}");

    let d = coarse(&SIZE_AXIS.replace("[c]", "[c, c]"), SIZE_RULES);
    let e = refusal(&d);
    assert!(e.contains("c is listed twice"), "{e}");
}

#[test]
fn covers_without_coarse_of_and_coarse_of_without_covers_are_refused() {
    let d = coarse(&SIZE_AXIS.replace("coarse_of: kind\n", ""), SIZE_RULES);
    let e = refusal(&d);
    assert!(e.contains("values.small.covers"), "{e}");
    assert!(e.contains("size is the coarse form of no axis"), "{e}");

    let d = coarse(
        "axis: size\nkind: single\ncoarse_of: kind\nvalues: {small: {}, large: {}}\n",
        SIZE_RULES,
    );
    let e = refusal(&d);
    assert!(e.contains("no value of it covers a value"), "{e}");
}

/// A value of a coarse axis may list the values it covers that a question
/// which writes one alone is read as it for (`read_for`). The loader holds
/// the list to `covers`: a value of the finer axis, one this value covers,
/// listed once, and the value covers another beside it.
#[test]
fn a_coarse_value_may_say_which_values_it_is_read_for() {
    let with = |small: &str, large: &str| {
        SIZE_AXIS
            .replace("small: {covers: [a, beta]}", small)
            .replace("large: {covers: [c]}", large)
    };
    let d = coarse(
        &with(
            "small: {covers: [a, beta], read_for: [beta]}",
            "large: {covers: [c]}",
        ),
        SIZE_RULES,
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    let size = &pack.axes[pack.axis_index("size").unwrap()];
    // a value named by an identity it had before is the value
    assert_eq!(size.values[0].read_for, ["b"]);
    assert!(size.values[1].read_for.is_empty());
    let kind = &pack.axes[pack.axis_index("kind").unwrap()];
    assert!(kind.values.iter().all(|v| v.read_for.is_empty()));

    let refused = |small: &str, large: &str| refusal(&coarse(&with(small, large), SIZE_RULES));
    // no value of the finer axis
    let e = refused(
        "small: {covers: [a, beta], read_for: [d]}",
        "large: {covers: [c]}",
    );
    assert!(e.contains("size.yml"), "{e}");
    assert!(e.contains("values.small.read_for"), "{e}");
    assert!(e.contains("d is no value of kind"), "{e}");
    // a value another coarse value covers
    let e = refused(
        "small: {covers: [a, beta], read_for: [c]}",
        "large: {covers: [c]}",
    );
    assert!(e.contains("values.small.read_for"), "{e}");
    assert!(e.contains("small does not cover c"), "{e}");
    // listed twice, by two of its names
    let e = refused(
        "small: {covers: [a, beta], read_for: [b, beta]}",
        "large: {covers: [c]}",
    );
    assert!(e.contains("b is listed twice"), "{e}");
    // a value that covers one thing is no wider than it
    let e = refused(
        "small: {covers: [a, beta]}",
        "large: {covers: [c], read_for: [c]}",
    );
    assert!(e.contains("values.large.read_for"), "{e}");
    assert!(e.contains("large covers c alone"), "{e}");
    // and on an axis that is the coarse form of none
    let d = coarse(
        "axis: size\nkind: single\nvalues: {small: {read_for: [a]}, large: {}}\n",
        SIZE_RULES,
    );
    let e = refusal(&d);
    assert!(e.contains("values.small.read_for"), "{e}");
    assert!(e.contains("size is the coarse form of no axis"), "{e}");
}

/// The shipped pack reads a brain written alone as the head, and no other
/// value as anything.
#[test]
fn the_shipped_pack_reads_a_brain_written_alone_as_the_head() {
    let pack = nils_pack::load(&common::shipped_mri(), None).unwrap();
    for axis in &pack.axes {
        for v in &axis.values {
            let expected: &[&str] = if axis.name == "body_region" && v.id == "head" {
                &["brain"]
            } else {
                &[]
            };
            assert_eq!(v.read_for, expected, "{}.{}", axis.name, v.id);
        }
    }
}

/// An axis may say which of its values a question counts unless it says
/// otherwise (`ask_default`): values of the axis by any of their names,
/// kept by identity, at least one, each once.
#[test]
fn an_axis_may_say_which_values_a_question_counts_by_default() {
    let kind = |extra: &str| {
        let d = coarse(SIZE_AXIS, SIZE_RULES);
        d.file(
            "axes/kind.yml",
            &format!(
                "axis: kind\nkind: single\n{extra}values: {{a: {{}}, b: {{aliases: [beta]}}, c: {{}}}}\n"
            ),
        );
        d
    };
    let pack = nils_pack::load(kind("ask_default: [a, beta]\n").path(), None).unwrap();
    assert_eq!(pack.axis("kind").unwrap().ask_default, ["a", "b"]);
    assert!(pack.axis("size").unwrap().ask_default.is_empty());
    let pack = nils_pack::load(kind("").path(), None).unwrap();
    assert!(pack.axes.iter().all(|a| a.ask_default.is_empty()));

    let e = refusal(&kind("ask_default: [a, d]\n"));
    assert!(e.contains("kind.yml"), "{e}");
    assert!(e.contains("ask_default"), "{e}");
    assert!(e.contains("d is no value of kind"), "{e}");
    let e = refusal(&kind("ask_default: [b, beta]\n"));
    assert!(e.contains("b is listed twice"), "{e}");
    let e = refusal(&kind("ask_default: []\n"));
    assert!(e.contains("names no value"), "{e}");
}

/// The shipped pack counts the acquisitions and the scanner-derived images
/// unless a question says otherwise, on its disposition and on no other
/// axis, and ships no family file of its own yet.
#[test]
fn the_shipped_pack_counts_acquisitions_and_scanner_derived_images_by_default() {
    let pack = nils_pack::load(&common::shipped_mri(), None).unwrap();
    for axis in &pack.axes {
        let expected: &[&str] = if axis.name == "disposition" {
            &["acquisition", "scanner_derived"]
        } else {
            &[]
        };
        assert_eq!(axis.ask_default, expected, "{}", axis.name);
    }
}

const FAMILY: &str = "\
family: two-echo
label: 'Two-echo imaging'
terms: ['two echo', 'dual echo']
summary: 'Two images of one excitation, an early echo and a late one.'
products:
  - id: first-echo
    what: 'The early echo.'
    acquired: true
    where:
      - ['=', {}, ['axis', {}, 'kind'], 'a']
      - ['not', {}, ['in', {}, ['axis', {}, 'size'], ['large']]]
    note: 'Makers name it differently.'
  - id: difference
    what: 'The late echo taken from the early one.'
    acquired: false
    where:
      - ['=', {}, ['axis', {}, 'kind'], 'beta']
ask:
  - words: 'the early echo of each visit'
    sets: |
      early:
        grain: stack
        of: visits
        where:
          - ['=', {}, ['axis', {}, 'kind'], 'a']
    note: 'The visit is the session.'
raw:
  - field: text_series_description
    what: 'The maker words for the echo.'
";

/// A pack with the two axes of [`coarse`] and the family files given.
fn with_families(files: &[(&str, &str)]) -> Dir {
    let d = coarse(SIZE_AXIS, SIZE_RULES);
    for (name, text) in files {
        d.file(&format!("families/{name}"), text);
    }
    d
}

/// A pack may say how an acquisition family is filed, a file a family in a
/// `families` folder the manifest does not name: each is read whole, in the
/// order of the files' names, and a pack without the folder has none.
#[test]
fn a_pack_may_say_how_a_family_is_filed() {
    let other = FAMILY
        .replace("family: two-echo", "family: a-map")
        .replace("Two-echo imaging", "A map");
    let d = with_families(&[("two-echo.yml", FAMILY), ("a-map.yml", &other)]);
    let pack = nils_pack::load(d.path(), None).unwrap();
    let names: Vec<&str> = pack.families.iter().map(|f| f.family.as_str()).collect();
    assert_eq!(names, ["a-map", "two-echo"]);
    let f = &pack.families[1];
    assert_eq!(f.label, "Two-echo imaging");
    assert_eq!(f.terms, ["two echo", "dual echo"]);
    assert_eq!(f.products.len(), 2);
    assert_eq!(f.products[0].id, "first-echo");
    assert!(f.products[0].acquired && !f.products[1].acquired);
    assert_eq!(
        f.products[0].where_[0],
        serde_json::json!(["=", {}, ["axis", {}, "kind"], "a"])
    );
    assert_eq!(f.products[1].note, None);
    assert_eq!(f.ask[0].words, "the early echo of each visit");
    assert!(f.ask[0].sets.starts_with("early:\n  grain: stack\n"));
    assert_eq!(f.raw[0].field, "text_series_description");
    // as the catalog serves it: the clauses as arrays, the sets as text
    let served = serde_json::to_value(f).unwrap();
    assert_eq!(served["products"][0]["where"][0][3], "a");
    assert!(served["products"][1].get("note").is_none());
    assert!(served["ask"][0]["sets"].is_string());
    // no folder, no family; and a file that is no YAML file is not read
    let none = coarse(SIZE_AXIS, SIZE_RULES);
    assert!(
        nils_pack::load(none.path(), None)
            .unwrap()
            .families
            .is_empty()
    );
    let d = with_families(&[("two-echo.yml", FAMILY), ("README.md", "not a family")]);
    assert_eq!(nils_pack::load(d.path(), None).unwrap().families.len(), 1);
    // a family without `raw` serves none
    let bare = FAMILY.split("raw:").next().unwrap().to_string();
    let d = with_families(&[("two-echo.yml", &bare)]);
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert!(pack.families[0].raw.is_empty());
    assert!(
        serde_json::to_value(&pack.families[0])
            .unwrap()
            .get("raw")
            .is_none()
    );
}

/// What the loader holds a family file to, each refusal with the file and
/// the path inside it.
#[test]
fn a_family_file_is_held_to_its_keys_its_names_and_the_pack_s_axes() {
    let refused = |from: &str, to: &str, path: &str, why: &str| {
        let text = FAMILY.replace(from, to);
        assert_ne!(text, FAMILY, "{from}");
        let e = refusal(&with_families(&[("two-echo.yml", &text)]));
        assert!(e.contains("two-echo.yml"), "{from}: {e}");
        assert!(e.contains(path), "{from}: {e}");
        assert!(e.contains(why), "{from}: {e}");
    };
    // every key is known, at each depth
    refused(
        "summary:",
        "colour: red\nsummary:",
        "colour",
        "colour is not a key here",
    );
    refused(
        "    acquired: true\n",
        "    acquired: true\n    weight: 3\n",
        "products[0].weight",
        "weight is not a key here",
    );
    refused(
        "    note: 'The visit is the session.'\n",
        "    answer: 4\n",
        "ask[0].answer",
        "answer is not a key here",
    );
    refused(
        "  - field: text_series_description\n",
        "  - field: text_series_description\n    type: text\n",
        "raw[0].type",
        "type is not a key here",
    );
    // the family is named as its file, in lower case letters, digits and
    // hyphens, and so are its products, each once
    refused(
        "family: two-echo",
        "family: two_echo",
        "family",
        "the family is two_echo and its file is two-echo",
    );
    refused(
        "  - id: first-echo",
        "  - id: First Echo",
        "products[0].id",
        "First Echo is not lower case letters, digits and hyphens",
    );
    refused(
        "  - id: difference",
        "  - id: first-echo",
        "products[1].id",
        "first-echo is a product of two-echo already",
    );
    // the words a person uses, and what the file must hold
    refused(
        "terms: ['two echo', 'dual echo']",
        "terms: []",
        "terms",
        "at least one word",
    );
    refused("label: 'Two-echo imaging'\n", "", "", "needs label");
    refused(
        "    acquired: true\n",
        "    acquired: 'yes'\n",
        "products[0].acquired",
        "expected true",
    );
    refused(
        "    what: 'The early echo.'\n",
        "    what: ''\n",
        "products[0].what",
        "is empty",
    );
    // a clause is a clause, of an axis the pack declares and a value of it
    refused(
        "      - ['=', {}, ['axis', {}, 'kind'], 'a']\n      - ['not'",
        "      - ['=', ['axis', {}, 'kind'], 'a']\n      - ['not'",
        "products[0].where[0]",
        "a clause is [op, {options}, ...arguments]",
    );
    refused(
        "['in', {}, ['axis', {}, 'size'], ['large']]",
        "['in', {}, ['axis', {}, 'weight'], ['large']]",
        "products[0].where[1][2][2]",
        "no axis named weight in the pack",
    );
    refused(
        "['in', {}, ['axis', {}, 'size'], ['large']]",
        "['in', {}, ['axis', {}, 'size'], ['large', 'huge']]",
        "products[0].where[1][2]",
        "huge is no value of size",
    );
    // a value by an earlier name is a value
    assert!(FAMILY.contains("'beta'"));
    // no clause of a family says `exact`, in a product or in an example,
    // at any depth
    refused(
        "    where:\n      - ['=', {}, ['axis', {}, 'kind'], 'a']\n      - ['not'",
        "    where:\n      - ['=', {exact: true}, ['axis', {}, 'kind'], 'a']\n      - ['not'",
        "products[0].where[0]",
        "a family's clause carries no exact",
    );
    refused(
        "['in', {}, ['axis', {}, 'size'], ['large']]",
        "['in', {exact: false}, ['axis', {}, 'size'], ['large']]",
        "products[0].where[1][2]",
        "a family's clause carries no exact",
    );
    refused(
        "          - ['=', {}, ['axis', {}, 'kind'], 'a']\n    note: 'The visit",
        "          - ['=', {exact: true}, ['axis', {}, 'kind'], 'a']\n    note: 'The visit",
        "ask[0].sets.early.where[0]",
        "a family's clause carries no exact",
    );
    // the sets of an example are YAML that maps a name to a set, their
    // clauses held as a product's
    refused(
        "    sets: |\n      early:\n        grain: stack\n        of: visits\n        where:\n          - ['=', {}, ['axis', {}, 'kind'], 'a']\n",
        "    sets: |\n      - early\n",
        "ask[0].sets",
        "the sets map each set's name to the set",
    );
    refused(
        "          - ['=', {}, ['axis', {}, 'kind'], 'a']\n    note: 'The visit",
        "          - ['=', {}, ['axis', {}, 'kind'], 'z']\n    note: 'The visit",
        "ask[0].sets.early.where[0]",
        "z is no value of kind",
    );
    // and a family has a product and an example
    let e = refusal(&with_families(&[(
        "two-echo.yml",
        "family: two-echo\nlabel: l\nterms: [t]\nsummary: s\nproducts: []\nask: []\n",
    )]));
    assert!(e.contains("a family leaves at least one product"), "{e}");
}

/// The shipped pack loads with a `families` folder beside its manifest:
/// one invented family, written in the pack's own axes, in a copy of the
/// pack.
#[test]
fn the_shipped_pack_loads_with_a_family_beside_it() {
    let d = Dir::mri_copy("mri-family");
    d.file(
        "families/invented-pair.yml",
        "\
family: invented-pair
label: 'An invented pair'
terms: ['invented pair']
summary: 'A magnitude and a phase of one acquisition, for a test.'
products:
  - id: magnitude
    what: 'The magnitude image.'
    acquired: true
    where:
      - ['=', {}, ['axis', {}, 'base'], 'T2starw']
      - ['=', {}, ['axis', {}, 'disposition'], 'acquisition']
      - ['not', {}, ['has', {}, ['axis', {}, 'construct'], 'Phase']]
ask:
  - words: 'the magnitude of each visit'
    sets: |
      magnitude:
        grain: stack
        where:
          - ['=', {}, ['axis', {}, 'base'], 'T2starw']
",
    );
    let pack = nils_pack::load(d.path(), None).unwrap_or_else(|e| panic!("{e}"));
    let shipped = nils_pack::load(&common::shipped_mri(), None).unwrap();
    assert_eq!(pack.families.len(), shipped.families.len() + 1);
    assert!(pack.families.iter().any(|f| f.family == "invented-pair"));
    assert!(!shipped.families.iter().any(|f| f.family == "invented-pair"));
}

/// The families the pack ships: each loads, in the order of the files'
/// names, with at least a word, a product and a worked example, and its
/// file carries the licence line and no clause that says `exact`.
#[test]
fn the_shipped_pack_s_families_load() {
    let dir = common::shipped_mri();
    let pack = nils_pack::load(&dir, None).unwrap_or_else(|e| panic!("{e}"));
    assert!(!pack.families.is_empty());
    let names: Vec<&str> = pack.families.iter().map(|f| f.family.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    for f in &pack.families {
        assert!(
            !f.terms.is_empty() && !f.products.is_empty() && !f.ask.is_empty(),
            "{}",
            f.family
        );
        let text = std::fs::read_to_string(dir.join("families").join(format!("{}.yml", f.family)))
            .unwrap();
        assert!(
            text.starts_with("# SPDX-License-Identifier: AGPL-3.0-only\n"),
            "{}",
            f.family
        );
        assert!(!text.contains("exact"), "{}", f.family);
        // the words of a family are no value's stored spelling by accident:
        // each is a text, trimmed
        for t in &f.terms {
            assert_eq!(t.trim(), t, "{}: {t:?}", f.family);
        }
    }
    // a family's identity is its own
    let mut ids: Vec<&str> = names.clone();
    ids.dedup();
    assert_eq!(ids.len(), names.len());
}

#[test]
fn a_coarse_form_the_rules_fold_otherwise_is_refused() {
    // the declaration says b is small, and the rules decide large for it
    let rules = SIZE_RULES
        .replace(
            "{any: [{axis: kind, is: a}, {axis: kind, is: b}]}",
            "{axis: kind, is: a}",
        )
        .replace(
            "{when: {axis: kind, is: c}",
            "{when: {any: [{axis: kind, is: c}, {axis: kind, is: b}]}",
        );
    let e = refusal(&coarse(SIZE_AXIS, &rules));
    assert!(e.contains("axis size.values"), "{e}");
    assert!(
        e.contains("covers says a kind of b is small of size, and the rules decide large"),
        "{e}"
    );

    // a value the declaration leaves out and the rules fold
    let e = refusal(&coarse(&SIZE_AXIS.replace("[a, beta]", "[a]"), SIZE_RULES));
    assert!(
        e.contains("covers says a kind of b is nothing of size, and the rules decide small"),
        "{e}"
    );

    // a value the declaration covers and no rule folds
    let rules = SIZE_RULES.replace(
        "{any: [{axis: kind, is: a}, {axis: kind, is: b}]}",
        "{axis: kind, is: a}",
    );
    let e = refusal(&coarse(SIZE_AXIS, &rules));
    assert!(
        e.contains("covers says a kind of b is small of size, and the rules decide nothing"),
        "{e}"
    );
}

#[test]
fn a_coarse_form_the_rules_read_from_the_file_is_refused() {
    // the size is read off the image type, not folded from the kind: the
    // refusal names the rule and what it reads, and is not the comparison
    // of the values, which a flag that never holds on an empty stack
    // would fail for another reason
    let rules = "\
rule_set: size
decides: [size]
tiers: {stated: 0.9}
order: [small]
rules:
  small:
    clauses: [{flag: is_original, tier: stated}]
    set: {size: small}
";
    let e = refusal(&coarse(SIZE_AXIS, rules));
    assert!(e.contains("axis size.coarse_of"), "{e}");
    assert!(
        e.contains("the rules do not decide size from kind alone"),
        "{e}"
    );
    assert!(!e.contains("covers says"), "{e}");

    // the two folding rules, each as the declaration says, and one rule
    // tried before them that reads a flag: every value folds as declared
    // over a stack that says nothing, and an original image of kind a
    // would still be sorted large against `covers`
    let rules = SIZE_RULES
        .replace("order: [small, large]", "order: [original, small, large]")
        .replace(
            "rules:\n",
            "rules:\n  original:\n    clauses: [{flag: is_original, tier: stated}]\n    set: {size: large}\n",
        );
    assert!(rules.contains("original:"), "{rules}");
    let e = refusal(&coarse(SIZE_AXIS, &rules));
    assert!(e.contains("axis size.coarse_of"), "{e}");
    assert!(
        e.contains(
            "the rules do not decide size from kind alone: size/original reads the flag is_original"
        ),
        "{e}"
    );
}

/// A pack whose `kind` axis stores labels and has every sort of name: an
/// identity, an identity it had before, a label, and the spelling rows kept
/// when the label changed; and whose `disposition` stores identities, one
/// of them with an earlier spelling.
fn spelled() -> nils_pack::Pack {
    let d = phased();
    d.file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nstores: label\nvalues:\n  a: {label: A1, stored: OLD-A, aliases: [was-a]}\n  b: {label: B}\n",
    )
    .file(
        "axes/disposition.yml",
        "axis: disposition\nphase: disposition\nkind: single\nvalues: {keep: {aliases: ['1']}, drop: {aliases: ['0']}}\n",
    );
    nils_pack::load(d.path(), None).unwrap()
}

#[test]
fn every_name_of_a_value_reads_as_the_value_and_one_is_stored() {
    let pack = spelled();
    let kind = pack.axis("kind").unwrap();
    // identity, an earlier identity, the label and the stored spelling
    for name in ["a", "was-a", "A1", "OLD-A"] {
        assert_eq!(kind.id_of(name), Some("a"), "{name}");
        assert_eq!(kind.stored_of(name), Some("OLD-A"), "{name}");
    }
    for name in ["b", "B"] {
        assert_eq!(kind.id_of(name), Some("b"), "{name}");
        assert_eq!(kind.stored_of(name), Some("B"), "{name}");
    }
    assert_eq!(kind.id_of("neither"), None);
    assert_eq!(kind.stored_of("neither"), None);
    // an axis that stores identities reads a spelling it stored before
    let disposition = pack.axis("disposition").unwrap();
    assert_eq!(disposition.stored_of("1"), Some("keep"));
    assert_eq!(disposition.id_of("0"), Some("drop"));
    assert_eq!(disposition.words_of_stored("1"), "keep");
    // what a person reads is the label, whichever name the row holds
    assert_eq!(kind.words_of_stored("a"), "A1");
    assert_eq!(kind.words_of_stored("OLD-A"), "A1");
    assert_eq!(kind.label_of_stored("was-a"), "A1");
}

#[test]
fn a_text_is_written_in_the_stored_form_and_compared_as_values() {
    let pack = spelled();
    let kind = pack.axis("kind").unwrap();
    // each comma-joined value, a value named twice once, an unknown kept
    assert_eq!(kind.stored_text("A1"), "OLD-A");
    assert_eq!(kind.stored_text("b, was-a, a"), "B,OLD-A");
    assert_eq!(kind.stored_text("neither,b"), "neither,B");
    assert_eq!(kind.stored_text(""), "");
    assert!(kind.same("a", "OLD-A"));
    assert!(kind.same("B,a", "was-a, b"));
    assert!(!kind.same("a", "b"));
    assert!(!kind.same("a", ""));
    // by the pack, for a caller that has an axis's name
    assert_eq!(pack.stored_text("kind", "a"), "OLD-A");
    assert_eq!(pack.stored_text("no_such_axis", "a"), "a");
    assert_eq!(pack.id_of("kind", "A1"), Some("a"));
    assert_eq!(pack.id_of("kind", "neither"), None);
}

#[test]
fn the_other_spellings_of_a_pack_are_listed_for_a_store_that_holds_none() {
    let pack = spelled();
    let kind = pack.axis("kind").unwrap();
    let mut other = kind.spellings();
    other.sort();
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        other,
        vec![
            pair("A1", "OLD-A"),
            pair("a", "OLD-A"),
            pair("b", "B"),
            pair("was-a", "OLD-A"),
        ]
    );
    let names = pack.names();
    let of = |axis: &str, text: &str| {
        names
            .iter()
            .find(|(a, t, _, _)| a == axis && t == text)
            .map(|(_, _, stored, id)| (stored.as_str(), id.as_str()))
    };
    assert_eq!(of("kind", "OLD-A"), Some(("OLD-A", "a")));
    assert_eq!(of("kind", "was-a"), Some(("OLD-A", "a")));
    assert_eq!(of("disposition", "1"), Some(("keep", "keep")));
    assert_eq!(of("disposition", "keep"), Some(("keep", "keep")));
    assert_eq!(of("kind", "neither"), None);
}

/// One spelling of a value in a row: a name belongs to one value of an axis,
/// whichever kind of name it is, and a pack that shares one is refused with
/// both values named.
#[test]
fn a_name_shared_by_two_values_of_an_axis_is_refused_naming_both() {
    let cases = [
        // an alias that is another value's label
        (
            "stores: label\nvalues: {a: {label: X}, b: {aliases: [X]}}",
            "X is a name of both a and b",
        ),
        // a label that is another value's identity
        (
            "stores: label\nvalues: {a: {label: b}, b: {}}",
            "b is a name of both a and b",
        ),
        // an alias that is another value's identity
        (
            "values: {a: {}, b: {aliases: [a]}}",
            "already names another value",
        ),
        // a stored word that is another value's identity
        (
            "stores: label\nvalues: {a: {stored: b}, b: {}}",
            "already names another value",
        ),
        // two labels alike
        (
            "stores: label\nvalues: {a: {label: X}, b: {label: X}}",
            "X is a name of both a and b",
        ),
        // an alias two values have
        ("values: {a: {aliases: [Z]}, b: {aliases: [Z]}}", "Z"),
    ];
    for (body, want) in cases {
        let d = phased();
        d.file(
            "axes/kind.yml",
            &format!("axis: kind\nkind: single\n{body}\n"),
        );
        let e = refusal(&d);
        assert!(e.contains(want), "{body}: {e}");
        assert!(e.contains("axes/kind.yml"), "{body}: {e}");
    }
    // distinct names load, a label equal to its own value's identity among them
    let d = phased();
    d.file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nstores: label\nvalues: {a: {label: a, aliases: [old]}, b: {label: B}}\n",
    );
    nils_pack::load(d.path(), None).expect("distinct names load");
}
