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

    let out = std::io::stdout();
    let mut w = BufWriter::with_capacity(1 << 20, out.lock());
    let mut n = 0u64;
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
                found.map(|a| format!("{:.2}", a.confidence)).unwrap_or_default(),
            )
            .ok();
        }
        n += 1;
    }
    w.flush().ok();
    eprintln!("{n} stacks");
}
