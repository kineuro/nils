// SPDX-License-Identifier: AGPL-3.0-only

//! Site participation (2026-11-01): how this install takes part, as one row
//! (`participation`), and every report it sent or tried to send
//! (`participation_send`). Default closed: with no row, nothing leaves.
//!
//! The row keeps the site's name, the tiers it shares (1, the counts; 3, a
//! sealed read), the receiver's address, the path of the file its bearer
//! key is read from (never the key), the public half of the signing key
//! pair made when it was enabled (the private half is a file in the
//! registry home, never a column), whether the counts go by themselves,
//! and the newest preview of each tier: a send needs the document it sends
//! previewed by a person first, which the preview's digest proves.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The tiers an install may share: counts, and a sealed read.
pub const TIERS: [i64; 2] = [1, 3];

/// How this install takes part.
#[derive(Debug, Clone, PartialEq)]
pub struct Participation {
    pub id: i64,
    pub site: String,
    pub tiers: BTreeSet<i64>,
    pub receiver: Option<String>,
    /// The path of the file the bearer key is read from; the key never.
    pub key_file: Option<String>,
    /// The public half of the signing key, base64.
    pub public_key: String,
    /// Whether the counts (tier 1) may go by themselves.
    pub auto: bool,
    pub enabled_at: String,
    pub enabled_by: String,
    /// `{"<tier>": {digest, at, draw}}`: the newest preview of each tier.
    pub previewed: Value,
}

impl Participation {
    /// The newest preview of a tier: its digest, when, and the draw it was
    /// of where it was a sealed read.
    pub fn preview_of(&self, tier: i64) -> Option<(String, String, Option<i64>)> {
        let p = &self.previewed[tier.to_string()];
        Some((
            p["digest"].as_str()?.to_string(),
            p["at"].as_str()?.to_string(),
            p["draw"].as_i64(),
        ))
    }

    pub fn as_json(&self) -> Value {
        json!({
            "site": self.site,
            "tiers": self.tiers,
            "receiver": self.receiver,
            "key_file": self.key_file,
            "public_key": self.public_key,
            "auto": self.auto,
            "enabled_at": self.enabled_at,
            "enabled_by": self.enabled_by,
            "previewed": self.previewed,
        })
    }
}

/// What enabling takes.
#[derive(Debug, Clone)]
pub struct Enable<'a> {
    pub site: &'a str,
    pub tiers: &'a BTreeSet<i64>,
    pub receiver: Option<&'a str>,
    pub key_file: Option<&'a str>,
    pub public_key: &'a str,
    pub auto: bool,
    pub who: &'a str,
}

fn select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("participation");
    format!(
        "SELECT id, site, {}, receiver, key_file, public_key, auto, {}, enabled_by, {} FROM {}",
        d.text_of(t.column("tiers").expect("tiers")),
        d.text_of(t.column("enabled_at").expect("enabled_at")),
        d.text_of(t.column("previewed").expect("previewed")),
        store.qualified("participation"),
    )
}

fn of_row(r: &Row) -> Result<Participation, StoreError> {
    let tiers: BTreeSet<i64> = r
        .opt_text(2)?
        .and_then(|t| serde_json::from_str::<Vec<i64>>(t).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    Ok(Participation {
        id: r.int(0)?,
        site: r.text(1)?.to_string(),
        tiers,
        receiver: r.opt_text(3)?.map(str::to_string),
        key_file: r.opt_text(4)?.map(str::to_string),
        public_key: r.text(5)?.to_string(),
        auto: r.int(6)? != 0,
        enabled_at: r.text(7)?.to_string(),
        enabled_by: r.text(8)?.to_string(),
        previewed: r
            .opt_text(9)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_else(|| json!({})),
    })
}

/// The row, where the install takes part.
pub fn get(store: &mut Store) -> Result<Option<Participation>, StoreError> {
    let sql = format!("{} ORDER BY id LIMIT 1", select(store));
    store.query_opt(&sql, &[])?.map(|r| of_row(&r)).transpose()
}

fn tiers_text(tiers: &BTreeSet<i64>) -> String {
    Value::Array(tiers.iter().map(|t| json!(t)).collect()).to_string()
}

/// Turn participation on, or change how the install takes part: the row is
/// written where there is none, else its fields are set (the key pair and
/// the previews are kept).
pub fn enable(store: &mut Store, e: &Enable<'_>) -> Result<Participation, StoreError> {
    match get(store)? {
        Some(p) => {
            store.update_by_id(
                table("participation"),
                &[
                    ("site", Param::from(e.site)),
                    ("tiers", Param::from(tiers_text(e.tiers))),
                    ("receiver", e.receiver.map_or(Param::Null, Param::from)),
                    ("key_file", e.key_file.map_or(Param::Null, Param::from)),
                    ("auto", Param::Int(i64::from(e.auto))),
                ],
                "id",
                p.id,
            )?;
        }
        None => {
            store.insert(
                &Insert::new(
                    table("participation"),
                    &[
                        "site",
                        "tiers",
                        "receiver",
                        "key_file",
                        "public_key",
                        "auto",
                        "enabled_at",
                        "enabled_by",
                        "previewed",
                    ],
                ),
                &[vec![
                    Param::from(e.site),
                    Param::from(tiers_text(e.tiers)),
                    e.receiver.map_or(Param::Null, Param::from),
                    e.key_file.map_or(Param::Null, Param::from),
                    Param::from(e.public_key),
                    Param::Int(i64::from(e.auto)),
                    Param::from(now_iso().as_str()),
                    Param::from(e.who),
                    Param::from("{}"),
                ]],
            )?;
        }
    }
    get(store)?.ok_or_else(|| StoreError::Message("the participation row was not kept".into()))
}

/// Take the tiers away: the row goes with the last of them, and nothing
/// leaves again until a person enables it (the sends stay on record).
pub fn disable(store: &mut Store, tiers: Option<&BTreeSet<i64>>) -> Result<bool, StoreError> {
    let Some(p) = get(store)? else {
        return Ok(false);
    };
    let left: BTreeSet<i64> = match tiers {
        Some(t) => p.tiers.difference(t).copied().collect(),
        None => BTreeSet::new(),
    };
    if left.is_empty() {
        let sql = format!(
            "DELETE FROM {} WHERE id = {}",
            store.qualified("participation"),
            store.dialect().param(1, Type::Int)
        );
        store.execute(&sql, &[Param::Int(p.id)])?;
        return Ok(true);
    }
    store.update_by_id(
        table("participation"),
        &[("tiers", Param::from(tiers_text(&left)))],
        "id",
        p.id,
    )?;
    Ok(false)
}

/// Record that a person saw a tier's document: its content digest, now,
/// and the draw it was of.
pub fn record_preview(
    store: &mut Store,
    tier: i64,
    digest: &str,
    draw: Option<i64>,
) -> Result<String, StoreError> {
    let p = get(store)?.ok_or_else(|| StoreError::Message("this install takes no part".into()))?;
    let now = now_iso();
    let mut previewed = p.previewed.clone();
    previewed[tier.to_string()] = json!({"digest": digest, "at": now, "draw": draw});
    store.update_by_id(
        table("participation"),
        &[("previewed", Param::from(previewed.to_string()))],
        "id",
        p.id,
    )?;
    Ok(now)
}

/// One send, tried or made.
#[derive(Debug, Clone, PartialEq)]
pub struct Send {
    pub id: i64,
    pub tier: i64,
    /// The sha256 of the body as sent.
    pub digest: String,
    pub bytes: i64,
    pub sent_at: String,
    /// The principal, or `auto` for a send the timer made.
    pub sent_by: String,
    pub receiver: String,
    pub receipt: Option<String>,
    pub error: Option<String>,
    pub previewed_at: Option<String>,
    pub draw_id: Option<i64>,
}

impl Send {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "tier": self.tier, "digest": self.digest, "bytes": self.bytes,
            "sent_at": self.sent_at, "sent_by": self.sent_by, "receiver": self.receiver,
            "receipt": self.receipt, "error": self.error, "previewed_at": self.previewed_at,
            "draw": self.draw_id, "ok": self.error.is_none(),
        })
    }
}

/// What a send records.
#[derive(Debug, Clone)]
pub struct Sent<'a> {
    pub tier: i64,
    pub digest: &'a str,
    pub bytes: i64,
    pub by: &'a str,
    pub receiver: &'a str,
    pub receipt: Option<&'a str>,
    pub error: Option<&'a str>,
    pub previewed_at: Option<&'a str>,
    pub draw: Option<i64>,
}

/// Write a send; answers its id.
pub fn record_send(store: &mut Store, s: &Sent<'_>) -> Result<i64, StoreError> {
    let rows = store.insert(
        &Insert::new(
            table("participation_send"),
            &[
                "tier",
                "digest",
                "bytes",
                "sent_at",
                "sent_by",
                "receiver",
                "receipt",
                "error",
                "previewed_at",
                "draw_id",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::Int(s.tier),
            Param::from(s.digest),
            Param::Int(s.bytes),
            Param::from(now_iso().as_str()),
            Param::from(s.by),
            Param::from(s.receiver),
            s.receipt.map_or(Param::Null, Param::from),
            s.error.map_or(Param::Null, Param::from),
            s.previewed_at.map_or(Param::Null, Param::from),
            s.draw.map_or(Param::Null, Param::Int),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| StoreError::Message("the send was not written back".into()))?
        .int(0)
}

fn sends_select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("participation_send");
    format!(
        "SELECT id, tier, digest, bytes, {}, sent_by, receiver, receipt, error, {}, draw_id FROM {}",
        d.text_of(t.column("sent_at").expect("sent_at")),
        d.text_of(t.column("previewed_at").expect("previewed_at")),
        store.qualified("participation_send"),
    )
}

fn send_of(r: &Row) -> Result<Send, StoreError> {
    Ok(Send {
        id: r.int(0)?,
        tier: r.int(1)?,
        digest: r.text(2)?.to_string(),
        bytes: r.int(3)?,
        sent_at: r.text(4)?.to_string(),
        sent_by: r.text(5)?.to_string(),
        receiver: r.text(6)?.to_string(),
        receipt: r.opt_text(7)?.map(str::to_string),
        error: r.opt_text(8)?.map(str::to_string),
        previewed_at: r.opt_text(9)?.map(str::to_string),
        draw_id: r.opt_int(10)?,
    })
}

/// The sends, newest first.
pub fn sends(store: &mut Store, limit: usize) -> Result<Vec<Send>, StoreError> {
    let sql = format!("{} ORDER BY id DESC LIMIT {limit}", sends_select(store));
    store.query(&sql, &[])?.iter().map(send_of).collect()
}

/// The newest send that went through, per tier.
pub fn last_sent(store: &mut Store) -> Result<Vec<Send>, StoreError> {
    let mut out: Vec<Send> = Vec::new();
    for s in sends(store, 1000)? {
        if s.error.is_none() && !out.iter().any(|o| o.tier == s.tier) {
            out.push(s);
        }
    }
    Ok(out)
}

/// Whether a send of a tier was made by itself on a day (`YYYY-MM-DD`,
/// UTC): the cap of one a day on sends by `auto`.
pub fn auto_sent_on(store: &mut Store, tier: i64, day: &str) -> Result<bool, StoreError> {
    Ok(sends(store, 1000)?
        .iter()
        .any(|s| s.tier == tier && s.sent_by == "auto" && s.sent_at.starts_with(day)))
}
