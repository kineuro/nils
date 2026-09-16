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
    assert!(e.contains("scoped by origin"), "{e}");
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

// ---------------------------------------------------------------------------
// Pack contract 5 (record 26, decision 12): every axis value's word list is
// a site's to amend, named `lists.<axis>.<value>`; the flags, the windows and
// the order the values are tried in stay the pack's.

/// A pack with an axis of its own rules (`kind`), a vocabulary axis a
/// longhand set decides by words (`mood`), and one bucket.
fn axis_pack() -> Dir {
    let d = Dir::new();
    std::fs::create_dir_all(d.path().join("axes")).unwrap();
    std::fs::create_dir_all(d.path().join("rules")).unwrap();
    d.file(
        "pack.yml",
        "\
pack: t
version: 1.0.0
contract: 5
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/kind.yml, axes/mood.yml]
rules: [rules/mood.yml]
order: [kind, mood]
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
        "\
axis: kind
kind: single
stores: label
search: text_series_description
order: [a, b, w]
values:
  a:
    detection: {exclusive: has_agent}
  b:
    label: bee
    keywords: [beta, bee]
  w: {}
  c: {}
physics:
  - {value: w, when: {field: echo_time, gt: 100}, confidence: 0.6, why: a long echo}
",
    )
    .file(
        "axes/mood.yml",
        "axis: mood\nkind: single\nvalues: {x: {}, 'y': {}}\n",
    )
    .file(
        "rules/mood.yml",
        "\
rule_set: mood
decides: [mood]
tiers: {keywords: 0.85}
order: [ex, why]
rules:
  ex:
    clauses: [{keywords: [ex], tier: keywords, field: text_series_description}]
    set: {mood: x}
  why:
    clauses: [{flag: has_agent, tier: exclusive}]
    set: {mood: 'y'}
",
    )
    .file(
        "corpus/cases.yml",
        "\
cases:
  - name: beta is b
    stack: {text_series_description: 'beta'}
    axes: {kind: bee}
",
    );
    d
}

fn kind_of(pack: &nils_pack::Pack, text: &str) -> nils_pack::Verdict {
    let mut s = nils_pack::Stack::new();
    s.set(
        "text_series_description",
        nils_pack::stack::Value::Text(Some(text)),
    )
    .unwrap();
    nils_pack::Evaluated::new(pack, &s).classify()
}

fn list_overlay(lists: &str, case_text: &str, axis: &str, value: &str) -> String {
    format!(
        "overlay: site\nversion: 1.0.0\npack: t\nscope: {{station: MR1}}\nlists:\n{lists}cases:\n  - name: the site's word\n    stack: {{text_series_description: '{case_text}'}}\n    axes: {{{axis}: {value}}}\n"
    )
}

#[test]
fn a_pack_of_this_contract_names_every_list_a_site_may_amend() {
    let d = axis_pack();
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.contract, 5);
    // a, b and w are tried by the axis (a flag, words, a window); x by a
    // longhand word rule; y by a flag alone and c by nothing at all
    assert_eq!(pack.lists, vec!["kind.a", "kind.b", "kind.w", "mood.x"]);
    let kind = &pack.axes[0];
    let b = &kind.values[1];
    assert_eq!(b.keywords, vec!["beta", "bee"]);
    assert!(b.tried);
    assert_eq!(
        kind.values[0].detection.exclusive.as_deref(),
        Some("has_agent")
    );
    assert_eq!(kind.values[2].detection.physics.len(), 1);
    assert!(
        kind.values[2].detection.physics[0]
            .when
            .contains("echo_time")
    );
    assert_eq!(kind.values[2].detection.physics[0].confidence, Some(0.6));
    assert!(!kind.values[3].tried, "c is vocabulary a route would set");
    let mood = &pack.axes[1];
    assert_eq!(mood.values[0].keywords, vec!["ex"]);
    assert_eq!(
        mood.values[1].detection.exclusive.as_deref(),
        Some("has_agent")
    );
}

#[test]
fn a_list_adds_a_site_word_to_an_axis_value_and_the_verdict_moves() {
    let d = axis_pack();
    let plain = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(kind_of(&plain, "zzqq scan").stored("kind"), "");
    let o = overlay(
        &d,
        &list_overlay("  kind.b: {add: [zzqq]}\n", "zzqq scan", "kind", "bee"),
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    let v = kind_of(&amended, "zzqq scan");
    assert_eq!(v.stored("kind"), "bee", "the row stores the label");
    assert_eq!(v.evidence[0].matched, "zzqq");
    assert_eq!(v.evidence[0].tier, "keywords");
    assert_eq!(
        kind_of(&amended, "beta").stored("kind"),
        "bee",
        "the pack's own words stay"
    );
    assert_eq!(
        amended.axes[0].values[1].keywords,
        vec!["beta", "bee", "zzqq"]
    );
    assert_eq!(amended.overlay_terms, vec!["zzqq"], "for overlay_unused");
    assert_eq!(amended.overlay.as_deref(), Some("site@1.0.0"));
    // and nothing was written into the pack
    assert_eq!(
        nils_pack::load(d.path(), None).unwrap().axes[0].values[1].keywords,
        vec!["beta", "bee"]
    );
}

#[test]
fn a_list_may_name_the_value_by_its_label_and_remove_a_word() {
    let d = axis_pack();
    let o = overlay(
        &d,
        "\
overlay: site
version: 1.0.0
pack: t
scope: {station: MR1}
lists:
  kind.bee: {add: [zzqq], remove: [BETA]}
cases:
  - name: this site does not say beta
    stack: {text_series_description: 'beta'}
    axes: {kind: ''}
",
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    assert_eq!(kind_of(&amended, "beta").stored("kind"), "");
    assert_eq!(kind_of(&amended, "zzqq").stored("kind"), "bee");
    assert_eq!(amended.axes[0].values[1].keywords, vec!["bee", "zzqq"]);
}

#[test]
fn a_list_gives_a_value_that_had_only_a_flag_or_a_window_a_word_tier() {
    let d = axis_pack();
    // a is reached by a flag alone; w by a physics window alone
    let o = overlay(
        &d,
        &list_overlay(
            "  kind.a: {add: [zza]}\n  kind.w: {add: [zzw]}\n",
            "zza",
            "kind",
            "a",
        ),
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    let v = kind_of(&amended, "zza");
    assert_eq!(v.stored("kind"), "a");
    assert_eq!(v.evidence[0].tier, "keywords");
    assert_eq!(kind_of(&amended, "zzw").stored("kind"), "w");
    assert_eq!(amended.axes[0].values[0].keywords, vec!["zza"]);
    assert_eq!(
        amended.axes[0].values[0].detection.exclusive.as_deref(),
        Some("has_agent"),
        "the flag stays"
    );
}

#[test]
fn a_list_amends_the_words_of_a_longhand_rule_that_reaches_the_value() {
    let d = axis_pack();
    let o = overlay(
        &d,
        &list_overlay("  mood.x: {add: [zzmm]}\n", "zzmm", "mood", "x"),
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    let plain = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(kind_of(&plain, "zzmm").stored("mood"), "");
    let amended = nils_pack::load(d.path(), Some(&ov)).unwrap();
    assert_eq!(kind_of(&amended, "zzmm").stored("mood"), "x");
    assert_eq!(amended.axes[1].values[0].keywords, vec!["ex", "zzmm"]);
}

#[test]
fn a_list_on_a_value_no_word_reaches_is_refused_with_why() {
    let d = axis_pack();
    for (lists, at, why) in [
        (
            "  kind.c: {add: [x]}\n",
            "lists.kind.c",
            "no word of the pack's reaches c on kind",
        ),
        (
            "  mood.y: {add: [x]}\n",
            "lists.mood.y",
            "no word of the pack's reaches y on mood",
        ),
        (
            "  kind.zz: {add: [x]}\n",
            "lists.kind.zz",
            "kind has no value named zz",
        ),
        (
            "  nope.a: {add: [x]}\n",
            "lists.nope.a",
            "the pack has no axis named nope; it decides kind, mood",
        ),
    ] {
        let o = overlay(&d, &list_overlay(lists, "x", "kind", "a"));
        let ov = nils_pack::Overlay::load(&o).unwrap();
        let e = nils_pack::load(d.path(), Some(&ov))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains(at), "{at}: {e}");
        assert!(e.contains(why), "{at}: {e}");
    }
}

#[test]
fn an_overlay_reaching_for_a_flag_or_the_order_is_refused_at_parse() {
    let d = axis_pack();
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nscope: {model: X}\nflags:\n  is_zz: has_agent\nlists:\n  kind.b: {add: [x]}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: bee}}\n",
    );
    let e = nils_pack::Overlay::load(&o).err().unwrap().to_string();
    assert!(e.contains("flags"), "{e}");
    assert!(e.contains("stay the pack's"), "{e}");
    let o = overlay(
        &d,
        "overlay: k\nversion: 1.0.0\npack: t\nscope: {model: X}\nlists:\n  kind.b: {add: [x], order: [b, a]}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: bee}}\n",
    );
    let e = nils_pack::Overlay::load(&o).err().unwrap().to_string();
    assert!(e.contains("lists.kind.b.order"), "{e}");
    assert!(
        e.contains("order the values are tried in stay the pack's"),
        "{e}"
    );
}

#[test]
fn a_pack_of_an_earlier_contract_loads_and_a_later_one_is_refused() {
    let d = axis_pack();
    let manifest = std::fs::read_to_string(d.path().join("pack.yml")).unwrap();
    d.file("pack.yml", &manifest.replace("contract: 5", "contract: 4"));
    let pack = nils_pack::load(d.path(), None).unwrap();
    assert_eq!(pack.contract, 4);
    assert_eq!(pack.lists.len(), 4, "its lists are amendable all the same");
    let o = overlay(
        &d,
        &list_overlay("  kind.b: {add: [zzqq]}\n", "zzqq", "kind", "bee"),
    );
    let ov = nils_pack::Overlay::load(&o).unwrap();
    assert_eq!(
        kind_of(&nils_pack::load(d.path(), Some(&ov)).unwrap(), "zzqq").stored("kind"),
        "bee"
    );
    d.file("pack.yml", &manifest.replace("contract: 5", "contract: 6"));
    let e = nils_pack::load(d.path(), None).err().unwrap().to_string();
    assert!(e.contains("wants contract 6"), "{e}");
    assert!(e.contains("this engine implements 5"), "{e}");
}
