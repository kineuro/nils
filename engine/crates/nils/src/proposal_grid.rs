// SPDX-License-Identifier: AGPL-3.0-only

//! The batch accept of a model's proposals (UX round 5, 2026-11-08): what a
//! model left unsure is not read one scan at a time. A person narrows the
//! model's proposals on one axis by the value proposed and by the model's
//! probability, sees the scans densely, about five hundred at a time, sets
//! only the wrong ones apart, and accepts the rest in one move. Every
//! accept is the person's own decision, with the model's probability kept
//! beside it. The rules' question grid ([`crate::review_questions`]) is the
//! seed: the same pictures, the same page shape, the same keys.
//!
//! - **The scans** (`GET /api/review/proposals/scans`, `nils review
//!   proposals-scans`): one page of the scans the model's open
//!   `<axis>:model` items hold (below its threshold; the staged ones are
//!   the commit's), not decided yet, that no open campaign asks, narrowed
//!   by `value` and by the band `min` to `max`, the most sure first; each with its picture, the value proposed, its probability,
//!   the value in force now and whether a person decided it; beside them
//!   the counts a filter row and a histogram draw: each value's scans in
//!   the band, and ten bands over the model's range for the value; and
//!   `legal`, the values the axis takes in the served pack, for a
//!   correction to choose from (UX round 6). A stack of a sample sealed
//!   now is withheld.
//! - **The accept** (`POST /api/review/proposals/accept`, `nils review
//!   proposals-accept`): one decision per scan seen and not set apart, in
//!   the caller's name, in force, with the model's value, its probability
//!   and its run in the decision's words, written through the review spine
//!   in one transaction; the items whose scans are all decided close as
//!   accepted; the re-sort of those scans is queued as the job a commit
//!   queues ([`crate::resort_job::queue_resort_committed`], `nils review
//!   resort --decisions`), so the door answers at once. The scans set apart
//!   are not written: they stay proposals. A scan a person decided meanwhile
//!   is never overwritten here. UX round 6: a scan seen may be *corrected*
//!   instead, given the person's value in place of the model's; its
//!   decision is written in the same transaction, in the same shape, with
//!   the model's value, probability and run kept in its words
//!   (`corrected bp-head@1's spine at 0.600 (run 41) to brain`), so what
//!   the model proposed and what the person said stand side by side.
//! - **The band** (`POST /api/review/proposals/accept-band`, `nils review
//!   proposals-accept-band`, UX round 6): every scan the band holds for one
//!   value, accepted without a look at each picture, as a job the page
//!   follows. The door checks what the page accept checks, holds `count`
//!   (the number the page said the band holds) against the band now, and
//!   queues the verb with `--who <principal>` as a commit's re-sort is
//!   queued; the job writes the decisions in the page accept's shape, in
//!   chunks of [`BAND_CHUNK`], each chunk its own transaction, with `done`
//!   of `of` on its row, stops at a cancel between chunks, and queues the
//!   re-sort of what it wrote at its end. A scan a person decided meanwhile
//!   keeps that decision, a sealed stack takes none, and a chunk that fails
//!   ends the job failed with why: what was written stands.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::labels;
use nils_registry::model::{self, Model};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 3] = [
    "GET /api/review/proposals/scans",
    "POST /api/review/proposals/accept",
    "POST /api/review/proposals/accept-band",
];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/review/proposals/scans",
        false,
        false,
        "bounded",
        "one page of one model's proposals on one axis",
        "Reading a model's proposals",
        "Read a model's proposals",
    ),
    (
        "POST /api/review/proposals/accept",
        true,
        false,
        "bounded",
        "one decision per scan seen and not set apart",
        "Accepting a model's proposals",
        "Accepted a model's proposals",
    ),
    (
        "POST /api/review/proposals/accept-band",
        true,
        false,
        "bounded",
        "one decision per scan the band holds, written by a job",
        "Accepting a band of a model's proposals",
        "Queued the accept of a band of a model's proposals",
    ),
];

/// A page's size when none is asked for, and the most a page holds.
const SIZE: usize = 500;
const SIZE_MOST: usize = 1000;

/// How many bands the histogram has over the model's range.
const BANDS: usize = 10;

/// How many decisions a band accept writes in one transaction: its
/// progress moves at least this often, and a cancel is heard between.
const BAND_CHUNK: usize = 500;

/// What each door needs, read by `serve::door` before its own table.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "proposals", "scans"]) => {
            (Need::One("review:see"), Detail::Plain)
        }
        ("POST", ["api", "review", "proposals", "accept"])
        | ("POST", ["api", "review", "proposals", "accept-band"]) => {
            (Need::One("review:work"), Detail::Plain)
        }
        _ => return None,
    })
}

/// The doors, or nothing when the path is not one of them. The grants were
/// checked by the table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "proposals", "scans"]) => {
            scans(doors, registry, caller, quasi, query)
        }
        ("POST", ["api", "review", "proposals", "accept"]) => accept(doors, registry, caller, body),
        ("POST", ["api", "review", "proposals", "accept-band"]) => {
            accept_band(doors, registry, caller, body)
        }
        _ => return None,
    })
}

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

// ------------------------------------------------------------- gathering

/// One scan a model proposes a value for, not decided yet.
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    stack: i64,
    item: i64,
    value: String,
    p: f64,
    run: Option<i64>,
}

/// What one model proposed on one axis that still waits.
#[derive(Debug, Default)]
struct Gathered {
    /// By stack, the newest item's proposal.
    scans: BTreeMap<i64, Candidate>,
    /// The thresholds its runs staged at, lowest first.
    thresholds: Vec<f64>,
    /// Scans an open campaign asks already, left out.
    asked: usize,
    /// Scans of a sample sealed now, left out.
    sealed: usize,
}

/// The model's open `<axis>:model` items and their members not decided
/// yet, no open campaign asking them, no sealed stack among them. A staged
/// item's members are the model's staged decision, which a commit puts in
/// force: never here.
fn gather(registry: &mut Registry, m: &Model, axis: &str) -> Result<Gathered, Reply> {
    let asked_by = nils_registry::campaign::held_review_items(registry.store())
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let store = registry.store();
    let t = table("review_item");
    let d = store.dialect();
    let sql = format!(
        "SELECT id, {} FROM {} WHERE status = 'open' AND scope = 'group' AND kind = {} ORDER BY id",
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Text),
    );
    let mut out = Gathered::default();
    // (item, value, run)
    let mut items: BTreeMap<i64, (String, Option<i64>)> = BTreeMap::new();
    let mut thresholds: BTreeSet<String> = BTreeSet::new();
    for r in store
        .query(&sql, &[Param::from(format!("{axis}:model"))])
        .map_err(failed)?
    {
        let id = r.int(0).map_err(failed)?;
        let evidence: Value = r
            .opt_text(1)
            .map_err(failed)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if evidence["model"]["id"].as_i64() != Some(m.id) {
            continue;
        }
        let Some(value) = evidence["value"].as_str() else {
            continue;
        };
        if let Some(t) = evidence["threshold"].as_f64() {
            thresholds.insert(format!("{t:.6}"));
        }
        items.insert(id, (value.to_string(), evidence["run_id"].as_i64()));
    }
    let mut asked_stacks: BTreeSet<i64> = BTreeSet::new();
    let ids: Vec<i64> = items.keys().copied().collect();
    let mt = table("review_member");
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT item_id, stack_id, {} FROM {} WHERE item_id IN ({}) AND decided_at IS NULL ORDER BY item_id, stack_id",
            d.text_of(mt.column("evidence").expect("evidence")),
            store.qualified("review_member"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[]).map_err(failed)? {
            let item = r.int(0).map_err(failed)?;
            let stack = r.int(1).map_err(failed)?;
            let (item_value, run) = &items[&item];
            if asked_by.contains_key(&item) {
                asked_stacks.insert(stack);
                continue;
            }
            let ev: Value = r
                .opt_text(2)
                .map_err(failed)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            let value = ev["value"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| item_value.clone());
            let p = ev["probabilities"][&value]
                .as_f64()
                .or_else(|| ev["confidence"].as_f64())
                .unwrap_or(0.0);
            let c = Candidate {
                stack,
                item,
                value,
                p,
                run: *run,
            };
            // two runs that still both wait on a scan: the newer item speaks
            match out.scans.get(&stack) {
                Some(have) if have.item > item => {}
                _ => {
                    out.scans.insert(stack, c);
                }
            }
        }
    }
    // a scan an open campaign asks is answered in the campaign
    for s in &asked_stacks {
        if out.scans.remove(s).is_some() {
            out.asked += 1;
        }
    }
    // record 48: a stack of a sample sealed now is withheld
    let stacks: Vec<i64> = out.scans.keys().copied().collect();
    let (sealed, _) = labels::sealed_now(store, &stacks, &[]).map_err(failed)?;
    for s in &sealed {
        if out.scans.remove(s).is_some() {
            out.sealed += 1;
        }
    }
    out.thresholds = thresholds
        .iter()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    out.thresholds.sort_by(f64::total_cmp);
    Ok(out)
}

/// The band asked for: `min` inclusive, `max` exclusive; none means no edge.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    min: Option<f64>,
    max: Option<f64>,
}

impl Band {
    fn holds(&self, p: f64) -> bool {
        self.min.is_none_or(|m| p >= m) && self.max.is_none_or(|m| p < m)
    }
}

fn probability(text: &str, name: &str) -> Result<f64, Reply> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        .ok_or_else(|| Reply::error(400, format!("{name} is a probability, 0 to 1")))
}

fn band_of(min: Option<&str>, max: Option<&str>) -> Result<Band, Reply> {
    let min = min
        .filter(|s| !s.trim().is_empty())
        .map(|s| probability(s, "min"))
        .transpose()?;
    let max = max
        .filter(|s| !s.trim().is_empty())
        .map(|s| probability(s, "max"))
        .transpose()?;
    if let (Some(a), Some(b)) = (min, max)
        && a >= b
    {
        return Err(Reply::error(400, "min is below max"));
    }
    Ok(Band { min, max })
}

/// The model a reference names: its id, its digest or `name@version`.
fn model_named(store: &mut Store, reference: &str) -> Result<Model, Reply> {
    model::resolve(store, reference)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no registered model answers to {reference}")))
}

/// The counts the filter row and the histogram draw: each value's scans in
/// the band, and ten bands over the model's whole range for the value.
struct Counts {
    values: Vec<(String, usize)>,
    bands: Vec<(f64, f64, usize)>,
    range: Option<(f64, f64)>,
    total: usize,
}

fn counts_of(g: &Gathered, value: Option<&str>, band: Band) -> Counts {
    let mut values: BTreeMap<&str, usize> = BTreeMap::new();
    for c in g.scans.values() {
        if band.holds(c.p) {
            *values.entry(c.value.as_str()).or_insert(0) += 1;
        }
    }
    let mut values: Vec<(String, usize)> = values
        .into_iter()
        .map(|(v, n)| (v.to_string(), n))
        .collect();
    values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let range = g
        .scans
        .values()
        .map(|c| c.p)
        .fold(None, |r: Option<(f64, f64)>, p| {
            Some(r.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))))
        });
    let mut bands = Vec::new();
    if let Some((lo, hi)) = range {
        let width = (hi - lo) / BANDS as f64;
        let n = if width > 0.0 { BANDS } else { 1 };
        let mut counts = vec![0usize; n];
        for c in g.scans.values() {
            if value.is_some_and(|v| v != c.value) {
                continue;
            }
            let i = if width > 0.0 {
                (((c.p - lo) / width).floor() as usize).min(n - 1)
            } else {
                0
            };
            counts[i] += 1;
        }
        for (i, count) in counts.into_iter().enumerate() {
            let from = if width > 0.0 {
                lo + width * i as f64
            } else {
                lo
            };
            let to = if i + 1 == n {
                hi
            } else {
                lo + width * (i + 1) as f64
            };
            bands.push((round(from), round(to), count));
        }
    }
    let total = g
        .scans
        .values()
        .filter(|c| band.holds(c.p) && value.is_none_or(|v| v == c.value))
        .count();
    Counts {
        values,
        bands,
        range,
        total,
    }
}

fn round(p: f64) -> f64 {
    (p * 10_000.0).round() / 10_000.0
}

/// The stacks a person decided on the axis, in force, at the stack's scope.
fn decided_by_person(
    store: &mut Store,
    axis: &str,
    stacks: &[i64],
) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    let d = store.dialect();
    for chunk in stacks.chunks(500) {
        let refs = chunk
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT ref FROM {} WHERE scope = 'stack' AND axis = {} AND withdrawn_at IS NULL \
             AND (staged_at IS NULL OR committed_at IS NOT NULL) AND author_kind = 'person' AND ref IN ({refs})",
            store.qualified("decision"),
            d.param(1, Type::Text),
        );
        for r in store.query(&sql, &[Param::from(axis)])? {
            if let Ok(s) = r.text(0)?.parse::<i64>() {
                out.insert(s);
            }
        }
    }
    Ok(out)
}

/// The page asked for: `page` from 1 and `size` up to [`SIZE_MOST`].
fn paging(page: Option<&str>, size: Option<&str>) -> Result<(usize, usize), Reply> {
    let page = match page.filter(|p| !p.is_empty()) {
        Some(p) => p
            .parse::<usize>()
            .ok()
            .filter(|p| *p >= 1)
            .ok_or_else(|| Reply::error(400, "page is a number from 1"))?,
        None => 1,
    };
    let size = match size.filter(|s| !s.is_empty()) {
        Some(s) => s
            .parse::<usize>()
            .ok()
            .filter(|s| *s >= 1)
            .ok_or_else(|| Reply::error(400, "size is a number from 1"))?
            .min(SIZE_MOST),
        None => SIZE,
    };
    Ok((page, size))
}

/// The page's scans, in order: the most sure first, then by stack.
fn page_of(
    g: &Gathered,
    value: Option<&str>,
    band: Band,
    page: usize,
    size: usize,
) -> Vec<Candidate> {
    let mut all: Vec<&Candidate> = g
        .scans
        .values()
        .filter(|c| band.holds(c.p) && value.is_none_or(|v| v == c.value))
        .collect();
    all.sort_by(|a, b| b.p.total_cmp(&a.p).then_with(|| a.stack.cmp(&b.stack)));
    all.into_iter()
        .skip((page - 1) * size)
        .take(size)
        .cloned()
        .collect()
}

/// The reading both doors share: the model, the band and what waits.
struct Asked {
    model: Model,
    axis: String,
    value: Option<String>,
    band: Band,
    gathered: Gathered,
}

fn asked_of(
    registry: &mut Registry,
    model: Option<&str>,
    axis: Option<&str>,
    value: Option<&str>,
    min: Option<&str>,
    max: Option<&str>,
) -> Result<Asked, Reply> {
    let model = model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| Reply::error(400, "model: the model's id, digest or name@version"))?;
    let axis = axis
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Reply::error(400, "axis: the axis the model proposed on"))?;
    let band = band_of(min, max)?;
    let m = model_named(registry.store(), model)?;
    let gathered = gather(registry, &m, axis)?;
    Ok(Asked {
        model: m,
        axis: axis.to_string(),
        value: value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        band,
        gathered,
    })
}

fn counts_json(c: &Counts) -> (Value, Value, Value) {
    (
        json!(
            c.values
                .iter()
                .map(|(v, n)| json!({"value": v, "count": n}))
                .collect::<Vec<_>>()
        ),
        json!(
            c.bands
                .iter()
                .map(|(from, to, n)| json!({"from": from, "to": to, "count": n}))
                .collect::<Vec<_>>()
        ),
        match c.range {
            Some((lo, hi)) => json!({"low": round(lo), "high": round(hi)}),
            None => Value::Null,
        },
    )
}

/// The document without pictures, as the keyboard prints it and the door
/// starts from.
fn document(a: &Asked, page: usize, size: usize) -> (Value, Vec<Candidate>) {
    let counts = counts_of(&a.gathered, a.value.as_deref(), a.band);
    let shown = page_of(&a.gathered, a.value.as_deref(), a.band, page, size);
    let pages = counts.total.div_ceil(size).max(1);
    let (values, bands, range) = counts_json(&counts);
    let doc = json!({
        "model": a.model.named(),
        "label": a.model.label(),
        "axis": a.axis,
        "value": a.value,
        "min": a.band.min,
        "max": a.band.max,
        "threshold": a.gathered.thresholds.first(),
        "thresholds": a.gathered.thresholds,
        "range": range,
        "values": values,
        "bands": bands,
        "total": counts.total,
        "page": page,
        "pages": pages,
        "size": size,
        "asked": a.gathered.asked,
        "withheld_sealed": a.gathered.sealed,
        "scans": shown.iter().map(|c| json!({
            "stack": c.stack, "item": c.item, "proposed": c.value, "probability": round(c.p), "run": c.run,
        })).collect::<Vec<_>>(),
    });
    (doc, shown)
}

/// Each shown scan's facts, name, picture, the value in force and whether a
/// person decided it, laid over the document.
fn with_scans(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    axis: &str,
    doc: &mut Value,
) -> Result<(), Reply> {
    let stacks: Vec<i64> = doc["scans"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["stack"].as_i64()).collect())
        .unwrap_or_default();
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let known = crate::review_questions::facts(registry.store(), &stacks, quasi).map_err(failed)?;
    let now = nils_registry::round::rules_answers(registry.store(), &stacks).map_err(failed)?;
    let decided = decided_by_person(registry.store(), axis, &stacks).map_err(failed)?;
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans.iter_mut() {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            if let Some(f) = known.get(&stack).and_then(Value::as_object) {
                for (k, v) in f {
                    if k != "stack" {
                        scan[k] = v.clone();
                    }
                }
            }
            scan["now"] = json!(
                now.get(&stack)
                    .and_then(|a| a.get(axis))
                    .and_then(|(v, _)| v.clone())
            );
            scan["decided"] = json!(decided.contains(&stack));
        }
    }
    crate::scans::with_names(registry, pack.as_deref(), doc)?;
    crate::scans::with_pictures(registry, caller, &doors.home, doc, false)?;
    Ok(())
}

/// `GET /api/review/proposals/scans`: one page of a model's proposals on
/// one axis, narrowed by the value and the band, the most sure first.
fn scans(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let get = |k: &str| query.get(k).map(String::as_str);
    let (page, size) = paging(get("page"), get("size"))?;
    let asked = asked_of(
        registry,
        get("model"),
        get("axis"),
        get("value"),
        get("min"),
        get("max"),
    )?;
    let (mut doc, _) = document(&asked, page, size);
    with_scans(doors, registry, caller, quasi, &asked.axis, &mut doc)?;
    // UX round 6: the values the axis takes, for a correction to pick from
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    doc["legal"] = json!(legal_values(pack.as_deref(), &asked.axis));
    Ok(Reply::ok(doc))
}

// --------------------------------------------------------------- accept

fn ids_of(v: &Value, name: &str) -> Result<Vec<i64>, Reply> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(list) => {
            let mut out = Vec::new();
            for s in list {
                let id = s
                    .as_i64()
                    .ok_or_else(|| Reply::error(400, format!("{name} is a list of scan ids")))?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            Ok(out)
        }
        _ => Err(Reply::error(400, format!("{name} is a list of scan ids"))),
    }
}

/// The ids as words for a refusal, the first five.
fn named(ids: &[i64]) -> String {
    let mut words: Vec<String> = ids.iter().take(5).map(i64::to_string).collect();
    if ids.len() > 5 {
        words.push(format!("and {} more", ids.len() - 5));
    }
    words.join(", ")
}

/// What an accept takes from its body or its command line.
struct Accepting<'a> {
    model: &'a str,
    axis: &'a str,
    value: &'a str,
    min: Option<&'a str>,
    max: Option<&'a str>,
    seen: Vec<i64>,
    except: Vec<i64>,
    /// UX round 6: (stack, value) for each scan seen that takes the person's
    /// value in place of the model's, the value as given.
    corrections: Vec<(i64, String)>,
    why: Option<&'a str>,
}

/// One decision an accept wrote.
struct Written {
    stack: i64,
    decision: i64,
    /// The value written: the model's, or the person's for a correction.
    value: String,
    corrected: bool,
}

/// What an accept wrote.
struct Accepted {
    model: Model,
    axis: String,
    value: String,
    decisions: Vec<Written>,
    closed: BTreeSet<i64>,
    left_in_items: i64,
    kept_decided: usize,
    left_out_sealed: usize,
    excepted: usize,
}

/// The corrected values and how many took each, the most first: `2 brain,
/// 1 neck`.
fn corrected_words(decisions: &[Written]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for w in decisions.iter().filter(|w| w.corrected) {
        *by.entry(w.value.as_str()).or_insert(0) += 1;
    }
    let mut list: Vec<(&str, usize)> = by.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    list.iter()
        .map(|(v, n)| format!("{n} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Accepted {
    /// The decisions written as the model's value.
    fn accepted(&self) -> usize {
        self.decisions.iter().filter(|w| !w.corrected).count()
    }

    /// The decisions written as the person's value instead.
    fn corrected(&self) -> impl Iterator<Item = &Written> {
        self.decisions.iter().filter(|w| w.corrected)
    }

    fn as_json(&self) -> Value {
        json!({
            "model": self.model.named(),
            "label": self.model.label(),
            "axis": self.axis,
            "value": self.value,
            "decided": self.accepted(),
            "corrected": self.corrected().count(),
            "decisions": self.decisions.iter().map(|w| json!({
                "stack": w.stack, "decision": w.decision, "value": w.value, "corrected": w.corrected,
            })).collect::<Vec<_>>(),
            "corrections": self.corrected().map(|w| json!({
                "stack": w.stack, "decision": w.decision, "value": w.value,
            })).collect::<Vec<_>>(),
            "closed": self.closed,
            "left_in_items": self.left_in_items,
            "kept_decided": self.kept_decided,
            "left_out_sealed": self.left_out_sealed,
            "set_apart": self.excepted,
        })
    }
}

/// The values the axis takes in the served pack, in the form the axis
/// stores, in the pack's order; none where no pack is served or the pack
/// has no such axis.
fn legal_values(pack: Option<&nils_pack::Pack>, axis: &str) -> Option<Vec<String>> {
    let a = pack?.axes.iter().find(|a| a.name == axis)?;
    Some(
        (0..a.values.len())
            .map(|i| a.stored(i).to_string())
            .collect(),
    )
}

/// One `--correct STACK=VALUE` at the keyboard.
fn correction_of(text: &str) -> Result<(i64, String), String> {
    let bad = || format!("--correct takes STACK=VALUE, a scan id and the value it takes: {text}");
    let (stack, value) = text.split_once('=').ok_or_else(bad)?;
    let stack = stack.trim().parse::<i64>().map_err(|_| bad())?;
    let value = value.trim();
    if value.is_empty() {
        return Err(bad());
    }
    Ok((stack, value.to_string()))
}

/// The corrections as the body gives them: `[{stack, value}]`.
fn corrections_of(v: &Value) -> Result<Vec<(i64, String)>, Reply> {
    let shape = || Reply::error(400, "corrections is a list of {stack, value}");
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(list) => list
            .iter()
            .map(|c| {
                let stack = c["stack"].as_i64().ok_or_else(shape)?;
                let value = c["value"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(shape)?;
                Ok((stack, value.to_string()))
            })
            .collect(),
        _ => Err(shape()),
    }
}

/// The corrections checked: each names a scan the move covers (`covered`:
/// seen on the page, or held by the band) and not set apart, once, and a
/// value the axis takes, which comes back in the form the axis stores.
/// None stands without a served pack to check the value against. A scan
/// not covered is refused with `stranger_words` after its id.
fn corrections_checked(
    axis: &str,
    corrections: &[(i64, String)],
    covered: &dyn Fn(i64) -> bool,
    except: &[i64],
    pack: Option<&nils_pack::Pack>,
    stranger_words: &str,
) -> Result<BTreeMap<i64, String>, Reply> {
    let mut out: BTreeMap<i64, String> = BTreeMap::new();
    if corrections.is_empty() {
        return Ok(out);
    }
    let pack = pack.ok_or_else(|| {
        Reply::gated(
            409,
            "a correction is held to the axis's values, and no pack is served here",
        )
    })?;
    let stranger: Vec<i64> = corrections
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| !covered(*s))
        .collect();
    if !stranger.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "scan {} is corrected, but {stranger_words}",
                named(&stranger)
            ),
        ));
    }
    let apart: Vec<i64> = corrections
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| except.contains(s))
        .collect();
    if !apart.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "scan {} is both set apart and corrected; a corrected scan is not in except",
                named(&apart)
            ),
        ));
    }
    for (stack, given) in corrections {
        let stored = crate::review_questions::stored_value(pack, axis, given)
            .ok_or_else(|| Reply::error(400, format!("{given} is not a value of {axis}")))?;
        if let Some(before) = out.insert(*stack, stored.clone())
            && before != stored
        {
            return Err(Reply::error(
                400,
                format!("scan {stack} is corrected twice, to {before} and to {stored}"),
            ));
        }
    }
    Ok(out)
}

/// The value a decision takes and its words: the person's value where the
/// scan is corrected, the model's where it is not; either way the model's
/// value, probability and run stay in the words beside it, and `why`'s
/// note after them.
fn decision_words(
    model: &Model,
    c: &Candidate,
    corrected: Option<&str>,
    why: Option<&str>,
) -> (String, String) {
    let note = why
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| format!(": {w}"))
        .unwrap_or_default();
    let run = c.run.map(|r| format!(" (run {r})")).unwrap_or_default();
    match corrected {
        Some(v) => (
            v.to_string(),
            format!(
                "corrected {}'s {} at {:.3}{run} to {v}{note}",
                model.label(),
                c.value,
                c.p,
            ),
        ),
        None => (
            c.value.clone(),
            format!(
                "accepted {}'s {} at {:.3}{run}{note}",
                model.label(),
                c.value,
                c.p,
            ),
        ),
    }
}

/// One decision at the stack's scope, in force, through the review spine,
/// inside the transaction the caller holds.
fn apply_one(
    registry: &mut Registry,
    c: &Candidate,
    value: &str,
    why: &str,
    principal: &str,
    author: (&str, Option<&str>, Option<i64>),
) -> Result<nils_registry::review::Applied, Reply> {
    let (kind, version, model) = author;
    nils_registry::review::apply_within(
        registry,
        &nils_registry::review::Apply {
            item: c.item,
            member: Some(c.stack),
            scope: "stack",
            value: Some(value),
            author: nils_registry::review::Author {
                who: principal,
                kind,
                version,
                model,
            },
            stage: false,
            why: Some(why),
            campaign: None,
        },
    )
    .map_err(|e| match e {
        nils_registry::review::Error::Refused(m) => Reply::gated(409, m),
        nils_registry::review::Error::Forbidden(m) => Reply::error(403, m),
        other => Reply::error(500, other.to_string()),
    })
}

/// The decisions, one a scan seen and not set apart, in one transaction.
/// `author` is the caller's kind, version and model as the door verified
/// them; the keyboard's is a person's. `pack` is the served pack, which a
/// correction's value is held to.
fn write(
    registry: &mut Registry,
    a: &Accepting<'_>,
    principal: &str,
    author: (&str, Option<&str>, Option<i64>),
    pack: Option<&nils_pack::Pack>,
) -> Result<Accepted, Reply> {
    if a.seen.is_empty() {
        return Err(Reply::error(400, "seen: the scans the page showed, as ids"));
    }
    let asked = asked_of(
        registry,
        Some(a.model),
        Some(a.axis),
        Some(a.value),
        a.min,
        a.max,
    )?;
    let value = asked
        .value
        .clone()
        .ok_or_else(|| Reply::error(400, "value: the value proposed, which the accept writes"))?;
    let stranger: Vec<i64> = a
        .except
        .iter()
        .copied()
        .filter(|s| !a.seen.contains(s))
        .collect();
    if !stranger.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "scan {} is set apart, but the page did not show it",
                named(&stranger)
            ),
        ));
    }
    let corrections = corrections_checked(
        a.axis,
        &a.corrections,
        &|s| a.seen.contains(&s),
        &a.except,
        pack,
        "the page did not show it",
    )?;
    // every scan seen still waits as the page showed it; one that moved on
    // (answered meanwhile, another value proposed since) is read again
    let gone: Vec<i64> = a
        .seen
        .iter()
        .copied()
        .filter(|s| {
            !asked
                .gathered
                .scans
                .get(s)
                .is_some_and(|c| c.value == value && asked.band.holds(c.p))
        })
        .collect();
    if !gone.is_empty() {
        return Err(Reply::gated(
            409,
            format!(
                "{} no longer proposes {value} on scan {} in this band; it may have been decided: read the page again",
                asked.model.label(),
                named(&gone)
            ),
        ));
    }
    let chosen: Vec<i64> = a
        .seen
        .iter()
        .copied()
        .filter(|s| !a.except.contains(s))
        .collect();
    let store = registry.store();
    // a scan a person decided meanwhile keeps that decision
    let decided = decided_by_person(store, &asked.axis, &chosen).map_err(failed)?;
    // record 48: a sealed stack takes no decision
    let sealed = labels::sealed_for_writes(store, &chosen).map_err(failed)?;
    let writing: Vec<&Candidate> = chosen
        .iter()
        .filter(|s| !decided.contains(s) && !sealed.contains(s))
        .map(|s| &asked.gathered.scans[s])
        .collect();
    let (kind, version, model_author) = author;
    let mut closed: BTreeSet<i64> = BTreeSet::new();
    let mut decisions: Vec<Written> = Vec::new();
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    registry.store().begin().map_err(failed)?;
    let written = (|| -> Result<(), Reply> {
        for c in &writing {
            let corrected = corrections.get(&c.stack).map(String::as_str);
            let (value, why) = decision_words(&asked.model, c, corrected, a.why);
            let applied = apply_one(
                registry,
                c,
                &value,
                &why,
                principal,
                (kind, version, model_author),
            )?;
            closed.extend(applied.closed.iter().copied());
            decisions.push(Written {
                stack: c.stack,
                decision: applied.decision,
                value,
                corrected: corrected.is_some(),
            });
            touched.insert(c.item);
        }
        Ok(())
    })();
    match written {
        Ok(()) => registry.store().commit().map_err(failed)?,
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            return Err(e);
        }
    }
    // what the touched items still hold, not decided
    let mut left_in_items = 0i64;
    let store = registry.store();
    let ids: Vec<i64> = touched.iter().copied().collect();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE item_id IN ({}) AND decided_at IS NULL",
            store.qualified("review_member"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(r) = store.query_opt(&sql, &[]).map_err(failed)? {
            left_in_items += r.int(0).map_err(failed)?;
        }
    }
    Ok(Accepted {
        model: asked.model,
        axis: asked.axis,
        value,
        decisions,
        closed,
        left_in_items,
        kept_decided: chosen.iter().filter(|s| decided.contains(s)).count(),
        left_out_sealed: chosen.iter().filter(|s| sealed.contains(s)).count(),
        excepted: a.except.len(),
    })
}

/// `POST /api/review/proposals/accept`: the decisions, then the re-sort
/// queued as the job a commit queues, so the door answers at once.
fn accept(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let text = |k: &str| -> Result<Option<String>, Reply> {
        match &doc[k] {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s.clone())),
            Value::Number(n) => Ok(Some(n.to_string())),
            _ => Err(Reply::error(400, format!("{k}: text"))),
        }
    };
    let model = text("model")?;
    let axis = text("axis")?;
    let value = text("value")?;
    let min = text("min")?;
    let max = text("max")?;
    let why = text("why")?;
    let author = crate::serve::author_at_apply(registry, caller, &doc)?;
    let a = Accepting {
        model: model.as_deref().unwrap_or(""),
        axis: axis.as_deref().unwrap_or(""),
        value: value.as_deref().unwrap_or(""),
        min: min.as_deref(),
        max: max.as_deref(),
        seen: ids_of(&doc["seen"], "seen")?,
        except: ids_of(&doc["except"], "except")?,
        corrections: corrections_of(&doc["corrections"])?,
        why: why.as_deref(),
    };
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let done = write(registry, &a, &caller.principal, author, pack.as_deref())?;
    // record 58 P1 (spec §12.5): the person's account name, for Results
    nils_registry::names::note(registry.store(), &caller.principal, caller.account_name())
        .map_err(failed)?;
    // the decisions are in force, so the scans they reach are sorted again:
    // the same job a commit queues, with these decisions in its arguments
    let decisions: Vec<i64> = done.decisions.iter().map(|w| w.decision).collect();
    let resort = crate::resort_job::queue_resort_committed(
        registry,
        doors.pack_dir.as_deref(),
        &caller.principal,
        &decisions,
    );
    let mut out = done.as_json();
    out["resort"] = resort;
    Ok(Reply::ok(out))
}

// ------------------------------------------------------------- the band

/// What a band accept takes from its body or its command line (UX round 6).
struct Banding<'a> {
    model: &'a str,
    axis: &'a str,
    value: &'a str,
    min: Option<&'a str>,
    max: Option<&'a str>,
    /// The number of scans the page said the band holds for the value: the
    /// guard that replaces the page accept's `seen`.
    count: usize,
    except: Vec<i64>,
    corrections: Vec<(i64, String)>,
    why: Option<&'a str>,
}

/// The band read and checked: what the job writes.
struct BandChecked {
    model: Model,
    axis: String,
    value: String,
    /// The band's scans of the value less those set apart, the most sure
    /// first.
    scans: Vec<Candidate>,
    excepted: usize,
    corrections: BTreeMap<i64, String>,
}

/// The band as the page shows it, in words: `between 0.80 and 0.90`,
/// `below 0.90`, `at 0.80 or above`, or nothing for the whole range.
fn band_words(b: Band) -> String {
    match (b.min, b.max) {
        (Some(lo), Some(hi)) => format!(" between {lo:.2} and {hi:.2}"),
        (Some(lo), None) => format!(" at {lo:.2} or above"),
        (None, Some(hi)) => format!(" below {hi:.2}"),
        (None, None) => String::new(),
    }
}

/// The band read as the page reads it and held to what the page said.
/// `strict` is the door's and the keyboard's reading: `count` must be the
/// band's count now, and a scan set apart or corrected that the band does
/// not hold is refused. The job runs lenient: the door checked, and a scan
/// that left the band meanwhile (decided, another value proposed since)
/// is simply not written.
fn band_checked(
    registry: &mut Registry,
    b: &Banding<'_>,
    pack: Option<&nils_pack::Pack>,
    strict: bool,
) -> Result<BandChecked, Reply> {
    let asked = asked_of(
        registry,
        Some(b.model),
        Some(b.axis),
        Some(b.value),
        b.min,
        b.max,
    )?;
    let value = asked.value.clone().ok_or_else(|| {
        Reply::error(
            400,
            "value: the value proposed, which the band accept writes; a band is accepted one value at a time",
        )
    })?;
    let mut held: Vec<Candidate> = asked
        .gathered
        .scans
        .values()
        .filter(|c| c.value == value && asked.band.holds(c.p))
        .cloned()
        .collect();
    held.sort_by(|a, b| b.p.total_cmp(&a.p).then_with(|| a.stack.cmp(&b.stack)));
    let label = asked.model.label();
    if held.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "{label} proposes {value} on no scan{} now; nothing to accept",
                band_words(asked.band)
            ),
        ));
    }
    if strict && held.len() != b.count {
        return Err(Reply::gated(
            409,
            format!(
                "{label} proposes {value} on {} scans in this band now, not {}: read the page again",
                held.len(),
                b.count
            ),
        ));
    }
    let in_band: BTreeSet<i64> = held.iter().map(|c| c.stack).collect();
    let stranger: Vec<i64> = b
        .except
        .iter()
        .copied()
        .filter(|s| !in_band.contains(s))
        .collect();
    if strict && !stranger.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "scan {} is set apart, but the band does not hold it",
                named(&stranger)
            ),
        ));
    }
    let corrections: Vec<(i64, String)> = if strict {
        b.corrections.clone()
    } else {
        b.corrections
            .iter()
            .filter(|(s, _)| in_band.contains(s))
            .cloned()
            .collect()
    };
    let corrections = corrections_checked(
        &asked.axis,
        &corrections,
        &|s| in_band.contains(&s),
        &b.except,
        pack,
        "the band does not hold it",
    )?;
    let excepted = held.iter().filter(|c| b.except.contains(&c.stack)).count();
    held.retain(|c| !b.except.contains(&c.stack));
    Ok(BandChecked {
        model: asked.model,
        axis: asked.axis,
        value,
        scans: held,
        excepted,
        corrections,
    })
}

/// The job's name on its row.
fn band_name(checked: &BandChecked, who: &str) -> String {
    format!(
        "accept {} {} of {} on {} for {who}",
        checked.scans.len(),
        checked.value,
        checked.model.label(),
        checked.axis
    )
}

/// `POST /api/review/proposals/accept-band`: the checks, then the verb
/// queued under the caller as a commit's re-sort is queued, and the job
/// said at once.
fn accept_band(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let text = |k: &str| -> Result<Option<String>, Reply> {
        match &doc[k] {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s.clone())),
            Value::Number(n) => Ok(Some(n.to_string())),
            _ => Err(Reply::error(400, format!("{k}: text"))),
        }
    };
    let model = text("model")?;
    let axis = text("axis")?;
    let value = text("value")?;
    let min = text("min")?;
    let max = text("max")?;
    let why = text("why")?;
    let count = match &doc["count"] {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => s.trim().parse::<usize>().ok(),
        _ => None,
    }
    .ok_or_else(|| {
        Reply::error(
            400,
            "count: the number of scans the page said the band holds, a whole number",
        )
    })?;
    // the author is the verified actor, never the body; and a band without
    // a look at each picture is a person's move
    let (kind, _, _) = crate::serve::author_at_apply(registry, caller, &doc)?;
    if kind != "person" {
        return Err(Reply::error(
            403,
            "a band of proposals is accepted by a person; an agent or a model answers one scan at a time",
        ));
    }
    let b = Banding {
        model: model.as_deref().unwrap_or(""),
        axis: axis.as_deref().unwrap_or(""),
        value: value.as_deref().unwrap_or(""),
        min: min.as_deref(),
        max: max.as_deref(),
        count,
        except: ids_of(&doc["except"], "except")?,
        corrections: corrections_of(&doc["corrections"])?,
        why: why.as_deref(),
    };
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let checked = band_checked(registry, &b, pack.as_deref(), true)?;
    // record 58 P1 (spec §12.5): the person's account name, for Results
    nils_registry::names::note(registry.store(), &caller.principal, caller.account_name())
        .map_err(failed)?;
    let who = caller.principal.as_str();
    let mut argv: Vec<String> = vec![
        "review".into(),
        "proposals-accept-band".into(),
        "--model".into(),
        checked.model.id.to_string(),
        "--axis".into(),
        checked.axis.clone(),
        "--value".into(),
        checked.value.clone(),
        "--count".into(),
        count.to_string(),
    ];
    if let Some(lo) = b.min.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push("--min".into());
        argv.push(lo.to_string());
    }
    if let Some(hi) = b.max.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push("--max".into());
        argv.push(hi.to_string());
    }
    if !b.except.is_empty() {
        argv.push("--except".into());
        argv.push(
            b.except
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    for (stack, value) in &checked.corrections {
        argv.push("--correct".into());
        argv.push(format!("{stack}={value}"));
    }
    if let Some(w) = b.why.map(str::trim).filter(|w| !w.is_empty()) {
        argv.push("--why".into());
        argv.push(w.to_string());
    }
    argv.push("--who".into());
    argv.push(who.to_string());
    if let Some(dir) = doors.pack_dir.as_deref() {
        argv.push("--pack-dir".into());
        argv.push(dir.display().to_string());
    }
    argv.push("--pack".into());
    argv.push(doors.ask_pack.clone());
    let name = band_name(&checked, who);
    let job = nils_registry::job::enqueue(registry.store(), &argv, Some(&name), Some(who))
        .map_err(|e| Reply::error(500, format!("the band accept could not be queued: {e}")))?;
    crate::worker::wake();
    let stacks = checked.scans.len();
    Ok(Reply::accepted(json!({
        "job": job,
        "stacks": stacks,
        "model": checked.model.named(),
        "label": checked.model.label(),
        "axis": checked.axis,
        "value": checked.value,
        "count": count,
        "set_apart": checked.excepted,
        "corrections": checked.corrections.len(),
        "words": format!(
            "accepting {stacks} scan(s) as {} of {} on {} in {who}'s name, queued as job {job}",
            checked.value,
            checked.model.label(),
            checked.axis
        ),
    })))
}

/// What a band's job wrote, so far or in all.
#[derive(Default)]
struct BandDone {
    decisions: Vec<Written>,
    closed: BTreeSet<i64>,
    /// The band's scans gone through, written or kept.
    done: usize,
    kept_decided: usize,
    left_out_sealed: usize,
    /// Why the job stopped, where a cancel was asked between chunks.
    stopped: Option<String>,
    /// Why a chunk failed, where one did; what was written before stands.
    failed: Option<String>,
}

/// The members among these candidates that still wait (not decided in
/// their item), by stack: a scan a campaign or another move decided
/// meanwhile is not written.
fn still_waiting(store: &mut Store, chunk: &[Candidate]) -> Result<BTreeSet<i64>, StoreError> {
    let list = chunk
        .iter()
        .map(|c| c.stack.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT item_id, stack_id FROM {} WHERE stack_id IN ({list}) AND decided_at IS NULL",
        store.qualified("review_member"),
    );
    let pairs: BTreeSet<(i64, i64)> = chunk.iter().map(|c| (c.item, c.stack)).collect();
    let mut out = BTreeSet::new();
    for r in store.query(&sql, &[])? {
        let pair = (r.int(0)?, r.int(1)?);
        if pairs.contains(&pair) {
            out.insert(pair.1);
        }
    }
    Ok(out)
}

/// The band's decisions, [`BAND_CHUNK`] at a time, each chunk its own
/// transaction with the job's progress after it; a cancel heard at a
/// heartbeat stops before the next chunk.
fn write_band(
    registry: &mut Registry,
    checked: &BandChecked,
    principal: &str,
    author: (&str, Option<&str>, Option<i64>),
    why: Option<&str>,
    job_id: i64,
) -> BandDone {
    let of = checked.scans.len();
    let mut done = BandDone::default();
    let beat = |registry: &mut Registry, n: usize| -> nils_registry::job::Asked {
        nils_registry::job::beat(
            registry.store(),
            job_id,
            Some(&json!({"done": n, "of": of, "what": "accepted"})),
        )
        .unwrap_or(nils_registry::job::Asked::Nothing)
    };
    beat(registry, 0);
    for chunk in checked.scans.chunks(BAND_CHUNK) {
        let stacks: Vec<i64> = chunk.iter().map(|c| c.stack).collect();
        // a scan a person decided meanwhile keeps that decision; a sealed
        // stack takes none; one its item no longer waits on is not written
        type Sets = (BTreeSet<i64>, BTreeSet<i64>, BTreeSet<i64>);
        let read = (|| -> Result<Sets, Reply> {
            let store = registry.store();
            let decided = decided_by_person(store, &checked.axis, &stacks).map_err(failed)?;
            let sealed = labels::sealed_for_writes(store, &stacks).map_err(failed)?;
            let waiting = still_waiting(store, chunk).map_err(failed)?;
            Ok((decided, sealed, waiting))
        })();
        let (decided, sealed, waiting) = match read {
            Ok(r) => r,
            Err(e) => {
                done.failed = Some(e.body["error"].as_str().unwrap_or("").to_string());
                break;
            }
        };
        let mut wrote: Vec<Written> = Vec::new();
        let mut closed: BTreeSet<i64> = BTreeSet::new();
        let mut kept = 0usize;
        let mut left_sealed = 0usize;
        if let Err(e) = registry.store().begin() {
            done.failed = Some(e.to_string());
            break;
        }
        let written = (|| -> Result<(), Reply> {
            for c in chunk {
                if sealed.contains(&c.stack) {
                    left_sealed += 1;
                    continue;
                }
                if decided.contains(&c.stack) || !waiting.contains(&c.stack) {
                    kept += 1;
                    continue;
                }
                let corrected = checked.corrections.get(&c.stack).map(String::as_str);
                let (value, words) = decision_words(&checked.model, c, corrected, why);
                let applied = apply_one(registry, c, &value, &words, principal, author)?;
                closed.extend(applied.closed.iter().copied());
                wrote.push(Written {
                    stack: c.stack,
                    decision: applied.decision,
                    value,
                    corrected: corrected.is_some(),
                });
            }
            Ok(())
        })();
        let committed = match written {
            Ok(()) => registry.store().commit().map_err(failed),
            Err(e) => Err(e),
        };
        if let Err(e) = committed {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            done.failed = Some(format!(
                "a chunk of {} failed at scan {}: {}",
                chunk.len(),
                done.done + 1,
                e.body["error"].as_str().unwrap_or("")
            ));
            break;
        }
        done.decisions.extend(wrote);
        done.closed.extend(closed);
        done.kept_decided += kept;
        done.left_out_sealed += left_sealed;
        done.done += chunk.len();
        if beat(registry, done.done) == nils_registry::job::Asked::Cancel && done.done < of {
            done.stopped = Some(format!(
                "stopped as asked after {} of {of} scan(s); what was written stands",
                done.done
            ));
            break;
        }
    }
    done
}

/// The job's result as the desk reads it, and its words.
fn band_result(checked: &BandChecked, done: &BandDone, resort: Value) -> Value {
    let decided = done.decisions.iter().filter(|w| !w.corrected).count();
    let corrected = done.decisions.iter().filter(|w| w.corrected).count();
    let state = if done.failed.is_some() {
        "failed"
    } else if done.stopped.is_some() {
        "cancelled"
    } else {
        "done"
    };
    let mut words = format!(
        "accepted {decided} scan(s) as {} of {} on {}",
        checked.value,
        checked.model.label(),
        checked.axis
    );
    if corrected > 0 {
        words.push_str(&format!(
            ", corrected {corrected} ({})",
            corrected_words(&done.decisions)
        ));
    }
    if checked.excepted > 0 {
        words.push_str(&format!(", {} set apart", checked.excepted));
    }
    if done.kept_decided > 0 {
        words.push_str(&format!(", {} kept a person's decision", done.kept_decided));
    }
    if done.left_out_sealed > 0 {
        words.push_str(&format!(", {} sealed", done.left_out_sealed));
    }
    if let Some(why) = done.failed.as_deref().or(done.stopped.as_deref()) {
        words.push_str(&format!("; {why}"));
    }
    if let Some(r) = resort["words"].as_str() {
        words.push_str(&format!("; {r}"));
    }
    json!({
        "model": checked.model.named(),
        "label": checked.model.label(),
        "axis": checked.axis,
        "value": checked.value,
        "state": state,
        "stacks": checked.scans.len(),
        "done": done.done,
        "decided": decided,
        "corrected": corrected,
        "kept_decided": done.kept_decided,
        "left_out_sealed": done.left_out_sealed,
        "excepted": checked.excepted,
        "closed": done.closed,
        "resort": resort,
        "words": words,
    })
}

// ------------------------------------------------------- the keyboard

/// `nils review proposals-scans`: one page of a model's proposals on one
/// axis, without pictures.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scans_verb(
    registry: &mut Registry,
    model: &str,
    axis: &str,
    value: Option<&str>,
    min: Option<&str>,
    max: Option<&str>,
    page: usize,
    size: usize,
    json_out: bool,
) -> Result<(), Exit> {
    let (page, size) = paging(Some(&page.to_string()), Some(&size.to_string()))
        .map_err(|r| usage(r.body["error"].as_str().unwrap_or("bad page").to_string()))?;
    let asked = asked_of(registry, Some(model), Some(axis), value, min, max).map_err(as_exit)?;
    let (mut doc, _) = document(&asked, page, size);
    let stacks: Vec<i64> = doc["scans"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["stack"].as_i64()).collect())
        .unwrap_or_default();
    let now = nils_registry::round::rules_answers(registry.store(), &stacks)
        .map_err(|e| fail(e.to_string()))?;
    let decided = decided_by_person(registry.store(), &asked.axis, &stacks)
        .map_err(|e| fail(e.to_string()))?;
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans.iter_mut() {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            scan["now"] = json!(
                now.get(&stack)
                    .and_then(|a| a.get(&asked.axis))
                    .and_then(|(v, _)| v.clone())
            );
            scan["decided"] = json!(decided.contains(&stack));
        }
    }
    if json_out {
        println!("{doc}");
        return Ok(());
    }
    println!(
        "{} on {}   {} scan(s){}   page {} of {}",
        asked.model.label(),
        asked.axis,
        doc["total"],
        asked
            .value
            .as_deref()
            .map(|v| format!(" proposed {v}"))
            .unwrap_or_default(),
        doc["page"],
        doc["pages"],
    );
    for v in doc["values"].as_array().into_iter().flatten() {
        println!("  {}: {}", v["value"].as_str().unwrap_or(""), v["count"]);
    }
    for s in doc["scans"].as_array().into_iter().flatten() {
        println!(
            "{}\t{}\t{:.3}\tnow {}{}",
            s["stack"],
            s["proposed"].as_str().unwrap_or(""),
            s["probability"].as_f64().unwrap_or(0.0),
            s["now"].as_str().unwrap_or("-"),
            if s["decided"] == true {
                "\tdecided by a person"
            } else {
                ""
            }
        );
    }
    Ok(())
}

fn as_exit(r: Reply) -> Exit {
    let words = r.body["error"].as_str().unwrap_or("refused").to_string();
    if r.status >= 500 {
        fail(words)
    } else {
        usage(words)
    }
}

/// `nils review proposals-accept`: the decisions, then the re-sort at once
/// at the keyboard (no worker is promised there).
#[allow(clippy::too_many_arguments)]
pub(crate) fn accept_verb(
    home: &Home,
    registry: &mut Registry,
    model: &str,
    axis: &str,
    value: &str,
    min: Option<&str>,
    max: Option<&str>,
    seen: Vec<i64>,
    except: Vec<i64>,
    correct: &[String],
    why: Option<&str>,
    pack_dir: Option<PathBuf>,
    pack_name: &str,
    json_out: bool,
) -> Result<(), Exit> {
    let principal = crate::campaigns::who();
    let corrections: Vec<(i64, String)> = correct
        .iter()
        .map(|c| correction_of(c))
        .collect::<Result<_, _>>()
        .map_err(usage)?;
    let a = Accepting {
        model,
        axis,
        value,
        min,
        max,
        seen,
        except,
        corrections,
        why,
    };
    let pack_dir = crate::pack_dir(home, pack_dir).ok();
    let pack = pack_dir
        .as_ref()
        .and_then(|d| nils_pack::load(&d.join(pack_name), None).ok());
    let done = write(
        registry,
        &a,
        &principal,
        ("person", None, None),
        pack.as_ref(),
    )
    .map_err(as_exit)?;
    let stacks: BTreeSet<i64> = done.decisions.iter().map(|w| w.stack).collect();
    let resort = if stacks.is_empty() {
        json!({"state": "done", "stacks": 0, "sorted": 0, "jobs": [], "words": "nothing accepted"})
    } else {
        crate::review_questions::resort_stacks(
            registry,
            pack_dir.as_deref(),
            &format!(
                "re-sort after {principal} accepted {} proposal(s)",
                stacks.len()
            ),
            "accepted proposals",
            &stacks,
        )
    };
    let mut out = done.as_json();
    out["resort"] = resort;
    if json_out {
        println!("{out}");
        return Ok(());
    }
    let corrected = done.corrected().count();
    println!(
        "accepted {} scan(s) as {} of {} on {}{}   {} item(s) closed, {} scan(s) left in them   {} set apart{}{}   {}",
        done.accepted(),
        done.value,
        done.model.label(),
        done.axis,
        if corrected > 0 {
            format!(
                ", corrected {corrected} ({})",
                corrected_words(&done.decisions)
            )
        } else {
            String::new()
        },
        done.closed.len(),
        done.left_in_items,
        done.excepted,
        if done.kept_decided > 0 {
            format!(", {} kept a person's decision", done.kept_decided)
        } else {
            String::new()
        },
        if done.left_out_sealed > 0 {
            format!(", {} sealed", done.left_out_sealed)
        } else {
            String::new()
        },
        out["resort"]["words"].as_str().unwrap_or(""),
    );
    Ok(())
}

/// What `nils review proposals-accept-band` takes.
pub(crate) struct BandArgs<'a> {
    pub(crate) model: &'a str,
    pub(crate) axis: &'a str,
    pub(crate) value: &'a str,
    pub(crate) min: Option<&'a str>,
    pub(crate) max: Option<&'a str>,
    pub(crate) count: usize,
    pub(crate) except: Vec<i64>,
    pub(crate) correct: &'a [String],
    pub(crate) why: Option<&'a str>,
    pub(crate) who: Option<&'a str>,
    pub(crate) pack_dir: Option<PathBuf>,
    pub(crate) pack_name: &'a str,
    pub(crate) no_wait: bool,
    pub(crate) json_out: bool,
}

/// `nils review proposals-accept-band`: the job's work, run by the worker
/// on the row the door queued (adopted through the job id in its
/// environment) or at the keyboard on a row of its own. The band is read
/// and checked, the decisions written in chunks with the progress on the
/// row, the re-sort queued at the end as a commit's is, and the result set
/// on the row. At the keyboard the re-sort is followed unless `--no-wait`;
/// under the worker it is left to the queue, which runs it next.
pub(crate) fn accept_band_verb(
    home: &Home,
    registry: &mut Registry,
    args: BandArgs<'_>,
) -> Result<(), Exit> {
    use nils_registry::job::{self, State};
    let principal = args
        .who
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .unwrap_or_else(crate::campaigns::who);
    let corrections: Vec<(i64, String)> = args
        .correct
        .iter()
        .map(|c| correction_of(c))
        .collect::<Result<_, _>>()
        .map_err(usage)?;
    let b = Banding {
        model: args.model,
        axis: args.axis,
        value: args.value,
        min: args.min,
        max: args.max,
        count: args.count,
        except: args.except,
        corrections,
        why: args.why,
    };
    let pack_dir = crate::pack_dir(home, args.pack_dir).ok();
    let pack = pack_dir
        .as_ref()
        .and_then(|d| nils_pack::load(&d.join(args.pack_name), None).ok());
    // the worker's run adopts the door's row and reads lenient: the door
    // checked, and the registry may have moved since
    let adopted = job::adopted_job().is_some();
    let checked = band_checked(registry, &b, pack.as_ref(), !adopted).map_err(as_exit)?;
    let name = band_name(&checked, &principal);
    let job_id = job::claim(
        registry.store(),
        &job::Claim {
            kind: job::ACCEPT_BAND,
            name: &name,
            args: json!({
                "model": checked.model.label(), "axis": checked.axis, "value": checked.value,
                "stacks": checked.scans.len(), "count": args.count, "who": principal,
                "principal": principal,
            }),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    let done = write_band(
        registry,
        &checked,
        &principal,
        ("person", None, None),
        b.why,
        job_id,
    );
    // what was written is in force, so its scans are sorted again: the
    // job a commit queues, with these decisions in its arguments
    let decisions: Vec<i64> = done.decisions.iter().map(|w| w.decision).collect();
    let resort = crate::resort_job::queue_resort_committed(
        registry,
        pack_dir.as_deref(),
        &principal,
        &decisions,
    );
    let result = band_result(&checked, &done, resort);
    let words = result["words"].as_str().unwrap_or("").to_string();
    let _ = job::set_result(registry.store(), job_id, &result);
    let state = match (&done.failed, &done.stopped) {
        (Some(_), _) => State::Failed,
        (None, Some(_)) => State::Cancelled,
        (None, None) => State::Done,
    };
    job::finish(registry.store(), job_id, state, done.failed.as_deref())
        .map_err(|e| fail(e.to_string()))?;
    if args.json_out {
        println!("{result}");
    } else {
        println!("{words}");
    }
    if let Some(why) = done.failed {
        return Err(fail(format!("band accept: {why}")));
    }
    if adopted || args.no_wait {
        return Ok(());
    }
    match result["resort"]["job"].as_i64() {
        Some(job) => crate::resort_job::follow(registry, job),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(stack: i64, value: &str, p: f64) -> Candidate {
        Candidate {
            stack,
            item: 1,
            value: value.into(),
            p,
            run: Some(1),
        }
    }

    fn gathered(list: Vec<Candidate>) -> Gathered {
        Gathered {
            scans: list.into_iter().map(|c| (c.stack, c)).collect(),
            thresholds: vec![0.9],
            asked: 0,
            sealed: 0,
        }
    }

    #[test]
    fn the_band_holds_min_in_and_max_out() {
        let b = band_of(Some("0.5"), Some("0.9")).ok().unwrap();
        assert!(b.holds(0.5));
        assert!(b.holds(0.89));
        assert!(!b.holds(0.9));
        assert!(!b.holds(0.1));
        assert!(band_of(None, None).ok().unwrap().holds(1.0));
        assert!(band_of(Some("0.9"), Some("0.5")).is_err());
        assert!(band_of(Some("x"), None).is_err());
        assert!(band_of(Some("1.5"), None).is_err());
    }

    #[test]
    fn the_counts_cross_the_value_and_the_band() {
        let g = gathered(vec![
            c(1, "neck", 0.95),
            c(2, "neck", 0.6),
            c(3, "spine", 0.6),
            c(4, "spine", 0.3),
            c(5, "brain", 0.1),
        ]);
        // values are counted in the band; bands over the whole range for the value
        let k = counts_of(
            &g,
            Some("spine"),
            Band {
                min: Some(0.5),
                max: Some(0.9),
            },
        );
        assert_eq!(k.values, vec![("neck".into(), 1), ("spine".into(), 1)]);
        assert_eq!(k.total, 1);
        assert_eq!(k.range, Some((0.1, 0.95)));
        assert_eq!(k.bands.len(), 10);
        assert_eq!(k.bands.iter().map(|b| b.2).sum::<usize>(), 2);
        // the last band takes the top of the range
        let all = counts_of(
            &g,
            None,
            Band {
                min: None,
                max: None,
            },
        );
        assert_eq!(all.bands.iter().map(|b| b.2).sum::<usize>(), 5);
        assert_eq!(all.bands[9].2, 1);
        assert_eq!(all.bands[0].0, 0.1);
        assert_eq!(all.bands[9].1, 0.95);
        // one probability, one band
        let one = counts_of(
            &gathered(vec![c(1, "neck", 0.5)]),
            None,
            Band {
                min: None,
                max: None,
            },
        );
        assert_eq!(one.bands, vec![(0.5, 0.5, 1)]);
        assert_eq!(one.total, 1);
    }

    #[test]
    fn the_page_is_the_most_sure_first() {
        let g = gathered(vec![
            c(1, "neck", 0.6),
            c(2, "neck", 0.8),
            c(3, "neck", 0.7),
            c(4, "spine", 0.9),
        ]);
        let p = page_of(
            &g,
            Some("neck"),
            Band {
                min: None,
                max: None,
            },
            1,
            2,
        );
        assert_eq!(p.iter().map(|c| c.stack).collect::<Vec<_>>(), vec![2, 3]);
        let p = page_of(
            &g,
            Some("neck"),
            Band {
                min: None,
                max: None,
            },
            2,
            2,
        );
        assert_eq!(p.iter().map(|c| c.stack).collect::<Vec<_>>(), vec![1]);
        assert_eq!(paging(None, None).ok(), Some((1, SIZE)));
        assert_eq!(paging(Some("3"), Some("5000")).ok(), Some((3, SIZE_MOST)));
        assert!(paging(Some("0"), None).is_err());
    }

    #[test]
    fn a_correction_at_the_keyboard_is_a_scan_and_its_value() {
        assert_eq!(correction_of("12=brain").ok(), Some((12, "brain".into())));
        assert_eq!(
            correction_of(" 7 = brain-neck ").ok(),
            Some((7, "brain-neck".into()))
        );
        assert!(correction_of("12").is_err());
        assert!(correction_of("x=brain").is_err());
        assert!(correction_of("12=").is_err());
        // the body's shape
        assert_eq!(
            corrections_of(&json!([{"stack": 3, "value": "neck"}])).ok(),
            Some(vec![(3, "neck".into())])
        );
        assert_eq!(corrections_of(&Value::Null).ok(), Some(Vec::new()));
        assert!(corrections_of(&json!([{"stack": 3}])).is_err());
        assert!(corrections_of(&json!([{"stack": "3", "value": "neck"}])).is_err());
        assert!(corrections_of(&json!({"stack": 3})).is_err());
    }

    #[test]
    fn the_band_s_words_are_the_page_s() {
        let b = |min: Option<f64>, max: Option<f64>| band_words(Band { min, max });
        assert_eq!(b(Some(0.8), Some(0.9)), " between 0.80 and 0.90");
        assert_eq!(b(None, Some(0.9)), " below 0.90");
        assert_eq!(b(Some(0.8), None), " at 0.80 or above");
        assert_eq!(b(None, None), "");
    }

    #[test]
    fn the_corrected_words_count_each_value_the_most_first() {
        let w = |stack: i64, value: &str, corrected: bool| Written {
            stack,
            decision: stack,
            value: value.into(),
            corrected,
        };
        let decisions = vec![
            w(1, "spine", false),
            w(2, "brain", true),
            w(3, "neck", true),
            w(4, "brain", true),
        ];
        assert_eq!(corrected_words(&decisions), "2 brain, 1 neck");
        assert_eq!(corrected_words(&decisions[..1]), "");
    }
}
