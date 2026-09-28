// SPDX-License-Identifier: AGPL-3.0-only

//! The A/B campaign (record 48, the reference read by judges): independent
//! voters read every stack of a sealed set, and a person settles only where
//! they differ, blind to which voter said what.
//!
//! - **Voters.** Raters that read the de-identified header (local judges,
//!   Claude sub-agents), each answering every axis with a reason, and the
//!   rules as a voter of their own. A voter's can't tell on an axis is no
//!   vote on it.
//! - **Items.** A stack is an item where the voters give two or more values
//!   on an asked axis (`split`), or where they agree on every asked axis
//!   and the pre-registered seed draws it for the audit of agreements
//!   (`audit`). The audit's share is a floor: while an axis has fewer
//!   agreeing cells audited than the campaign asks, the next agreeing stack
//!   in the seed's order that adds one is drawn too. Split and audit items
//!   are interleaved in the seed's order, and nothing a door serves while
//!   the campaign is open says which is which.
//! - **Candidates.** On each axis the voters' distinct values, each under a
//!   letter drawn at random for the item and axis, with the reason of one
//!   voter that gave it, drawn at random too, so neither the letter nor the
//!   number of reasons tells who said it. Which voters gave a value is kept
//!   for the export and never served while the campaign is open.
//! - **Localizers (reading guide ruling 1).** A stack every voter calls a
//!   localizer asks only the axes the campaign names for one (provenance
//!   and body part); every other axis is answered `not_asked`, which an
//!   answer may say only where its provenance is the localizer's.
//! - **What the answer chose.** The engine reads each axis of the answer
//!   against the item's candidates: a letter, `confirm` for the one value
//!   the voters agreed on, `neither`, `cant_tell` or `not_asked`, kept on
//!   the answer. The person may give a cause per axis afterwards.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::campaign::{self, CANT_TELL, Campaign, Error, NOT_ASKED};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

/// Where a settled disagreement came from (record 48's triage), routed to a
/// pack fix, a ruling, can't tell, a prompt fix, or the reference's own
/// error.
pub const CAUSES: [&str; 5] = [
    "rule_bug",
    "convention_gap",
    "header_ambiguity",
    "rater_error",
    "reader_slip",
];

/// The kinds of item.
pub const SPLIT: &str = "split";
pub const AUDIT: &str = "audit";

/// Whether a campaign settles candidates: it was made by `nils campaign ab`.
pub fn is_ab(c: &Campaign) -> bool {
    c.source["ab"].is_object()
}

/// What a localizer is and the axes it is asked, as the campaign keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct Localizer {
    pub axis: String,
    pub value: String,
    pub asks: Vec<String>,
}

impl Localizer {
    pub fn of(c: &Campaign) -> Option<Localizer> {
        let l = &c.source["ab"]["localizer"];
        Some(Localizer {
            axis: l["axis"].as_str()?.to_string(),
            value: l["value"].as_str()?.to_string(),
            asks: words(&l["asks"]),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({"axis": self.axis, "value": self.value, "asks": self.asks})
    }
}

fn words(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// One voter's value of one axis, as an axes answer says it (a value, a
/// sorted list, null for none, or `cant_tell`), with its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub value: Value,
    pub reason: String,
}

/// A voter and what it said of each stack it read.
#[derive(Debug, Clone, Default)]
pub struct Voter {
    pub name: String,
    pub stacks: BTreeMap<i64, BTreeMap<String, Said>>,
}

/// A voter's value of one axis read against the campaign's question: every
/// name a pack gives it is the caller's to map to the identity first; here
/// it is held to the vocabulary and written as an answer writes it.
pub fn said_value(
    axes: &[String],
    constraints: &Value,
    axis: &str,
    raw: &Value,
) -> Result<Value, String> {
    if !axes.iter().any(|a| a == axis) {
        return Err(format!("{axis} is not asked"));
    }
    if raw.as_str() == Some(NOT_ASKED) {
        return Err(format!(
            "{NOT_ASKED} is a person's word for a localizer, never a voter's"
        ));
    }
    let one = [axis.to_string()];
    let text = json!({ axis: raw }).to_string();
    let joint = campaign::answer_joint_of(&one, constraints, &text).map_err(|e| e.to_string())?;
    let canon: Value = serde_json::from_str(&campaign::canonical_joint(constraints, &joint))
        .map_err(|e| e.to_string())?;
    Ok(canon[axis].clone())
}

/// Which stacks a campaign takes, by whether the voters call them
/// localizers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Localizers {
    With,
    Without,
    Only,
}

impl Localizers {
    pub fn parse(s: &str) -> Result<Localizers, Error> {
        match s {
            "with" => Ok(Localizers::With),
            "without" => Ok(Localizers::Without),
            "only" => Ok(Localizers::Only),
            other => Err(invalid(format!(
                "localizers: with, without or only, not {other}"
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Localizers::With => "with",
            Localizers::Without => "without",
            Localizers::Only => "only",
        }
    }
}

/// How a plan is drawn.
#[derive(Debug, Clone)]
pub struct Options<'a> {
    pub axes: &'a [String],
    pub localizer: Option<&'a Localizer>,
    pub localizers: Localizers,
    /// The share of the agreeing stacks audited, from 0 to 1.
    pub audit_share: f64,
    /// The least agreeing cells audited per axis, where the pool has them.
    pub audit_cells: usize,
    /// The seed, written down before the draw.
    pub seed: &'a str,
}

/// One candidate value of an axis and the voters that gave it, each with
/// its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub value: Value,
    pub sources: Vec<(String, String)>,
}

/// An axis of a stack: whether it is asked, whether the voters split on it
/// (or none gave a value), and its candidates, most voters first.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisPlan {
    pub axis: String,
    pub asked: bool,
    pub split: bool,
    pub candidates: Vec<Candidate>,
}

/// One stack as the plan reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct StackPlan {
    pub stack: i64,
    pub kind: &'static str,
    pub localizer: bool,
    pub axes: Vec<AxisPlan>,
}

impl StackPlan {
    pub fn split_axes(&self) -> Vec<String> {
        self.axes
            .iter()
            .filter(|a| a.asked && a.split)
            .map(|a| a.axis.clone())
            .collect()
    }

    /// The asked axes the voters agree on a value of the pack for: the
    /// cells an audit of this stack checks.
    fn agreed_cells(&self) -> Vec<&str> {
        self.axes
            .iter()
            .filter(|a| a.asked && !a.split)
            .filter(|a| {
                a.candidates
                    .first()
                    .is_some_and(|c| c.value != json!(CANT_TELL))
            })
            .map(|a| a.axis.as_str())
            .collect()
    }
}

/// The plan: the items in the order they are read, and the counts.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub items: Vec<StackPlan>,
    /// Stacks asked about, read by at least one voter, and not.
    pub stacks: usize,
    pub unread: usize,
    /// Left out as localizers, or as not localizers.
    pub left_out: usize,
    pub localizers: usize,
    pub split: usize,
    pub agree: usize,
    pub audit: usize,
    /// Per axis: the stacks split on it, the agreeing cells, and those the
    /// audit checks.
    pub split_by_axis: BTreeMap<String, usize>,
    pub agree_cells: BTreeMap<String, usize>,
    pub audit_cells: BTreeMap<String, usize>,
}

impl Plan {
    pub fn counts(&self) -> Value {
        json!({
            "stacks": self.stacks, "unread": self.unread, "left_out": self.left_out,
            "localizers": self.localizers, "split": self.split, "agree": self.agree,
            "audit": self.audit, "items": self.items.len(),
            "split_by_axis": self.split_by_axis, "agree_cells": self.agree_cells,
            "audit_cells": self.audit_cells,
        })
    }
}

/// A draw in 0..1_000_000 from a seed and a text, the same every time.
fn drawn(seed: &str, what: &str) -> u64 {
    let d = ring::digest::digest(&ring::digest::SHA256, format!("{seed}:{what}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d.as_ref()[..8]);
    u64::from_be_bytes(b)
}

/// Read every stack against the voters and draw the items.
pub fn plan(stacks: &[i64], voters: &[Voter], o: &Options<'_>) -> Plan {
    let mut out = Plan {
        stacks: stacks.len(),
        ..Plan::default()
    };
    let mut split = Vec::new();
    let mut agree = Vec::new();
    for &stack in stacks {
        let read: Vec<(&str, &BTreeMap<String, Said>)> = voters
            .iter()
            .filter_map(|v| v.stacks.get(&stack).map(|m| (v.name.as_str(), m)))
            .collect();
        if read.is_empty() {
            out.unread += 1;
            continue;
        }
        // a localizer where every voter that named the axis's value named
        // the localizer's
        let localizer = o.localizer.is_some_and(|l| {
            let said: Vec<&Value> = read
                .iter()
                .filter_map(|(_, m)| m.get(&l.axis).map(|s| &s.value))
                .filter(|v| v.as_str() != Some(CANT_TELL))
                .collect();
            !said.is_empty() && said.iter().all(|v| v.as_str() == Some(l.value.as_str()))
        });
        let wanted = match o.localizers {
            Localizers::With => true,
            Localizers::Without => !localizer,
            Localizers::Only => localizer,
        };
        if !wanted {
            out.left_out += 1;
            continue;
        }
        if localizer {
            out.localizers += 1;
        }
        let mut axes = Vec::new();
        for axis in o.axes {
            let asked = !localizer || o.localizer.is_some_and(|l| l.asks.contains(axis));
            if !asked {
                axes.push(AxisPlan {
                    axis: axis.clone(),
                    asked: false,
                    split: false,
                    candidates: Vec::new(),
                });
                continue;
            }
            // the voters' values, grouped, in the voters' order
            let mut groups: Vec<Candidate> = Vec::new();
            for (name, m) in &read {
                let Some(s) = m.get(axis) else { continue };
                match groups.iter_mut().find(|c| c.value == s.value) {
                    Some(c) => c.sources.push((name.to_string(), s.reason.clone())),
                    None => groups.push(Candidate {
                        value: s.value.clone(),
                        sources: vec![(name.to_string(), s.reason.clone())],
                    }),
                }
            }
            let (told, unknown): (Vec<Candidate>, Vec<Candidate>) = groups
                .into_iter()
                .partition(|c| c.value != json!(CANT_TELL));
            let (is_split, mut candidates) = match told.len() {
                0 if !unknown.is_empty() => (false, unknown),
                0 => (true, Vec::new()),
                1 => (false, told),
                _ => (true, told),
            };
            candidates.sort_by(|a, b| {
                b.sources
                    .len()
                    .cmp(&a.sources.len())
                    .then_with(|| a.value.to_string().cmp(&b.value.to_string()))
            });
            axes.push(AxisPlan {
                axis: axis.clone(),
                asked: true,
                split: is_split,
                candidates,
            });
        }
        let p = StackPlan {
            stack,
            kind: SPLIT,
            localizer,
            axes,
        };
        if p.split_axes().is_empty() {
            agree.push(p);
        } else {
            for a in p.split_axes() {
                *out.split_by_axis.entry(a).or_default() += 1;
            }
            split.push(p);
        }
    }
    out.split = split.len();
    out.agree = agree.len();
    for p in &agree {
        for a in p.agreed_cells() {
            *out.agree_cells.entry(a.to_string()).or_default() += 1;
        }
    }
    // the audit: the seed's order, its share first, then whatever adds a
    // cell to an axis still short
    agree.sort_by_key(|p| drawn(o.seed, &format!("audit:{}", p.stack)));
    let share = o.audit_share.clamp(0.0, 1.0);
    let first = ((agree.len() as f64) * share).ceil() as usize;
    let mut taken: Vec<StackPlan> = Vec::new();
    let mut cells: BTreeMap<String, usize> = BTreeMap::new();
    for (i, p) in agree.into_iter().enumerate() {
        let short = |cells: &BTreeMap<String, usize>| {
            p.agreed_cells()
                .iter()
                .any(|a| cells.get(*a).copied().unwrap_or(0) < o.audit_cells)
        };
        if i < first || short(&cells) {
            for a in p.agreed_cells() {
                *cells.entry(a.to_string()).or_default() += 1;
            }
            taken.push(StackPlan { kind: AUDIT, ..p });
        }
    }
    out.audit = taken.len();
    out.audit_cells = cells;
    let mut items: Vec<StackPlan> = split.into_iter().chain(taken).collect();
    items.sort_by_key(|p| drawn(o.seed, &format!("order:{}", p.stack)));
    out.items = items;
    out
}

/// A key of its own for one campaign's letters and reasons, drawn now and
/// never kept, so no one can draw them again.
pub fn fresh_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The letters of the candidates, A to Z.
fn letter(i: usize) -> String {
    char::from(b'A' + (i.min(25) as u8)).to_string()
}

/// Keep the plan's candidates on the campaign's items, `item_of` naming the
/// item each stack became. The letters and the reason shown are drawn with
/// `key`, a secret of the call's own that is not kept.
pub fn write(
    store: &mut Store,
    campaign: i64,
    plan: &Plan,
    item_of: &BTreeMap<i64, i64>,
    key: &str,
) -> Result<(), Error> {
    let mut items: Vec<Vec<Param>> = Vec::new();
    let mut cands: Vec<Vec<Param>> = Vec::new();
    for p in &plan.items {
        let Some(&item) = item_of.get(&p.stack) else {
            continue;
        };
        items.push(vec![
            Param::Int(campaign),
            Param::Int(item),
            Param::Int(p.stack),
            Param::from(p.kind),
            Param::Int(i64::from(p.localizer)),
            Param::from(json!(p.split_axes()).to_string()),
        ]);
        for a in p.axes.iter().filter(|a| a.asked) {
            let mut list: Vec<&Candidate> = a.candidates.iter().collect();
            list.sort_by_key(|c| drawn(key, &format!("{}:{}:{}", p.stack, a.axis, c.value)));
            for (i, c) in list.into_iter().enumerate() {
                // one reason shown, of a voter drawn at random among those
                // that gave one
                let reason = c
                    .sources
                    .iter()
                    .filter(|(_, r)| !r.trim().is_empty())
                    .min_by_key(|(v, _)| drawn(key, &format!("{}:{}:{v}", p.stack, a.axis)))
                    .map(|(_, r)| r.trim().to_string());
                let sources: Vec<Value> = c
                    .sources
                    .iter()
                    .map(|(v, r)| json!({"voter": v, "reason": r}))
                    .collect();
                cands.push(vec![
                    Param::Int(campaign),
                    Param::Int(item),
                    Param::from(a.axis.as_str()),
                    Param::from(letter(i)),
                    Param::from(c.value.to_string()),
                    reason.map_or(Param::Null, Param::from),
                    Param::from(json!(sources).to_string()),
                ]);
            }
        }
    }
    for chunk in items.chunks(200) {
        store.insert(
            &Insert::new(
                table("campaign_ab_item"),
                &[
                    "campaign_id",
                    "item_id",
                    "stack_id",
                    "kind",
                    "localizer",
                    "split_axes",
                ],
            ),
            chunk,
        )?;
    }
    for chunk in cands.chunks(200) {
        store.insert(
            &Insert::new(
                table("campaign_ab_candidate"),
                &[
                    "campaign_id",
                    "item_id",
                    "axis",
                    "label",
                    "value",
                    "reason",
                    "sources",
                ],
            ),
            chunk,
        )?;
    }
    Ok(())
}

/// An A/B item as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct AbItem {
    pub item: i64,
    pub stack: i64,
    pub kind: String,
    pub localizer: bool,
    pub split_axes: Vec<String>,
}

/// A candidate as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub item: i64,
    pub axis: String,
    pub label: String,
    pub value: Value,
    pub reason: Option<String>,
    pub sources: Value,
}

fn json_text(store: &Store, t: &str, c: &str) -> String {
    store
        .dialect()
        .text_of(table(t).column(c).unwrap_or_else(|| panic!("{t}.{c}")))
}

fn parsed(t: Option<&str>) -> Value {
    t.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

/// The A/B items of a campaign, by item; one item where `item` is given.
pub fn items_of(
    store: &mut Store,
    campaign: i64,
    item: Option<i64>,
) -> Result<BTreeMap<i64, AbItem>, Error> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT item_id, stack_id, kind, localizer, {} FROM {} WHERE campaign_id = {}",
        json_text(store, "campaign_ab_item", "split_axes"),
        store.qualified("campaign_ab_item"),
        d.param(1, Type::Int)
    );
    let mut params = vec![Param::Int(campaign)];
    if let Some(i) = item {
        sql.push_str(&format!(" AND item_id = {}", d.param(2, Type::Int)));
        params.push(Param::Int(i));
    }
    let mut out = BTreeMap::new();
    for r in store.query(&sql, &params)? {
        let it = AbItem {
            item: r.int(0)?,
            stack: r.int(1)?,
            kind: r.text(2)?.to_string(),
            localizer: r.int(3)? != 0,
            split_axes: words(&parsed(r.opt_text(4)?)),
        };
        out.insert(it.item, it);
    }
    Ok(out)
}

/// The candidates of a campaign, by item and in letter order; one item's
/// where `item` is given.
pub fn candidates_of(
    store: &mut Store,
    campaign: i64,
    item: Option<i64>,
) -> Result<BTreeMap<i64, Vec<Kept>>, Error> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT item_id, axis, label, {}, reason, {} FROM {} WHERE campaign_id = {}",
        json_text(store, "campaign_ab_candidate", "value"),
        json_text(store, "campaign_ab_candidate", "sources"),
        store.qualified("campaign_ab_candidate"),
        d.param(1, Type::Int)
    );
    let mut params = vec![Param::Int(campaign)];
    if let Some(i) = item {
        sql.push_str(&format!(" AND item_id = {}", d.param(2, Type::Int)));
        params.push(Param::Int(i));
    }
    sql.push_str(" ORDER BY item_id, axis, label");
    let mut out: BTreeMap<i64, Vec<Kept>> = BTreeMap::new();
    for r in store.query(&sql, &params)? {
        let k = Kept {
            item: r.int(0)?,
            axis: r.text(1)?.to_string(),
            label: r.text(2)?.to_string(),
            value: parsed(r.opt_text(3)?),
            reason: r.opt_text(4)?.map(str::to_string),
            sources: parsed(r.opt_text(5)?),
        };
        out.entry(k.item).or_default().push(k);
    }
    Ok(out)
}

/// The sheet a person settles an item from: per axis of the question,
/// whether it is asked and split, and its candidates with their letters,
/// values and reasons. Never the voters, never whether the item is an
/// audit. None for an item that is not an A/B item.
pub fn sheet(store: &mut Store, c: &Campaign, item: i64) -> Result<Option<Value>, Error> {
    let Some(it) = items_of(store, c.id, Some(item))?.remove(&item) else {
        return Ok(None);
    };
    let kept = candidates_of(store, c.id, Some(item))?
        .remove(&item)
        .unwrap_or_default();
    let axes = campaign::axes_of(&c.question()?);
    let loc = Localizer::of(c);
    let mut rows = Vec::new();
    for axis in &axes {
        let asked = !it.localizer || loc.as_ref().is_some_and(|l| l.asks.contains(axis));
        let list: Vec<Value> = kept
            .iter()
            .filter(|k| &k.axis == axis)
            .map(|k| json!({"label": k.label, "value": k.value, "reason": k.reason}))
            .collect();
        rows.push(json!({
            "axis": axis,
            "asked": asked,
            "split": asked && it.split_axes.contains(axis),
            "candidates": list,
        }));
    }
    Ok(Some(json!({
        "item": it.item,
        "stack": it.stack,
        "localizer": it.localizer,
        "not_asked": NOT_ASKED,
        "localizer_rule": loc.as_ref().map(Localizer::to_json),
        "axes": rows,
        "causes": CAUSES,
    })))
}

/// Record 48: `not_asked` only where the answer's provenance is the
/// localizer's, on an axis a localizer is not asked, in an A/B campaign.
/// Every other answer passes as it is.
pub fn gate(c: &Campaign, value: Option<&str>) -> Result<(), Error> {
    let Some(obj) = value
        .and_then(|v| serde_json::from_str::<Value>(v).ok())
        .and_then(|v| v.as_object().cloned())
    else {
        return Ok(());
    };
    let says = |v: &Value| {
        v.as_str() == Some(NOT_ASKED)
            || v.as_array()
                .is_some_and(|l| l.iter().any(|x| x.as_str() == Some(NOT_ASKED)))
    };
    let named: Vec<&String> = obj
        .iter()
        .filter(|(_, v)| says(v))
        .map(|(k, _)| k)
        .collect();
    if named.is_empty() {
        return Ok(());
    }
    let Some(l) = is_ab(c).then(|| Localizer::of(c)).flatten() else {
        return Err(invalid(format!(
            "{NOT_ASKED} says a localizer is not asked an axis, which only an A/B campaign's localizer answer says"
        )));
    };
    if obj.get(&l.axis).and_then(Value::as_str) != Some(l.value.as_str()) {
        return Err(invalid(format!(
            "{NOT_ASKED} is said of a localizer only: the answer's {} is not {}",
            l.axis, l.value
        )));
    }
    if let Some(a) = named.iter().find(|a| l.asks.contains(a)) {
        return Err(invalid(format!(
            "{a} is asked of a localizer, so it is answered, never {NOT_ASKED}"
        )));
    }
    Ok(())
}

/// What an answer chose on each axis of an A/B item, from its kept value.
pub fn choices(axes: &[String], it: &AbItem, kept: &[Kept], value: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for axis in axes {
        let v = &value[axis.as_str()];
        let list: Vec<&Kept> = kept.iter().filter(|k| &k.axis == axis).collect();
        let hit = list.iter().find(|k| &k.value == v);
        let chose = match hit {
            Some(k) if it.split_axes.contains(axis) => k.label.clone(),
            Some(_) => "confirm".to_string(),
            None if v.as_str() == Some(CANT_TELL) => "cant_tell".to_string(),
            None if v.as_str() == Some(NOT_ASKED) => "not_asked".to_string(),
            None => "neither".to_string(),
        };
        out.insert(axis.clone(), json!(chose));
    }
    Value::Object(out)
}

/// Keep on an answer what it chose, where its item is an A/B item.
pub fn keep_choices(
    store: &mut Store,
    c: &Campaign,
    item: i64,
    answer: i64,
    value: Option<&str>,
) -> Result<(), Error> {
    if !is_ab(c) {
        return Ok(());
    }
    let Some(it) = items_of(store, c.id, Some(item))?.remove(&item) else {
        return Ok(());
    };
    let Some(v) = value.and_then(|v| serde_json::from_str::<Value>(v).ok()) else {
        return Ok(());
    };
    let kept = candidates_of(store, c.id, Some(item))?
        .remove(&item)
        .unwrap_or_default();
    let axes = campaign::axes_of(&c.question()?);
    let chose = choices(&axes, &it, &kept, &v);
    store.update_by_id(
        table("campaign_answer"),
        &[("choices", Param::from(chose.to_string()))],
        "id",
        answer,
    )?;
    Ok(())
}

/// The causes in force, by answer and axis.
pub fn causes_of(
    store: &mut Store,
    campaign: i64,
) -> Result<BTreeMap<(i64, String), String>, Error> {
    let sql = format!(
        "SELECT answer_id, axis, cause FROM {} WHERE campaign_id = {} ORDER BY id",
        store.qualified("campaign_ab_cause"),
        store.dialect().param(1, Type::Int)
    );
    let mut out = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(campaign)])? {
        let key = (r.int(0)?, r.text(1)?.to_string());
        match r.opt_text(2)? {
            Some(c) => {
                out.insert(key, c.to_string());
            }
            None => {
                out.remove(&key);
            }
        }
    }
    Ok(out)
}

/// A cause given, or taken away with none.
#[derive(Debug, Clone)]
pub struct Cause<'a> {
    pub answer: i64,
    pub axis: &'a str,
    pub cause: Option<&'a str>,
    pub principal: &'a str,
}

/// Record 48: the person who settled an A/B item gives the cause of one
/// axis of their answer, or takes it away. Only on the giver's own answer
/// in an A/B campaign that is not closed, on an axis the question asks.
pub fn set_cause(
    registry: &mut Registry,
    campaign: i64,
    g: &Cause<'_>,
    now: &str,
) -> Result<Value, Error> {
    let store = registry.store();
    let c = campaign::get(store, campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {campaign}")))?;
    if !is_ab(&c) {
        return Err(refused(format!(
            "campaign {} settles no candidates, so its answers take no cause",
            c.name
        )));
    }
    if c.status == "closed" {
        return Err(refused(format!("campaign {} is closed", c.name)));
    }
    let a = campaign::answer_by_id(store, g.answer)?
        .filter(|a| a.campaign_id == c.id)
        .ok_or_else(|| Error::NotFound(format!("no answer {} in campaign {}", g.answer, c.name)))?;
    if a.principal != g.principal {
        return Err(Error::Forbidden(format!(
            "answer {} is not {}'s; a cause is given by who answered",
            a.id, g.principal
        )));
    }
    let axes = campaign::axes_of(&c.question()?);
    if !axes.iter().any(|x| x == g.axis) {
        return Err(invalid(format!(
            "{} is not an axis the campaign asks: {}",
            g.axis,
            axes.join(", ")
        )));
    }
    if let Some(cause) = g.cause
        && !CAUSES.contains(&cause)
    {
        return Err(invalid(format!(
            "cause: {}, not {cause}",
            CAUSES.join(", ")
        )));
    }
    store.insert(
        &Insert::new(
            table("campaign_ab_cause"),
            &[
                "campaign_id",
                "item_id",
                "answer_id",
                "axis",
                "cause",
                "principal",
                "at",
            ],
        ),
        &[vec![
            Param::Int(c.id),
            Param::Int(a.item_id),
            Param::Int(a.id),
            Param::from(g.axis),
            g.cause.map_or(Param::Null, Param::from),
            Param::from(g.principal),
            Param::from(now),
        ]],
    )?;
    audit::record(
        registry,
        &Entry {
            principal: g.principal,
            action: Action::CampaignCause,
            scope: json!({"campaign": c.id, "item": a.item_id, "answer": a.id}),
            policy: None,
            job_id: None,
            details: Some(json!({"axis": g.axis, "cause": g.cause})),
        },
    )?;
    Ok(json!({"answer": a.id, "axis": g.axis, "cause": g.cause}))
}

/// Audit the making of an A/B campaign: its counts and the seed's digest.
pub fn record_made(
    registry: &mut Registry,
    who: &str,
    c: &Campaign,
    plan: &Plan,
) -> Result<(), StoreError> {
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::CampaignAb,
            scope: json!({"campaign": c.id, "name": c.name}),
            policy: None,
            job_id: None,
            details: Some(json!({"counts": plan.counts(), "voters": c.source["ab"]["voters"]})),
        },
    )
    .map(|_| ())
}

fn quantile(mut v: Vec<f64>, q: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    Some((v[i] * 10.0).round() / 10.0)
}

/// The decisions of an A/B campaign, one row per answer standing now and
/// axis: the candidates, what was chosen, the cause, the seconds. With
/// `sources`, each candidate's voters, the item's kind and the voters the
/// chosen value came from; never while the campaign is open.
pub fn decisions(store: &mut Store, c: &Campaign, sources: bool) -> Result<Vec<Value>, Error> {
    let items = items_of(store, c.id, None)?;
    let cands = candidates_of(store, c.id, None)?;
    let causes = causes_of(store, c.id)?;
    let axes = campaign::axes_of(&c.question()?);
    let mut rows = Vec::new();
    for a in campaign::current_answers(store, c.id)? {
        let Some(it) = items.get(&a.item_id) else {
            continue;
        };
        let value: Value = a
            .value
            .as_deref()
            .and_then(|v| serde_json::from_str(v).ok())
            .unwrap_or(Value::Null);
        let kept = cands.get(&a.item_id).cloned().unwrap_or_default();
        let chose = a
            .choices
            .clone()
            .unwrap_or_else(|| choices(&axes, it, &kept, &value));
        for axis in &axes {
            let list: Vec<&Kept> = kept.iter().filter(|k| &k.axis == axis).collect();
            let chosen = chose[axis.as_str()]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let mut row = json!({
                "item": it.item, "stack": it.stack, "answer": a.id, "principal": a.principal,
                "axis": axis, "split": it.split_axes.contains(axis), "localizer": it.localizer,
                "candidates": list.iter().map(|k| {
                    let mut v = json!({"label": k.label, "value": k.value, "reason": k.reason});
                    if sources {
                        v["voters"] = json!(k.sources.as_array().into_iter().flatten()
                            .filter_map(|s| s["voter"].as_str()).collect::<Vec<_>>());
                    }
                    v
                }).collect::<Vec<_>>(),
                "value": value[axis.as_str()],
                "choice": chosen,
                "cause": causes.get(&(a.id, axis.clone())),
                "seconds": a.seconds, "unsure": a.unsure, "answered_at": a.answered_at,
            });
            if sources {
                row["kind"] = json!(it.kind);
                row["chosen_voters"] = json!(
                    list.iter()
                        .find(|k| k.value == value[axis.as_str()])
                        .map(|k| {
                            k.sources
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|s| s["voter"].as_str())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                );
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

/// How an A/B campaign is going: the items answered, the seconds per
/// decision, the share of each choice on the split axes and all told, and
/// the causes per axis. With `sources`, the voters chosen per axis and the
/// audit's joint error.
pub fn summary(store: &mut Store, c: &Campaign, sources: bool) -> Result<Value, Error> {
    let items = items_of(store, c.id, None)?;
    let rows = decisions(store, c, sources)?;
    let answered: BTreeSet<i64> = rows.iter().filter_map(|r| r["item"].as_i64()).collect();
    let mut seconds: BTreeMap<i64, f64> = BTreeMap::new();
    let mut split: BTreeMap<String, usize> = BTreeMap::new();
    let mut every: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_axis: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut causes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut voters: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let (mut audit_cells, mut audit_changed) = (0usize, 0usize);
    for r in &rows {
        if let (Some(i), Some(s)) = (r["item"].as_i64(), r["seconds"].as_f64()) {
            seconds.insert(i, s);
        }
        let axis = r["axis"].as_str().unwrap_or_default().to_string();
        let choice = r["choice"].as_str().unwrap_or_default().to_string();
        *every.entry(choice.clone()).or_default() += 1;
        if r["split"] == json!(true) {
            // a letter is a letter; which one is the draw's
            let k = if choice.len() == 1 {
                "letter".to_string()
            } else {
                choice.clone()
            };
            *split.entry(choice.clone()).or_default() += 1;
            *by_axis
                .entry(axis.clone())
                .or_default()
                .entry(k)
                .or_default() += 1;
        }
        if let Some(cause) = r["cause"].as_str() {
            *causes
                .entry(axis.clone())
                .or_default()
                .entry(cause.to_string())
                .or_default() += 1;
        }
        if sources {
            if r["split"] == json!(true) {
                let chosen = r["chosen_voters"].as_array().cloned().unwrap_or_default();
                let e = voters.entry(axis.clone()).or_default();
                if chosen.is_empty() {
                    *e.entry(choice.clone()).or_default() += 1;
                }
                for v in chosen.iter().filter_map(Value::as_str) {
                    *e.entry(v.to_string()).or_default() += 1;
                }
            }
            if r["kind"] == json!(AUDIT) && choice != "not_asked" {
                let agreed_told = r["candidates"][0]["value"] != json!(CANT_TELL);
                if agreed_told {
                    audit_cells += 1;
                    if choice != "confirm" {
                        audit_changed += 1;
                    }
                }
            }
        }
    }
    let secs: Vec<f64> = seconds.values().copied().collect();
    let mut out = json!({
        "campaign": c.id,
        "items": items.len(),
        "answered": answered.len(),
        "seconds": {"median": quantile(secs.clone(), 0.5), "p90": quantile(secs, 0.9)},
        "split_choices": split,
        "split_by_axis": by_axis,
        "choices": every,
        "causes": causes,
        "sources": sources,
    });
    if sources {
        out["chosen_voters"] = json!(voters);
        out["audit"] = json!({"cells": audit_cells, "changed": audit_changed,
            "items": items.values().filter(|i| i.kind == AUDIT).count()});
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(v: Value) -> Said {
        Said {
            value: v,
            reason: "because".into(),
        }
    }

    #[test]
    fn a_split_axis_makes_an_item_and_agreement_draws_an_audit() {
        let axes = vec!["provenance".to_string(), "base".to_string()];
        let loc = Localizer {
            axis: "provenance".into(),
            value: "Localizer".into(),
            asks: vec!["provenance".into()],
        };
        let mut rows_a = Vec::new();
        let mut rows_b = Vec::new();
        let mut rows_r = Vec::new();
        for s in 1..=40i64 {
            let base = if s <= 5 { json!("T2w") } else { json!("T1w") };
            let a: Vec<(&str, Value)> =
                vec![("provenance", json!("RawRecon")), ("base", json!("T1w"))];
            let b: Vec<(&str, Value)> = vec![
                ("provenance", json!("RawRecon")),
                ("base", json!(CANT_TELL)),
            ];
            let r: Vec<(&str, Value)> = vec![("provenance", json!("RawRecon")), ("base", base)];
            rows_a.push((s, a));
            rows_b.push((s, b));
            rows_r.push((s, r));
        }
        // stack 41: a localizer, whose base is not asked
        rows_a.push((
            41,
            vec![("provenance", json!("Localizer")), ("base", json!("T1w"))],
        ));
        rows_r.push((
            41,
            vec![("provenance", json!("Localizer")), ("base", json!("none"))],
        ));
        let mk = |name: &str, rows: &Vec<(i64, Vec<(&str, Value)>)>| Voter {
            name: name.into(),
            stacks: rows
                .iter()
                .map(|(s, a)| {
                    (
                        *s,
                        a.iter()
                            .map(|(k, v)| (k.to_string(), said(v.clone())))
                            .collect(),
                    )
                })
                .collect(),
        };
        let voters = vec![
            mk("judge-a", &rows_a),
            mk("judge-b", &rows_b),
            mk("rules", &rows_r),
        ];
        let stacks: Vec<i64> = (1..=42).collect();
        let o = Options {
            axes: &axes,
            localizer: Some(&loc),
            localizers: Localizers::With,
            audit_share: 0.1,
            audit_cells: 0,
            seed: "pre-registered",
        };
        let p = plan(&stacks, &voters, &o);
        assert_eq!(p.unread, 1, "stack 42 was read by no voter");
        assert_eq!(p.split, 5, "{:?}", p.counts());
        assert_eq!(p.localizers, 1);
        assert_eq!(p.agree, 36);
        assert_eq!(p.audit, 4, "a tenth of 36, rounded up");
        let split: Vec<&StackPlan> = p.items.iter().filter(|i| i.kind == SPLIT).collect();
        assert!(
            split
                .iter()
                .all(|i| i.split_axes() == vec!["base".to_string()])
        );
        // can't tell is no vote: the two values are the candidates, the
        // one two voters gave first
        let base = &split[0].axes[1];
        assert_eq!(base.candidates.len(), 2);
        assert_eq!(base.candidates[0].value, json!("T1w"));
        // the same seed draws the same items in the same order
        let again = plan(&stacks, &voters, &o);
        assert_eq!(again.items, p.items);
        let other = plan(
            &stacks,
            &voters,
            &Options {
                seed: "another",
                ..o.clone()
            },
        );
        assert_ne!(
            other.items.iter().map(|i| i.stack).collect::<Vec<_>>(),
            p.items.iter().map(|i| i.stack).collect::<Vec<_>>()
        );
        // the floor of cells draws more where an axis is short
        let more = plan(
            &stacks,
            &voters,
            &Options {
                audit_cells: 10,
                ..o.clone()
            },
        );
        assert!(more.audit >= 10, "{:?}", more.counts());
        assert!(more.audit_cells["base"] >= 10);
        // localizers apart
        let only = plan(
            &stacks,
            &voters,
            &Options {
                localizers: Localizers::Only,
                audit_share: 1.0,
                ..o.clone()
            },
        );
        assert_eq!(only.items.len(), 1);
        assert!(only.items[0].localizer);
        assert!(!only.items[0].axes[1].asked);
        let without = plan(
            &stacks,
            &voters,
            &Options {
                localizers: Localizers::Without,
                ..o.clone()
            },
        );
        assert_eq!(without.left_out, 1);
    }

    #[test]
    fn what_an_answer_chose_is_read_against_the_candidates() {
        let axes = vec![
            "technique".to_string(),
            "base".to_string(),
            "body_part".to_string(),
        ];
        let it = AbItem {
            item: 7,
            stack: 3,
            kind: SPLIT.into(),
            localizer: false,
            split_axes: vec!["technique".into()],
        };
        let k = |axis: &str, label: &str, v: Value| Kept {
            item: 7,
            axis: axis.into(),
            label: label.into(),
            value: v,
            reason: None,
            sources: json!([]),
        };
        let kept = vec![
            k("technique", "A", json!("TSE")),
            k("technique", "B", json!("SE")),
            k("base", "A", json!("T2w")),
            k("body_part", "A", json!("brain")),
        ];
        let got = choices(
            &axes,
            &it,
            &kept,
            &json!({"technique": "SE", "base": "T2w", "body_part": "spine"}),
        );
        assert_eq!(
            got,
            json!({"technique": "B", "base": "confirm", "body_part": "neither"})
        );
        let got = choices(
            &axes,
            &it,
            &kept,
            &json!({"technique": CANT_TELL, "base": "T2w", "body_part": "brain"}),
        );
        assert_eq!(got["technique"], "cant_tell");
    }

    #[test]
    fn a_letter_is_a_letter() {
        assert_eq!(letter(0), "A");
        assert_eq!(letter(2), "C");
    }
}
