// SPDX-License-Identifier: AGPL-3.0-only

//! The modality seam (`docs/specs/wave2-fingerprint-and-classify.md`, §13,
//! slice 8).
//!
//! Not a second clinical pack, but the foundation one would stand on, tested
//! rather than asserted: a pack for a modality that does not exist, with two
//! axes of its own, a route, a vocabulary the engine has never heard of, and
//! not one line of Rust to make it work. If this passes, a CT pack and a PET
//! pack are vocabulary and a corpus, which is what the wave promised.

use std::fs;
use std::path::{Path, PathBuf};

use nils_pack::stack::Value;
use nils_pack::{Evaluated, Stack};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("nils-seam-{}-{n}", std::process::id()));
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

/// A pack for the imaginary modality XR: two axes, one route, one flag.
fn xr() -> Dir {
    let d = Dir::new();
    d.file(
        "pack.yml",
        "\
pack: xr
version: 0.1.0
contract: 1
modality: XR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/purpose.yml, axes/plane.yml]
rules: [rules/survey.yml]
order: [survey, purpose, plane]
review:
  low_confidence: {default: 0.8}
  missing: [purpose]
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
    // An axis of a vocabulary the engine has never seen.
    d.file(
        "axes/purpose.yml",
        "\
axis: purpose
kind: single
default: unstated
values:
  'diagnosis':
    keywords: [diagnostic, report]
  'planning':
    keywords: [planning, plan]
  'survey':
    detection:
      exclusive: is_survey
",
    );
    // And a multi-valued one, to prove the engine does not care how many
    // values an axis holds either.
    d.file(
        "axes/plane.yml",
        "\
axis: plane
kind: multi
values:
  'flat': {keywords: [flat, ap, pa]}
  'oblique': {keywords: [oblique, angled]}
",
    );
    // A route: a rule set with an entry condition, overriding the axes for
    // the stacks it claims.
    d.file(
        "rules/survey.yml",
        "\
rule_set: survey
decides: [purpose]
enter_when: is_final
order: [final_is_a_diagnosis]
rules:
  final_is_a_diagnosis:
    clauses:
      - {flag: is_final, tier: exclusive}
    set: {purpose: diagnosis}
    confidence: 0.99
    why: 'a final image was made to be read'
",
    );
    d.file(
        "corpus/cases.yml",
        "\
cases:
  - name: a survey image says what it is
    stack:
      image_type: 'ORIGINAL\\\\SURVEY'
      text_all: 'ap survey'
    flags: {is_survey: true}
    axes:
      purpose: survey
      plane: flat

  - name: and a final one is claimed by the route
    stack:
      image_type: 'DERIVED\\\\FINAL'
      text_all: 'oblique report'
    axes:
      purpose: diagnosis
      plane: oblique
",
    );
    d
}

fn stack(image_type: &str, text: &str) -> Stack {
    let mut s = Stack::new();
    s.set("image_type", Value::Text(Some(image_type))).unwrap();
    s.set("text_all", Value::Text(Some(text))).unwrap();
    s.set("modality", Value::Text(Some("XR"))).unwrap();
    s
}

#[test]
fn a_pack_for_another_modality_loads_and_decides_its_own_axes() {
    let d = xr();
    let pack = nils_pack::load(d.path(), None).expect("the XR pack loads");
    assert_eq!(pack.modality, "XR");
    assert_eq!(pack.cases, 2, "its own corpus judged it");
    assert_eq!(
        pack.axes
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["purpose", "plane"],
        "the axes are the pack's, and the engine has never heard of them"
    );

    let verdict = Evaluated::new(&pack, &stack("ORIGINAL\\SURVEY", "ap survey")).classify();
    assert_eq!(verdict.stored("purpose"), "survey");
    assert_eq!(verdict.stored("plane"), "flat");

    // the route claims the stacks its condition holds for, and overrides
    let verdict = Evaluated::new(&pack, &stack("DERIVED\\FINAL", "oblique report")).classify();
    assert_eq!(verdict.stored("purpose"), "diagnosis");
    assert_eq!(verdict.stored("plane"), "oblique");
    assert!(verdict.entered.iter().any(|r| r == "survey"));

    // and an axis nothing decided takes the pack's default
    let verdict = Evaluated::new(&pack, &stack("ORIGINAL\\PRIMARY", "nothing here")).classify();
    assert_eq!(verdict.stored("purpose"), "unstated");
    assert_eq!(verdict.stored("plane"), "");
}

#[test]
fn the_engine_carries_no_vocabulary_of_its_own() {
    // Every value this pack decides, and every axis it decides them on, is a
    // word the engine has never seen. If the engine held any of the MRI
    // pack's vocabulary, the two packs could not both be right.
    let d = xr();
    let pack = nils_pack::load(d.path(), None).expect("the XR pack loads");
    let mri = nils_pack::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri"),
        None,
    )
    .expect("the MRI pack loads");
    for a in &pack.axes {
        assert!(
            !mri.axes.iter().any(|m| m.name == a.name),
            "{} is in both packs, so one of them borrowed it",
            a.name
        );
    }
    assert_ne!(pack.modality, mri.modality);
    // and a pack that judges XR says nothing about an MR stack: the engine
    // records that as an outcome, which is what `no_pack` is for.
    assert_eq!(mri.modality, "MR");
}
