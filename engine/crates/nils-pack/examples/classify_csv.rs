// SPDX-License-Identifier: AGPL-3.0-only

//! Evaluate a pack over a CSV of stacks, with no registry in sight.
//!
//!   classify_csv --pack DIR --csv FILE [--overlay FILE] [--axis NAME]
//!
//! One row out per stack: the id, then for each axis its value, the tier that
//! decided it and what the evidence cites. This is how a pack is checked
//! against a corpus by someone who has never seen our schema, and it is how
//! this pack is checked against v0.
//!
//! Column names are the fingerprint's (`nils_pack::stack::FIELDS`). A v0
//! export uses its own names for the same things, so those are accepted too.

use std::io::{BufWriter, Write};

/// v0's column name for the same field, where it differs.
const ALIASES: &[(&str, &str)] = &[
    ("stack_sequence_name", "text_sequence_name"),
    ("text_search_blob", "text_all"),
    ("contrast_search_blob", "text_contrast"),
    ("mr_te", "echo_time"),
    ("mr_tr", "repetition_time"),
    ("mr_ti", "inversion_time"),
    ("mr_flip_angle", "flip_angle"),
    ("mr_echo_train_length", "echo_train_length"),
    ("mr_echo_number", "echo_numbers"),
    ("mr_acquisition_type", "mr_acquisition_type"),
    ("mr_diffusion_b_value", "diffusion_b_value"),
    ("stack_n_instances", "n_instances"),
    ("manufacturer_model", "manufacturer_model_name"),
    ("stack_orientation", "orientation"),
];

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = arg(&args, "--pack").expect("--pack DIR");
    let csv_path = arg(&args, "--csv").expect("--csv FILE");
    let only = arg(&args, "--axis");

    let overlay = arg(&args, "--overlay").map(|p| {
        nils_pack::Overlay::load(std::path::Path::new(&p)).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        })
    });
    let pack = nils_pack::load(std::path::Path::new(&dir), overlay.as_ref()).unwrap_or_else(|e| {
        eprintln!("the pack does not load:\n{e}");
        std::process::exit(2)
    });
    eprintln!(
        "{}: {} flags, {} axes, {} rule sets",
        pack.id(),
        pack.flags.len(),
        pack.axes.len(),
        pack.rule_sets.len()
    );

    let mut rdr = csv::Reader::from_path(&csv_path).unwrap_or_else(|e| {
        eprintln!("{csv_path}: {e}");
        std::process::exit(2)
    });
    let head = rdr.headers().expect("a header row").clone();

    // Which column feeds which field, and which column is the id.
    let mut id_col = None;
    let mut cols: Vec<(usize, String)> = Vec::new();
    for (i, name) in head.iter().enumerate() {
        if name == "series_stack_id" || name == "stack_id" || name == "id" {
            id_col = Some(i);
            continue;
        }
        let field = ALIASES
            .iter()
            .find(|(from, _)| *from == name)
            .map(|(_, to)| *to)
            .unwrap_or(name);
        if nils_pack::stack::field_index(field).is_some() {
            cols.push((i, field.to_string()));
        }
    }
    let id_col = id_col.unwrap_or_else(|| {
        eprintln!("{csv_path}: no id column (series_stack_id, stack_id or id)");
        std::process::exit(2)
    });
    eprintln!("{} columns feed a field", cols.len());

    // With --passes, the phases that read more than one stack run too, over
    // the corpus in the file: the reference is the CSV, which is what makes
    // the result comparable with v0's over the same rows.
    let with_passes = args.iter().any(|a| a == "--passes");
    // v0 votes for every MR stack whether or not the answer is written, so a
    // comparison asks for every one of them.
    let vote_all = args.iter().any(|a| a == "--vote-all");
    let mut held: Vec<Held> = Vec::new();

    let out = std::io::stdout();
    let mut w = BufWriter::with_capacity(1 << 20, out.lock());
    let mut n = 0u64;
    // The queue the pack's own policy would raise over this corpus, counted
    // by the engine's rule and not by a script that reimplements it.
    let mut queue: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut tiers: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    let mut asked = 0u64;
    let mut silent = 0u64;
    let mut stacks = 0u64;
    for rec in rdr.records() {
        let r = rec.expect("a row");
        let mut stack = nils_pack::Stack::new();
        for (i, field) in &cols {
            let v = r.get(*i).unwrap_or("");
            stack
                .set(field, nils_pack::stack::Value::Text(Some(v)))
                .expect("a field the header named");
        }
        let ev = nils_pack::Evaluated::new(&pack, &stack);
        let id = r.get(id_col).unwrap_or("");
        // `--axis <name>` also names a text the pack derives, so the
        // normalizer can be checked the same way an axis is.
        if let Some(name) = only.as_deref()
            && let Some(text) = ev.derived_text(name)
        {
            writeln!(w, "{id}\t{name}\t{text}\t\t").ok();
            n += 1;
            continue;
        }
        let verdict = ev.classify();
        stacks += 1;
        if with_passes {
            held.push(Held {
                id: id.to_string(),
                fields: (0..nils_pack::stack::FIELDS.len())
                    .map(|i| stack.text(i).to_string())
                    .collect(),
                axes: pack
                    .axes
                    .iter()
                    .map(|a| {
                        verdict
                            .axis(&a.name)
                            .map(|v| v.stored())
                            .unwrap_or_default()
                    })
                    .collect(),
            });
        }
        silent += u64::from(verdict.silent);
        let mut any = false;
        for axis in &pack.axes {
            let found = verdict.axis(&axis.name);
            let value = found.map(|a| a.stored()).unwrap_or_default();
            if let Some(a) = found {
                *tiers
                    .entry(format!("{}:{}", axis.name, a.tier))
                    .or_default() += 1;
            }
            if verdict.silent {
                continue;
            }
            let kind = if value.is_empty() {
                pack.review
                    .asks_when_missing(&axis.name)
                    .then_some("missing")
            } else {
                let c = found.map(|a| a.confidence).unwrap_or(0.0);
                (c > 0.0 && c < pack.review.below(&axis.name)).then_some("low_confidence")
            };
            if let Some(kind) = kind {
                *queue.entry(format!("{}:{kind}", axis.name)).or_default() += 1;
                any = true;
            }
        }
        asked += u64::from(any);
        // A row per axis per stack, whether or not the axis resolved: an
        // axis that resolved to nothing is a fact, and leaving the row out
        // would make it look like a stack nobody judged.
        for axis in &pack.axes {
            if only.as_deref().is_some_and(|o| *o != axis.name) {
                continue;
            }
            let found = verdict.axis(&axis.name);
            let cited = verdict
                .evidence
                .iter()
                .find(|e| e.axis == axis.name)
                .map(|e| (e.tier.as_str(), e.matched.as_str()))
                .unwrap_or(("", ""));
            // The confidence is the sixth column: it decides nothing about
            // the verdict, and it is what says whether a person is asked.
            writeln!(
                w,
                "{id}\t{}\t{}\t{}\t{}\t{}",
                axis.name,
                found.map(|a| a.stored()).unwrap_or_default(),
                cited.0,
                cited.1,
                found
                    .map(|a| format!("{:.2}", a.confidence))
                    .unwrap_or_default(),
            )
            .ok();
        }
        n += 1;
    }
    w.flush().ok();
    if with_passes {
        run_passes(&pack, &held, vote_all, &mut w);
    }
    eprintln!("{n} stacks");
    if stacks > 0 {
        let items: u64 = queue.values().sum();
        eprintln!(
            "queue: {items} item(s) over {asked} stack(s), {:.2}% of {stacks}; {silent} ruled out",
            100.0 * asked as f64 / stacks as f64
        );
        let mut by: Vec<(&String, &u64)> = queue.iter().collect();
        by.sort_by_key(|(_, n)| -(**n as i64));
        for (kind, count) in by {
            eprintln!("  {kind:<28} {count:>8}");
        }
        let mut weak: Vec<(&String, &u64)> = tiers
            .iter()
            .filter(|(k, _)| k.ends_with(":physics") || k.ends_with(":default"))
            .collect();
        weak.sort_by_key(|(_, n)| -(**n as i64));
        for (what, count) in weak.iter().take(4) {
            eprintln!("  {what:<28} {count:>8}   decided with no keyword");
        }
    }
}

/// One classified stack, kept for the passes: what the fingerprint said, and
/// what the rules decided.
struct Held {
    id: String,
    fields: Vec<String>,
    axes: Vec<String>,
}

/// The subject a pass reads: fields and decided axes, and nothing else.
struct Row<'a> {
    fields: &'a [String],
    axes: &'a [String],
}

impl nils_pack::expr::Ctx for Row<'_> {
    fn pred(&self, _parser: usize, _pred: usize) -> bool {
        unreachable!("a pass may not name a parser predicate")
    }
    fn subject(&self, _parser: usize) -> nils_pack::expr::Subject<'_> {
        unreachable!("a pass may not name a parser")
    }
    fn flag(&self, _flag: usize) -> bool {
        unreachable!("a pass may not name a flag")
    }
    fn num(&self, field: usize) -> Option<f64> {
        self.fields.get(field).and_then(|s| s.parse().ok())
    }
    fn present(&self, field: usize) -> bool {
        self.fields.get(field).is_some_and(|s| !s.is_empty())
    }
    fn text(&self, field: usize) -> &str {
        self.fields.get(field).map(String::as_str).unwrap_or("")
    }
    fn re(&self, _idx: usize) -> &nils_pack::expr::Regex {
        unreachable!("a pass carries no patterns of its own")
    }
    fn axis_is(&self, axis: usize, value: &str) -> bool {
        self.axes.get(axis).is_some_and(|v| v == value)
    }
    fn axis_empty(&self, axis: usize) -> bool {
        self.axes.get(axis).is_none_or(|v| v.is_empty())
    }
}

fn cond_holds(c: &nils_pack::pass::Cond, row: &Row<'_>) -> bool {
    use nils_pack::pass::What;
    let value = match c.what {
        What::Field(i) => row.fields.get(i).map(String::as_str),
        What::Axis(i) => row.axes.get(i).map(String::as_str),
    }
    .filter(|v| !v.is_empty());
    if let Some(want) = c.present
        && value.is_some() != want
    {
        return false;
    }
    match value {
        Some(v) => {
            (c.is.is_empty() || c.is.iter().any(|w| w == v)) && !c.not.iter().any(|w| w == v)
        }
        None => c.is.is_empty(),
    }
}

/// Every pass of the pack over the corpus in the file, written in the shape
/// the referee writes v0's, so the two diff row by row.
fn run_passes(pack: &nils_pack::Pack, held: &[Held], vote_all: bool, w: &mut impl Write) {
    use nils_pack::pass::{Pool, key_of, take};
    use std::collections::HashMap;

    for pass in &pack.passes {
        let Some(vote) = pass.vote() else { continue };
        let mut pools: HashMap<String, Pool> = HashMap::new();
        let mut global = Pool::default();
        for h in held {
            let row = Row {
                fields: &h.fields,
                axes: &h.axes,
            };
            if !pass.reference.filter.iter().all(|c| cond_holds(c, &row)) {
                continue;
            }
            let answer: Option<Vec<usize>> = vote
                .vote_on
                .iter()
                .map(|a| {
                    let axis = &pack.axes[*a];
                    (0..axis.values.len()).find(|i| axis.stored(*i) == h.axes[*a])
                })
                .collect();
            let Some(answer) = answer else { continue };
            let values: Vec<Option<f64>> = vote
                .dims
                .iter()
                .map(|d| nils_pack::expr::Ctx::num(&row, d.field))
                .collect();
            let key = key_of(vote, &values);
            if let Some(p) = pass.reference.partition_by {
                pools
                    .entry(h.axes[p].clone())
                    .or_default()
                    .add(key, answer.clone());
            }
            global.add(key, answer);
        }
        eprintln!(
            "{}: reference {} stacks, {} pools",
            pass.name,
            global.len(),
            pools.len()
        );

        for h in held {
            let row = Row {
                fields: &h.fields,
                axes: &h.axes,
            };
            if !vote_all
                && let Some(t) = &pass.target
                && !t.eval(None, &row)
            {
                continue;
            }
            let values: Vec<Option<f64>> = vote
                .dims
                .iter()
                .map(|d| nils_pack::expr::Ctx::num(&row, d.field))
                .collect();
            let key = key_of(vote, &values);
            let sequence = nils_pack::expr::Ctx::text(&row, vote.compat.subject_field).to_string();
            let partition = pass.reference.partition_by.map(|p| h.axes[p].clone());
            let ask = |pool: &Pool, name: &str| {
                take(vote, pool, &key, &sequence, &pack.axes, &pack.regexes, name)
            };
            let mut outcome = match &partition {
                Some(name)
                    if pools.contains_key(name)
                        && !pass.reference.fallback_except.contains(name) =>
                {
                    ask(&pools[name], "scoped")
                }
                _ => ask(&global, "global"),
            };
            if outcome.answer.is_none()
                && pass.reference.fallback
                && outcome.partition != "global"
                && pass.reference.fallback_when.contains(&outcome.method)
            {
                outcome = ask(&global, "global");
            }
            let answer = match &outcome.answer {
                Some(a) => vote
                    .vote_on
                    .iter()
                    .enumerate()
                    .map(|(i, axis)| pack.axes[*axis].stored(a[i]).to_string())
                    .collect::<Vec<_>>()
                    .join("|"),
                None => "|".to_string(),
            };
            writeln!(
                w,
                "{}\tvote\t{answer}\t{}\t{} of {} in {}",
                h.id, outcome.method, outcome.matches, outcome.neighbours, outcome.partition
            )
            .ok();
        }
    }
}
