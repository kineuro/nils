// SPDX-License-Identifier: AGPL-3.0-only
//! The pack expression language.
//!
//! One enum, evaluated against one stack. Everything a v0 predicate, flag,
//! branch clause or pass filter does is one of these; the point of the spike
//! is that this list stopped growing before v0's grammar ran out.

use regex::Regex;
use std::collections::HashSet;

/// A parser's view of one field: the case-folded value and, unless the parser
/// declines to tokenize, its token set. The two are separate on purpose:
/// a backslash survives in `raw` and never in a token.
pub struct Subject<'a> {
    pub raw: &'a str,
    pub tokens: Option<&'a HashSet<String>>,
}

impl<'a> Subject<'a> {
    pub fn text(raw: &'a str) -> Subject<'a> {
        Subject { raw, tokens: None }
    }
}

/// How a text field is folded before an atom reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Case {
    Raw,
    Lower,
    Upper,
}

impl Case {
    pub fn parse(s: &str) -> Option<Case> {
        Some(match s {
            "raw" => Case::Raw,
            "lower" => Case::Lower,
            "upper" => Case::Upper,
            _ => return None,
        })
    }
    pub fn apply<'a>(self, s: &'a str) -> std::borrow::Cow<'a, str> {
        match self {
            Case::Raw => std::borrow::Cow::Borrowed(s),
            Case::Lower => std::borrow::Cow::Owned(s.to_lowercase()),
            Case::Upper => std::borrow::Cow::Owned(s.to_uppercase()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl NumOp {
    pub fn parse(s: &str) -> Option<NumOp> {
        Some(match s {
            "eq" => NumOp::Eq,
            "ne" => NumOp::Ne,
            "lt" => NumOp::Lt,
            "le" => NumOp::Le,
            "gt" => NumOp::Gt,
            "ge" => NumOp::Ge,
            _ => return None,
        })
    }
    pub fn apply(self, a: f64, b: f64) -> bool {
        match self {
            NumOp::Eq => a == b,
            NumOp::Ne => a != b,
            NumOp::Lt => a < b,
            NumOp::Le => a <= b,
            NumOp::Gt => a > b,
            NumOp::Ge => a >= b,
        }
    }
}

/// What a scalar comparison compares against.
#[derive(Clone, Debug)]
pub enum Cmp {
    Num(NumOp, f64),
    Str(bool, String),
    /// `{present: true}` / `{present: false}`
    Present(bool),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Lit(bool),

    // --- against a Subject (a parser's field, or a text field) ---
    Token(String),
    AnyToken(Vec<String>),
    AllTokens(Vec<String>),
    TokenCount(NumOp, usize),
    Substring(String),
    AnySubstring(Vec<String>),
    Prefix {
        s: String,
        trim_start: bool,
    },
    Equals(String),
    Matches(usize),
    /// The subject is empty.
    Empty,

    // --- against the stack ---
    /// A named predicate of a parser: `image_type.is_original`.
    Pred {
        parser: usize,
        pred: usize,
    },
    /// An inline atom evaluated against a parser's subject: the two places v0
    /// reaches past its own named predicates into the token set.
    InParser {
        parser: usize,
        inner: Box<Expr>,
    },
    /// Another flag, by bare name.
    Flag(usize),
    /// A comparison on a fingerprint scalar.
    Field {
        field: usize,
        cmp: Cmp,
    },
    /// A match against a text field, in the case the atom asks for.
    Text {
        field: usize,
        case: Case,
        inner: Box<Expr>,
    },

    // --- against the candidate a pass is testing, not the stack ---
    /// The candidate's family, from the pass's own table.
    Family(String),
    /// There is no candidate.
    CandidateEmpty,

    Any(Vec<Expr>),
    All(Vec<Expr>),
    Not(Box<Expr>),
}

/// What the evaluator can see while it works on one stack.
pub trait Ctx {
    fn pred(&self, parser: usize, pred: usize) -> bool;
    fn subject(&self, parser: usize) -> Subject<'_>;
    fn flag(&self, flag: usize) -> bool;
    /// The scalar as a number, when it is one.
    fn num(&self, field: usize) -> Option<f64>;
    /// The scalar as text, when it is present and not empty.
    fn scalar(&self, field: usize) -> Option<&str>;
    fn text(&self, field: usize, case: Case) -> std::borrow::Cow<'_, str>;
    fn re(&self, idx: usize) -> &Regex;
    /// The candidate answer a pass is testing, empty when there is none.
    fn candidate(&self) -> &str {
        ""
    }
    fn candidate_family(&self) -> &str {
        ""
    }
}

impl Expr {
    /// Evaluate. `subj` is the subject in scope, if any: a parser's field
    /// while its predicates are being evaluated, or the field a `text:` atom
    /// opened. A subject atom with no subject in scope is false; the loader
    /// rejects that case, so it cannot happen at run time.
    pub fn eval<C: Ctx>(&self, subj: Option<&Subject<'_>>, c: &C) -> bool {
        match self {
            Expr::Lit(b) => *b,

            Expr::Token(t) => subj
                .and_then(|s| s.tokens)
                .is_some_and(|set| set.contains(t)),
            Expr::AnyToken(ts) => subj
                .and_then(|s| s.tokens)
                .is_some_and(|set| ts.iter().any(|t| set.contains(t))),
            Expr::AllTokens(ts) => subj
                .and_then(|s| s.tokens)
                .is_some_and(|set| ts.iter().all(|t| set.contains(t))),
            Expr::TokenCount(op, n) => subj
                .and_then(|s| s.tokens)
                .is_some_and(|set| op.apply(set.len() as f64, *n as f64)),

            Expr::Substring(t) => subj.is_some_and(|s| s.raw.contains(t.as_str())),
            Expr::AnySubstring(ts) => {
                subj.is_some_and(|s| ts.iter().any(|t| s.raw.contains(t.as_str())))
            }
            Expr::Prefix { s: p, trim_start } => subj.is_some_and(|s| {
                let raw = if *trim_start { s.raw.trim_start() } else { s.raw };
                raw.starts_with(p.as_str())
            }),
            Expr::Equals(t) => subj.is_some_and(|s| s.raw == t),
            Expr::Matches(i) => subj.is_some_and(|s| c.re(*i).is_match(s.raw)),
            Expr::Empty => subj.is_some_and(|s| s.raw.is_empty()),

            Expr::Pred { parser, pred } => c.pred(*parser, *pred),
            Expr::InParser { parser, inner } => {
                let s = c.subject(*parser);
                inner.eval(Some(&s), c)
            }
            Expr::Flag(i) => c.flag(*i),

            Expr::Field { field, cmp } => match cmp {
                // A missing or unparsable value makes a comparison false,
                // never an error. v0 does the same, by try/except.
                Cmp::Num(op, v) => c.num(*field).is_some_and(|x| op.apply(x, *v)),
                Cmp::Str(want, v) => {
                    let got = c.scalar(*field);
                    (got == Some(v.as_str())) == *want
                }
                Cmp::Present(want) => c.scalar(*field).is_some() == *want,
            },

            Expr::Text { field, case, inner } => {
                let t = c.text(*field, *case);
                inner.eval(Some(&Subject::text(t.as_ref())), c)
            }

            Expr::Family(f) => c.candidate_family() == f,
            Expr::CandidateEmpty => c.candidate().is_empty(),

            Expr::Any(xs) => xs.iter().any(|x| x.eval(subj, c)),
            Expr::All(xs) => xs.iter().all(|x| x.eval(subj, c)),
            Expr::Not(x) => !x.eval(subj, c),
        }
    }
}
