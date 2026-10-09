// SPDX-License-Identifier: AGPL-3.0-only

//! Record 51 R6: v0's nine reasons for a look at a pick, on the MRI pack's
//! own numbers. Each case is ported from v0's
//! `tests/cohort_main_qc/test_cohort_main_qc_service.py` at 5ee391f where v0
//! has one, with v0's rows (`make_row`'s defaults: an MPRAGE, 3D, 176
//! slices, a 240 mm field of view, RawRecon, contrast not given) and v0's
//! cohort built the way v0's `_compute_cohort_profile` builds it. The
//! boundaries of each number are the unit tests in `src/pick.rs`.
//!
//! Also here: a pack without the new keys raises only the three it declares,
//! a role without its own tables is refused, and so is a key of contract 7
//! in a pack that declares 6.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nils_pack::pick::{self, Border, Candidate, Model, Reference};

fn mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn main_pick() -> Model {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    pack.picks
        .into_iter()
        .find(|m| m.name == "main")
        .expect("the main pick")
}

/// One stack as v0's `make_row` makes it, in this pack's names.
fn row(changes: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut v: BTreeMap<String, String> = [
        ("base", "T1w"),
        ("technique", "MPRAGE"),
        ("mr_acquisition_type", "3D"),
        ("n_instances", "176"),
        ("fov_x", "240"),
        ("image_orientation_patient", "0\\1\\0\\0\\0\\-1"),
        ("provenance", "RawRecon"),
        ("post_contrast", "0"),
        ("orientation", "Sagittal"),
        ("directory_type", "anat"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, x) in changes {
        if x.is_empty() {
            v.remove(*k);
        } else {
            v.insert((*k).to_string(), (*x).to_string());
        }
    }
    v
}

/// A candidate of these stacks, as the registry side builds one: each name
/// at the first stack's value, each number at its largest.
fn cand(stacks: &[(i64, BTreeMap<String, String>)], family: Option<&str>) -> Candidate {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for (_, s) in stacks {
        for (k, v) in s {
            match (
                values.get(k).and_then(|b| b.parse::<f64>().ok()),
                v.parse::<f64>(),
            ) {
                (Some(b), Ok(n)) if n > b => {
                    values.insert(k.clone(), v.clone());
                }
                (None, _) if !values.contains_key(k) => {
                    values.insert(k.clone(), v.clone());
                }
                _ => {}
            }
        }
    }
    Candidate {
        stacks: stacks.iter().map(|(s, _)| *s).collect(),
        values,
        each: stacks.iter().map(|(_, v)| v.clone()).collect(),
        family: family.map(str::to_string),
        acquired: Vec::new(),
    }
}

/// v0's cohort profile over these rows: every name's counts, and the
/// percentiles of each population in its buckets.
fn cohort(model: &Model, rows: &[BTreeMap<String, String>]) -> Reference {
    let mut counts: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for name in model.reads() {
        for r in rows {
            if let Some(v) = r.get(&name) {
                *counts
                    .entry(name.clone())
                    .or_default()
                    .entry(v.clone())
                    .or_insert(0) += 1;
            }
        }
    }
    let mut percentiles = BTreeMap::new();
    for (population, of, split_by) in model.populations() {
        let mut buckets: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rows {
            let Some(v) = r.get(&of).and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let key = match &split_by {
                Some(s) => format!("{population}:{}", r.get(s).cloned().unwrap_or_default()),
                None => population.clone(),
            };
            buckets.entry(key).or_default().push(v);
        }
        for (key, values) in buckets {
            if let Some(p) = Reference::of(&values) {
                percentiles.insert(key, p);
            }
        }
    }
    Reference {
        name: "v0-cohort".into(),
        counts,
        percentiles,
        total: rows.len() as i64,
    }
}

fn many(n: usize, changes: &[(&str, &str)]) -> Vec<BTreeMap<String, String>> {
    (0..n).map(|_| row(changes)).collect()
}

#[test]
fn v0_s_retake_for_a_bundle_outside_a_family() {
    // test_retake_triggers_border_for_non_family_bundle: two stacks of one
    // fingerprint are one bundle, and a retake.
    let m = main_pick();
    let r = cohort(&m, &many(10, &[]));
    let two = cand(&[(1, row(&[])), (2, row(&[]))], None);
    let p = pick::pick(&m, "t1w", &[two], &r);
    assert!(p.borders.contains(&Border::Retake), "{:?}", p.borders);
    assert!(p.notes["retake"].starts_with("plain"), "{:?}", p.notes);
    // v0's demotion read first: 176 beside an aborted 40 is no retake.
    let short = cand(&[(1, row(&[])), (2, row(&[("n_instances", "40")]))], None);
    let p = pick::pick(&m, "t1w", &[short], &r);
    assert!(!p.borders.contains(&Border::Retake), "{:?}", p.borders);
}

#[test]
fn v0_s_dixon_sisters_are_no_retake_and_two_in_phase_are() {
    // test_dixon_sisters_dont_trigger_retake: the family keeps its in-phase
    // image alone, so its water and fat sisters are not a second take.
    let m = main_pick();
    let dixon = |c: &str| {
        row(&[
            ("technique", "VIBE"),
            ("modifier", "Dixon"),
            ("construct", c),
        ])
    };
    let mut rows = many(
        10,
        &[
            ("technique", "VIBE"),
            ("modifier", "Dixon"),
            ("construct", "InPhase"),
        ],
    );
    rows.extend(many(
        10,
        &[
            ("technique", "VIBE"),
            ("modifier", "Dixon"),
            ("construct", "Water"),
        ],
    ));
    let r = cohort(&m, &rows);
    let one = cand(&[(1, dixon("InPhase"))], Some("dixon"));
    let p = pick::pick(&m, "t1w", &[one], &r);
    assert!(!p.borders.contains(&Border::Retake), "{:?}", p.borders);
    let two = cand(
        &[(1, dixon("InPhase")), (2, dixon("InPhase"))],
        Some("dixon"),
    );
    let p = pick::pick(&m, "t1w", &[two], &r);
    assert!(p.borders.contains(&Border::Retake), "{:?}", p.borders);
    assert!(p.notes["retake"].starts_with("dixon"), "{:?}", p.notes);
}

#[test]
fn v0_s_mp2rage_retake_is_more_than_two() {
    let m = main_pick();
    let uni = || row(&[("technique", "MP2RAGE"), ("construct", "UniformDenoised")]);
    let r = cohort(
        &m,
        &many(
            10,
            &[("technique", "MP2RAGE"), ("construct", "UniformDenoised")],
        ),
    );
    let two = cand(&[(1, uni()), (2, uni())], Some("mp2rage"));
    assert!(
        !pick::pick(&m, "t1w", &[two], &r)
            .borders
            .contains(&Border::Retake)
    );
    let three = cand(&[(1, uni()), (2, uni()), (3, uni())], Some("mp2rage"));
    let p = pick::pick(&m, "t1w", &[three], &r);
    assert!(p.borders.contains(&Border::Retake), "{:?}", p.borders);
    assert!(p.notes["retake"].starts_with("mp2rage"), "{:?}", p.notes);
}

#[test]
fn v0_s_unknown_dimension() {
    // test_unknown_dim_triggers_border: a GRE with no acquisition type.
    let m = main_pick();
    let mut rows = many(10, &[]);
    rows.push(row(&[
        ("technique", "GRE"),
        ("mr_acquisition_type", ""),
        ("n_instances", "120"),
    ]));
    let r = cohort(&m, &rows);
    let gre = cand(
        &[(
            99,
            row(&[
                ("technique", "GRE"),
                ("mr_acquisition_type", ""),
                ("n_instances", "120"),
            ]),
        )],
        None,
    );
    let p = pick::pick(&m, "t1w", &[gre], &r);
    assert!(p.borders.contains(&Border::UnknownDim), "{:?}", p.borders);
    let known = cand(&[(1, row(&[]))], None);
    assert!(
        !pick::pick(&m, "t1w", &[known], &r)
            .borders
            .contains(&Border::UnknownDim)
    );
}

#[test]
fn v0_s_slice_count_outlier_in_its_own_dimension() {
    // v0 has no test of its own for this one. Its cohort profile: slice
    // percentiles per dimension bucket, so a 2D stack of 24 slices is not an
    // outlier among 3D stacks of 176.
    let m = main_pick();
    let mut rows = Vec::new();
    for n in [150, 160, 170, 176, 176, 176, 176, 180, 192, 208] {
        rows.push(row(&[("n_instances", &n.to_string())]));
    }
    for _ in 0..5 {
        rows.push(row(&[
            ("technique", "TSE"),
            ("mr_acquisition_type", "2D"),
            ("n_instances", "24"),
        ]));
    }
    let r = cohort(&m, &rows);
    let at = |n: &str, dim: &str| {
        let c = cand(
            &[(1, row(&[("n_instances", n), ("mr_acquisition_type", dim)]))],
            None,
        );
        pick::pick(&m, "t1w", &[c], &r)
            .borders
            .contains(&Border::SliceOutlier)
    };
    assert!(at("40", "3D"), "40 slices among 3D stacks of 150 to 208");
    assert!(at("400", "3D"));
    assert!(!at("176", "3D"));
    assert!(!at("24", "2D"), "24 slices among 2D stacks of 24");
}

#[test]
fn v0_s_pre_and_post_twin_is_a_border_and_not_a_second_main() {
    // test_pre_post_twin_both_get_main: an MPRAGE before and after contrast,
    // otherwise the same. v0 tagged both main; here one is the pick and the
    // other is named in the border, for a person to choose.
    let m = main_pick();
    let mut rows = many(10, &[("post_contrast", "0")]);
    rows.extend(many(5, &[("post_contrast", "1")]));
    let r = cohort(&m, &rows);
    let pre = cand(&[(1, row(&[("post_contrast", "0")]))], None);
    let post = cand(&[(2, row(&[("post_contrast", "1")]))], None);
    let p = pick::pick(&m, "t1w", &[pre, post], &r);
    assert!(p.borders.contains(&Border::PrePostTwin), "{:?}", p.borders);
    assert_eq!(
        p.winner.as_ref().unwrap().stacks.len(),
        1,
        "one pick, not two"
    );
    let twin = &p.notes["pre_post_twin"];
    assert!(twin == "1" || twin == "2", "{twin}");
}

#[test]
fn v0_s_epimix_fallback() {
    // test_epimix_penalty_halves_score's cohort: EPIMix alone in the session
    // wins, halved, and says so.
    let m = main_pick();
    let r = cohort(&m, &many(5, &[("provenance", "EPIMix")]));
    let epimix = cand(&[(1, row(&[("provenance", "EPIMix")]))], None);
    let p = pick::pick(&m, "t1w", &[epimix], &r);
    assert!(
        p.borders.contains(&Border::EpimixFallback),
        "{:?}",
        p.borders
    );
    assert_eq!(p.scored.unwrap().penalty, 0.5);
    // Beside a RawRecon it does not win, and nothing is said.
    let raw = cand(&[(2, row(&[]))], None);
    let epimix = cand(&[(1, row(&[("provenance", "EPIMix")]))], None);
    let p = pick::pick(&m, "t1w", &[epimix, raw], &r);
    assert_eq!(p.winner.unwrap().stacks, [2]);
    assert!(!p.borders.contains(&Border::EpimixFallback));
}

#[test]
fn v0_s_dixon_against_plain() {
    // test_dixon_vs_plain_border_when_close: "if Dixon family wins AND plain
    // MPRAGE within 10%, expect border". Here the family's in-phase image
    // and a plain VIBE of the same session, the plain one scored close.
    let m = main_pick();
    let mut rows = many(10, &[("technique", "VIBE")]);
    rows.extend(many(
        10,
        &[
            ("technique", "VIBE"),
            ("modifier", "Dixon"),
            ("construct", "InPhase"),
        ],
    ));
    let r = cohort(&m, &rows);
    let dixon = cand(
        &[(
            2,
            row(&[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "InPhase"),
            ]),
        )],
        Some("dixon"),
    );
    let plain = cand(&[(1, row(&[("technique", "VIBE")]))], None);
    let p = pick::pick(&m, "t1w", &[dixon, plain], &r);
    assert_eq!(
        p.winner.as_ref().unwrap().stacks,
        [2],
        "the Dixon wins on its bonuses"
    );
    let margin = p.margin;
    assert!(margin <= 0.10, "the plain VIBE is within a tenth: {margin}");
    assert!(p.borders.contains(&Border::DixonVsPlain), "{:?}", p.borders);
    assert_eq!(p.notes["dixon_vs_plain"], "1");
    // A water-excited one is not plain.
    let dixon = cand(
        &[(
            2,
            row(&[
                ("technique", "VIBE"),
                ("modifier", "Dixon"),
                ("construct", "InPhase"),
            ]),
        )],
        Some("dixon"),
    );
    let we = cand(
        &[(1, row(&[("technique", "VIBE"), ("modifier", "WaterExc")]))],
        None,
    );
    let p = pick::pick(&m, "t1w", &[dixon, we], &r);
    assert!(
        !p.borders.contains(&Border::DixonVsPlain),
        "{:?}",
        p.borders
    );
}

#[test]
fn v0_s_three_that_v1_already_had_still_hold() {
    // close_runner_up, rare_technique and no_canonical_construct.
    let m = main_pick();
    let r = cohort(&m, &many(10, &[]));
    let p = pick::pick(
        &m,
        "t1w",
        &[cand(&[(1, row(&[]))], None), cand(&[(2, row(&[]))], None)],
        &r,
    );
    assert!(p.borders.contains(&Border::TooClose), "{:?}", p.borders);
    let mut rows = many(99, &[]);
    rows.push(row(&[("technique", "FIESTA")]));
    let r = cohort(&m, &rows);
    let odd = cand(&[(1, row(&[("technique", "FIESTA")]))], None);
    assert!(
        pick::pick(&m, "t1w", &[odd], &r)
            .borders
            .contains(&Border::Rare)
    );
    assert_eq!(pick::pick(&m, "t1w", &[], &r).borders, [Border::Nothing]);
}

#[test]
fn every_border_has_a_name_of_its_own() {
    let names: Vec<&str> = Border::ALL.iter().map(|b| b.name()).collect();
    assert_eq!(
        names,
        [
            "too_close",
            "rare",
            "nothing_eligible",
            "retake",
            "unknown_dim",
            "slice_count_outlier",
            "pre_post_twin",
            "epimix_fallback",
            "dixon_vs_plain",
        ]
    );
}

#[test]
fn the_mri_pack_scores_t2w_on_its_own_tables_and_declares_all_nine() {
    let m = main_pick();
    assert_eq!(m.roles, ["t1w", "flair", "t2w"]);
    let b = &m.borders;
    assert!(b.retake.is_some() && b.unknown_dim.is_some() && b.slice_outlier.is_some());
    assert!(b.pre_post_twin.is_some() && b.fallback.is_some() && b.dixon_vs_plain.is_some());
    let names: Vec<&str> = m.families.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["dixon", "mp2rage"]);
    let mp2rage = &m.families[1];
    assert_eq!(mp2rage.canonical, ["UniformDenoised", "Uniform"]);
    assert!(mp2rage.apart_without_canonical);
    assert_eq!(mp2rage.retake_above, 2);
    // A 3D turbo spin echo is the T2w a volume analysis wants, over the 2D
    // one nearly every protocol runs.
    let r = cohort(
        &m,
        &[
            many(10, &[("base", "T2w"), ("technique", "SPACE")]),
            many(
                10,
                &[
                    ("base", "T2w"),
                    ("technique", "TSE"),
                    ("mr_acquisition_type", "2D"),
                    ("n_instances", "30"),
                ],
            ),
        ]
        .concat(),
    );
    let space = cand(
        &[(1, row(&[("base", "T2w"), ("technique", "SPACE")]))],
        None,
    );
    let tse = cand(
        &[(
            2,
            row(&[
                ("base", "T2w"),
                ("technique", "TSE"),
                ("mr_acquisition_type", "2D"),
                ("n_instances", "30"),
            ]),
        )],
        None,
    );
    let p = pick::pick(&m, "t2w", &[tse, space], &r);
    assert_eq!(p.winner.unwrap().stacks, [1]);
    let tech = p
        .scored
        .unwrap()
        .parts
        .into_iter()
        .find(|x| x.name == "tech")
        .unwrap();
    assert_eq!(tech.score, 1.0, "its own table's SPACE: {tech:?}");
}

// ------------------------------------------------------------- the loader

/// Why the pack at `dir` did not load, which it must not.
fn refused(dir: &Path, what: &str) -> String {
    match nils_pack::load(dir, None) {
        Ok(_) => panic!("{what}: the pack loaded"),
        Err(e) => e.to_string(),
    }
}

/// The MRI pack, copied, with `picks/main.yml` edited by `edit` and the
/// contract it declares set to `contract`.
fn edited(contract: u32, edit: impl Fn(&str) -> String) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let to = std::env::temp_dir().join(format!(
        "nils-borders-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&to);
    copy(&mri(), &to);
    let manifest = std::fs::read_to_string(to.join("pack.yml")).unwrap();
    let manifest = manifest
        .lines()
        .map(|l| {
            if l.starts_with("contract:") {
                format!("contract: {contract}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(to.join("pack.yml"), manifest).unwrap();
    if contract < 8 {
        without_contract_8(&to);
    }
    let main = std::fs::read_to_string(to.join("picks/main.yml")).unwrap();
    std::fs::write(to.join("picks/main.yml"), edit(&main)).unwrap();
    to
}

/// Record 53: the MRI pack without what pack contract 8 added (its session
/// pass, its fallback border's list and the private elements it shows), so
/// a copy can declare an earlier contract.
fn without_contract_8(dir: &Path) {
    let edit = |name: &str, f: &dyn Fn(&str) -> String| {
        let p = dir.join(name);
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, f(&text)).unwrap();
    };
    edit("pack.yml", &|t| {
        t.lines()
            .filter(|l| !l.trim().starts_with("- passes/session"))
            .collect::<Vec<_>>()
            .join("\n")
    });
    edit("picks/main.yml", &|t| {
        t.replace("is: [EPIMix, NeuroMix]}", "is: EPIMix}")
    });
    edit("private.yml", &|t| {
        let mut out = Vec::new();
        let mut skipping = false;
        for l in t.lines() {
            if l == "  shown:" {
                skipping = true;
                continue;
            }
            if skipping && (l.starts_with("    ") || l.trim().is_empty()) {
                continue;
            }
            skipping = false;
            out.push(l);
        }
        out.join("\n")
    });
}

/// The pick file as it was before record 51: one family, the old three
/// borders, the two old roles.
fn before_51(main: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in main.lines() {
        if line.starts_with("roles:") {
            out.push_str("roles: [t1w, flair]\n");
            continue;
        }
        if line.starts_with("family:") {
            out.push_str(
                "family:\n  when: {of: modifier, token: Dixon}\n  over: construct\n  \
                 ignoring: [echo_time, repetition_time, inversion_time, flip_angle]\n  \
                 canonical: [InPhase, Water]\n",
            );
            skip = true;
            continue;
        }
        if skip {
            if line.starts_with(' ') || line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            skip = false;
        }
        let key = line.trim_start().split(':').next().unwrap_or("");
        if line.starts_with("  ")
            && !line.starts_with("   ")
            && [
                "retake",
                "unknown_dim",
                "slice_outlier",
                "pre_post_twin",
                "fallback",
                "dixon_vs_plain",
            ]
            .contains(&key)
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn a_pack_without_the_new_keys_raises_only_the_three_it_declares() {
    // A contract-6 pick file, as every pack before 0.17.0 wrote it, loads
    // under this engine and raises too_close, rare and nothing_eligible
    // alone, whatever the session holds.
    let dir = edited(6, before_51);
    let pack = nils_pack::load(&dir, None).unwrap_or_else(|e| panic!("{e}"));
    let m = pack.picks.into_iter().next().unwrap();
    assert_eq!(m.families.len(), 1);
    assert!(m.borders.retake.is_none() && m.borders.dixon_vs_plain.is_none());
    let mut rows = many(10, &[("post_contrast", "0")]);
    rows.push(row(&[
        ("technique", "GRE"),
        ("mr_acquisition_type", ""),
        ("provenance", "EPIMix"),
    ]));
    let r = cohort(&m, &rows);
    // Everything planted: a retake of an EPIMix GRE of unknown dimension, and
    // a post-contrast twin close behind.
    let odd = || {
        row(&[
            ("technique", "GRE"),
            ("mr_acquisition_type", ""),
            ("provenance", "EPIMix"),
            ("n_instances", "400"),
        ])
    };
    let planted = cand(&[(1, odd()), (2, odd())], None);
    let twin = cand(
        &[(3, row(&[("post_contrast", "1"), ("provenance", "EPIMix")]))],
        None,
    );
    let p = pick::pick(&m, "t1w", &[planted, twin], &r);
    for b in &p.borders {
        assert!(
            matches!(b, Border::TooClose | Border::Rare | Border::Nothing),
            "{b:?} from a pack that never declared it: {:?}",
            p.borders
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_role_without_its_own_tables_is_refused_at_load() {
    // Record 51 R7: t2w named, and its technique tiers taken out.
    let dir = edited(7, |main| {
        let at = main
            .find("      t2w:\n        SPACE: 1.00")
            .expect("the t2w tiers");
        let rest = &main[at + "      t2w:\n".len()..];
        let end = rest
            .lines()
            .take_while(|l| l.starts_with("        "))
            .map(|l| l.len() + 1)
            .sum::<usize>();
        format!("{}{}", &main[..at], &rest[end..])
    });
    let e = refused(&dir, "a role without tiers");
    assert!(e.contains("the role t2w has no tiers in tech"), "{e}");
    assert!(e.contains("a role is scored on its own numbers"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_key_of_contract_7_in_a_pack_that_declares_6_is_refused_in_words() {
    // An engine at 6 reads `borders` for two keys and ignores the rest, so
    // a pack using the new keys declares 7 and an engine at 6 refuses it
    // ("the pack wants contract 7"). The same pack declaring 6 is refused
    // here, rather than loaded as a contract-6 pack it is not.
    let dir = edited(6, |main| main.to_string());
    let e = refused(&dir, "new keys at 6");
    assert!(
        e.contains("is pack contract 7's; this pack declares contract 6"),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    // And a pack declaring more than this engine implements is refused
    // before anything else is read: what an engine at 6 says of this one.
    let later = nils_pack::CONTRACT + 1;
    let dir = edited(later, |main| main.to_string());
    let e = refused(&dir, "a later contract");
    assert!(
        e.contains(&format!(
            "the pack wants contract {later}; this engine implements {}",
            nils_pack::CONTRACT
        )),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_border_this_engine_does_not_know_is_refused_and_so_are_bad_numbers() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "  runner_up_within: 0.05\n",
            "  runner_up_within: 0.05\n  close_runner_up_pct: 0.05\n",
            "close_runner_up_pct is not a border this engine knows",
        ),
        (
            "slice_outlier: {population: slices, below: 0.05, above: 0.95}",
            "slice_outlier: {population: slices, below: 0.10, above: 0.95}",
            "below is one of the quantiles a population keeps",
        ),
        (
            "slice_outlier: {population: slices,",
            "slice_outlier: {population: nothing,",
            "nothing is not a population a percentile component of this pick builds",
        ),
        (
            "dixon_vs_plain: {family: dixon,",
            "dixon_vs_plain: {family: flow,",
            "flow is not a family this pick declares",
        ),
        (
            "pre_post_twin: {of: post_contrast, at_least: 0.85}",
            "pre_post_twin: {of: post_contrast, at_least: 85}",
            "at_least is a fraction from 0 to 1, not 85",
        ),
        (
            "    without_canonical: apart",
            "    without_canonical: keep",
            "without_canonical is drop or apart, not keep",
        ),
        (
            "unknown_dim: {of: mr_acquisition_type}",
            "unknown_dim: {of: dimension}",
            "dimension is neither an axis of this pack nor a field of the fingerprint",
        ),
    ];
    for (from, to, want) in cases {
        let dir = edited(7, |main| {
            assert!(main.contains(from), "{from}");
            main.replacen(from, to, 1)
        });
        let e = refused(&dir, want);
        assert!(e.contains(want), "{want}: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
