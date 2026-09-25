// SPDX-License-Identifier: AGPL-3.0-only

//! Axes a person is not asked, computed from what they answered (record 48,
//! after the first real read).
//!
//! Some axes of a pack are not read off the pictures at all: they are the
//! pack's own rules applied to other axes (directory type, disposition,
//! convertible and role in the MRI pack), or a token of the header (quality,
//! from ImageType). A rater answers the axes that need a person, and these
//! are carried through the pack from that answer. They are the rater's
//! answer carried through the pack's rules, never the pack's opinion of the
//! stack, so a derived axis is refused where one of its rules would bring
//! the pack's own reading of the file's numbers or words into it.
//!
//! - [`check`] says whether an axis may be derived beside the asked ones.
//! - [`infer`] finds every axis that may be, when a question names none.
//! - [`depends`] says which asked axes a derived one reads, so a can't tell
//!   on one of them makes the derived axis can't tell as well.
//! - [`derive`] computes them for one stack from an answer, with the pack's
//!   own evaluation ([`crate::Evaluated::with_pins`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::rules::{Rule, RuleSet, Tier};
use crate::{Evaluated, Pack, Stack};

/// The header fields an axis read from the file alone may read: ImageType's
/// tokens, which the scanner writes and no person types.
const TOKENS: &[&str] = &["image_type"];

/// The rules of the pack that write an axis, with their set.
fn writers(pack: &Pack, axis: usize) -> Vec<(&RuleSet, &Rule)> {
    pack.rule_sets
        .iter()
        .flat_map(|s| s.rules.iter().map(move |r| (s, r)))
        .filter(|(_, r)| r.sets.iter().any(|x| x.axis == axis))
        .collect()
}

fn index(pack: &Pack, name: &str) -> Result<usize, String> {
    pack.axis_index(name)
        .ok_or_else(|| format!("{name} is not an axis of the {} pack", pack.name))
}

/// Whether `axis` may be derived when `asked` are answered and `covered`
/// (the asked and the derived) are known. Every rule that writes it and
/// writes no asked axis (a route that decides an asked axis goes with the
/// answer) must be of a tier that reads no numbers and no words of its own,
/// and the axes they read must all be covered; an axis that reads no other
/// axis may read ImageType alone.
fn qualifies(
    pack: &Pack,
    axis: usize,
    asked: &BTreeSet<usize>,
    covered: &BTreeSet<usize>,
) -> Result<(), String> {
    let name = &pack.axes[axis].name;
    let mut axes_read: BTreeSet<String> = BTreeSet::new();
    let mut fields_read: BTreeSet<String> = BTreeSet::new();
    for (set, rule) in writers(pack, axis) {
        if rule.sets.iter().any(|x| asked.contains(&x.axis)) {
            continue;
        }
        if let Some(c) = rule.clauses.iter().find(|c| {
            matches!(
                c.tier(),
                Tier::Physics | Tier::Keywords | Tier::Combination | Tier::Alternative
            )
        }) {
            return Err(format!(
                "{name} is decided by {}/{}, a rule of the {} tier that reads the file's own numbers or words, so a derived {name} would carry the pack's answer rather than the rater's",
                set.name,
                rule.id,
                c.tier().name()
            ));
        }
        let r = crate::reads::rule_reads(pack, set, rule);
        axes_read.extend(r.axes);
        fields_read.extend(r.fields);
        fields_read.extend(r.texts);
    }
    let covered_names: BTreeSet<&str> = covered
        .iter()
        .map(|i| pack.axes[*i].name.as_str())
        .collect();
    if let Some(a) = axes_read
        .iter()
        .find(|a| a.as_str() != name && !covered_names.contains(a.as_str()))
    {
        return Err(format!(
            "{name} reads {a}, which the question neither asks nor derives, so a derived {name} would carry the pack's answer for {a}"
        ));
    }
    let reads_others = axes_read.iter().any(|a| a != name);
    if !reads_others && let Some(f) = fields_read.iter().find(|f| !TOKENS.contains(&f.as_str())) {
        return Err(format!(
            "{name} reads none of the answered axes, and reads {f} from the file, more than ImageType's tokens"
        ));
    }
    Ok(())
}

/// Whether the axes `derive` may be derived beside `asked`, in the pack.
pub fn check(pack: &Pack, asked: &[String], derive: &[String]) -> Result<(), String> {
    let asked_i: BTreeSet<usize> = asked
        .iter()
        .map(|a| index(pack, a))
        .collect::<Result<_, _>>()?;
    let mut covered = asked_i.clone();
    for d in derive {
        let i = index(pack, d)?;
        if asked_i.contains(&i) {
            return Err(format!(
                "{d} is asked, and an asked axis is the rater's to answer, never derived"
            ));
        }
        if !covered.insert(i) && !asked_i.contains(&i) {
            return Err(format!("derive names {d} twice"));
        }
    }
    for d in derive {
        qualifies(pack, index(pack, d)?, &asked_i, &covered)?;
    }
    Ok(())
}

/// The rest of the pack's axes, in the pack's order, where every one of
/// them may be derived beside `asked` ([`check`] allows each); none
/// otherwise. A question that asks part of the pack and leaves the rest to
/// the pack's own reading derives nothing unless it says so.
pub fn infer(pack: &Pack, asked: &[String]) -> Result<Vec<String>, String> {
    let asked_i: BTreeSet<usize> = asked
        .iter()
        .map(|a| index(pack, a))
        .collect::<Result<_, _>>()?;
    let mut covered = asked_i.clone();
    loop {
        let mut grew = false;
        for i in 0..pack.axes.len() {
            if covered.contains(&i) {
                continue;
            }
            let mut with = covered.clone();
            with.insert(i);
            if qualifies(pack, i, &asked_i, &with).is_ok() {
                covered = with;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    if covered.len() < pack.axes.len() {
        return Ok(Vec::new());
    }
    Ok((0..pack.axes.len())
        .filter(|i| !asked_i.contains(i))
        .map(|i| pack.axes[i].name.clone())
        .collect())
}

/// The asked axes a derived axis reads, through the derived axes it reads.
pub fn depends(pack: &Pack, axis: &str, asked: &[String], derive: &[String]) -> BTreeSet<String> {
    let asked_s: BTreeSet<&str> = asked.iter().map(String::as_str).collect();
    let asked_i: BTreeSet<usize> = asked.iter().filter_map(|a| pack.axis_index(a)).collect();
    let mut out = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut todo = vec![axis.to_string()];
    while let Some(a) = todo.pop() {
        if !seen.insert(a.clone()) {
            continue;
        }
        let Some(i) = pack.axis_index(&a) else {
            continue;
        };
        for (set, rule) in writers(pack, i) {
            if rule.sets.iter().any(|x| asked_i.contains(&x.axis)) {
                continue;
            }
            for r in crate::reads::rule_reads(pack, set, rule).axes {
                if asked_s.contains(r.as_str()) {
                    out.insert(r);
                } else if derive.contains(&r) {
                    todo.push(r);
                }
            }
        }
    }
    out
}

/// One derived axis: its values as the pack names them (identities), or
/// none where an asked axis it reads was answered can't tell.
pub type Derived = BTreeMap<String, Option<Vec<String>>>;

/// The derived axes of one stack from an answer. `answer` holds each asked
/// axis's identities, `None` for can't tell; an asked axis missing from it
/// is taken as can't tell. `private` are the series' ingested elements, as
/// the classifier hands them in.
pub fn derive(
    pack: &Pack,
    stack: &Stack,
    private: Vec<String>,
    asked: &[String],
    derive: &[String],
    answer: &BTreeMap<String, Option<Vec<String>>>,
) -> Derived {
    let mut pins: Vec<Option<Vec<String>>> = vec![None; pack.axes.len()];
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    for a in asked {
        let Some(i) = pack.axis_index(a) else {
            continue;
        };
        let axis = &pack.axes[i];
        match answer.get(a) {
            Some(Some(ids)) => {
                pins[i] = Some(
                    ids.iter()
                        .map(|v| match axis.value_index(v) {
                            Some(j) => axis.stored(j).to_string(),
                            None => v.clone(),
                        })
                        .collect(),
                );
            }
            _ => {
                unknown.insert(a.clone());
                pins[i] = Some(Vec::new());
            }
        }
    }
    // a route that decides an asked axis is the pack's reading of the
    // stack, and the rater's answer stands for it now: left out, unless it
    // reads ImageType's tokens alone (a screenshot, an error map)
    let asked_i: BTreeSet<usize> = asked.iter().filter_map(|a| pack.axis_index(a)).collect();
    let skip = |set: &RuleSet, rule: &Rule| {
        if !rule.sets.iter().any(|x| asked_i.contains(&x.axis)) {
            return false;
        }
        let r = crate::reads::rule_reads(pack, set, rule);
        !(r.texts.is_empty()
            && r.axes.iter().all(|a| !asked.contains(a))
            && r.fields.iter().all(|f| TOKENS.contains(&f.as_str())))
    };
    let verdict = Evaluated::with_private(pack, stack, private).with_pins(&pins, &skip);
    let mut out = Derived::new();
    for d in derive {
        let reads = depends(pack, d, asked, derive);
        if reads.iter().any(|r| unknown.contains(r)) {
            out.insert(d.clone(), None);
            continue;
        }
        let Some(i) = pack.axis_index(d) else {
            continue;
        };
        let axis = &pack.axes[i];
        let mut values: Vec<String> = verdict
            .axis(d)
            .map(|v| v.values.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|v| !v.is_empty())
            .map(|v| axis.id_of_stored(&v).map(str::to_string).unwrap_or(v))
            .collect();
        values.sort();
        values.dedup();
        out.insert(d.clone(), Some(values));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn mri() -> Pack {
        crate::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri"),
            None,
        )
        .expect("the MRI pack loads")
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const PHASE0: &[&str] = &[
        "provenance",
        "technique",
        "modifier",
        "construct",
        "base",
        "body_part",
        "post_contrast",
    ];

    /// Phase 0's seven asked axes leave five the pack computes from them:
    /// directory type, disposition, convertible and role from the answer,
    /// and quality from ImageType.
    #[test]
    fn the_seven_asked_axes_derive_the_other_five() {
        let pack = mri();
        let mut got = infer(&pack, &words(PHASE0)).unwrap();
        got.sort();
        assert_eq!(
            got,
            words(&[
                "convertible",
                "directory_type",
                "disposition",
                "quality",
                "role"
            ])
        );
        check(&pack, &words(PHASE0), &got).unwrap();
    }

    /// An axis the pack reads off the file's numbers or words is never
    /// derived, and neither is one that reads an axis nobody answered: a
    /// body-part campaign derives nothing the rules decide from the file.
    #[test]
    fn nothing_the_pack_reads_off_the_file_is_derived() {
        let pack = mri();
        let got = infer(&pack, &words(&["body_part"])).unwrap();
        assert!(got.is_empty(), "{got:?}");
        assert!(
            infer(&pack, &words(&["base", "technique"]))
                .unwrap()
                .is_empty()
        );
        for a in [
            "base",
            "technique",
            "modifier",
            "construct",
            "provenance",
            "directory_type",
            "disposition",
            "role",
        ] {
            assert!(!got.iter().any(|g| g == a), "{a} derived from body_part");
        }
        let e = check(&pack, &words(&["technique"]), &words(&["base"])).unwrap_err();
        assert!(e.contains("base"), "{e}");
        let e = check(&pack, &words(&["base"]), &words(&["directory_type"])).unwrap_err();
        assert!(e.contains("reads"), "{e}");
        let e = check(&pack, &words(PHASE0), &words(&["base"])).unwrap_err();
        assert!(e.contains("asked"), "{e}");
    }

    /// Can't tell on base leaves what reads base can't tell, and nothing
    /// else.
    #[test]
    fn a_derived_axis_depends_on_the_asked_axes_it_reads() {
        let pack = mri();
        let asked = words(PHASE0);
        let derive = infer(&pack, &asked).unwrap();
        let dir = depends(&pack, "directory_type", &asked, &derive);
        assert!(
            dir.contains("base") && dir.contains("provenance"),
            "{dir:?}"
        );
        assert!(!dir.contains("body_part"));
        let role = depends(&pack, "role", &asked, &derive);
        assert!(
            role.contains("modifier") && role.contains("base"),
            "{role:?}"
        );
        assert!(depends(&pack, "quality", &asked, &derive).is_empty());
    }

    fn stack(fields: &[(&str, &str)]) -> Stack {
        let mut s = Stack::new();
        for (f, v) in fields {
            s.set(f, crate::stack::Value::Text(Some(v))).unwrap();
        }
        s
    }

    fn answer(pairs: &[(&str, Option<&[&str]>)]) -> BTreeMap<String, Option<Vec<String>>> {
        pairs
            .iter()
            .map(|(a, v)| (a.to_string(), v.map(words)))
            .collect()
    }

    /// The derived axes come from the rater's answer, not the file's words:
    /// a stack whose description says FLAIR, answered T1w, derives the T1w
    /// role; answered a T2w FLAIR, the FLAIR role.
    #[test]
    fn derived_axes_follow_the_answer() {
        let pack = mri();
        let asked = words(PHASE0);
        let derive = infer(&pack, &asked).unwrap();
        let s = stack(&[
            ("text_series_description", "t2 flair tra"),
            ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
            ("modality", "MR"),
        ]);
        let t1 = answer(&[
            ("provenance", Some(&["RawRecon"])),
            ("technique", Some(&["MPRAGE"])),
            ("modifier", Some(&[])),
            ("construct", Some(&[])),
            ("base", Some(&["T1w"])),
            ("body_part", Some(&["brain"])),
            ("post_contrast", Some(&["not_given"])),
        ]);
        let got = super::derive(&pack, &s, Vec::new(), &asked, &derive, &t1);
        assert_eq!(got["directory_type"], Some(words(&["anat"])));
        assert_eq!(got["disposition"], Some(words(&["acquisition"])));
        assert_eq!(got["convertible"], Some(words(&["yes"])));
        assert_eq!(got["role"], Some(words(&["t1w"])));
        assert_eq!(got["quality"], Some(Vec::new()));

        let flair = answer(&[
            ("provenance", Some(&["RawRecon"])),
            ("technique", Some(&["TSE"])),
            ("modifier", Some(&["FLAIR"])),
            ("construct", Some(&[])),
            ("base", Some(&["T2w"])),
            ("body_part", Some(&["brain"])),
            ("post_contrast", Some(&["not_given"])),
        ]);
        let got = super::derive(&pack, &s, Vec::new(), &asked, &derive, &flair);
        assert_eq!(got["role"], Some(words(&["flair"])));

        // can't tell on base: what reads base is can't tell too; quality,
        // read from ImageType, still stands
        let unknown = answer(&[
            ("provenance", Some(&["RawRecon"])),
            ("technique", Some(&["TSE"])),
            ("modifier", Some(&[])),
            ("construct", Some(&[])),
            ("base", None),
            ("body_part", Some(&["brain"])),
            ("post_contrast", Some(&["not_given"])),
        ]);
        let got = super::derive(&pack, &s, Vec::new(), &asked, &derive, &unknown);
        assert_eq!(got["directory_type"], None);
        assert_eq!(got["role"], None);
        assert_eq!(got["quality"], Some(Vec::new()));

        // a route that reads the file's words is left out: an EPIMix answered
        // as a diffusion acquisition is filed as dwi from the answer, where
        // the route would read "t2 flair" and say anat
        let epimix = answer(&[
            ("provenance", Some(&["EPIMix"])),
            ("technique", Some(&["SE-EPI"])),
            ("modifier", Some(&[])),
            ("construct", Some(&[])),
            ("base", Some(&["DWI"])),
            ("body_part", Some(&["brain"])),
            ("post_contrast", Some(&["not_given"])),
        ]);
        let got = super::derive(&pack, &s, Vec::new(), &asked, &derive, &epimix);
        assert_eq!(got["directory_type"], Some(words(&["dwi"])));

        // a localizer is a scout whatever else was answered
        let scout = answer(&[
            ("provenance", Some(&["Localizer"])),
            ("technique", Some(&["GRE"])),
            ("modifier", Some(&[])),
            ("construct", Some(&[])),
            ("base", Some(&["T1w"])),
            ("body_part", Some(&["brain"])),
            ("post_contrast", Some(&["not_given"])),
        ]);
        let got = super::derive(&pack, &s, Vec::new(), &asked, &derive, &scout);
        assert_eq!(got["directory_type"], Some(words(&["localizer"])));
        assert_eq!(got["disposition"], Some(words(&["scout"])));
    }
}
