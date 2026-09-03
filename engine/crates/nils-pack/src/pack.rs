// SPDX-License-Identifier: AGPL-3.0-only

//! Loading a pack (`docs/specs/wave2-fingerprint-and-classify.md`, §5).
//!
//! A pack is vocabulary and grammar as data, versioned and diffable. This
//! module reads one, checks it, applies an overlay to its editable buckets,
//! compiles its parsers and flags, and runs its own corpus before handing it
//! back. A pack whose corpus fails does not load: that is what makes one
//! written elsewhere safe to install.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, R};
use crate::expr::{Case, Cmp, Expr, NumOp};
use crate::overlay::Overlay;
use crate::stack::field_index;
use crate::version::Version;
use crate::yaml::{self, File};

/// The pack contract this engine implements. A pack declaring a higher one is
/// refused rather than half-understood.
pub const CONTRACT: u32 = 1;

pub struct ParserDef {
    pub name: String,
    pub field: usize,
    pub case: Case,
    pub strip: Option<Regex>,
    pub split: Option<Regex>,
    pub pred_names: Vec<String>,
    pub preds: Vec<Expr>,
}

/// A loaded pack.
pub struct Pack {
    pub name: String,
    pub version: Version,
    pub contract: u32,
    pub modality: String,
    pub dir: PathBuf,
    /// The editable lists, after any overlay.
    pub buckets: BTreeMap<String, Vec<String>>,
    pub parsers: Vec<ParserDef>,
    pub flag_names: Vec<String>,
    pub flags: Vec<Expr>,
    /// Flags in an order where every reference is already computed.
    pub flag_order: Vec<usize>,
    pub regexes: Vec<Regex>,
    /// The overlay applied, when one was, for the classified row to record.
    pub overlay: Option<String>,
    /// How many cases its own corpus holds, all of which passed.
    pub cases: usize,
}

impl Pack {
    /// `name@version`, which is how a classified row names what judged it.
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn flag_index(&self, name: &str) -> Option<usize> {
        self.flag_names.iter().position(|f| f == name)
    }

    pub fn parser_index(&self, name: &str) -> Option<usize> {
        self.parsers.iter().position(|p| p.name == name)
    }
}

/// Load a pack, with an overlay when one is given.
///
/// The pack's own corpus judges the **pack**, always, and an overlay's cases
/// judge the **overlay**. Running the author's cases against a site's
/// amendment would make the author's claim a constraint on the site, which is
/// the opposite of what an overlay is for: a site that adds a word to a bucket
/// would break a case that says the word is not there.
pub fn load(dir: &Path, overlay: Option<&Overlay>) -> R<Pack> {
    let bare = build(dir, None)?;
    let Some(o) = overlay else {
        return Ok(bare);
    };
    let mut amended = build(dir, Some(o))?;
    amended.cases = bare.cases;
    crate::corpus::run(&amended, &o.cases, "the overlay's cases")?;
    Ok(amended)
}

fn build(dir: &Path, overlay: Option<&Overlay>) -> R<Pack> {
    let manifest = File::read(&dir.join("pack.yml"))?;
    let m = manifest.blame(yaml::obj(&manifest.value, "pack.yml"))?;
    let at = "pack.yml";
    let name = manifest.blame(yaml::text(yaml::get(m, "pack", at)?, "pack"))?;
    let version = manifest.blame(Version::parse(
        &yaml::text(yaml::get(m, "version", at)?, "version")?,
        "version",
    ))?;
    let contract = manifest.blame(yaml::number(yaml::get(m, "contract", at)?, "contract"))? as u32;
    if contract > CONTRACT {
        return Err(Error::at(
            "contract",
            format!(
                "the pack wants contract {contract}; this engine implements {CONTRACT}, so it would be half-understood"
            ),
        )
        .in_file(&manifest.path, Some(&manifest.source)));
    }
    let modality = manifest.blame(yaml::text(yaml::get(m, "modality", at)?, "modality"))?;

    // --- buckets, then the overlay on top of them
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(b) = m.get("buckets") {
        for (k, v) in manifest.blame(yaml::obj(b, "buckets"))? {
            buckets.insert(
                k.clone(),
                manifest.blame(yaml::texts(v, &format!("buckets.{k}")))?,
            );
        }
    }
    let mut overlay_id = None;
    if let Some(o) = overlay {
        o.check_against(&name, &buckets)?;
        for (bucket, edit) in &o.buckets {
            let base = buckets.get(bucket).cloned().unwrap_or_default();
            buckets.insert(bucket.clone(), crate::overlay::merge(&base, edit));
        }
        overlay_id = Some(o.id.clone());
    }

    // --- parsers
    let mut regexes: Vec<Regex> = Vec::new();
    let mut parsers: Vec<ParserDef> = Vec::new();
    let mut parser_ix: HashMap<String, usize> = HashMap::new();
    let parser_files = files_of(m, &manifest, dir, "parsers")?;
    let mut parser_bodies: Vec<(File, Vec<(String, Value)>)> = Vec::new();
    for f in parser_files {
        let root = f.blame(yaml::obj(&f.value, "parsers"))?;
        let list = f.blame(yaml::obj(yaml::get(root, "parsers", "parsers")?, "parsers"))?;
        let mut bodies = Vec::new();
        for (pname, def) in list {
            let at = format!("parsers.{pname}");
            let d = f.blame(yaml::obj(def, &at))?;
            let field_name = f.blame(yaml::text(yaml::get(d, "field", &at)?, &at))?;
            let field = field_index(&field_name).ok_or_else(|| {
                Error::at(
                    format!("{at}.field"),
                    format!("no field named {field_name}"),
                )
                .in_file(&f.path, Some(&f.source))
            })?;
            let case = Case::parse(&f.blame(yaml::text(yaml::get(d, "case", &at)?, &at))?)
                .ok_or_else(|| {
                    Error::at(format!("{at}.case"), "expected raw, lower or upper")
                        .in_file(&f.path, Some(&f.source))
                })?;
            let (mut strip, mut split) = (None, None);
            if let Some(t) = d.get("tokenize") {
                let t = f.blame(yaml::obj(t, &format!("{at}.tokenize")))?;
                if let Some(s) = t.get("strip") {
                    strip = Some(compile_re(
                        &f.blame(yaml::text(s, &at))?,
                        &format!("{at}.tokenize.strip"),
                        &f,
                    )?);
                }
                let sp = f.blame(yaml::get(t, "split", &format!("{at}.tokenize")))?;
                split = Some(compile_re(
                    &f.blame(yaml::text(sp, &at))?,
                    &format!("{at}.tokenize.split"),
                    &f,
                )?);
            }
            if parser_ix.contains_key(pname) {
                return Err(Error::at(&at, "is declared twice").in_file(&f.path, Some(&f.source)));
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
            bodies.push((
                pname.clone(),
                f.blame(yaml::get(d, "predicates", &at))?.clone(),
            ));
        }
        parser_bodies.push((f, bodies));
    }

    // Predicates compile in a second pass, so one parser may name another.
    for (f, bodies) in &parser_bodies {
        for (pname, body) in bodies {
            let at = format!("parsers.{pname}.predicates");
            let preds = f.blame(yaml::obj(body, &at))?;
            let pi = parser_ix[pname];
            parsers[pi].pred_names = preds.keys().cloned().collect();
            let mut out = Vec::with_capacity(preds.len());
            for (n, e) in preds {
                let mut sc = Scope {
                    parsers: &parsers,
                    parser_ix: &parser_ix,
                    buckets: &buckets,
                    within: Some(pi),
                    flags: None,
                    regexes: &mut regexes,
                    deps: HashSet::new(),
                };
                out.push(f.blame(compile(e, &format!("{at}.{n}"), &mut sc))?);
            }
            parsers[pi].preds = out;
        }
    }

    // --- flags
    let mut flag_names: Vec<String> = Vec::new();
    let mut flag_bodies: Vec<(usize, Value, String)> = Vec::new();
    let mut flag_files: Vec<File> = Vec::new();
    for f in files_of(m, &manifest, dir, "flags")? {
        let root = f.blame(yaml::obj(&f.value, "flags"))?;
        let list = f.blame(yaml::obj(yaml::get(root, "flags", "flags")?, "flags"))?;
        let which = flag_files.len();
        for (n, body) in list {
            if flag_names.contains(n) {
                return Err(Error::at(format!("flags.{n}"), "is declared twice")
                    .in_file(&f.path, Some(&f.source)));
            }
            flag_names.push(n.clone());
            flag_bodies.push((which, body.clone(), n.clone()));
        }
        flag_files.push(f);
    }
    let flag_ix: HashMap<String, usize> = flag_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();
    let mut flags = Vec::with_capacity(flag_bodies.len());
    let mut deps: Vec<HashSet<usize>> = Vec::with_capacity(flag_bodies.len());
    for (which, body, n) in &flag_bodies {
        let f = &flag_files[*which];
        let mut sc = Scope {
            parsers: &parsers,
            parser_ix: &parser_ix,
            buckets: &buckets,
            within: None,
            flags: Some(&flag_ix),
            regexes: &mut regexes,
            deps: HashSet::new(),
        };
        flags.push(f.blame(compile(body, &format!("flags.{n}"), &mut sc))?);
        deps.push(std::mem::take(&mut sc.deps));
    }
    let flag_order = topological(&deps, &flag_names)?;

    let mut pack = Pack {
        name,
        version,
        contract,
        modality,
        dir: dir.to_path_buf(),
        buckets,
        parsers,
        flag_names,
        flags,
        flag_order,
        regexes,
        overlay: overlay_id,
        cases: 0,
    };

    // The pack's own corpus is the last thing between it and use, and it
    // judges the pack as its author wrote it.
    if overlay.is_none() {
        pack.cases = crate::corpus::check(&pack, dir)?;
    }
    Ok(pack)
}

/// The files a manifest key names, relative to the pack directory.
fn files_of(
    m: &serde_json::Map<String, Value>,
    manifest: &File,
    dir: &Path,
    key: &str,
) -> R<Vec<File>> {
    let Some(v) = m.get(key) else {
        return Ok(Vec::new());
    };
    let names = manifest.blame(yaml::texts(v, key))?;
    names.iter().map(|n| File::read(&dir.join(n))).collect()
}

fn compile_re(pattern: &str, at: &str, f: &File) -> R<Regex> {
    Regex::new(pattern).map_err(|e| {
        Error::at(at, format!("{pattern} is not a regular expression: {e}"))
            .in_file(&f.path, Some(&f.source))
    })
}

/// Flags may name flags. Order them so every reference is already computed,
/// and refuse a cycle by name rather than by index.
fn topological(deps: &[HashSet<usize>], names: &[String]) -> R<Vec<usize>> {
    fn visit(
        i: usize,
        deps: &[HashSet<usize>],
        names: &[String],
        state: &mut [u8],
        order: &mut Vec<usize>,
    ) -> R<()> {
        match state[i] {
            2 => return Ok(()),
            1 => {
                return Err(Error::at(
                    format!("flags.{}", names[i]),
                    "depends on itself, directly or through another flag",
                ));
            }
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
    let mut state = vec![0u8; deps.len()];
    let mut order = Vec::with_capacity(deps.len());
    for i in 0..deps.len() {
        visit(i, deps, names, &mut state, &mut order)?;
    }
    Ok(order)
}

// ---------------------------------------------------------------------------
// The compiler. `Scope` says which references are legal where.

struct Scope<'a> {
    parsers: &'a [ParserDef],
    parser_ix: &'a HashMap<String, usize>,
    buckets: &'a BTreeMap<String, Vec<String>>,
    /// The parser whose own predicates a bare name refers to.
    within: Option<usize>,
    /// Flag names, when flag references are legal.
    flags: Option<&'a HashMap<String, usize>>,
    regexes: &'a mut Vec<Regex>,
    /// Every flag this expression depends on.
    deps: HashSet<usize>,
}

impl Scope<'_> {
    fn re(&mut self, pattern: &str, at: &str) -> R<usize> {
        let r = Regex::new(pattern)
            .map_err(|e| Error::at(at, format!("{pattern} is not a regular expression: {e}")))?;
        self.regexes.push(r);
        Ok(self.regexes.len() - 1)
    }

    /// A list of strings, written out or taken from an editable bucket.
    fn strings(&self, v: &Value, at: &str) -> R<Vec<String>> {
        if let Value::Object(m) = v
            && let Some(b) = m.get("bucket")
        {
            let name = yaml::text(b, at)?;
            return self.buckets.get(&name).cloned().ok_or_else(|| {
                Error::at(
                    at,
                    format!("no bucket named {name} is declared by the pack"),
                )
            });
        }
        yaml::texts(v, at)
    }
}

fn compile(v: &Value, at: &str, sc: &mut Scope) -> R<Expr> {
    match v {
        Value::Bool(b) => Ok(Expr::Lit(*b)),
        Value::String(s) => reference(s, at, sc),
        Value::Object(m) => {
            if m.len() == 1 {
                let (k, val) = m.iter().next().expect("one key");
                single_key(k, val, at, sc)
            } else {
                multi_key(m, at, sc)
            }
        }
        _ => Err(Error::at(at, "expected a condition")),
    }
}

fn reference(s: &str, at: &str, sc: &mut Scope) -> R<Expr> {
    if let Some((p, pred)) = s.split_once('.') {
        let pi = *sc
            .parser_ix
            .get(p)
            .ok_or_else(|| Error::at(at, format!("no parser named {p}")))?;
        let px = sc.parsers[pi]
            .pred_names
            .iter()
            .position(|n| n == pred)
            .ok_or_else(|| Error::at(at, format!("parser {p} has no predicate {pred}")))?;
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
    Err(Error::at(
        at,
        format!("{s} is not a predicate of a parser or a flag of this pack"),
    ))
}

/// The atoms that read whatever subject is in scope.
fn subject_atom(k: &str, val: &Value, at: &str, sc: &mut Scope) -> R<Option<Expr>> {
    let e = match k {
        "token" => Expr::Token(yaml::text(val, at)?),
        "any_token" => Expr::AnyToken(sc.strings(val, at)?),
        "all_tokens" => Expr::AllTokens(sc.strings(val, at)?),
        "substring" => Expr::Substring(yaml::text(val, at)?),
        "any_substring" => Expr::AnySubstring(sc.strings(val, at)?),
        "equals" => Expr::Equals(yaml::text(val, at)?),
        "prefix" => Expr::Prefix {
            s: yaml::text(val, at)?,
            trim_start: false,
        },
        "matches" => {
            let i = sc.re(&yaml::text(val, at)?, at)?;
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
            let m = yaml::obj(val, at)?;
            let (op, n) = m
                .iter()
                .next()
                .ok_or_else(|| Error::at(at, "tokens needs a comparison"))?;
            let op = NumOp::parse(op)
                .ok_or_else(|| Error::at(at, format!("{op} is not a comparison")))?;
            Expr::TokenCount(op, yaml::number(n, at)? as usize)
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
        _ => {
            let mut m = serde_json::Map::new();
            m.insert(k.to_string(), val.clone());
            return multi_key(&m, at, sc);
        }
    })
}

fn list(v: &Value, at: &str, sc: &mut Scope) -> R<Vec<Expr>> {
    yaml::arr(v, at)?
        .iter()
        .enumerate()
        .map(|(i, x)| compile(x, &format!("{at}[{i}]"), sc))
        .collect()
}

/// The shapes that carry a subject of their own.
fn multi_key(m: &serde_json::Map<String, Value>, at: &str, sc: &mut Scope) -> R<Expr> {
    if let Some(f) = m.get("field") {
        let name = yaml::text(f, at)?;
        let ix =
            field_index(&name).ok_or_else(|| Error::at(at, format!("no field named {name}")))?;
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
                            return Err(Error::at(
                                at,
                                format!("{k} compares numbers, and {s} is a string"),
                            ));
                        }
                        Cmp::Str(op == NumOp::Eq, s.clone())
                    }
                    _ => Cmp::Num(op, yaml::number(v, at)?),
                }
            } else {
                return Err(Error::at(at, format!("{k} is not a comparison")));
            };
            out.push(Expr::Field { field: ix, cmp });
        }
        return match out.len() {
            0 => Err(Error::at(
                at,
                "a field without a comparison decides nothing",
            )),
            1 => Ok(out.pop().expect("one")),
            _ => Ok(Expr::All(out)),
        };
    }

    let text_key = if m.contains_key("text") {
        Some(("text", Case::Lower))
    } else if m.contains_key("text_raw") {
        Some(("text_raw", Case::Raw))
    } else {
        None
    };
    if let Some((key, default_case)) = text_key {
        let name = yaml::text(&m[key], at)?;
        let ix =
            field_index(&name).ok_or_else(|| Error::at(at, format!("no field named {name}")))?;
        let case = match m.get("case") {
            Some(c) => Case::parse(&yaml::text(c, at)?)
                .ok_or_else(|| Error::at(at, "case is raw, lower or upper"))?,
            None => default_case,
        };
        let trim = m
            .get("trim_start")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut inner: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == key || k == "case" || k == "trim_start" {
                continue;
            }
            let mut e = subject_atom(k, v, at, sc)?
                .ok_or_else(|| Error::at(at, format!("{k} is not an atom a text field takes")))?;
            if trim && let Expr::Prefix { s, .. } = e {
                e = Expr::Prefix {
                    s,
                    trim_start: true,
                };
            }
            inner.push(e);
        }
        return match inner.len() {
            0 => Err(Error::at(
                at,
                "a text field without an atom decides nothing",
            )),
            _ => Ok(Expr::Text {
                field: ix,
                case,
                inner: Box::new(if inner.len() == 1 {
                    inner.pop().expect("one")
                } else {
                    Expr::All(inner)
                }),
            }),
        };
    }

    if let Some(p) = m.get("parser") {
        let name = yaml::text(p, at)?;
        let pi = *sc
            .parser_ix
            .get(&name)
            .ok_or_else(|| Error::at(at, format!("no parser named {name}")))?;
        let mut inner: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == "parser" {
                continue;
            }
            inner.push(
                subject_atom(k, v, at, sc)?
                    .ok_or_else(|| Error::at(at, format!("{k} is not an atom a parser takes")))?,
            );
        }
        return match inner.len() {
            0 => Err(Error::at(at, "a parser without an atom decides nothing")),
            _ => Ok(Expr::InParser {
                parser: pi,
                inner: Box::new(if inner.len() == 1 {
                    inner.pop().expect("one")
                } else {
                    Expr::All(inner)
                }),
            }),
        };
    }

    Err(Error::at(
        at,
        format!(
            "no condition has the keys {:?}",
            m.keys().collect::<Vec<_>>()
        ),
    ))
}
