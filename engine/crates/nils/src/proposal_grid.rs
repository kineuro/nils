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
//!   the band, and ten bands over the model's range for the value. A stack
//!   of a sample sealed now is withheld.
//! - **The accept** (`POST /api/review/proposals/accept`, `nils review
//!   proposals-accept`): one decision per scan seen and not set apart, in
//!   the caller's name, in force, with the model's value, its probability
//!   and its run in the decision's words, written through the review spine
//!   in one transaction; the items whose scans are all decided close as
//!   accepted; the re-sort of those scans is queued as the job a commit
//!   queues ([`crate::resort_job::queue_resort_committed`], `nils review
//!   resort --decisions`), so the door answers at once. The scans set apart
//!   are not written: they stay proposals. A scan a person decided meanwhile
//!   is never overwritten here.

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
pub(crate) const DOORS: [&str; 2] = [
    "GET /api/review/proposals/scans",
    "POST /api/review/proposals/accept",
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
];

/// A page's size when none is asked for, and the most a page holds.
const SIZE: usize = 500;
const SIZE_MOST: usize = 1000;

/// How many bands the histogram has over the model's range.
const BANDS: usize = 10;

/// What each door needs, read by `serve::door` before its own table.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "proposals", "scans"]) => {
            (Need::One("review:see"), Detail::Plain)
        }
        ("POST", ["api", "review", "proposals", "accept"]) => {
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
    why: Option<&'a str>,
}

/// What an accept wrote.
struct Accepted {
    model: Model,
    axis: String,
    value: String,
    /// (stack, decision)
    decisions: Vec<(i64, i64)>,
    closed: BTreeSet<i64>,
    left_in_items: i64,
    kept_decided: usize,
    left_out_sealed: usize,
    excepted: usize,
}

impl Accepted {
    fn as_json(&self) -> Value {
        json!({
            "model": self.model.named(),
            "label": self.model.label(),
            "axis": self.axis,
            "value": self.value,
            "decided": self.decisions.len(),
            "decisions": self.decisions.iter().map(|(s, d)| json!({"stack": s, "decision": d})).collect::<Vec<_>>(),
            "closed": self.closed,
            "left_in_items": self.left_in_items,
            "kept_decided": self.kept_decided,
            "left_out_sealed": self.left_out_sealed,
            "set_apart": self.excepted,
        })
    }
}

/// The decisions, one a scan seen and not set apart, in one transaction.
/// `author` is the caller's kind, version and model as the door verified
/// them; the keyboard's is a person's.
fn write(
    registry: &mut Registry,
    a: &Accepting<'_>,
    principal: &str,
    author: (&str, Option<&str>, Option<i64>),
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
    let mut decisions: Vec<(i64, i64)> = Vec::new();
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    registry.store().begin().map_err(failed)?;
    let written = (|| -> Result<(), Reply> {
        for c in &writing {
            let why = format!(
                "accepted {}'s {} at {:.3}{}{}",
                asked.model.label(),
                c.value,
                c.p,
                c.run.map(|r| format!(" (run {r})")).unwrap_or_default(),
                a.why
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                    .map(|w| format!(": {w}"))
                    .unwrap_or_default()
            );
            let applied = nils_registry::review::apply_within(
                registry,
                &nils_registry::review::Apply {
                    item: c.item,
                    member: Some(c.stack),
                    scope: "stack",
                    value: Some(c.value.as_str()),
                    author: nils_registry::review::Author {
                        who: principal,
                        kind,
                        version,
                        model: model_author,
                    },
                    stage: false,
                    why: Some(why.as_str()),
                    campaign: None,
                },
            )
            .map_err(|e| match e {
                nils_registry::review::Error::Refused(m) => Reply::gated(409, m),
                nils_registry::review::Error::Forbidden(m) => Reply::error(403, m),
                other => Reply::error(500, other.to_string()),
            })?;
            closed.extend(applied.closed.iter().copied());
            decisions.push((c.stack, applied.decision));
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
        why: why.as_deref(),
    };
    let done = write(registry, &a, &caller.principal, author)?;
    // record 58 P1 (spec §12.5): the person's account name, for Results
    nils_registry::names::note(registry.store(), &caller.principal, caller.account_name())
        .map_err(failed)?;
    // the decisions are in force, so the scans they reach are sorted again:
    // the same job a commit queues, with these decisions in its arguments
    let decisions: Vec<i64> = done.decisions.iter().map(|(_, d)| *d).collect();
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
    why: Option<&str>,
    pack_dir: Option<PathBuf>,
    json_out: bool,
) -> Result<(), Exit> {
    let principal = crate::campaigns::who();
    let a = Accepting {
        model,
        axis,
        value,
        min,
        max,
        seen,
        except,
        why,
    };
    let done = write(registry, &a, &principal, ("person", None, None)).map_err(as_exit)?;
    let stacks: BTreeSet<i64> = done.decisions.iter().map(|(s, _)| *s).collect();
    let pack_dir = crate::pack_dir(home, pack_dir).ok();
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
    println!(
        "accepted {} scan(s) as {} of {} on {}   {} item(s) closed, {} scan(s) left in them   {} set apart{}{}   {}",
        done.decisions.len(),
        done.value,
        done.model.label(),
        done.axis,
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
}
