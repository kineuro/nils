// SPDX-License-Identifier: AGPL-3.0-only
//! Loading a pack: YAML in, a compiled pack out, with the line that is wrong
//! named when it will not load.

use crate::expr::{Case, Cmp, Expr, NumOp};
use crate::row::{FIELDS, field_index};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub type R<T> = Result<T, String>;

pub struct ParserDef {
    pub name: String,
    pub field: usize,
    pub case: Case,
    pub strip: Option<Regex>,
    pub split: Option<Regex>,
    pub pred_names: Vec<String>,
    pub preds: Vec<Expr>,
}

pub struct Clause {
    pub cite: String,
    pub source: String,
    pub when: Expr,
}

/// One member of a multi-valued axis: a value, kept when its condition holds.
pub struct SetVal {
    pub value: String,
    pub when: Option<Expr>,
}

pub enum Slot {
    Literal(String),
    /// `{from: name}`: a value the branch derived for this stack.
    Derived(usize),
}

pub struct BranchRule {
    pub id: String,
    pub clauses: Vec<Clause>,
    pub base: Option<Slot>,
    pub technique: Option<Slot>,
    pub directory_type: Option<Slot>,
    pub construct: Vec<SetVal>,
    pub confidence: f64,
}

pub struct DeriveCase {
    pub when: Option<Expr>,
    pub value: String,
}

pub struct Branch {
    pub name: String,
    pub enter_provenance: String,
    pub derive_names: Vec<String>,
    pub derives: Vec<Vec<DeriveCase>>,
    pub rules: Vec<BranchRule>,
}

pub struct KeyDim {
    pub name: String,
    pub field: usize,
    pub round: Option<f64>,
    pub ceil: Option<f64>,
    pub half_even: bool,
}

pub struct CompatRule {
    pub when: Expr,
    pub allow: Expr,
}

pub struct Compat {
    pub subject_field: usize,
    pub subject_case: Case,
    pub default_family: String,
    pub family_of: HashMap<String, String>,
    pub rules: Vec<CompatRule>,
}

pub struct VotePass {
    pub name: String,
    pub dims: Vec<KeyDim>,
    pub max_distance: i64,
    pub pairs: Vec<(usize, usize)>,
    pub relaxed: Option<(usize, i64, usize, i64)>,
    pub min_matches: usize,
    pub partition_by: Option<String>,
    pub fallback_except: Vec<String>,
    pub compat: Compat,
}

pub struct Pack {
    pub name: String,
    pub version: String,
    pub parsers: Vec<ParserDef>,
    pub flag_names: Vec<String>,
    pub flags: Vec<Expr>,
    /// Flags in an order where every reference is already computed.
    pub flag_order: Vec<usize>,
    pub regexes: Vec<Regex>,
    pub branches: Vec<Branch>,
    pub passes: Vec<VotePass>,
}

// ---------------------------------------------------------------------------
// YAML helpers. Every one names the path it failed at.

fn yaml(path: &Path) -> R<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_saphyr::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn obj<'a>(v: &'a Value, at: &str) -> R<&'a serde_json::Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| format!("{at}: expected a mapping"))
}

fn strs(v: &Value, at: &str) -> R<Vec<String>> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => Ok(s.clone()),
                Value::Number(n) => Ok(n.to_string()),
                Value::Bool(b) => Ok(b.to_string()),
                _ => Err(format!("{at}: expected a string")),
            })
            .collect(),
        Value::Number(n) => Ok(vec![n.to_string()]),
        _ => Err(format!("{at}: expected a string or a list of strings")),
    }
}

fn one_str(v: &Value, at: &str) -> R<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(format!("{at}: expected a string")),
    }
}

fn f64_of(v: &Value, at: &str) -> R<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| format!("{at}: expected a number"))
}

// ---------------------------------------------------------------------------
// The compiler. `Scope` says which references are legal where.

struct Scope<'a> {
    parsers: &'a [ParserDef],
    parser_ix: &'a HashMap<String, usize>,
    /// The parser whose own predicates a bare name refers to.
    within: Option<usize>,
    /// Flag names, when flag references are legal.
    flags: Option<&'a HashMap<String, usize>>,
    /// Whether candidate atoms (`family`, `candidate_empty`) are legal.
    candidate: bool,
    regexes: &'a mut Vec<Regex>,
    /// Filled with every flag this expression depends on.
    deps: HashSet<usize>,
}

impl Scope<'_> {
    fn re(&mut self, pat: &str, at: &str) -> R<usize> {
        let r = Regex::new(pat).map_err(|e| format!("{at}: {pat}: {e}"))?;
        self.regexes.push(r);
        Ok(self.regexes.len() - 1)
    }
}

fn compile(v: &Value, at: &str, sc: &mut Scope) -> R<Expr> {
    match v {
        Value::Bool(b) => return Ok(Expr::Lit(*b)),
        Value::String(s) => return reference(s, at, sc),
        Value::Object(m) => {
            if m.len() != 1 {
                // A mapping with several keys is a comparison on one field, or
                // a mistake. `{field: ti, gt: 0}` is the only shape with two.
                return multi_key(m, at, sc);
            }
            let (k, val) = m.iter().next().unwrap();
            return single_key(k, val, at, sc);
        }
        _ => {}
    }
    Err(format!("{at}: expected a condition"))
}

fn reference(s: &str, at: &str, sc: &mut Scope) -> R<Expr> {
    if let Some((p, pred)) = s.split_once('.') {
        let pi = *sc
            .parser_ix
            .get(p)
            .ok_or_else(|| format!("{at}: no parser named {p}"))?;
        let px = sc.parsers[pi]
            .pred_names
            .iter()
            .position(|n| n == pred)
            .ok_or_else(|| format!("{at}: parser {p} has no predicate {pred}"))?;
        return Ok(Expr::Pred {
            parser: pi,
            pred: px,
        });
    }
    if let Some(within) = sc.within
        && let Some(px) = sc.parsers[within].pred_names.iter().position(|n| n == s)
    {
        return Ok(Expr::Pred {
            parser: within,
            pred: px,
        });
    }
    if let Some(flags) = sc.flags
        && let Some(fx) = flags.get(s)
    {
        sc.deps.insert(*fx);
        return Ok(Expr::Flag(*fx));
    }
    Err(format!("{at}: {s} is not a predicate or a flag"))
}

fn subject_atom(k: &str, val: &Value, at: &str, sc: &mut Scope) -> R<Option<Expr>> {
    let e = match k {
        "token" => Expr::Token(one_str(val, at)?),
        "any_token" => Expr::AnyToken(strs(val, at)?),
        "all_tokens" => Expr::AllTokens(strs(val, at)?),
        "substring" => Expr::Substring(one_str(val, at)?),
        "any_substring" => Expr::AnySubstring(strs(val, at)?),
        "equals" => Expr::Equals(one_str(val, at)?),
        "prefix" => Expr::Prefix {
            s: one_str(val, at)?,
            trim_start: false,
        },
        "matches" => {
            let i = sc.re(&one_str(val, at)?, at)?;
            Expr::Matches(i)
        }
        "empty" => {
            if val.as_bool() == Some(true) {
                Expr::Empty
            } else {
                Expr::Not(Box::new(Expr::Empty))
            }
        }
        "tokens" => {
            let m = obj(val, at)?;
            let (op, n) = m
                .iter()
                .next()
                .ok_or_else(|| format!("{at}: tokens needs a comparison"))?;
            let op = NumOp::parse(op).ok_or_else(|| format!("{at}: {op} is not a comparison"))?;
            Expr::TokenCount(op, f64_of(n, at)? as usize)
        }
        _ => return Ok(None),
    };
    Ok(Some(e))
}

fn single_key(k: &str, val: &Value, at: &str, sc: &mut Scope) -> R<Expr> {
    let here = format!("{at}.{k}");
    if let Some(e) = subject_atom(k, val, &here, sc)? {
        return Ok(e);
    }
    Ok(match k {
        "any" => Expr::Any(list(val, &here, sc)?),
        "all" => Expr::All(list(val, &here, sc)?),
        "not" => Expr::Not(Box::new(compile(val, &here, sc)?)),
        "family" => {
            if !sc.candidate {
                return Err(format!("{here}: family is only legal in a pass filter"));
            }
            Expr::Family(one_str(val, &here)?)
        }
        "candidate_empty" => {
            if !sc.candidate {
                return Err(format!(
                    "{here}: candidate_empty is only legal in a pass filter"
                ));
            }
            if val.as_bool() == Some(true) {
                Expr::CandidateEmpty
            } else {
                Expr::Not(Box::new(Expr::CandidateEmpty))
            }
        }
        _ => {
            let m = single(k, val);
            return multi_key(&m, at, sc);
        }
    })
}

fn single(k: &str, v: &Value) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert(k.to_string(), v.clone());
    m
}

fn list(v: &Value, at: &str, sc: &mut Scope) -> R<Vec<Expr>> {
    let a = v
        .as_array()
        .ok_or_else(|| format!("{at}: expected a list"))?;
    a.iter()
        .enumerate()
        .map(|(i, x)| compile(x, &format!("{at}[{i}]"), sc))
        .collect()
}

/// The shapes that carry a subject of their own: a field comparison, a text
/// match, an inline parser atom.
fn multi_key(m: &serde_json::Map<String, Value>, at: &str, sc: &mut Scope) -> R<Expr> {
    if let Some(f) = m.get("field") {
        let name = one_str(f, at)?;
        let ix = field_index(&name).ok_or_else(|| format!("{at}: no field named {name}"))?;
        let mut out: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == "field" {
                continue;
            }
            let cmp = if k == "present" {
                Cmp::Present(v.as_bool().unwrap_or(true))
            } else if let Some(op) = NumOp::parse(k) {
                match v {
                    Value::String(s) => {
                        if op != NumOp::Eq && op != NumOp::Ne {
                            return Err(format!("{at}: {k} on a string"));
                        }
                        Cmp::Str(op == NumOp::Eq, s.clone())
                    }
                    _ => Cmp::Num(op, f64_of(v, at)?),
                }
            } else {
                return Err(format!("{at}: {k} is not a comparison"));
            };
            out.push(Expr::Field { field: ix, cmp });
        }
        if out.is_empty() {
            return Err(format!("{at}: field without a comparison"));
        }
        return Ok(if out.len() == 1 {
            out.pop().unwrap()
        } else {
            Expr::All(out)
        });
    }

    let text_key = if m.contains_key("text") {
        Some(("text", Case::Lower))
    } else if m.contains_key("text_raw") {
        Some(("text_raw", Case::Raw))
    } else {
        None
    };
    if let Some((key, default_case)) = text_key {
        let name = one_str(&m[key], at)?;
        let ix = field_index(&name).ok_or_else(|| format!("{at}: no field named {name}"))?;
        let case = match m.get("case") {
            Some(c) => Case::parse(&one_str(c, at)?)
                .ok_or_else(|| format!("{at}: case is raw, lower or upper"))?,
            None => default_case,
        };
        let trim = m.get("trim_start").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut inner: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == key || k == "case" || k == "trim_start" {
                continue;
            }
            let mut e = subject_atom(k, v, at, sc)?
                .ok_or_else(|| format!("{at}: {k} is not a text atom"))?;
            if trim && let Expr::Prefix { s, .. } = e {
                e = Expr::Prefix { s, trim_start: true };
            }
            inner.push(e);
        }
        if inner.is_empty() {
            return Err(format!("{at}: text without an atom"));
        }
        let inner = if inner.len() == 1 {
            inner.pop().unwrap()
        } else {
            Expr::All(inner)
        };
        return Ok(Expr::Text {
            field: ix,
            case,
            inner: Box::new(inner),
        });
    }

    if let Some(p) = m.get("parser") {
        let name = one_str(p, at)?;
        let pi = *sc
            .parser_ix
            .get(&name)
            .ok_or_else(|| format!("{at}: no parser named {name}"))?;
        let mut inner: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == "parser" {
                continue;
            }
            inner.push(
                subject_atom(k, v, at, sc)?
                    .ok_or_else(|| format!("{at}: {k} is not a parser atom"))?,
            );
        }
        if inner.is_empty() {
            return Err(format!("{at}: parser without an atom"));
        }
        let inner = if inner.len() == 1 {
            inner.pop().unwrap()
        } else {
            Expr::All(inner)
        };
        return Ok(Expr::InParser {
            parser: pi,
            inner: Box::new(inner),
        });
    }

    Err(format!(
        "{at}: unknown condition with keys {:?}",
        m.keys().collect::<Vec<_>>()
    ))
}

// ---------------------------------------------------------------------------
// Loading

impl Pack {
    pub fn load(dir: &Path) -> R<Pack> {
        let manifest = yaml(&dir.join("pack.yml"))?;
        let m = obj(&manifest, "pack.yml")?;
        let name = one_str(&m["pack"], "pack.yml.pack")?;
        let version = one_str(&m["version"], "pack.yml.version")?;

        let mut regexes: Vec<Regex> = Vec::new();

        // --- parsers ---
        let pfile = dir.join(one_str(&m["parsers"], "pack.yml.parsers")?);
        let pv = yaml(&pfile)?;
        let pm = obj(&pv["parsers"], "parsers")?;
        let mut parsers: Vec<ParserDef> = Vec::new();
        let mut parser_ix: HashMap<String, usize> = HashMap::new();
        for (pname, def) in pm {
            let at = format!("parsers.{pname}");
            let d = obj(def, &at)?;
            let field_name = one_str(&d["field"], &format!("{at}.field"))?;
            let field = field_index(&field_name)
                .ok_or_else(|| format!("{at}.field: no field named {field_name}"))?;
            let case = Case::parse(&one_str(&d["case"], &format!("{at}.case"))?)
                .ok_or_else(|| format!("{at}.case: raw, lower or upper"))?;
            let (mut strip, mut split) = (None, None);
            if let Some(t) = d.get("tokenize") {
                let t = obj(t, &format!("{at}.tokenize"))?;
                if let Some(s) = t.get("strip") {
                    strip = Some(
                        Regex::new(&one_str(s, &at)?)
                            .map_err(|e| format!("{at}.tokenize.strip: {e}"))?,
                    );
                }
                let sp = t
                    .get("split")
                    .ok_or_else(|| format!("{at}.tokenize: needs split"))?;
                split = Some(
                    Regex::new(&one_str(sp, &at)?)
                        .map_err(|e| format!("{at}.tokenize.split: {e}"))?,
                );
            }
            parser_ix.insert(pname.clone(), parsers.len());
            parsers.push(ParserDef {
                name: pname.clone(),
                field,
                case,
                strip,
                split,
                pred_names: Vec::new(),
                preds: Vec::new(),
            });
        }
        // Predicates are compiled in a second pass so that a parser may be
        // named before it is declared.
        for (pname, def) in pm {
            let at = format!("parsers.{pname}");
            let pi = parser_ix[pname];
            let preds = obj(&def["predicates"], &format!("{at}.predicates"))?;
            let names: Vec<String> = preds.keys().cloned().collect();
            parsers[pi].pred_names = names.clone();
            let mut out = Vec::with_capacity(names.len());
            for (n, body) in preds {
                let mut sc = Scope {
                    parsers: &parsers,
                    parser_ix: &parser_ix,
                    within: Some(pi),
                    flags: None,
                    candidate: false,
                    regexes: &mut regexes,
                    deps: HashSet::new(),
                };
                out.push(compile(body, &format!("{at}.predicates.{n}"), &mut sc)?);
            }
            parsers[pi].preds = out;
        }

        // --- flags ---
        let ffile = dir.join(one_str(&m["flags"], "pack.yml.flags")?);
        let fv = yaml(&ffile)?;
        let fm = obj(&fv["flags"], "flags")?;
        let flag_names: Vec<String> = fm.keys().cloned().collect();
        let flag_ix: HashMap<String, usize> = flag_names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        let mut flags = Vec::with_capacity(flag_names.len());
        let mut deps: Vec<HashSet<usize>> = Vec::with_capacity(flag_names.len());
        for (n, body) in fm {
            let mut sc = Scope {
                parsers: &parsers,
                parser_ix: &parser_ix,
                within: None,
                flags: Some(&flag_ix),
                candidate: false,
                regexes: &mut regexes,
                deps: HashSet::new(),
            };
            flags.push(compile(body, &format!("flags.{n}"), &mut sc)?);
            deps.push(std::mem::take(&mut sc.deps));
        }
        let flag_order = topological(&deps, &flag_names)?;

        // --- branches ---
        let mut branches = Vec::new();
        if let Some(list) = m.get("branches") {
            for f in list.as_array().unwrap_or(&vec![]) {
                let path = dir.join(one_str(f, "pack.yml.branches")?);
                branches.push(load_branch(
                    &path,
                    &parsers,
                    &parser_ix,
                    &flag_ix,
                    &mut regexes,
                )?);
            }
        }

        // --- passes ---
        let mut passes = Vec::new();
        if let Some(list) = m.get("passes") {
            for f in list.as_array().unwrap_or(&vec![]) {
                let path = dir.join(one_str(f, "pack.yml.passes")?);
                passes.push(load_pass(&path, &parsers, &parser_ix, &mut regexes)?);
            }
        }

        Ok(Pack {
            name,
            version,
            parsers,
            flag_names,
            flags,
            flag_order,
            regexes,
            branches,
            passes,
        })
    }
}

/// Flags may name flags. Order them so every reference is already computed,
/// and refuse a cycle by name rather than by index.
fn topological(deps: &[HashSet<usize>], names: &[String]) -> R<Vec<usize>> {
    let n = deps.len();
    let mut state = vec![0u8; n]; // 0 unvisited, 1 in progress, 2 done
    let mut order = Vec::with_capacity(n);
    fn visit(
        i: usize,
        deps: &[HashSet<usize>],
        names: &[String],
        state: &mut [u8],
        order: &mut Vec<usize>,
    ) -> R<()> {
        match state[i] {
            2 => return Ok(()),
            1 => return Err(format!("flags.{}: a flag cannot depend on itself", names[i])),
            _ => {}
        }
        state[i] = 1;
        let mut ds: Vec<usize> = deps[i].iter().copied().collect();
        ds.sort_unstable();
        for d in ds {
            visit(d, deps, names, state, order)?;
        }
        state[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..n {
        visit(i, deps, names, &mut state, &mut order)?;
    }
    Ok(order)
}

fn slot(v: &Value, at: &str, derive_names: &[String]) -> R<Slot> {
    if let Value::Object(m) = v
        && let Some(f) = m.get("from")
    {
        let name = one_str(f, at)?;
        let i = derive_names
            .iter()
            .position(|d| *d == name)
            .ok_or_else(|| format!("{at}: nothing derives {name}"))?;
        return Ok(Slot::Derived(i));
    }
    Ok(Slot::Literal(one_str(v, at)?))
}

fn load_branch(
    path: &Path,
    parsers: &[ParserDef],
    parser_ix: &HashMap<String, usize>,
    flag_ix: &HashMap<String, usize>,
    regexes: &mut Vec<Regex>,
) -> R<Branch> {
    let v = yaml(path)?;
    let b = obj(&v, &path.display().to_string())?;
    let name = one_str(&b["branch"], "branch")?;
    let at = format!("branch {name}");
    let enter = obj(&b["enter_when"], &format!("{at}.enter_when"))?;
    let enter_provenance = one_str(&enter["provenance"], &format!("{at}.enter_when.provenance"))?;

    let mut mk = |body: &Value, at: &str, regexes: &mut Vec<Regex>| -> R<Expr> {
        let mut sc = Scope {
            parsers,
            parser_ix,
            within: None,
            flags: Some(flag_ix),
            candidate: false,
            regexes,
            deps: HashSet::new(),
        };
        compile(body, at, &mut sc)
    };

    // derived values, ordered cases
    let mut derive_names = Vec::new();
    let mut derives = Vec::new();
    if let Some(d) = b.get("derive") {
        for (dn, cases) in obj(d, &format!("{at}.derive"))? {
            let mut out = Vec::new();
            for (i, c) in cases
                .as_array()
                .ok_or_else(|| format!("{at}.derive.{dn}: expected a list"))?
                .iter()
                .enumerate()
            {
                let cm = obj(c, &format!("{at}.derive.{dn}[{i}]"))?;
                let when = match cm.get("when") {
                    Some(w) => Some(mk(w, &format!("{at}.derive.{dn}[{i}].when"), regexes)?),
                    None => None,
                };
                out.push(DeriveCase {
                    when,
                    value: one_str(&cm["value"], &format!("{at}.derive.{dn}[{i}].value"))?,
                });
            }
            derive_names.push(dn.clone());
            derives.push(out);
        }
    }

    // defaults every rule inherits
    let (mut d_base, mut d_tech, mut d_dt) = (None, None, None);
    if let Some(d) = b.get("defaults")
        && let Some(s) = obj(d, &format!("{at}.defaults"))?.get("set")
    {
        let s = obj(s, &format!("{at}.defaults.set"))?;
        if let Some(x) = s.get("base") {
            d_base = Some(slot(x, &format!("{at}.defaults.set.base"), &derive_names)?);
        }
        if let Some(x) = s.get("technique") {
            d_tech = Some(slot(
                x,
                &format!("{at}.defaults.set.technique"),
                &derive_names,
            )?);
        }
        if let Some(x) = s.get("directory_type") {
            d_dt = Some(slot(
                x,
                &format!("{at}.defaults.set.directory_type"),
                &derive_names,
            )?);
        }
    }

    let order: Vec<String> = strs(&b["order"], &format!("{at}.order"))?;
    let rules_m = obj(&b["rules"], &format!("{at}.rules"))?;
    let mut rules = Vec::with_capacity(order.len());
    for id in &order {
        let r = rules_m
            .get(id)
            .ok_or_else(|| format!("{at}.order: no rule named {id}"))?;
        let rm = obj(r, &format!("{at}.rules.{id}"))?;
        let mut clauses = Vec::new();
        for (i, c) in rm["clauses"]
            .as_array()
            .ok_or_else(|| format!("{at}.rules.{id}.clauses: expected a list"))?
            .iter()
            .enumerate()
        {
            let cm = obj(c, &format!("{at}.rules.{id}.clauses[{i}]"))?;
            clauses.push(Clause {
                cite: one_str(&cm["cite"], &format!("{at}.rules.{id}.clauses[{i}].cite"))?,
                source: one_str(&cm["source"], &format!("{at}.rules.{id}.clauses[{i}].source"))?,
                when: mk(
                    &cm["when"],
                    &format!("{at}.rules.{id}.clauses[{i}].when"),
                    regexes,
                )?,
            });
        }
        let set = obj(&rm["set"], &format!("{at}.rules.{id}.set"))?;
        let mut construct = Vec::new();
        if let Some(c) = set.get("construct") {
            for (i, x) in c
                .as_array()
                .ok_or_else(|| format!("{at}.rules.{id}.set.construct: expected a list"))?
                .iter()
                .enumerate()
            {
                let cat = format!("{at}.rules.{id}.set.construct[{i}]");
                match x {
                    Value::Object(mm) if mm.contains_key("value") => construct.push(SetVal {
                        value: one_str(&mm["value"], &cat)?,
                        when: match mm.get("when") {
                            Some(w) => Some(mk(w, &format!("{cat}.when"), regexes)?),
                            None => None,
                        },
                    }),
                    _ => construct.push(SetVal {
                        value: one_str(x, &cat)?,
                        when: None,
                    }),
                }
            }
        }
        let pick = |k: &str, dflt: &Option<Slot>| -> R<Option<Slot>> {
            match set.get(k) {
                Some(x) => Ok(Some(slot(
                    x,
                    &format!("{at}.rules.{id}.set.{k}"),
                    &derive_names,
                )?)),
                None => Ok(match dflt {
                    Some(Slot::Literal(s)) => Some(Slot::Literal(s.clone())),
                    Some(Slot::Derived(i)) => Some(Slot::Derived(*i)),
                    None => None,
                }),
            }
        };
        rules.push(BranchRule {
            id: id.clone(),
            clauses,
            base: pick("base", &d_base)?,
            technique: pick("technique", &d_tech)?,
            directory_type: pick("directory_type", &d_dt)?,
            construct,
            confidence: f64_of(
                &rm["confidence"],
                &format!("{at}.rules.{id}.confidence"),
            )?,
        });
    }

    Ok(Branch {
        name,
        enter_provenance,
        derive_names,
        derives,
        rules,
    })
}

fn load_pass(
    path: &Path,
    parsers: &[ParserDef],
    parser_ix: &HashMap<String, usize>,
    regexes: &mut Vec<Regex>,
) -> R<VotePass> {
    let v = yaml(path)?;
    let p = obj(&v, &path.display().to_string())?;
    let name = one_str(&p["pass"], "pass")?;
    let at = format!("pass {name}");
    let kind = one_str(&p["kind"], &format!("{at}.kind"))?;
    if kind != "nearest_neighbour_vote" {
        return Err(format!("{at}.kind: the spike implements only nearest_neighbour_vote, not {kind}"));
    }

    let mut dims = Vec::new();
    for (dn, spec) in obj(&p["key"], &format!("{at}.key"))? {
        let s = obj(spec, &format!("{at}.key.{dn}"))?;
        // `slices` is the pass's name for the instance count.
        let fname = if dn == "slices" { "n_instances" } else { dn };
        let field = field_index(fname)
            .ok_or_else(|| format!("{at}.key.{dn}: no field named {fname}"))?;
        let half_even = match s.get("rounding") {
            None => false,
            Some(r) => match one_str(r, &at)?.as_str() {
                "half_even" => true,
                "half_away" => false,
                other => return Err(format!("{at}.key.{dn}.rounding: half_even or half_away, not {other}")),
            },
        };
        dims.push(KeyDim {
            name: dn.clone(),
            field,
            round: s.get("round").map(|x| f64_of(x, &at)).transpose()?,
            ceil: s.get("ceil").map(|x| f64_of(x, &at)).transpose()?,
            half_even,
        });
    }
    let dim_ix = |n: &str| -> R<usize> {
        dims.iter()
            .position(|d| d.name == n)
            .ok_or_else(|| format!("{at}.widen: no key dimension named {n}"))
    };

    let w = obj(&p["widen"], &format!("{at}.widen"))?;
    let max_distance = f64_of(&w["max_distance"], &format!("{at}.widen.max_distance"))? as i64;
    let mut pairs = Vec::new();
    for pr in w["pairs"]
        .as_array()
        .ok_or_else(|| format!("{at}.widen.pairs: expected a list"))?
    {
        let a = strs(pr, &format!("{at}.widen.pairs"))?;
        pairs.push((dim_ix(&a[0])?, dim_ix(&a[1])?));
    }
    let mut relaxed = None;
    if let Some(rs) = w.get("relaxed")
        && let Some(first) = rs.as_array().and_then(|a| a.first())
    {
        let rm = obj(first, &format!("{at}.widen.relaxed[0]"))?;
        let req = one_str(&rm["requires"], &format!("{at}.widen.relaxed[0].requires"))?;
        let vary = obj(&rm["vary"], &format!("{at}.widen.relaxed[0].vary"))?;
        let mut it = vary.iter();
        let (a, av) = it.next().ok_or_else(|| format!("{at}.widen.relaxed[0].vary: empty"))?;
        let (b, bv) = it.next().ok_or_else(|| format!("{at}.widen.relaxed[0].vary: needs two"))?;
        if *a != req {
            return Err(format!("{at}.widen.relaxed[0]: vary must name {req} first"));
        }
        relaxed = Some((
            dim_ix(a)?,
            f64_of(av, &at)? as i64,
            dim_ix(b)?,
            f64_of(bv, &at)? as i64,
        ));
    }

    let d = obj(&p["decide"], &format!("{at}.decide"))?;
    let min_matches = f64_of(&d["min_matches"], &format!("{at}.decide.min_matches"))? as usize;

    let r = obj(&p["reference"], &format!("{at}.reference"))?;
    let partition_by = r.get("partition_by").map(|x| one_str(x, &at)).transpose()?;
    let fallback_except = match r.get("fallback_except") {
        Some(x) => strs(x, &at)?,
        None => Vec::new(),
    };

    // the compatibility filter
    let c = obj(&p["compatibility"], &format!("{at}.compatibility"))?;
    let subj = obj(&c["subject"], &format!("{at}.compatibility.subject"))?;
    let sf = one_str(&subj["text"], &format!("{at}.compatibility.subject.text"))?;
    let subject_field =
        field_index(&sf).ok_or_else(|| format!("{at}.compatibility.subject: no field {sf}"))?;
    let subject_case = Case::parse(&one_str(&subj["case"], &at)?)
        .ok_or_else(|| format!("{at}.compatibility.subject.case: raw, lower or upper"))?;
    let default_family = one_str(
        &c["default_family"],
        &format!("{at}.compatibility.default_family"),
    )?;
    let mut family_of = HashMap::new();
    for (t, f) in obj(&c["family_of"], &format!("{at}.compatibility.family_of"))? {
        family_of.insert(t.clone(), one_str(f, &at)?);
    }
    let mut rules = Vec::new();
    for (i, rv) in c["rules"]
        .as_array()
        .ok_or_else(|| format!("{at}.compatibility.rules: expected a list"))?
        .iter()
        .enumerate()
    {
        let rm = obj(rv, &format!("{at}.compatibility.rules[{i}]"))?;
        let mut sc = Scope {
            parsers,
            parser_ix,
            within: None,
            flags: None,
            candidate: true,
            regexes,
            deps: HashSet::new(),
        };
        let when = compile(&rm["when"], &format!("{at}.compatibility.rules[{i}].when"), &mut sc)?;
        let allow = compile(
            &rm["allow"],
            &format!("{at}.compatibility.rules[{i}].allow"),
            &mut sc,
        )?;
        rules.push(CompatRule { when, allow });
    }

    Ok(VotePass {
        name,
        dims,
        max_distance,
        pairs,
        relaxed,
        min_matches,
        partition_by,
        fallback_except,
        compat: Compat {
            subject_field,
            subject_case,
            default_family,
            family_of,
            rules,
        },
    })
}
