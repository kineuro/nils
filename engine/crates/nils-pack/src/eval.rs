// SPDX-License-Identifier: AGPL-3.0-only

//! Evaluating a pack against one stack: the parsers, then the flags.
//!
//! Nothing is stored. Deriving 220 predicates and 145 flags for the whole
//! live corpus costs 5.5 seconds on one core (`spikes/pack/README.md`), which
//! is why a flag lives in a pack and not in a column.

use std::collections::HashSet;

use regex::Regex;

use crate::expr::{Ctx, Subject};
use crate::pack::Pack;
use crate::stack::Stack;

/// One stack, mid-evaluation.
pub struct Evaluated<'a> {
    pack: &'a Pack,
    stack: &'a Stack,
    raws: Vec<String>,
    tokens: Vec<HashSet<String>>,
    preds: Vec<Vec<bool>>,
    flags: Vec<bool>,
}

impl<'a> Evaluated<'a> {
    pub fn new(pack: &'a Pack, stack: &'a Stack) -> Evaluated<'a> {
        let mut raws = Vec::with_capacity(pack.parsers.len());
        let mut tokens = Vec::with_capacity(pack.parsers.len());
        for p in &pack.parsers {
            let raw = p.case.apply(stack.text(p.field)).into_owned();
            let mut set = HashSet::new();
            if let Some(split) = &p.split {
                let stripped = match &p.strip {
                    Some(s) => s.replace_all(&raw, "").into_owned(),
                    None => raw.clone(),
                };
                for t in split.split(&stripped) {
                    if !t.is_empty() {
                        set.insert(t.to_string());
                    }
                }
            }
            raws.push(raw);
            tokens.push(set);
        }
        let mut e = Evaluated {
            pack,
            stack,
            raws,
            tokens,
            preds: Vec::with_capacity(pack.parsers.len()),
            flags: vec![false; pack.flags.len()],
        };
        // Predicates, parser by parser, in file order. `preds` grows as it
        // goes, so a predicate may name an earlier one; a forward reference
        // reads false, which the loader is what stops.
        for (pi, p) in pack.parsers.iter().enumerate() {
            e.preds.push(Vec::with_capacity(p.preds.len()));
            for expr in &p.preds {
                let v = {
                    let subj = Subject {
                        raw: &e.raws[pi],
                        tokens: Some(&e.tokens[pi]),
                    };
                    expr.eval(Some(&subj), &e)
                };
                e.preds[pi].push(v);
            }
        }
        for i in &pack.flag_order {
            let v = pack.flags[*i].eval(None, &e);
            e.flags[*i] = v;
        }
        e
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        self.pack.flag_index(name).map(|i| self.flags[i])
    }

    /// The flags that hold, by name, in the pack's declared order.
    pub fn flags_on(&self) -> Vec<&str> {
        self.pack
            .flag_names
            .iter()
            .enumerate()
            .filter(|(i, _)| self.flags[*i])
            .map(|(_, n)| n.as_str())
            .collect()
    }

    pub fn predicate(&self, parser: &str, name: &str) -> Option<bool> {
        let pi = self.pack.parser_index(parser)?;
        let px = self.pack.parsers[pi]
            .pred_names
            .iter()
            .position(|n| n == name)?;
        Some(self.preds[pi][px])
    }
}

impl Ctx for Evaluated<'_> {
    fn pred(&self, parser: usize, pred: usize) -> bool {
        self.preds
            .get(parser)
            .and_then(|v| v.get(pred))
            .copied()
            .unwrap_or(false)
    }
    fn subject(&self, parser: usize) -> Subject<'_> {
        Subject {
            raw: &self.raws[parser],
            tokens: Some(&self.tokens[parser]),
        }
    }
    fn flag(&self, flag: usize) -> bool {
        self.flags[flag]
    }
    fn num(&self, field: usize) -> Option<f64> {
        self.stack.num(field)
    }
    fn present(&self, field: usize) -> bool {
        self.stack.present(field)
    }
    fn text(&self, field: usize) -> &str {
        self.stack.text(field)
    }
    fn re(&self, idx: usize) -> &Regex {
        &self.pack.regexes[idx]
    }
}
