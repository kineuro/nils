// SPDX-License-Identifier: AGPL-3.0-only

//! Pair mode (the post-contrast study, P6): the picture gold of whether
//! contrast was given is read two stacks at a time. A person tells pre from
//! post far more surely by comparing two stacks of one session than by
//! looking at one, and a rerun can carry the same header as the stack
//! before it and still differ in contrast, so no name or header decides it.
//!
//! - **Items.** Each item is a pair of stacks of the same kind from one
//!   session (a pre/post candidate pair, a same-header rerun included). The
//!   item's own row names neither stack; which stack is shown on the left
//!   and which on the right is drawn from the campaign's seed and kept in
//!   `campaign_pair_side`, and the items are read in the seed's order, so
//!   neither the side nor the position follows the order of acquisition.
//! - **Blind.** A pair campaign suggests nothing, and its doors serve the
//!   two stacks' pictures alone: no time, no series name, no header, and no
//!   value the rules gave either stack.
//! - **The answer.** One of five words: the left is post, the right is
//!   post, both are pre, both are post, or can't tell ([`ANSWERS`]).
//! - **What it comes to.** Each answer resolves into one value of the axis
//!   per stack ([`resolve`]): the stack said to be post takes the question's
//!   `post` value (given), one said to be pre its `pre` value (not given),
//!   and can't tell leaves both at can't tell. The close keeps the values
//!   on each item's outcome, and [`values`] reads them for every answer.
//!
//! A stack with no partner (a post-only or a pre-only session) is read in
//! single mode, an ordinary campaign asking its post-contrast axis, made to
//! suggest nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::campaign::{self, CANT_TELL, Campaign, Error};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};

/// The axis a pair question resolves into, and its two values, where the
/// question does not name others.
pub const AXIS: &str = "post_contrast";
pub const POST: &str = "given";
pub const PRE: &str = "not_given";

/// The answers of a pair, in the order of the keys 1 to 5.
pub const LEFT_POST: &str = "left_post";
pub const RIGHT_POST: &str = "right_post";
pub const BOTH_PRE: &str = "both_pre";
pub const BOTH_POST: &str = "both_post";
pub const ANSWERS: [&str; 5] = [LEFT_POST, RIGHT_POST, BOTH_PRE, BOTH_POST, CANT_TELL];

/// The two sides.
pub const LEFT: &str = "left";
pub const RIGHT: &str = "right";

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// Whether a campaign asks of pairs: its question is a pair question.
pub fn is_pair(c: &Campaign) -> bool {
    c.question["kind"] == "pair"
}

/// A draw from a seed and a text, the same every time.
fn drawn(seed: &str, what: &str) -> u64 {
    let d = ring::digest::digest(&ring::digest::SHA256, format!("{seed}:{what}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d.as_ref()[..8]);
    u64::from_be_bytes(b)
}

/// The pairs as they are shown and read: each as (left, right), the side
/// drawn from the seed, and the pairs in the seed's order. The same pairs
/// and seed give the same draw whatever order a pair's two stacks, or the
/// pairs, were named in. A pair of one stack twice, or a pair named twice,
/// is refused.
pub fn draw(pairs: &[(i64, i64)], seed: &str) -> Result<Vec<(i64, i64)>, Error> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(pairs.len());
    for &(a, b) in pairs {
        if a == b {
            return Err(invalid(format!("a pair is two stacks; {a} is named twice")));
        }
        let (lo, hi) = (a.min(b), a.max(b));
        if !seen.insert((lo, hi)) {
            return Err(invalid(format!("the pair {lo} and {hi} is named twice")));
        }
        let shown = if drawn(seed, &format!("side:{lo}:{hi}")).is_multiple_of(2) {
            (lo, hi)
        } else {
            (hi, lo)
        };
        out.push(shown);
    }
    out.sort_by_key(|&(l, r)| drawn(seed, &format!("order:{}:{}", l.min(r), l.max(r))));
    Ok(out)
}

/// Keep which stack each item of a new pair campaign shows on which side:
/// the item at position `i` shows `pairs[i]`. Inside the transaction that
/// wrote the items.
pub fn write_sides(store: &mut Store, campaign: i64, pairs: &[(i64, i64)]) -> Result<(), Error> {
    let sql = format!(
        "SELECT id, position FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    let mut item_at: BTreeMap<i64, i64> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(campaign)])? {
        item_at.insert(r.int(1)?, r.int(0)?);
    }
    let mut rows = Vec::with_capacity(pairs.len() * 2);
    for (i, &(left, right)) in pairs.iter().enumerate() {
        let item = *item_at
            .get(&(i as i64))
            .ok_or_else(|| invalid(format!("the pair at position {i} was not written")))?;
        for (side, stack) in [(LEFT, left), (RIGHT, right)] {
            rows.push(vec![
                Param::Int(campaign),
                Param::Int(item),
                Param::from(side),
                Param::Int(stack),
            ]);
        }
    }
    for chunk in rows.chunks(400) {
        store.insert(
            &Insert::new(
                table("campaign_pair_side"),
                &["campaign_id", "item_id", "side", "stack_id"],
            ),
            chunk,
        )?;
    }
    Ok(())
}

/// The sides of a campaign's items, as (left, right) by item; one item's
/// where `item` is given.
pub fn sides_of(
    store: &mut Store,
    campaign: i64,
    item: Option<i64>,
) -> Result<BTreeMap<i64, (i64, i64)>, StoreError> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT item_id, side, stack_id FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_pair_side"),
        d.param(1, Type::Int)
    );
    let mut params = vec![Param::Int(campaign)];
    if let Some(i) = item {
        sql.push_str(&format!(" AND item_id = {}", d.param(2, Type::Int)));
        params.push(Param::Int(i));
    }
    let mut half: BTreeMap<i64, (Option<i64>, Option<i64>)> = BTreeMap::new();
    for r in store.query(&sql, &params)? {
        let e = half.entry(r.int(0)?).or_default();
        match r.text(1)? {
            LEFT => e.0 = Some(r.int(2)?),
            _ => e.1 = Some(r.int(2)?),
        }
    }
    Ok(half
        .into_iter()
        .filter_map(|(i, (l, r))| Some((i, (l?, r?))))
        .collect())
}

/// Every stack a campaign's pairs show.
pub fn stacks_of(store: &mut Store, campaign: i64) -> Result<Vec<i64>, StoreError> {
    let mut out: Vec<i64> = sides_of(store, campaign, None)?
        .values()
        .flat_map(|&(l, r)| [l, r])
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// What an answer says of each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    Post,
    Pre,
    CantTell,
}

/// The left's state and the right's, from a pair's answer; none for a word
/// that is not one.
pub fn resolve(answer: &str) -> Option<(Said, Said)> {
    Some(match answer.trim() {
        LEFT_POST => (Said::Post, Said::Pre),
        RIGHT_POST => (Said::Pre, Said::Post),
        BOTH_PRE => (Said::Pre, Said::Pre),
        BOTH_POST => (Said::Post, Said::Post),
        CANT_TELL => (Said::CantTell, Said::CantTell),
        _ => return None,
    })
}

/// The value of the axis a state comes to.
fn value_of(s: Said, post: &str, pre: &str) -> String {
    match s {
        Said::Post => post.to_string(),
        Said::Pre => pre.to_string(),
        Said::CantTell => CANT_TELL.to_string(),
    }
}

/// The two stacks' values from a pair's answer, left first: `[{stack,
/// side, axis, value}, ...]`, the value the question's post or pre value,
/// or can't tell.
pub fn resolved(
    answer: &str,
    left: i64,
    right: i64,
    axis: &str,
    post: &str,
    pre: &str,
) -> Option<Value> {
    let (l, r) = resolve(answer)?;
    Some(json!([
        {"stack": left, "side": LEFT, "axis": axis, "value": value_of(l, post, pre)},
        {"stack": right, "side": RIGHT, "axis": axis, "value": value_of(r, post, pre)},
    ]))
}

/// The pair question's axis and values.
fn words(c: &Campaign) -> Result<(String, String, String), Error> {
    match c.question()? {
        campaign::Question::Pair { axis, post, pre } => Ok((axis, post, pre)),
        _ => Err(invalid(format!("campaign {} asks of no pair", c.name))),
    }
}

/// What a person reads a pair from: the item, the stack shown on each side
/// for its pictures, and the answers with their keys. Nothing else: no
/// time, no series name, no header and no value of the axis. None for an
/// item that is not a pair.
pub fn sheet(store: &mut Store, c: &Campaign, item: i64) -> Result<Option<Value>, Error> {
    let Some(&(left, right)) = sides_of(store, c.id, Some(item))?.get(&item) else {
        return Ok(None);
    };
    let keys: serde_json::Map<String, Value> = ANSWERS
        .iter()
        .enumerate()
        .map(|(i, a)| ((i + 1).to_string(), json!(a)))
        .collect();
    Ok(Some(json!({
        "item": item,
        "left": {"stack": left},
        "right": {"stack": right},
        "answers": ANSWERS,
        "keys": keys,
    })))
}

/// Every answer standing now resolved per stack: one row per answer and
/// side, `{item, answer, principal, said, stack, side, axis, value,
/// seconds, unsure, answered_at}`.
pub fn values(store: &mut Store, c: &Campaign) -> Result<Vec<Value>, Error> {
    let (axis, post, pre) = words(c)?;
    let sides = sides_of(store, c.id, None)?;
    let mut rows = Vec::new();
    for a in campaign::current_answers(store, c.id)? {
        let Some(&(left, right)) = sides.get(&a.item_id) else {
            continue;
        };
        let Some(said) = a.value.as_deref() else {
            continue;
        };
        let Some((l, r)) = resolve(said) else {
            continue;
        };
        for (side, stack, s) in [(LEFT, left, l), (RIGHT, right, r)] {
            rows.push(json!({
                "item": a.item_id, "answer": a.id, "principal": a.principal,
                "said": said.trim(), "stack": stack, "side": side, "axis": axis,
                "value": value_of(s, &post, &pre),
                "seconds": a.seconds, "unsure": a.unsure, "answered_at": a.answered_at,
            }));
        }
    }
    Ok(rows)
}

fn quantile(mut v: Vec<f64>, q: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    Some((v[i] * 10.0).round() / 10.0)
}

/// How a pair campaign goes: the items, those answered, each answer's
/// count, and the seconds per pair. `rows` are the answers standing now
/// that the caller may read ([`values`], filtered by the door).
pub fn summary(store: &mut Store, c: &Campaign, rows: &[Value]) -> Result<Value, Error> {
    let items = sides_of(store, c.id, None)?.len();
    let mut answered: BTreeSet<i64> = BTreeSet::new();
    let mut said: BTreeMap<String, usize> = ANSWERS.iter().map(|a| (a.to_string(), 0)).collect();
    let mut seconds: BTreeMap<i64, f64> = BTreeMap::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for r in rows {
        let (Some(item), Some(answer)) = (r["item"].as_i64(), r["answer"].as_i64()) else {
            continue;
        };
        answered.insert(item);
        // two rows an answer, one per side: counted once
        if !seen.insert(answer) {
            continue;
        }
        if let Some(w) = r["said"].as_str() {
            *said.entry(w.to_string()).or_default() += 1;
        }
        if let Some(s) = r["seconds"].as_f64() {
            seconds.insert(answer, s);
        }
    }
    let secs: Vec<f64> = seconds.values().copied().collect();
    Ok(json!({
        "campaign": c.id,
        "items": items,
        "answered": answered.len(),
        "answers": said,
        "seconds": {"median": quantile(secs.clone(), 0.5), "p90": quantile(secs, 0.9)},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_resolves_into_a_value_per_stack() {
        let v = |a: &str| {
            resolved(a, 11, 12, AXIS, POST, PRE)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        r["stack"].as_i64().unwrap(),
                        r["value"].as_str().unwrap().to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let s = |x: &str| x.to_string();
        assert_eq!(v(LEFT_POST), vec![(11, s(POST)), (12, s(PRE))]);
        assert_eq!(v(RIGHT_POST), vec![(11, s(PRE)), (12, s(POST))]);
        assert_eq!(v(BOTH_PRE), vec![(11, s(PRE)), (12, s(PRE))]);
        assert_eq!(v(BOTH_POST), vec![(11, s(POST)), (12, s(POST))]);
        assert_eq!(v(CANT_TELL), vec![(11, s(CANT_TELL)), (12, s(CANT_TELL))]);
        assert!(resolve("given").is_none());
        assert!(resolve("left").is_none());
    }

    #[test]
    fn the_seed_draws_the_sides_and_the_order() {
        let pairs: Vec<(i64, i64)> = (0..40).map(|i| (2 * i + 1, 2 * i + 2)).collect();
        let a = draw(&pairs, "written down first").unwrap();
        // the same seed, the same draw, whatever order the pairs and their
        // stacks were named in
        let turned: Vec<(i64, i64)> = pairs.iter().rev().map(|&(x, y)| (y, x)).collect();
        assert_eq!(draw(&turned, "written down first").unwrap(), a);
        // every pair is there, once
        let mut back: Vec<(i64, i64)> = a.iter().map(|&(l, r)| (l.min(r), l.max(r))).collect();
        back.sort();
        assert_eq!(back, pairs);
        // the earlier stack is not always on the left, and the order is not
        // the stacks' order
        let earlier_left = a.iter().filter(|(l, r)| l < r).count();
        assert!(
            earlier_left > 5 && earlier_left < 35,
            "{earlier_left} of 40"
        );
        assert_ne!(
            a.iter().map(|&(l, r)| l.min(r)).collect::<Vec<_>>(),
            pairs.iter().map(|p| p.0).collect::<Vec<_>>()
        );
        // another seed, another draw
        assert_ne!(draw(&pairs, "another").unwrap(), a);
        // a stack paired with itself, or a pair named twice, is refused
        assert!(draw(&[(3, 3)], "s").is_err());
        assert!(draw(&[(3, 4), (4, 3)], "s").is_err());
    }
}
