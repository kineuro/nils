// SPDX-License-Identifier: AGPL-3.0-only

//! The A/B campaign at the keyboard and the door (record 48, the reference
//! read by judges). The mechanism is the registry's (`nils_registry::ab`);
//! here the voters' readings are brought in, the rules are read as a voter
//! of their own, and the doors serve an item's sheet, take a cause, and
//! tell how the campaign goes.
//!
//! - **Voters from files.** `--voter NAME=DIR` reads one JSON file per stack
//!   (`<stack>.json`), either the answer itself or a record with the answer
//!   under `answer` and the stack under `stack`, as the header judge writes
//!   it: per axis a `value` (a value, a list, or `cant_tell`) and a
//!   `reason`. A value is read by any name the pack gives it.
//! - **The rules as a voter.** `--rules` reads the values in force and, as
//!   the reason, the header facts the deciding clause read and the words it
//!   matched. On a stack of a sample sealed now that is what a system said
//!   of it, so it is read only with `--unsealed-access REASON`, which the
//!   audit keeps; the sheet then shows it as one candidate among others,
//!   never as the rules'.
//! - **Blind while open.** A door serves the candidates' letters, values and
//!   reasons; which voter gave which, and which items are the audit's, are
//!   told only once the campaign is closed, and on a stack sealed now only
//!   to a holder of `sealed:see`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Args;
use nils_ask::ast::Grain;
use nils_registry::ab::{self, Localizer, Localizers, Options, Said, Voter};
use nils_registry::campaign::{self, Items, New};
use nils_registry::home::Home;
use nils_registry::{Registry, Store};
use serde_json::{Value, json};

use crate::campaigns::{
    campaign_err, cerr, complete_axes, content_hash, handle_keys, pictures_of, print, rerr, shown,
    who,
};
use crate::serve::{Caller, Reply};
use crate::{Exit, usage};

/// The axes a person answers of a stack in the reference read (record 48,
/// how the reference is read).
pub(crate) const ASKED: [&str; 7] = [
    "provenance",
    "technique",
    "modifier",
    "construct",
    "base",
    "body_part",
    "post_contrast",
];

#[derive(Debug, Args)]
pub(crate) struct AbArgs {
    /// The campaign's name
    name: String,
    /// The stacks the voters read: a saved selection, frozen now (the
    /// sealed set)
    #[arg(long, value_name = "selection:NAME@V")]
    select: String,
    /// A voter's readings, one JSON file per stack in DIR; repeat for more
    #[arg(long = "voter", value_name = "NAME=DIR")]
    voters: Vec<String>,
    /// The rules as a voter of their own: the values in force, with the
    /// header facts the deciding clause read as the reason
    #[arg(long)]
    rules: bool,
    /// The axes asked, joined by commas
    #[arg(long, value_name = "AXES", value_delimiter = ',')]
    axes: Option<Vec<String>>,
    /// The axes derived from each answer; the pack's own when not given
    #[arg(long, value_name = "AXES", value_delimiter = ',')]
    derive: Option<Vec<String>>,
    /// The seed of the audit's draw, written down before the draw
    #[arg(long, value_name = "TEXT")]
    seed: String,
    /// The share of the stacks the voters agree on that is audited
    #[arg(long, default_value_t = 0.1, value_name = "SHARE")]
    audit_share: f64,
    /// The least agreeing cells audited per axis (150 bounds a joint error
    /// at 2 % by the rule of three), where the agreeing stacks have them
    #[arg(long, default_value_t = 150, value_name = "N")]
    audit_cells: usize,
    /// Localizers read with the rest, without them, or only them
    #[arg(long, default_value = "with", value_name = "with|without|only")]
    localizers: String,
    /// What a localizer is: an axis and its value
    #[arg(
        long,
        default_value = "provenance=Localizer",
        value_name = "AXIS=VALUE"
    )]
    localizer: String,
    /// The axes a localizer is asked; every other is answered not_asked
    #[arg(
        long,
        value_name = "AXES",
        value_delimiter = ',',
        default_value = "provenance,body_part"
    )]
    localizer_asks: Vec<String>,
    /// Who settles the items; anyone with campaigns:work when none is named
    #[arg(long = "rater", value_name = "PRINCIPAL")]
    raters: Vec<String>,
    #[arg(long, default_value = "none", value_name = "decision|stage|none")]
    closes_into: String,
    #[arg(long, default_value_t = 3600, value_name = "SECONDS")]
    lease_seconds: i64,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Count the items and the audit, and make nothing
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

/// Read one voter's value of an axis by any name the pack gives it.
fn named(names: &BTreeMap<String, String>, raw: &Value) -> Value {
    let one = |s: &str| {
        names
            .get(s.trim())
            .cloned()
            .unwrap_or_else(|| s.trim().to_string())
    };
    match raw {
        Value::String(s) if s == campaign::CANT_TELL => raw.clone(),
        // the header judge's schema says none for an axis with no value,
        // which an axes answer says as null, where the pack has no value
        // of that name
        Value::String(s)
            if s.trim().eq_ignore_ascii_case("none") && !names.contains_key(s.trim()) =>
        {
            Value::Null
        }
        Value::String(s) => json!(one(s)),
        Value::Array(list) => json!(
            list.iter()
                .filter_map(Value::as_str)
                .map(one)
                .collect::<Vec<_>>()
        ),
        other => other.clone(),
    }
}

/// What reading a voter found beside its readings.
#[derive(Debug, Default)]
struct Read {
    files: usize,
    not_a_stack: usize,
    no_answer: usize,
    refused: BTreeMap<String, usize>,
}

impl Read {
    fn as_json(&self) -> Value {
        json!({"files": self.files, "not_a_stack": self.not_a_stack,
            "no_answer": self.no_answer, "refused": self.refused})
    }
}

/// One voter's readings from a folder of JSON files, one per stack.
fn voter_of(
    name: &str,
    dir: &Path,
    axes: &[String],
    constraints: &Value,
    pack: Option<&nils_pack::Pack>,
) -> Result<(Voter, Read), Exit> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| usage(format!("--voter {name}: {}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let names: BTreeMap<&str, BTreeMap<String, String>> = axes
        .iter()
        .map(|a| (a.as_str(), crate::campaigns::value_names(pack, Some(a))))
        .collect();
    let mut v = Voter {
        name: name.to_string(),
        ..Voter::default()
    };
    let mut read = Read::default();
    for f in files {
        read.files += 1;
        let text =
            std::fs::read_to_string(&f).map_err(|e| usage(format!("{}: {e}", f.display())))?;
        let doc: Value = serde_json::from_str(&text)
            .map_err(|e| usage(format!("{}: not JSON: {e}", f.display())))?;
        let stack = doc["stack"].as_i64().or_else(|| {
            f.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<i64>().ok())
        });
        let Some(stack) = stack else {
            read.not_a_stack += 1;
            continue;
        };
        let answer = if doc["answer"].is_object() {
            &doc["answer"]
        } else if doc.get("answer").is_some() {
            read.no_answer += 1;
            continue;
        } else {
            &doc
        };
        let mut said = BTreeMap::new();
        for axis in axes {
            let e = &answer[axis.as_str()];
            let (raw, reason) = if e.is_object() {
                (e["value"].clone(), e["reason"].as_str().unwrap_or_default())
            } else if e.is_null() && answer.get(axis.as_str()).is_none() {
                continue;
            } else {
                (e.clone(), "")
            };
            let raw = named(&names[axis.as_str()], &raw);
            match ab::said_value(axes, constraints, axis, &raw) {
                Ok(value) => {
                    said.insert(
                        axis.clone(),
                        Said {
                            value,
                            reason: reason.trim().to_string(),
                        },
                    );
                }
                Err(_) => *read.refused.entry(axis.clone()).or_default() += 1,
            }
        }
        if !said.is_empty() {
            v.stacks.insert(stack, said);
        }
    }
    Ok((v, read))
}

/// A header field as a reason names it.
fn field_name(f: &str) -> &str {
    match f {
        "repetition_time" => "TR",
        "echo_time" => "TE",
        "inversion_time" => "TI",
        "flip_angle" => "FlipAngle",
        "text_sequence_name" => "SequenceName",
        "image_type" => "ImageType",
        "scanning_sequence" => "ScanningSequence",
        "sequence_variant" => "SequenceVariant",
        "scan_options" => "ScanOptions",
        "mr_acquisition_type" | "acquisition_type_filled" => "MRAcquisitionType",
        "echo_train_length" => "EchoTrainLength",
        "diffusion_b_value" | "dwi_b_values" => "b",
        "dwi_directions" => "directions",
        "slice_thickness" => "SliceThickness",
        "n_slices" => "slices",
        "orientation" => "orientation",
        "magnetic_field_strength" | "field_strength_normalized" => "field",
        other => other,
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
            Some(f) => format!("{}", (f * 100.0).round() / 100.0),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The rules' reason for one axis, from the evidence line: the words the
/// deciding clause matched in the file's text, and the header facts it
/// read. Never the rule's name.
fn rules_reason(entry: &Value) -> String {
    let d = &entry["decided"];
    let mut parts = Vec::new();
    if d["source"] == "text"
        && let Some(m) = d["matched"].as_str().filter(|m| !m.trim().is_empty())
    {
        parts.push(format!("the words \"{}\"", m.trim()));
    }
    for (f, v) in d["header"].as_object().into_iter().flatten() {
        if !v.is_null() {
            parts.push(format!("{} {}", field_name(f), short(v)));
        }
    }
    parts.join(", ")
}

/// The rules as a voter: the values in force of every stack classified,
/// each axis with its reason.
fn rules_voter(
    store: &mut Store,
    stacks: &[i64],
    axes: &[String],
    constraints: &Value,
    pack: Option<&nils_pack::Pack>,
) -> Result<(Voter, Read), Exit> {
    let withheld =
        crate::sealed::keyboard_withheld(store, stacks).map_err(|e| crate::fail(e.to_string()))?;
    if !withheld.is_empty() {
        return Err(usage(crate::sealed::refusal(&format!(
            "{} of the stacks the rules would vote on",
            withheld.len()
        ))));
    }
    let mut v = Voter {
        name: "rules".to_string(),
        ..Voter::default()
    };
    let mut read = Read::default();
    for &stack in stacks {
        read.files += 1;
        let Some(doc) = crate::reader::why(store, stack, pack, true, false)
            .map_err(|e| crate::fail(e.to_string()))?
        else {
            read.no_answer += 1;
            continue;
        };
        let mut said = BTreeMap::new();
        for entry in doc["axes"].as_array().into_iter().flatten() {
            let Some(axis) = entry["axis"].as_str() else {
                continue;
            };
            if !axes.iter().any(|a| a == axis) {
                continue;
            }
            match ab::said_value(axes, constraints, axis, &entry["value"]) {
                Ok(value) => {
                    said.insert(
                        axis.to_string(),
                        Said {
                            value,
                            reason: rules_reason(entry),
                        },
                    );
                }
                Err(_) => *read.refused.entry(axis.to_string()).or_default() += 1,
            }
        }
        if !said.is_empty() {
            v.stacks.insert(stack, said);
        }
    }
    Ok((v, read))
}

fn sha256_hex(s: &str) -> String {
    crate::campaigns::sha256(s.as_bytes())
}

/// `nils campaign ab`: read the voters, draw the items and the audit, and
/// make the campaign.
pub(crate) fn make(home: &Home, a: AbArgs) -> Result<(), Exit> {
    if a.voters.is_empty() && !a.rules {
        return Err(usage(
            "an A/B campaign needs voters: --voter NAME=DIR, --rules",
        ));
    }
    if a.seed.trim().is_empty() {
        return Err(usage(
            "--seed: the audit's seed, written down before the draw",
        ));
    }
    if !(0.0..=1.0).contains(&a.audit_share) {
        return Err(usage("--audit-share is from 0 to 1"));
    }
    let localizers = Localizers::parse(&a.localizers).map_err(cerr)?;
    let (laxis, lvalue) = a
        .localizer
        .split_once('=')
        .ok_or_else(|| usage("--localizer AXIS=VALUE"))?;
    let axes: Vec<String> = a
        .axes
        .clone()
        .unwrap_or_else(|| ASKED.iter().map(|s| s.to_string()).collect());
    let mut question = json!({"kind": "axes", "axes": axes});
    if let Some(d) = &a.derive {
        question["derive"] = json!(d);
    }
    let served = crate::pack_dir(home, a.pack_dir.clone())
        .ok()
        .and_then(|d| nils_pack::load(&d.join(&a.pack), None).ok());
    complete_axes(&mut question, served.as_ref()).map_err(|(_, m)| usage(m))?;
    let q = campaign::Question::parse(&question).map_err(cerr)?;
    let constraints = question["constraints"].clone();
    let loc = Localizer {
        axis: laxis.to_string(),
        value: lvalue.to_string(),
        asks: a.localizer_asks.clone(),
    };
    if !axes.contains(&loc.axis) {
        return Err(usage(format!(
            "--localizer names {}, which is not asked",
            loc.axis
        )));
    }
    if let Some(x) = loc.asks.iter().find(|x| !axes.contains(x)) {
        return Err(usage(format!(
            "--localizer-asks names {x}, which is not asked"
        )));
    }
    // the stacks: the selection, frozen now
    let h = crate::ask_cli::freeze_selection(
        home,
        &a.select,
        Grain::Stack,
        a.pack_dir.clone(),
        &a.pack,
    )?;
    let mut registry = crate::open(home)?;
    let stacks: Vec<i64> = handle_keys(registry.store(), h, Grain::Stack)
        .map_err(rerr)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let mut voters = Vec::new();
    let mut reads = serde_json::Map::new();
    for spec in &a.voters {
        let (name, dir) = spec
            .split_once('=')
            .ok_or_else(|| usage(format!("--voter {spec}: NAME=DIR")))?;
        if name == "rules" && a.rules {
            return Err(usage("the voter name rules is --rules's"));
        }
        if voters.iter().any(|v: &Voter| v.name == name) {
            return Err(usage(format!("--voter {name} is named twice")));
        }
        let (v, read) = voter_of(name, Path::new(dir), &axes, &constraints, served.as_ref())?;
        reads.insert(name.to_string(), read.as_json());
        voters.push(v);
    }
    if a.rules {
        let (v, read) = rules_voter(
            registry.store(),
            &stacks,
            &axes,
            &constraints,
            served.as_ref(),
        )?;
        reads.insert("rules".into(), read.as_json());
        voters.push(v);
    }
    let plan = ab::plan(
        &stacks,
        &voters,
        &Options {
            axes: &axes,
            localizer: Some(&loc),
            localizers,
            audit_share: a.audit_share,
            audit_cells: a.audit_cells,
            seed: &a.seed,
        },
    );
    let seed_sha256 = sha256_hex(&a.seed);
    let mut out = json!({
        "name": a.name, "selection": a.select, "counts": plan.counts(),
        "voters": reads, "seed_sha256": seed_sha256, "dry_run": a.dry_run,
    });
    if a.dry_run || plan.items.is_empty() {
        if !a.dry_run {
            out["made"] = json!(false);
            out["why"] = json!("no stack is split or drawn for the audit");
        }
        return report(&out, a.json);
    }
    let names: Vec<&str> = voters.iter().map(|v| v.name.as_str()).collect();
    let source = json!({
        "selection": a.select, "handle": h,
        "ab": {
            "voters": names,
            "seed_sha256": seed_sha256,
            "audit": {"share": a.audit_share, "cells": a.audit_cells},
            "localizers": localizers.name(),
            "localizer": loc.to_json(),
        },
    });
    let hash = content_hash(registry.store(), h).map_err(rerr)?;
    let pack_version = served.as_ref().map(|p| format!("{}@{}", p.name, p.version));
    let principal = who();
    let made = campaign::create(
        &mut registry,
        &New {
            name: &a.name,
            owner: &principal,
            question: &question,
            source,
            items: Items::Stacks(plan.items.iter().map(|p| p.stack).collect()),
            handle_id: Some(h),
            content_hash: hash.as_deref(),
            pack_version: pack_version.as_deref(),
            raters_per_item: 1,
            raters: a.raters.clone(),
            adjudicators: Vec::new(),
            adjudication: &json!({"when": "never", "metric": "exact"}),
            closes_into: &a.closes_into,
            lease_seconds: a.lease_seconds,
            inputs: BTreeMap::new(),
            hold_back: None,
            // the candidates are the only answers shown, and never as a
            // system's: nothing is suggested, filled in, batched or ranked
            suggest: Some(campaign::Suggest::None),
        },
    )
    .map_err(cerr)?;
    let item_of: BTreeMap<i64, i64> = campaign::items(registry.store(), made.id)
        .map_err(cerr)?
        .into_iter()
        .filter_map(|i| i.stack_id.map(|s| (s, i.id)))
        .collect();
    // the letters and the reason shown are drawn with a key nothing keeps
    let key = ab::fresh_key();
    ab::write(registry.store(), made.id, &plan, &item_of, &key).map_err(cerr)?;
    ab::record_made(&mut registry, &principal, &made, &plan)
        .map_err(|e| crate::fail(e.to_string()))?;
    let pictures = pictures_of(registry.store(), &made).map_err(rerr)?;
    out["campaign"] = json!(made.id);
    out["question"] = json!(q.kind());
    out["pictures"] = pictures;
    if a.json {
        let mut v = shown(registry.store(), &made).map_err(rerr)?;
        v["ab"] = out;
        print(&v);
        return Ok(());
    }
    report(&out, false)
}

fn report(out: &Value, json: bool) -> Result<(), Exit> {
    if json {
        print(out);
        return Ok(());
    }
    let c = &out["counts"];
    match out["campaign"].as_i64() {
        Some(id) => println!(
            "campaign {id} {}: {} item(s) to settle",
            out["name"].as_str().unwrap_or_default(),
            c["items"]
        ),
        None if out["dry_run"] == json!(true) => println!(
            "would make {}: {} item(s) to settle",
            out["name"].as_str().unwrap_or_default(),
            c["items"]
        ),
        None => println!("made nothing: {}", out["why"].as_str().unwrap_or_default()),
    }
    println!(
        "  {} stack(s): {} split, {} agree ({} audited), {} localizer(s), {} left out, {} read by no voter",
        c["stacks"],
        c["split"],
        c["agree"],
        c["audit"],
        c["localizers"],
        c["left_out"],
        c["unread"]
    );
    println!(
        "  seed sha256 {}",
        out["seed_sha256"].as_str().unwrap_or_default()
    );
    for (axis, n) in c["split_by_axis"].as_object().into_iter().flatten() {
        println!(
            "  {axis:<14} split on {n:>5}   agreeing cells {:>5}, audited {:>4}",
            c["agree_cells"][axis].as_u64().unwrap_or(0),
            c["audit_cells"][axis].as_u64().unwrap_or(0)
        );
    }
    for (v, r) in out["voters"].as_object().into_iter().flatten() {
        let refused: u64 = r["refused"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, n)| n.as_u64())
            .sum();
        println!(
            "  voter {v:<24} {} read, {} with no answer, {} value(s) refused",
            r["files"], r["no_answer"], refused
        );
    }
    Ok(())
}

/// `nils campaign ab-export`: every decision of an A/B campaign with its
/// candidates, choice and cause, and with the voters where the campaign is
/// closed (and, on a stack sealed now, with `--unsealed-access`).
pub(crate) fn export(
    home: &Home,
    which: &str,
    out: Option<&Path>,
    summary_only: bool,
) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let c = campaign::find(registry.store(), which).map_err(cerr)?;
    if !ab::is_ab(&c) {
        return Err(usage(format!("campaign {} settles no candidates", c.name)));
    }
    let closed = c.status == "closed";
    let items = ab::items_of(registry.store(), c.id, None).map_err(cerr)?;
    let stacks: Vec<i64> = items.values().map(|i| i.stack).collect();
    let withheld = crate::sealed::keyboard_withheld(registry.store(), &stacks)
        .map_err(|e| crate::fail(e.to_string()))?;
    let sources = closed && withheld.is_empty();
    let summary = ab::summary(registry.store(), &c, sources).map_err(cerr)?;
    let mut doc = json!({"campaign": c.id, "name": c.name, "status": c.status,
        "seed_sha256": c.source["ab"]["seed_sha256"], "voters": c.source["ab"]["voters"],
        "summary": summary, "sources": sources});
    if !sources {
        doc["why_no_sources"] = json!(if closed {
            crate::sealed::refusal("A stack of the campaign")
        } else {
            "the voters are told once the campaign is closed".to_string()
        });
    }
    if !summary_only {
        doc["decisions"] = json!(ab::decisions(registry.store(), &c, sources).map_err(cerr)?);
    }
    match out {
        Some(p) => {
            std::fs::write(p, serde_json::to_string_pretty(&doc).unwrap_or_default())
                .map_err(|e| usage(format!("{}: {e}", p.display())))?;
            println!("wrote {}", p.display());
            Ok(())
        }
        None => {
            print(&doc);
            Ok(())
        }
    }
}

// ------------------------------------------------------------ the doors

/// `GET /api/campaigns/{id}/items/{item}/ab`: the sheet one item is settled
/// from, for the caller who holds it (or a holder of review:work).
pub(crate) fn sheet_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    item: i64,
) -> Result<Reply, Reply> {
    if !ab::is_ab(c) {
        return Err(Reply::error(
            409,
            format!("campaign {} settles no candidates", c.name),
        ));
    }
    let holds = campaign::assignments(registry.store(), c.id)
        .map_err(campaign_err)?
        .iter()
        .any(|a| a.item_id == item && a.principal.as_deref() == Some(caller.principal.as_str()));
    if !holds && !caller.access.holds("review:work") {
        return Err(Reply::error(
            403,
            format!("item {item} is settled by who claimed it; claim it first"),
        ));
    }
    let doc = ab::sheet(registry.store(), c, item)
        .map_err(campaign_err)?
        .ok_or_else(|| Reply::error(404, format!("item {item} has no candidates")))?;
    Ok(Reply::ok(doc))
}

/// Whether a door tells the voters: once the campaign is closed, and on a
/// stack sealed now only to a holder of `sealed:see`.
fn told(store: &mut Store, caller: &Caller, c: &campaign::Campaign) -> Result<bool, Reply> {
    if c.status != "closed" {
        return Ok(false);
    }
    let stacks: Vec<i64> = ab::items_of(store, c.id, None)
        .map_err(campaign_err)?
        .values()
        .map(|i| i.stack)
        .collect();
    Ok(crate::sealed::withheld(store, &caller.access, &stacks)?.is_empty())
}

/// `GET /api/campaigns/{id}/ab`: how the campaign goes.
pub(crate) fn summary_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !ab::is_ab(c) {
        return Err(Reply::error(
            409,
            format!("campaign {} settles no candidates", c.name),
        ));
    }
    let sources = told(registry.store(), caller, c)?;
    let mut doc = ab::summary(registry.store(), c, sources).map_err(campaign_err)?;
    doc["causes_offered"] = json!(ab::CAUSES);
    doc["seed_sha256"] = c.source["ab"]["seed_sha256"].clone();
    Ok(Reply::ok(doc))
}

/// `GET /api/campaigns/{id}/ab/decisions`: every decision with its
/// candidates, choice and cause; the voters where [`told`].
pub(crate) fn decisions_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
) -> Result<Reply, Reply> {
    if !ab::is_ab(c) {
        return Err(Reply::error(
            409,
            format!("campaign {} settles no candidates", c.name),
        ));
    }
    let sources = told(registry.store(), caller, c)?;
    // rating is blind until the close (record 45): a rater reads their own
    // decisions alone, as at GET /api/campaigns/{id}/answers
    let principal = caller.principal.as_str();
    let all = crate::campaigns::sees_all(registry.store(), caller, principal, c)?;
    let rows: Vec<Value> = ab::decisions(registry.store(), c, sources)
        .map_err(campaign_err)?
        .into_iter()
        .filter(|r| all || r["principal"].as_str() == Some(principal))
        .collect();
    Ok(Reply::ok(json!({
        "campaign": c.id, "name": c.name, "status": c.status, "sources": sources,
        "count": rows.len(), "decisions": rows, "blind": !all,
    })))
}

/// `POST /api/campaigns/{id}/answers/{answer}/cause`: `{axis, cause}`, the
/// cause null to take it away.
pub(crate) fn cause_door(
    registry: &mut Registry,
    caller: &Caller,
    c: &campaign::Campaign,
    answer: i64,
    doc: &Value,
    now: &str,
) -> Result<Reply, Reply> {
    let axis = doc["axis"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "axis: the axis the cause is of"))?;
    let cause = match &doc["cause"] {
        Value::Null => None,
        Value::String(s) => Some(s.as_str()),
        _ => {
            return Err(Reply::error(
                400,
                format!("cause: {} or null", ab::CAUSES.join(", ")),
            ));
        }
    };
    let done = ab::set_cause(
        registry,
        c.id,
        &ab::Cause {
            answer,
            axis,
            cause,
            principal: caller.principal.as_str(),
        },
        now,
    )
    .map_err(campaign_err)?;
    Ok(Reply::ok(done))
}
