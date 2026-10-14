// SPDX-License-Identifier: AGPL-3.0-only

//! The engine half of the Rules page and of adopting a rule change (record
//! 56, sections 6.4 and 6.5).
//!
//! Record 56 §6.4: "the pack-quality report (equal-rank disagreements, the
//! overrides, later System 1's disagreements); proposals with their effect
//! report and an approval card; a form for each typed operation". §6.5:
//! "Adopting a proposal: an overlay, or a rules release (B5) with its
//! version, then the re-sort and its counts on the development curve."
//!
//! - **The quality report** (`GET /api/packs/{name}/quality`): what the pack
//!   cannot rank, who beat whom where it could, by rule pair, from what each
//!   sort keeps on a stack's classification, and System 1's confident
//!   disagreements once a System 1 model serves.
//! - **Proposals** (`/api/packs/{name}/proposals`): typed operations
//!   rehearsed into their effect report and kept with it
//!   ([`nils_registry::rule_proposal`]), every answer they break named.
//! - **A decision** (`POST .../{id}/decide`), a person's, never a rule's
//!   own (§5.3), with review:work and data:work (record 25: a reviewer
//!   proposes, an operator adopts). A pack edit is written as the pack's
//!   next version where the engine reads its packs, the version before it
//!   kept, and recorded as a rules version to release; a scoped change is
//!   kept for every sort to apply to its scope alone. Either way a re-sort
//!   is queued, and it writes on the proposal what it did ([`measured`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nils_pack::patch::{Patch, Scope};
use nils_registry::Registry;
use nils_registry::rule_proposal::{self as rp, Proposal};
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 6] = [
    "GET /api/packs/{name}/quality",
    "GET /api/packs/{name}/proposals",
    "POST /api/packs/{name}/proposals",
    "GET /api/packs/{name}/proposals/{id}",
    "POST /api/packs/{name}/proposals/{id}/rehearse",
    "POST /api/packs/{name}/proposals/{id}/decide",
];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/packs/{name}/quality",
        false,
        false,
        "bounded",
        "limit pairs a part",
        "Reading the pack's quality",
        "Read the pack's quality",
    ),
    (
        "GET /api/packs/{name}/proposals",
        false,
        false,
        "bounded",
        "limit proposals",
        "Reading the proposed rule changes",
        "Read the proposed rule changes",
    ),
    (
        "POST /api/packs/{name}/proposals",
        true,
        false,
        "bounded",
        "one proposal",
        "Proposing a rule change",
        "Proposed a rule change",
    ),
    (
        "GET /api/packs/{name}/proposals/{id}",
        false,
        false,
        "bounded",
        "one proposal",
        "Reading a proposed rule change",
        "Read a proposed rule change",
    ),
    (
        "POST /api/packs/{name}/proposals/{id}/rehearse",
        true,
        false,
        "bounded",
        "one proposal",
        "Rehearsing a rule change again",
        "Rehearsed a rule change again",
    ),
    (
        "POST /api/packs/{name}/proposals/{id}/decide",
        true,
        false,
        "job",
        "one proposal",
        "Deciding a rule change",
        "Decided a rule change",
    ),
];

/// What each door needs: reading is a reviewer's, as the disagreements
/// door; proposing is the work of whoever rehearses, as the rehearse door;
/// deciding changes how data is sorted, so it is work on Review and on
/// Data at once, an operator's (record 25).
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "packs", _, "quality" | "proposals"])
        | ("GET", ["api", "packs", _, "proposals", _]) => (Need::One("review:see"), Detail::Plain),
        ("POST", ["api", "packs", _, "proposals"])
        | ("POST", ["api", "packs", _, "proposals", _, "rehearse"]) => (
            Need::AnyOf(&["pipelines:work", "review:work"]),
            Detail::Plain,
        ),
        ("POST", ["api", "packs", _, "proposals", _, "decide"]) => {
            (Need::Both("review:work", "data:work"), Detail::Plain)
        }
        _ => return None,
    })
}

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

/// The doors' routes. The grants were checked by the table already.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    let id_of = |s: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(400, format!("{s} is not a proposal id")))
    };
    Some(match segs {
        ["api", "packs", name, "quality"] if get => {
            quality_door(doors, registry, caller, name, query)
        }
        ["api", "packs", name, "proposals"] if get => {
            list_door(doors, registry, caller, name, query)
        }
        ["api", "packs", name, "proposals"] if post => {
            create_door(doors, registry, caller, name, body)
        }
        ["api", "packs", name, "proposals", id] if get => {
            id_of(id).and_then(|id| read_door(doors, registry, caller, name, id))
        }
        ["api", "packs", name, "proposals", id, "rehearse"] if post => {
            id_of(id).and_then(|id| rehearse_door(doors, registry, caller, name, id, body))
        }
        ["api", "packs", name, "proposals", id, "decide"] if post => {
            id_of(id).and_then(|id| decide_door(doors, registry, caller, name, id, body))
        }
        _ => return None,
    })
}

// ------------------------------------------------------------ the pack

/// The served pack's directory, where it is served.
fn pack_dir(doors: &Doors, name: &str) -> Option<PathBuf> {
    doors
        .pack_dir
        .as_ref()
        .map(|d| d.join(name))
        .filter(|d| d.join("pack.yml").is_file())
}

/// What a pack directory's manifest states under `key`, read without
/// loading the pack.
fn manifest_says(dir: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("pack.yml")).ok()?;
    let v: Value = serde_saphyr::from_str(&text).ok()?;
    match &v[key] {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// The version a pack directory's manifest states.
fn version_in(dir: &Path) -> Option<String> {
    manifest_says(dir, "version")
}

// ------------------------------------------------------------ quality

/// One side of a pair, as the classification keeps it.
#[derive(serde::Deserialize, Clone)]
struct Ranked {
    rule_set: String,
    rule: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    confidence: f64,
}

#[derive(serde::Deserialize)]
struct OverrideNote {
    axis: String,
    value: String,
    by: Ranked,
    other: String,
    over: Ranked,
    #[serde(default)]
    rank: String,
}

#[derive(serde::Deserialize)]
struct EqualSide {
    value: String,
    rule_set: String,
    rule: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    confidence: f64,
}

#[derive(serde::Deserialize)]
struct EqualNote {
    axis: String,
    why: String,
    sides: Vec<EqualSide>,
}

/// What a classification notes, as far as the report reads it.
#[derive(serde::Deserialize, Default)]
struct Notes {
    #[serde(default)]
    overrides: Vec<OverrideNote>,
    #[serde(default)]
    equal_rank: Vec<EqualNote>,
}

/// One rule pair, gathered.
#[derive(Default)]
struct Pair {
    axis: String,
    kinds: BTreeSet<String>,
    sides: Vec<Value>,
    stacks: BTreeSet<i64>,
    versions: BTreeSet<String>,
    datasets: BTreeMap<String, i64>,
    count: i64,
}

fn side(rule_set: &str, rule: &str, value: &str, tier: &str, confidence: f64) -> Value {
    json!({
        "rule_set": rule_set, "rule": rule, "value": value,
        "tier": tier, "confidence": confidence,
    })
}

/// The stacks of the active dataset a door names: those its tree holds a
/// file of, as its summary counts them.
fn dataset_stacks(
    registry: &mut Registry,
    name: &str,
) -> Result<(nils_registry::place::Place, BTreeSet<i64>), Reply> {
    let place = crate::scans::dataset_named(registry, name)?;
    let store = registry.store();
    let sources = crate::sources::source_ids(store, &place).map_err(failed)?;
    let mut out = BTreeSet::new();
    for chunk in sources.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for r in store
            .query(
                &format!(
                    "SELECT DISTINCT stack_id FROM {} WHERE source_id IN ({list})",
                    store.qualified("source_stack")
                ),
                &[],
            )
            .map_err(failed)?
        {
            out.insert(r.int(0).map_err(failed)?);
        }
    }
    Ok((place, out))
}

/// Every stack's datasets by name, from the sources holding a file of it
/// (a stack in two datasets is in both).
fn datasets_by_stack(store: &mut Store) -> Result<HashMap<i64, Vec<String>>, StoreError> {
    let owned = nils_registry::place::dataset_sources(store)?;
    let mut by_source: HashMap<i64, String> = HashMap::new();
    for d in owned {
        for s in d.sources {
            by_source.insert(s, d.name.clone());
        }
    }
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if by_source.is_empty() {
        return Ok(out);
    }
    for r in store.query(
        &format!(
            "SELECT stack_id, source_id FROM {}",
            store.qualified("source_stack")
        ),
        &[],
    )? {
        if let Some(name) = by_source.get(&r.int(1)?) {
            let v = out.entry(r.int(0)?).or_default();
            if !v.contains(name) {
                v.push(name.clone());
            }
        }
    }
    Ok(out)
}

/// The pairs of one part, most stacks first, `limit` of them.
fn pairs_doc(
    pairs: BTreeMap<String, Pair>,
    limit: usize,
    examples: usize,
    overrides: bool,
) -> (Vec<Value>, usize) {
    let mut list: Vec<(String, Pair)> = pairs.into_iter().collect();
    list.sort_by(|(ka, a), (kb, b)| b.stacks.len().cmp(&a.stacks.len()).then(ka.cmp(kb)));
    let more = list.len().saturating_sub(limit);
    let out = list
        .into_iter()
        .take(limit)
        .map(|(key, p)| {
            let mut doc = json!({
                "pair": key,
                "axis": p.axis,
                "stacks": p.stacks.len(),
                "examples": p.stacks.iter().take(examples).collect::<Vec<_>>(),
                "versions": p.versions.into_iter().collect::<Vec<_>>(),
            });
            if overrides {
                doc["rank"] = json!(p.kinds.into_iter().collect::<Vec<_>>());
                doc["winner"] = p.sides.first().cloned().unwrap_or(Value::Null);
                doc["over"] = p.sides.get(1).cloned().unwrap_or(Value::Null);
                doc["datasets"] = json!(p.datasets);
                doc["overrides"] = json!(p.count);
            } else {
                doc["why"] = json!(p.kinds.into_iter().collect::<Vec<_>>());
                doc["sides"] = json!(p.sides);
            }
            doc
        })
        .collect();
    (out, more)
}

/// The quality report of a pack, over the stacks it judged last (of one
/// dataset where `only` names them).
pub(crate) fn quality(
    store: &mut Store,
    pack: &str,
    only: Option<&BTreeSet<i64>>,
    reads_sealed: bool,
    limit: usize,
    examples: usize,
) -> Result<Value, StoreError> {
    let d = store.dialect();
    let notes_col = {
        let t = nils_registry::schema::table("classification");
        d.text_of(t.column("notes").expect("notes"))
    };
    let sealed: BTreeSet<i64> = if reads_sealed {
        BTreeSet::new()
    } else {
        store
            .query(
                &format!(
                    "SELECT DISTINCT stack_id FROM {} WHERE unsealed_at IS NULL",
                    store.qualified("sealed_stack")
                ),
                &[],
            )?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?
    };
    let by_stack = datasets_by_stack(store)?;
    let mut equal: BTreeMap<String, Pair> = BTreeMap::new();
    let mut overrides: BTreeMap<String, Pair> = BTreeMap::new();
    let (mut read, mut with_override, mut with_equal, mut counted) = (0i64, 0i64, 0i64, 0i64);
    let sql = format!(
        "SELECT stack_id, pack_version, {notes_col} FROM {} WHERE pack = {} ORDER BY stack_id",
        store.qualified("classification"),
        d.param(1, nils_registry::schema::Type::Text)
    );
    store.query_stream(&sql, &[Param::from(pack)], |r| {
        let stack = r.int(0)?;
        if sealed.contains(&stack) || only.is_some_and(|o| !o.contains(&stack)) {
            return Ok(true);
        }
        read += 1;
        let version = r.text(1)?.to_string();
        let Some(text) = r.opt_text(2)? else {
            return Ok(true);
        };
        let notes: Notes = serde_json::from_str(text).unwrap_or_default();
        let mut equal_here = false;
        for e in &notes.equal_rank {
            for (i, a) in e.sides.iter().enumerate() {
                for b in &e.sides[i + 1..] {
                    if a.value == b.value {
                        continue;
                    }
                    let one = format!("{}/{}={}", a.rule_set, a.rule, a.value);
                    let two = format!("{}/{}={}", b.rule_set, b.rule, b.value);
                    let (x, y, first, second) = if one <= two {
                        (one, two, a, b)
                    } else {
                        (two, one, b, a)
                    };
                    let p = equal.entry(format!("{}: {x} | {y}", e.axis)).or_default();
                    if p.sides.is_empty() {
                        p.axis = e.axis.clone();
                        p.sides = [first, second]
                            .iter()
                            .map(|s| side(&s.rule_set, &s.rule, &s.value, &s.tier, s.confidence))
                            .collect();
                    }
                    p.kinds.insert(e.why.clone());
                    p.stacks.insert(stack);
                    p.versions.insert(version.clone());
                    equal_here = true;
                }
            }
        }
        with_equal += i64::from(equal_here);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for o in &notes.overrides {
            let key = format!(
                "{}: {}/{}={} > {}/{}={}",
                o.axis, o.by.rule_set, o.by.rule, o.value, o.over.rule_set, o.over.rule, o.other
            );
            let p = overrides.entry(key.clone()).or_default();
            if p.sides.is_empty() {
                p.axis = o.axis.clone();
                p.sides = vec![
                    side(
                        &o.by.rule_set,
                        &o.by.rule,
                        &o.value,
                        &o.by.tier,
                        o.by.confidence,
                    ),
                    side(
                        &o.over.rule_set,
                        &o.over.rule,
                        &o.other,
                        &o.over.tier,
                        o.over.confidence,
                    ),
                ];
            }
            if !o.rank.is_empty() {
                p.kinds.insert(o.rank.clone());
            }
            p.versions.insert(version.clone());
            // a stack once per pair
            if seen.insert(key) && p.stacks.insert(stack) {
                p.count += 1;
                counted += 1;
                match by_stack.get(&stack) {
                    Some(names) => {
                        for n in names {
                            *p.datasets.entry(n.clone()).or_insert(0) += 1;
                        }
                    }
                    None => *p.datasets.entry("(none)".into()).or_insert(0) += 1,
                }
            }
        }
        with_override += i64::from(!notes.overrides.is_empty());
        Ok(true)
    })?;
    let (equal_pairs, equal_more) = pairs_doc(equal, limit, examples, false);
    let (override_pairs, override_more) = pairs_doc(overrides, limit, examples, true);
    Ok(json!({
        "stacks": read,
        "disagreements": {"stacks": with_equal, "pairs": equal_pairs, "more": equal_more},
        "overrides": {
            "stacks": with_override,
            "overrides": counted,
            "pairs": override_pairs,
            "more": override_more,
        },
    }))
}

/// System 1's confident disagreements with the rules, by axis and from
/// what to what, over the stacks the quality report read; empty until a
/// System 1 model serves (record 56 section 5.1).
fn system1_part(
    store: &mut Store,
    pack: &str,
    only: Option<&BTreeSet<i64>>,
    reads_sealed: bool,
    examples: usize,
) -> Result<Value, StoreError> {
    let Some(chosen) = nils_registry::opinion::chosen(store)? else {
        return Ok(json!({
            "served": false, "model": null, "threshold": null, "disagreements": [],
            "why": "no System 1 model answers here: it stays in research until it is confirmed (record 56, section 3)",
        }));
    };
    let threshold = chosen.model.threshold().unwrap_or(0.9);
    // the stacks the pack judged, of the dataset where one is named
    let mut stacks: Vec<i64> = Vec::new();
    let d = store.dialect();
    for r in store.query(
        &format!(
            "SELECT stack_id FROM {} WHERE pack = {} ORDER BY stack_id",
            store.qualified("classification"),
            d.param(1, nils_registry::schema::Type::Text)
        ),
        &[Param::from(pack)],
    )? {
        let s = r.int(0)?;
        if only.is_none_or(|o| o.contains(&s)) {
            stacks.push(s);
        }
    }
    if !reads_sealed {
        let (sealed, _) = nils_registry::labels::sealed_now(store, &stacks, &[])
            .map_err(|e| StoreError::Message(e.to_string()))?;
        stacks.retain(|s| !sealed.contains(s));
    }
    let opinions = nils_registry::opinion::newest(store, &stacks, Some(chosen.model.id))?;
    // the rules' stored answers of those stacks
    let mut stored: HashMap<(i64, String), String> = HashMap::new();
    let ids: Vec<i64> = opinions.keys().copied().collect();
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for r in store.query(
            &format!(
                "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) ORDER BY id",
                store.qualified("classification_axis")
            ),
            &[],
        )? {
            let Some(v) = r.opt_text(2)?.filter(|v| !v.is_empty()) else {
                continue;
            };
            stored
                .entry((r.int(0)?, r.text(1)?.to_string()))
                .and_modify(|x| {
                    x.push(',');
                    x.push_str(v);
                })
                .or_insert_with(|| v.to_string());
        }
    }
    let mut rows: BTreeMap<(String, String, String), BTreeSet<i64>> = BTreeMap::new();
    for (stack, by_axis) in &opinions {
        for (axis, o) in by_axis {
            if o.p < threshold {
                continue;
            }
            let have = stored.get(&(*stack, axis.clone())).map(String::as_str);
            if o.agrees(have) {
                continue;
            }
            rows.entry((
                axis.clone(),
                o.rules_compared(have).unwrap_or("").to_string(),
                o.value.clone().unwrap_or_default(),
            ))
            .or_default()
            .insert(*stack);
        }
    }
    let mut list: Vec<((String, String, String), BTreeSet<i64>)> = rows.into_iter().collect();
    list.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    Ok(json!({
        "served": true,
        "model": chosen.model.named(),
        "threshold": threshold,
        "why": null,
        "disagreements": list.into_iter().map(|((axis, rules, s1), stacks)| json!({
            "axis": axis, "rules": rules, "system1": s1, "stacks": stacks.len(),
            "examples": stacks.iter().take(examples).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

fn bounded(
    query: &HashMap<String, String>,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, Reply> {
    match query.get(key) {
        None => Ok(default),
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| Reply::error(400, format!("{key} is a whole number from 1 to {max}"))),
    }
}

fn quality_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let limit = bounded(query, "limit", 100, 500)?;
    let examples = bounded(query, "examples", 10, 50)?;
    let reads_sealed = crate::sealed::reads(&caller.access);
    let dataset = match query.get("dataset").filter(|d| !d.trim().is_empty()) {
        Some(d) => Some(dataset_stacks(registry, d.trim().trim_start_matches('@'))?),
        None => None,
    };
    let only = dataset.as_ref().map(|(_, s)| s);
    let store = registry.store();
    let mut doc = quality(store, name, only, reads_sealed, limit, examples).map_err(failed)?;
    doc["system1"] = system1_part(store, name, only, reads_sealed, examples).map_err(failed)?;
    doc["pack"] = json!(name);
    doc["version"] = json!(pack_dir(doors, name).and_then(|d| version_in(&d)));
    doc["dataset"] = json!(dataset.as_ref().map(|(p, _)| p.name.clone()));
    Ok(Reply::ok(doc))
}

// ------------------------------------------------------------ proposals

/// The kind a patch is: every operation scoped to the pack, a pack edit;
/// every one scoped narrower, a scoped change; a mix, refused.
fn kind_of(patch: &Patch) -> Result<&'static str, Reply> {
    let pack = patch
        .operations
        .iter()
        .filter(|o| o.scope.is_pack())
        .count();
    if pack == patch.operations.len() {
        Ok(rp::PACK)
    } else if pack == 0 {
        Ok(rp::SCOPED)
    } else {
        Err(Reply::error(
            400,
            "a proposal is a pack edit (every operation scoped pack, shipped as a rules release) or a scoped change (every operation scoped to a site, a dataset, a scanner or a batch); these operations mix the two, so propose them as two",
        ))
    }
}

/// A title in words where none is given: the first operation, and how many
/// more.
fn title_of(patch: &Patch) -> String {
    let first = patch.operations.first().map(|o| {
        let w = &o.written;
        let what = match o.kind.as_str() {
            "add_words" | "remove_words" => format!(
                "{} {} on {}",
                o.kind.replace('_', " "),
                w["words"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default(),
                match (
                    w["axis"].as_str(),
                    w["value"].as_str(),
                    w["bucket"].as_str()
                ) {
                    (Some(a), Some(v), _) => format!("{a}={v}"),
                    (Some(a), None, _) => a.to_string(),
                    (_, _, Some(b)) => format!("the bucket {b}"),
                    _ => String::new(),
                }
            ),
            other => {
                let named: Vec<String> = ["axis", "value", "rule", "set", "pick", "role", "border"]
                    .iter()
                    .filter_map(|k| w[k].as_str().map(|v| format!("{k} {v}")))
                    .collect();
                format!("{} {}", other.replace('_', " "), named.join(", "))
            }
        };
        let scope = if o.scope.is_pack() {
            String::new()
        } else {
            format!(" for {}", o.scope.text())
        };
        format!("{}{scope}", what.trim())
    });
    let more = patch.operations.len().saturating_sub(1);
    match (first, more) {
        (Some(f), 0) => f,
        (Some(f), n) => format!("{f}, and {n} more"),
        (None, _) => "a rule change".into(),
    }
}

/// The datasets a site's root holds, by name.
fn site_datasets(store: &mut Store, site: &str) -> Vec<String> {
    let Ok(places) = nils_registry::place::list(store) else {
        return Vec::new();
    };
    let places: Vec<_> = places
        .into_iter()
        .filter(|p| p.role == nils_registry::place::Role::Source && p.retired_at.is_none())
        .collect();
    let Some(root) = places
        .iter()
        .find(|p| p.name == site && p.dataset["kind"] == "root")
    else {
        return Vec::new();
    };
    places
        .iter()
        .filter(|p| p.dataset["kind"] != "root")
        .filter(|p| p.dataset["root"].as_str() == Some(site) || root.holds_path(Path::new(&p.path)))
        .map(|p| p.name.clone())
        .collect()
}

/// The dataset names evidence names: `dataset:NAME`, or `{dataset: NAME |
/// [NAME]}` (or `datasets`) at any depth.
fn evidence_datasets(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            if let Some(rest) = s.trim().strip_prefix("dataset:") {
                for n in rest.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                    out.insert(n.to_string());
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| evidence_datasets(x, out)),
        Value::Object(m) => {
            for (k, x) in m {
                if k == "dataset" || k == "datasets" {
                    match x {
                        Value::String(s) => {
                            out.insert(s.trim().trim_start_matches('@').to_string());
                        }
                        Value::Array(a) => {
                            for n in a.iter().filter_map(Value::as_str) {
                                out.insert(n.trim().trim_start_matches('@').to_string());
                            }
                        }
                        _ => {}
                    }
                } else {
                    evidence_datasets(x, out);
                }
            }
        }
        _ => {}
    }
}

/// The datasets a proposal touches: its scopes name them or their site,
/// its evidence names them, or its effect report moves their stacks.
fn datasets_touched(
    store: &mut Store,
    patch: &Patch,
    evidence: &Value,
    report: &Value,
) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    // the scopes as the report resolved them (an id read as its name)
    let scopes: Vec<String> = report["scope"]["by_operation"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["scope"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(|| patch.operations.iter().map(|o| o.scope.text()).collect());
    for s in scopes {
        match Scope::parse(&s) {
            Ok(Scope::Dataset(names)) => out.extend(names),
            Ok(Scope::Site(site)) => out.extend(site_datasets(store, &site)),
            _ => {}
        }
    }
    evidence_datasets(evidence, &mut out);
    for op in &patch.operations {
        evidence_datasets(&op.evidence, &mut out);
    }
    for a in report["axes"].as_array().into_iter().flatten() {
        for t in a["transitions"].as_array().into_iter().flatten() {
            for (name, n) in t["datasets"].as_object().into_iter().flatten() {
                if name != "(none)" && n.as_i64().unwrap_or(0) > 0 {
                    out.insert(name.clone());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The distinct scopes of a patch's operations, as the report named them
/// (a dataset or a site given by its id is named by its name), in order.
fn scopes_of(patch: &Patch, report: &Value) -> Vec<String> {
    let named: Vec<String> = report["scope"]["by_operation"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["scope"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let all = if named.len() == patch.operations.len() {
        named
    } else {
        patch.operations.iter().map(|o| o.scope.text()).collect()
    };
    let mut out: Vec<String> = Vec::new();
    for s in all {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The approval card: what a person decides on (record 56 sections 5.4 and
/// 5.6), from the effect report.
pub(crate) fn card_of(report: &Value, kind: &str, scopes: &[String]) -> Value {
    let n = |v: &Value| v.as_i64().unwrap_or(0);
    let moved = n(&report["moved"]["stacks"]);
    let mut transitions: Vec<(String, String, String, i64)> = Vec::new();
    for a in report["axes"].as_array().into_iter().flatten() {
        let axis = a["axis"].as_str().unwrap_or("").to_string();
        for t in a["transitions"].as_array().into_iter().flatten() {
            transitions.push((
                axis.clone(),
                t["from"].as_str().unwrap_or("").to_string(),
                t["to"].as_str().unwrap_or("").to_string(),
                n(&t["stacks"]),
            ));
        }
    }
    transitions.sort_by(|a, b| b.3.cmp(&a.3).then(a.cmp(b)));
    transitions.truncate(5);
    let answers = &report["answers"];
    let (fixes, breaks, net) = (
        n(&answers["fixes"]),
        n(&answers["breaks"]),
        n(&answers["net"]),
    );
    let right_to_wrong = answers["right_to_wrong"].clone();
    let settled: i64 = answers["sets"]
        .as_array()
        .map(|s| s.iter().map(|x| n(&x["answers"])).sum())
        .unwrap_or(0);
    let held = report["patch"]["cases"]["held"].as_bool().unwrap_or(true);
    let word = |v: &str| {
        if v.is_empty() {
            "nothing".to_string()
        } else {
            v.to_string()
        }
    };
    let mut says: Vec<String> = Vec::new();
    if breaks > 0 {
        let named: Vec<String> = right_to_wrong
            .as_array()
            .map(|a| {
                a.iter()
                    .take(10)
                    .map(|x| {
                        format!(
                            "stack {} on {} ({} to {}, the answer {})",
                            x["stack"],
                            x["axis"].as_str().unwrap_or(""),
                            word(x["before"].as_str().unwrap_or("")),
                            word(x["after"].as_str().unwrap_or("")),
                            x["answer"].as_str().unwrap_or("none")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let listed = right_to_wrong.as_array().map_or(0, Vec::len);
        says.push(format!(
            "It breaks {breaks} answer{} people gave, right to wrong: {}{}.",
            if breaks == 1 { "" } else { "s" },
            named.join("; "),
            if listed > named.len() {
                format!("; and {} more, every one named below", listed - named.len())
            } else {
                String::new()
            }
        ));
    }
    if !held {
        says.push(
            "The pack's own cases or the patch's no longer hold with it, so it cannot be adopted as it is."
                .into(),
        );
    }
    let moves: Vec<String> = transitions
        .iter()
        .take(3)
        .map(|(axis, from, to, k)| format!("{axis} {} to {} {k}", word(from), word(to)))
        .collect();
    says.push(if moved == 0 {
        "It moves no scan.".into()
    } else {
        format!(
            "It moves {moved} scan{}: {}.",
            if moved == 1 { "" } else { "s" },
            moves.join(", ")
        )
    });
    says.push(format!(
        "Against the {settled} answer{} people gave, it fixes {fixes}, breaks {breaks}, net {net}.",
        if settled == 1 { "" } else { "s" }
    ));
    says.push(if kind == rp::PACK {
        format!(
            "Adopted, it ships as a rules release: {} {}.",
            report["ships"]["pack"].as_str().unwrap_or(""),
            report["ships"]["version"].as_str().unwrap_or("")
        )
    } else {
        format!(
            "Adopted, it applies to {} and to no other scan.",
            scopes.join(" and ")
        )
    });
    json!({
        "moved": moved,
        "transitions": transitions.iter().map(|(axis, from, to, k)| json!({
            "axis": axis, "from": from, "to": to, "stacks": k,
        })).collect::<Vec<_>>(),
        "fixes": fixes,
        "breaks": breaks,
        "net": net,
        "right_to_wrong": right_to_wrong,
        "cases": {"held": held, "failures": report["patch"]["cases"]["failures"].clone()},
        "review": {
            "before": n(&report["review"]["stacks_asked"]["before"]),
            "after": n(&report["review"]["stacks_asked"]["after"]),
        },
        "says": says,
    })
}

/// A patch from a door's body, for the pack the door names.
fn patch_from(name: &str, doc: &Value) -> Result<Patch, Reply> {
    let patch =
        crate::rehearse::patch_of("the body", Some(name), doc).map_err(|e| Reply::error(400, e))?;
    if patch.pack != name {
        return Err(Reply::error(
            400,
            format!(
                "the operations amend {}, and this door is {name}'s",
                patch.pack
            ),
        ));
    }
    Ok(patch)
}

/// The effect report of a patch on the pack served now, every answer it
/// breaks named.
fn rehearsed(
    registry: &mut Registry,
    dir: &Path,
    patch: &Patch,
    doc: &Value,
) -> Result<Value, Reply> {
    let scope = match doc.get("scope").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(Scope::of(v).map_err(|e| Reply::error(400, e))?),
    };
    let examples = doc["examples"]
        .as_u64()
        .map(|n| (n as usize).clamp(1, 50))
        .unwrap_or(5);
    // the replay shares the machine with the doors that serve: half its
    // threads at most
    let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
    let settings = nils_classify::effect::Settings {
        scope,
        examples,
        workers,
        all_breaks: true,
    };
    crate::rehearse::report(registry, dir, patch, &settings).map_err(|e| match e {
        nils_classify::effect::Error::Refused(m) => Reply::error(400, m),
        e => Reply::error(500, e.to_string()),
    })
}

/// The patch as a proposal keeps it: the operations as written, each with
/// its scope, reason and evidence, and the patch's own cases and ships.
fn kept_patch(patch: &Patch, doc: &Value) -> Value {
    let mut out = json!({
        "patch": nils_pack::patch::FORMAT,
        "pack": patch.pack,
        "operations": patch.written(),
    });
    if let Some(s) = &patch.ships {
        out["ships"] = json!(s);
    }
    // the cases as the body wrote them, which the patch reads again
    if let Some(c) = doc.get("cases").or_else(|| doc["patch"].get("cases")) {
        out["cases"] = c.clone();
    }
    out
}

/// Whether a proposed one no longer reads the pack and the registry as they
/// are: the pack served is another version, or a scoped change was
/// adopted since it was rehearsed.
fn stale(doors: &Doors, store: &mut Store, p: &Proposal) -> Result<bool, StoreError> {
    if p.status != rp::PROPOSED {
        return Ok(false);
    }
    let served = pack_dir(doors, &p.pack).and_then(|d| version_in(&d));
    if served.as_deref() != Some(p.on_version.as_str()) {
        return Ok(true);
    }
    let last = rp::last_scoped_adoption(store, &p.pack)?;
    Ok(match (last, &p.rehearsed_at) {
        (Some(l), Some(r)) => l > *r,
        (Some(_), None) => true,
        _ => false,
    })
}

/// What adopting a proposal makes of it.
fn ships_of(p: &Proposal) -> Value {
    if p.kind == rp::PACK {
        let mut s = p.report["ships"].clone();
        if s.is_null() {
            s = json!({"as": "rules release", "pack": p.pack, "from": p.on_version});
        }
        s
    } else {
        json!({"as": "scoped change", "pack": p.pack, "on": p.on_version, "scopes": p.scopes})
    }
}

/// A proposal as the doors answer it: whole, or without its operations and
/// its report for a list.
fn doc_of(
    doors: &Doors,
    store: &mut Store,
    p: &Proposal,
    full: bool,
    quasi: bool,
) -> Result<Value, StoreError> {
    let job = match p.job_id {
        Some(id) => nils_registry::job::show(store, id)
            .map_err(|e| StoreError::Message(e.to_string()))?
            .map(|j| json!({"id": j.id, "state": j.state.name()})),
        None => None,
    };
    let mut applied = p.applied.clone();
    if applied.is_object() && applied["measured"].is_null() {
        applied["state"] = job
            .as_ref()
            .map(|j| j["state"].clone())
            .unwrap_or_else(|| json!("queued"));
    }
    let mut card = p.card.clone();
    if !quasi {
        // record 55 K7: nothing quasi-identifying in a card
        card.as_object_mut().map(|m| m.remove("scanners"));
    }
    let mut doc = json!({
        "id": p.id,
        "pack": p.pack,
        "title": p.title,
        "kind": p.kind,
        "on": p.on_version,
        "scopes": p.scopes,
        "status": p.status,
        "reason": p.reason,
        "evidence": p.evidence,
        "datasets": p.datasets,
        "author": p.author,
        "author_kind": p.author_kind,
        "actor": p.actor,
        "created_at": p.created_at,
        "rehearsed_at": p.rehearsed_at,
        "decided_at": p.decided_at,
        "decided_by": p.decided_by,
        "why": p.why,
        "stale": stale(doors, store, p)?,
        "card": card,
        "ships": ships_of(p),
        "version": p.version,
        "job": job,
        "applied": applied,
    });
    if full {
        doc["operations"] = p.patch["operations"].clone();
        let mut report = p.report.clone();
        if !quasi {
            crate::rehearse::shape_scanners(&mut report);
        }
        doc["report"] = report;
    }
    Ok(doc)
}

fn is_quasi(caller: &Caller) -> bool {
    caller.access.detail >= Detail::Quasi
}

fn list_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let status = match query.get("status").map(String::as_str) {
        None | Some("") => None,
        Some(s) if rp::STATUSES.contains(&s) => Some(s.to_string()),
        Some(s) => {
            return Err(Reply::error(
                400,
                format!("status is proposed, adopted or rejected, not {s}"),
            ));
        }
    };
    let limit = bounded(query, "limit", 100, 500)?;
    let dataset = match query.get("dataset").filter(|d| !d.trim().is_empty()) {
        Some(d) => {
            Some(crate::scans::dataset_named(registry, d.trim().trim_start_matches('@'))?.name)
        }
        None => None,
    };
    let store = registry.store();
    let all = rp::list(store, Some(name)).map_err(failed)?;
    let mut counts = serde_json::Map::new();
    for s in rp::STATUSES {
        counts.insert(
            s.to_string(),
            json!(all.iter().filter(|p| p.status == s).count()),
        );
    }
    let mut out = Vec::new();
    for p in all
        .iter()
        .filter(|p| status.as_deref().is_none_or(|s| p.status == s))
        .filter(|p| dataset.as_deref().is_none_or(|d| p.touches(d)))
        .take(limit)
    {
        out.push(doc_of(doors, store, p, false, is_quasi(caller)).map_err(failed)?);
    }
    Ok(Reply::ok(json!({
        "pack": name,
        "count": out.len(),
        "counts": counts,
        "proposals": out,
    })))
}

fn read_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    id: i64,
) -> Result<Reply, Reply> {
    let store = registry.store();
    let p = rp::get(store, id)
        .map_err(failed)?
        .filter(|p| p.pack == name)
        .ok_or_else(|| Reply::error(404, format!("no proposal {id} of {name}")))?;
    Ok(Reply::ok(
        doc_of(doors, store, &p, true, is_quasi(caller)).map_err(failed)?,
    ))
}

fn create_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let dir =
        pack_dir(doors, name).ok_or_else(|| Reply::error(404, format!("no pack named {name}")))?;
    let patch = patch_from(name, &doc)?;
    let kind = kind_of(&patch)?;
    let report = rehearsed(registry, &dir, &patch, &doc)?;
    let on = version_in(&dir).unwrap_or_default();
    let scopes = scopes_of(&patch, &report);
    let card = card_of(&report, kind, &scopes);
    let evidence = doc.get("evidence").cloned().unwrap_or(Value::Null);
    let title = doc["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| title_of(&patch));
    let store = registry.store();
    let datasets = datasets_touched(store, &patch, &evidence, &report);
    let (author_kind, _) = crate::serve::author_of(caller);
    let kept = kept_patch(&patch, &doc);
    let id = rp::create(
        store,
        &rp::New {
            pack: name,
            title: &title,
            kind,
            on_version: &on,
            scopes: &scopes,
            patch: &kept,
            reason: doc["reason"].as_str(),
            evidence: &evidence,
            datasets: &datasets,
            report: &report,
            card: &card,
            author: &caller.principal,
            author_kind,
            actor: &caller.actor,
        },
    )
    .map_err(failed)?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::RulesPropose,
            scope: json!({"proposal": id, "pack": name, "scopes": scopes}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "title": title,
                "kind": kind,
                "on": on,
                "moved": card["moved"],
                "fixes": card["fixes"],
                "breaks": card["breaks"],
            })),
        },
    )
    .map_err(failed)?;
    let store = registry.store();
    let p = rp::get(store, id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(500, "the proposal was not kept"))?;
    Ok(Reply::created(
        doc_of(doors, store, &p, true, is_quasi(caller)).map_err(failed)?,
    ))
}

fn rehearse_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    id: i64,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let p = rp::get(registry.store(), id)
        .map_err(failed)?
        .filter(|p| p.pack == name)
        .ok_or_else(|| Reply::error(404, format!("no proposal {id} of {name}")))?;
    if p.status != rp::PROPOSED {
        return Err(Reply::error(
            409,
            format!(
                "proposal {id} is {}, and only a proposed one is rehearsed again",
                p.status
            ),
        ));
    }
    let dir =
        pack_dir(doors, name).ok_or_else(|| Reply::error(404, format!("no pack named {name}")))?;
    let patch = nils_classify::scoped::patch_of(&p).map_err(|e| Reply::error(400, e))?;
    let patch = with_cases(patch, &p)?;
    let report = rehearsed(registry, &dir, &patch, &doc)?;
    let on = version_in(&dir).unwrap_or_default();
    let card = card_of(&report, &p.kind, &p.scopes);
    let store = registry.store();
    let datasets = datasets_touched(store, &patch, &p.evidence, &report);
    if !rp::rehearsed(store, id, &on, &report, &card, &datasets).map_err(failed)? {
        return Err(Reply::error(
            409,
            format!("proposal {id} was decided while it was rehearsed"),
        ));
    }
    let p = rp::get(store, id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no proposal {id} of {name}")))?;
    Ok(Reply::ok(
        doc_of(doors, store, &p, true, is_quasi(caller)).map_err(failed)?,
    ))
}

/// A kept patch with its own cases read again, which a rehearsal judges.
fn with_cases(mut patch: Patch, p: &Proposal) -> Result<Patch, Reply> {
    if p.patch.get("cases").is_some() {
        let mut doc = p.patch.clone();
        doc.as_object_mut().map(|m| m.remove("version"));
        let full = Patch::of_value(&format!("proposal {}", p.id), &doc)
            .map_err(|e| Reply::error(400, e.to_string()))?;
        patch.cases = full.cases;
    }
    if let Some(s) = p.patch["ships"].as_str() {
        patch.ships = Some(s.to_string());
    }
    Ok(patch)
}

// ------------------------------------------------------------ deciding

/// One adoption at a time: a pack's next version is written and swapped in
/// whole, and two adoptions at once would both write the same version.
static ADOPTING: Mutex<()> = Mutex::new(());

/// Where the versions a pack edit replaced are kept: beside the deployment's
/// pack directory, one folder per pack and version (record 56 section 5.6:
/// every rules version stays available).
fn versions_dir(packs: &Path) -> PathBuf {
    let name = packs
        .file_name()
        .map_or_else(|| "packs".to_string(), |n| n.to_string_lossy().into_owned());
    packs.with_file_name(format!("{name}.versions"))
}

/// The pack edit's operations written as the pack's next version where the
/// engine reads its packs, the version they replace kept whole beside it.
/// Answers the rules version recorded.
fn write_version(doors: &Doors, name: &str, p: &Proposal) -> Result<Value, Reply> {
    let packs = doors.pack_dir.clone().ok_or_else(|| {
        Reply::error(
            409,
            "no pack directory: this engine serves no packs of its own, so a pack edit has nowhere to be written",
        )
    })?;
    let dir = packs.join(name);
    if !dir.join("pack.yml").is_file() {
        return Err(Reply::error(404, format!("no pack named {name}")));
    }
    let patch = nils_classify::scoped::patch_of(p).map_err(|e| Reply::error(409, e))?;
    let patch = with_cases(patch, p)?;
    let patched = nils_pack::patch::apply(&dir, &patch, &|_| true).map_err(|e| {
        Reply::error(
            409,
            format!("the operations no longer apply to the pack served: {e}; rehearse it again"),
        )
    })?;
    if let Some(e) = &patched.cases {
        return Err(Reply::error(
            409,
            format!(
                "the pack's own cases or the patch's do not hold with it, so it is not adopted: {e}"
            ),
        ));
    }
    let from = patched.pack.version;
    let next = match &patch.ships {
        Some(s) => s.clone(),
        None => {
            let mut v = from;
            v.patch += 1;
            v.to_string()
        }
    };
    let next_v =
        nils_pack::Version::parse(&next, "ships").map_err(|e| Reply::error(409, e.to_string()))?;
    if next_v <= from {
        return Err(Reply::error(
            409,
            format!("{name} is {from}, and a pack edit ships as a later version than {next}"),
        ));
    }
    let keep_root = versions_dir(&packs).join(name);
    std::fs::create_dir_all(&keep_root).map_err(|e| {
        Reply::error(
            409,
            format!(
                "the pack directory cannot take a new version (its packs may be an image's): {e}"
            ),
        )
    })?;
    let incoming = keep_root.join(format!(".incoming-{}", p.id));
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming).map_err(failed)?;
    }
    patched
        .docs
        .write(&incoming, Some(&next))
        .map_err(|e| Reply::error(409, format!("the new version could not be written: {e}")))?;
    let loaded = match nils_pack::load(&incoming, None) {
        Ok(l) => l,
        Err(e) => {
            std::fs::remove_dir_all(&incoming).ok();
            return Err(Reply::error(
                409,
                format!("the new version does not load, so it is not adopted: {e}"),
            ));
        }
    };
    let mut kept = keep_root.join(from.to_string());
    let mut n = 2;
    while kept.exists() {
        kept = keep_root.join(format!("{from}.{n}"));
        n += 1;
    }
    // the swap: the version in use moves aside whole, the new one takes its
    // place; a failure puts the old one back
    std::fs::rename(&dir, &kept).map_err(|e| {
        Reply::error(
            409,
            format!("the pack directory cannot take a new version: {e}"),
        )
    })?;
    if let Err(e) = std::fs::rename(&incoming, &dir) {
        std::fs::rename(&kept, &dir).ok();
        return Err(Reply::error(
            409,
            format!("the new version could not be put in place: {e}"),
        ));
    }
    Ok(json!({
        "pack": name,
        "version": loaded.version.to_string(),
        "from": from.to_string(),
        "tag": crate::rules::tag(name, &loaded.version.to_string()),
        "state": "to_release",
        "kept": kept.display().to_string(),
        "files": patched.docs.changed(),
    }))
}

fn decide_door(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    name: &str,
    id: i64,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let decision = doc["decision"].as_str().unwrap_or("");
    if !matches!(decision, "adopt" | "reject") {
        return Err(Reply::error(400, "decision is adopt or reject"));
    }
    let why = doc["why"].as_str().map(str::trim).filter(|w| !w.is_empty());
    // record 56 section 5.3: a person decides, and the system applies
    let (kind, _) = crate::serve::author_of(caller);
    if kind != "person" {
        return Err(Reply::error(
            403,
            format!(
                "a person decides a rule change (record 56, section 5.3), and this caller acts as {}",
                nils_registry::a_or_an(kind)
            ),
        ));
    }
    let _one = ADOPTING.lock().unwrap_or_else(|e| e.into_inner());
    let p = rp::get(registry.store(), id)
        .map_err(failed)?
        .filter(|p| p.pack == name)
        .ok_or_else(|| Reply::error(404, format!("no proposal {id} of {name}")))?;
    if p.status != rp::PROPOSED {
        return Err(Reply::error(
            409,
            format!("proposal {id} is {} already", p.status),
        ));
    }
    let principal = caller.principal.as_str();
    if decision == "reject" {
        let done = rp::decide(
            registry.store(),
            id,
            &rp::Decided {
                status: rp::REJECTED,
                by: principal,
                why,
                job: None,
                version: &Value::Null,
                applied: &Value::Null,
            },
        )
        .map_err(failed)?;
        if !done {
            return Err(Reply::error(
                409,
                format!("proposal {id} was decided already"),
            ));
        }
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal,
                action: nils_registry::audit::Action::RulesReject,
                scope: json!({"proposal": id, "pack": name}),
                policy: None,
                job_id: None,
                details: Some(json!({"why": why})),
            },
        )
        .map_err(failed)?;
        let store = registry.store();
        let p = rp::get(store, id).map_err(failed)?.expect("kept");
        return Ok(Reply::ok(json!({
            "proposal": doc_of(doors, store, &p, false, is_quasi(caller)).map_err(failed)?,
            "status": rp::REJECTED,
        })));
    }
    // adopt: it reads the pack and the registry as they are
    if stale(doors, registry.store(), &p).map_err(failed)? {
        return Err(Reply::error(
            409,
            format!(
                "proposal {id} was rehearsed against {name} {} or before a scoped change adopted since; rehearse it again before it is adopted",
                p.on_version
            ),
        ));
    }
    if p.card["cases"]["held"] == false {
        return Err(Reply::error(
            409,
            "the pack's own cases or the patch's do not hold with it, so it is not adopted",
        ));
    }
    let expected = json!({
        "moved": p.report["moved"],
        "answers": {
            "fixes": p.report["answers"]["fixes"],
            "breaks": p.report["answers"]["breaks"],
            "net": p.report["answers"]["net"],
            "by_axis": p.report["answers"]["by_axis"],
        },
        "review": p.report["review"]["stacks_asked"],
    });
    let applied = json!({"state": "queued", "expected": expected, "measured": null});
    let (version, scopes, command) = if p.kind == rp::PACK {
        let version = write_version(doors, name, &p)?;
        let mut command = vec![
            "classify".to_string(),
            "--pack".into(),
            name.to_string(),
            "--proposal".into(),
            id.to_string(),
        ];
        if let Some(dir) = &doors.pack_dir {
            command.push("--pack-dir".into());
            command.push(dir.display().to_string());
        }
        (version, vec!["pack".to_string()], command)
    } else {
        let mut command = vec!["classify".to_string(), "--pack".into(), name.to_string()];
        for s in &p.scopes {
            command.push("--scope".into());
            command.push(s.clone());
        }
        command.push("--proposal".into());
        command.push(id.to_string());
        if let Some(dir) = &doors.pack_dir {
            command.push("--pack-dir".into());
            command.push(dir.display().to_string());
        }
        (Value::Null, p.scopes.clone(), command)
    };
    if !rp::decide(
        registry.store(),
        id,
        &rp::Decided {
            status: rp::ADOPTED,
            by: principal,
            why,
            job: None,
            version: &version,
            applied: &applied,
        },
    )
    .map_err(failed)?
    {
        return Err(Reply::error(
            409,
            format!("proposal {id} was decided already"),
        ));
    }
    // what stops reproducing: the handles over what is re-sorted
    let closure = if p.kind == rp::PACK {
        let v = version["version"].as_str().unwrap_or("").to_string();
        match crate::depends::of(doors, registry, "pack", &format!("{name}@{v}"))? {
            crate::depends::Outcome::Closure(c) => Some(c),
            _ => None,
        }
    } else {
        let modality = pack_dir(doors, name)
            .and_then(|d| manifest_says(&d, "modality"))
            .unwrap_or_else(|| "MR".into());
        let stacks = scoped_stacks(registry.store(), &modality, &p.scopes)?;
        Some(crate::depends::of_stacks(
            registry,
            "proposal",
            &id.to_string(),
            stacks,
        )?)
    };
    let job = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(&format!("adopt rule change {id}")),
        Some(principal),
        json!({
            "detail": caller.access.detail.name(),
            "actor": caller.actor,
            "proposal": id,
        }),
    )
    .map_err(|e| {
        Reply::error(
            500,
            format!("adopted, and the re-sort could not be queued: {e}"),
        )
    })?;
    rp::set_job(registry.store(), id, job).map_err(failed)?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::RulesAdopt,
            scope: json!({"proposal": id, "pack": name, "scopes": scopes}),
            policy: None,
            job_id: Some(job),
            details: Some(json!({
                "why": why,
                "version": version,
                "closure": closure.as_ref().map(crate::depends::Closure::counts),
            })),
        },
    )
    .map_err(failed)?;
    let mut invalidated = Vec::new();
    if let Some(c) = &closure {
        invalidated = crate::depends::invalidate(
            registry,
            c,
            &format!("rule change {id} adopted"),
            principal,
        )?;
    }
    let store = registry.store();
    let p = rp::get(store, id).map_err(failed)?.expect("kept");
    Ok(Reply::accepted(json!({
        "proposal": doc_of(doors, store, &p, false, is_quasi(caller)).map_err(failed)?,
        "status": rp::ADOPTED,
        "job": job,
        "version": version,
        "scopes": scopes,
        "invalidated": invalidated,
    })))
}

/// The stacks a scoped change's scopes hold now.
fn scoped_stacks(store: &mut Store, modality: &str, scopes: &[String]) -> Result<Vec<i64>, Reply> {
    let resolver = nils_classify::scoped::Resolver::load(store, modality).map_err(failed)?;
    let mut out: BTreeSet<i64> = BTreeSet::new();
    for s in scopes {
        let scope = Scope::parse(s).map_err(|e| Reply::error(400, e))?;
        let scope = resolver
            .known(&scope, store)
            .map_err(|e| Reply::error(409, e.to_string()))?;
        out.extend(resolver.stacks(&scope));
    }
    Ok(out.into_iter().collect())
}

// ------------------------------------------------------------ the re-sort

/// The stored answers of these stacks, per axis and value, and each stack's
/// answers as a digest per axis, for telling before from after; compact, so
/// a re-sort of the whole archive holds it in a hundred megabytes or so.
pub(crate) struct Snapshot {
    axes: BTreeMap<String, BTreeMap<String, i64>>,
    names: HashMap<u64, String>,
    /// stack → (axis, value) digests, by axis
    by_stack: HashMap<i64, Vec<(u64, u64)>>,
}

/// FNV-1a, 64 bits: a digest of a text, for comparing, never for trust.
fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Snapshot {
    fn new() -> Snapshot {
        Snapshot {
            axes: BTreeMap::new(),
            names: HashMap::new(),
            by_stack: HashMap::new(),
        }
    }

    /// One stack's answers, axis by axis.
    fn keep(&mut self, stack: i64, answers: &BTreeMap<String, String>) {
        let mut digests: Vec<(u64, u64)> = Vec::with_capacity(answers.len());
        for (axis, value) in answers {
            let a = fnv(axis);
            self.names.entry(a).or_insert_with(|| axis.clone());
            digests.push((a, fnv(value)));
            *self
                .axes
                .entry(axis.clone())
                .or_default()
                .entry(value.clone())
                .or_insert(0) += 1;
        }
        digests.sort_unstable();
        self.by_stack.insert(stack, digests);
    }
}

/// What the registry holds of these stacks' answers now (every stack's
/// where `stacks` is none), one stack at a time.
pub(crate) fn snapshot(
    store: &mut Store,
    stacks: Option<&BTreeSet<i64>>,
) -> Result<Snapshot, StoreError> {
    let mut snap = Snapshot::new();
    let mut current: Option<(i64, BTreeMap<String, String>)> = None;
    let mut take =
        |snap: &mut Snapshot, r: nils_registry::store::Row| -> Result<bool, StoreError> {
            let id = r.int(0)?;
            if stacks.is_some_and(|s| !s.contains(&id)) {
                return Ok(true);
            }
            if current.as_ref().is_some_and(|(s, _)| *s != id)
                && let Some((s, answers)) = current.take()
            {
                snap.keep(s, &answers);
            }
            let entry = current.get_or_insert_with(|| (id, BTreeMap::new()));
            let axis = r.text(1)?.to_string();
            let value = r.opt_text(2)?.unwrap_or("").trim().to_string();
            let slot = entry.1.entry(axis).or_default();
            if !value.is_empty() {
                if !slot.is_empty() {
                    slot.push(',');
                }
                slot.push_str(&value);
            }
            Ok(true)
        };
    match stacks {
        Some(s) if s.len() <= 20_000 => {
            let ids: Vec<i64> = s.iter().copied().collect();
            for chunk in ids.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) ORDER BY stack_id, id",
                    store.qualified("classification_axis")
                );
                store.query_stream(&sql, &[], |r| take(&mut snap, r))?;
            }
        }
        _ => {
            let sql = format!(
                "SELECT stack_id, axis, value FROM {} ORDER BY stack_id, id",
                store.qualified("classification_axis")
            );
            store.query_stream(&sql, &[], |r| take(&mut snap, r))?;
        }
    }
    if let Some((s, answers)) = current.take() {
        snap.keep(s, &answers);
    }
    Ok(snap)
}

/// What the re-sort after an adoption did, as the proposal records it: the
/// stacks it judged, those whose stored answer changed and on which axes,
/// and the answers per axis and value before and after it.
pub(crate) fn measured(
    before: &Snapshot,
    after: &Snapshot,
    stacks: usize,
    scope: &[String],
    job: i64,
    state: &str,
) -> Value {
    let mut moved = 0i64;
    let mut by_axis: BTreeMap<String, i64> = BTreeMap::new();
    let ids: BTreeSet<i64> = before
        .by_stack
        .keys()
        .chain(after.by_stack.keys())
        .copied()
        .collect();
    let empty: Vec<(u64, u64)> = Vec::new();
    let name = |a: u64| {
        before
            .names
            .get(&a)
            .or_else(|| after.names.get(&a))
            .cloned()
            .unwrap_or_default()
    };
    for id in ids {
        let b = before.by_stack.get(&id).unwrap_or(&empty);
        let a = after.by_stack.get(&id).unwrap_or(&empty);
        if b == a {
            continue;
        }
        moved += 1;
        let bm: BTreeMap<u64, u64> = b.iter().copied().collect();
        let am: BTreeMap<u64, u64> = a.iter().copied().collect();
        let axes: BTreeSet<u64> = bm.keys().chain(am.keys()).copied().collect();
        for axis in axes {
            if bm.get(&axis) != am.get(&axis) {
                *by_axis.entry(name(axis)).or_insert(0) += 1;
            }
        }
    }
    let label = |v: &str| {
        if v.is_empty() {
            "(none)".to_string()
        } else {
            v.to_string()
        }
    };
    let counts = |s: &Snapshot| -> Value {
        Value::Object(
            s.axes
                .iter()
                .map(|(axis, values)| {
                    (
                        axis.clone(),
                        Value::Object(values.iter().map(|(v, n)| (label(v), json!(n))).collect()),
                    )
                })
                .collect(),
        )
    };
    json!({
        "state": state,
        "scope": scope,
        "stacks": stacks,
        "moved": {"stacks": moved, "axes": by_axis},
        "before": counts(before),
        "after": counts(after),
        "job": job,
        "at": nils_registry::time::now_iso(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_names_every_break_first_and_says_what_moves() {
        let report = json!({
            "moved": {"stacks": 47},
            "axes": [{"axis": "base", "transitions": [
                {"from": "T2w", "to": "FLAIR", "stacks": 44},
                {"from": "", "to": "FLAIR", "stacks": 3},
            ]}],
            "answers": {
                "sets": [{"answers": 60}],
                "fixes": 47, "breaks": 1, "net": 46,
                "right_to_wrong": [{"stack": 9, "axis": "base", "answer": "T2w", "before": "T2w", "after": "FLAIR"}],
            },
            "patch": {"cases": {"held": true, "failures": null}},
            "review": {"stacks_asked": {"before": 5, "after": 2}},
            "ships": {"as": "rules release", "pack": "mri", "version": "1.2.1"},
        });
        let card = card_of(&report, rp::PACK, &["pack".into()]);
        assert_eq!(card["moved"], 47);
        assert_eq!(card["breaks"], 1);
        assert_eq!(card["right_to_wrong"][0]["stack"], 9);
        let says: Vec<&str> = card["says"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            says[0].starts_with("It breaks 1 answer people gave"),
            "{says:?}"
        );
        assert!(
            says[0].contains("stack 9 on base (T2w to FLAIR"),
            "{says:?}"
        );
        assert!(
            says.iter().any(|s| s.contains("base T2w to FLAIR 44")),
            "{says:?}"
        );
        assert!(
            says.iter().any(|s| s.contains("rules release: mri 1.2.1")),
            "{says:?}"
        );
        assert_eq!(card["transitions"][1]["from"], "");
        assert_eq!(card["review"]["after"], 2);
        let scoped = card_of(&report, rp::SCOPED, &["dataset:partner".into()]);
        assert!(
            scoped["says"]
                .to_string()
                .contains("applies to dataset:partner and to no other scan")
        );
    }

    #[test]
    fn evidence_names_datasets_however_it_is_written() {
        let mut out = BTreeSet::new();
        evidence_datasets(
            &json!({"read": "dataset:a,b", "from": [{"dataset": "@c"}], "datasets": ["d"], "review_item": 4}),
            &mut out,
        );
        assert_eq!(out.into_iter().collect::<Vec<_>>(), ["a", "b", "c", "d"]);
    }

    #[test]
    fn a_re_sort_is_measured_by_what_changed() {
        let snap = |rows: &[(i64, &str, &str)]| {
            let mut s = Snapshot::new();
            for (id, a, v) in rows {
                let mut answers = BTreeMap::new();
                answers.insert(a.to_string(), v.to_string());
                s.keep(*id, &answers);
            }
            s
        };
        let before = snap(&[(1, "base", "T2w"), (2, "base", "T2w"), (3, "base", "")]);
        let after = snap(&[(1, "base", "FLAIR"), (2, "base", "T2w"), (3, "base", "")]);
        let m = measured(&before, &after, 3, &["dataset:x".into()], 7, "done");
        assert_eq!(m["moved"]["stacks"], 1);
        assert_eq!(m["moved"]["axes"]["base"], 1);
        assert_eq!(m["before"]["base"]["T2w"], 2);
        assert_eq!(m["after"]["base"]["FLAIR"], 1);
        assert_eq!(m["after"]["base"]["(none)"], 1);
        assert_eq!(m["stacks"], 3);
    }
}
