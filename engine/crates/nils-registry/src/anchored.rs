// SPDX-License-Identifier: AGPL-3.0-only

//! Anchored reading (the post-contrast study, record 48 of 2026-09-29,
//! night): when two stacks of a pair look alike nobody can tell pre from
//! post, since a lone scan's brightness says nothing. So a candidate stack
//! is read beside a known-pre and a known-post anchor of the same subject,
//! from the same session where there is one, else the closest stack with
//! the same settings, and the answer says which of the two it looks like.
//!
//! - **Items.** Each item is a candidate and its two anchors. The item's own
//!   row names none of the three; which stack is in which panel, and which
//!   is the candidate and which anchor is which, is kept in
//!   `campaign_anchor_panel`. The seed draws the panels' order per item and
//!   the order the items are read in.
//! - **Blind.** An anchored campaign suggests nothing, and its doors serve
//!   the three stacks' pictures alone: no time, no series name, no header,
//!   and no value the rules gave. The anchors are labelled "reference pre"
//!   and "reference post" and nothing else; an anchor from another session
//!   than the candidate's is flagged as such.
//! - **The answer.** One of three words: like the pre, like the post, or
//!   can't tell ([`ANSWERS`]).
//! - **What it comes to.** The answer resolves into one value of the axis
//!   for the candidate alone ([`resolve`]): like the post is the question's
//!   `post` value (given), like the pre its `pre` value (not given), and
//!   can't tell stays can't tell. The anchors are never labelled by it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::campaign::{self, CANT_TELL, Campaign, Error};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};

/// The answers of an anchored item, in the order of the keys 1 to 3.
pub const LIKE_PRE: &str = "like_pre";
pub const LIKE_POST: &str = "like_post";
pub const ANSWERS: [&str; 3] = [LIKE_PRE, LIKE_POST, CANT_TELL];

/// The three roles a panel shows.
pub const CANDIDATE: &str = "candidate";
pub const PRE: &str = "pre";
pub const POST: &str = "post";

/// What a person reads on each panel: the candidate unlabelled but for its
/// name, the anchors as references and nothing more.
pub fn label(role: &str) -> &'static str {
    match role {
        PRE => "reference pre",
        POST => "reference post",
        _ => "candidate",
    }
}

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// Whether a campaign asks of anchored items.
pub fn is_anchored(c: &Campaign) -> bool {
    c.question["kind"] == "anchored"
}

/// A candidate and its two anchors, as named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Named {
    pub candidate: i64,
    pub pre: i64,
    pub post: i64,
}

/// An item as it is shown: the three stacks, the panels' order (a role
/// per panel, left to right), and whether each anchor is of another
/// session than the candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub named: Named,
    pub order: [&'static str; 3],
    pub pre_other_session: bool,
    pub post_other_session: bool,
}

impl Shown {
    /// The stack a role names.
    pub fn stack(&self, role: &str) -> i64 {
        match role {
            PRE => self.named.pre,
            POST => self.named.post,
            _ => self.named.candidate,
        }
    }

    /// The three stacks in the panels' order.
    pub fn stacks(&self) -> [i64; 3] {
        self.order.map(|r| self.stack(r))
    }
}

/// A draw from a seed and a text, the same every time.
fn drawn(seed: &str, what: &str) -> u64 {
    let d = ring::digest::digest(&ring::digest::SHA256, format!("{seed}:{what}").as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d.as_ref()[..8]);
    u64::from_be_bytes(b)
}

/// The six orders of three panels.
const ORDERS: [[&str; 3]; 6] = [
    [CANDIDATE, PRE, POST],
    [CANDIDATE, POST, PRE],
    [PRE, CANDIDATE, POST],
    [POST, CANDIDATE, PRE],
    [PRE, POST, CANDIDATE],
    [POST, PRE, CANDIDATE],
];

/// The items as they are shown and read: each with its panels' order drawn
/// from the seed, and the items in the seed's order. The same items and
/// seed give the same draw whatever order they were named in. An item whose
/// three stacks are not three, and a candidate named twice, are refused.
/// `other` says, per item, whether its pre and its post anchor are of
/// another session than its candidate.
pub fn draw(
    items: &[Named],
    other: &BTreeMap<i64, (bool, bool)>,
    seed: &str,
) -> Result<Vec<Shown>, Error> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(items.len());
    for n in items {
        let three: BTreeSet<i64> = [n.candidate, n.pre, n.post].into_iter().collect();
        if three.len() != 3 {
            return Err(invalid(format!(
                "an anchored item is three stacks: candidate {}, pre {}, post {}",
                n.candidate, n.pre, n.post
            )));
        }
        if !seen.insert(n.candidate) {
            return Err(invalid(format!(
                "stack {} is a candidate twice; each is read once",
                n.candidate
            )));
        }
        let at = drawn(
            seed,
            &format!("panels:{}:{}:{}", n.candidate, n.pre, n.post),
        ) % ORDERS.len() as u64;
        let (pre_other, post_other) = other.get(&n.candidate).copied().unwrap_or_default();
        out.push(Shown {
            named: *n,
            order: ORDERS[at as usize],
            pre_other_session: pre_other,
            post_other_session: post_other,
        });
    }
    out.sort_by_key(|s| drawn(seed, &format!("order:{}", s.named.candidate)));
    Ok(out)
}

/// Keep the panels of each item of a new anchored campaign: the item at
/// position `i` shows `shown[i]`. Inside the transaction that wrote the
/// items.
pub fn write_panels(store: &mut Store, campaign: i64, shown: &[Shown]) -> Result<(), Error> {
    let sql = format!(
        "SELECT id, position FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    let mut item_at: BTreeMap<i64, i64> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(campaign)])? {
        item_at.insert(r.int(1)?, r.int(0)?);
    }
    let mut rows = Vec::with_capacity(shown.len() * 3);
    for (i, s) in shown.iter().enumerate() {
        let item = *item_at
            .get(&(i as i64))
            .ok_or_else(|| invalid(format!("the item at position {i} was not written")))?;
        for (panel, role) in s.order.iter().enumerate() {
            let other = match *role {
                PRE => s.pre_other_session,
                POST => s.post_other_session,
                _ => false,
            };
            rows.push(vec![
                Param::Int(campaign),
                Param::Int(item),
                Param::Int(panel as i64),
                Param::from(*role),
                Param::Int(s.stack(role)),
                Param::Bool(other),
            ]);
        }
    }
    for chunk in rows.chunks(300) {
        store.insert(
            &Insert::new(
                table("campaign_anchor_panel"),
                &[
                    "campaign_id",
                    "item_id",
                    "panel",
                    "role",
                    "stack_id",
                    "other_session",
                ],
            ),
            chunk,
        )?;
    }
    Ok(())
}

/// One panel as it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub panel: i64,
    pub role: String,
    pub stack: i64,
    pub other_session: bool,
}

/// An item's three panels, and which stack plays which role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panels {
    pub panels: Vec<Panel>,
    pub candidate: i64,
    pub pre: i64,
    pub post: i64,
}

/// The panels of a campaign's items, by item; one item's where `item` is
/// given. An item missing a role is left out.
pub fn panels_of(
    store: &mut Store,
    campaign: i64,
    item: Option<i64>,
) -> Result<BTreeMap<i64, Panels>, StoreError> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT item_id, panel, role, stack_id, other_session FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_anchor_panel"),
        d.param(1, Type::Int)
    );
    let mut params = vec![Param::Int(campaign)];
    if let Some(i) = item {
        sql.push_str(&format!(" AND item_id = {}", d.param(2, Type::Int)));
        params.push(Param::Int(i));
    }
    let mut by: BTreeMap<i64, Vec<Panel>> = BTreeMap::new();
    for r in store.query(&sql, &params)? {
        by.entry(r.int(0)?).or_default().push(Panel {
            panel: r.int(1)?,
            role: r.text(2)?.to_string(),
            stack: r.int(3)?,
            other_session: r.opt_int(4)?.unwrap_or(0) != 0,
        });
    }
    let mut out = BTreeMap::new();
    for (i, mut panels) in by {
        panels.sort_by_key(|p| p.panel);
        let of = |role: &str| panels.iter().find(|p| p.role == role).map(|p| p.stack);
        if let (Some(candidate), Some(pre), Some(post)) = (of(CANDIDATE), of(PRE), of(POST)) {
            out.insert(
                i,
                Panels {
                    panels,
                    candidate,
                    pre,
                    post,
                },
            );
        }
    }
    Ok(out)
}

/// Every stack a campaign's items show.
pub fn stacks_of(store: &mut Store, campaign: i64) -> Result<Vec<i64>, StoreError> {
    let mut out: Vec<i64> = panels_of(store, campaign, None)?
        .values()
        .flat_map(|p| [p.candidate, p.pre, p.post])
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// What an answer says of the candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    Post,
    Pre,
    CantTell,
}

/// The candidate's state from an answer; none for a word that is not one.
pub fn resolve(answer: &str) -> Option<Said> {
    Some(match answer.trim() {
        LIKE_POST => Said::Post,
        LIKE_PRE => Said::Pre,
        CANT_TELL => Said::CantTell,
        _ => return None,
    })
}

fn value_of(s: Said, post: &str, pre: &str) -> String {
    match s {
        Said::Post => post.to_string(),
        Said::Pre => pre.to_string(),
        Said::CantTell => CANT_TELL.to_string(),
    }
}

/// The candidate's value from an answer, as the close keeps it: `[{stack,
/// role: candidate, axis, value}]`. The anchors are not in it.
pub fn resolved(answer: &str, candidate: i64, axis: &str, post: &str, pre: &str) -> Option<Value> {
    let s = resolve(answer)?;
    Some(json!([
        {"stack": candidate, "role": CANDIDATE, "axis": axis, "value": value_of(s, post, pre)},
    ]))
}

fn words(c: &Campaign) -> Result<(String, String, String), Error> {
    match c.question()? {
        campaign::Question::Anchored { axis, post, pre } => Ok((axis, post, pre)),
        _ => Err(invalid(format!(
            "campaign {} asks of no anchored item",
            c.name
        ))),
    }
}

/// What a person reads an item from: the three panels in their order, each
/// with the stack for its pictures and its label (candidate, reference pre,
/// reference post), an anchor of another session flagged, and the answers
/// with their keys. Nothing else: no time, no series name, no header and no
/// value of the axis. None for an item that is not anchored.
pub fn sheet(store: &mut Store, c: &Campaign, item: i64) -> Result<Option<Value>, Error> {
    let Some(p) = panels_of(store, c.id, Some(item))?.remove(&item) else {
        return Ok(None);
    };
    let keys: serde_json::Map<String, Value> = ANSWERS
        .iter()
        .enumerate()
        .map(|(i, a)| ((i + 1).to_string(), json!(a)))
        .collect();
    let panels: Vec<Value> = p
        .panels
        .iter()
        .map(|x| {
            let role = match x.role.as_str() {
                PRE => "reference_pre",
                POST => "reference_post",
                _ => CANDIDATE,
            };
            json!({
                "panel": x.panel, "role": role, "label": label(&x.role),
                "stack": x.stack, "other_session": x.other_session,
            })
        })
        .collect();
    Ok(Some(json!({
        "item": item,
        "panels": panels,
        "answers": ANSWERS,
        "keys": keys,
    })))
}

/// Every answer standing now resolved for its candidate: one row per
/// answer, `{item, answer, principal, said, stack, axis, value, pre_anchor,
/// post_anchor, pre_other_session, post_other_session, seconds, unsure,
/// answered_at}`. The anchors are named for provenance and given no value.
pub fn values(store: &mut Store, c: &Campaign) -> Result<Vec<Value>, Error> {
    let (axis, post, pre) = words(c)?;
    let panels = panels_of(store, c.id, None)?;
    let mut rows = Vec::new();
    for a in campaign::current_answers(store, c.id)? {
        let Some(p) = panels.get(&a.item_id) else {
            continue;
        };
        let Some(said) = a.value.as_deref() else {
            continue;
        };
        let Some(s) = resolve(said) else {
            continue;
        };
        let other = |role: &str| p.panels.iter().any(|x| x.role == role && x.other_session);
        rows.push(json!({
            "item": a.item_id, "answer": a.id, "principal": a.principal,
            "said": said.trim(), "stack": p.candidate, "axis": axis,
            "value": value_of(s, &post, &pre),
            "pre_anchor": p.pre, "post_anchor": p.post,
            "pre_other_session": other(PRE), "post_other_session": other(POST),
            "seconds": a.seconds, "unsure": a.unsure, "answered_at": a.answered_at,
        }));
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

/// How an anchored campaign goes: the items, those answered, each answer's
/// count, the items with an anchor of another session, and the seconds per
/// item. `rows` are the answers standing now that the caller may read.
pub fn summary(store: &mut Store, c: &Campaign, rows: &[Value]) -> Result<Value, Error> {
    let panels = panels_of(store, c.id, None)?;
    let other = panels
        .values()
        .filter(|p| p.panels.iter().any(|x| x.other_session))
        .count();
    let mut answered: BTreeSet<i64> = BTreeSet::new();
    let mut said: BTreeMap<String, usize> = ANSWERS.iter().map(|a| (a.to_string(), 0)).collect();
    let mut secs = Vec::new();
    for r in rows {
        if let Some(item) = r["item"].as_i64() {
            answered.insert(item);
        }
        if let Some(w) = r["said"].as_str() {
            *said.entry(w.to_string()).or_default() += 1;
        }
        if let Some(s) = r["seconds"].as_f64() {
            secs.push(s);
        }
    }
    Ok(json!({
        "campaign": c.id,
        "items": panels.len(),
        "answered": answered.len(),
        "answers": said,
        "items_with_an_anchor_of_another_session": other,
        "seconds": {"median": quantile(secs.clone(), 0.5), "p90": quantile(secs, 0.9)},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_resolves_into_the_candidate_alone() {
        let v = |a: &str| {
            let r = resolved(a, 11, "post_contrast", "given", "not_given").unwrap();
            let rows = r.as_array().unwrap().clone();
            assert_eq!(rows.len(), 1, "{r}");
            assert_eq!(rows[0]["stack"], 11);
            assert_eq!(rows[0]["role"], CANDIDATE);
            rows[0]["value"].as_str().unwrap().to_string()
        };
        assert_eq!(v(LIKE_POST), "given");
        assert_eq!(v(LIKE_PRE), "not_given");
        assert_eq!(v(CANT_TELL), CANT_TELL);
        assert!(resolve("given").is_none());
        assert!(resolve("left_post").is_none());
    }

    #[test]
    fn the_seed_draws_the_panels_and_the_order() {
        let items: Vec<Named> = (0..60)
            .map(|i| Named {
                candidate: 3 * i + 1,
                pre: 3 * i + 2,
                post: 3 * i + 3,
            })
            .collect();
        let none = BTreeMap::new();
        let a = draw(&items, &none, "written down first").unwrap();
        let back: Vec<Named> = items.iter().rev().copied().collect();
        assert_eq!(draw(&back, &none, "written down first").unwrap(), a);
        assert_eq!(a.len(), 60);
        // every order of the three panels comes up, and the candidate is
        // in each place
        let orders: BTreeSet<[&str; 3]> = a.iter().map(|s| s.order).collect();
        assert_eq!(orders.len(), 6, "{orders:?}");
        for place in 0..3 {
            let n = a.iter().filter(|s| s.order[place] == CANDIDATE).count();
            assert!(n > 5, "the candidate in panel {place} {n} times of 60");
        }
        // the order read is not the order named
        assert_ne!(
            a.iter().map(|s| s.named.candidate).collect::<Vec<_>>(),
            items.iter().map(|n| n.candidate).collect::<Vec<_>>()
        );
        assert_ne!(draw(&items, &none, "another").unwrap(), a);
        // three stacks, each a candidate once
        let one = |c, p, q| Named {
            candidate: c,
            pre: p,
            post: q,
        };
        assert!(draw(&[one(1, 1, 2)], &none, "s").is_err());
        assert!(draw(&[one(1, 2, 2)], &none, "s").is_err());
        assert!(draw(&[one(1, 2, 3), one(1, 4, 5)], &none, "s").is_err());
        // an anchor may serve two candidates
        assert!(draw(&[one(1, 2, 3), one(4, 2, 3)], &none, "s").is_ok());
        // the flags ride with the item
        let flags: BTreeMap<i64, (bool, bool)> = [(1, (false, true))].into_iter().collect();
        let s = draw(&[one(1, 2, 3)], &flags, "s").unwrap();
        assert!(!s[0].pre_other_session && s[0].post_other_session);
        assert_eq!(
            s[0].stacks().iter().copied().collect::<BTreeSet<_>>().len(),
            3
        );
    }
}
