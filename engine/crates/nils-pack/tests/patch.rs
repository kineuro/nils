// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56 §5.5: rule edits as typed operations on the MRI pack. Each
//! operation applies, and the pack's own loader builds and validates what it
//! wrote; one that cannot apply is refused with a plain why; a patched pack
//! is written as a diff of the pack and loads from where it is written.

use std::path::{Path, PathBuf};

use nils_pack::patch::{self, Patch};
use nils_pack::{Evaluated, Stack};

fn mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn patch_of(ops: &str) -> Patch {
    Patch::parse(
        "test",
        &format!("patch: 1\npack: mri\nreason: a test\nevidence: this test\noperations:\n{ops}"),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

fn applied(ops: &str) -> patch::Patched {
    patch::apply(&mri(), &patch_of(ops), &|_| true).unwrap_or_else(|e| panic!("{ops}\n{e}"))
}

fn refused(ops: &str) -> String {
    let p = match Patch::parse(
        "test",
        &format!("patch: 1\npack: mri\nreason: a test\nevidence: this test\noperations:\n{ops}"),
    ) {
        Ok(p) => p,
        // refused for its shape, before anything is read
        Err(e) => return format!("operation 1: {e}"),
    };
    match patch::apply(&mri(), &p, &|_| true) {
        Ok(_) => panic!("applied, and it should have been refused:\n{ops}"),
        Err(e) => e.to_string(),
    }
}

fn stack(fields: &[(&str, &str)]) -> Stack {
    let mut s = Stack::new();
    s.set("modality", nils_pack::stack::Value::Text(Some("MR")))
        .unwrap();
    for (k, v) in fields {
        s.set(k, nils_pack::stack::Value::Text(Some(v))).unwrap();
    }
    s
}

fn stored(pack: &nils_pack::Pack, s: &Stack, axis: &str) -> String {
    Evaluated::new(pack, s).classify().stored(axis)
}

#[test]
fn add_words_reaches_the_bucket_a_value_reads_and_moves_the_stack() {
    let bare = nils_pack::load(&mri(), None).unwrap();
    let s = stack(&[
        ("text_series_description", "ax t1 mdc"),
        ("manufacturer", "SIEMENS"),
    ]);
    assert_eq!(stored(&bare, &s, "post_contrast"), "");
    let p = applied("  - {op: add_words, axis: post_contrast, value: given, words: [mdc]}\n");
    assert_eq!(p.applied[0].files, ["pack.yml"]);
    assert!(
        p.applied[0].changes[0].contains("contrast_positive"),
        "{:?}",
        p.applied[0].changes
    );
    assert!(p.pack.buckets["contrast_positive"].contains(&"mdc".to_string()));
    assert_eq!(stored(&p.pack, &s, "post_contrast"), "1");
    // the pack itself is untouched
    let again = nils_pack::load(&mri(), None).unwrap();
    assert!(!again.buckets["contrast_positive"].contains(&"mdc".to_string()));
}

#[test]
fn remove_words_takes_a_word_out_of_its_list() {
    let p =
        applied("  - {op: remove_words, axis: post_contrast, value: not_given, words: [nativ]}\n");
    assert!(!p.pack.buckets["contrast_negative"].contains(&"nativ".to_string()));
}

#[test]
fn every_kind_of_operation_applies_and_the_loader_builds_it() {
    let p = applied(concat!(
        "  - {op: move_rule, rule: post_contrast/positive, before: negative}\n",
        "  - {op: move_set, set: symri, after: technique}\n",
        "  - {op: set_priority, axis: modifier, value: STIR, priority: 0}\n",
        "  - op: add_value\n",
        "    axis: technique\n",
        "    value: ZZ-GRE\n",
        "    label: ZZGRE\n",
        "    family: GRE\n",
        "    words: [zzgre]\n",
        "    after: MPRAGE\n",
        "    bids: {acq: ZZGRE}\n",
        "  - op: add_rule\n",
        "    axis: base\n",
        "    value: T2w\n",
        "    when: [{tag: te, ge: 200}, {tag: tr, ge: 4000}, {not: {axis: directory_type, is: localizer}}]\n",
        "    position: last\n",
        "  - {op: silence, axis: base, when: {axis: directory_type, is: localizer}}\n",
        "  - {op: silence, when: {axis: technique, is: MRS}}\n",
        "  - {op: by_model, axis: base}\n",
        "  - {op: map_name, axis: technique, value: MPRAGE, bids: MPRAGEx}\n",
    ));
    assert_eq!(p.applied.len(), 9);
    let order: Vec<&str> = p.pack.rule_sets.iter().map(|s| s.name.as_str()).collect();
    let at = |n: &str| order.iter().position(|s| *s == n).unwrap();
    assert!(at("symri") > at("technique"), "{order:?}");
    // the silence runs after what decides directory_type, and decides base
    assert!(at("silence_base") > at("intent"), "{order:?}");
    assert!(p.pack.review.by_model.contains(&"base".to_string()));
    let pc = p
        .pack
        .rule_sets
        .iter()
        .find(|s| s.name == "post_contrast")
        .unwrap();
    let ids: Vec<&str> = pc.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["perfusion", "structured", "positive", "negative"]);
    let modifier = p.pack.axes.iter().find(|a| a.name == "modifier").unwrap();
    let stir = modifier.values.iter().find(|v| v.id == "STIR").unwrap();
    assert_eq!(stir.priority, Some(0));
    let technique = p.pack.axes.iter().find(|a| a.name == "technique").unwrap();
    assert!(technique.values.iter().any(|v| v.id == "ZZ-GRE" && v.tried));
    // a stack named for the new value is it
    let s = stack(&[
        ("text_series_description", "zzgre ax"),
        ("scanning_sequence", "GR"),
    ]);
    assert_eq!(stored(&p.pack, &s, "technique"), "ZZGRE");
    // the pass that fills a base leaves a localizer out of its target now
    let vote = p.applied[5].changes.join("; ");
    assert!(vote.contains("physics_vote"), "{vote}");
    // the BIDS token moved
    let acq = p
        .pack
        .bids
        .acq
        .iter()
        .find(|g| g.from == "technique")
        .unwrap();
    assert_eq!(acq.tokens["MPRAGE"], "MPRAGEx");
    assert_eq!(acq.tokens["ZZ-GRE"], "ZZGRE");
}

#[test]
fn a_rule_added_to_a_set_says_where_and_one_alone_runs_before_its_axis() {
    let e = refused("  - {op: add_rule, axis: base, value: T2w, when: [{tag: te, ge: 200}]}\n");
    assert!(e.contains("say where in base it goes"), "{e}");
    let p = applied(
        "  - {op: add_rule, axis: technique, value: TSE, when: [{words: [zzturbo]}, {tag: manufacturer, is: philips}]}\n",
    );
    let order: Vec<&str> = p.pack.rule_sets.iter().map(|s| s.name.as_str()).collect();
    let added = order.iter().position(|s| *s == "technique_added").unwrap();
    assert_eq!(order[added + 1], "technique", "{order:?}");
    let s = stack(&[
        ("text_series_description", "zzturbo ax"),
        ("manufacturer", "Philips"),
    ]);
    assert_eq!(stored(&p.pack, &s, "technique"), "TSE");
    let other = stack(&[
        ("text_series_description", "zzturbo ax"),
        ("manufacturer", "SIEMENS"),
    ]);
    assert_ne!(stored(&p.pack, &other, "technique"), "TSE");
}

#[test]
fn a_silenced_axis_is_answered_as_nothing_and_asked_nothing() {
    let p = applied(
        "  - {op: silence, axis: post_contrast, when: {tag: manufacturer, is: synthetic}}\n",
    );
    let s = stack(&[
        ("text_series_description", "t1 post gd"),
        ("manufacturer", "SYNTHETIC"),
    ]);
    let v = Evaluated::new(&p.pack, &s).classify();
    assert_eq!(v.stored("post_contrast"), "");
    assert!(!v.unresolved.contains(&"post_contrast".to_string()));
    let bare = nils_pack::load(&mri(), None).unwrap();
    assert_eq!(stored(&bare, &s, "post_contrast"), "1");
}

#[test]
fn an_operation_that_cannot_apply_is_refused_with_why() {
    for (ops, why) in [
        (
            "  - {op: add_words, axis: post_contrast, value: given, words: [dotarem]}\n",
            "already holds dotarem",
        ),
        (
            "  - {op: add_words, axis: post_contrast, value: maybe, words: [x]}\n",
            "post_contrast has no value named maybe",
        ),
        (
            "  - {op: add_words, axis: post_kontrast, value: given, words: [x]}\n",
            "no axis named post_kontrast",
        ),
        (
            "  - {op: add_words, axis: post_contrast, value: given, words: [x], field: series_description}\n",
            "reads text_series_description",
        ),
        (
            "  - {op: remove_words, bucket: contrast_positive, words: [zzz]}\n",
            "holds none of zzz",
        ),
        (
            "  - {op: move_set, set: symri, after: swi}\n",
            "already runs after swi",
        ),
        (
            "  - {op: move_set, set: symrii, before: technique}\n",
            "no rule set named symrii",
        ),
        (
            "  - {op: move_rule, rule: post_contrast/positive, before: base/x}\n",
            "a rule moves inside its set",
        ),
        (
            "  - {op: set_priority, axis: modifier, value: FatSat, priority: 1}\n",
            "in no exclusion group",
        ),
        (
            "  - {op: set_priority, axis: modifier, value: FLAIR, priority: 1}\n",
            "already has priority 1",
        ),
        (
            "  - {op: add_value, axis: technique, value: MPRAGE}\n",
            "already names the value MPRAGE",
        ),
        (
            "  - {op: add_value, axis: base, value: T3w, words: [t3]}\n",
            "decided by longhand rules",
        ),
        (
            "  - {op: silence, axis: modifier, when: {axis: directory_type, is: localizer}}\n",
            "holds several values",
        ),
        (
            "  - {op: by_model, axis: body_part}\n",
            "already its image model's",
        ),
        (
            "  - {op: map_name, axis: technique, value: MPRAGE, bids: 'MP_RAGE'}\n",
            "is not a BIDS label",
        ),
        (
            "  - {op: add_rule, axis: base, value: T2w, when: [{tag: no_such_field, ge: 1}], position: first}\n",
            "no field named no_such_field",
        ),
    ] {
        let e = refused(ops);
        assert!(e.contains(why), "{ops}: {e}");
        assert!(
            e.contains("operation 1") || e.contains("does not load"),
            "{e}"
        );
    }
}

#[test]
fn a_patch_for_another_pack_or_version_is_refused() {
    let p = Patch::parse(
        "t",
        "patch: 1\npack: ct\nreason: r\nevidence: e\noperations:\n  - {op: by_model, axis: base}\n",
    )
    .unwrap();
    let e = patch::apply(&mri(), &p, &|_| true)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("this pack is mri"), "{e}");
    let p = Patch::parse(
        "t",
        "patch: 1\npack: mri\nversion: 0.9.0\nreason: r\nevidence: e\noperations:\n  - {op: by_model, axis: base}\n",
    )
    .unwrap();
    let e = patch::apply(&mri(), &p, &|_| true)
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("written against mri 0.9.0"), "{e}");
}

#[test]
fn a_change_the_corpus_disagrees_with_loads_and_says_which_cases_fail() {
    // a scout's own words moved out: the pack loads, and its cases say what broke
    let p = applied("  - {op: move_set, set: symri, after: base}\n");
    if let Some(e) = &p.cases {
        assert!(e.to_string().contains("case assertions do not hold"), "{e}");
    }
}

#[test]
fn an_overlay_reads_as_the_word_edits_it_is() {
    let o = nils_pack::Overlay::parse(
        "o",
        "overlay: site\nversion: 1.0.0\npack: mri\nscope: {station: MR7}\nbuckets:\n  contrast_positive: {add: [mdc]}\nlists:\n  technique.TSE: {add: [zzturbo]}\ncases:\n  - {name: c, stack: {text_series_description: 'ax t1 mdc'}, axes: {post_contrast: '1'}}\n",
    )
    .unwrap();
    let p = Patch::from_overlay(&o);
    assert_eq!(p.operations.len(), 2);
    assert!(
        p.operations
            .iter()
            .all(|op| op.scope.text() == "scanner:station=MR7")
    );
    let patched = patch::apply(&mri(), &p, &|_| true).unwrap();
    assert!(
        patched.cases.is_none(),
        "{:?}",
        patched.cases.map(|e| e.to_string())
    );
    let via_overlay = nils_pack::load(&mri(), Some(&o)).unwrap();
    assert_eq!(
        patched.pack.buckets["contrast_positive"],
        via_overlay.buckets["contrast_positive"]
    );
    let s = stack(&[
        ("text_series_description", "zzturbo ax"),
        ("scanning_sequence", "SE"),
    ]);
    assert_eq!(
        stored(&patched.pack, &s, "technique"),
        stored(&via_overlay, &s, "technique")
    );
}

#[test]
fn a_patched_pack_is_written_as_a_diff_and_loads_where_it_is_written() {
    let p = applied(concat!(
        "  - {op: add_words, axis: post_contrast, value: given, words: [mdc]}\n",
        "  - {op: move_set, set: symri, after: technique}\n",
        "  - {op: by_model, axis: base}\n",
        "  - {op: silence, axis: base, when: {axis: directory_type, is: localizer}}\n",
    ));
    let out = std::env::temp_dir().join(format!("nils-patch-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let written = p.docs.write(&out, Some("1.0.2")).unwrap();
    let names: Vec<&str> = written.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"pack.yml"), "{names:?}");
    assert!(names.contains(&"rules/silence_base.yml"), "{names:?}");
    // the manifest kept its comments: it was rewritten where it changed
    let (_, whole) = written.iter().find(|(n, _)| n == "pack.yml").unwrap();
    assert!(!whole, "the manifest was written whole");
    let text = std::fs::read_to_string(out.join("pack.yml")).unwrap();
    assert!(
        text.contains("# The order the rule sets run in."),
        "comments kept"
    );
    assert!(text.contains("version: 1.0.2"));
    let (written_pack, failures) = nils_pack::load_patched(&out, &Default::default(), &[]).unwrap();
    assert_eq!(written_pack.version.to_string(), "1.0.2");
    assert!(written_pack.buckets["contrast_positive"].contains(&"mdc".to_string()));
    assert_eq!(
        failures.map(|e| e.to_string()),
        p.cases.map(|e| e.to_string())
    );
    let _ = std::fs::remove_dir_all(&out);
}

// ------------------------------------------- the 2026-10-10 pick borders

fn main_pick(pack: &nils_pack::Pack) -> &nils_pack::pick::Model {
    pack.picks.iter().find(|m| m.name == "main").unwrap()
}

#[test]
fn the_pick_file_is_reached_by_four_operations() {
    use nils_pack::pick::{Condition, Rank};
    // The study's R4 (two border kinds out), R1 (the brain roles not a
    // spine's) and an R8-like order, as typed operations.
    let p = applied(concat!(
        "  - {op: remove_border, border: rare}\n",
        "  - {op: remove_border, border: slice_count_outlier}\n",
        "  - {op: set_runner_up_within, within: 0.02}\n",
        "  - op: set_candidates\n",
        "    role: t1w\n",
        "    unless: [{axis: body_part, is: [spine, neck, chest, other]}]\n",
        "  - op: set_near_tie\n",
        "    order:\n",
        "      - {of: mr_acquisition_type, prefer: ['3D']}\n",
        "      - {of: orientation, prefer: [Axial, Coronal, Sagittal]}\n",
        "      - {of: post_contrast, avoid: ['1'], roles: [t1w]}\n",
        "      - {of: slice_thickness, lowest: true}\n",
    ));
    assert_eq!(p.applied.len(), 5);
    assert!(
        p.applied
            .iter()
            .all(|a| a.files.contains(&"picks/main.yml".to_string())),
        "{:?}",
        p.applied
    );
    let m = main_pick(&p.pack);
    assert!(m.borders.rare_within.is_none());
    assert!(m.borders.slice_outlier.is_none());
    assert!(m.borders.retake.is_some(), "the others stay");
    assert_eq!(m.borders.runner_up_within, 0.02);
    assert_eq!(
        m.candidates["t1w"].unless,
        [Condition::Holds {
            of: "body_part".into(),
            any: vec![
                "spine".into(),
                "neck".into(),
                "chest".into(),
                "other".into()
            ],
        }]
    );
    assert_eq!(m.near_tie.len(), 4);
    // A value named by its label is the stored value: post-contrast's 1.
    assert_eq!(m.near_tie[2].rank, Rank::Avoid(vec!["1".into()]));
    assert_eq!(m.near_tie[3].rank, Rank::Lowest);
    // The keys are contract 9's, which the shipped pack declares since
    // 1.0.2, so nothing had to raise it.
    assert_eq!(p.pack.contract, 9);
    let candidacy = p.applied[3].changes.join("; ");
    assert!(
        candidacy.contains("unless body_part is spine or neck or chest or other"),
        "{candidacy}"
    );
    assert!(
        !candidacy.contains("declares contract 9 now"),
        "{candidacy}"
    );
    assert!(
        p.applied[0].changes[0].contains("raises rare no more"),
        "{:?}",
        p.applied[0]
    );
    // The shipped pack is untouched.
    let bare = nils_pack::load(&mri(), None).unwrap();
    assert!(main_pick(&bare).borders.rare_within.is_some());
    assert!(main_pick(&bare).candidates.is_empty() && main_pick(&bare).near_tie.is_empty());
    // And a patched pack is written, its pick file where it changed.
    let out = std::env::temp_dir().join(format!("nils-patch-pick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    p.docs.write(&out, Some("1.1.1")).unwrap();
    let (written, _) = nils_pack::load_patched(&out, &Default::default(), &[]).unwrap();
    assert_eq!(main_pick(&written).candidates, m.candidates);
    assert_eq!(main_pick(&written).near_tie, m.near_tie);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_candidacy_or_a_near_tie_order_is_taken_out_as_it_was_put_in() {
    let p = applied(concat!(
        "  - {op: set_candidates, role: t2w, when: [{tag: fov_x, ge: 175}]}\n",
        "  - {op: set_near_tie, order: [{of: series_number, lowest: true}]}\n",
        "  - {op: set_candidates, role: t2w, when: [], unless: []}\n",
        "  - {op: set_near_tie, order: []}\n",
    ));
    let m = main_pick(&p.pack);
    assert!(m.candidates.is_empty() && m.near_tie.is_empty());
    assert!(
        p.applied[2].changes[0].contains("competes for it again"),
        "{:?}",
        p.applied[2]
    );
}

#[test]
fn a_pick_operation_that_cannot_apply_is_refused_with_why() {
    for (ops, why) in [
        (
            "  - {op: set_candidates, role: t3w, unless: [{axis: body_part, is: spine}]}\n",
            "t3w is not a role the pick main picks for",
        ),
        (
            "  - {op: set_candidates, role: t1w, unless: [{axis: body_part, is: knee}]}\n",
            "body_part has no value named knee",
        ),
        (
            "  - {op: set_candidates, role: t1w, unless: [{words: [spine]}]}\n",
            "a pick's candidacy reads what a pick reads",
        ),
        (
            "  - {op: set_candidates, role: t1w, when: [{tag: no_such_field, ge: 1}]}\n",
            "no_such_field is neither an axis of the pack nor a field",
        ),
        (
            "  - {op: set_candidates, role: t1w}\n",
            "says when or unless",
        ),
        (
            "  - {op: set_candidates, role: t1w, when: [], unless: []}\n",
            "competes for it already",
        ),
        (
            "  - {op: remove_border, border: too_close}\n",
            "too_close is the margin every pick declares",
        ),
        (
            "  - {op: remove_border, border: flow}\n",
            "flow is not a border a pick file declares",
        ),
        (
            "  - {op: remove_border, border: rare, pick: other}\n",
            "the pack has no pick named other",
        ),
        (
            "  - {op: set_runner_up_within, within: 0.05}\n",
            "runner_up_within is 0.05 already",
        ),
        (
            "  - {op: set_runner_up_within, within: 5}\n",
            "within is a fraction of the best score",
        ),
        (
            "  - {op: set_near_tie, order: [{of: mr_acquisition_type, first: ['3D']}]}\n",
            "a step is {of, prefer: [values]}",
        ),
        (
            "  - {op: set_near_tie, order: [{of: dimension, lowest: true}]}\n",
            "dimension is neither an axis of the pack nor a field",
        ),
        (
            "  - {op: set_near_tie, order: [{of: series_number, lowest: true, roles: [t9w]}]}\n",
            "t9w is not a role the pick main picks for",
        ),
        (
            "  - {op: set_near_tie, order: []}\n",
            "has no near-tie order",
        ),
        (
            "  - {op: remove_border, border: rare, scope: 'site:north'}\n",
            "it is a pack edit, scope pack",
        ),
    ] {
        let e = refused(ops);
        assert!(e.contains(why), "{ops}: {e}");
    }
    // A border taken out twice is refused the second time.
    let e = refused(concat!(
        "  - {op: remove_border, border: rare}\n",
        "  - {op: remove_border, border: rare_within}\n",
    ));
    assert!(
        e.contains("operation 2") && e.contains("raises no rare"),
        "{e}"
    );
}

/// The roles a stack of these decided axes holds under the pack, and the
/// verdict that said so.
fn disposed(pack: &nils_pack::Pack, decided: &[(&str, &str)]) -> nils_pack::verdict::Verdict {
    let seed: Vec<Vec<String>> = pack
        .axes
        .iter()
        .map(|a| {
            decided
                .iter()
                .filter(|(n, _)| *n == a.name)
                .map(|(_, v)| v.to_string())
                .collect()
        })
        .collect();
    Evaluated::new(pack, &stack(&[])).dispose(&seed)
}

#[test]
fn a_rule_may_say_nothing_of_an_axis_that_holds_several_values() {
    // The stack holds none of its values where the conditions hold: the
    // study's R1 written in the rules, a spine's stack no role's.
    let t1 = [
        ("base", "T1w"),
        ("directory_type", "anat"),
        ("body_part", "spine"),
    ];
    let bare = nils_pack::load(&mri(), None).unwrap();
    assert_eq!(disposed(&bare, &t1).stored("role"), "t1w");
    let p = applied(
        "  - {op: add_rule, axis: role, nothing: true, when: [{axis: body_part, is: spine}]}\n",
    );
    let set = p.pack.rule_sets.iter().find(|s| s.name == "role").unwrap();
    assert_eq!(
        set.rules[0].id, "added_1",
        "first, before every rule that sets a role"
    );
    let v = disposed(&p.pack, &t1);
    assert_eq!(v.stored("role"), "");
    // The rule it stopped is ranked against it, not against a person.
    let o = v
        .overrides
        .iter()
        .find(|o| o.axis == "role")
        .expect("the t1w rule, stopped");
    assert_eq!(o.by.rule, "added_1");
    assert_eq!(o.rank, "rule_order");
    let brain = [
        ("base", "T1w"),
        ("directory_type", "anat"),
        ("body_part", "brain"),
    ];
    assert_eq!(disposed(&p.pack, &brain).stored("role"), "t1w");
    // Anywhere but first, it would stop only some of them.
    let e = refused(
        "  - {op: add_rule, axis: role, nothing: true, when: [{axis: body_part, is: spine}], position: last}\n",
    );
    assert!(e.contains("runs first in its set"), "{e}");
}

/// A pack that declares contract 8 is raised to 9 by an operation that
/// writes one of 9's keys, and the change says so. The shipped pack
/// declares 9 since 1.0.2, so a copy of it declaring 8 stands in.
#[test]
fn an_operation_that_writes_a_later_contract_s_key_raises_the_pack_s_contract() {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.path().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    let at_8 = std::env::temp_dir().join(format!("nils-patch-contract-8-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&at_8);
    copy(&mri(), &at_8);
    let manifest = std::fs::read_to_string(at_8.join("pack.yml")).unwrap();
    assert!(manifest.contains("\ncontract: 9\n"));
    std::fs::write(
        at_8.join("pack.yml"),
        manifest.replace("\ncontract: 9\n", "\ncontract: 8\n"),
    )
    .unwrap();
    let p = patch::apply(
        &at_8,
        &patch_of(concat!(
            "  - op: set_candidates\n",
            "    role: t1w\n",
            "    unless: [{axis: body_part, is: [spine]}]\n",
        )),
        &|_| true,
    )
    .unwrap();
    assert_eq!(p.pack.contract, 9);
    let changes = p.applied[0].changes.join("; ");
    assert!(
        changes.contains("declares contract 9 now, 8 before"),
        "{changes}"
    );
    let _ = std::fs::remove_dir_all(&at_8);
}
