// SPDX-License-Identifier: AGPL-3.0-only

//! A pack's families held to the ask's language and the catalog's fields.
//!
//! The pack's loader holds a family file to what a pack knows by itself:
//! its keys, its names, the axes and values its clauses name
//! (`nils_pack::families`). What only the ask can say is held here, where
//! the language and the fields are: that every clause parses and its op is
//! one of the language, that a field it reads is a field of its level and
//! a derived field one the catalog lists, that a `sets` text parses as the
//! sets of a document, and that `raw` names a text field of a stack.
//!
//! [`held`] is run when a catalog is built, which refuses a pack whose
//! families do not hold, and by `nils pack validate`, so an author hears
//! of it before a release. Nothing is run against a registry: a family is
//! held to what a question may write, never to what a registry holds.

use std::collections::BTreeMap;

use nils_ask::ast::{Clause, Set, clause_of};
use nils_pack::families::Family;
use nils_pack::{Error, Pack};

use crate::Field;

/// The families of a pack, held; the first that does not hold is refused
/// with its file and the path inside it.
pub fn held(pack: &Pack, fields: &BTreeMap<(String, String), Field>) -> Result<(), Error> {
    for family in &pack.families {
        one(family, fields).map_err(|mut e| {
            e.file = Some(
                pack.dir
                    .join("families")
                    .join(format!("{}.yml", family.family)),
            );
            e
        })?;
    }
    Ok(())
}

/// [`held`], against the fields every catalog has: what `nils pack
/// validate` runs, which opens no registry.
pub fn held_alone(pack: &Pack) -> Result<(), Error> {
    held(pack, &crate::level_fields())
}

fn one(family: &Family, fields: &BTreeMap<(String, String), Field>) -> Result<(), Error> {
    for (i, p) in family.products.iter().enumerate() {
        for (j, c) in p.where_.iter().enumerate() {
            let at = format!("products[{i}].where[{j}]");
            let clause = clause_of(c).map_err(|why| Error::at(&at, why))?;
            reads(&clause, &at, "stack", &[], fields)?;
        }
    }
    for (i, e) in family.ask.iter().enumerate() {
        let at = format!("ask[{i}].sets");
        let value = nils_ask::read(&e.sets).map_err(|why| Error::at(&at, why.to_string()))?;
        let sets: BTreeMap<String, Set> = serde_json::from_value(value).map_err(|why| {
            Error::at(
                &at,
                format!("the sets do not read as the sets of a document: {why}"),
            )
        })?;
        for (name, set) in &sets {
            // the names a clause may read beside the level's fields: the
            // set's bindings and the partners its relations name
            let bound: Vec<&str> = set
                .bind
                .names()
                .chain(set.near.iter().map(|n| n.as_.as_str()))
                .chain(set.attach.iter().map(|a| a.as_.as_str()))
                .chain(set.has.iter().filter_map(|h| h.as_.as_deref()))
                .collect();
            for (j, c) in set.where_.iter().enumerate() {
                reads(
                    c,
                    &format!("{at}.{name}.where[{j}]"),
                    set.grain.name(),
                    &bound,
                    fields,
                )?;
            }
        }
    }
    for (i, r) in family.raw.iter().enumerate() {
        let at = format!("raw[{i}].field");
        match fields.get(&("stack".to_string(), r.field.clone())) {
            Some(f) if f.type_ == "text" => {}
            Some(f) => {
                return Err(Error::at(
                    at,
                    format!(
                        "{} is a {} field of a stack, and raw names a text field",
                        r.field, f.type_
                    ),
                ));
            }
            None => {
                return Err(Error::at(
                    at,
                    format!("{} is no field of a stack the catalog lists", r.field),
                ));
            }
        }
    }
    Ok(())
}

/// What a clause reads, held: its ops are the language's, a field is a
/// field of the level (or of a level it names before a dot, or a name the
/// set binds), and a derived field is one the catalog lists.
fn reads(
    c: &Clause,
    at: &str,
    level: &str,
    bound: &[&str],
    fields: &BTreeMap<(String, String), Field>,
) -> Result<(), Error> {
    let mut all = Vec::new();
    c.walk(&mut all);
    for x in all {
        if !nils_ask::schema::OPS.contains(&x.op.as_str()) {
            return Err(Error::at(at, format!("{} is no op of the language", x.op)));
        }
        match (x.op.as_str(), x.ref_name()) {
            ("field", Some(path)) => {
                let known = |level: &str, path: &str| {
                    fields.contains_key(&(level.to_string(), path.to_string()))
                };
                let found = match path.split_once('.') {
                    None => known(level, path) || bound.contains(&path),
                    Some((first, rest)) => {
                        // a level's field by the level's name, or what a
                        // partner or a binding of the set carries
                        bound.contains(&first)
                            || (nils_ask::validate::levels().contains(&first) && known(first, rest))
                    }
                };
                if !found {
                    return Err(Error::at(
                        at,
                        format!("{path} is no field of a {level} the catalog lists"),
                    ));
                }
            }
            ("derived", Some(name)) => {
                if !crate::derived_fields().iter().any(|d| d.name == name) {
                    return Err(Error::at(
                        at,
                        format!("{name} is no derived field the catalog lists"),
                    ));
                }
            }
            ("field" | "derived" | "axis" | "param", None) => {
                return Err(Error::at(at, format!("{} names nothing", x.op)));
            }
            _ => {}
        }
    }
    Ok(())
}
