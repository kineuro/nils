// SPDX-License-Identifier: AGPL-3.0-only

//! Which joint answers the pack allows (record 45, S1): the constraints an
//! `axes` question freezes when a campaign is made, so an answer that the
//! pack forbids is refused whoever gives it, and a later pack does not move
//! the ground under a campaign already running.
//!
//! Four facts of the pack say what may hold together, and all four are
//! already the pack's own words:
//!
//! - **the vocabulary**: a value of an axis is one the axis declares, named
//!   by its identity (its label is taken too, and read as the identity);
//! - **the kind of the axis**: a single-valued axis holds one value or none,
//!   a multi-valued one a set;
//! - **the exclusion groups**: at most one member of a group holds
//!   (`group:` on a value, `IR_CONTRAST` of the modifiers among them);
//! - **the schema implications** `nils pack shape` reports: a clause that
//!   reads other axes' values and nothing else, such as `technique =
//!   MPRAGE` setting `base = T1w`. Its condition holding on an answer means
//!   the rule fires, so the answer must hold what the rule writes.
//!
//! Only what the asked axes can decide is frozen: an implication that reads
//! an axis the question does not ask, or writes one, cannot be judged from
//! the answer and is left out; so is one whose rule set is a route entered
//! on anything but axis values, since whether it fires depends on the stack.
//!
//! The constraints are JSON, in identities throughout, so the registry,
//! which knows no pack, checks an answer against them
//! (`nils_registry::campaign`). The condition language is the axis subset of
//! the pack's own: `true`, `false`, `{axis, is}`, `{axis, missing_or}`,
//! `{all: [..]}`, `{any: [..]}` and `{not: ..}`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::expr::Expr;
use crate::pack::Pack;
use crate::rules::{Axis, Clause, Which};

/// An axis the question asks about, and what the pack says of it.
fn axis<'a>(pack: &'a Pack, name: &str) -> Result<(usize, &'a Axis), String> {
    pack.axes
        .iter()
        .enumerate()
        .find(|(_, a)| a.name == name)
        .ok_or_else(|| {
            format!(
                "{name} is not an axis of {}; its axes are {}",
                pack.id(),
                pack.axes
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// A value as the pack writes it in a condition (its stored form) or as a
/// person names it (identity or label), as the identity.
fn identity(a: &Axis, value: &str) -> Option<String> {
    a.values
        .iter()
        .find(|v| v.id == value || v.label == value)
        .map(|v| v.id.clone())
        .or_else(|| a.id_of_stored(value).map(str::to_string))
}

/// An axis-only expression as the constraint language writes it, or none
/// when it reads anything but the asked axes' values.
fn condition(pack: &Pack, e: &Expr, asked: &[usize]) -> Option<Value> {
    Some(match e {
        Expr::Lit(b) => json!(b),
        Expr::Axis { axis, value } if asked.contains(axis) => {
            let a = &pack.axes[*axis];
            json!({"axis": a.name, "is": identity(a, value)?})
        }
        Expr::AxisMissingOr { axis, value } if asked.contains(axis) => {
            let a = &pack.axes[*axis];
            json!({"axis": a.name, "missing_or": identity(a, value)?})
        }
        Expr::All(xs) => {
            json!({"all": xs.iter().map(|x| condition(pack, x, asked)).collect::<Option<Vec<_>>>()?})
        }
        Expr::Any(xs) => {
            json!({"any": xs.iter().map(|x| condition(pack, x, asked)).collect::<Option<Vec<_>>>()?})
        }
        Expr::Not(x) => json!({"not": condition(pack, x, asked)?}),
        _ => return None,
    })
}

/// The constraints of an `axes` question over `axes`: the vocabulary (the
/// values given, each checked against the pack, or every value of the
/// axis), which axes are multi-valued, the exclusion groups and the
/// implications among the asked axes. Refused for an axis the pack does not
/// have or a value its axis does not declare.
pub fn constraints(
    pack: &Pack,
    axes: &[String],
    values: &BTreeMap<String, Vec<String>>,
) -> Result<Value, String> {
    if axes.is_empty() {
        return Err("an axes question names at least one axis".into());
    }
    let mut asked: Vec<usize> = Vec::new();
    let mut vocabulary = serde_json::Map::new();
    let mut multi: Vec<&str> = Vec::new();
    let mut groups = serde_json::Map::new();
    for name in axes {
        let (i, a) = axis(pack, name)?;
        if asked.contains(&i) {
            return Err(format!("the question names {name} twice"));
        }
        asked.push(i);
        let listed: Vec<String> = match values.get(name) {
            Some(given) => given
                .iter()
                .map(|v| {
                    identity(a, v)
                        .ok_or_else(|| format!("{v} is not a value of {name} in {}", pack.id()))
                })
                .collect::<Result<_, _>>()?,
            None => a.values.iter().map(|v| v.id.clone()).collect(),
        };
        vocabulary.insert(name.clone(), json!(listed));
        if a.multi {
            multi.push(&a.name);
        }
        let mut by_group: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for v in &a.values {
            if let Some(g) = &v.group {
                by_group.entry(g).or_default().push(&v.id);
            }
        }
        if !by_group.is_empty() {
            groups.insert(name.clone(), json!(by_group));
        }
    }
    for name in values.keys() {
        if !axes.contains(name) {
            return Err(format!(
                "values names {name}, which the question does not ask"
            ));
        }
    }
    let mut implications = Vec::new();
    for set in &pack.rule_sets {
        // a route entered on anything but axis values fires on some stacks
        // and not others, which no answer says
        let entered = match &set.enter_when {
            None => None,
            Some(e) => match condition(pack, e, &asked) {
                Some(c) => Some(c),
                None => continue,
            },
        };
        for rule in &set.rules {
            for (ci, c) in rule.clauses.iter().enumerate() {
                if !rule.restates(ci) {
                    continue;
                }
                let Clause::When { expr, .. } = c else {
                    continue;
                };
                let Some(mut when) = condition(pack, expr, &asked) else {
                    continue;
                };
                let mut all = Vec::new();
                if let Some(e) = &entered {
                    all.push(e.clone());
                }
                if let Some(r) = &rule.requires {
                    match condition(pack, r, &asked) {
                        Some(r) => all.push(r),
                        None => continue,
                    }
                }
                if !all.is_empty() {
                    all.push(when);
                    when = json!({"all": all});
                }
                let mut then = Vec::new();
                for s in &rule.sets {
                    if !asked.contains(&s.axis) {
                        continue;
                    }
                    let a = &pack.axes[s.axis];
                    for v in &s.values {
                        let Which::Fixed(i) = v.value else {
                            continue;
                        };
                        let mut t = json!({"axis": a.name, "value": a.values[i].id});
                        if let Some(w) = &v.when {
                            match condition(pack, w, &asked) {
                                Some(w) => t["when"] = w,
                                None => continue,
                            }
                        }
                        then.push(t);
                    }
                }
                if then.is_empty() {
                    continue;
                }
                implications.push(json!({
                    "rule": format!("{}/{}", set.name, rule.id),
                    "when": when,
                    "then": then,
                }));
            }
        }
    }
    Ok(json!({
        "pack": pack.id(),
        "values": vocabulary,
        "multi": multi,
        "groups": groups,
        "implications": implications,
    }))
}

/// Record 48, the reader's search: every name a person may know each value
/// of the asked axes by, so typing a vendor's name finds the value. Per axis,
/// per value the question asks: its `label` where it differs from the
/// identity, its `description` where the pack gives one, its `terms` (the
/// pack's display synonyms) and its `keywords` (the words the pack's rules
/// read for it, after any overlay, less the words only a route's rule
/// reads as its cue for a whole combination). Generic vocabulary of the pack, the same
/// for every stack, so it is served on a blind item alike. An axis the pack
/// does not have, or a value its axis does not declare, is left out.
pub fn vocabulary(pack: &Pack, values: &BTreeMap<String, Vec<String>>) -> Value {
    let mut out = serde_json::Map::new();
    for (name, listed) in values {
        let Ok((_, a)) = axis(pack, name) else {
            continue;
        };
        let mut per = serde_json::Map::new();
        for id in listed {
            let Some(v) = a.values.iter().find(|v| &v.id == id) else {
                continue;
            };
            let mut seen: Vec<String> = Vec::new();
            let mut fresh = |list: &[String]| -> Vec<String> {
                let mut kept = Vec::new();
                for t in list {
                    let t = t.trim();
                    let folded = t.to_lowercase();
                    if t.is_empty() || seen.contains(&folded) || folded == v.id.to_lowercase() {
                        continue;
                    }
                    seen.push(folded);
                    kept.push(t.to_string());
                }
                kept
            };
            let terms = fresh(&v.terms);
            // a route's cue for a whole combination is no name of each value it sets
            let own: Vec<String> = v
                .keywords
                .iter()
                .filter(|k| !v.route_words.contains(&k.trim().to_lowercase()))
                .cloned()
                .collect();
            let keywords = fresh(&own);
            let mut e = json!({"terms": terms, "keywords": keywords});
            if v.label != v.id {
                e["label"] = json!(v.label);
            }
            if let Some(d) = &v.description {
                e["description"] = json!(d);
            }
            per.insert(v.id.clone(), e);
        }
        out.insert(name.clone(), Value::Object(per));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mri() -> Pack {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
        crate::load(&dir, None).expect("the MRI pack loads")
    }

    #[test]
    fn the_mri_pack_freezes_its_groups_and_its_implications() {
        let pack = mri();
        let axes: Vec<String> = ["base", "technique", "modifier"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = constraints(&pack, &axes, &BTreeMap::new()).unwrap();
        assert_eq!(c["multi"], json!(["modifier"]));
        let ir = c["groups"]["modifier"]["IR_CONTRAST"].as_array().unwrap();
        assert!(ir.contains(&json!("FLAIR")) && ir.contains(&json!("STIR")));
        // technique MPRAGE sets base T1w, and says so as an implication
        let mprage = c["implications"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["when"] == json!({"axis": "technique", "is": "MPRAGE"}))
            .expect("the MPRAGE implication");
        assert_eq!(mprage["then"], json!([{"axis": "base", "value": "T1w"}]));
        // an implication is frozen only when the question asks what it reads
        let only_base = constraints(&pack, &["base".to_string()], &BTreeMap::new()).unwrap();
        assert!(
            only_base["implications"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| !i["when"].to_string().contains("technique"))
        );
        // a value the axis does not declare is refused, and a label is read as its identity
        let mut values = BTreeMap::new();
        values.insert("base".to_string(), vec!["T3w".to_string()]);
        assert!(constraints(&pack, &["base".to_string()], &values).is_err());
        values.insert("base".to_string(), vec!["T2*w".to_string()]);
        let c = constraints(&pack, &["base".to_string()], &values).unwrap();
        assert_eq!(c["values"]["base"], json!(["T2starw"]));
        assert!(constraints(&pack, &["colour".to_string()], &BTreeMap::new()).is_err());
    }

    #[test]
    fn the_vocabulary_finds_a_value_by_a_vendors_name() {
        let pack = mri();
        let mut values = BTreeMap::new();
        values.insert(
            "technique".to_string(),
            vec!["MPRAGE".to_string(), "3D-TSE".to_string()],
        );
        values.insert("base".to_string(), vec!["T2starw".to_string()]);
        values.insert("colour".to_string(), vec!["red".to_string()]);
        let v = vocabulary(&pack, &values);
        let words = |axis: &str, value: &str, list: &str| -> Vec<String> {
            v[axis][value][list]
                .as_array()
                .unwrap_or_else(|| panic!("{axis}.{value}.{list} in {v}"))
                .iter()
                .map(|x| x.as_str().unwrap().to_lowercase())
                .collect()
        };
        // the pack's rules read ir spgr for MPRAGE, and a person knows it as MP-RAGE or TFL;
        // a word both lists hold is served once, as a term
        assert!(words("technique", "MPRAGE", "keywords").contains(&"ir spgr".to_string()));
        assert!(words("technique", "MPRAGE", "terms").contains(&"bravo".to_string()));
        assert!(!words("technique", "MPRAGE", "keywords").contains(&"bravo".to_string()));
        assert!(words("technique", "MPRAGE", "terms").contains(&"mp-rage".to_string()));
        assert!(words("technique", "MPRAGE", "terms").contains(&"tfl".to_string()));
        // a word said twice is served once, and never the identity itself
        let all: Vec<String> = [
            words("technique", "MPRAGE", "terms"),
            words("technique", "MPRAGE", "keywords"),
        ]
        .concat();
        assert!(!all.contains(&"mprage".to_string()), "{all:?}");
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
        // a label that differs from the identity is said
        assert_eq!(v["technique"]["3D-TSE"]["label"], "SPACE");
        assert_eq!(v["base"]["T2starw"]["label"], "T2*w");
        assert!(v["technique"]["MPRAGE"].get("label").is_none());
        // longhand keyword rules count: base's words come from rules/base.yml
        assert!(words("base", "T2starw", "keywords").contains(&"t2*-w".to_string()));
        // only what the question asks, and nothing of an axis the pack lacks
        assert!(v["technique"].get("TSE").is_none());
        assert!(v.get("colour").is_none());
    }

    #[test]
    fn a_route_s_cue_is_no_name_of_every_value_it_sets() {
        let pack = mri();
        let mut values = BTreeMap::new();
        values.insert(
            "technique".to_string(),
            vec![
                "EPI".to_string(),
                "SE-EPI".to_string(),
                "GRE-EPI".to_string(),
            ],
        );
        values.insert(
            "base".to_string(),
            vec!["SWI".to_string(), "T2starw".to_string()],
        );
        let v = vocabulary(&pack, &values);
        let words = |axis: &str, value: &str| -> Vec<String> {
            ["terms", "keywords"]
                .iter()
                .flat_map(|l| v[axis][value][*l].as_array().cloned().unwrap_or_default())
                .map(|x| x.as_str().unwrap().to_lowercase())
                .collect()
        };
        // EPIMix reads swi for its 3D EPI SWI, yet SWI is no name of EPI
        assert!(!words("technique", "EPI").contains(&"swi".to_string()));
        assert!(!words("technique", "SE-EPI").contains(&"iso dwi".to_string()));
        assert!(!words("technique", "GRE-EPI").contains(&"t2star".to_string()));
        // the SWI route's words for its outputs are no name of base SWI
        for w in ["minip", "magnitude", "qsm", "r2star"] {
            assert!(!words("base", "SWI").contains(&w.to_string()), "{w}");
        }
        // base's own rules still find base SWI by its words
        assert!(words("base", "SWI").contains(&"swan".to_string()));
        // the packs door still shows every word that reaches the value
        let (_, technique) = axis(&pack, "technique").unwrap();
        let epi = technique.values.iter().find(|x| x.id == "EPI").unwrap();
        assert!(epi.keywords.contains(&"swi".to_string()));
        assert_eq!(epi.route_words, vec!["swi".to_string()]);
        // a word the axis file lists for the value stays its name, though a route reads it too
        values.insert("construct".to_string(), vec!["MyelinMap".to_string()]);
        let v = vocabulary(&pack, &values);
        let kw: Vec<String> = v["construct"]["MyelinMap"]["keywords"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_lowercase())
            .collect();
        assert!(kw.contains(&"myelin".to_string()), "{kw:?}");
    }
}
