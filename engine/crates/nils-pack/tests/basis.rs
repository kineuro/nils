// SPDX-License-Identifier: AGPL-3.0-only

//! The sequence research of 2026-09-28, item 8: every axis verdict and every
//! evidence row says whether the header or the name decided it, from the tier
//! it records.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-basis-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(p.join("axes")).unwrap();
        std::fs::create_dir_all(p.join("rules")).unwrap();
        std::fs::create_dir_all(p.join("corpus")).unwrap();
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

/// `kind` decided by a keyword in the series name, `agent` by a flag, and
/// `mood` left to its default.
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
axes: [axes/kind.yml, axes/agent.yml, axes/mood.yml]
rules: [rules/kind.yml, rules/agent.yml]
order: [kind, agent]
buckets:
  agents: [gd]
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
        "axes/agent.yml",
        "axis: agent\nkind: single\nvalues: {yes: {}}\n",
    )
    .file(
        "axes/mood.yml",
        "axis: mood\nkind: single\nvalues: {x: {}}\ndefault: x\n",
    )
    .file(
        "rules/kind.yml",
        "\
rule_set: kind
decides: [kind]
tiers: {keywords: 0.85}
order: [alpha]
rules:
  alpha:
    clauses: [{keywords: [alpha], tier: keywords, field: text_series_description}]
    set: {kind: a}
",
    )
    .file(
        "rules/agent.yml",
        "\
rule_set: agent
decides: [agent]
tiers: {exclusive: 0.95}
order: [given]
rules:
  given:
    clauses: [{flag: has_agent, tier: exclusive}]
    set: {agent: 'yes'}
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

#[test]
fn a_keyword_says_name_a_flag_says_header_and_a_default_says_default() {
    let d = pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    let mut s = nils_pack::Stack::new();
    s.set(
        "text_series_description",
        nils_pack::stack::Value::Text(Some("alpha")),
    )
    .unwrap();
    s.set("text_contrast", nils_pack::stack::Value::Text(Some("gd")))
        .unwrap();
    let v = nils_pack::Evaluated::new(&pack, &s).classify();
    let axis = |name: &str| {
        v.axes
            .iter()
            .find(|a| a.axis == name)
            .unwrap_or_else(|| panic!("{name} in {:?}", v.axes))
    };
    assert_eq!(
        (axis("kind").tier.as_str(), axis("kind").basis.as_str()),
        ("keywords", "name")
    );
    assert_eq!(
        (axis("agent").tier.as_str(), axis("agent").basis.as_str()),
        ("exclusive", "header")
    );
    assert_eq!(
        (axis("mood").tier.as_str(), axis("mood").basis.as_str()),
        ("default", "default")
    );
    for e in &v.evidence {
        assert_eq!(e.basis, nils_pack::basis_of(&e.tier), "{e:?}");
    }
    let json = serde_json::to_value(&v.axes).unwrap();
    assert_eq!(
        json[0]["basis"],
        axis(json[0]["axis"].as_str().unwrap()).basis
    );
}
