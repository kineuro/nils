// SPDX-License-Identifier: AGPL-3.0-only

//! What a question authored as text names a value by, read from the pack's
//! vocabulary and said.
//!
//! A person, and a model, writes a question in the words they know: `FLASH`
//! for the technique the pack stores as `SPGRE`, `body_part = brain` for a
//! brain scan. The first names the value by another of its names, and was
//! resolved without a word; the second names a fine value where the
//! ordinary words mean the wider one, and dropped every stack the fine
//! split put under its sibling. Both are read here, where a document
//! arrives as text and is repaired ([`crate::parse_read`]): the document is
//! rewritten, so what is stored, hashed, described and run is what was
//! read, and each reading is an entry of the repairs with what was written
//! and what it was read as ([`Read`]).
//!
//! Nothing is counted and nothing is guessed. A reading is a lookup in what
//! the pack declares:
//!
//! - **word**: a clause that holds an axis against a value it writes out
//!   (`=`, `<>`, `has`, `in` and `not_in`, anywhere in a `where`) and names
//!   the value by an identity, an earlier name or a label that is not the
//!   word a row stores is rewritten to that word, where the vocabulary
//!   names exactly one value by the text. The clause selects the same
//!   rows before and after. A text that names no value exactly is looked
//!   up among the names and among the words the pack lists for each value
//!   (`terms`: `BRAVO` for the sequence stored as `MPRAGE`), letter case
//!   and surrounding space aside, and is read as the one value that has
//!   it; a word two values share is left, and the validator refuses it
//!   and names them
//!   ([`AxisNames::read_word`](crate::validate::AxisNames::read_word)).
//! - **wider**: a clause that keeps exactly one value of an axis, a value
//!   the pack says is read as its wider value where it is written alone
//!   (`read_for` on the coarse axis's value), is rewritten to the clause
//!   on the coarse axis. The fine value stays expressible, and is taken as
//!   written where the clause carries `exact: true`, where the set, or a
//!   set it is drawn from, already keeps the wider value in a clause of
//!   its own (the heads, and of those the brains), where the clause is
//!   under a `not`, where it stands in an `or` beside another value of the
//!   same axis, and where it lists several values. The word is read first,
//!   so a listed word of such a value (`neuro` for `brain`) is read wider
//!   as the value itself is, in one reading that says what was written.
//!
//! A document that arrives structured (a stored one, a strict door) never
//! comes here: it is taken as written.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::{Arg, Ask, Clause, Grain};
use crate::describe::clause_text;
use crate::diagnose::{narrowed_on_purpose, value_filter};
use crate::repair::Repair;
use crate::validate::{Names, Spelling};

/// What a reading read: which kind, the axis and the text written, and the
/// axis and the value it was read as, in the word a row stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Read {
    /// `word`: another name of the value, read as the stored word.
    /// `wider`: a fine value written alone, read as the value that covers it.
    pub kind: String,
    pub axis: String,
    pub wrote: String,
    pub as_axis: String,
    pub as_value: String,
    /// For `wider` alone: the clause that keeps the fine value by itself,
    /// as a document writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<String>,
}

/// Whether a clause says its value is meant as written.
pub(crate) fn is_exact(c: &Clause) -> bool {
    c.opts.get("exact").and_then(Value::as_bool) == Some(true)
}

/// The name a pipeline stage's set gets (`crate::sugar`).
fn stage_name(ask: &Ask, i: usize) -> String {
    ask.pipeline[i]
        .name
        .clone()
        .unwrap_or_else(|| format!("p{}", i + 1))
}

/// Read the values of an authored document in place, and say what was
/// read: an entry for each, at the path of its clause.
pub fn read(ask: &mut Ask, names: &dyn Names) -> Vec<Repair> {
    // the sets as the validator will see them: a pipeline's stages named
    // and chained, so a stage is read with the stages it is drawn from
    let mut view = ask.clone();
    if !view.pipeline.is_empty() && crate::sugar::pipeline(&mut view).is_err() {
        // a stage named as a set is refused at its path later: the named
        // sets are read, the stages left
        view = ask.clone();
        view.pipeline.clear();
    }
    // where each set is written
    let mut written: BTreeMap<String, String> = ask
        .sets
        .keys()
        .map(|n| (n.clone(), format!("sets.{n}")))
        .collect();
    for i in 0..ask.pipeline.len() {
        written
            .entry(stage_name(ask, i))
            .or_insert_with(|| format!("pipeline[{i}]"));
    }
    // the document the checks around a clause read: every value in the
    // word the pack stores, so a clause that keeps the wider value by a
    // listed word of it is the wider clause it will be read as
    let mut before = view.clone();
    for set in before.sets.values_mut() {
        if set.grain == Grain::Stack {
            for c in set.where_.iter_mut() {
                *c = in_stored_words(c, names);
            }
        }
    }
    let mut out = Vec::new();
    for (name, set) in view.sets.iter_mut() {
        let Some(at) = written.get(name).filter(|_| set.grain == Grain::Stack) else {
            continue;
        };
        for (i, c) in set.where_.iter_mut().enumerate() {
            let path = format!("{at}.where[{i}]");
            let within = Within {
                ask: &before,
                set: name,
                names,
            };
            wider(c, &path, &within, &BTreeSet::new(), &mut out);
            words(c, &path, names, &mut out);
        }
    }
    // back into the document as it was written
    for (name, set) in ask.sets.iter_mut() {
        if let Some(read) = view.sets.get(name) {
            set.where_.clone_from(&read.where_);
        }
    }
    for i in 0..ask.pipeline.len() {
        let name = stage_name(ask, i);
        if written.get(&name) == Some(&format!("pipeline[{i}]"))
            && let Some(read) = view.sets.get(&name)
        {
            ask.pipeline[i].set.where_.clone_from(&read.where_);
        }
    }
    out
}

/// A clause with each value it writes out in the word the pack stores,
/// and nothing said: how the wider reading sees a clause, so that a text
/// the word reading will read is read before it.
fn in_stored_words(c: &Clause, names: &dyn Names) -> Clause {
    let mut read = c.clone();
    words(&mut read, "", names, &mut Vec::new());
    read
}

/// Where a clause stands: the document as written, its values in the
/// words the pack stores, and its set.
struct Within<'a> {
    ask: &'a Ask,
    set: &'a str,
    names: &'a dyn Names,
}

/// The axes a clause keeps a value of, itself or in a part of an `and` or
/// an `or` of it.
fn axes_kept(c: &Clause, names: &dyn Names, out: &mut BTreeSet<String>) {
    match c.op.as_str() {
        "and" | "or" => {
            for a in &c.args {
                if let Arg::Clause(x) = a {
                    axes_kept(x, names, out);
                }
            }
        }
        _ => {
            if let Some((axis, _, _)) = value_filter(&in_stored_words(c, names), names) {
                out.insert(axis);
            }
        }
    }
}

/// The wider reading of one `where` clause: the clause itself, or each
/// part of an `and` or an `or` of it. Nothing under a `not` is read, and
/// in an `or` an axis two alternatives keep a value of (`blocked`) is
/// taken as written: the alternatives say which values are meant.
fn wider(
    c: &mut Clause,
    path: &str,
    within: &Within<'_>,
    blocked: &BTreeSet<String>,
    out: &mut Vec<Repair>,
) {
    if matches!(c.op.as_str(), "and" | "or") {
        let mut blocked = blocked.clone();
        if c.op == "or" {
            let mut times: BTreeMap<String, usize> = BTreeMap::new();
            for a in &c.args {
                if let Arg::Clause(x) = a {
                    let mut axes = BTreeSet::new();
                    axes_kept(x, within.names, &mut axes);
                    for axis in axes {
                        *times.entry(axis).or_default() += 1;
                    }
                }
            }
            blocked.extend(times.into_iter().filter(|(_, n)| *n > 1).map(|(a, _)| a));
        }
        for (i, a) in c.args.iter_mut().enumerate() {
            if let Arg::Clause(x) = a {
                wider(x, &format!("{path}[{}]", i + 2), within, &blocked, out);
            }
        }
        return;
    }
    if is_exact(c) {
        return;
    }
    let Some((axis, of, kept)) = value_filter(&in_stored_words(c, within.names), within.names)
    else {
        return;
    };
    let [fine] = kept.as_slice() else {
        return;
    };
    let kin = within.names.axis_kin(&axis);
    if kin.multi || blocked.contains(&axis) {
        return;
    }
    // the value the pack reads this one as, where it is written alone
    let Some((coarse_axis, coarse)) = kin.wider.iter().find_map(|w| {
        w.read_for
            .iter()
            .find(|(_, fines)| fines.contains(fine))
            .map(|(coarse, _)| (w, coarse))
    }) else {
        return;
    };
    // the heads, and of those the brains: a narrowing written on purpose
    if narrowed_on_purpose(within.ask, within.set, &kin, &kept, within.names) {
        return;
    }
    let Some(coarse_names) = within.names.axis_names(&coarse_axis.axis) else {
        return;
    };
    let Some(as_value) = coarse_names
        .values
        .iter()
        .find(|v| v.id == *coarse)
        .map(|v| coarse_names.stored(v).to_string())
    else {
        return;
    };
    let stored = |id: &str| {
        of.values
            .iter()
            .find(|v| v.id == id)
            .map_or(id.to_string(), |v| of.stored(v).to_string())
    };
    let fine_word = stored(fine);
    let covers: Vec<String> = coarse_axis
        .values
        .iter()
        .find(|(v, _)| v == coarse)
        .map(|(_, covers)| covers.iter().map(|c| stored(c)).collect())
        .unwrap_or_default();
    let wrote = match c.args.get(1) {
        Some(Arg::List(items)) => items.first().map(literal_text),
        Some(other) => Some(literal_text(other)),
        None => None,
    }
    .flatten()
    .unwrap_or_else(|| fine_word.clone());
    // the clause that keeps the fine value alone, as a document writes it
    let value = serde_json::to_string(&fine_word).unwrap_or_default();
    let exact = match c.op.as_str() {
        "in" => format!("[\"in\", {{exact: true}}, [\"axis\", {{}}, \"{axis}\"], [{value}]]"),
        op => format!("[\"{op}\", {{exact: true}}, [\"axis\", {{}}, \"{axis}\"], {value}]"),
    };
    let read = Clause::new("=")
        .arg(Arg::Clause(
            Clause::new("axis").arg(Arg::Text(coarse_axis.axis.clone())),
        ))
        .arg(Arg::Text(as_value.clone()));
    // how the text names the fine value, where it is not its stored word
    let named = match of.read_word(&wrote) {
        Some((_, _, Spelling::Name)) => format!(", another name of {fine_word},"),
        Some((_, _, Spelling::Folded)) => {
            format!(", a name of {fine_word} but for letter case or surrounding space,")
        }
        Some((_, _, Spelling::Term)) => format!(", a word the pack lists for {fine_word},"),
        None => String::new(),
    };
    out.push(Repair {
        path: path.to_string(),
        what: format!(
            "{}{named} was read as {}, which covers {}: the pack reads {fine_word} written alone \
             as the wider {as_value}. To keep {fine_word} alone, write {exact}.",
            clause_text(c),
            clause_text(&read),
            listed(&covers),
        ),
        read: Some(Read {
            kind: "wider".to_string(),
            axis,
            wrote,
            as_axis: coarse_axis.axis.clone(),
            as_value,
            exact: Some(exact),
        }),
    });
    *c = read;
}

/// The word reading of one `where` clause and of every clause inside it:
/// each value of an axis it writes out by another name than the stored
/// word, or by a word the pack lists for one value alone, rewritten to
/// the stored word.
fn words(c: &mut Clause, path: &str, names: &dyn Names, out: &mut Vec<Repair>) {
    let axis = match (c.op.as_str(), c.args.first()) {
        ("=" | "<>" | "in" | "not_in" | "has", Some(Arg::Clause(l)))
            if l.op == "axis" && c.args.len() == 2 =>
        {
            l.ref_name().map(str::to_string)
        }
        _ => None,
    };
    if let Some(axis) = axis
        && let Some(of) = names.axis_names(&axis)
    {
        let one = |a: &mut Arg, out: &mut Vec<Repair>| {
            let Some(wrote) = literal_text(a) else {
                return;
            };
            let Some((_, stored, spelling)) = of.read_word(&wrote) else {
                return;
            };
            out.push(Repair {
                path: path.to_string(),
                what: match spelling {
                    Spelling::Name => format!(
                        "On {axis}, {wrote} is another name of {stored}, the word the pack \
                         stores: it was read as {stored}."
                    ),
                    Spelling::Folded => format!(
                        "On {axis}, {wrote} is a name of {stored} but for letter case or \
                         surrounding space: it was read as {stored}, the word the pack stores."
                    ),
                    Spelling::Term => format!(
                        "On {axis}, the pack lists {wrote} as a word for {stored}: it was read \
                         as {stored}, the word the pack stores."
                    ),
                },
                read: Some(Read {
                    kind: "word".to_string(),
                    axis: axis.clone(),
                    wrote,
                    as_axis: axis.clone(),
                    as_value: stored.to_string(),
                    exact: None,
                }),
            });
            *a = Arg::Text(stored.to_string());
        };
        match c.args.get_mut(1) {
            Some(Arg::List(items)) => {
                let before = out.len();
                for item in items.iter_mut() {
                    one(item, out);
                }
                // two names of one value in a list are that value once
                if out.len() > before {
                    let mut kept: Vec<Arg> = Vec::with_capacity(items.len());
                    for item in items.drain(..) {
                        if !kept.contains(&item) {
                            kept.push(item);
                        }
                    }
                    *items = kept;
                }
            }
            Some(other) => one(other, out),
            None => {}
        }
    }
    for (i, a) in c.args.iter_mut().enumerate() {
        if let Arg::Clause(x) = a {
            words(x, &format!("{path}[{}]", i + 2), names, out);
        }
    }
}

/// A value as a document writes it: a text, or a number read as its
/// digits, as the validator and the compiler read it.
fn literal_text(a: &Arg) -> Option<String> {
    match a {
        Arg::Text(t) => Some(t.clone()),
        Arg::Int(n) => Some(n.to_string()),
        Arg::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Words joined as a list is said: a, b and c.
fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [most @ .., end] => format!("{} and {end}", most.join(", ")),
    }
}
