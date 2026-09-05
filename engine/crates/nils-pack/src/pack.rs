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
use crate::normalize::{Conditional, Normalizer};
use crate::overlay::Overlay;
use crate::pass::{
    Compat, CompatRule, Cond, Emit, KeyDim, Kind, OnTie, Pass, Phase, Reference, Relaxed, Vote,
    What,
};
use crate::rules::{
    Axis, AxisPhase, AxisValue, Clause, Derive, DeriveCase, Rule, RuleSet, SetValue, Sets, Tier,
    Which,
};
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
    /// Text the pack derives for itself, published as fields of its own: the
    /// semantic normalizer of §6.4 is one.
    pub derived: Vec<Normalizer>,
    /// The axes this pack decides, in declaration order.
    pub axes: Vec<Axis>,
    /// The rule sets, in the order they run.
    pub rule_sets: Vec<RuleSet>,
    /// The passes: the phases that read more than one stack (§7). A pack that
    /// declares none gets none.
    pub passes: Vec<Pass>,
    /// The overlay applied, when one was, for the classified row to record.
    pub overlay: Option<String>,
    /// How many cases its own corpus holds, all of which passed.
    pub cases: usize,
    /// When a person is asked about an axis. The pack's call, not the
    /// engine's (§8.2): what counts as doubt is knowledge about the domain.
    pub review: Review,
}

/// The pack's emission thresholds for review items.
///
/// v0 asks a person about 84 percent of its stacks, mostly because a keyword
/// was missing rather than because anything was in doubt, and a queue that
/// long is read by nobody. So the thresholds are declared, per axis, by the
/// pack that knows what a weak answer means, and a run reports the size of
/// the queue it produced.
#[derive(Debug, Clone, Default)]
pub struct Review {
    /// An axis resolved below this confidence is asked about.
    pub low_confidence: f64,
    /// Per-axis overrides of it.
    pub per_axis: BTreeMap<String, f64>,
    /// The axes whose absence is itself a question. An axis not named here
    /// simply does not apply to a stack that has no value for it.
    pub missing: Vec<String>,
    /// Stacks nobody is asked about at all. An excluded localizer is a
    /// decided outcome, and a queue that carries it is v0's queue.
    pub silent_when: Option<Expr>,
}

impl Review {
    /// The confidence below which this axis is asked about.
    pub fn below(&self, axis: &str) -> f64 {
        *self.per_axis.get(axis).unwrap_or(&self.low_confidence)
    }

    /// Whether an axis with no value at all is a question for a person.
    pub fn asks_when_missing(&self, axis: &str) -> bool {
        self.missing.iter().any(|a| a == axis)
    }
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

    pub fn axis_index(&self, name: &str) -> Option<usize> {
        self.axes.iter().position(|a| a.name == name)
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

    // --- when a person is asked about an axis
    let mut review = Review {
        low_confidence: 0.0,
        ..Review::default()
    };
    if let Some(r) = m.get("review") {
        let rm = manifest.blame(yaml::obj(r, "review"))?;
        if let Some(v) = rm.get("low_confidence") {
            match yaml::obj(v, "review.low_confidence") {
                Ok(per) => {
                    for (axis, value) in per {
                        let at = format!("review.low_confidence.{axis}");
                        let n = manifest.blame(yaml::number(value, &at))?;
                        if axis == "default" {
                            review.low_confidence = n;
                        } else {
                            review.per_axis.insert(axis.clone(), n);
                        }
                    }
                }
                Err(_) => {
                    review.low_confidence =
                        manifest.blame(yaml::number(v, "review.low_confidence"))?;
                }
            }
        }
        if let Some(v) = rm.get("missing") {
            review.missing = manifest.blame(yaml::texts(v, "review.missing"))?;
        }
    }

    // --- text the pack derives for itself. First, because a parser, a flag
    // or an axis may read it, and it reads only the fingerprint.
    let mut derived: Vec<Normalizer> = Vec::new();
    for f in files_of(m, &manifest, dir, "normalize")? {
        derived.push(load_normalizer(&f)?);
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
                    derived: &derived,
                    // Parsers and flags are read before any axis is decided,
                    // so an axis atom in one has nothing to name.
                    axes: &[],
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
            derived: &derived,
            axes: &[],
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

    // --- axes, each of which is a rule set written compactly (§6.3)
    let mut axes: Vec<Axis> = Vec::new();
    let mut rule_sets: Vec<RuleSet> = Vec::new();
    for f in files_of(m, &manifest, dir, "axes")? {
        let loaded = load_axis(
            &f,
            axes.len(),
            &axes,
            &derived,
            &flag_ix,
            &parsers,
            &parser_ix,
            &buckets,
            &mut regexes,
        )?;
        if axes.iter().any(|a| a.name == loaded.axis.name) {
            return Err(
                Error::at(format!("axis {}", loaded.axis.name), "is declared twice")
                    .in_file(&f.path, Some(&f.source)),
            );
        }
        axes.push(loaded.axis);
        // An axis with no rules of its own is vocabulary: a longhand rule set
        // decides it, and that set carries the name.
        if !loaded.set.rules.is_empty() {
            rule_sets.push(loaded.set);
        }
    }

    // --- rule sets written longhand, after the axes they decide
    for f in files_of(m, &manifest, dir, "rules")? {
        let set = load_rule_set(
            &f,
            &axes,
            &derived,
            &flag_ix,
            &parsers,
            &parser_ix,
            &buckets,
            &mut regexes,
        )?;
        if rule_sets.iter().any(|r| r.name == set.name) {
            return Err(
                Error::at(format!("rules {}", set.name), "is declared twice")
                    .in_file(&f.path, Some(&f.source)),
            );
        }
        rule_sets.push(set);
    }

    // The order the rule sets run in, when the pack states one. Without it
    // they run as declared: the axes, then the longhand sets.
    if let Some(o) = m.get("order") {
        let want = manifest.blame(yaml::texts(o, "order"))?;
        for n in &want {
            if !rule_sets.iter().any(|r| r.name == *n) {
                return Err(Error::at("order", format!("no rule set named {n}"))
                    .in_file(&manifest.path, Some(&manifest.source)));
            }
        }
        for r in &rule_sets {
            if !want.contains(&r.name) {
                return Err(Error::at(
                    "order",
                    format!("{} is not in the order, so it would never run", r.name),
                )
                .in_file(&manifest.path, Some(&manifest.source)));
            }
        }
        rule_sets.sort_by_key(|r| want.iter().position(|n| *n == r.name).unwrap_or(usize::MAX));
    }

    // Wave 3 §7. A rule that runs before the passes may not read an axis that
    // is decided after them: nothing stops it at run time, because an axis
    // nothing has decided reads as empty, so the rule would quietly take the
    // wrong branch rather than fail.
    for set in &rule_sets {
        if set.phase != AxisPhase::Class {
            continue;
        }
        for a in set.axes_read() {
            if axes[a].phase == AxisPhase::Disposition {
                return Err(Error::at(
                    &set.name,
                    format!(
                        "reads {}, which is decided after the passes; a rule of the class phase \
                         would read it as empty",
                        axes[a].name
                    ),
                )
                .in_file(&manifest.path, Some(&manifest.source)));
            }
        }
    }

    // The one part of `review` that is an expression, compiled here because
    // it may read an axis and the axes exist only now. A stack it holds for
    // raises no question at all: an excluded localizer is a decided outcome
    // and not a gap in anybody's knowledge.
    if let Some(r) = m.get("review")
        && let Some(w) = manifest.blame(yaml::obj(r, "review"))?.get("silent_when")
    {
        let mut sc = Scope {
            derived: &derived,
            axes: &axes,
            parsers: &parsers,
            parser_ix: &parser_ix,
            buckets: &buckets,
            within: None,
            flags: Some(&flag_ix),
            regexes: &mut regexes,
            deps: HashSet::new(),
        };
        review.silent_when = Some(manifest.blame(compile(w, "review.silent_when", &mut sc))?);
    }

    // A threshold for an axis the pack does not decide is a name that names
    // nothing: it would sit in the file looking like policy and do nothing.
    for axis in review.per_axis.keys().chain(review.missing.iter()) {
        if !axes.iter().any(|a| a.name == *axis) {
            return Err(
                Error::at(format!("review.{axis}"), format!("no axis named {axis}"))
                    .in_file(&manifest.path, Some(&manifest.source)),
            );
        }
    }

    // --- passes, last: a pass may name an axis and a flag, and both exist now
    let mut passes: Vec<Pass> = Vec::new();
    for pf in files_of(m, &manifest, dir, "passes")? {
        let pass = load_pass(&pf, &axes, &derived, &buckets, &mut regexes)?;
        if passes.iter().any(|p: &Pass| p.name == pass.name) {
            return Err(
                Error::at(format!("pass {}", pass.name), "is declared twice")
                    .in_file(&pf.path, Some(&pf.source)),
            );
        }
        passes.push(pass);
    }

    let mut pack = Pack {
        derived,
        axes,
        rule_sets,
        passes,
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
        review,
    };

    // The pack's own corpus is the last thing between it and use, and it
    // judges the pack as its author wrote it.
    if overlay.is_none() {
        pack.cases = crate::corpus::check(&pack, dir)?;
    }
    Ok(pack)
}

/// A field a pack may name: one the fingerprint carries, or one the pack
/// derives for itself. A derived field sits past the end of the fingerprint's
/// own, which is how the evaluator tells them apart.
fn resolve_field(derived: &[Normalizer], name: &str) -> Option<usize> {
    if let Some(i) = derived.iter().position(|d| d.into == name) {
        return Some(crate::stack::FIELDS.len() + i);
    }
    field_index(name)
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
    /// The text the pack derives, which a field or text atom may name.
    derived: &'a [Normalizer],
    /// The axes declared before this point. An atom may only name one of
    /// them, which is also what guarantees it has been decided by the time
    /// this expression runs.
    axes: &'a [Axis],
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
        let ix = resolve_field(sc.derived, &name)
            .ok_or_else(|| Error::at(at, format!("no field named {name}")))?;
        let mut out: Vec<Expr> = Vec::new();
        for (k, v) in m {
            if k == "field" {
                continue;
            }
            let cmp = if k == "present" {
                Cmp::Present(v.as_bool().unwrap_or(true))
            } else if let Some(op) = NumOp::parse(k) {
                match v {
                    // Against another of the stack's own numbers.
                    Value::Object(mm) if mm.contains_key("field") => {
                        let other = yaml::text(&mm["field"], at)?;
                        let oi = resolve_field(sc.derived, &other)
                            .ok_or_else(|| Error::at(at, format!("no field named {other}")))?;
                        Cmp::Field(op, oi)
                    }
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
        let ix = resolve_field(sc.derived, &name)
            .ok_or_else(|| Error::at(at, format!("no field named {name}")))?;
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

    if let Some(a) = m.get("axis") {
        let name = yaml::text(a, at)?;
        let ix = sc.axes.iter().position(|x| x.name == name).ok_or_else(|| {
            Error::at(
                at,
                format!(
                    "no axis named {name} is declared before this one; an axis atom reads what an earlier rule set decided"
                ),
            )
        })?;
        let missing_or = m.get("missing_or").is_some();
        let want = yaml::text(
            m.get("is").or_else(|| m.get("missing_or")).ok_or_else(|| {
                Error::at(at, "an axis atom needs `is`, or `missing_or` for a gap")
            })?,
            at,
        )?;
        // An axis atom may name a value by its identity or by what a row
        // stores, and it compares against what is stored, since that is what
        // an earlier rule set left behind.
        let axis = &sc.axes[ix];
        let value = match axis.value_index(&want) {
            Some(i) => axis.stored(i).to_string(),
            None if axis.values.iter().any(|v| v.label == want) => want,
            None if axis.default.as_deref() == Some(&want) => want,
            None => {
                return Err(Error::at(
                    at,
                    format!("{want} is not a value of the {name} axis"),
                ));
            }
        };
        return Ok(if missing_or {
            Expr::AxisMissingOr { axis: ix, value }
        } else {
            Expr::Axis { axis: ix, value }
        });
    }

    // The two atoms that read the candidate a pass is judging rather than the
    // stack. A pack may write them anywhere; they are simply false wherever
    // there is no candidate, which is everywhere but a pass (§7.2).
    if let Some(f) = m.get("family") {
        return Ok(Expr::Family(yaml::text(f, at)?));
    }
    if let Some(c) = m.get("candidate_empty") {
        let want = c.as_bool().unwrap_or(true);
        return Ok(if want {
            Expr::CandidateEmpty
        } else {
            Expr::Not(Box::new(Expr::CandidateEmpty))
        });
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

// ---------------------------------------------------------------------------
// Axes, and the rule sets they expand into (§6.3).

/// The confidences v0 fixes per tier, which a pack may restate.
const DEFAULT_TIERS: [(&str, f64); 7] = [
    ("stated", 0.0),
    ("exclusive", 0.95),
    ("keywords", 0.85),
    ("combination", 0.75),
    ("alternative", 0.75),
    ("physics", 0.70),
    ("default", 0.0),
];

struct AxisFile {
    axis: Axis,
    set: RuleSet,
}

/// An axis file is a rule set written compactly: one rule per value, in
/// `order`, with up to three clauses in v0's order. Writing four hundred
/// keyword rules longhand would be worse than v0, which is why the dense form
/// stays and the loader expands it.
#[allow(clippy::too_many_arguments)]
fn load_axis(
    f: &File,
    axis_index: usize,
    axes: &[Axis],
    derived: &[Normalizer],
    flag_ix: &HashMap<String, usize>,
    parsers: &[ParserDef],
    parser_ix: &HashMap<String, usize>,
    buckets: &BTreeMap<String, Vec<String>>,
    regexes: &mut Vec<Regex>,
) -> R<AxisFile> {
    let m = f.blame(yaml::obj(&f.value, "axis"))?;
    let name = f.blame(yaml::text(yaml::get(m, "axis", "axis")?, "axis"))?;
    let multi = match m.get("kind") {
        None => false,
        Some(k) => match f.blame(yaml::text(k, "kind"))?.as_str() {
            "single" => false,
            "multi" => true,
            other => {
                return Err(Error::at("kind", format!("single or multi, not {other}"))
                    .in_file(&f.path, Some(&f.source)));
            }
        },
    };
    // Wave 3 §7. An axis that says what to do with a stack rather than what it
    // is, is decided after the passes, from what they left. Saying nothing
    // means the ordinary phase, so every pack written before this reads the
    // same way.
    let phase = match m.get("phase") {
        None => AxisPhase::Class,
        Some(v) => match f.blame(yaml::text(v, "phase"))?.as_str() {
            "class" => AxisPhase::Class,
            "disposition" => AxisPhase::Disposition,
            other => {
                return Err(
                    Error::at("phase", format!("class or disposition, not {other}"))
                        .in_file(&f.path, Some(&f.source)),
                );
            }
        },
    };
    let search_field = match m.get("search") {
        Some(v) => f.blame(yaml::text(v, "search"))?,
        None => "text_all".to_string(),
    };
    let search = resolve_field(derived, &search_field).ok_or_else(|| {
        Error::at("search", format!("no field named {search_field}"))
            .in_file(&f.path, Some(&f.source))
    })?;

    let mut tiers: HashMap<String, f64> = DEFAULT_TIERS
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
    if let Some(t) = m.get("tiers") {
        for (k, v) in f.blame(yaml::obj(t, "tiers"))? {
            if !tiers.contains_key(k) {
                return Err(Error::at(
                    format!("tiers.{k}"),
                    "no clause has that tier; they are exclusive, keywords, alternative, combination, physics and default",
                )
                .in_file(&f.path, Some(&f.source)));
            }
            tiers.insert(k.clone(), f.blame(yaml::number(v, &format!("tiers.{k}")))?);
        }
    }
    let tier_of = |t: Tier| -> f64 { tiers.get(t.name()).copied().unwrap_or(0.0) };

    // --- the vocabulary, in the order it is tried
    // An axis with no order of its own is vocabulary: a longhand rule set
    // decides it, which is what v0's base contrast needs, since its tiers are
    // tier-major and not value-major.
    let order = match m.get("order") {
        Some(o) => f.blame(yaml::texts(o, "order"))?,
        None => Vec::new(),
    };
    let values_m = f.blame(yaml::obj(yaml::get(m, "values", "axis")?, "values"))?;
    for id in &order {
        if !values_m.contains_key(id) {
            return Err(Error::at(
                "order",
                format!("{id} is in the order and not in the values"),
            )
            .in_file(&f.path, Some(&f.source)));
        }
    }
    // A value outside the order is part of the vocabulary and not part of
    // this axis's own rule set: a route sets it, and it is checked against
    // this vocabulary when the route is loaded. v0 has seven such constructs,
    // which its SWI and SyMRI branches write and its detector never tries.

    let mut vocabulary: Vec<String> = order.clone();
    for id in values_m.keys() {
        if !vocabulary.contains(id) {
            vocabulary.push(id.clone());
        }
    }
    let mut values = Vec::with_capacity(vocabulary.len());
    for id in &vocabulary {
        let at = format!("values.{id}");
        let v = f.blame(yaml::obj(&values_m[id], &at))?;
        values.push(AxisValue {
            id: id.clone(),
            group: match v.get("group") {
                Some(x) => Some(f.blame(yaml::text(x, &at))?),
                None => None,
            },
            priority: match v.get("priority") {
                Some(x) => Some(f.blame(yaml::number(x, &at))? as i64),
                None => None,
            },
            label: match v.get("label") {
                Some(l) => f.blame(yaml::text(l, &at))?,
                None => id.clone(),
            },
            family: match v.get("family") {
                Some(x) => Some(f.blame(yaml::text(x, &at))?),
                None => None,
            },
        });
    }

    let default = match m.get("default") {
        Some(d) => Some(f.blame(yaml::text(d, "default"))?),
        None => None,
    };
    let default_confidence = tier_of(Tier::Default);

    let stores_label = match m.get("stores") {
        None => false,
        Some(v) => match f.blame(yaml::text(v, "stores"))?.as_str() {
            "id" => false,
            "label" => true,
            other => {
                return Err(Error::at("stores", format!("id or label, not {other}"))
                    .in_file(&f.path, Some(&f.source)));
            }
        },
    };

    // --- one rule per value, three clauses in v0's order
    // v0 tries the priority order first and then every remaining value in
    // declaration order: the order is a partial ordering, not a filter. A
    // value no clause can reach produces no rule at all, which is what its
    // loop does when it finds nothing to check.
    let mut rules = Vec::with_capacity(vocabulary.len());
    for id in vocabulary.iter() {
        let i = vocabulary
            .iter()
            .position(|v| v == id)
            .expect("the order is inside the vocabulary");
        let at = format!("values.{id}");
        let v = f.blame(yaml::obj(&values_m[id], &at))?;
        let mut clauses: Vec<Clause> = Vec::new();

        let detection = match v.get("detection") {
            Some(d) => f.blame(yaml::obj(d, &format!("{at}.detection")))?.clone(),
            None => serde_json::Map::new(),
        };
        if let Some(x) = detection.get("exclusive") {
            let flag = f.blame(yaml::text(x, &format!("{at}.detection.exclusive")))?;
            clauses.push(Clause::Flag {
                tier: Tier::Exclusive,
                confidence: tier_of(Tier::Exclusive),
                flag: *flag_ix.get(&flag).ok_or_else(|| {
                    Error::at(
                        format!("{at}.detection.exclusive"),
                        format!("{flag} is not a flag of this pack"),
                    )
                    .in_file(&f.path, Some(&f.source))
                })?,
                name: flag,
            });
        }
        if let Some(c) = v.get("alternative_flags") {
            let at_a = format!("{at}.alternative_flags");
            let names = f.blame(yaml::texts(c, &at_a))?;
            let mut flags = Vec::with_capacity(names.len());
            for n in &names {
                flags.push(*flag_ix.get(n).ok_or_else(|| {
                    Error::at(&at_a, format!("{n} is not a flag of this pack"))
                        .in_file(&f.path, Some(&f.source))
                })?);
            }
            if !flags.is_empty() {
                clauses.push(Clause::AnyFlag {
                    tier: Tier::Alternative,
                    confidence: tier_of(Tier::Alternative),
                    names,
                    flags,
                });
            }
        }
        if let Some(k) = v.get("keywords").or_else(|| detection.get("keywords")) {
            let at_k = format!("{at}.keywords");
            let list = match k {
                Value::Object(mm) if mm.contains_key("bucket") => {
                    let b = f.blame(yaml::text(&mm["bucket"], &at_k))?;
                    buckets.get(&b).cloned().ok_or_else(|| {
                        Error::at(
                            &at_k,
                            format!("no bucket named {b} is declared by the pack"),
                        )
                        .in_file(&f.path, Some(&f.source))
                    })?
                }
                _ => f.blame(yaml::texts(k, &at_k))?,
            };
            if !list.is_empty() {
                clauses.push(Clause::Keywords {
                    tier: Tier::Keywords,
                    confidence: tier_of(Tier::Keywords),
                    field: search,
                    list,
                });
            }
        }
        if let Some(c) = detection.get("combination") {
            let at_c = format!("{at}.detection.combination");
            let names = f.blame(yaml::texts(c, &at_c))?;
            let mut flags = Vec::with_capacity(names.len());
            for n in &names {
                flags.push(*flag_ix.get(n).ok_or_else(|| {
                    Error::at(&at_c, format!("{n} is not a flag of this pack"))
                        .in_file(&f.path, Some(&f.source))
                })?);
            }
            if !flags.is_empty() {
                clauses.push(Clause::Combination {
                    tier: Tier::Combination,
                    confidence: tier_of(Tier::Combination),
                    names,
                    flags,
                });
            }
        }

        if clauses.is_empty() {
            // Vocabulary only: a route sets it, or it is the axis's default.
            continue;
        }

        let requires = match v.get("requires") {
            Some(r) => {
                let mut sc = Scope {
                    derived,
                    axes,
                    parsers,
                    parser_ix,
                    buckets,
                    within: None,
                    flags: Some(flag_ix),
                    regexes,
                    deps: HashSet::new(),
                };
                Some(f.blame(compile(r, &format!("{at}.requires"), &mut sc))?)
            }
            None => None,
        };
        let stated = match v.get("confidence") {
            Some(c) => Some(f.blame(yaml::number(c, &format!("{at}.confidence")))?),
            None => None,
        };
        rules.push(Rule {
            id: id.clone(),
            requires,
            clauses,
            sets: vec![Sets {
                axis: axis_index,
                values: vec![SetValue {
                    value: Which::Fixed(i),
                    when: None,
                }],
            }],
            confidence: stated,
            why: None,
        });
    }

    // --- physics windows: rules with a numeric clause, tried after the
    // vocabulary, so a keyword always beats a number as v0 intends
    if let Some(ph) = m.get("physics") {
        for (i, r) in f.blame(yaml::arr(ph, "physics"))?.iter().enumerate() {
            let at = format!("physics[{i}]");
            let rm = f.blame(yaml::obj(r, &at))?;
            let id = f.blame(yaml::text(yaml::get(rm, "value", &at)?, &at))?;
            let value = vocabulary.iter().position(|v| *v == id).ok_or_else(|| {
                Error::at(&at, format!("{id} is not a value of this axis"))
                    .in_file(&f.path, Some(&f.source))
            })?;
            let mut sc = Scope {
                derived,
                axes,
                parsers,
                parser_ix,
                buckets,
                within: None,
                flags: Some(flag_ix),
                regexes,
                deps: HashSet::new(),
            };
            let expr = f.blame(compile(
                yaml::get(rm, "when", &at)?,
                &format!("{at}.when"),
                &mut sc,
            ))?;
            let confidence = match rm.get("confidence") {
                Some(c) => Some(f.blame(yaml::number(c, &at))?),
                None => None,
            };
            rules.push(Rule {
                id: format!("physics:{id}"),
                requires: None,
                clauses: vec![Clause::When {
                    tier: Tier::Physics,
                    confidence: tier_of(Tier::Physics),
                    cite: format!("physics window {i}"),
                    source: "physics".into(),
                    expr,
                }],
                sets: vec![Sets {
                    axis: axis_index,
                    values: vec![SetValue {
                        value: Which::Fixed(value),
                        when: None,
                    }],
                }],
                confidence,
                why: rm.get("why").map(|w| yaml::text(w, &at)).transpose()?,
            });
        }
    }

    Ok(AxisFile {
        axis: Axis {
            name: name.clone(),
            phase,
            multi,
            values,
            default,
            default_confidence,
            stores_label,
        },
        set: RuleSet {
            name,
            derives: Vec::new(),
            adds: Vec::new(),
            collect: multi,
            decides: vec![axis_index],
            enter_when: None,
            rules,
            phase,
        },
    })
}

/// The semantic normalizer of §6.4, as data: twelve ordered steps, no code.
fn load_normalizer(f: &File) -> R<Normalizer> {
    let m = f.blame(yaml::obj(&f.value, "normalize"))?;
    let into = f.blame(yaml::text(
        yaml::get(m, "normalize", "normalize")?,
        "normalize",
    ))?;
    if field_index(&into).is_some() {
        return Err(Error::at(
            "normalize",
            format!("{into} is already a field of the fingerprint; a pack's derived text needs a name of its own"),
        )
        .in_file(&f.path, Some(&f.source)));
    }
    let mut from = Vec::new();
    for (i, n) in f
        .blame(yaml::texts(yaml::get(m, "from", "normalize")?, "from"))?
        .iter()
        .enumerate()
    {
        from.push(field_index(n).ok_or_else(|| {
            Error::at(format!("from[{i}]"), format!("no field named {n}"))
                .in_file(&f.path, Some(&f.source))
        })?);
    }

    let one_char = |s: &str, at: &str| -> R<char> {
        let mut it = s.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(Error::at(at, format!("{s:?} is not a single character"))),
        }
    };

    let (mut meaningful, mut to_space, mut remove) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(c) = m.get("characters") {
        let c = f.blame(yaml::obj(c, "characters"))?;
        if let Some(x) = c.get("meaningful") {
            for (k, v) in f.blame(yaml::obj(x, "characters.meaningful"))? {
                meaningful.push((
                    f.blame(one_char(k, "characters.meaningful"))?,
                    f.blame(yaml::text(v, "characters.meaningful"))?,
                ));
            }
        }
        for (key, into) in [("to_space", &mut to_space), ("remove", &mut remove)] {
            if let Some(x) = c.get(key) {
                for s in f.blame(yaml::texts(x, &format!("characters.{key}")))? {
                    into.push(f.blame(one_char(&s, &format!("characters.{key}")))?);
                }
            }
        }
    }

    let token_removals = match m.get("token_removals") {
        Some(v) => f
            .blame(yaml::texts(v, "token_removals"))?
            .iter()
            .map(|t| t.to_lowercase())
            .collect(),
        None => Vec::new(),
    };
    let mut token_replacements = std::collections::BTreeMap::new();
    if let Some(v) = m.get("token_replacements") {
        for (token, canonical) in f.blame(yaml::obj(v, "token_replacements"))? {
            token_replacements.insert(
                token.to_lowercase(),
                f.blame(yaml::text(canonical, "token_replacements"))?
                    .to_lowercase(),
            );
        }
    }

    let mut conditional = Vec::new();
    if let Some(v) = m.get("conditional_replacements") {
        for (i, r) in f
            .blame(yaml::arr(v, "conditional_replacements"))?
            .iter()
            .enumerate()
        {
            let at = format!("conditional_replacements[{i}]");
            let rm = f.blame(yaml::obj(r, &at))?;
            let words = |key: &str| -> R<Vec<String>> {
                Ok(match rm.get(key) {
                    Some(x) => yaml::texts(x, &at)?
                        .iter()
                        .map(|w| w.to_lowercase())
                        .collect(),
                    None => Vec::new(),
                })
            };
            let c = Conditional {
                canonical: f
                    .blame(yaml::text(yaml::get(rm, "canonical", &at)?, &at))?
                    .to_lowercase(),
                replace: f
                    .blame(yaml::text(yaml::get(rm, "replace", &at)?, &at))?
                    .to_lowercase(),
                when_has_any: f.blame(words("when_has_any"))?,
                when_has_all: f.blame(words("when_has_all"))?,
            };
            if c.when_has_any.is_empty() && c.when_has_all.is_empty() {
                return Err(Error::at(&at, "has no condition, so it is not conditional")
                    .in_file(&f.path, Some(&f.source)));
            }
            // v0's one rule reads a token an earlier step has already
            // replaced, so it never fires. A pack that repeats the mistake is
            // refused rather than left to be discovered on a corpus.
            for w in c.when_has_any.iter().chain(c.when_has_all.iter()) {
                if let Some(to) = token_replacements.get(w) {
                    return Err(Error::at(
                        &at,
                        format!(
                            "waits for {w}, which an unconditional replacement has already turned into {to}, so it could never fire"
                        ),
                    )
                    .in_file(&f.path, Some(&f.source)));
                }
            }
            conditional.push(c);
        }
    }

    Ok(Normalizer {
        into,
        from,
        raw_removals: match m.get("raw_removals") {
            Some(v) => f.blame(yaml::texts(v, "raw_removals"))?,
            None => Vec::new(),
        },
        meaningful,
        to_space,
        remove,
        token_removals,
        token_replacements,
        conditional,
    })
}

/// A rule set written longhand (§6.5): the routes with their own logic, and
/// the axes whose tiers are not value-major. Nothing distinguishes a route
/// from any other rule set but its `enter_when`.
#[allow(clippy::too_many_arguments)]
fn load_rule_set(
    f: &File,
    axes: &[Axis],
    derived: &[Normalizer],
    flag_ix: &HashMap<String, usize>,
    parsers: &[ParserDef],
    parser_ix: &HashMap<String, usize>,
    buckets: &BTreeMap<String, Vec<String>>,
    regexes: &mut Vec<Regex>,
) -> R<RuleSet> {
    let m = f.blame(yaml::obj(&f.value, "rule_set"))?;
    let name = f.blame(yaml::text(
        yaml::get(m, "rule_set", "rule_set")?,
        "rule_set",
    ))?;
    let at = format!("rule set {name}");

    let mut decides = Vec::new();
    for a in f.blame(yaml::texts(yaml::get(m, "decides", &at)?, "decides"))? {
        decides.push(axes.iter().position(|x| x.name == a).ok_or_else(|| {
            Error::at("decides", format!("no axis named {a}")).in_file(&f.path, Some(&f.source))
        })?);
    }
    // A set decides axes of one phase (§7). What a stack is and what to do
    // with it are worked out at different moments, over different material,
    // and a set that mixed them would have to run at both.
    let phase = match decides.first() {
        Some(i) => axes[*i].phase,
        None => AxisPhase::Class,
    };
    if let Some(i) = decides.iter().find(|i| axes[**i].phase != phase) {
        return Err(Error::at(
            "decides",
            format!(
                "{} is a {} axis and {} is a {} one; a rule set decides one phase",
                axes[*i].name,
                axes[*i].phase.name(),
                axes[decides[0]].name,
                phase.name()
            ),
        )
        .in_file(&f.path, Some(&f.source)));
    }
    // Axes this set contributes to rather than decides: v0's branches replace
    // the construct list and add to the modifiers.
    let mut adds = Vec::new();
    if let Some(v) = m.get("adds") {
        for a in f.blame(yaml::texts(v, "adds"))? {
            let i = axes.iter().position(|x| x.name == a).ok_or_else(|| {
                Error::at("adds", format!("no axis named {a}")).in_file(&f.path, Some(&f.source))
            })?;
            if !decides.contains(&i) {
                return Err(Error::at("adds", format!("{a} is not in `decides`"))
                    .in_file(&f.path, Some(&f.source)));
            }
            if !axes[i].multi {
                return Err(Error::at(
                    "adds",
                    format!("{a} holds one value, so there is nothing to add to"),
                )
                .in_file(&f.path, Some(&f.source)));
            }
            adds.push(i);
        }
    }

    let compile_here = |body: &Value, at: &str, regexes: &mut Vec<Regex>| -> R<Expr> {
        let mut sc = Scope {
            derived,
            axes,
            parsers,
            parser_ix,
            buckets,
            within: None,
            flags: Some(flag_ix),
            regexes,
            deps: HashSet::new(),
        };
        compile(body, at, &mut sc)
    };

    let mut tiers: HashMap<String, f64> = DEFAULT_TIERS
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
    if let Some(t) = m.get("tiers") {
        for (k, v) in f.blame(yaml::obj(t, "tiers"))? {
            if !tiers.contains_key(k) {
                return Err(Error::at(format!("tiers.{k}"), "no clause has that tier")
                    .in_file(&f.path, Some(&f.source)));
            }
            tiers.insert(k.clone(), f.blame(yaml::number(v, "tiers"))?);
        }
    }

    let enter_when = match m.get("enter_when") {
        Some(w) => Some(f.blame(compile_here(w, &format!("{at}.enter_when"), regexes))?),
        None => None,
    };

    // Values the set works out per stack, before its rules run.
    let mut derives: Vec<Derive> = Vec::new();
    if let Some(d) = m.get("derive") {
        for (dname, spec) in f.blame(yaml::obj(d, &format!("{at}.derive")))? {
            let dat = format!("{at}.derive.{dname}");
            let dm = f.blame(yaml::obj(spec, &dat))?;
            let axis_name = f.blame(yaml::text(yaml::get(dm, "axis", &dat)?, &dat))?;
            let ai = axes
                .iter()
                .position(|x| x.name == axis_name)
                .ok_or_else(|| {
                    Error::at(&dat, format!("no axis named {axis_name}"))
                        .in_file(&f.path, Some(&f.source))
                })?;
            let mut cases = Vec::new();
            for (i, c) in f
                .blame(yaml::arr(yaml::get(dm, "cases", &dat)?, &dat))?
                .iter()
                .enumerate()
            {
                let cat = format!("{dat}.cases[{i}]");
                let cm = f.blame(yaml::obj(c, &cat))?;
                let id = f.blame(yaml::text(yaml::get(cm, "value", &cat)?, &cat))?;
                let value = axes[ai].value_index(&id).ok_or_else(|| {
                    Error::at(&cat, format!("{id} is not a value of the {axis_name} axis"))
                        .in_file(&f.path, Some(&f.source))
                })?;
                let when = match cm.get("when") {
                    Some(w) => Some(f.blame(compile_here(w, &format!("{cat}.when"), regexes))?),
                    None => None,
                };
                cases.push(DeriveCase { when, value });
            }
            if cases.is_empty() {
                return Err(Error::at(&dat, "has no cases").in_file(&f.path, Some(&f.source)));
            }
            derives.push(Derive {
                name: dname.clone(),
                cases,
            });
        }
    }

    let order = f.blame(yaml::texts(yaml::get(m, "order", &at)?, "order"))?;
    let bodies = f.blame(yaml::obj(yaml::get(m, "rules", &at)?, &at))?;
    let mut rules = Vec::with_capacity(order.len());
    for id in &order {
        let rat = format!("{at}.rules.{id}");
        let r = f.blame(yaml::obj(
            bodies
                .get(id)
                .ok_or_else(|| Error::at("order", format!("no rule named {id}")))?,
            &rat,
        ))?;

        let mut clauses = Vec::new();
        for (i, c) in f
            .blame(yaml::arr(yaml::get(r, "clauses", &rat)?, &rat))?
            .iter()
            .enumerate()
        {
            let cat = format!("{rat}.clauses[{i}]");
            let cm = f.blame(yaml::obj(c, &cat))?;
            let tier = match cm.get("tier") {
                None => Tier::Stated,
                Some(t) => match f.blame(yaml::text(t, &cat))?.as_str() {
                    "exclusive" => Tier::Exclusive,
                    "keywords" => Tier::Keywords,
                    "combination" => Tier::Combination,
                    "alternative" => Tier::Alternative,
                    "physics" => Tier::Physics,
                    "stated" => Tier::Stated,
                    other => {
                        return Err(Error::at(&cat, format!("{other} is not a tier"))
                            .in_file(&f.path, Some(&f.source)));
                    }
                },
            };
            let confidence = tiers.get(tier.name()).copied().unwrap_or(0.0);
            clauses.push(if let Some(flag) = cm.get("flag") {
                // Cites the flag, as an axis rule does.
                let name = f.blame(yaml::text(flag, &cat))?;
                Clause::Flag {
                    tier,
                    confidence,
                    flag: *flag_ix.get(&name).ok_or_else(|| {
                        Error::at(&cat, format!("{name} is not a flag of this pack"))
                            .in_file(&f.path, Some(&f.source))
                    })?,
                    name,
                }
            } else if let Some(kw) = cm.get("keywords") {
                // Cites the keyword that matched, which is what makes the
                // evidence worth reading.
                let field_name = match cm.get("field") {
                    Some(x) => f.blame(yaml::text(x, &cat))?,
                    None => "search_text".to_string(),
                };
                let field = resolve_field(derived, &field_name).ok_or_else(|| {
                    Error::at(&cat, format!("no field named {field_name}"))
                        .in_file(&f.path, Some(&f.source))
                })?;
                Clause::Keywords {
                    tier,
                    confidence,
                    field,
                    list: f.blame(yaml::texts(kw, &cat))?,
                }
            } else {
                Clause::When {
                    tier,
                    confidence,
                    cite: f.blame(yaml::text(yaml::get(cm, "cite", &cat)?, &cat))?,
                    source: f.blame(yaml::text(yaml::get(cm, "source", &cat)?, &cat))?,
                    expr: f.blame(compile_here(
                        yaml::get(cm, "when", &cat)?,
                        &format!("{cat}.when"),
                        regexes,
                    ))?,
                }
            });
        }
        if clauses.is_empty() {
            return Err(Error::at(&rat, "has no clauses, so it can never fire")
                .in_file(&f.path, Some(&f.source)));
        }

        let mut sets = Vec::new();
        for (axis_name, v) in f.blame(yaml::obj(yaml::get(r, "set", &rat)?, &rat))? {
            let sat = format!("{rat}.set.{axis_name}");
            let ai = axes
                .iter()
                .position(|x| x.name == *axis_name)
                .ok_or_else(|| {
                    Error::at(&sat, format!("no axis named {axis_name}"))
                        .in_file(&f.path, Some(&f.source))
                })?;
            if !decides.contains(&ai) {
                return Err(Error::at(
                    &sat,
                    format!("this rule set does not declare {axis_name} in `decides`"),
                )
                .in_file(&f.path, Some(&f.source)));
            }
            let items = match v {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            let mut values = Vec::with_capacity(items.len());
            for (j, item) in items.iter().enumerate() {
                let iat = format!("{sat}[{j}]");
                if let Value::Object(mm) = item
                    && let Some(from) = mm.get("from")
                {
                    let dname = f.blame(yaml::text(from, &iat))?;
                    let di = derives
                        .iter()
                        .position(|d| d.name == dname)
                        .ok_or_else(|| {
                            Error::at(&iat, format!("nothing derives {dname}"))
                                .in_file(&f.path, Some(&f.source))
                        })?;
                    values.push(SetValue {
                        value: Which::Derived(di),
                        when: match mm.get("when") {
                            Some(w) => {
                                Some(f.blame(compile_here(w, &format!("{iat}.when"), regexes))?)
                            }
                            None => None,
                        },
                    });
                    continue;
                }
                if item.is_null() {
                    values.push(SetValue {
                        value: Which::Nothing,
                        when: None,
                    });
                    continue;
                }
                let (id, when) = match item {
                    Value::Object(mm) if mm.contains_key("value") => (
                        f.blame(yaml::text(&mm["value"], &iat))?,
                        match mm.get("when") {
                            Some(w) => {
                                Some(f.blame(compile_here(w, &format!("{iat}.when"), regexes))?)
                            }
                            None => None,
                        },
                    ),
                    other => (f.blame(yaml::text(other, &iat))?, None),
                };
                // A value outside the axis's vocabulary fails the pack here,
                // which is what stops one name drifting into two.
                let value = axes[ai]
                    .value_index(&id)
                    .or_else(|| axes[ai].values.iter().position(|v| v.label == id))
                    .ok_or_else(|| {
                        Error::at(&iat, format!("{id} is not a value of the {axis_name} axis"))
                            .in_file(&f.path, Some(&f.source))
                    })?;
                values.push(SetValue {
                    value: Which::Fixed(value),
                    when,
                });
            }
            sets.push(Sets { axis: ai, values });
        }
        if sets.is_empty() {
            return Err(Error::at(&rat, "sets nothing").in_file(&f.path, Some(&f.source)));
        }

        let confidence = match r.get("confidence") {
            Some(c) => Some(f.blame(yaml::number(c, &rat))?),
            None => None,
        };
        rules.push(Rule {
            id: id.clone(),
            requires: None,
            clauses,
            sets,
            confidence,
            why: r.get("why").map(|w| yaml::text(w, &rat)).transpose()?,
        });
    }

    Ok(RuleSet {
        name,
        derives,
        adds,
        collect: m.get("collect").and_then(|c| c.as_bool()).unwrap_or(false),
        decides,
        enter_when,
        rules,
        phase,
    })
}

// ---------------------------------------------------------------------------
// Passes (§7). A pass file declares a configured instance of a kind the engine
// provides. Nothing here is an algorithm; everything here is the numbers one
// runs on.

fn load_pass(
    f: &File,
    axes: &[Axis],
    derived: &[Normalizer],
    buckets: &BTreeMap<String, Vec<String>>,
    regexes: &mut Vec<Regex>,
) -> R<Pass> {
    let m = f.blame(yaml::obj(&f.value, "pass"))?;
    let name = f.blame(yaml::text(yaml::get(m, "pass", "pass")?, "pass"))?;
    let at = format!("pass {name}");
    let kind_name = f.blame(yaml::text(yaml::get(m, "kind", &at)?, "kind"))?;
    let phase = match m.get("phase") {
        None => Phase::After,
        Some(p) => match f.blame(yaml::text(p, "phase"))?.as_str() {
            "before" => Phase::Before,
            "after" => Phase::After,
            other => {
                return Err(Error::at("phase", format!("before or after, not {other}"))
                    .in_file(&f.path, Some(&f.source)));
            }
        },
    };

    let axis_ix = |n: &str, at: &str| -> R<usize> {
        axes.iter()
            .position(|a| a.name == n)
            .ok_or_else(|| Error::at(at, format!("no axis named {n}")))
    };
    // A pass runs where the rules have already run and only the fingerprint
    // and the decided axes are at hand, so its expressions may read those and
    // nothing else. A pack that names a flag here is refused with the line
    // rather than quietly reading false.
    let no_parsers: Vec<ParserDef> = Vec::new();
    let no_parser_ix: HashMap<String, usize> = HashMap::new();
    let mut compile_here = |body: &Value, at: &str, regexes: &mut Vec<Regex>| -> R<Expr> {
        let mut sc = Scope {
            derived,
            axes,
            parsers: &no_parsers,
            parser_ix: &no_parser_ix,
            buckets,
            within: None,
            flags: None,
            regexes,
            deps: HashSet::new(),
        };
        compile(body, at, &mut sc)
    };

    let target = match m.get("target") {
        None => None,
        Some(t) => {
            let tm = f.blame(yaml::obj(t, &format!("{at}.target")))?;
            let w = yaml::get(tm, "when", &format!("{at}.target"))?;
            Some(f.blame(compile_here(w, &format!("{at}.target.when"), regexes))?)
        }
    };

    // --- the reference
    let r = f.blame(yaml::obj(yaml::get(m, "reference", &at)?, "reference"))?;
    let scope = f.blame(yaml::text(
        yaml::get(r, "scope", &format!("{at}.reference"))?,
        "reference.scope",
    ))?;
    let mut filter = Vec::new();
    if let Some(fl) = r.get("filter") {
        for (k, v) in f.blame(yaml::obj(fl, &format!("{at}.reference.filter")))? {
            let path = format!("{at}.reference.filter.{k}");
            let what = match axes.iter().position(|a| a.name == *k) {
                Some(i) => What::Axis(i),
                None => match resolve_field(derived, k) {
                    Some(i) => What::Field(i),
                    None => {
                        return Err(Error::at(
                            path,
                            format!("{k} is neither an axis of this pack nor a field"),
                        )
                        .in_file(&f.path, Some(&f.source)));
                    }
                },
            };
            let mut cond = Cond {
                what,
                is: Vec::new(),
                not: Vec::new(),
                present: None,
            };
            match v {
                Value::Object(o) => {
                    if let Some(x) = o.get("is") {
                        cond.is = f.blame(yaml::texts(x, &path))?;
                    }
                    if let Some(x) = o.get("not") {
                        cond.not = f.blame(yaml::texts(x, &path))?;
                    }
                    if let Some(x) = o.get("present") {
                        cond.present = x.as_bool();
                    }
                }
                other => cond.is = f.blame(yaml::texts(other, &path))?,
            }
            filter.push(cond);
        }
    }
    let partition_by = match r.get("partition_by") {
        None => None,
        Some(v) => {
            let n = f.blame(yaml::text(v, &format!("{at}.reference.partition_by")))?;
            Some(f.blame(axis_ix(&n, &format!("{at}.reference.partition_by")))?)
        }
    };
    let reference = Reference {
        scope,
        filter,
        partition_by,
        fallback: matches!(r.get("fallback").and_then(|v| v.as_str()), Some("global")),
        fallback_when: match r.get("fallback_when") {
            Some(v) => f.blame(yaml::texts(v, &format!("{at}.reference.fallback_when")))?,
            None => Vec::new(),
        },
        fallback_except: match r.get("fallback_except") {
            Some(v) => f.blame(yaml::texts(v, &format!("{at}.reference.fallback_except")))?,
            None => Vec::new(),
        },
    };

    // --- what it writes down about itself
    let emit = match m.get("emit") {
        None => Emit {
            evidence: true,
            review_below: 0.0,
            review_all_touched: false,
        },
        Some(e) => {
            let em = f.blame(yaml::obj(e, &format!("{at}.emit")))?;
            Emit {
                evidence: !matches!(em.get("evidence").and_then(|v| v.as_str()), Some("never")),
                review_below: match em.get("review_item_below") {
                    Some(v) => f.blame(yaml::number(v, &format!("{at}.emit.review_item_below")))?,
                    None => 0.0,
                },
                review_all_touched: em
                    .get("review_all_touched")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            }
        }
    };

    let kind = match kind_name.as_str() {
        "nearest_neighbour_vote" => Kind::Vote(load_vote(
            f,
            m,
            &at,
            axes,
            derived,
            &mut compile_here,
            regexes,
        )?),
        other => {
            return Err(Error::at(
                "kind",
                format!(
                    "{other} is not a pass kind this engine provides; it has nearest_neighbour_vote"
                ),
            )
            .in_file(&f.path, Some(&f.source)));
        }
    };

    Ok(Pass {
        name,
        phase,
        kind,
        target,
        reference,
        emit,
    })
}

fn load_vote(
    f: &File,
    m: &serde_json::Map<String, Value>,
    at: &str,
    axes: &[Axis],
    derived: &[Normalizer],
    compile_here: &mut impl FnMut(&Value, &str, &mut Vec<Regex>) -> R<Expr>,
    regexes: &mut Vec<Regex>,
) -> R<Vote> {
    // --- the key: which fields, and how a value is binned
    let mut dims: Vec<KeyDim> = Vec::new();
    for (n, spec) in f.blame(yaml::obj(yaml::get(m, "key", at)?, "key"))? {
        let path = format!("{at}.key.{n}");
        let s = f.blame(yaml::obj(spec, &path))?;
        let field = resolve_field(derived, n)
            .ok_or_else(|| Error::at(&path, format!("no field named {n}")))
            .map_err(|e| e.in_file(&f.path, Some(&f.source)))?;
        let half_even = match s.get("rounding") {
            None => false,
            Some(r) => match f.blame(yaml::text(r, &path))?.as_str() {
                "half_even" => true,
                "half_away" => false,
                other => {
                    return Err(Error::at(
                        format!("{path}.rounding"),
                        format!("half_even or half_away, not {other}"),
                    )
                    .in_file(&f.path, Some(&f.source)));
                }
            },
        };
        dims.push(KeyDim {
            name: n.clone(),
            field,
            round: s
                .get("round")
                .map(|x| f.blame(yaml::number(x, &path)))
                .transpose()?,
            ceil: s
                .get("ceil")
                .map(|x| f.blame(yaml::number(x, &path)))
                .transpose()?,
            half_even,
        });
    }
    if dims.is_empty() || dims.len() > 5 {
        return Err(Error::at(
            format!("{at}.key"),
            format!(
                "a vote is binned on one to five dimensions, not {}",
                dims.len()
            ),
        )
        .in_file(&f.path, Some(&f.source)));
    }
    let dim_ix = |n: &str, at: &str| -> R<usize> {
        dims.iter()
            .position(|d| d.name == n)
            .ok_or_else(|| Error::at(at, format!("no key dimension named {n}")))
    };

    // --- how far the search goes when the bin is empty
    let w = f.blame(yaml::obj(yaml::get(m, "widen", at)?, "widen"))?;
    let max_distance = f.blame(yaml::number(
        yaml::get(w, "max_distance", &format!("{at}.widen"))?,
        "widen.max_distance",
    ))? as i64;
    let mut pairs = Vec::new();
    if let Some(ps) = w.get("pairs") {
        for (i, pair) in f
            .blame(yaml::arr(ps, &format!("{at}.widen.pairs")))?
            .iter()
            .enumerate()
        {
            let path = format!("{at}.widen.pairs[{i}]");
            let two = f.blame(yaml::texts(pair, &path))?;
            if two.len() != 2 {
                return Err(
                    Error::at(&path, "a pair is two dimensions").in_file(&f.path, Some(&f.source))
                );
            }
            pairs.push((
                f.blame(dim_ix(&two[0], &path))?,
                f.blame(dim_ix(&two[1], &path))?,
            ));
        }
    }
    let mut relaxed = None;
    if let Some(rs) = w.get("relaxed")
        && let Some(first) = f
            .blame(yaml::arr(rs, &format!("{at}.widen.relaxed")))?
            .first()
    {
        let path = format!("{at}.widen.relaxed[0]");
        let rm = f.blame(yaml::obj(first, &path))?;
        let anchor = f.blame(yaml::text(yaml::get(rm, "requires", &path)?, &path))?;
        let vary = f.blame(yaml::obj(yaml::get(rm, "vary", &path)?, &path))?;
        let spans: Vec<(&String, f64)> = vary
            .iter()
            .map(|(k, v)| f.blame(yaml::number(v, &path)).map(|n| (k, n)))
            .collect::<R<_>>()?;
        if spans.len() != 2 || *spans[0].0 != anchor {
            return Err(
                Error::at(&path, format!("vary names two dimensions, {anchor} first"))
                    .in_file(&f.path, Some(&f.source)),
            );
        }
        relaxed = Some(Relaxed {
            requires: f.blame(dim_ix(spans[0].0, &path))?,
            requires_span: spans[0].1 as i64,
            other: f.blame(dim_ix(spans[1].0, &path))?,
            other_span: spans[1].1 as i64,
            method: match rm.get("method") {
                Some(v) => f.blame(yaml::text(v, &path))?,
                None => "expanded_relaxed".to_string(),
            },
        });
    }

    // --- what the vote is about, and when the winner is written
    let d = f.blame(yaml::obj(yaml::get(m, "decide", at)?, "decide"))?;
    let axis_ix = |n: &str, at: &str| -> R<usize> {
        axes.iter()
            .position(|a| a.name == n)
            .ok_or_else(|| Error::at(at, format!("no axis named {n}")))
    };
    let named = |key: &str| -> R<Vec<usize>> {
        let path = format!("{at}.decide.{key}");
        f.blame(yaml::texts(yaml::get(d, key, &path)?, &path))?
            .iter()
            .map(|n| f.blame(axis_ix(n, &path)))
            .collect()
    };
    let vote_on = named("vote_on")?;
    if vote_on.len() > crate::pass::MAX_VOTE_AXES {
        return Err(Error::at(
            format!("{at}.decide.vote_on"),
            format!(
                "a vote is about at most {} axes, not {}",
                crate::pass::MAX_VOTE_AXES,
                vote_on.len()
            ),
        )
        .in_file(&f.path, Some(&f.source)));
    }
    for a in &vote_on {
        if axes[*a].values.len() >= u16::MAX as usize {
            return Err(Error::at(
                format!("{at}.decide.vote_on"),
                format!("{} has too many values to vote on", axes[*a].name),
            )
            .in_file(&f.path, Some(&f.source)));
        }
    }
    let writes = named("writes")?;
    for a in &writes {
        if !vote_on.contains(a) {
            return Err(Error::at(
                format!("{at}.decide.writes"),
                format!(
                    "{} is written but not voted on; a pass writes what it decided",
                    axes[*a].name
                ),
            )
            .in_file(&f.path, Some(&f.source)));
        }
    }
    let on_tie = match d.get("on_tie") {
        None => OnTie::Nothing,
        Some(v) => match f
            .blame(yaml::text(v, &format!("{at}.decide.on_tie")))?
            .as_str()
        {
            "none" => OnTie::Nothing,
            "first_seen" => OnTie::FirstSeen,
            other => {
                return Err(Error::at(
                    format!("{at}.decide.on_tie"),
                    format!("none or first_seen, not {other}"),
                )
                .in_file(&f.path, Some(&f.source)));
            }
        },
    };
    let write_when = match d.get("write_when") {
        None => Vec::new(),
        Some(v) => {
            let wm = f.blame(yaml::obj(v, &format!("{at}.decide.write_when")))?;
            match wm.get("missing_or") {
                Some(x) => f.blame(yaml::texts(x, &format!("{at}.decide.write_when")))?,
                None => Vec::new(),
            }
        }
    };

    // --- the compatibility filter, which is what makes this kind offer a
    // second subject: its rules judge the candidate, not the stack.
    let c = f.blame(yaml::obj(
        yaml::get(m, "compatibility", at)?,
        "compatibility",
    ))?;
    let path = format!("{at}.compatibility");
    let subject = f.blame(yaml::obj(yaml::get(c, "subject", &path)?, &path))?;
    let sf = f.blame(yaml::text(
        yaml::get(subject, "text", &path)?,
        &format!("{path}.subject.text"),
    ))?;
    let subject_field = resolve_field(derived, &sf)
        .ok_or_else(|| Error::at(&path, format!("no field named {sf}")))
        .map_err(|e| e.in_file(&f.path, Some(&f.source)))?;
    let subject_case = match subject.get("case") {
        None => Case::Raw,
        Some(v) => Case::parse(&f.blame(yaml::text(v, &path))?).ok_or_else(|| {
            Error::at(&path, "case is raw, lower or upper").in_file(&f.path, Some(&f.source))
        })?,
    };
    let default_family = match c.get("default_family") {
        Some(v) => f.blame(yaml::text(v, &path))?,
        None => "OTHER".to_string(),
    };
    let mut family_of = HashMap::new();
    if let Some(fo) = c.get("family_of") {
        for (k, v) in f.blame(yaml::obj(fo, &format!("{path}.family_of")))? {
            family_of.insert(k.clone(), f.blame(yaml::text(v, &path))?);
        }
    }
    let mut rules = Vec::new();
    for (i, rv) in f
        .blame(yaml::arr(
            yaml::get(c, "rules", &path)?,
            &format!("{path}.rules"),
        ))?
        .iter()
        .enumerate()
    {
        let rp = format!("{path}.rules[{i}]");
        let rm = f.blame(yaml::obj(rv, &rp))?;
        rules.push(CompatRule {
            when: compile_here(yaml::get(rm, "when", &rp)?, &format!("{rp}.when"), regexes)?,
            allow: compile_here(
                yaml::get(rm, "allow", &rp)?,
                &format!("{rp}.allow"),
                regexes,
            )?,
        });
    }

    // Which of the voted axes the filter judges. Declared, never guessed: a
    // family table names values, and two axes can both have a value called
    // SWI, so guessing picks the wrong one and every verdict after it is
    // judged against the wrong vocabulary.
    let judged = f.blame(yaml::text(
        yaml::get(c, "judges", &path)?,
        &format!("{path}.judges"),
    ))?;
    let judged_axis = f.blame(axis_ix(&judged, &format!("{path}.judges")))?;
    let compat_axis = vote_on
        .iter()
        .position(|a| *a == judged_axis)
        .ok_or_else(|| {
            Error::at(
                format!("{path}.judges"),
                format!("{judged} is not one of the axes this pass votes on"),
            )
        })
        .map_err(|e| e.in_file(&f.path, Some(&f.source)))?;

    Ok(Vote {
        dims,
        max_distance,
        pairs,
        relaxed,
        min_matches: f.blame(yaml::number(
            yaml::get(d, "min_matches", &format!("{at}.decide"))?,
            "decide.min_matches",
        ))? as usize,
        on_tie,
        vote_on,
        writes,
        write_when,
        compat: Compat {
            subject_field,
            subject_case,
            default_family,
            family_of,
            rules,
        },
        compat_axis,
    })
}
