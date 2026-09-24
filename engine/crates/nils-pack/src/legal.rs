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
}
