// SPDX-License-Identifier: AGPL-3.0-only
//! packeval: evaluate a prototype pack against real stacks.
//!
//!   packeval flags  --pack DIR --fp FILE.csv                > flags.tsv
//!   packeval branch --pack DIR --fp FILE.csv --scc FILE.csv > branch.tsv
//!   packeval vote   --pack DIR --fp FILE.csv --scc FILE.csv > vote.tsv
//!
//! Nothing here knows anything about MRI. Every fact about MRI is in the pack.

mod expr;
mod pack;
mod row;
mod vote;

use expr::{Case, Ctx, Subject};
use pack::{Pack, Slot};
use regex::Regex;
use row::{F_FLIP, F_N_INSTANCES, F_TE, F_TI, F_TR, Fingerprint, Verdict};
use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Write};
use std::path::Path;

/// One stack, mid-evaluation.
struct Stack<'a> {
    pack: &'a Pack,
    fp: &'a Fingerprint,
    raws: Vec<String>,
    tokens: Vec<HashSet<String>>,
    preds: Vec<Vec<bool>>,
    flags: Vec<bool>,
}

impl<'a> Stack<'a> {
    fn new(pack: &'a Pack, fp: &'a Fingerprint) -> Stack<'a> {
        let mut raws = Vec::with_capacity(pack.parsers.len());
        let mut tokens = Vec::with_capacity(pack.parsers.len());
        for p in &pack.parsers {
            let src = field_text(fp, p.field);
            let raw = p.case.apply(src).into_owned();
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
        let mut s = Stack {
            pack,
            fp,
            raws,
            tokens,
            preds: Vec::new(),
            flags: vec![false; pack.flags.len()],
        };
        // Predicates, parser by parser, in file order. `s.preds` grows as it
        // goes, so a predicate may name an earlier one, of its own parser or
        // of a parser already done, and a forward reference reads false,
        // which the loader is what stops.
        for (pi, p) in pack.parsers.iter().enumerate() {
            s.preds.push(Vec::with_capacity(p.preds.len()));
            for e in &p.preds {
                let v = {
                    let subj = Subject {
                        raw: &s.raws[pi],
                        tokens: Some(&s.tokens[pi]),
                    };
                    e.eval(Some(&subj), &s)
                };
                s.preds[pi].push(v);
            }
        }
        // flags, in dependency order
        for i in &pack.flag_order {
            let val = pack.flags[*i].eval(None, &s);
            s.flags[*i] = val;
        }
        s
    }
}

fn field_text(fp: &Fingerprint, i: usize) -> &str {
    if i < 10 { "" } else { fp.s(i) }
}

impl Ctx for Stack<'_> {
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
        if field < 10 {
            self.fp.num[field]
        } else {
            self.fp.s(field).trim().parse().ok()
        }
    }
    fn scalar(&self, field: usize) -> Option<&str> {
        if field < 10 {
            self.fp.num[field].map(|_| "")
        } else {
            let s = self.fp.s(field);
            if s.is_empty() { None } else { Some(s) }
        }
    }
    fn text(&self, field: usize, case: Case) -> std::borrow::Cow<'_, str> {
        case.apply(field_text(self.fp, field))
    }
    fn re(&self, idx: usize) -> &Regex {
        &self.pack.regexes[idx]
    }
}

// ---------------------------------------------------------------------------

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).cloned().unwrap_or_default();
    let packdir = arg(&args, "--pack").unwrap_or_else(|| "pack".into());
    let fpfile = arg(&args, "--fp").expect("--fp");
    let sccfile = arg(&args, "--scc");

    let pack = match Pack::load(Path::new(&packdir)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pack will not load: {e}");
            std::process::exit(2);
        }
    };
    eprintln!(
        "pack {}@{}: {} parsers, {} predicates, {} flags, {} branches, {} passes",
        pack.name,
        pack.version,
        pack.parsers.len(),
        pack.parsers.iter().map(|p| p.preds.len()).sum::<usize>(),
        pack.flags.len(),
        pack.branches.len(),
        pack.passes.len(),
    );

    let fps = row::read_fingerprints(&fpfile).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    eprintln!("{} stacks", fps.len());

    let out = std::io::stdout();
    let mut w = BufWriter::with_capacity(1 << 20, out.lock());

    match mode.as_str() {
        "flags" => run_flags(&pack, &fps, &mut w),
        "branch" => {
            let scc = load_scc(&sccfile);
            run_branch(&pack, &fps, &scc, &mut w);
        }
        "vote" => {
            let scc = load_scc(&sccfile);
            run_vote(&pack, &fps, &scc, &mut w);
        }
        _ => {
            eprintln!("usage: packeval flags|branch|vote --pack DIR --fp FILE [--scc FILE]");
            std::process::exit(2);
        }
    }
    w.flush().ok();
}

fn load_scc(path: &Option<String>) -> Vec<Verdict> {
    let p = path.as_ref().expect("--scc");
    row::read_verdicts(p).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    })
}

fn run_flags(pack: &Pack, fps: &[Fingerprint], w: &mut impl Write) {
    for fp in fps {
        let s = Stack::new(pack, fp);
        let on: Vec<&str> = pack
            .flag_names
            .iter()
            .enumerate()
            .filter(|(i, _)| s.flags[*i])
            .map(|(_, n)| n.as_str())
            .collect();
        writeln!(w, "{}\t{}", fp.id, on.join(",")).ok();
    }
}

fn resolve(slot: &Option<Slot>, derived: &[String]) -> String {
    match slot {
        None => String::new(),
        Some(Slot::Literal(s)) => s.clone(),
        Some(Slot::Derived(i)) => derived[*i].clone(),
    }
}

fn run_branch(pack: &Pack, fps: &[Fingerprint], scc: &[Verdict], w: &mut impl Write) {
    let Some(branch) = pack.branches.first() else {
        eprintln!("the pack has no branch");
        return;
    };
    let prov: HashMap<i64, &str> = scc
        .iter()
        .map(|v| (v.id, v.provenance.as_str()))
        .collect();
    let mut n = 0usize;
    for fp in fps {
        if prov.get(&fp.id).copied().unwrap_or("") != branch.enter_provenance {
            continue;
        }
        let s = Stack::new(pack, fp);
        let derived: Vec<String> = branch
            .derives
            .iter()
            .map(|cases| {
                for c in cases {
                    match &c.when {
                        None => return c.value.clone(),
                        Some(e) if e.eval(None, &s) => return c.value.clone(),
                        _ => {}
                    }
                }
                String::new()
            })
            .collect();
        for r in &branch.rules {
            let Some(cl) = r.clauses.iter().find(|c| c.when.eval(None, &s)) else {
                continue;
            };
            let construct: Vec<&str> = r
                .construct
                .iter()
                .filter(|v| v.when.as_ref().is_none_or(|e| e.eval(None, &s)))
                .map(|v| v.value.as_str())
                .collect();
            writeln!(
                w,
                "{}\t{}\t{}\t{}\t{}\t{:.2}\t{}\t{}",
                fp.id,
                r.id,
                resolve(&r.base, &derived),
                construct.join(","),
                resolve(&r.technique, &derived),
                r.confidence,
                cl.source,
                cl.cite,
            )
            .ok();
            n += 1;
            break;
        }
    }
    eprintln!("{n} stacks entered the {} branch", branch.name);
}

fn run_vote(pack: &Pack, fps: &[Fingerprint], scc: &[Verdict], w: &mut impl Write) {
    let Some(p) = pack.passes.first() else {
        eprintln!("the pack has no pass");
        return;
    };
    let verdict: HashMap<i64, &Verdict> = scc.iter().map(|v| (v.id, v)).collect();

    // Intern the answers so a bin is small and a comparison is an integer.
    let mut names: Vec<String> = Vec::new();
    let mut name_ix: HashMap<String, u32> = HashMap::new();
    let mut intern = |s: &str, names: &mut Vec<String>| -> u32 {
        if let Some(i) = name_ix.get(s) {
            return *i;
        }
        let i = names.len() as u32;
        names.push(s.to_string());
        name_ix.insert(s.to_string(), i);
        i
    };

    let vals = |fp: &Fingerprint| -> Vec<Option<f64>> {
        vec![
            fp.num[F_TR],
            fp.num[F_TE],
            fp.num[F_TI],
            fp.num[F_FLIP],
            fp.num[F_N_INSTANCES],
        ]
    };

    let mut pools: HashMap<String, vote::Pool> = HashMap::new();
    let mut global = vote::Pool::default();
    let mut refs = 0usize;
    for fp in fps {
        let Some(v) = verdict.get(&fp.id) else {
            continue;
        };
        if fp.s(row::F_MODALITY) != "MR" {
            continue;
        }
        if v.base.is_empty() || v.base == "Unknown" || v.technique.is_empty() || v.technique == "Unknown" {
            continue;
        }
        if v.directory_type.is_empty() || v.directory_type == "excluded" {
            continue;
        }
        let r = vote::Ref {
            base: intern(&v.base, &mut names),
            technique: intern(&v.technique, &mut names),
        };
        let key = vote::key_of(p, &vals(fp));
        pools.entry(v.directory_type.clone()).or_default().add(key, r);
        global.add(key, r);
        refs += 1;
    }
    eprintln!(
        "reference: {refs} stacks, {} pools, {} bins in the global pool",
        pools.len(),
        global.bins.len()
    );

    for fp in fps {
        if fp.s(row::F_MODALITY) != "MR" {
            continue;
        }
        let dt = verdict.get(&fp.id).map(|v| v.directory_type.as_str()).unwrap_or("");
        let key = vote::key_of(p, &vals(fp));
        let seq = fp.s(19);
        let pool = pools.get(dt).unwrap_or(&global);
        let mut o = vote::vote(p, pool, &key, seq, &names, &pack.regexes);
        let mut which = if pools.contains_key(dt) { "scoped" } else { "global" };
        let fell_back = matches!(
            o.method,
            "no_match" | "insufficient_matches" | "no_compatible_match"
        ) && !p.fallback_except.iter().any(|x| x == dt);
        if fell_back {
            o = vote::vote(p, &global, &key, seq, &names, &pack.regexes);
            which = "global";
        }
        writeln!(
            w,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            fp.id,
            o.method,
            o.base.map(|i| names[i as usize].as_str()).unwrap_or(""),
            o.technique.map(|i| names[i as usize].as_str()).unwrap_or(""),
            o.matches,
            o.total_in_bin,
            which,
        )
        .ok();
    }
}
