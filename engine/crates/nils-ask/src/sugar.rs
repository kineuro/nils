// SPDX-License-Identifier: AGPL-3.0-only

//! Desugar (§4.3, §6, §11.1): a fixed order, add only, so that
//! `desugar(d) == desugar(desugar(d))`. What is sugar and what is core:
//!
//! - `pipeline` becomes named sets chained by `from` (C17's stage list).
//! - a structural parameter (a window, a rounding map, a level) is inlined
//!   where it is used, because a parameter that changes the emitted SQL is
//!   not a parameter (rule 13); its declaration stays.
//! - `every` becomes a hidden `except` set, `max: 0` on it, and `min: 1` on
//!   the universe, so it is never vacuously true.
//! - `pairs` becomes a hidden clone of the set and a `near best` on it.
//! - `same` becomes a hidden group set keyed by this set's key and the `by`
//!   tuple, two aggregates on this set (`<as>.largest`, `<as>.groups`) and
//!   the bound.
//! - `change` and `share` stay as clause ops of the core: their meaning is
//!   fixed (§4.3) and the compiler emits them; nothing about them is a
//!   rewrite of the document.
//!
//! Hidden sets are named `<set>__<what>`; a document may not name a set
//! with a double underscore, so a rerun never collides.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ast::{
    AST_VERSION, AlgOp, Algebra, Arg, Ask, Clause, Grain, GroupSpec, Has, IntSpec, Near, ParamType,
    Policy, Set, Src, WindowSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SugarError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SugarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Upgrade an older document to the current grammar (§4.5). Version 1 is
/// the first; an unknown later version is refused.
pub fn upgrade(ask: &mut Ask) -> Result<(), SugarError> {
    match ask.ast_version {
        AST_VERSION => Ok(()),
        v if v < AST_VERSION => {
            ask.ast_version = AST_VERSION;
            Ok(())
        }
        v => Err(SugarError {
            path: "ast_version".into(),
            message: format!("version {v} is newer than this engine's {AST_VERSION}"),
        }),
    }
}

/// The whole desugar, in its fixed order. Idempotent.
pub fn desugar(ask: &mut Ask) -> Result<(), SugarError> {
    upgrade(ask)?;
    pipeline(ask)?;
    structural_params(ask)?;
    let names: Vec<String> = ask.sets.keys().cloned().collect();
    for name in &names {
        every(ask, name)?;
        pairs(ask, name)?;
        same(ask, name)?;
    }
    Ok(())
}

fn hidden(set: &str, what: &str) -> String {
    format!("{set}__{what}")
}

/// `pipeline: [stage, ...]` becomes sets `p1`, `p2`, ... (or the stage's own
/// name), each `from` the one before.
fn pipeline(ask: &mut Ask) -> Result<(), SugarError> {
    if ask.pipeline.is_empty() {
        return Ok(());
    }
    let stages = std::mem::take(&mut ask.pipeline);
    let mut previous: Option<String> = None;
    for (i, stage) in stages.into_iter().enumerate() {
        let name = stage.name.unwrap_or_else(|| format!("p{}", i + 1));
        let mut set = stage.set;
        if let Some(p) = &previous
            && set.from.is_none()
            && set.of.is_none()
            && set.algebra.is_none()
            && set.group.is_none()
        {
            set.from = Some(Src::Set(p.clone()));
        }
        if ask.sets.contains_key(&name) {
            return Err(SugarError {
                path: format!("pipeline[{i}]"),
                message: format!("a set named {name} already exists"),
            });
        }
        ask.sets.insert(name.clone(), set);
        previous = Some(name);
    }
    if ask.out.set.is_empty()
        && let Some(last) = previous
    {
        ask.out.set = last;
    }
    Ok(())
}

/// Inline every structural parameter where a `param` ref names it.
fn structural_params(ask: &mut Ask) -> Result<(), SugarError> {
    let structural: BTreeMap<String, (ParamType, Option<Value>)> = ask
        .params
        .iter()
        .filter(|(_, d)| d.type_.structural())
        .map(|(n, d)| (n.clone(), (d.type_, d.value.clone())))
        .collect();
    if structural.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = ask.sets.keys().cloned().collect();
    for name in names {
        let set = ask.sets.get_mut(&name).expect("a named set");
        for (i, n) in set.near.iter_mut().enumerate() {
            inline_window(
                &mut n.window,
                &structural,
                &format!("sets.{name}.near[{i}].window"),
            )?;
        }
        for (i, h) in set.has.iter_mut().enumerate() {
            if let Some(w) = &mut h.window {
                inline_window(w, &structural, &format!("sets.{name}.has[{i}].window"))?;
            }
        }
        if let Some(p) = &mut set.pairs {
            inline_window(
                &mut p.window,
                &structural,
                &format!("sets.{name}.pairs.window"),
            )?;
        }
        let path = format!("sets.{name}");
        for (_, c) in set.bind.0.iter_mut() {
            inline_in_clause(c, &structural, &path)?;
        }
        for c in set.where_.iter_mut() {
            inline_in_clause(c, &structural, &path)?;
        }
        if let Some(pick) = &mut set.pick {
            for o in pick.by.iter_mut() {
                inline_in_clause(&mut o.0, &structural, &path)?;
            }
        }
        for s in set.same.iter_mut() {
            for c in s.by.iter_mut() {
                inline_in_clause(c, &structural, &path)?;
            }
        }
        if let Some(g) = &mut set.group {
            for c in g.by.iter_mut() {
                inline_in_clause(c, &structural, &path)?;
            }
        }
    }
    for c in ask.out.columns.iter_mut() {
        inline_in_clause(c, &structural, "out")?;
    }
    Ok(())
}

fn inline_window(
    w: &mut WindowSpec,
    structural: &BTreeMap<String, (ParamType, Option<Value>)>,
    path: &str,
) -> Result<(), SugarError> {
    if let WindowSpec::Param(c) = w
        && let Some(name) = c.ref_name()
        && let Some((ty, value)) = structural.get(name)
    {
        if *ty != ParamType::Window {
            return Err(SugarError {
                path: path.into(),
                message: format!("{name} is a {} parameter, not a window", ty_name(*ty)),
            });
        }
        let Some(value) = value else {
            return Err(SugarError {
                path: path.into(),
                message: format!(
                    "window parameter {name} has no value; a window is part of the question and desugars into it"
                ),
            });
        };
        let literal = serde_json::from_value(value.clone()).map_err(|e| SugarError {
            path: path.into(),
            message: format!("window parameter {name}: {e}"),
        })?;
        *w = WindowSpec::Literal(literal);
    }
    Ok(())
}

/// A `derived` ref whose options name a structural parameter gets the
/// literal instead: `{round: ["param", {}, "rounding"]}`,
/// `{level: ["param", {}, "level"]}`.
fn inline_in_clause(
    c: &mut Clause,
    structural: &BTreeMap<String, (ParamType, Option<Value>)>,
    path: &str,
) -> Result<(), SugarError> {
    for (key, v) in c.opts.iter_mut() {
        if let Ok(inner) = crate::ast::clause_of(v)
            && inner.op == "param"
            && let Some(name) = inner.ref_name()
            && let Some((_, value)) = structural.get(name)
        {
            let Some(value) = value else {
                return Err(SugarError {
                    path: path.into(),
                    message: format!(
                        "parameter {name} (option {key}) has no value; it changes the question and desugars into it"
                    ),
                });
            };
            *v = value.clone();
        }
    }
    for a in c.args.iter_mut() {
        if let Arg::Clause(inner) = a {
            inline_in_clause(inner, structural, path)?;
        }
    }
    Ok(())
}

fn ty_name(t: ParamType) -> &'static str {
    match t {
        ParamType::Text => "text",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Date => "date",
        ParamType::List => "list",
        ParamType::Cohort => "cohort",
        ParamType::Window => "window",
        ParamType::Rounding => "rounding",
        ParamType::Level => "level",
    }
}

/// `every: [{of: U, in: S}]` on set X: a hidden set `X__not_S` =
/// `except(U, S)`, then `has: [{set: X__not_S, max: 0}, {set: U, min: 1}]`.
fn every(ask: &mut Ask, name: &str) -> Result<(), SugarError> {
    let Some(set) = ask.sets.get(name) else {
        return Ok(());
    };
    if set.every.is_empty() {
        return Ok(());
    }
    let clauses = std::mem::take(&mut ask.sets.get_mut(name).expect("a set").every);
    for e in clauses {
        let universe = ask.sets.get(&e.of).ok_or_else(|| SugarError {
            path: format!("sets.{name}.every"),
            message: format!("no set named {}", e.of),
        })?;
        let grain = universe.grain;
        let hidden_name = hidden(name, &format!("not_{}", e.in_));
        let mut h = Set::at(grain);
        h.algebra = Some(Algebra {
            op: AlgOp::Except,
            sets: vec![e.of.clone(), e.in_.clone()],
            tag: None,
        });
        ask.sets.insert(hidden_name.clone(), h);
        let set = ask.sets.get_mut(name).expect("a set");
        set.has.push(Has {
            set: hidden_name,
            window: None,
            on: None,
            min: None,
            max: Some(IntSpec::Literal(0)),
            as_: None,
        });
        if !e.vacuous {
            set.has.push(Has {
                set: e.of.clone(),
                window: None,
                on: None,
                min: Some(IntSpec::Literal(1)),
                max: None,
                as_: None,
            });
        }
    }
    Ok(())
}

/// `pairs: {as, window, order}` on set X: a hidden clone `X__pair` (`from:
/// X`) and `near: [{as, set: X__pair, window, policy: best, order}]`.
fn pairs(ask: &mut Ask, name: &str) -> Result<(), SugarError> {
    let Some(set) = ask.sets.get(name) else {
        return Ok(());
    };
    let Some(p) = set.pairs.clone() else {
        return Ok(());
    };
    let grain = set.grain;
    let clone_name = hidden(name, "pair");
    let mut clone = Set::at(grain);
    clone.from = Some(Src::Set(name.to_string()));
    ask.sets.insert(clone_name.clone(), clone);
    let set = ask.sets.get_mut(name).expect("a set");
    set.pairs = None;
    set.near.push(Near {
        as_: p.as_,
        set: clone_name,
        window: p.window,
        on: None,
        policy: Policy::Best,
        tie: None,
        order: p.order,
        optional: false,
        strict: false,
    });
    Ok(())
}

/// `same: [{as, over, by, min, all}]` on set X (§6): a hidden group set
/// `X__same_<as>` over `over` keyed by X's key and the `by` tuple, then on
/// X `bind: {<as>.largest: max n, <as>.groups: count}` and `where`.
fn same(ask: &mut Ask, name: &str) -> Result<(), SugarError> {
    let Some(set) = ask.sets.get(name) else {
        return Ok(());
    };
    if set.same.is_empty() {
        return Ok(());
    }
    let grain = set.grain;
    let clauses = std::mem::take(&mut ask.sets.get_mut(name).expect("a set").same);
    for s in clauses {
        let group_name = hidden(name, &format!("same_{}", s.as_));
        let mut by = vec![Clause::field(&format!("{}.id", grain.name()))];
        by.extend(s.by.iter().cloned());
        let mut g = Set::at(Grain::Group);
        g.group = Some(GroupSpec {
            of: s.over.clone(),
            by,
        });
        g.bind.0.push((
            "n".to_string(),
            Clause::new("count").opt("set", Value::String(s.over.clone())),
        ));
        ask.sets.insert(group_name.clone(), g);
        let set = ask.sets.get_mut(name).expect("a set");
        let largest = format!("{}.largest", s.as_);
        let groups = format!("{}.groups", s.as_);
        set.bind.0.push((
            largest.clone(),
            Clause::new("max")
                .opt("set", Value::String(group_name.clone()))
                .arg(Arg::Clause(Clause::field("n"))),
        ));
        set.bind.0.push((
            groups.clone(),
            Clause::new("count").opt("set", Value::String(group_name)),
        ));
        if s.all {
            set.where_.push(
                Clause::new("=")
                    .arg(Arg::Clause(Clause::field(&groups)))
                    .arg(Arg::Int(1)),
            );
        }
        if let Some(min) = s.min {
            let bound = match min {
                IntSpec::Literal(n) => Arg::Int(n),
                IntSpec::Param(c) => Arg::Clause(c),
            };
            set.where_.push(
                Clause::new(">=")
                    .arg(Arg::Clause(Clause::field(&largest)))
                    .arg(bound),
            );
        }
    }
    Ok(())
}
