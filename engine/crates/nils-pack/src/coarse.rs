// SPDX-License-Identifier: AGPL-3.0-only

//! An axis that is the coarse form of another.
//!
//! A pack may hold one fact at two grains: the MRI pack's body part splits
//! the head into a brain and a brain and neck, and its body region says
//! head for both. The rules already fold the fine answer into the coarse
//! one, and until an axis file could say so that relation was only in the
//! rule set: nothing told a person, or a model, drafting a question that
//! "a brain scan" in ordinary words is the region and not the fine value.
//!
//! An axis file says it with two keys: `coarse_of`, the axis this one is
//! the coarse form of, and `covers` on each of its values, the values of
//! that axis the value covers.
//!
//! ```yaml
//! axis: body_region
//! coarse_of: body_part
//! values:
//!   head: {covers: [brain, brain-neck], read_for: [brain]}
//! ```
//!
//! A third key, `read_for` on a value of the coarse axis, lists the values
//! it covers that a question which writes one of them alone is read as the
//! coarse value for: "a brain scan" in ordinary words is the head, so the
//! ask reads `body_part = brain` in an authored question as `body_region =
//! head` and says so. It is a declaration and no heuristic: a fine value
//! that is not listed is taken as written. The loader holds it to `covers`
//! ([`declared`]): each listed value is one the value covers, and the value
//! covers another beside it, since a value that covers one thing is no
//! wider than it.
//!
//! The keys decide nothing the rules decide. The rules decide, and the
//! loader holds the declaration to them twice: [`declared`] when the axis file is read (the
//! axis named is declared before this one, every covered value is a value
//! of it, and no value is covered twice), and [`folded`] when the whole
//! pack is loaded (the rules decide the coarse axis from the fine one
//! alone, and for each fine value they decide the coarse value that covers
//! it, and none where no value covers it). A pack without the keys loads
//! as it always did.
//!
//! A sort reads the declaration as well ([`forms`]). The rules fold their
//! own answer of the finer axis, and a decision arrives after they ran, so
//! where a decision stands on one of the two axes the sort holds the other
//! to it by what is declared here: a rule's value the decided coarse value
//! does not cover is set aside, and a decided value of the finer axis
//! decides the coarse one ([`Form::covers`], [`Form::fold`]).

use std::collections::BTreeMap;

use crate::error::{Error, R};
use crate::rules::{Axis, AxisValue};
use crate::{Pack, Stack};

/// The declaration of one axis file, checked against the axes declared
/// before it. Each covered value is rewritten to its identity, so a value
/// named by an identity it had before a rename is the value it became.
pub(crate) fn declared(
    name: &str,
    coarse_of: Option<&str>,
    values: &mut [AxisValue],
    before: &[Axis],
) -> R<()> {
    let Some(fine_name) = coarse_of else {
        if let Some(v) = values.iter().find(|v| !v.read_for.is_empty()) {
            return Err(Error::at(
                format!("values.{}.read_for", v.id),
                format!(
                    "{name} is the coarse form of no axis; read_for is read beside coarse_of \
                     and covers, and lists values this one covers"
                ),
            ));
        }
        if let Some(v) = values.iter().find(|v| !v.covers.is_empty()) {
            return Err(Error::at(
                format!("values.{}.covers", v.id),
                format!(
                    "{name} is the coarse form of no axis; covers is read beside coarse_of, \
                     which names the axis the covered values are of"
                ),
            ));
        }
        return Ok(());
    };
    if fine_name == name {
        return Err(Error::at(
            "coarse_of",
            format!("{name} cannot be the coarse form of itself"),
        ));
    }
    let Some(fine) = before.iter().find(|a| a.name == fine_name) else {
        return Err(Error::at(
            "coarse_of",
            format!(
                "no axis named {fine_name} is declared before {name}; the coarse form comes \
                 after the axis it folds"
            ),
        ));
    };
    if values.iter().all(|v| v.covers.is_empty()) {
        return Err(Error::at(
            "coarse_of",
            format!("{name} is the coarse form of {fine_name} and no value of it covers a value"),
        ));
    }
    let mut covered: BTreeMap<String, String> = BTreeMap::new();
    for v in values.iter_mut() {
        for c in v.covers.iter_mut() {
            let Some(i) = fine.value_index(c) else {
                return Err(Error::at(
                    format!("values.{}.covers", v.id),
                    format!("{c} is no value of {fine_name}"),
                ));
            };
            *c = fine.values[i].id.clone();
            if let Some(other) = covered.insert(c.clone(), v.id.clone()) {
                return Err(Error::at(
                    format!("values.{}.covers", v.id),
                    if other == v.id {
                        format!("{c} is listed twice")
                    } else {
                        format!(
                            "{c} is covered by {other} already; a value of {fine_name} has one \
                             coarse value"
                        )
                    },
                ));
            }
        }
    }
    // What a value is read for is held to what it covers: each value listed
    // is one it covers, by any name of it, and it covers another beside it.
    for v in values.iter_mut() {
        let at = format!("values.{}.read_for", v.id);
        let mut listed: Vec<String> = Vec::new();
        for r in &v.read_for {
            let Some(i) = fine.value_index(r) else {
                return Err(Error::at(at, format!("{r} is no value of {fine_name}")));
            };
            let id = fine.values[i].id.clone();
            if !v.covers.contains(&id) {
                return Err(Error::at(
                    at,
                    format!(
                        "{} does not cover {id}; a value is read for values it covers",
                        v.id
                    ),
                ));
            }
            if listed.contains(&id) {
                return Err(Error::at(at, format!("{id} is listed twice")));
            }
            listed.push(id);
        }
        if !listed.is_empty() && v.covers.len() < 2 {
            return Err(Error::at(
                at,
                format!(
                    "{} covers {} alone; a value read for another covers one more beside it, \
                     or it is no wider than the value written",
                    v.id, listed[0]
                ),
            ));
        }
        v.read_for = listed;
    }
    Ok(())
}

/// Every declared coarse form, held against what the pack's rules do. The
/// check is the pack's own evaluation ([`crate::derive`]): the coarse axis
/// must be derivable from the fine one alone, and with each fine value in
/// turn given as the answer, over a stack that says nothing else, the
/// rules must decide the coarse value declared to cover it, or nothing
/// where none is.
pub(crate) fn folded(pack: &Pack) -> R<()> {
    for coarse in &pack.axes {
        let Some(fine_name) = &coarse.coarse_of else {
            continue;
        };
        let at = format!("axis {}.coarse_of", coarse.name);
        let Some(fine) = pack.axes.iter().find(|a| a.name == *fine_name) else {
            return Err(Error::at(at, format!("no axis named {fine_name}")));
        };
        let asked = vec![fine.name.clone()];
        let derive = vec![coarse.name.clone()];
        if let Err(why) = crate::derive::check(pack, &asked, &derive) {
            return Err(Error::at(
                at,
                format!(
                    "the rules do not decide {} from {} alone: {why}",
                    coarse.name, fine.name
                ),
            ));
        }
        // "From the fine axis alone" is held of every rule that writes the
        // coarse axis, by what the rule reads: the derivation check above
        // lets a rule of a stated tier read a flag where another rule of
        // its set reads an axis, and the comparison below runs over a stack
        // on which no flag holds, so such a rule would never fire in it and
        // would decide the coarse axis of real stacks against `covers`.
        let coarse_index = pack.axis_index(&coarse.name);
        for set in &pack.rule_sets {
            for rule in &set.rules {
                if !rule.sets.iter().any(|x| Some(x.axis) == coarse_index) {
                    continue;
                }
                let reads = crate::reads::rule_reads(pack, set, rule);
                let other = reads
                    .axes
                    .iter()
                    .find(|a| **a != fine.name && **a != coarse.name)
                    .map(|a| format!("the axis {a}"))
                    .or_else(|| reads.flags.first().map(|f| format!("the flag {f}")))
                    .or_else(|| reads.fields.first().map(|f| format!("the field {f}")))
                    .or_else(|| reads.texts.first().map(|t| format!("the text {t}")));
                if let Some(other) = other {
                    return Err(Error::at(
                        at,
                        format!(
                            "the rules do not decide {} from {} alone: {}/{} reads {other}",
                            coarse.name, fine.name, set.name, rule.id
                        ),
                    ));
                }
            }
        }
        let stack = Stack::new();
        for v in &fine.values {
            let answer = BTreeMap::from([(fine.name.clone(), Some(vec![v.id.clone()]))]);
            let got = crate::derive::derive(pack, &stack, Vec::new(), &asked, &derive, &answer);
            let mut rules: Vec<String> =
                got.get(&coarse.name).cloned().flatten().unwrap_or_default();
            // what the axis says when nothing matched is no value of it
            rules.retain(|r| {
                coarse.default.as_deref() != Some(r.as_str()) || coarse.value_index(r).is_some()
            });
            let declared: Vec<String> = coarse
                .values
                .iter()
                .filter(|c| c.covers.contains(&v.id))
                .map(|c| c.id.clone())
                .collect();
            if rules != declared {
                let said = |list: &[String]| match list {
                    [] => "nothing".to_string(),
                    some => some.join(", "),
                };
                return Err(Error::at(
                    format!("axis {}.values", coarse.name),
                    format!(
                        "covers says a {} of {} is {} of {}, and the rules decide {}",
                        fine.name,
                        v.id,
                        said(&declared),
                        coarse.name,
                        said(&rules)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// A coarse form of an axis: the coarse axis's name and, by each of its
/// values that covers any, the values of the finer axis it covers.
pub type CoarseForm<'a> = (&'a str, Vec<(&'a str, &'a [String])>);

/// The coarse forms of an axis: every axis declared the coarse form of
/// `fine`. What a catalog serves so a drafter sees the wider choice beside
/// the fine value.
pub fn of<'a>(axes: &'a [Axis], fine: &str) -> Vec<CoarseForm<'a>> {
    axes.iter()
        .filter(|a| a.coarse_of.as_deref() == Some(fine))
        .map(|a| {
            (
                a.name.as_str(),
                a.values
                    .iter()
                    .filter(|v| !v.covers.is_empty())
                    .map(|v| (v.id.as_str(), v.covers.as_slice()))
                    .collect(),
            )
        })
        .collect()
}

/// A declared coarse form as a sort reads it: the coarse axis and the
/// finer axis it folds.
#[derive(Debug, Clone, Copy)]
pub struct Form<'a> {
    pub coarse: &'a Axis,
    pub fine: &'a Axis,
}

/// Every coarse form the pack declares between two axes that hold one
/// value each, in the order the coarse axes are declared, which is after
/// the axes they fold. What a sort holds a decision on either axis to.
pub fn forms(pack: &Pack) -> Vec<Form<'_>> {
    pack.axes
        .iter()
        .filter_map(|coarse| {
            let fine = pack.axis(coarse.coarse_of.as_deref()?)?;
            (!coarse.multi && !fine.multi).then_some(Form { coarse, fine })
        })
        .collect()
}

impl<'a> Form<'a> {
    /// The coarse value that covers the value of the finer axis a text
    /// names, as a row stores it, and `Some(None)` where no coarse value
    /// covers it. None where the text names no value of the finer axis,
    /// the empty text of an axis that holds nothing among them.
    pub fn fold(&self, fine: &str) -> Option<Option<&'a str>> {
        let id = &self.fine.values[self.fine.value_named(fine.trim())?].id;
        Some(
            self.coarse
                .values
                .iter()
                .position(|c| c.covers.contains(id))
                .map(|i| self.coarse.stored(i)),
        )
    }

    /// Whether the coarse value one text names covers the value of the
    /// finer axis another names. None where either names no value of its
    /// axis: nothing can be said of the two then.
    pub fn covers(&self, coarse: &str, fine: &str) -> Option<bool> {
        let c = &self.coarse.values[self.coarse.value_named(coarse.trim())?];
        let id = &self.fine.values[self.fine.value_named(fine.trim())?].id;
        Some(c.covers.contains(id))
    }

    /// By value of the coarse axis, as a row stores it, the values of the
    /// finer axis, as a row stores them, that it does not cover: the pairs
    /// of rows that do not agree, for whoever counts them.
    pub fn apart(&self) -> Vec<(String, Vec<String>)> {
        self.coarse
            .values
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let fine = self
                    .fine
                    .values
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !c.covers.contains(&f.id))
                    .map(|(j, _)| self.fine.stored(j).to_string())
                    .collect();
                (self.coarse.stored(i).to_string(), fine)
            })
            .collect()
    }
}
