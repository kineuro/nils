// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: the evaluator reports what it noticed and did not act on.
//! The evidence rows cannot tell these apart from silence, which is why the
//! evaluator says so itself (spec §14, answered here).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-diag-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(p.join("corpus")).unwrap();
        std::fs::create_dir_all(p.join("axes")).unwrap();
        std::fs::create_dir_all(p.join("rules")).unwrap();
        Dir(p)
    }
    fn file(&self, name: &str, body: &str) -> &Dir {
        std::fs::write(self.0.join(name), body).unwrap();
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

/// Two rule sets over one axis, in order, and a second axis nobody decides.
fn pack() -> Dir {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/mood.yml]
rules: [rules/first.yml, rules/second.yml]
order: [first, second]
buckets:
  agents: [gd, dotarem]
",
    )
    .file(
        "parsers.yml",
        "\
parsers:
  contrast:
    field: text_contrast
    case: lower
    tokenize: {split: '\\s+'}
    predicates:
      has_agent: {any_token: {bucket: agents}}
",
    )
    .file("flags.yml", "flags:\n  has_agent: contrast.has_agent\n")
    .file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues: {a: {}, b: {}}\n",
    )
    .file(
        "axes/mood.yml",
        "axis: mood\nkind: single\nvalues: {x: {}, y: {}}\n",
    )
    .file(
        "rules/first.yml",
        "\
rule_set: first
decides: [kind]
tiers: {keywords: 0.85}
order: [alpha, beta]
rules:
  alpha:
    clauses: [{keywords: [alpha, alfa], tier: keywords, field: text_series_description}]
    set: {kind: a}
  beta:
    clauses: [{keywords: [beta], tier: keywords, field: text_series_description}]
    set: {kind: b}
",
    )
    .file(
        "rules/second.yml",
        "\
rule_set: second
decides: [kind]
tiers: {keywords: 0.85}
order: [gamma, delta]
rules:
  gamma:
    clauses: [{keywords: [gamma], tier: keywords, field: text_series_description}]
    set: {kind: b}
  delta:
    clauses: [{keywords: [delta], tier: keywords, field: text_series_description}]
    set: {kind: a}
",
    )
    .file(
        "corpus/cases.yml",
        "\
cases:
  - name: alpha is a
    stack: {text_series_description: 'alpha'}
    axes: {kind: a}
",
    );
    d
}

fn verdict(pack: &nils_pack::Pack, text: &str) -> nils_pack::Verdict {
    let mut s = nils_pack::Stack::new();
    s.set(
        "text_series_description",
        nils_pack::stack::Value::Text(Some(text)),
    )
    .unwrap();
    nils_pack::Evaluated::new(pack, &s).classify()
}

fn kinds(v: &nils_pack::Verdict) -> Vec<&str> {
    v.diagnostics.iter().map(|d| d.kind.as_str()).collect()
}

#[test]
fn a_later_set_that_disagrees_is_a_conflict_and_both_are_recorded() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "alpha beta gamma");
    assert_eq!(v.stored("kind"), "a", "the order decided, as before");
    let conflict = v
        .diagnostics
        .iter()
        .find(|d| d.kind == "axis_conflict")
        .expect("gamma reached kind after first closed it");
    assert_eq!(conflict.axis, "kind");
    assert_eq!(
        (conflict.rule_set.as_str(), conflict.rule.as_str()),
        ("second", "gamma")
    );
    assert_eq!(conflict.value, "b");
    assert_eq!(conflict.matched, "gamma");
    assert_eq!(
        (conflict.by_rule_set.as_str(), conflict.by_rule.as_str()),
        ("first", "alpha")
    );
    assert_eq!(conflict.by_value, "a");
    assert_eq!(conflict.by_matched, "alpha");
    // beta would have fired in the same set and never ran: its keyword is
    // shadowed by alpha.
    let shadowed: Vec<_> = v
        .diagnostics
        .iter()
        .filter(|d| d.kind == "keyword_shadowed")
        .collect();
    assert_eq!(shadowed.len(), 1, "{:?}", v.diagnostics);
    assert_eq!(shadowed[0].matched, "beta");
    assert_eq!(shadowed[0].rule, "beta");
    assert_eq!(shadowed[0].by_rule, "alpha");
    assert_eq!(shadowed[0].by_matched, "alpha");
    // and the axis nobody decides, with no default, is unresolved
    assert!(
        v.diagnostics
            .iter()
            .any(|d| d.kind == "axis_unresolved" && d.axis == "mood"),
        "{:?}",
        v.diagnostics
    );
    assert!(!kinds(&v).contains(&"axis_unresolved_kind"));
}

#[test]
fn a_later_set_that_agrees_is_not_a_conflict() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "alpha delta");
    assert_eq!(v.stored("kind"), "a");
    assert_eq!(kinds(&v), vec!["axis_unresolved"], "{:?}", v.diagnostics);
}

#[test]
fn a_second_keyword_of_the_winning_list_is_shadowed_by_the_first() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "alfa alpha");
    // v0 cites the first in the list, not the first in the text
    assert_eq!(v.evidence[0].matched, "alpha");
    let shadowed: Vec<_> = v
        .diagnostics
        .iter()
        .filter(|d| d.kind == "keyword_shadowed")
        .collect();
    assert_eq!(shadowed.len(), 1, "{:?}", v.diagnostics);
    assert_eq!(shadowed[0].matched, "alfa");
    assert_eq!(
        shadowed[0].rule, "alpha",
        "hidden by its own rule's earlier keyword"
    );
    assert_eq!(shadowed[0].by_rule, "alpha");
}

#[test]
fn a_stack_nothing_matches_is_only_unresolved_and_the_verdict_is_unchanged() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "nothing here");
    assert_eq!(v.stored("kind"), "");
    let mut k = kinds(&v);
    k.sort();
    assert_eq!(
        k,
        vec!["axis_unresolved", "axis_unresolved"],
        "{:?}",
        v.diagnostics
    );
}

#[test]
fn an_overlay_parsed_from_text_carries_its_added_terms() {
    let d = pack();
    let text = "\
overlay: site
version: 1.0.0
pack: t
scope: {station: MR1}
buckets:
  agents: {add: [clariscan, ' -gd'], remove: [dotarem]}
cases:
  - name: the site's agent
    stack: {text_contrast: 'clariscan dose 15'}
    flags: {has_agent: true}
";
    let o = nils_pack::Overlay::parse("overlay", text).unwrap();
    assert_eq!(o.id, "site@1.0.0");
    assert_eq!(o.added_terms(), vec!["clariscan", " -gd"]);
    let (pack, failures) = nils_pack::load_judged(d.path(), Some(&o)).unwrap();
    assert!(failures.is_none(), "{failures:?}");
    assert_eq!(pack.overlay_terms, vec!["clariscan", " -gd"]);
    // and a failing case is answered, not refused
    let bad = text.replace("flags: {has_agent: true}", "flags: {has_agent: false}");
    let o = nils_pack::Overlay::parse("overlay", &bad).unwrap();
    let (_, failures) = nils_pack::load_judged(d.path(), Some(&o)).unwrap();
    let f = failures.expect("the case does not hold").to_string();
    assert!(f.contains("1 of 1 case assertions do not hold"), "{f}");
    assert!(
        nils_pack::load(d.path(), Some(&o)).is_err(),
        "load still refuses"
    );
}
