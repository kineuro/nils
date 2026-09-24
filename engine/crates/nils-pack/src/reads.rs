// SPDX-License-Identifier: AGPL-3.0-only

//! What one clause of a rule reads (record 48 R1): the fields of the
//! fingerprint, the texts the pack derives and the axes decided before it,
//! followed through every flag and parser the clause names. The reader's
//! evidence line shows the header values the deciding clause read, and a
//! batch groups stacks by them, so the pack is what says which ones.

use std::collections::BTreeSet;

use crate::expr::Expr;
use crate::rules::Clause;
use crate::{Pack, stack};

/// What a clause reads, each list sorted and without repeats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reads {
    /// Fields of the fingerprint and the pack's ingested private elements.
    pub fields: Vec<String>,
    /// Texts the pack derives for itself (`search_text`), which are words
    /// a stack carried.
    pub texts: Vec<String>,
    /// Axes decided before the clause.
    pub axes: Vec<String>,
    /// The flags it holds on, by name.
    pub flags: Vec<String>,
}

#[derive(Default)]
struct Walk {
    fields: BTreeSet<usize>,
    flags: BTreeSet<usize>,
    axes: BTreeSet<usize>,
}

impl Walk {
    fn expr(&mut self, pack: &Pack, e: &Expr) {
        match e {
            Expr::Pred { parser, .. } => {
                if let Some(p) = pack.parsers.get(*parser) {
                    self.fields.insert(p.field);
                }
            }
            Expr::InParser { parser, inner } => {
                if let Some(p) = pack.parsers.get(*parser) {
                    self.fields.insert(p.field);
                }
                self.expr(pack, inner);
            }
            Expr::Flag(f) => self.flag(pack, *f),
            Expr::Field { .. } | Expr::Text { .. } => {
                let mut out = Vec::new();
                e.fields(&mut out);
                self.fields.extend(out);
                if let Expr::Text { inner, .. } = e {
                    self.expr(pack, inner);
                }
            }
            Expr::Axis { axis, .. } | Expr::AxisMissingOr { axis, .. } => {
                self.axes.insert(*axis);
            }
            Expr::Any(es) | Expr::All(es) => es.iter().for_each(|x| self.expr(pack, x)),
            Expr::Not(x) => self.expr(pack, x),
            _ => {}
        }
    }

    fn flag(&mut self, pack: &Pack, f: usize) {
        if !self.flags.insert(f) {
            return;
        }
        if let Some(e) = pack.flags.get(f) {
            self.expr(pack, e);
        }
    }
}

/// The name of a field a pack numbered: the fingerprint's own, then the
/// pack's derived texts, then its ingested private elements.
pub fn field_name(pack: &Pack, i: usize) -> Option<(String, bool)> {
    if let Some(name) = stack::FIELDS.get(i) {
        return Some((name.to_string(), false));
    }
    let j = i - stack::FIELDS.len();
    if let Some(d) = pack.derived.get(j) {
        return Some((d.into.clone(), true));
    }
    pack.ingest
        .get(j - pack.derived.len())
        .map(|p| (p.name.clone(), false))
}

/// What clause `clause` of rule `rule` in rule set `rule_set` reads; none
/// when the pack has no such clause (a pack of another version).
pub fn clause_reads(pack: &Pack, rule_set: &str, rule: &str, clause: usize) -> Option<Reads> {
    let set = pack.rule_sets.iter().find(|s| s.name == rule_set)?;
    let r = set.rules.iter().find(|r| r.id == rule)?;
    let c = r.clauses.get(clause)?;
    let mut w = Walk::default();
    match c {
        Clause::Flag { flag, .. } => w.flag(pack, *flag),
        Clause::Keywords { field, .. } => {
            w.fields.insert(*field);
        }
        Clause::AnyFlag { flags, .. } | Clause::Combination { flags, .. } => {
            for f in flags {
                w.flag(pack, *f);
            }
        }
        Clause::When { expr, .. } => w.expr(pack, expr),
    }
    let mut out = Reads::default();
    for i in &w.fields {
        match field_name(pack, *i) {
            Some((name, true)) => out.texts.push(name),
            Some((name, false)) => out.fields.push(name),
            None => {}
        }
    }
    out.axes = w
        .axes
        .iter()
        .filter_map(|a| pack.axes.get(*a).map(|x| x.name.clone()))
        .collect();
    out.flags = w
        .flags
        .iter()
        .filter_map(|f| pack.flag_names.get(*f).cloned())
        .collect();
    for list in [
        &mut out.fields,
        &mut out.texts,
        &mut out.axes,
        &mut out.flags,
    ] {
        list.sort();
        list.dedup();
    }
    Some(out)
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

    /// A physics rule reads the numbers it compares, through its flags; a
    /// keyword rule reads the text it searches; an axis clause reads the
    /// axis; and a clause the pack does not have reads nothing.
    #[test]
    fn a_clause_names_what_it_reads_through_its_flags() {
        let pack = mri();
        let mut physics = 0;
        let mut words = 0;
        for set in &pack.rule_sets {
            for rule in &set.rules {
                for (i, c) in rule.clauses.iter().enumerate() {
                    let reads = clause_reads(&pack, &set.name, &rule.id, i).unwrap();
                    match c {
                        Clause::Keywords { .. } => {
                            assert!(
                                !reads.texts.is_empty() || !reads.fields.is_empty(),
                                "{}/{} reads its text",
                                set.name,
                                rule.id
                            );
                            words += 1;
                        }
                        _ if c.tier() == crate::rules::Tier::Physics
                            && reads
                                .fields
                                .iter()
                                .any(|f| f == "repetition_time" || f == "echo_time") =>
                        {
                            physics += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(physics > 0, "some physics clause reads TR or TE");
        assert!(words > 0, "some clause reads words");
        let mprage = clause_reads(&pack, "base", "technique:MPRAGE", 0).unwrap();
        assert_eq!(mprage.axes, vec!["technique".to_string()]);
        assert!(mprage.fields.is_empty());
        assert!(clause_reads(&pack, "base", "no such rule", 0).is_none());
        assert!(clause_reads(&pack, "base", "technique:MPRAGE", 9).is_none());
    }
}
