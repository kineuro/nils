// SPDX-License-Identifier: AGPL-3.0-only

//! The pack's expression language (record 57 P2; spec section 5.3): a
//! small language with no side effects. Comparisons (`==`, `!=`, `<`,
//! `<=`, `>`, `>=`, `in`), `and`, `or`, `not`, and the named tests
//! `present(role)`, `aligned(a, b)`, `min(...)` and `max(...)`. No loops,
//! no user functions, no arithmetic beyond `min` and `max`.
//!
//! A fact is written `<role>.<fact>` (`t1w.dimension`); inside a role's own
//! needs a bare fact is that role's, which [`Expr::scoped`] resolves once
//! against the vocabulary. The person's `goal` and the pack's switches, a
//! session's `age_years`, a cohort's `diagnosis` and the host's `host.*`
//! facts are bare names of the vocabulary.
//!
//! [`parse`] reads the text, [`check`] types it against a [`Vocabulary`]
//! (an unknown fact, a wrong type or a text outside a fact's vocabulary is
//! refused, so a pack that holds one is refused at load), and [`eval`] is
//! the one evaluator every reader uses. A fact the registry cannot tell is
//! [`Val::Unknown`], and a comparison over it is unknown too: `and`, `or`
//! and `not` follow Kleene's three values, so `not present(flair) and
//! t1w.dimension == '3D'` is false where the FLAIR is present whatever the
//! T1w's dimension says.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use super::facts::{FactType, Facts, Val, Vocabulary};

/// A comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn text(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }

    /// Whether it orders (needs numbers) rather than tells apart.
    fn orders(self) -> bool {
        !matches!(self, Op::Eq | Op::Ne)
    }
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A fact by name, as written (`t1w.dimension`, or a bare name).
    Fact(String),
    Text(String),
    Number(f64),
    Bool(bool),
    List(Vec<Expr>),
    Cmp(Op, Box<Expr>, Box<Expr>),
    In(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Present(String),
    Aligned(String, String),
    Min(Vec<Expr>),
    Max(Vec<Expr>),
}

/// The type an expression has, as the checker tells it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Number,
    Text,
    Bool,
    List(Box<Ty>),
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Number => f.write_str("a number"),
            Ty::Text => f.write_str("text"),
            Ty::Bool => f.write_str("true or false"),
            Ty::List(t) => write!(f, "a list of {t}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Number(f64),
    Text(String),
    Punct(&'static str),
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word.ends_with('.') || word.contains("..") {
                return Err(format!("`{word}` is not a fact: a fact is role.fact"));
            }
            out.push(Tok::Ident(word));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let n: f64 = word
                .parse()
                .map_err(|_| format!("`{word}` is not a number"))?;
            out.push(Tok::Number(n));
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            if i >= chars.len() {
                return Err("a quoted text is not closed".into());
            }
            out.push(Tok::Text(chars[start..i].iter().collect()));
            i += 1;
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        let punct = match two.as_str() {
            "==" => Some("=="),
            "!=" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            _ => None,
        };
        if let Some(p) = punct {
            out.push(Tok::Punct(p));
            i += 2;
            continue;
        }
        let one = match c {
            '(' => "(",
            ')' => ")",
            ',' => ",",
            '[' => "[",
            ']' => "]",
            '<' => "<",
            '>' => ">",
            '=' => return Err("`=` is not a comparison; write `==`".into()),
            other => return Err(format!("`{other}` has no meaning here")),
        };
        out.push(Tok::Punct(one));
        i += 1;
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::Punct(q)) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(q)) if q == w)
    }

    fn expect(&mut self, p: &str) -> Result<(), String> {
        if self.is_punct(p) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected `{p}`{}", self.here()))
        }
    }

    fn here(&self) -> String {
        match self.peek() {
            None => " at the end".into(),
            Some(Tok::Ident(w)) => format!(" before `{w}`"),
            Some(Tok::Number(n)) => format!(" before `{n}`"),
            Some(Tok::Text(t)) => format!(" before '{t}'"),
            Some(Tok::Punct(p)) => format!(" before `{p}`"),
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut left = self.and()?;
        while self.is_word("or") {
            self.at += 1;
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut left = self.not()?;
        while self.is_word("and") {
            self.at += 1;
            let right = self.not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.is_word("not") {
            self.at += 1;
            let e = self.not()?;
            return Ok(Expr::Not(Box::new(e)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let left = self.primary()?;
        let op = match self.peek() {
            Some(Tok::Punct("==")) => Some(Op::Eq),
            Some(Tok::Punct("!=")) => Some(Op::Ne),
            Some(Tok::Punct("<")) => Some(Op::Lt),
            Some(Tok::Punct("<=")) => Some(Op::Le),
            Some(Tok::Punct(">")) => Some(Op::Gt),
            Some(Tok::Punct(">=")) => Some(Op::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.at += 1;
            let right = self.primary()?;
            return Ok(Expr::Cmp(op, Box::new(left), Box::new(right)));
        }
        if self.is_word("in") {
            self.at += 1;
            let right = self.primary()?;
            if !matches!(right, Expr::List(_)) {
                return Err("`in` takes a list: `x in ['a', 'b']`".into());
            }
            return Ok(Expr::In(Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn args(&mut self) -> Result<Vec<Expr>, String> {
        self.expect("(")?;
        let mut out = Vec::new();
        if self.is_punct(")") {
            self.at += 1;
            return Ok(out);
        }
        loop {
            out.push(self.or()?);
            if self.is_punct(",") {
                self.at += 1;
                continue;
            }
            self.expect(")")?;
            return Ok(out);
        }
    }

    fn role_arg(e: &Expr, what: &str) -> Result<String, String> {
        match e {
            Expr::Fact(name) if !name.contains('.') => Ok(name.clone()),
            _ => Err(format!("{what} takes a role by name, such as t1w")),
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            None => Err("the expression ends early".into()),
            Some(Tok::Number(n)) => Ok(Expr::Number(n)),
            Some(Tok::Text(t)) => Ok(Expr::Text(t)),
            Some(Tok::Punct("(")) => {
                let e = self.or()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Punct("[")) => {
                let mut items = Vec::new();
                if self.is_punct("]") {
                    self.at += 1;
                    return Ok(Expr::List(items));
                }
                loop {
                    items.push(self.primary()?);
                    if self.is_punct(",") {
                        self.at += 1;
                        continue;
                    }
                    self.expect("]")?;
                    return Ok(Expr::List(items));
                }
            }
            Some(Tok::Punct(p)) => Err(format!("`{p}` cannot start a value")),
            Some(Tok::Ident(w)) => match w.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "and" | "or" | "not" | "in" => Err(format!("`{w}` is not a value")),
                "present" | "aligned" | "min" | "max" => {
                    let args = self.args()?;
                    match w.as_str() {
                        "present" => {
                            if args.len() != 1 {
                                return Err("present(role) takes one role".into());
                            }
                            Ok(Expr::Present(Self::role_arg(&args[0], "present")?))
                        }
                        "aligned" => {
                            if args.len() != 2 {
                                return Err("aligned(a, b) takes two roles".into());
                            }
                            Ok(Expr::Aligned(
                                Self::role_arg(&args[0], "aligned")?,
                                Self::role_arg(&args[1], "aligned")?,
                            ))
                        }
                        "min" => {
                            if args.len() < 2 {
                                return Err("min takes two values or more".into());
                            }
                            Ok(Expr::Min(args))
                        }
                        _ => {
                            if args.len() < 2 {
                                return Err("max takes two values or more".into());
                            }
                            Ok(Expr::Max(args))
                        }
                    }
                }
                _ => {
                    if self.is_punct("(") {
                        return Err(format!(
                            "`{w}` is not a function; the language has present, aligned, min and max"
                        ));
                    }
                    Ok(Expr::Fact(w))
                }
            },
        }
    }
}

/// Parse an expression, or say what is wrong with it.
pub fn parse(text: &str) -> Result<Expr, String> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err("the expression is empty".into());
    }
    let mut p = Parser { toks, at: 0 };
    let e = p.or()?;
    if p.at < p.toks.len() {
        return Err(format!("unexpected text{}", p.here()));
    }
    Ok(e)
}

impl Expr {
    /// The expression with every bare fact resolved against the
    /// vocabulary under `role`: a bare name that is a fact of the role is
    /// written `<role>.<name>`; one the vocabulary knows bare (`goal`, a
    /// switch, `age_years`) stays; any other stays too, and the checker
    /// names it unknown.
    pub fn scoped(&self, role: Option<&str>, vocab: &Vocabulary) -> Expr {
        let s = |e: &Expr| e.scoped(role, vocab);
        match self {
            Expr::Fact(name) => {
                if name.contains('.') || vocab.facts.contains_key(name) {
                    return Expr::Fact(name.clone());
                }
                if let Some(r) = role {
                    let full = format!("{r}.{name}");
                    if vocab.facts.contains_key(&full) {
                        return Expr::Fact(full);
                    }
                }
                Expr::Fact(name.clone())
            }
            Expr::List(items) => Expr::List(items.iter().map(s).collect()),
            Expr::Cmp(op, a, b) => Expr::Cmp(*op, Box::new(s(a)), Box::new(s(b))),
            Expr::In(a, b) => Expr::In(Box::new(s(a)), Box::new(s(b))),
            Expr::And(a, b) => Expr::And(Box::new(s(a)), Box::new(s(b))),
            Expr::Or(a, b) => Expr::Or(Box::new(s(a)), Box::new(s(b))),
            Expr::Not(a) => Expr::Not(Box::new(s(a))),
            Expr::Min(items) => Expr::Min(items.iter().map(s).collect()),
            Expr::Max(items) => Expr::Max(items.iter().map(s).collect()),
            other => other.clone(),
        }
    }

    /// Every fact the expression reads, by its resolved name, and the roles
    /// `present` and `aligned` ask about as `present:<role>` and
    /// `aligned:<a>:<b>`.
    pub fn reads(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut BTreeSet<String>) {
        match self {
            Expr::Fact(name) => {
                out.insert(name.clone());
            }
            Expr::Present(r) => {
                out.insert(format!("present:{r}"));
            }
            Expr::Aligned(a, b) => {
                out.insert(format!("aligned:{a}:{b}"));
            }
            Expr::List(items) | Expr::Min(items) | Expr::Max(items) => {
                items.iter().for_each(|i| i.collect(out));
            }
            Expr::Cmp(_, a, b) | Expr::In(a, b) | Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect(out);
                b.collect(out);
            }
            Expr::Not(a) => a.collect(out),
            Expr::Text(_) | Expr::Number(_) | Expr::Bool(_) => {}
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Fact(n) => f.write_str(n),
            Expr::Text(t) => write!(f, "'{t}'"),
            Expr::Number(n) => write!(f, "{}", super::super::descriptor::number_text(*n)),
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::List(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            Expr::Cmp(op, a, b) => write!(f, "{a} {} {b}", op.text()),
            Expr::In(a, b) => write!(f, "{a} in {b}"),
            Expr::And(a, b) => write!(f, "({a} and {b})"),
            Expr::Or(a, b) => write!(f, "({a} or {b})"),
            Expr::Not(a) => write!(f, "not {a}"),
            Expr::Present(r) => write!(f, "present({r})"),
            Expr::Aligned(a, b) => write!(f, "aligned({a}, {b})"),
            Expr::Min(items) | Expr::Max(items) => {
                f.write_str(if matches!(self, Expr::Min(_)) {
                    "min("
                } else {
                    "max("
                })?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str(")")
            }
        }
    }
}

/// Type-check an expression against the vocabulary: its type, or why it is
/// refused (an unknown fact, a wrong type, a text outside a fact's
/// vocabulary, a role `present` or `aligned` does not know).
pub fn check(e: &Expr, vocab: &Vocabulary) -> Result<Ty, String> {
    match e {
        Expr::Fact(name) => match vocab.facts.get(name) {
            Some(FactType::Number) => Ok(Ty::Number),
            Some(FactType::Bool) => Ok(Ty::Bool),
            Some(FactType::Text(_)) => Ok(Ty::Text),
            None => Err(format!(
                "unknown fact `{name}`; the facts are {}",
                vocab.facts_named()
            )),
        },
        Expr::Text(_) => Ok(Ty::Text),
        Expr::Number(_) => Ok(Ty::Number),
        Expr::Bool(_) => Ok(Ty::Bool),
        Expr::List(items) => {
            let mut ty: Option<Ty> = None;
            for item in items {
                let t = check(item, vocab)?;
                if matches!(t, Ty::List(_)) {
                    return Err("a list holds values, not lists".into());
                }
                match &ty {
                    None => ty = Some(t),
                    Some(have) if *have != t => {
                        return Err(format!("a list holds values of one type; this one holds {have} and {t}"));
                    }
                    _ => {}
                }
            }
            Ok(Ty::List(Box::new(ty.unwrap_or(Ty::Text))))
        }
        Expr::Cmp(op, a, b) => {
            let (ta, tb) = (check(a, vocab)?, check(b, vocab)?);
            if matches!(ta, Ty::List(_)) || matches!(tb, Ty::List(_)) {
                return Err(format!("`{}` compares values, not lists", op.text()));
            }
            if ta != tb {
                return Err(format!(
                    "`{a} {} {b}` compares {ta} with {tb}",
                    op.text()
                ));
            }
            if op.orders() && ta != Ty::Number {
                return Err(format!(
                    "`{}` orders numbers, and `{a}` is {ta}",
                    op.text()
                ));
            }
            // a text compared to a fact with a vocabulary is one of its words
            in_vocabulary(a, b, vocab)?;
            in_vocabulary(b, a, vocab)?;
            Ok(Ty::Bool)
        }
        Expr::In(a, b) => {
            let ta = check(a, vocab)?;
            let Ty::List(inner) = check(b, vocab)? else {
                return Err("`in` takes a list on its right".into());
            };
            if let Expr::List(items) = b.as_ref()
                && !items.is_empty()
                && *inner != ta
            {
                return Err(format!("`{a} in {b}`: `{a}` is {ta}, the list holds {inner}"));
            }
            if let Expr::List(items) = b.as_ref() {
                for item in items {
                    in_vocabulary(a, item, vocab)?;
                }
            }
            Ok(Ty::Bool)
        }
        Expr::And(a, b) | Expr::Or(a, b) => {
            let word = if matches!(e, Expr::And(..)) { "and" } else { "or" };
            for side in [a, b] {
                let t = check(side, vocab)?;
                if t != Ty::Bool {
                    return Err(format!("`{word}` joins true-or-false values, and `{side}` is {t}"));
                }
            }
            Ok(Ty::Bool)
        }
        Expr::Not(a) => {
            let t = check(a, vocab)?;
            if t != Ty::Bool {
                return Err(format!("`not` takes a true-or-false value, and `{a}` is {t}"));
            }
            Ok(Ty::Bool)
        }
        Expr::Present(r) => {
            vocab.role(r)?;
            Ok(Ty::Bool)
        }
        Expr::Aligned(a, b) => {
            vocab.role(a)?;
            vocab.role(b)?;
            if a == b {
                return Err(format!("aligned({a}, {b}) names one role twice"));
            }
            Ok(Ty::Bool)
        }
        Expr::Min(items) | Expr::Max(items) => {
            let word = if matches!(e, Expr::Min(_)) { "min" } else { "max" };
            for item in items {
                let t = check(item, vocab)?;
                if t != Ty::Number {
                    return Err(format!("`{word}` takes numbers, and `{item}` is {t}"));
                }
            }
            Ok(Ty::Number)
        }
    }
}

/// Where `fact` is a fact with a vocabulary and `lit` a text, the text is
/// one of the fact's words, else the pack is refused.
fn in_vocabulary(fact: &Expr, lit: &Expr, vocab: &Vocabulary) -> Result<(), String> {
    if let (Expr::Fact(name), Expr::Text(t)) = (fact, lit)
        && let Some(FactType::Text(Some(words))) = vocab.facts.get(name)
        && !words.iter().any(|w| w == t)
    {
        return Err(format!(
            "'{t}' is outside the vocabulary of {name}, which is {}",
            words.join(", ")
        ));
    }
    Ok(())
}

/// Evaluate an expression over a unit's facts. A fact the registry cannot
/// tell is [`Val::Unknown`], and so is any comparison over it; `and`,
/// `or` and `not` follow Kleene's three values.
pub fn eval(e: &Expr, facts: &Facts) -> Val {
    match e {
        Expr::Fact(name) => facts.get(name),
        Expr::Text(t) => Val::Text(t.clone()),
        Expr::Number(n) => Val::Number(*n),
        Expr::Bool(b) => Val::Bool(*b),
        Expr::List(items) => Val::List(items.iter().map(|i| eval(i, facts)).collect()),
        Expr::Cmp(op, a, b) => {
            let (va, vb) = (eval(a, facts), eval(b, facts));
            match (va, vb) {
                (Val::Unknown, _) | (_, Val::Unknown) => Val::Unknown,
                (Val::Number(x), Val::Number(y)) => Val::Bool(match op {
                    Op::Eq => x == y,
                    Op::Ne => x != y,
                    Op::Lt => x < y,
                    Op::Le => x <= y,
                    Op::Gt => x > y,
                    Op::Ge => x >= y,
                }),
                (x, y) => match op {
                    Op::Eq => Val::Bool(x == y),
                    Op::Ne => Val::Bool(x != y),
                    _ => Val::Unknown,
                },
            }
        }
        Expr::In(a, b) => {
            let va = eval(a, facts);
            if va == Val::Unknown {
                return Val::Unknown;
            }
            match eval(b, facts) {
                Val::List(items) => {
                    if items.iter().any(|i| *i == Val::Unknown) {
                        return Val::Unknown;
                    }
                    Val::Bool(items.contains(&va))
                }
                _ => Val::Unknown,
            }
        }
        Expr::And(a, b) => match (eval(a, facts), eval(b, facts)) {
            (Val::Bool(false), _) | (_, Val::Bool(false)) => Val::Bool(false),
            (Val::Bool(true), Val::Bool(true)) => Val::Bool(true),
            _ => Val::Unknown,
        },
        Expr::Or(a, b) => match (eval(a, facts), eval(b, facts)) {
            (Val::Bool(true), _) | (_, Val::Bool(true)) => Val::Bool(true),
            (Val::Bool(false), Val::Bool(false)) => Val::Bool(false),
            _ => Val::Unknown,
        },
        Expr::Not(a) => match eval(a, facts) {
            Val::Bool(b) => Val::Bool(!b),
            _ => Val::Unknown,
        },
        Expr::Present(r) => Val::Bool(facts.present(r)),
        Expr::Aligned(a, b) => facts.aligned(a, b),
        Expr::Min(items) | Expr::Max(items) => {
            let mut best: Option<f64> = None;
            for item in items {
                let Val::Number(n) = eval(item, facts) else {
                    return Val::Unknown;
                };
                best = Some(match best {
                    None => n,
                    Some(b) if matches!(e, Expr::Min(_)) => b.min(n),
                    Some(b) => b.max(n),
                });
            }
            best.map_or(Val::Unknown, Val::Number)
        }
    }
}

/// The facts an expression read, each with the value it had, as a unit
/// records them beside the rule that read them (spec section 5.5).
pub fn read(e: &Expr, facts: &Facts) -> BTreeMap<String, Value> {
    e.reads()
        .into_iter()
        .map(|name| {
            let v = if let Some(r) = name.strip_prefix("present:") {
                Val::Bool(facts.present(r))
            } else if let Some(pair) = name.strip_prefix("aligned:") {
                match pair.split_once(':') {
                    Some((a, b)) => facts.aligned(a, b),
                    None => Val::Unknown,
                }
            } else {
                facts.get(&name)
            };
            (name, v.to_json())
        })
        .collect()
}

/// The first fact an expression reads that the facts cannot tell, for the
/// words "cannot tell: <fact>".
pub fn first_unknown(e: &Expr, facts: &Facts) -> Option<String> {
    e.reads().into_iter().find(|name| {
        if name.starts_with("present:") {
            return false;
        }
        if let Some(pair) = name.strip_prefix("aligned:") {
            return pair
                .split_once(':')
                .is_none_or(|(a, b)| facts.aligned(a, b) == Val::Unknown);
        }
        facts.get(name) == Val::Unknown
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::facts::Goals;

    fn vocab() -> Vocabulary {
        Vocabulary::new(
            &["t1w".into(), "flair".into()],
            &Goals {
                goal: vec!["fast".into(), "standard".into(), "research".into()],
                switches: [("lesions".to_string(), vec![Value::from("on"), Value::from("off")])]
                    .into_iter()
                    .collect(),
            },
        )
    }

    fn facts() -> Facts {
        let mut f = Facts::default();
        f.set("t1w.dimension", Val::Text("3D".into()));
        f.set("t1w.slice_thickness_mm", Val::Number(1.0));
        f.set("flair.dimension", Val::Text("2D".into()));
        f.set("goal", Val::Text("standard".into()));
        f.set("lesions", Val::Text("on".into()));
        f.roles.insert("t1w".into());
        f.roles.insert("flair".into());
        f
    }

    fn good(text: &str) -> Val {
        let e = parse(text).unwrap();
        let e = e.scoped(None, &vocab());
        check(&e, &vocab()).unwrap_or_else(|err| panic!("{text}: {err}"));
        eval(&e, &facts())
    }

    #[test]
    fn good_expressions_parse_check_and_evaluate() {
        assert_eq!(good("t1w.dimension == '3D'"), Val::Bool(true));
        assert_eq!(good("t1w.slice_thickness_mm <= 1.5"), Val::Bool(true));
        assert_eq!(good("present(flair) and not present(t2w)"), Val::Bool(true));
        assert_eq!(good("lesions == 'on'"), Val::Bool(true));
        assert_eq!(good("goal in ['fast', 'standard']"), Val::Bool(true));
        assert_eq!(
            good("min(t1w.slice_thickness_mm, 2) < max(t1w.slice_thickness_mm, 3)"),
            Val::Bool(true)
        );
        assert_eq!(good("(flair.dimension == '3D') or t1w.dimension == '3D'"), Val::Bool(true));
        // unknown propagates, and Kleene's values decide what it can
        assert_eq!(good("t1w.field_strength_t >= 3"), Val::Unknown);
        assert_eq!(good("t1w.field_strength_t >= 3 and present(t2w)"), Val::Bool(false));
        assert_eq!(good("t1w.field_strength_t >= 3 or present(t1w)"), Val::Bool(true));
        assert_eq!(good("not t1w.field_strength_t >= 3"), Val::Unknown);
        assert_eq!(good("aligned(flair, t1w)"), Val::Unknown);
        let e = parse("t1w.dimension == '3D'").unwrap();
        assert_eq!(e.to_string(), "t1w.dimension == '3D'");
        assert_eq!(
            read(&e, &facts()),
            [("t1w.dimension".to_string(), Value::from("3D"))]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_bare_fact_in_a_role_s_needs_is_that_role_s() {
        let e = parse("dimension == '3D' and slice_thickness_mm <= 1.5")
            .unwrap()
            .scoped(Some("t1w"), &vocab());
        assert_eq!(check(&e, &vocab()).unwrap(), Ty::Bool);
        assert_eq!(e.to_string(), "(t1w.dimension == '3D' and t1w.slice_thickness_mm <= 1.5)");
        assert_eq!(eval(&e, &facts()), Val::Bool(true));
    }

    fn bad(text: &str) -> String {
        let e = match parse(text) {
            Ok(e) => e.scoped(None, &vocab()),
            Err(err) => return err,
        };
        check(&e, &vocab()).err().unwrap_or_else(|| panic!("{text} was taken"))
    }

    #[test]
    fn bad_expressions_are_refused_with_words() {
        assert!(bad("t1w.colour == 'red'").contains("unknown fact `t1w.colour`"));
        assert!(bad("t1w.dimension == 3").contains("compares text with a number"));
        assert!(bad("t1w.dimension == '4D'").contains("outside the vocabulary of t1w.dimension"));
        assert!(bad("t1w.dimension < '3D'").contains("orders numbers"));
        assert!(bad("present(t1w) and 3").contains("joins true-or-false values"));
        assert!(bad("present(elbow)").contains("elbow"));
        assert!(bad("aligned(t1w, t1w)").contains("twice"));
        assert!(bad("t1w.dimension = '3D'").contains("write `==`"));
        assert!(bad("foo(1)").contains("not a function"));
        assert!(bad("goal in ['fast', 'slow']").contains("outside the vocabulary of goal"));
        assert!(bad("min(t1w.dimension, 1)").contains("takes numbers"));
        assert!(bad("").contains("empty"));
        assert!(bad("t1w.dimension == '3D' extra").contains("unexpected text"));
    }
}
