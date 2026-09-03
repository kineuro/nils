// SPDX-License-Identifier: AGPL-3.0-only

//! An overlay amends a pack's editable buckets at load, and is refused when
//! it reaches for anything else.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("nils-overlay-test-{}-{n}", std::process::id()));
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

/// A pack whose one predicate reads an editable bucket.
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
        "corpus/cases.yml",
        "\
cases:
  - name: a default agent is an agent
    stack: {text_contrast: 'dotarem dose 15'}
    flags: {has_agent: true}
  - name: a word the pack has not got is not
    stack: {text_contrast: 'clariscan dose 15'}
    flags: {has_agent: false}
",
    );
    d
}

fn overlay(d: &Dir, body: &str) -> PathBuf {
    let p = d.0.join("overlay.yml");
    std::fs::write(&p, body).unwrap();
    p
}

fn has_agent(pack: &nils_pack::Pack, text: &str) -> bool {
    let mut s = nils_pack::Stack::new();
    s.set("text_contrast", nils_pack::stack::Value::Text(Some(text)))
        .unwrap();
    nils_pack::Evaluated::new(pack, &s)
        .flag("has_agent")
        .unwrap()
}

#[test]
fn an_overlay_adds_a_site_word_without_touching_the_pack() {
    let d = pack();
    let o = overlay(
        &d,
        "\
overlay: karolinska
version: 1.0.0
pack: t
scope: {manufacturer: SIEMENS}
buckets:
  agents: {add: [clariscan]}
cases:
  - name: the site's own agent is an agent here
    stack: {text_contrast: 'clariscan dose 15'}
    flags: {has_agent: true}
",
    );
    let plain = nils_pack::load(d.path(), None).unwrap();
    assert!(!has_agent(&plain, "clariscan dose 15"));
    assert!(plain.overlay.is_none());

    let ov = nils_pack::Overlay::load(&o).unwrap();
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    assert!(has_agent(&amended, "clariscan dose 15"));
    assert!(has_agent(&amended, "dotarem dose 15"), "the defaults stay");
    assert_eq!(
        amended.overlay.as_deref(),
        Some("karolinska@1.0.0"),
        "the row has to be able to say it was judged under an overlay"
    );
    assert_eq!(
        amended.buckets["agents"],
        vec!["gd", "dotarem", "clariscan"],
        "the order is the order they were written"
    );
}

#[test]
fn an_overlay_removes_a_word_the_site_does_not_use() {
    let d = pack();
    let o = overlay(
        &d,
        "\
overlay: k
version: 1.0.0
pack: t
scope: {station: MR1}
buckets:
  agents: {remove: [DOTAREM]}
cases:
  - name: this site does not use dotarem
    stack: {text_contrast: 'dotarem dose 15'}
    flags: {has_agent: false}
",
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    assert!(!has_agent(&amended, "dotarem dose 15"));
    assert!(has_agent(&amended, "gd 15"), "the rest of the bucket stays");
    // The pack's own case still says dotarem is an agent, and that is fine:
    // it is the author's claim about the pack, and the site amended the pack.
    assert_eq!(
        amended.cases, 2,
        "the pack's cases still ran, against the pack"
    );
}

#[test]
fn an_overlay_for_another_pack_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: other\nscope: {model: X}\nbuckets:\n  agents: {add: [x]}\ncases:\n  - {name: c, stack: {text_contrast: x}, flags: {has_agent: true}}\n",
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let e = nils_pack::load(d.path(), Some(&ov))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("the overlay amends other, and this pack is t"),
        "{e}"
    );
}

#[test]
fn an_overlay_reaching_for_a_bucket_the_pack_keeps_closed_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nscope: {model: X}\nbuckets:\n  physics: {add: [x]}\ncases:\n  - {name: c, stack: {text_contrast: x}, flags: {has_agent: true}}\n",
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let e = nils_pack::load(d.path(), Some(&ov))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("does not open physics for editing"), "{e}");
    assert!(e.contains("it opens agents"), "{e}");
}

#[test]
fn an_overlay_scoped_by_a_selection_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nscope: {cohort: nmosd}\nbuckets:\n  agents: {add: [x]}\ncases:\n  - {name: c, stack: {text_contrast: x}, flags: {has_agent: true}}\n",
    );
    let e = nils_pack::Overlay::load(&o).err().unwrap().to_string();
    assert!(e.contains("scoped by provenance"), "{e}");
    assert!(e.contains("never by a selection"), "{e}");
}

#[test]
fn an_overlay_with_no_scope_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nbuckets:\n  agents: {add: [x]}\ncases:\n  - {name: c, stack: {text_contrast: x}, flags: {has_agent: true}}\n",
    );
    let e = nils_pack::Overlay::load(&o).err().unwrap().to_string();
    assert!(e.contains("amends everything"), "{e}");
}

#[test]
fn an_overlay_that_says_nothing_about_what_it_changed_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nscope: {model: X}\nbuckets:\n  agents: {add: [clariscan]}\n",
    );
    let e = nils_pack::Overlay::load(&o).err().unwrap().to_string();
    assert!(
        e.contains("ships the cases that show what it changed"),
        "{e}"
    );
}

#[test]
fn an_overlay_whose_own_cases_do_not_hold_is_refused() {
    let d = pack();
    let o = overlay(
        &d,
        "\
overlay: k
version: 1.0.0
pack: t
scope: {model: X}
buckets:
  agents: {add: [clariscan]}
cases:
  - name: a claim the overlay does not keep
    stack: {text_contrast: 'omniscan dose 10'}
    flags: {has_agent: true}
",
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let e = nils_pack::load(d.path(), Some(&ov))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("the overlay's cases"), "{e}");
    assert!(e.contains("a claim the overlay does not keep"), "{e}");
}
