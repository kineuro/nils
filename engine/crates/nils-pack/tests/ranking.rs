// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3 (Nima's ruling of 2026-10-09): the pack decides. Rank is the
//! pack's order (its rule sets' order, a deciding set's rule order, an
//! exclusion group's priority), so one rule decided over another is
//! evidence and never a question, and two answers nothing in that order
//! ranks are a defect of the pack. These are the facts the evaluator hands
//! the classifier for both, and for the axes no rule answered.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-rank-test-{}-{n}", std::process::id()));
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

/// A pack whose rules decide over each other in every way the pack can
/// rank, and in the ways it cannot.
fn pack() -> Dir {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 1
modality: MR
axes: [axes/kind.yml, axes/mods.yml, axes/pair.yml, axes/both.yml, axes/mood.yml, axes/gone.yml]
rules: [rules/first.yml, rules/second.yml, rules/mods.yml, rules/pair.yml, rules/both.yml, rules/gone.yml]
order: [first, second, mods, pair, both, gone]
",
    )
    .file(
        "axes/kind.yml",
        "axis: kind\nkind: single\nvalues: {a: {}, b: {}}\n",
    )
    .file(
        "axes/mods.yml",
        "axis: mods\nkind: multi\nvalues: {p: {group: G, priority: 1}, q: {group: G, priority: 1}, r: {group: G, priority: 2}, s: {}}\n",
    )
    .file(
        "axes/pair.yml",
        "axis: pair\nkind: single\nvalues: {left: {}, right: {}}\n",
    )
    .file(
        "axes/both.yml",
        "axis: both\nkind: single\nvalues: {u: {}, v: {}}\n",
    )
    .file(
        "axes/mood.yml",
        "axis: mood\nkind: single\nvalues: {calm: {}}\n",
    )
    .file(
        "axes/gone.yml",
        "axis: gone\nkind: single\nvalues: {here: {}}\n",
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
    clauses: [{keywords: [alpha], tier: keywords, field: text_series_description}]
    set: {kind: a}
  beta:
    clauses: [{keywords: [beta], tier: keywords, field: text_series_description}]
    set: {kind: b}
    confidence: 0.6
",
    )
    .file(
        "rules/second.yml",
        "\
rule_set: second
decides: [kind]
tiers: {exclusive: 0.95}
order: [gamma]
rules:
  gamma:
    clauses: [{keywords: [gamma], tier: exclusive, field: text_series_description}]
    set: {kind: b}
",
    )
    .file(
        "rules/mods.yml",
        "\
rule_set: mods
decides: [mods]
collect: true
tiers: {keywords: 0.85}
order: [rp, rq, rr]
rules:
  rp:
    clauses: [{keywords: [pee], tier: keywords, field: text_series_description}]
    set: {mods: p}
  rq:
    clauses: [{keywords: [queue], tier: keywords, field: text_series_description}]
    set: {mods: q}
  rr:
    clauses: [{keywords: [are], tier: keywords, field: text_series_description}]
    set: {mods: r}
",
    )
    .file(
        "rules/pair.yml",
        "\
rule_set: pair
decides: [pair]
tiers: {keywords: 0.85}
order: [two]
rules:
  two:
    clauses: [{keywords: [twin], tier: keywords, field: text_series_description}]
    set: {pair: [{value: left, when: {text: text_series_description, substring: ex}}, {value: right, when: {text: text_series_description, substring: why}}]}
",
    )
    .file(
        "rules/both.yml",
        "\
rule_set: both
decides: [both]
collect: true
tiers: {keywords: 0.85}
order: [you, vee]
rules:
  you:
    clauses: [{keywords: [you], tier: keywords, field: text_series_description}]
    set: {both: u}
  vee:
    clauses: [{keywords: [vee], tier: keywords, field: text_series_description}]
    set: {both: v}
",
    )
    .file(
        "rules/gone.yml",
        "\
rule_set: gone
decides: [gone]
order: [none]
rules:
  none:
    clauses: [{keywords: [nothing], field: text_series_description}]
    set: {gone: null}
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

#[test]
fn a_rule_decided_over_another_is_evidence_with_what_ranked_it() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "alpha beta gamma");
    assert_eq!(v.stored("kind"), "a", "the order decided");
    // a later rule of the same set: the set's order ranked it
    let beta = v
        .overrides
        .iter()
        .find(|o| o.over.rule == "beta")
        .expect("beta was decided over");
    assert_eq!(beta.rank, "rule_order");
    assert_eq!((beta.value.as_str(), beta.other.as_str()), ("a", "b"));
    assert_eq!(
        (beta.by.rule_set.as_str(), beta.by.rule.as_str()),
        ("first", "alpha")
    );
    assert_eq!((beta.by.rule_set_at, beta.by.rule_at), (0, 0));
    assert_eq!((beta.over.rule_set_at, beta.over.rule_at), (0, 1));
    assert_eq!(beta.by.tier, "keywords");
    assert_eq!(beta.by.confidence, 0.85);
    assert_eq!(beta.over.confidence, 0.6, "the rule's own confidence");
    assert_eq!(
        (beta.by.matched.as_str(), beta.over.matched.as_str()),
        ("alpha", "beta")
    );
    // a later set, at a higher tier: the sets' order ranked it, the tier did
    // not
    let gamma = v
        .overrides
        .iter()
        .find(|o| o.over.rule == "gamma")
        .expect("gamma was decided over");
    assert_eq!(gamma.rank, "rule_set_order");
    assert_eq!(gamma.over.tier, "exclusive");
    assert_eq!(gamma.over.confidence, 0.95);
    assert_eq!((gamma.over.rule_set_at, gamma.over.rule_at), (1, 0));
    // the same overrides are still counted as diagnostics, per batch
    assert_eq!(
        v.diagnostics
            .iter()
            .filter(|d| d.kind == "axis_conflict")
            .count(),
        2,
        "{:?}",
        v.diagnostics
    );
    assert!(v.equal_rank.is_empty(), "{:?}", v.equal_rank);
}

#[test]
fn a_lower_priority_is_an_override_and_the_same_priority_a_defect() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "pee queue are");
    assert_eq!(
        v.stored("mods"),
        "p",
        "the first collected keeps a tie, as before"
    );
    let r = v
        .overrides
        .iter()
        .find(|o| o.axis == "mods")
        .expect("r lost on its priority");
    assert_eq!(
        (r.rank.as_str(), r.value.as_str(), r.other.as_str()),
        ("priority", "p", "r")
    );
    let tie = v
        .equal_rank
        .iter()
        .find(|e| e.axis == "mods")
        .expect("p and q tie");
    assert_eq!(tie.why, "priority_tie");
    assert_eq!(tie.kept, "p");
    assert_eq!(tie.pairs(), vec!["mods: mods/rp=p | mods/rq=q".to_string()]);
    // a lower priority alone is no defect
    let v = verdict(&pack, "pee are");
    assert!(v.equal_rank.is_empty(), "{:?}", v.equal_rank);
}

#[test]
fn two_values_on_an_axis_that_holds_one_are_a_defect_of_the_pack() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    // one rule whose two values' conditions both hold
    let v = verdict(&pack, "twin ex why");
    let one = v
        .equal_rank
        .iter()
        .find(|e| e.axis == "pair")
        .expect("left and right");
    assert_eq!(one.why, "one_rule");
    assert_eq!(
        one.kept, "left,right",
        "the stack keeps both, as it always did"
    );
    assert_eq!(
        one.pairs(),
        vec!["pair: pair/two=left | pair/two=right".to_string()]
    );
    // only one holds: no defect
    let v = verdict(&pack, "twin ex");
    assert_eq!(v.stored("pair"), "left");
    assert!(v.equal_rank.is_empty(), "{:?}", v.equal_rank);
    // a set that collects writing two values on an axis that holds one
    let v = verdict(&pack, "you vee");
    let both = v
        .equal_rank
        .iter()
        .find(|e| e.axis == "both")
        .expect("u and v");
    assert_eq!(both.why, "collected");
    assert_eq!(
        both.pairs(),
        vec!["both: both/vee=v | both/you=u".to_string()]
    );
}

#[test]
fn an_axis_no_rule_answered_is_unresolved_and_one_decided_to_nothing_is_not() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let v = verdict(&pack, "alpha nothing");
    assert!(
        v.unresolved.contains(&"mood".to_string()),
        "{:?}",
        v.unresolved
    );
    assert!(
        !v.unresolved.contains(&"mods".to_string()),
        "an empty multi-valued axis says none: {:?}",
        v.unresolved
    );
    assert!(
        !v.unresolved.contains(&"gone".to_string()),
        "a rule said nothing: an answer, {:?}",
        v.unresolved
    );
    assert!(!v.unresolved.contains(&"kind".to_string()));
    let v = verdict(&pack, "alpha");
    assert!(
        v.unresolved.contains(&"gone".to_string()),
        "{:?}",
        v.unresolved
    );
}

/// The axes that matter are the ones a release, a pick or a release's name
/// reads; a missing answer is asked on those that hold one value, have no
/// default and are not left to an image model.
#[test]
fn a_missing_answer_is_asked_only_where_the_axis_matters() {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "\
pack: m
version: 1.0.0
contract: 1
modality: MR
axes: [axes/base.yml, axes/body_part.yml, axes/technique.yml, axes/modifier.yml, axes/mood.yml]
review:
  by_model: [body_part]
",
    )
    .file(
        "axes/base.yml",
        "axis: base\nkind: single\nvalues: {T1w: {}}\n",
    )
    .file(
        "axes/body_part.yml",
        "axis: body_part\nkind: single\nvalues: {brain: {}}\n",
    )
    .file(
        "axes/technique.yml",
        "axis: technique\nkind: single\ndefault: Unknown\nvalues: {SE: {}}\n",
    )
    .file(
        "axes/modifier.yml",
        "axis: modifier\nkind: multi\nvalues: {FLAIR: {}}\n",
    )
    .file(
        "axes/mood.yml",
        "axis: mood\nkind: single\nvalues: {calm: {}}\n",
    )
    .file(
        "corpus/cases.yml",
        "cases:\n  - name: nothing named\n    stack: {text_series_description: 'x'}\n    axes: {technique: Unknown}\n",
    );
    let pack = nils_pack::load(d.path(), None).unwrap();
    let m = nils_pack::matters::of(&pack);
    for axis in ["base", "body_part", "technique", "modifier"] {
        assert!(m.contains(axis), "a release reads {axis}: {:?}", m.axes);
    }
    assert!(!m.contains("mood"), "nothing reads mood: {:?}", m.axes);
    assert_eq!(
        nils_pack::matters::missing_asked(&pack),
        vec!["base".to_string()]
    );
}

#[test]
fn an_image_model_s_axis_must_be_an_axis_of_the_pack() {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "pack: m\nversion: 1.0.0\ncontract: 1\nmodality: MR\naxes: [axes/base.yml]\nreview:\n  by_model: [body_part]\n",
    )
    .file(
        "axes/base.yml",
        "axis: base\nkind: single\nvalues: {T1w: {}}\n",
    )
    .file(
        "corpus/cases.yml",
        "cases:\n  - name: nothing named\n    stack: {text_series_description: 'x'}\n    axes: {}\n",
    );
    let err = match nils_pack::load(d.path(), None) {
        Ok(_) => panic!("a pack naming no such axis loaded"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("no axis named body_part"), "{err}");
}
