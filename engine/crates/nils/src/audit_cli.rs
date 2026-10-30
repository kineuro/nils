// SPDX-License-Identifier: AGPL-3.0-only

//! The sealed protocol as a command (record 58 P6, Wave 7b spec §16): the
//! first half of the label-quality passport a site returns about its own
//! labels, on its own install.
//!
//! - `nils audit draw --seed S --count N --packets DIR`: the seeded draw of
//!   accepted stacks the draw door makes ([`crate::review_rule::make_draw`],
//!   sha256 of the seed and each stack's DICOM identity), and the verify
//!   campaign over every drawn stack with the accepted answer as each
//!   proposal, planted from the seed, as the settle makes it
//!   ([`crate::review_rule::verify_draw`]); with `--raters`, the raters'
//!   campaign of the draw door instead, settled later through its door.
//!   Each drawn stack's header, as the campaign's header door gives it for
//!   a read, is written as text, one file per stack named by its place in
//!   the draw (never by a subject code or a path), beside a `MANIFEST.json`
//!   with the seed and the frame's digest.
//! - `nils audit export --out FILE`: the draw's counts (frame, drawn, read,
//!   settled), the wrong count with its exact one-sided bound, and the
//!   reader-disagreement count, by axis and by vendor and acquisition year,
//!   every cell under the minimum of record 14 (D27, k = 5) held back with
//!   its complement. Nothing else: no subject code, no path, no stack, no
//!   header text.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nils_registry::Registry;
use nils_registry::audit_draw::{self, Draw, Framed};
use nils_registry::store::Store;
use serde_json::{Map, Value, json};

use crate::campaigns::rerr;
use crate::{Exit, Home, fail, usage};

/// The smallest cell an export shows: record 14's D27 default for imaging
/// metadata, the k the ask and the pipelines' totals already hold to.
pub(crate) const MIN_CELL: u64 = nils_ask::validate::MEASURE_K as u64;

#[derive(Debug, clap::Args)]
pub(crate) struct DrawArgs {
    /// The registered seed the draw is made from (kept with it)
    #[arg(long, value_name = "SEED")]
    seed: String,
    /// How many accepted stacks to draw (the whole frame where it holds
    /// fewer)
    #[arg(long, value_name = "N")]
    count: u64,
    /// Where the header packets and MANIFEST.json are written: a new or
    /// empty directory
    #[arg(long, value_name = "DIR")]
    packets: PathBuf,
    /// The stacks accepted after this time (default: after the round's last
    /// draw by count)
    #[arg(long, value_name = "STAMP")]
    since: Option<String>,
    /// The axes audited, joined by commas (default: the header
    /// certificate's)
    #[arg(long, value_name = "AXES", value_delimiter = ',')]
    axes: Option<Vec<String>>,
    /// The review round (default: the round of the pack and its version)
    #[arg(long, value_name = "ID")]
    round: Option<i64>,
    /// The people who read the verify campaign, as principals joined by
    /// commas (the desk's account names them); a campaign is open only to
    /// those it names
    #[arg(
        long,
        value_name = "WHO",
        value_delimiter = ',',
        required_unless_present = "raters"
    )]
    readers: Option<Vec<String>>,
    /// How many people read each stack (two or more give a disagreement
    /// count)
    #[arg(long, value_name = "N", default_value_t = 1)]
    readers_per_item: i64,
    /// Instead of the verify campaign, the raters' campaign of the monthly
    /// audit, for these principals; its settle is made through its door
    /// once both have answered
    #[arg(long, value_name = "WHO", value_delimiter = ',')]
    raters: Option<Vec<String>>,
    /// The pack whose round is drawn and whose axes are asked
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

/// `nils audit draw`.
pub(crate) fn draw(home: &Home, a: DrawArgs) -> Result<(), Exit> {
    if a.seed.trim().is_empty() {
        return Err(usage("--seed: the registered word the draw is made from"));
    }
    if a.count == 0 {
        return Err(usage("--count: at least one stack"));
    }
    // the packets go to a place of their own, checked before anything is drawn
    if a.packets.exists() {
        let busy = std::fs::read_dir(&a.packets)
            .map_err(|e| fail(format!("{}: {e}", a.packets.display())))?
            .next()
            .is_some();
        if busy {
            return Err(usage(format!(
                "{} is not empty: the packets of a draw go to a new or empty directory, so that no packet of another draw is read with them",
                a.packets.display()
            )));
        }
    }
    std::fs::create_dir_all(&a.packets)
        .map_err(|e| fail(format!("{}: {e}", a.packets.display())))?;
    let pack_dir = crate::pack_dir(home, a.pack_dir.clone())?;
    let mut registry = crate::open(home)?;
    let who = crate::ask_cli::principal();
    let mut doc = json!({"seed": a.seed, "count": a.count});
    if let Some(t) = &a.since {
        doc["since"] = json!(t);
    }
    if let Some(r) = a.round {
        doc["round"] = json!(r);
    }
    if let Some(axes) = &a.axes {
        doc["axes"] = json!(axes);
    }
    let made =
        crate::review_rule::make_draw(&mut registry, Some(pack_dir.as_path()), &a.pack, &who, &doc)
            .map_err(rerr)?;
    let (campaign, purpose, planted, raters): (i64, &str, Value, bool) = match &a.raters {
        Some(raters) => {
            doc["raters"] = json!(raters);
            let answer = crate::review_rule::raters_campaign(&mut registry, &who, &made, &doc)
                .map_err(rerr)?;
            (
                answer["campaign"]["id"].as_i64().unwrap_or_default(),
                "audit",
                Value::Null,
                true,
            )
        }
        None => {
            let v = crate::review_rule::verify_draw(
                &mut registry,
                &who,
                &made,
                a.readers.clone().unwrap_or_default(),
                a.readers_per_item,
                &Value::Null,
            )
            .map_err(rerr)?;
            let planted = nils_registry::verify::summary(registry.store(), v.campaign.id)
                .map_err(|e| fail(e.to_string()))?
                .unwrap_or(Value::Null);
            (v.campaign.id, "verify", planted, false)
        }
    };
    let d = audit_draw::get(registry.store(), made.draw.id)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail("the draw was not kept"))?;
    let pack = crate::reader::served_pack(Some(pack_dir.as_path()), &a.pack);
    let written = write_packets(
        registry.store(),
        &a.packets,
        &d,
        &made.chosen,
        campaign,
        purpose,
        pack.as_deref(),
    )?;
    let out = json!({
        "draw": {"id": d.id, "round_id": d.round_id, "seed": d.seed, "count": d.count,
                 "since": d.since, "frame": {"stacks": d.frame_count, "digest": d.frame_digest},
                 "drawn": d.drawn},
        "campaign": {"id": campaign, "purpose": purpose, "planted": planted},
        "axes": made.axes,
        "packets": {"dir": a.packets.display().to_string(), "files": written},
        "next": if raters {
            "the raters read the raters' campaign blind; once both have answered, POST /api/audit/draws/{id}/settle makes the verify campaign a person settles; then nils audit export"
        } else {
            "a person reads the verify campaign (the desk's verify screen) and closes it; then nils audit export --draw <id> --out <file>"
        },
    });
    if a.json {
        crate::campaigns::print(&out);
        return Ok(());
    }
    println!(
        "draw {}: {} of {} accepted stacks, seed {}, frame {}",
        d.id, d.drawn, d.frame_count, d.seed, d.frame_digest
    );
    if raters {
        println!("raters' campaign {campaign}: {} items, read blind", d.drawn);
    } else {
        println!(
            "verify campaign {campaign}: {} items, the accepted answer proposed, {} planted",
            d.drawn,
            planted["count"].as_i64().unwrap_or_default()
        );
    }
    println!("packets: {written} in {}", a.packets.display());
    println!("next: {}", out["next"].as_str().unwrap_or_default());
    Ok(())
}

/// One packet's text: a drawn stack's header as the campaign's header door
/// gives it for a read (the whole stored header less what names a person,
/// and the private elements the pack shows), headed by its place in the
/// draw. No subject code, no path, no stack id, nothing NILS said of it.
fn packet_text(
    store: &mut Store,
    stack: i64,
    head: &str,
    pack: Option<&nils_pack::Pack>,
) -> Result<String, Exit> {
    let e = |e: nils_registry::store::Error| fail(e.to_string());
    let doc = crate::file_header::whole(store, stack, true)
        .map_err(e)?
        .ok_or_else(|| {
            fail(format!(
                "a drawn stack is no longer in the registry ({head})"
            ))
        })?;
    let (private, withheld) = crate::file_header::private_shown(store, stack, pack).map_err(e)?;
    let text_of = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let mut out = String::new();
    out.push_str(head);
    out.push('\n');
    let mut level = String::new();
    for f in doc["fields"].as_array().into_iter().flatten() {
        let l = f["level"].as_str().unwrap_or_default();
        if l != level {
            level = l.to_string();
            out.push_str(&format!("\n[{level}]\n"));
        }
        let name = f["keyword"]
            .as_str()
            .filter(|k| !k.is_empty())
            .or(f["column"].as_str())
            .unwrap_or_default();
        let tag = f["tag"]
            .as_str()
            .map(|t| format!(" {t}"))
            .unwrap_or_default();
        out.push_str(&format!("{name}{tag}: {}\n", text_of(&f["value"])));
    }
    if !private.is_empty() {
        out.push_str("\n[private]\n");
        for (k, v) in &private {
            out.push_str(&format!("{k}: {}\n", text_of(v)));
        }
    }
    let n = |v: &Value| v.as_u64().unwrap_or_default();
    out.push_str(&format!(
        "\nleft out: {} fields that name a person, {} private values shaped like an identifier\n",
        n(&doc["left_out"]["identifying"]),
        withheld
    ));
    Ok(out)
}

/// The packets of a draw and their manifest: one text file per drawn stack
/// named by its place in the draw, in the order drawn.
fn write_packets(
    store: &mut Store,
    dir: &Path,
    d: &Draw,
    chosen: &[Framed],
    campaign: i64,
    purpose: &str,
    pack: Option<&nils_pack::Pack>,
) -> Result<usize, Exit> {
    let item_of: BTreeMap<i64, i64> = nils_registry::campaign::items(store, campaign)
        .map_err(crate::campaigns::cerr)?
        .into_iter()
        .filter_map(|i| i.stack_id.map(|s| (s, i.id)))
        .collect();
    let width = chosen.len().to_string().len().max(4);
    let mut listed = Vec::new();
    for (i, f) in chosen.iter().enumerate() {
        let position = i + 1;
        let file = format!("{position:0width$}.txt");
        let item = item_of.get(&f.stack_id).copied();
        let head = format!(
            "NILS audit packet {position} of {}: draw {}, seed {}, frame {}\n{purpose} campaign {campaign}, item {}\nread blind: nothing NILS said of this stack is in this packet",
            chosen.len(),
            d.id,
            d.seed,
            d.frame_digest,
            item.map_or_else(|| "none".to_string(), |i| i.to_string()),
        );
        let text = packet_text(store, f.stack_id, &head, pack)?;
        let path = dir.join(&file);
        std::fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        listed.push(json!({"position": position, "file": file, "item": item}));
    }
    let manifest = json!({
        "kind": "nils audit packets",
        "draw": d.id, "round_id": d.round_id, "seed": d.seed,
        "count": d.count, "since": d.since, "created_at": d.created_at,
        "frame": {"stacks": d.frame_count, "digest": d.frame_digest},
        "drawn": d.drawn,
        "campaign": {"id": campaign, "purpose": purpose},
        "order": "sha256 of the seed and each stack's DICOM identity",
        "packets": listed,
        "trains": false,
    });
    let path = dir.join("MANIFEST.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n",
    )
    .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    Ok(chosen.len())
}

// ------------------------------------------------------------ the export

/// `nils audit export`: the draw named, or the newest draw by count.
pub(crate) fn export(home: &Home, draw: Option<i64>, out: &Path) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let doc = export_doc(&mut registry, draw)?;
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
    std::fs::write(out, text).map_err(|e| fail(format!("{}: {e}", out.display())))?;
    let c = &doc["counts"];
    let shown = |v: &Value| {
        if v.is_null() {
            "held back".to_string()
        } else {
            v.to_string()
        }
    };
    println!(
        "draw {}: {} drawn, {} read, {} wrong, bound {} ({} cells held back); written to {}",
        doc["draw"]["id"],
        shown(&c["drawn"]),
        shown(&c["read"]),
        shown(&c["wrong"]),
        shown(&c["bound"]),
        doc["min_cell"]["held_back"].as_array().map_or(0, Vec::len),
        out.display()
    );
    Ok(())
}

fn export_doc(registry: &mut Registry, draw: Option<i64>) -> Result<Value, Exit> {
    let e = |e: nils_registry::store::Error| fail(e.to_string());
    let d = match draw {
        Some(id) => audit_draw::get(registry.store(), id)
            .map_err(e)?
            .ok_or_else(|| usage(format!("no audit draw {id}; nils audit draw makes one")))?,
        None => audit_draw::list(registry.store(), None)
            .map_err(e)?
            .into_iter()
            .filter(|d| d.count.is_some())
            .max_by_key(|d| d.id)
            .ok_or_else(|| usage("no audit draw by count yet; nils audit draw makes one"))?,
    };
    if !d.settled() {
        return Err(usage(format!(
            "audit draw {} is not settled yet: once both raters have answered, POST /api/audit/draws/{}/settle, read the verify campaign, then export",
            d.id, d.id
        )));
    }
    let (reads, settle) = audit_draw::settled_reads(registry.store(), &d).map_err(fail)?;
    let closed = settle["closed"] == true;
    let g =
        audit_draw::grade_of(registry.store(), &d, &reads, closed, settle.clone()).map_err(e)?;
    let drawn = audit_draw::stacks(registry.store(), d.id).map_err(e)?;
    let axes: Vec<String> = drawn
        .first()
        .and_then(|f| f.accepted.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let mut campaigns: Vec<i64> = Vec::new();
    campaigns.extend(d.campaign_id);
    campaigns.extend(d.settle_campaign_id);
    let disagree = disagreements(registry.store(), &campaigns, &axes)?;
    Ok(assemble(&d, &g, &settle, &drawn, &axes, &disagree))
}

/// Where readers disagreed: the stacks read by two or more readers of one
/// campaign, and those where two of them gave different answers, in all
/// and by axis.
#[derive(Debug, Default, Clone, PartialEq)]
struct Disagreed {
    /// Stacks read by two or more readers of one campaign.
    two_or_more: BTreeSet<i64>,
    /// Stacks where two of a campaign's readers answered differently.
    stacks: BTreeSet<i64>,
    by_axis: BTreeMap<String, BTreeSet<i64>>,
}

/// The readers' answers of the draw's campaigns (each reader's latest, a
/// rater's, never an adjudicator's), compared within each campaign on the
/// audited axes.
fn disagreements(store: &mut Store, campaigns: &[i64], axes: &[String]) -> Result<Disagreed, Exit> {
    let mut out = Disagreed::default();
    for &c in campaigns {
        let campaign = nils_registry::campaign::get(store, c)
            .map_err(crate::campaigns::cerr)?
            .ok_or_else(|| fail(format!("no campaign {c}")))?;
        let single = campaign
            .question()
            .ok()
            .and_then(|q| nils_registry::campaign::single_axis(&q));
        let stack_of: BTreeMap<i64, i64> = nils_registry::campaign::items(store, c)
            .map_err(crate::campaigns::cerr)?
            .into_iter()
            .filter_map(|i| i.stack_id.map(|s| (i.id, s)))
            .collect();
        let mut latest: BTreeMap<(i64, String), audit_draw::Answered> = BTreeMap::new();
        for a in
            nils_registry::campaign::current_answers(store, c).map_err(crate::campaigns::cerr)?
        {
            if a.role != "rater" {
                continue;
            }
            let (Some(stack), Some(text)) = (stack_of.get(&a.item_id), a.value.as_ref()) else {
                continue;
            };
            let read: audit_draw::Answered = match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(m)) => m.into_iter().collect(),
                _ => match &single {
                    Some(axis) => [(axis.clone(), json!(text))].into(),
                    None => continue,
                },
            };
            latest.insert((*stack, a.principal.clone()), read);
        }
        let mut by_stack: BTreeMap<i64, Vec<audit_draw::Answered>> = BTreeMap::new();
        for ((stack, _), read) in latest {
            by_stack.entry(stack).or_default().push(read);
        }
        for (stack, reads) in by_stack {
            if reads.len() < 2 {
                continue;
            }
            out.two_or_more.insert(stack);
            for axis in axes {
                let seen: BTreeSet<Option<String>> = reads
                    .iter()
                    .map(|r| audit_draw::comparable(r.get(axis).unwrap_or(&Value::Null)))
                    .collect();
                if seen.len() > 1 {
                    out.stacks.insert(stack);
                    out.by_axis.entry(axis.clone()).or_default().insert(stack);
                }
            }
        }
    }
    Ok(out)
}

/// The export from a draw's grade: counts only, every cell of fewer than
/// [`MIN_CELL`] drawn stacks held back, with the smallest other cell of its
/// list where one alone would be (so that no total gives it away).
fn assemble(
    d: &Draw,
    g: &Value,
    settle: &Value,
    drawn: &[Framed],
    axes: &[String],
    dis: &Disagreed,
) -> Value {
    let u = |v: &Value| v.as_u64().unwrap_or(0);
    let mut held: Vec<String> = Vec::new();
    // the whole draw is a cell too, and so is its frame
    let whole_held = (d.drawn as u64) < MIN_CELL;
    let frame_held = (d.frame_count as u64) < MIN_CELL;
    if frame_held {
        held.push("frame".into());
    }
    if whole_held {
        held.push("draw".into());
    }
    let keep = |v: Value| if whole_held { Value::Null } else { v };
    // the drawn stacks' cells, with their disagreements
    let mut vendor_of: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut year_of: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for f in drawn {
        vendor_of
            .entry(f.vendor.clone().unwrap_or_else(|| "unknown".into()))
            .or_default()
            .push(f.stack_id);
        year_of
            .entry(
                f.year
                    .map_or_else(|| "unknown".to_string(), |y| y.to_string()),
            )
            .or_default()
            .push(f.stack_id);
    }
    let cells = |name: &str,
                 of: &Value,
                 members: &BTreeMap<String, Vec<i64>>,
                 held: &mut Vec<String>| {
        let sizes: BTreeMap<String, u64> = of
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, c)| (k.clone(), u(&c["drawn"])))
            .collect();
        let hidden = held_cells(&sizes, whole_held);
        let mut out = Map::new();
        for (k, c) in of.as_object().into_iter().flatten() {
            if hidden.contains(k) {
                held.push(format!("{name}:{k}"));
                out.insert(
                    k.clone(),
                    json!({"drawn": null, "read": null, "wrong": null, "disagreed": null, "held_back": true}),
                );
                continue;
            }
            let disagreed = members
                .get(k)
                .map_or(0, |m| m.iter().filter(|s| dis.stacks.contains(*s)).count());
            out.insert(
                k.clone(),
                json!({"drawn": u(&c["drawn"]), "read": u(&c["read"]), "wrong": u(&c["wrong"]),
                       "disagreed": disagreed}),
            );
        }
        Value::Object(out)
    };
    let by_vendor = cells("vendor", &g["by_vendor"], &vendor_of, &mut held);
    let by_year = cells("year", &g["by_year"], &year_of, &mut held);
    let mut by_axis = Map::new();
    for axis in axes {
        by_axis.insert(
            axis.clone(),
            keep(json!({
                "wrong": u(&g["wrong_by_axis"][axis.as_str()]),
                "disagreed": dis.by_axis.get(axis).map_or(0, BTreeSet::len),
            })),
        );
    }
    json!({
        "kind": "nils audit export",
        "protocol": "record 58 P6: a seeded draw of accepted stacks, read in verify mode against the accepted answer",
        "draw": {"id": d.id, "seed": d.seed, "count": d.count, "since": d.since,
                 "created_at": d.created_at, "frame_digest": d.frame_digest},
        "counts": {
            "frame": if frame_held { Value::Null } else { json!(d.frame_count) },
            "drawn": keep(json!(d.drawn)),
            "read": keep(g["read"].clone()),
            "settled": keep(settle["settled"].clone()),
            "stood": keep(settle["stood"].clone()),
            "not_settled": keep(g["not_settled"].clone()),
            "cant_tell": keep(g["cant_tell"].clone()),
            "wrong": keep(g["wrong"].clone()),
            "bound": keep(g["bound"].clone()),
            "confidence": g["confidence"], "method": g["method"],
            "campaign_closed": g["campaign_closed"],
            "rule": keep(g["rule"].clone()),
        },
        "disagreement": keep(json!({
            "read_by_two_or_more": dis.two_or_more.len(),
            "disagreed": dis.stacks.len(),
        })),
        "by_axis": by_axis,
        "by_vendor": by_vendor,
        "by_year": by_year,
        "min_cell": {"k": MIN_CELL, "of": "drawn stacks", "rule": "record 14, D27: a cell under k is held back, and the smallest other cell of its list with it where it is held alone",
                     "held_back": held},
        "trains": false,
    })
}

/// The cells of one list held back: every cell of fewer than [`MIN_CELL`]
/// stacks, every cell where the whole draw is held, and, where exactly one
/// cell is held while others are shown, the smallest shown one too, since
/// the draw's total less the shown cells would give the held one away.
fn held_cells(sizes: &BTreeMap<String, u64>, all: bool) -> BTreeSet<String> {
    if all {
        return sizes.keys().cloned().collect();
    }
    let mut held: BTreeSet<String> = sizes
        .iter()
        .filter(|(_, n)| **n < MIN_CELL)
        .map(|(k, _)| k.clone())
        .collect();
    if held.len() == 1 && sizes.len() > 1 {
        let next = sizes
            .iter()
            .filter(|(k, _)| !held.contains(*k))
            .min_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
            .map(|(k, _)| k.clone());
        held.extend(next);
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(list: &[(&str, u64)]) -> BTreeMap<String, u64> {
        list.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    }

    #[test]
    fn a_cell_under_five_is_held_back_with_its_complement() {
        let h = held_cells(
            &sizes(&[("ge", 30), ("philips", 3), ("siemens", 60)]),
            false,
        );
        assert_eq!(h, ["ge", "philips"].iter().map(|s| s.to_string()).collect());
        // two small cells hide each other
        let h = held_cells(&sizes(&[("a", 2), ("b", 4), ("c", 90)]), false);
        assert_eq!(h, ["a", "b"].iter().map(|s| s.to_string()).collect());
        // nothing small, nothing held; five is shown
        assert!(held_cells(&sizes(&[("a", 5), ("b", 7)]), false).is_empty());
        // a draw held whole holds every cell
        assert_eq!(held_cells(&sizes(&[("a", 2), ("b", 1)]), true).len(), 2);
        // a single cell under five has no complement to hold
        assert_eq!(held_cells(&sizes(&[("a", 3)]), false).len(), 1);
    }
}
