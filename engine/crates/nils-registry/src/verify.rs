// SPDX-License-Identifier: AGPL-3.0-only

//! Verify mode in campaigns (record 58 P1; Wave 7b spec §12).
//!
//! A fifth campaign purpose, `verify`, beside `decide`, `train`, `measure`
//! and `audit`, which keep their behaviour exactly (spec §4.5). A verify
//! campaign's items carry a suggestion (`suggest: rules` or `imported`) and
//! the reader is shown it first, filled in on every asked axis: the one
//! exception to blind first (record 55 D6). What makes it safe is the
//! **planted values**: a seeded share of the items ([`DEFAULT_SHARE`])
//! carries one axis whose proposal is replaced by a plausible wrong value,
//! kept legal with the item's other axes under the pack's constraints. The
//! wrong value comes from a confusion table the campaign is made with
//! (`planted.from`: true value, wrong value, count; no stack identities):
//! the commonest wrong value for the true value is planted, and where no
//! table is given, or the table names none that is legal, a random other
//! legal value drawn from the seed is. The seed and the planted list are
//! recorded with the campaign, returned by no door before the close, and
//! opened at the close ([`open`]). The reader is told that planted values
//! exist, not where.
//!
//! The planted items are drawn when the campaign is made ([`plant`]):
//! exactly `round(share * items)` of them, the first of a shuffle the seed
//! draws. Their axis and wrong value are fixed when the item's proposal is
//! first served ([`shown`]), since an imported suggestion arrives after the
//! campaign is made; the same seed, item and proposal always fix the same
//! value. An item whose proposal was never served keeps them null.
//!
//! The **catch rate** ([`catch`]) is the share of planted items whose
//! planted axis the reader did not accept, overall and per reader, reported
//! against the registered bar of 90 % ([`BAR`]): a campaign under it is
//! marked `below_bar` and its result never stands alone.
//!
//! An answer given in verify is recorded `blind: false` and never becomes a
//! training label ([`crate::blind::campaign_answer_blind`],
//! [`crate::blind::never_trains`]); the decisions its close writes are
//! marked the same ([`crate::blind::mark_decisions_not_blind`]). A planted
//! value is known to be wrong, so a close into decisions never writes one:
//! the planted axis of an item whose settled answer kept the plant is left
//! out (`kept_plant`), the item's other axes are written as for any item,
//! and an item whose reader corrected the plant is written whole.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::campaign::{self, CANT_TELL, Campaign, Error, NOT_ASKED, Question};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};

/// The purpose's name.
pub const PURPOSE: &str = "verify";

/// The share of items planted where the campaign says none (spec §20).
pub const DEFAULT_SHARE: f64 = 0.10;

/// The registered catch-rate bar (record 58, question 11): a campaign under
/// it is reported as such and its result never stands alone.
pub const BAR: f64 = 0.90;

/// Whether a purpose is verify.
pub fn is_verify(purpose: &str) -> bool {
    purpose == PURPOSE
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// One row of a confusion table: how often a true value was read as a
/// wrong one, on an axis where the row names one.
#[derive(Debug, Clone, PartialEq)]
pub struct Confusion {
    pub axis: Option<String>,
    pub true_value: String,
    pub wrong_value: String,
    pub count: i64,
}

/// How a verify campaign plants, as it was made (`planted: {share, seed,
/// from}`), and when its list was opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Planting {
    pub share: f64,
    pub seed: String,
    pub from: Option<Vec<Confusion>>,
    pub opened_at: Option<String>,
}

const ROW_KEYS: [&str; 4] = ["axis", "true", "wrong", "count"];

impl Planting {
    /// Read `planted` as a campaign is made with it: `share` (the default
    /// where none is given), `seed` (a fresh one where none is given) and
    /// `from`, rows of `{true, wrong, count}` with an `axis` beside them
    /// where two axes could be confused; a row that names anything else
    /// is refused, since the table carries no stack identity.
    pub fn parse(v: &Value) -> Result<Planting, Error> {
        if !v.is_null() && !v.is_object() {
            return Err(invalid("planted: {share, seed, from}"));
        }
        let share = match &v["share"] {
            Value::Null => DEFAULT_SHARE,
            Value::Number(n) => n
                .as_f64()
                .filter(|s| (0.0..=1.0).contains(s))
                .ok_or_else(|| invalid("planted.share: the share of items planted, from 0 to 1"))?,
            _ => {
                return Err(invalid(
                    "planted.share: the share of items planted, from 0 to 1",
                ));
            }
        };
        let seed = match &v["seed"] {
            Value::Null => uuid::Uuid::new_v4().to_string(),
            Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Err(invalid("planted.seed: a string the draw is made from")),
        };
        let from = match &v["from"] {
            Value::Null => None,
            Value::Array(rows) => {
                let mut out = Vec::with_capacity(rows.len());
                for r in rows {
                    let Some(m) = r.as_object() else {
                        return Err(invalid("planted.from: rows of {true, wrong, count}"));
                    };
                    if let Some(k) = m.keys().find(|k| !ROW_KEYS.contains(&k.as_str())) {
                        return Err(invalid(format!(
                            "planted.from: a row is {{axis?, true, wrong, count}} and carries no stack identity, not {k}"
                        )));
                    }
                    let word = |key: &str| {
                        r[key]
                            .as_str()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    };
                    let (Some(true_value), Some(wrong_value)) = (word("true"), word("wrong"))
                    else {
                        return Err(invalid(
                            "planted.from: every row names a true value and a wrong value",
                        ));
                    };
                    if true_value == wrong_value {
                        return Err(invalid(format!(
                            "planted.from: {true_value} is read as itself, which is no confusion"
                        )));
                    }
                    let count = match &r["count"] {
                        Value::Null => 1,
                        Value::Number(n) if n.as_i64().is_some_and(|c| c >= 1) => {
                            n.as_i64().unwrap_or(1)
                        }
                        _ => {
                            return Err(invalid(
                                "planted.from: a row's count is a whole number of one or more",
                            ));
                        }
                    };
                    out.push(Confusion {
                        axis: word("axis"),
                        true_value,
                        wrong_value,
                        count,
                    });
                }
                Some(out)
            }
            _ => {
                return Err(invalid(
                    "planted.from: rows of {true, wrong, count}, or nothing",
                ));
            }
        };
        Ok(Planting {
            share,
            seed,
            from,
            opened_at: None,
        })
    }

    /// As kept on the campaign.
    pub fn as_json(&self) -> Value {
        json!({
            "share": self.share,
            "seed": self.seed,
            "from": self.from.as_ref().map(|rows| rows.iter().map(|c| json!({
                "axis": c.axis, "true": c.true_value, "wrong": c.wrong_value, "count": c.count,
            })).collect::<Vec<_>>()),
            "opened_at": self.opened_at,
        })
    }

    fn of_json(v: &Value) -> Option<Planting> {
        Some(Planting {
            share: v["share"].as_f64()?,
            seed: v["seed"].as_str()?.to_string(),
            from: v["from"].as_array().map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(Confusion {
                            axis: r["axis"].as_str().map(str::to_string),
                            true_value: r["true"].as_str()?.to_string(),
                            wrong_value: r["wrong"].as_str()?.to_string(),
                            count: r["count"].as_i64().unwrap_or(1),
                        })
                    })
                    .collect()
            }),
            opened_at: v["opened_at"].as_str().map(str::to_string),
        })
    }
}

/// How a campaign plants; none for a campaign that plants nothing.
pub fn planting(store: &mut Store, campaign: i64) -> Result<Option<Planting>, StoreError> {
    let d = store.dialect();
    let t = table("campaign");
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        d.text_of(t.column("planted").expect("planted")),
        store.qualified("campaign"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(campaign)])?
        .and_then(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
        })
        .as_ref()
        .and_then(Planting::of_json))
}

fn write_planting(store: &mut Store, campaign: i64, p: &Planting) -> Result<(), StoreError> {
    store.update_by_id(
        table("campaign"),
        &[("planted", Param::from(p.as_json().to_string()))],
        "id",
        campaign,
    )?;
    Ok(())
}

fn hash(seed: &str, what: &str) -> u64 {
    let d = ring::digest::digest(
        &ring::digest::SHA256,
        format!("{seed}:planted:{what}").as_bytes(),
    );
    let mut b = [0u8; 8];
    b.copy_from_slice(&d.as_ref()[..8]);
    u64::from_be_bytes(b)
}

/// A coin the seed draws for one thing, such as the order two proposals
/// are shown in on one item.
pub fn coin(seed: &str, what: &str) -> bool {
    hash(seed, what) % 2 == 1
}

/// The items planted among these: exactly `round(share * n)` of them, the
/// first of a shuffle the seed draws, in id order. The same seed and items
/// always draw the same.
pub fn draw(seed: &str, items: &[i64], share: f64) -> Vec<i64> {
    let n = (items.len() as f64 * share.clamp(0.0, 1.0)).round() as usize;
    let mut keyed: Vec<(u64, i64)> = items
        .iter()
        .map(|i| (hash(seed, &format!("item:{i}")), *i))
        .collect();
    keyed.sort_unstable();
    let mut out: Vec<i64> = keyed.into_iter().take(n).map(|(_, i)| i).collect();
    out.sort_unstable();
    out
}

/// Record the planting of a campaign and draw its planted items (spec
/// §12.3). Answers how many were planted.
pub fn plant(store: &mut Store, campaign: i64, p: &Planting) -> Result<usize, Error> {
    write_planting(store, campaign, p)?;
    let ids: Vec<i64> = campaign::items(store, campaign)?
        .iter()
        .map(|i| i.id)
        .collect();
    let chosen = draw(&p.seed, &ids, p.share);
    if chosen.is_empty() {
        return Ok(0);
    }
    let rows: Vec<Vec<Param>> = chosen
        .iter()
        .map(|id| vec![Param::Int(campaign), Param::Int(*id)])
        .collect();
    store.insert(
        &Insert::new(table("campaign_planted"), &["campaign_id", "item_id"]),
        &rows,
    )?;
    Ok(chosen.len())
}

/// A planted item as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Planted {
    pub id: i64,
    pub campaign_id: i64,
    pub item_id: i64,
    /// The axis planted, the proposal's value on it and the wrong value
    /// shown in its place; null until the item's proposal is first served,
    /// and null for good where no axis had a legal other value.
    pub axis: Option<String>,
    pub true_value: Option<String>,
    pub wrong_value: Option<String>,
    pub served_at: Option<String>,
}

impl Planted {
    /// Whether a wrong value was shown for the item.
    pub fn fixed(&self) -> bool {
        self.axis.is_some() && self.wrong_value.is_some()
    }

    /// As the opened list names it.
    pub fn as_json(&self) -> Value {
        json!({
            "item": self.item_id, "axis": self.axis, "true": self.true_value,
            "wrong": self.wrong_value, "served_at": self.served_at,
        })
    }
}

const COLUMNS: [&str; 7] = [
    "id",
    "campaign_id",
    "item_id",
    "axis",
    "true_value",
    "wrong_value",
    "served_at",
];

fn select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("campaign_planted");
    COLUMNS
        .iter()
        .map(|c| {
            d.text_of(
                t.column(c)
                    .unwrap_or_else(|| panic!("campaign_planted.{c}")),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn planted_of(r: &Row) -> Result<Planted, StoreError> {
    Ok(Planted {
        id: r.int(0)?,
        campaign_id: r.int(1)?,
        item_id: r.int(2)?,
        axis: r.opt_text(3)?.map(str::to_string),
        true_value: r.opt_text(4)?.map(str::to_string),
        wrong_value: r.opt_text(5)?.map(str::to_string),
        served_at: r.opt_text(6)?.map(str::to_string),
    })
}

/// The planted items of a campaign, in item order.
pub fn planted(store: &mut Store, campaign: i64) -> Result<Vec<Planted>, StoreError> {
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} ORDER BY item_id",
        select(store),
        store.qualified("campaign_planted"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(planted_of)
        .collect()
}

/// One item's plant, if it is planted.
pub fn planted_item(
    store: &mut Store,
    campaign: i64,
    item: i64,
) -> Result<Option<Planted>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} AND item_id = {}",
        select(store),
        store.qualified("campaign_planted"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(campaign), Param::Int(item)])?
        .as_ref()
        .map(planted_of)
        .transpose()
}

fn words(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The plant chosen for an item from its proposal: the axis, the
/// proposal's value on it and the wrong value; none where no asked axis
/// has a legal other value. The axes are tried in an order the seed draws
/// per item; on each, the table's commonest wrong value for the true value
/// where it is a legal other value, else a legal other value the seed
/// draws ([`pick`]).
fn choose(
    question: &Question,
    p: &Planting,
    item: i64,
    proposal: &str,
) -> Option<(String, String, String)> {
    match question {
        Question::Axis { axis, values } => {
            let true_value = proposal.trim();
            if true_value.is_empty() || true_value == CANT_TELL || true_value == NOT_ASKED {
                return None;
            }
            let others: Vec<String> = values
                .iter()
                .filter(|v| *v != true_value && *v != CANT_TELL && *v != NOT_ASKED)
                .cloned()
                .collect();
            let wrong = pick(p, item, axis, true_value, &others, |_| true)?;
            Some((axis.clone(), true_value.to_string(), wrong))
        }
        Question::Axes {
            axes, constraints, ..
        } => {
            let joint = campaign::joint_of(axes, constraints, proposal).ok()?;
            let mut order: Vec<&String> = axes.iter().collect();
            order.sort_by_key(|a| hash(&p.seed, &format!("axis:{item}:{a}")));
            for axis in order {
                let Some(held) = joint.get(axis) else {
                    continue;
                };
                if held.is_empty() || campaign::cant_tell(held) || campaign::not_asked(held) {
                    continue;
                }
                let true_value = held.join(",");
                let others: Vec<String> = words(&constraints["values"][axis.as_str()])
                    .into_iter()
                    .filter(|v| !held.contains(v) && v != CANT_TELL && v != NOT_ASKED)
                    .collect();
                let legal_with = |w: &String| {
                    let mut j = joint.clone();
                    j.insert(axis.clone(), vec![w.clone()]);
                    campaign::legal(constraints, &j).is_ok()
                };
                if let Some(wrong) = pick(p, item, axis, &true_value, &others, legal_with) {
                    return Some((axis.clone(), true_value, wrong));
                }
            }
            None
        }
        _ => None,
    }
}

/// The wrong value for a true value on an axis, among the other values that
/// are legal with the item's other axes: the table's commonest wrong value
/// for the true value (ties by name), on this axis where the row names
/// one; where the table has none that is legal, or there is no table, one
/// the seed draws. None where no other value is legal.
fn pick(
    p: &Planting,
    item: i64,
    axis: &str,
    true_value: &str,
    others: &[String],
    legal: impl Fn(&String) -> bool,
) -> Option<String> {
    let legal_others: Vec<&String> = others.iter().filter(|w| legal(w)).collect();
    if legal_others.is_empty() {
        return None;
    }
    if let Some(rows) = &p.from {
        let mut fitting: Vec<&Confusion> = rows
            .iter()
            .filter(|c| {
                c.true_value == true_value
                    && c.axis.as_deref().is_none_or(|a| a == axis)
                    && legal_others.iter().any(|w| **w == c.wrong_value)
            })
            .collect();
        fitting.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.wrong_value.cmp(&b.wrong_value))
        });
        if let Some(c) = fitting.first() {
            return Some(c.wrong_value.clone());
        }
    }
    let i = (hash(&p.seed, &format!("wrong:{item}:{axis}")) % legal_others.len() as u64) as usize;
    Some(legal_others[i].clone())
}

/// The proposal with the planted axis's value replaced, as an answer to the
/// question keeps it.
fn replaced(question: &Question, proposal: &str, axis: &str, wrong: &str) -> Option<String> {
    match question {
        Question::Axis { .. } => Some(wrong.to_string()),
        Question::Axes {
            axes, constraints, ..
        } => {
            let mut joint = campaign::joint_of(axes, constraints, proposal).ok()?;
            joint.insert(axis.to_string(), vec![wrong.to_string()]);
            Some(campaign::canonical_joint(constraints, &joint))
        }
        _ => None,
    }
}

/// The proposal of an item as a verify reader is shown it (spec §12.3): on
/// a planted item the planted axis says the wrong value in place of the
/// proposal's; any other item's proposal is itself. The plant is fixed the
/// first time the proposal is served, from the campaign's seed and the
/// table it was made with, and kept: a second serve reads what the first
/// wrote.
pub fn shown(
    store: &mut Store,
    c: &Campaign,
    item: i64,
    proposal: &str,
    now: &str,
) -> Result<String, Error> {
    let Some(row) = planted_item(store, c.id, item)? else {
        return Ok(proposal.to_string());
    };
    let question = c.question()?;
    let row = if row.served_at.is_some() {
        row
    } else {
        let Some(p) = planting(store, c.id)? else {
            return Ok(proposal.to_string());
        };
        let fix = choose(&question, &p, item, proposal);
        let d = store.dialect();
        store.execute(
            &format!(
                "UPDATE {} SET axis = {}, true_value = {}, wrong_value = {}, served_at = {} \
                 WHERE id = {} AND served_at IS NULL",
                store.qualified("campaign_planted"),
                d.param(1, Type::Text),
                d.param(2, Type::Text),
                d.param(3, Type::Text),
                d.param(4, Type::Timestamp),
                d.param(5, Type::Int)
            ),
            &[
                Param::from(fix.as_ref().map(|f| f.0.as_str())),
                Param::from(fix.as_ref().map(|f| f.1.as_str())),
                Param::from(fix.as_ref().map(|f| f.2.as_str())),
                Param::from(now),
                Param::Int(row.id),
            ],
        )?;
        planted_item(store, c.id, item)?.unwrap_or(row)
    };
    Ok(match (&row.axis, &row.wrong_value) {
        (Some(axis), Some(wrong)) => {
            replaced(&question, proposal, axis, wrong).unwrap_or_else(|| proposal.to_string())
        }
        _ => proposal.to_string(),
    })
}

/// Whether an answer accepted the planted value: its value on the planted
/// axis holds the wrong value.
fn accepted(question: &Question, value: Option<&str>, row: &Planted) -> bool {
    let (Some(axis), Some(wrong), Some(value)) = (&row.axis, &row.wrong_value, value) else {
        return false;
    };
    match question {
        Question::Axis { .. } => value.trim() == wrong,
        Question::Axes {
            axes, constraints, ..
        } => campaign::stored_joint_of(axes, constraints, value)
            .ok()
            .and_then(|j| j.get(axis).cloned())
            .is_some_and(|held| held.iter().any(|v| v == wrong)),
        _ => false,
    }
}

/// The planted axis a close into decisions leaves out of an item (spec
/// §12.4): that of a planted item whose settled answer kept the planted
/// wrong value. A planted value is known to be wrong, so it is never
/// written as a decision, whoever settled the item; the item's other axes
/// are written as for any item. None for an item that is not planted, whose
/// plant was never shown, or whose settled answer corrected the plant (a
/// caught item is written whole).
pub(crate) fn kept_plant(
    store: &mut Store,
    campaign: i64,
    question: &Question,
    item: i64,
    settled: Option<&str>,
) -> Result<Option<String>, StoreError> {
    let Some(row) = planted_item(store, campaign, item)? else {
        return Ok(None);
    };
    Ok(kept_plant_of(question, &row, settled))
}

/// [`kept_plant`] over a plant already read, for a caller that holds every
/// plant of a campaign (one query, not one per item).
pub(crate) fn kept_plant_of(
    question: &Question,
    row: &Planted,
    settled: Option<&str>,
) -> Option<String> {
    if accepted(question, settled, row) {
        row.axis.clone()
    } else {
        None
    }
}

/// The catch (spec §12.5), over the raters' answers now: `planted`, the
/// planted items a rater answered (an item two raters answered counts for
/// each), `caught`, those whose planted axis the rater did not accept,
/// `rate` their share against the `bar`, `below_bar` whether the rate is
/// under it, `items` every item drawn, `shown` those whose wrong value was
/// shown (a drawn item whose proposal was never served planted nothing) and
/// `unread` those shown that no rater has answered, and the same per rater
/// by principal.
pub fn catch(store: &mut Store, c: &Campaign) -> Result<Value, Error> {
    let question = c.question()?;
    let rows = planted(store, c.id)?;
    let fixed: BTreeMap<i64, &Planted> = rows
        .iter()
        .filter(|r| r.fixed())
        .map(|r| (r.item_id, r))
        .collect();
    let answers = campaign::current_answers(store, c.id)?;
    let mut per: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut read: BTreeSet<i64> = BTreeSet::new();
    let (mut planted_n, mut caught_n) = (0usize, 0usize);
    for a in answers.iter().filter(|a| a.role == "rater") {
        let Some(row) = fixed.get(&a.item_id) else {
            continue;
        };
        let caught = !accepted(&question, a.value.as_deref(), row);
        read.insert(a.item_id);
        planted_n += 1;
        caught_n += usize::from(caught);
        let e = per.entry(a.principal.clone()).or_default();
        e.0 += 1;
        e.1 += usize::from(caught);
    }
    let rate =
        |caught: usize, planted: usize| (planted > 0).then(|| caught as f64 / planted as f64);
    let whole = rate(caught_n, planted_n);
    Ok(json!({
        "planted": planted_n,
        "caught": caught_n,
        "rate": whole,
        "bar": BAR,
        "below_bar": whole.is_some_and(|r| r < BAR),
        "items": rows.len(),
        "shown": fixed.len(),
        "unread": fixed.len() - read.len(),
        "per_rater": per.into_iter().map(|(p, (n, caught))| json!({
            "rater": p, "planted": n, "caught": caught, "rate": rate(caught, n),
        })).collect::<Vec<_>>(),
    }))
}

/// Open the planted list at the close: from here on the doors return the
/// seed and the list.
pub fn open(store: &mut Store, campaign: i64, now: &str) -> Result<(), StoreError> {
    let Some(mut p) = planting(store, campaign)? else {
        return Ok(());
    };
    if p.opened_at.is_some() {
        return Ok(());
    }
    p.opened_at = Some(now.to_string());
    write_planting(store, campaign, &p)
}

/// `planted` as `GET /api/campaigns/{id}` reports it (spec §12.7): `{share,
/// count, opened}`, and once opened `seed`, `from` and the `list`, each
/// item's axis, true value and wrong value. None for a campaign that
/// plants nothing.
pub fn summary(store: &mut Store, campaign: i64) -> Result<Option<Value>, StoreError> {
    let Some(p) = planting(store, campaign)? else {
        return Ok(None);
    };
    let rows = planted(store, campaign)?;
    let mut v = json!({"share": p.share, "count": rows.len(), "opened": p.opened_at.is_some()});
    if p.opened_at.is_some() {
        v["opened_at"] = json!(p.opened_at);
        v["seed"] = json!(p.seed);
        v["from"] = p.as_json()["from"].clone();
        v["list"] = json!(rows.iter().map(Planted::as_json).collect::<Vec<_>>());
    }
    Ok(Some(v))
}

/// The sha256 of the planted list, canonical: each item's id, axis, true
/// value and wrong value, in item order.
pub fn list_digest(rows: &[Planted]) -> String {
    let canon: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({"item": r.item_id, "axis": r.axis, "true": r.true_value, "wrong": r.wrong_value})
        })
        .collect();
    hex::encode(
        ring::digest::digest(
            &ring::digest::SHA256,
            Value::Array(canon).to_string().as_bytes(),
        )
        .as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes() -> Question {
        Question::parse(&json!({
            "kind": "axes",
            "axes": ["base", "technique", "modifier"],
            "constraints": {
                "pack": "test@1",
                "values": {
                    "base": ["T1w", "T2w", "PDw"],
                    "technique": ["MPRAGE", "TSE", "SE"],
                    "modifier": ["FLAIR", "STIR", "FatSat"]
                },
                "multi": ["modifier"],
                "groups": {"modifier": {"IR_CONTRAST": ["FLAIR", "STIR"]}},
                "implications": [{
                    "rule": "base/technique:MPRAGE",
                    "when": {"axis": "technique", "is": "MPRAGE"},
                    "then": [{"axis": "base", "value": "T1w"}]
                }]
            }
        }))
        .unwrap()
    }

    fn planting(from: Option<Vec<Confusion>>) -> Planting {
        Planting {
            share: 0.1,
            seed: "paper-2026".into(),
            from,
            opened_at: None,
        }
    }

    #[test]
    fn the_draw_takes_exactly_the_share_and_always_the_same() {
        let items: Vec<i64> = (1..=200).collect();
        let a = draw("s", &items, 0.1);
        assert_eq!(a.len(), 20);
        assert_eq!(a, draw("s", &items, 0.1));
        assert_ne!(a, draw("t", &items, 0.1));
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        assert!(draw("s", &items, 0.0).is_empty());
        assert_eq!(draw("s", &items, 1.0).len(), 200);
        assert_eq!(draw("s", &[], 0.5).len(), 0);
    }

    #[test]
    fn the_table_s_commonest_wrong_value_is_planted_where_it_is_legal() {
        let table = vec![
            Confusion {
                axis: None,
                true_value: "T2w".into(),
                wrong_value: "PDw".into(),
                count: 7,
            },
            Confusion {
                axis: None,
                true_value: "T2w".into(),
                wrong_value: "T1w".into(),
                count: 2,
            },
            Confusion {
                axis: Some("technique".into()),
                true_value: "TSE".into(),
                wrong_value: "SE".into(),
                count: 9,
            },
        ];
        let p = planting(Some(table));
        let proposal = r#"{"base":"T2w","technique":"TSE","modifier":["FLAIR"]}"#;
        let (axis, true_value, wrong) = choose(&axes(), &p, 1, proposal).expect("a plant");
        match axis.as_str() {
            "base" => {
                assert_eq!(true_value, "T2w");
                assert_eq!(wrong, "PDw");
            }
            "technique" => {
                assert_eq!(true_value, "TSE");
                assert_eq!(wrong, "SE");
            }
            "modifier" => {
                assert_eq!(true_value, "FLAIR");
                // no row: a legal other value, never STIR beside FLAIR's group
                assert_eq!(wrong, "FatSat");
            }
            other => panic!("{other}"),
        }
        // the same item, seed and proposal plant the same
        assert_eq!(
            choose(&axes(), &p, 1, proposal),
            choose(&axes(), &p, 1, proposal)
        );
        // the plant keeps the joint legal: MPRAGE forces T1w, so a T1w
        // MPRAGE is never planted T2w on the base
        let mprage = r#"{"base":"T1w","technique":"MPRAGE","modifier":[]}"#;
        for item in 1..40 {
            if let Some((axis, _, wrong)) = choose(&axes(), &p, item, mprage) {
                let shown = replaced(&axes(), mprage, &axis, &wrong).unwrap();
                let Question::Axes {
                    axes: a,
                    constraints,
                    ..
                } = axes()
                else {
                    unreachable!()
                };
                let j = campaign::joint_of(&a, &constraints, &shown).unwrap();
                assert!(campaign::legal(&constraints, &j).is_ok(), "{shown}");
            }
        }
    }

    #[test]
    fn without_a_table_a_random_other_legal_value_is_planted() {
        let p = planting(None);
        let q = Question::parse(&json!({
            "kind": "axis", "axis": "body_part", "values": ["brain", "spine", "neck"]
        }))
        .unwrap();
        let mut seen = BTreeSet::new();
        for item in 1..60 {
            let (axis, t, w) = choose(&q, &p, item, "brain").unwrap();
            assert_eq!(axis, "body_part");
            assert_eq!(t, "brain");
            assert_ne!(w, "brain");
            seen.insert(w);
        }
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(choose(&q, &p, 1, CANT_TELL).is_none());
        let one =
            Question::parse(&json!({"kind": "axis", "axis": "x", "values": ["only"]})).unwrap();
        assert!(choose(&one, &p, 1, "only").is_none());
    }

    #[test]
    fn an_answer_that_keeps_the_planted_value_is_not_a_catch() {
        let row = Planted {
            id: 1,
            campaign_id: 1,
            item_id: 1,
            axis: Some("base".into()),
            true_value: Some("T2w".into()),
            wrong_value: Some("PDw".into()),
            served_at: Some("2026-10-18T10:00:00Z".into()),
        };
        let q = axes();
        assert!(accepted(
            &q,
            Some(r#"{"base":"PDw","technique":"TSE","modifier":["FLAIR"]}"#),
            &row
        ));
        assert!(!accepted(
            &q,
            Some(r#"{"base":"T2w","technique":"TSE","modifier":["FLAIR"]}"#),
            &row
        ));
        assert!(!accepted(
            &q,
            Some(r#"{"base":"cant_tell","technique":"TSE","modifier":["FLAIR"]}"#),
            &row
        ));
        assert!(!accepted(&q, None, &row));
        let unfixed = Planted {
            axis: None,
            wrong_value: None,
            ..row
        };
        assert!(!accepted(&q, Some("anything"), &unfixed));
    }

    #[test]
    fn a_planting_is_read_from_the_door_s_words_and_refuses_an_identity() {
        let p = Planting::parse(&json!({
            "share": 0.2, "seed": "s",
            "from": [{"true": "T2w", "wrong": "PDw", "count": 3}, {"axis": "technique", "true": "TSE", "wrong": "SE"}]
        }))
        .unwrap();
        assert_eq!(p.share, 0.2);
        assert_eq!(p.from.as_ref().map(Vec::len), Some(2));
        assert_eq!(p.from.as_ref().unwrap()[1].count, 1);
        let fresh = Planting::parse(&Value::Null).unwrap();
        assert_eq!(fresh.share, DEFAULT_SHARE);
        assert!(!fresh.seed.is_empty());
        assert!(fresh.from.is_none());
        for bad in [
            json!({"share": 2}),
            json!({"seed": 5}),
            json!({"from": [{"true": "a", "wrong": "a"}]}),
            json!({"from": [{"true": "a", "wrong": "b", "stack": 7}]}),
            json!({"from": [{"true": "a", "wrong": "b", "count": 0}]}),
            json!({"from": "table"}),
            json!("planted"),
        ] {
            assert!(Planting::parse(&bad).is_err(), "{bad}");
        }
        let kept = Planting::of_json(&p.as_json()).unwrap();
        assert_eq!(kept, p);
    }
}
