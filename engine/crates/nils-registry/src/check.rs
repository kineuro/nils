// SPDX-License-Identifier: AGPL-3.0-only

//! Job type 3, check a result (record 55 H3; the build spec of 2026-10-11,
//! sections 5 to 7, the campaign's half). A check is a campaign whose
//! question is "is this result right?": its item is one result (a mask a
//! pipeline made), or at part grain one part of it, shown over the scan it
//! was made on and the visit's other main scans, its panels frozen when the
//! campaign is made. Nothing here is a mechanism of its own: the items,
//! leases, answers, agreement and adjudication are the campaign's, and the
//! close writes through the one write path, [`review::apply_check_within`].
//!
//! - **The answer.** `value` is the verdict (accepted, rejected, flagged),
//!   `form` what it rests on (`reasons` on the whole result, `parts` with a
//!   reason each, `points` for a missed lesion, the parts `seen`, the scans
//!   `shown` beside the panels), `why` the note.
//! - **Two raters.** Compared on the verdict; an agreed flag is the union of
//!   their parts and points ([`outcome`]); reasons are compared as a set per
//!   item and reported, never sent to an adjudicator ([`reasons_overlap`]).
//! - **The close** ([`close_item`]): a decision on the result and one on each
//!   part it judged, each a person's, and the status columns beside them,
//!   in the caller's transaction. A part never seen gets nothing.
//! - **A direct check** ([`check_directly`], [`withdraw`]): the same
//!   decisions for one result outside a campaign; refused while an open
//!   check holds it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::campaign::{Answer, Campaign, Error, Item};
use crate::derivative;
use crate::review::{self, CHECK_AXIS};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};

/// The verdicts a check answers with, as the keys 1 and 2 and any reason
/// say them.
pub const VERDICTS: [&str; 3] = derivative::CHECKS;

/// What one item of a check is: a whole result (the default), or one part
/// of a `components` set.
pub const GRAINS: [&str; 2] = ["result", "part"];

/// What a reason needs beside it: nothing, a note, or a point on a picture.
pub const NEEDS: [&str; 3] = ["none", "note", "point"];

/// A check declares one to seven reasons, the keys 3 to 9.
pub const REASONS_MAX: usize = 7;

/// A reason's words fit on its key.
pub const WORDS_MAX: usize = 32;

/// A reason's id: `[a-z_]{1,24}`.
pub const ID_MAX: usize = 24;

/// The most points one answer places.
pub const POINTS_MAX: usize = 50;

/// The longest note an answer carries, in characters.
pub const NOTE_MAX: usize = 2000;

/// The most panels an item shows, the made-on scan's included.
pub const PANELS_MAX: usize = 8;

/// The role of panel 0: the scan the result was made on.
pub const MADE_ON: &str = "made_on";

/// How a panel's scan is tied to the result: voxel for voxel, or by world
/// position in the same scanner frame.
pub const PANEL_LINKS: [&str; 2] = ["same", "world"];

/// The kind of the review item a direct check closes (section 8.5).
pub const DIRECT_KIND: &str = "derivative.check";

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

// ------------------------------------------------------------ the question

/// One reason a check declares: its id, its words on the key, and what it
/// needs beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub id: String,
    pub words: String,
    pub needs: String,
}

/// Whether a text is a reason's id: `[a-z_]{1,24}`.
pub fn is_reason_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= ID_MAX && s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// The reasons a check question declares, held to section 6.1: one to
/// seven, each an id of `[a-z_]{1,24}` named once, words of at most 32
/// characters, and what it needs (none, note or point).
pub fn parse_reasons(v: &Value) -> Result<Vec<Reason>, Error> {
    let list = v
        .as_array()
        .ok_or_else(|| invalid("question.reasons: a list of {id, words, needs}, one to seven"))?;
    if list.is_empty() || list.len() > REASONS_MAX {
        return Err(invalid(format!(
            "question.reasons: one to {REASONS_MAX} reasons, the keys 3 to 9; {} given",
            list.len()
        )));
    }
    let mut out: Vec<Reason> = Vec::with_capacity(list.len());
    for r in list {
        let id = r["id"].as_str().unwrap_or_default().trim().to_string();
        if !is_reason_id(&id) {
            return Err(invalid(format!(
                "question.reasons: an id is 1 to {ID_MAX} of a to z and _, not {:?}",
                r["id"]
            )));
        }
        if out.iter().any(|x| x.id == id) {
            return Err(invalid(format!("question.reasons names {id} twice")));
        }
        let words = r["words"].as_str().unwrap_or_default().trim().to_string();
        if words.is_empty() || words.chars().count() > WORDS_MAX {
            return Err(invalid(format!(
                "question.reasons: {id}'s words are 1 to {WORDS_MAX} characters"
            )));
        }
        let needs = match &r["needs"] {
            Value::Null => "none".to_string(),
            Value::String(n) if NEEDS.contains(&n.as_str()) => n.clone(),
            other => {
                return Err(invalid(format!(
                    "question.reasons: {id} needs none, note or point, not {other}"
                )));
            }
        };
        out.push(Reason { id, words, needs });
    }
    Ok(out)
}

/// The reasons as a question keeps them.
pub fn reasons_json(reasons: &[Reason]) -> Value {
    json!(
        reasons
            .iter()
            .map(|r| json!({"id": r.id, "words": r.words, "needs": r.needs}))
            .collect::<Vec<_>>()
    )
}

/// The reasons the New check fills in, by the kind of the part set looked
/// at (section 6.1): lesions, structures, or a whole result. The person may
/// change them; the engine takes what the question declares.
pub fn presets() -> Value {
    let r = |id: &str, words: &str, needs: &str| json!({"id": id, "words": words, "needs": needs});
    json!({
        "components": [
            r("under", "Under-segmented", "none"),
            r("over", "Over-segmented", "none"),
            r("under_over", "Under and over", "none"),
            r("joined", "Two or more joined", "none"),
            r("not_lesion", "Not a lesion", "none"),
            r("missed", "Missed lesion", "point"),
            r("other", "Other", "note"),
        ],
        "labels": [
            r("boundary_off", "Boundary off", "none"),
            r("wrong_label", "Wrong label", "none"),
            r("part_missing", "Part missing", "none"),
            r("extra_part", "Extra part", "none"),
            r("other", "Other", "note"),
        ],
        "whole": [
            r("partly_wrong", "Partly wrong", "none"),
            r("misplaced", "Misplaced", "none"),
            r("other", "Other", "note"),
        ],
    })
}

/// Whether a campaign is a check.
pub fn is_check(c: &Campaign) -> bool {
    c.question["kind"] == "check"
}

// ------------------------------------------------------------ the items

/// One panel of an item: its place, the role it shows, the scan and how the
/// result is tied to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub panel: i64,
    pub role: String,
    pub stack: i64,
    pub link: String,
}

/// One item of a check to make: the result, the set it is read as, the part
/// at part grain, and its panels in order, the made-on scan first.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckItem {
    pub derivative_id: i64,
    pub part_set: String,
    pub part_label: Option<i64>,
    pub panels: Vec<Panel>,
}

impl CheckItem {
    /// The item's key: the result, or the result's part.
    pub fn key(&self) -> String {
        match self.part_label {
            Some(l) => format!("part:{}:{}:{l}", self.derivative_id, self.part_set),
            None => format!("result:{}", self.derivative_id),
        }
    }

    /// What its review item names.
    pub fn reference(&self) -> Value {
        match self.part_label {
            Some(l) => json!({
                "derivative_id": self.derivative_id, "part_set": self.part_set, "label": l,
            }),
            None => json!({"derivative_id": self.derivative_id}),
        }
    }

    /// The scope of its review item.
    pub fn scope(&self) -> &'static str {
        if self.part_label.is_some() {
            "derivative_part"
        } else {
            "derivative"
        }
    }

    /// Held to section 5: the made-on scan first, at most eight panels, no
    /// scan twice, each tied same or world.
    pub fn check(&self) -> Result<(), Error> {
        let Some(first) = self.panels.first() else {
            return Err(invalid(format!(
                "result {} has no panel: its made-on scan is shown first",
                self.derivative_id
            )));
        };
        if first.role != MADE_ON || first.panel != 0 {
            return Err(invalid(format!(
                "result {}'s first panel is the scan it was made on",
                self.derivative_id
            )));
        }
        if self.panels.len() > PANELS_MAX {
            return Err(invalid(format!(
                "panels: at most {PANELS_MAX}, the made-on scan included"
            )));
        }
        let stacks: BTreeSet<i64> = self.panels.iter().map(|p| p.stack).collect();
        if stacks.len() != self.panels.len() {
            return Err(invalid(format!(
                "result {} shows one scan on two panels",
                self.derivative_id
            )));
        }
        if let Some(p) = self
            .panels
            .iter()
            .find(|p| !PANEL_LINKS.contains(&p.link.as_str()))
        {
            return Err(invalid(format!(
                "a panel is tied same or world, not {}",
                p.link
            )));
        }
        Ok(())
    }
}

/// Keep the panels of each item of a new check: the item at position `i`
/// shows `items[i]`'s. Inside the transaction that wrote the items.
pub fn write_panels(store: &mut Store, campaign: i64, items: &[CheckItem]) -> Result<(), Error> {
    let sql = format!(
        "SELECT id, position FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_item"),
        store.dialect().param(1, Type::Int)
    );
    let mut item_at: BTreeMap<i64, i64> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(campaign)])? {
        item_at.insert(r.int(1)?, r.int(0)?);
    }
    let mut rows = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let item = *item_at
            .get(&(i as i64))
            .ok_or_else(|| invalid(format!("the item at position {i} was not written")))?;
        for p in &it.panels {
            rows.push(vec![
                Param::Int(campaign),
                Param::Int(item),
                Param::Int(p.panel),
                Param::from(p.role.as_str()),
                Param::Int(p.stack),
                Param::from(p.link.as_str()),
            ]);
        }
    }
    for chunk in rows.chunks(300) {
        store.insert(
            &Insert::new(
                table("campaign_check_panel"),
                &[
                    "campaign_id",
                    "item_id",
                    "panel",
                    "role",
                    "stack_id",
                    "link",
                ],
            ),
            chunk,
        )?;
    }
    Ok(())
}

/// The panels of a campaign's items, by item, in order; one item's where
/// `item` is given.
pub fn panels_of(
    store: &mut Store,
    campaign: i64,
    item: Option<i64>,
) -> Result<BTreeMap<i64, Vec<Panel>>, StoreError> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT item_id, panel, role, stack_id, link FROM {} WHERE campaign_id = {}",
        store.qualified("campaign_check_panel"),
        d.param(1, Type::Int)
    );
    let mut params = vec![Param::Int(campaign)];
    if let Some(i) = item {
        sql.push_str(&format!(" AND item_id = {}", d.param(2, Type::Int)));
        params.push(Param::Int(i));
    }
    sql.push_str(" ORDER BY item_id, panel");
    let mut out: BTreeMap<i64, Vec<Panel>> = BTreeMap::new();
    for r in store.query(&sql, &params)? {
        out.entry(r.int(0)?).or_default().push(Panel {
            panel: r.int(1)?,
            role: r.text(2)?.to_string(),
            stack: r.int(3)?,
            link: r.text(4)?.to_string(),
        });
    }
    Ok(out)
}

/// Every scan a check's panels show.
pub fn stacks_of(store: &mut Store, campaign: i64) -> Result<Vec<i64>, StoreError> {
    let sql = format!(
        "SELECT DISTINCT stack_id FROM {} WHERE campaign_id = {} ORDER BY stack_id",
        store.qualified("campaign_check_panel"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(|r| r.int(0))
        .collect()
}

/// The open (or closing) check campaign that holds a result, by id and
/// name: it decides the result, and no direct check does meanwhile.
pub fn held_by_open(store: &mut Store, derivative: i64) -> Result<Option<(i64, String)>, Error> {
    let sql = format!(
        "SELECT c.id, c.name FROM {} i JOIN {} c ON c.id = i.campaign_id \
         WHERE c.status IN ('open', 'closing') AND i.derivative_id = {} ORDER BY c.id",
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        store.dialect().param(1, Type::Int)
    );
    Ok(
        match store.query(&sql, &[Param::Int(derivative)])?.first() {
            Some(r) => Some((r.int(0)?, r.text(1)?.to_string())),
            None => None,
        },
    )
}

/// The open (or closing) check campaign other than `campaign` that holds a
/// result, by id and name: a later close of `campaign` leaves the result to
/// it.
pub fn held_by_another(
    store: &mut Store,
    derivative: i64,
    campaign: i64,
) -> Result<Option<(i64, String)>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT c.id, c.name FROM {} i JOIN {} c ON c.id = i.campaign_id \
         WHERE c.status IN ('open', 'closing') AND i.derivative_id = {} AND c.id <> {} ORDER BY c.id",
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
    );
    Ok(
        match store
            .query(&sql, &[Param::Int(derivative), Param::Int(campaign)])?
            .first()
        {
            Some(r) => Some((r.int(0)?, r.text(1)?.to_string())),
            None => None,
        },
    )
}

/// A check in force on a result or one of its parts, written at or after
/// `since` by anyone but `campaign` (another check's close, or a direct
/// check): the newest such decision. A later close of `campaign` leaves it
/// standing rather than write an older judgement over it.
pub fn checked_since(
    store: &mut Store,
    derivative: i64,
    campaign: i64,
    since: &str,
) -> Result<Option<i64>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE axis = {} AND withdrawn_at IS NULL \
         AND ((scope = 'derivative' AND ref = {}) OR (scope = 'derivative_part' AND ref LIKE {})) \
         AND (campaign_id IS NULL OR campaign_id <> {}) AND decided_at >= {} \
         ORDER BY id DESC",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        d.param(4, Type::Int),
        d.param(5, Type::Timestamp),
    );
    Ok(
        match store
            .query(
                &sql,
                &[
                    Param::from(CHECK_AXIS),
                    Param::from(derivative.to_string()),
                    Param::from(format!("{derivative}:%")),
                    Param::Int(campaign),
                    Param::from(since),
                ],
            )?
            .first()
        {
            Some(r) => Some(r.int(0)?),
            None => None,
        },
    )
}

/// Of these results, the ones an open (or closing) check campaign holds,
/// read inside the caller's transaction with the results' rows locked
/// (Postgres; SQLite's immediate transaction holds the whole database), so
/// two writers that both mean to hold a result take turns, and the second
/// sees the first's hold (the review of E2, item 8).
pub fn held_locked(store: &mut Store, derivatives: &[i64]) -> Result<BTreeSet<i64>, Error> {
    if matches!(store, Store::Postgres { .. }) {
        for chunk in derivatives.chunks(500) {
            let list = chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            store.query(
                &format!(
                    "SELECT id FROM {} WHERE id IN ({list}) ORDER BY id FOR UPDATE",
                    store.qualified("derivative")
                ),
                &[],
            )?;
        }
    }
    held_among(store, derivatives)
}

/// Of these results, the ones an open (or closing) check campaign holds.
pub fn held_among(store: &mut Store, derivatives: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let mut out = BTreeSet::new();
    for chunk in derivatives.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT i.derivative_id FROM {} i JOIN {} c ON c.id = i.campaign_id \
             WHERE c.status IN ('open', 'closing') AND i.derivative_id IN ({list})",
            store.qualified("campaign_item"),
            store.qualified("campaign"),
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------ the answer

/// What an answer rests on (section 6.2), read from its form.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Form {
    /// Reasons on the whole result.
    pub reasons: Vec<String>,
    /// Parts flagged, each with its reason.
    pub parts: Vec<(i64, String)>,
    /// Points placed on a picture, each with its reason and where, in
    /// millimetres (LPS).
    pub points: Vec<(String, [f64; 3])>,
    /// The parts the rater had in focus for half a second or more.
    pub seen: Vec<i64>,
    /// The scans shown beside the panels for this item.
    pub shown: Vec<i64>,
}

impl Form {
    /// Every reason it gives, on the result, on a part or with a point,
    /// each once.
    pub fn every_reason(&self) -> BTreeSet<String> {
        self.all_reasons().into_iter().collect()
    }

    /// Every reason it gives, in the order it gives them: on the result,
    /// on its parts (each of a part's reasons where an outcome joined
    /// several), with its points; each once.
    pub fn all_reasons(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self
            .reasons
            .iter()
            .map(String::as_str)
            .chain(self.parts.iter().flat_map(|(_, r)| reasons_in(r)))
            .chain(self.points.iter().map(|(r, _)| r.as_str()))
        {
            if !out.iter().any(|x| x == r) {
                out.push(r.to_string());
            }
        }
        out
    }

    /// The reasons on the whole result: its own and its points', in order,
    /// each once.
    pub fn result_reasons(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self
            .reasons
            .iter()
            .chain(self.points.iter().map(|(r, _)| r))
        {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
        out
    }

    pub fn as_json(&self) -> Value {
        json!({
            "reasons": self.reasons,
            "parts": self.parts.iter().map(|(l, r)| json!({"label": l, "reason": r})).collect::<Vec<_>>(),
            "points": self.points.iter().map(|(r, mm)| json!({"reason": r, "mm": mm})).collect::<Vec<_>>(),
            "seen": self.seen,
            "shown": self.shown,
        })
    }

    fn is_empty_judgement(&self) -> bool {
        self.reasons.is_empty() && self.parts.is_empty() && self.points.is_empty()
    }
}

/// The reasons one text gives: a reason id, or the reasons of a part
/// several raters flagged, joined (see [`outcome`]). Reason ids hold no
/// comma, so the join is read back whole.
pub fn reasons_in(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').map(str::trim).filter(|r| !r.is_empty())
}

/// A part's reasons as the one text its outcome entry, its decision's why
/// and its status hold: each once, in order, joined as a decision's why
/// joins a result's reasons.
fn joined(reasons: &[String]) -> String {
    reasons.join(", ")
}

/// Read a check answer's form, held to its shape: each field a list of what
/// section 6.2 names, no other field, no part named twice, at most fifty
/// points. A form that is not given is an empty one.
pub fn form_of(v: Option<&Value>) -> Result<Form, Error> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(Form::default());
    };
    let m = v
        .as_object()
        .ok_or_else(|| invalid("form: {reasons, parts, points, seen, shown}"))?;
    if let Some(k) = m
        .keys()
        .find(|k| !["reasons", "parts", "points", "seen", "shown"].contains(&k.as_str()))
    {
        return Err(invalid(format!(
            "form.{k} is not part of a check's answer: reasons, parts, points, seen, shown"
        )));
    }
    let list = |k: &str| -> Result<Vec<Value>, Error> {
        match &v[k] {
            Value::Null => Ok(Vec::new()),
            Value::Array(a) => Ok(a.clone()),
            _ => Err(invalid(format!("form.{k} is a list"))),
        }
    };
    let ints = |k: &str| -> Result<Vec<i64>, Error> {
        let mut out = Vec::new();
        for x in list(k)? {
            let n = x
                .as_i64()
                .ok_or_else(|| invalid(format!("form.{k}: whole numbers")))?;
            if !out.contains(&n) {
                out.push(n);
            }
        }
        Ok(out)
    };
    let mut f = Form::default();
    for r in list("reasons")? {
        let r = r
            .as_str()
            .ok_or_else(|| invalid("form.reasons: reason ids"))?
            .to_string();
        if !f.reasons.contains(&r) {
            f.reasons.push(r);
        }
    }
    for p in list("parts")? {
        let (Some(label), Some(reason)) = (p["label"].as_i64(), p["reason"].as_str()) else {
            return Err(invalid("form.parts: each {label, reason}"));
        };
        if f.parts.iter().any(|(l, _)| *l == label) {
            return Err(invalid(format!(
                "form.parts names part {label} twice; a part takes one reason"
            )));
        }
        f.parts.push((label, reason.to_string()));
    }
    let points = list("points")?;
    if points.len() > POINTS_MAX {
        return Err(invalid(format!(
            "form.points: at most {POINTS_MAX}; {} given",
            points.len()
        )));
    }
    for p in points {
        let reason = p["reason"]
            .as_str()
            .ok_or_else(|| invalid("form.points: each {reason, mm: [x, y, z]}"))?;
        let mm: Vec<f64> = p["mm"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if mm.len() != 3 || mm.iter().any(|x| !x.is_finite()) {
            return Err(invalid(
                "form.points: each point's mm is three numbers, x, y and z in millimetres",
            ));
        }
        f.points.push((reason.to_string(), [mm[0], mm[1], mm[2]]));
    }
    f.seen = ints("seen")?;
    f.shown = ints("shown")?;
    if f.shown.len() > PANELS_MAX {
        return Err(invalid(format!(
            "form.shown: at most {PANELS_MAX} scans beside the panels"
        )));
    }
    Ok(f)
}

/// Whether an answer fits a check question (section 6.2), from the answer
/// alone: a verdict; `accepted` with no reason, part or point; `flagged`
/// with at least one; `rejected` with reasons at most (at part grain, none:
/// it says "not a lesion"); every reason one the question declares; a
/// reason that needs a point given as a point and a point only with such a
/// reason; a reason that needs a note with a note; a note of at most 2,000
/// characters.
pub fn fits(
    grain: &str,
    reasons: &[Reason],
    value: Option<&str>,
    form: Option<&Value>,
    why: Option<&str>,
) -> Result<Form, Error> {
    let verdict = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid("the answer is a verdict: accepted, rejected or flagged"))?;
    if !VERDICTS.contains(&verdict) {
        return Err(invalid(format!(
            "{verdict} is not a check's verdict: accepted, rejected or flagged"
        )));
    }
    let f = form_of(form)?;
    if let Some(w) = why
        && w.chars().count() > NOTE_MAX
    {
        return Err(invalid(format!(
            "the note is at most {NOTE_MAX} characters"
        )));
    }
    let declared = |id: &str| reasons.iter().find(|r| r.id == id);
    for r in f.reasons.iter().chain(f.parts.iter().map(|(_, r)| r)) {
        let Some(d) = declared(r) else {
            return Err(invalid(format!(
                "{r} is not a reason this check declares: {}",
                reasons
                    .iter()
                    .map(|r| r.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        if d.needs == "point" {
            return Err(invalid(format!(
                "{r} ({}) is given as a point on a picture",
                d.words
            )));
        }
    }
    for (r, _) in &f.points {
        match declared(r) {
            Some(d) if d.needs == "point" => {}
            Some(d) => {
                return Err(invalid(format!(
                    "a point is placed for a reason that needs one; {r} ({}) does not",
                    d.words
                )));
            }
            None => {
                return Err(invalid(format!("{r} is not a reason this check declares")));
            }
        }
    }
    let wants_note = f
        .every_reason()
        .iter()
        .filter_map(|r| declared(r))
        .find(|d| d.needs == "note");
    if let Some(d) = wants_note
        && why.is_none_or(|w| w.trim().is_empty())
    {
        return Err(invalid(format!("{} ({}) comes with a note", d.id, d.words)));
    }
    match verdict {
        "accepted" if !f.is_empty_judgement() => {
            return Err(invalid(
                "an accepted result carries no reason, part or point; flag it instead",
            ));
        }
        "flagged" if f.is_empty_judgement() => {
            return Err(invalid(
                "a flagged result carries a reason, a flagged part or a point",
            ));
        }
        "rejected" if grain == "part" && !f.is_empty_judgement() => {
            return Err(invalid(
                "at part grain a rejection says not a lesion, and carries no reason",
            ));
        }
        "rejected" if !f.parts.is_empty() || !f.points.is_empty() => {
            return Err(invalid(
                "a rejected result is unusable whole and carries reasons at most; its parts are not judged",
            ));
        }
        _ => {}
    }
    Ok(f)
}

/// Whether an answer fits its item, as only the registry can say: every
/// part it flags or saw is a part of the item's set (at part grain, the
/// item's own part alone is flagged), and every scan shown beside the
/// panels is a scan of the made-on scan's frame.
pub fn fits_item(store: &mut Store, item: &Item, f: &Form) -> Result<(), Error> {
    let (Some(d), Some(set)) = (item.derivative_id, item.part_set.as_deref()) else {
        return Err(invalid(format!(
            "item {} names no result; it is not a check's",
            item.id
        )));
    };
    if let Some(own) = item.part_label
        && let Some((l, _)) = f.parts.iter().find(|(l, _)| *l != own)
    {
        return Err(invalid(format!(
            "item {} asks about part {own}; part {l} is answered in its own item",
            item.position + 1
        )));
    }
    let named: Vec<i64> = f
        .parts
        .iter()
        .map(|(l, _)| *l)
        .chain(f.seen.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let held = derivative::labels_held(store, d, set, &named)?;
    if let Some(l) = named.iter().find(|l| !held.contains(l)) {
        return Err(invalid(format!(
            "result {d} has no part {l} in its set {set}"
        )));
    }
    if !f.shown.is_empty() {
        let panels = panels_of(store, item.campaign_id, Some(item.id))?
            .remove(&item.id)
            .unwrap_or_default();
        let Some(made_on) = panels.first().map(|p| p.stack) else {
            return Err(invalid(format!("item {} has no panel", item.id)));
        };
        let frames = frames_of(store, &[&[made_on][..], &f.shown[..]].concat())?;
        let own = frames.get(&made_on).cloned().flatten();
        for s in &f.shown {
            match frames.get(s) {
                None => return Err(invalid(format!("form.shown: no stack {s}"))),
                Some(fr) if own.is_none() || *fr != own => {
                    return Err(invalid(format!(
                        "form.shown: stack {s} is not in the scanner frame of the scan the result was made on"
                    )));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// The frame of reference of each stack named, by stack; none where its
/// series names no frame.
pub fn frames_of(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, Option<String>>, StoreError> {
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, se.frame_of_reference_uid FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
        );
        for r in store.query(&sql, &[])? {
            out.insert(
                r.int(0)?,
                r.opt_text(1)?
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_string),
            );
        }
    }
    Ok(out)
}

/// A union's flagged parts, each once by label with its reasons joined,
/// from parts that may name a label more than once.
fn parts_joined<'a>(parts: impl IntoIterator<Item = (i64, &'a str)>) -> Vec<(i64, String)> {
    let mut by_label: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (label, reason) in parts {
        let list = by_label.entry(label).or_default();
        for r in reasons_in(reason) {
            if !list.iter().any(|x| x == r) {
                list.push(r.to_string());
            }
        }
    }
    by_label
        .into_iter()
        .map(|(l, rs)| (l, joined(&rs)))
        .collect()
}

/// What a check's item came to from the answers behind it (section 6.3):
/// the verdict they agree on, and the union of their reasons, flagged parts
/// (a part either rater flagged goes to correction), points, parts seen and
/// scans shown. A part several raters flagged is in the union once, with
/// its reasons as a set: each once, in the order the answers give them,
/// joined with ", " as a decision's why joins a result's reasons (section
/// 6.4), so the close writes one decision and one status for it.
pub fn outcome(answers: &[Answer]) -> Value {
    let mut union = Form::default();
    let mut flagged: Vec<(i64, String)> = Vec::new();
    for a in answers {
        let Ok(f) = form_of(a.form.as_ref()) else {
            continue;
        };
        for r in f.reasons {
            if !union.reasons.contains(&r) {
                union.reasons.push(r);
            }
        }
        flagged.extend(f.parts);
        for p in f.points {
            if !union.points.contains(&p) {
                union.points.push(p);
            }
        }
        for s in f.seen {
            if !union.seen.contains(&s) {
                union.seen.push(s);
            }
        }
        for s in f.shown {
            if !union.shown.contains(&s) {
                union.shown.push(s);
            }
        }
    }
    union.parts = parts_joined(flagged.iter().map(|(l, r)| (*l, r.as_str())));
    union.seen.sort_unstable();
    json!({
        "value": answers.first().and_then(|a| a.value.clone()),
        "form": union.as_json(),
        "answers": answers.iter().map(|a| a.id).collect::<Vec<_>>(),
    })
}

/// Read the form of an item's outcome, the union [`outcome`] made, held to
/// an answer's shape. An outcome made before a part's reasons were joined
/// names such a part once per reason; it is read as one part with its
/// reasons joined.
pub fn outcome_form(v: &Value) -> Result<Form, Error> {
    let mut v = v.clone();
    if let Some(parts) = v.get("parts").and_then(Value::as_array)
        && parts
            .iter()
            .all(|p| p["label"].is_i64() && p["reason"].is_string())
    {
        let joined = parts_joined(
            parts
                .iter()
                .filter_map(|p| Some((p["label"].as_i64()?, p["reason"].as_str()?))),
        );
        v["parts"] = json!(
            joined
                .iter()
                .map(|(l, r)| json!({"label": l, "reason": r}))
                .collect::<Vec<_>>()
        );
    }
    form_of(Some(&v))
}

/// How far the raters' reasons agree (section 6.3): over the items every
/// rater answered, the mean Jaccard overlap of the sets of reasons each
/// answer gives, pair by pair; two answers with no reason agree. Reported,
/// never sent to an adjudicator.
pub fn reasons_overlap(all: &[Answer], n: usize) -> Value {
    let mut by_item: BTreeMap<i64, Vec<BTreeSet<String>>> = BTreeMap::new();
    for a in all.iter().filter(|a| a.role == "rater") {
        let set = form_of(a.form.as_ref())
            .map(|f| f.every_reason())
            .unwrap_or_default();
        by_item.entry(a.item_id).or_default().push(set);
    }
    let mut overlaps = Vec::new();
    for sets in by_item.values().filter(|v| v.len() == n && n >= 2) {
        let mut sum = 0.0;
        let mut pairs = 0usize;
        for i in 0..sets.len() {
            for j in i + 1..sets.len() {
                let union = sets[i].union(&sets[j]).count();
                let inter = sets[i].intersection(&sets[j]).count();
                sum += if union == 0 {
                    1.0
                } else {
                    inter as f64 / union as f64
                };
                pairs += 1;
            }
        }
        if pairs > 0 {
            overlaps.push(sum / pairs as f64);
        }
    }
    json!({
        "items": overlaps.len(),
        "jaccard": (!overlaps.is_empty()).then(|| overlaps.iter().sum::<f64>() / overlaps.len() as f64),
    })
}

/// The answers' verdicts and reasons counted: each reason once per answer
/// that gives it, wherever it gives it.
pub fn tally(answers: &[&Answer]) -> Value {
    let mut verdicts: BTreeMap<&str, usize> = VERDICTS.iter().map(|v| (*v, 0)).collect();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for a in answers {
        if let Some(v) = a.value.as_deref()
            && let Some(n) = verdicts.get_mut(v)
        {
            *n += 1;
        }
        if let Ok(f) = form_of(a.form.as_ref()) {
            for r in f.every_reason() {
                *reasons.entry(r).or_default() += 1;
            }
        }
    }
    json!({"verdicts": verdicts, "reasons": reasons})
}

// ------------------------------------------------------------ the close

/// Who a check's decisions are by: a person, always.
#[derive(Debug, Clone, Copy)]
pub struct By<'a> {
    pub who: &'a str,
    pub kind: &'a str,
    pub campaign: Option<i64>,
}

/// The why a check's decision carries: its reasons, then the note.
fn why_of(reasons: &[String], note: Option<&str>) -> Option<String> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    match (reasons.is_empty(), note) {
        (true, None) => None,
        (true, Some(n)) => Some(n.to_string()),
        (false, None) => Some(reasons.join(", ")),
        (false, Some(n)) => Some(format!("{}: {n}", reasons.join(", "))),
    }
}

/// Write one decision of a check through the one write path: on the
/// result, or on one part of it. A refusal of the spine comes back as
/// words, a store's failure as an error.
fn decide(
    registry: &mut Registry,
    review_item: i64,
    part: Option<(&str, i64)>,
    value: &str,
    why: Option<&str>,
    by: By<'_>,
) -> Result<Result<i64, String>, Error> {
    let applied = review::apply_check_within(
        registry,
        &review::Apply {
            item: review_item,
            member: None,
            scope: if part.is_some() {
                "derivative_part"
            } else {
                "derivative"
            },
            value: Some(value),
            author: review::Author {
                who: by.who,
                kind: by.kind,
                version: None,
                model: None,
            },
            stage: false,
            why,
            campaign: by.campaign,
        },
        part,
    );
    match applied {
        Ok(a) if a.staged => Ok(Err(
            "a check is a person's decision and is written in force; it was staged".into(),
        )),
        Ok(a) => Ok(Ok(a.decision)),
        Err(review::Error::Store(e)) => Err(e.into()),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// Write the decisions and statuses of a result's parts: each flagged part
/// `flagged` with its reasons, once however many raters flagged it, and
/// each part seen and not flagged `accepted`. Answers the decisions by
/// label, or why not.
fn decide_parts(
    registry: &mut Registry,
    review_item: i64,
    derivative_id: i64,
    set: &str,
    flagged: &[(i64, String)],
    seen: &[i64],
    by: By<'_>,
) -> Result<Result<BTreeMap<i64, i64>, String>, Error> {
    let flagged = parts_joined(flagged.iter().map(|(l, r)| (*l, r.as_str())));
    let mut judged: Vec<(i64, &str, Option<String>)> = flagged
        .iter()
        .map(|(l, r)| (*l, "flagged", Some(r.clone()).filter(|r| !r.is_empty())))
        .collect();
    for l in seen {
        if !flagged.iter().any(|(f, _)| f == l) {
            judged.push((*l, "accepted", None));
        }
    }
    judged.sort_by_key(|(l, ..)| *l);
    let labels: Vec<i64> = judged.iter().map(|(l, ..)| *l).collect();
    let held = derivative::labels_held(registry.store(), derivative_id, set, &labels)?;
    if let Some(l) = labels.iter().find(|l| !held.contains(l)) {
        return Ok(Err(format!(
            "part {l} of set {set} is no part of result {derivative_id} now; its parts were made again"
        )));
    }
    let mut out = BTreeMap::new();
    // the decision's why and the status's reason are the part's reasons,
    // joined where several raters gave different ones
    for (label, verdict, reason) in judged {
        let decision = match decide(
            registry,
            review_item,
            Some((set, label)),
            verdict,
            reason.as_deref(),
            by,
        )? {
            Ok(d) => d,
            Err(why) => return Ok(Err(why)),
        };
        derivative::set_part_check(
            registry.store(),
            derivative_id,
            set,
            label,
            Some(verdict),
            reason.as_deref(),
            Some(decision),
        )?;
        out.insert(label, decision);
    }
    Ok(Ok(out))
}

/// Withdraw the decisions in force on a result's parts that an earlier
/// check wrote (any but `campaign`'s own, where one is named), and clear
/// the statuses they wrote, inside the caller's transaction: a later check
/// of a result takes the place of every earlier one, on the result and on
/// each of its parts, and a part it does not judge is left unchecked
/// rather than keep an earlier verdict. Nothing is deleted. Answers the
/// decisions withdrawn.
fn earlier_parts_withdrawn(
    store: &mut Store,
    derivative: i64,
    campaign: Option<i64>,
    now: &str,
) -> Result<Vec<i64>, StoreError> {
    let d = store.dialect();
    let mut sql = format!(
        "SELECT id FROM {} WHERE scope = 'derivative_part' AND axis = {} AND ref LIKE {} \
         AND withdrawn_at IS NULL",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let mut params = vec![
        Param::from(CHECK_AXIS),
        Param::from(format!("{derivative}:%")),
    ];
    if let Some(c) = campaign {
        params.push(Param::Int(c));
        sql.push_str(&format!(
            " AND (campaign_id IS NULL OR campaign_id <> {})",
            d.param(3, Type::Int)
        ));
    }
    let ids: Vec<i64> = store
        .query(&sql, &params)?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Ok(ids);
    }
    store.update_by_ids(
        table("decision"),
        &[("withdrawn_at", Param::from(now))],
        "id",
        &ids,
    )?;
    let list = ids
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    store.execute(
        &format!(
            "UPDATE {} SET check_status = NULL, check_reason = NULL, check_decision_id = NULL \
             WHERE derivative_id = {} AND check_decision_id IN ({list})",
            store.qualified("derivative_part"),
            d.param(1, Type::Int)
        ),
        &[Param::Int(derivative)],
    )?;
    Ok(ids)
}

/// What one check item's close wrote: the item's own decision first, then
/// every other.
pub type Closed = Vec<i64>;

/// Close one agreed or adjudicated item of a check into `check` (section
/// 6.4), inside the transaction the caller holds: at result grain, the
/// result's decision and status and one per part flagged or seen; at part
/// grain, the part's, and the result's once every part item of it in the
/// campaign is resolved (accepted when every part is, else flagged: a part
/// grain never rejects a whole result). The item's review item is closed
/// by its own decision and the item resolved. Answers the decisions, or why
/// the item was refused (the caller rolls back).
pub fn close_item(
    registry: &mut Registry,
    c: &Campaign,
    it: &Item,
    by: By<'_>,
    now: &str,
) -> Result<Result<Closed, String>, Error> {
    let (Some(d), Some(set)) = (it.derivative_id, it.part_set.clone()) else {
        return Ok(Err("a check item names its result".into()));
    };
    let Some(result) = derivative::get(registry.store(), d)? else {
        return Ok(Err(format!("result {d} is not in the registry")));
    };
    if result.withdrawn_at.is_some() {
        return Ok(Err(format!(
            "result {d} was withdrawn; a check is not written on it"
        )));
    }
    let Some(verdict) = it.outcome["value"]
        .as_str()
        .filter(|v| VERDICTS.contains(v))
        .map(str::to_string)
    else {
        return Ok(Err("the item came to no verdict".into()));
    };
    // an outcome that cannot be read is refused, never written as an empty
    // form: the item stays unresolved and the close names it
    let form = match outcome_form(&it.outcome["form"]) {
        Ok(f) => f,
        Err(e) => {
            return Ok(Err(format!(
                "the outcome of item {} (result {d}) cannot be read, so nothing is written for it: {e}",
                it.position + 1
            )));
        }
    };
    let mut decisions: Vec<i64> = Vec::new();
    let mut parts_written: BTreeMap<i64, i64> = BTreeMap::new();
    let mut result_decision = None;
    // a later check of the result takes the place of every earlier one on
    // its parts (the review of E2, item 5); this campaign's own stand
    earlier_parts_withdrawn(registry.store(), d, Some(c.id), now)?;
    match it.part_label {
        None => {
            let reasons = form.result_reasons();
            let own = match decide(
                registry,
                it.review_item_id,
                None,
                &verdict,
                why_of(&reasons, None).as_deref(),
                by,
            )? {
                Ok(id) => id,
                Err(why) => return Ok(Err(why)),
            };
            derivative::set_check(
                registry.store(),
                d,
                Some(&verdict),
                &json!(reasons),
                Some(own),
                Some(now),
            )?;
            decisions.push(own);
            result_decision = Some(own);
            if verdict != "rejected" {
                match decide_parts(
                    registry,
                    it.review_item_id,
                    d,
                    &set,
                    &form.parts,
                    &form.seen,
                    by,
                )? {
                    Ok(written) => parts_written = written,
                    Err(why) => return Ok(Err(why)),
                }
            }
        }
        Some(label) => {
            let held = derivative::labels_held(registry.store(), d, &set, &[label])?;
            if !held.contains(&label) {
                return Ok(Err(format!(
                    "part {label} of set {set} is no part of result {d} now; its parts were made again"
                )));
            }
            // the part's reasons, every rater's, joined as at result grain:
            // the decision's why and the status's reason alike
            let reason = why_of(&form.all_reasons(), None);
            let own = match decide(
                registry,
                it.review_item_id,
                Some((&set, label)),
                &verdict,
                reason.as_deref(),
                by,
            )? {
                Ok(id) => id,
                Err(why) => return Ok(Err(why)),
            };
            derivative::set_part_check(
                registry.store(),
                d,
                &set,
                label,
                Some(&verdict),
                reason.as_deref(),
                Some(own),
            )?;
            decisions.push(own);
            parts_written.insert(label, own);
            // the result follows once every part item of it is resolved
            let store = registry.store();
            let dl = store.dialect();
            let rows = store.query(
                &format!(
                    "SELECT id, state, part_label FROM {} WHERE campaign_id = {} AND derivative_id = {} AND part_set = {}",
                    store.qualified("campaign_item"),
                    dl.param(1, Type::Int),
                    dl.param(2, Type::Int),
                    dl.param(3, Type::Text),
                ),
                &[Param::Int(c.id), Param::Int(d), Param::from(set.as_str())],
            )?;
            let mut labels = Vec::new();
            let mut all = true;
            for r in &rows {
                let (id, state) = (r.int(0)?, r.text(1)?);
                if id != it.id && state != "resolved" {
                    all = false;
                }
                if let Some(l) = r.opt_int(2)? {
                    labels.push(l);
                }
            }
            if all {
                let (_, parts) = derivative::parts(
                    store,
                    d,
                    &set,
                    derivative::PartOrder::Position,
                    0,
                    derivative::PARTS_MAX,
                )?;
                let ours: Vec<&derivative::Part> =
                    parts.iter().filter(|p| labels.contains(&p.label)).collect();
                let every = ours
                    .iter()
                    .all(|p| p.check_status.as_deref() == Some("accepted"));
                let status = if every { "accepted" } else { "flagged" };
                let mut reasons: Vec<String> = Vec::new();
                for r in ours
                    .iter()
                    .filter_map(|p| p.check_reason.as_deref())
                    .flat_map(reasons_in)
                {
                    if !reasons.iter().any(|x| x == r) {
                        reasons.push(r.to_string());
                    }
                }
                let whole = match decide(
                    registry,
                    it.review_item_id,
                    None,
                    status,
                    why_of(&reasons, None).as_deref(),
                    by,
                )? {
                    Ok(id) => id,
                    Err(why) => return Ok(Err(why)),
                };
                derivative::set_check(
                    registry.store(),
                    d,
                    Some(status),
                    &json!(reasons),
                    Some(whole),
                    Some(now),
                )?;
                decisions.push(whole);
                result_decision = Some(whole);
            }
        }
    }
    let store = registry.store();
    let mut outcome = it.outcome.clone();
    outcome["decisions"] = json!({
        "result": result_decision,
        "parts": parts_written.iter().map(|(l, id)| (l.to_string(), json!(id))).collect::<serde_json::Map<_, _>>(),
    });
    store.update_by_id(
        table("campaign_item"),
        &[
            ("state", Param::from("resolved")),
            ("decision_id", Param::Int(decisions[0])),
            ("outcome", Param::from(outcome.to_string())),
            ("resolved_at", Param::from(now)),
            // an earlier close's refusal of the item ends as it is written
            ("refused_at", Param::Null),
            ("refused_why", Param::Null),
        ],
        "id",
        it.id,
    )?;
    Ok(Ok(decisions))
}

// ------------------------------------------------------------ directly

/// One result checked by a person outside a campaign (section 8.5).
#[derive(Debug, Clone)]
pub struct Direct<'a> {
    pub derivative: i64,
    pub value: &'a str,
    /// Reasons on the whole result.
    pub reasons: &'a [String],
    /// Parts flagged, each with its reason, of `set`.
    pub parts: &'a [(i64, String)],
    /// The part set the parts are of; the result's first where none.
    pub set: Option<&'a str>,
    pub why: Option<&'a str>,
    pub who: &'a str,
    pub kind: &'a str,
}

/// What a direct check wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub derivative: i64,
    pub decision: i64,
    pub parts: BTreeMap<i64, i64>,
}

impl Checked {
    pub fn as_json(&self) -> Value {
        json!({
            "derivative": self.derivative, "decision": self.decision,
            "parts": self.parts.iter().map(|(l, d)| (l.to_string(), json!(d))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

fn a_persons(kind: &str) -> Result<(), Error> {
    if kind != "person" {
        return Err(Error::Forbidden(format!(
            "a check is a person's decision; this caller acts as {}",
            crate::a_or_an(kind)
        )));
    }
    Ok(())
}

/// The result a direct check or its withdrawal is about: a mask, not
/// withdrawn, that no open check campaign holds.
fn checkable(store: &mut Store, id: i64) -> Result<derivative::Derivative, Error> {
    let d = derivative::get(store, id)?
        .ok_or_else(|| Error::NotFound(format!("no derivative {id}")))?;
    if d.kind != "mask" {
        return Err(invalid(format!(
            "derivative {id} is {}; a check is of a mask",
            crate::a_or_an(&d.kind)
        )));
    }
    if d.withdrawn_at.is_some() {
        return Err(refused(format!("result {id} is withdrawn")));
    }
    if let Some((c, name)) = held_by_open(store, id)? {
        return Err(refused(format!(
            "result {id} is held by the open check {name} (campaign {c}); the campaign decides it"
        )));
    }
    Ok(d)
}

/// Check one result directly (section 8.5): the same decisions and status a
/// close writes, by a person, through a review item of its own that the
/// decision closes. The earlier decision on the result, and on each part it
/// names, is withdrawn by the new one, never deleted.
pub fn check_directly(registry: &mut Registry, x: &Direct<'_>) -> Result<Checked, Error> {
    a_persons(x.kind)?;
    let d = checkable(registry.store(), x.derivative)?;
    let value = x.value.trim();
    if !VERDICTS.contains(&value) {
        return Err(invalid(format!(
            "value: accepted, rejected or flagged, not {value}"
        )));
    }
    if let Some(r) = x
        .reasons
        .iter()
        .chain(x.parts.iter().map(|(_, r)| r))
        .find(|r| !is_reason_id(r))
    {
        return Err(invalid(format!(
            "a reason is 1 to {ID_MAX} of a to z and _, not {r:?}"
        )));
    }
    if let Some(w) = x.why
        && w.chars().count() > NOTE_MAX
    {
        return Err(invalid(format!("why: at most {NOTE_MAX} characters")));
    }
    let judged = !x.reasons.is_empty() || !x.parts.is_empty();
    match value {
        "accepted" if judged => {
            return Err(invalid(
                "an accepted result carries no reason or part; flag it instead",
            ));
        }
        "flagged" if !judged => {
            return Err(invalid("a flagged result carries a reason or a part"));
        }
        "rejected" if !x.parts.is_empty() => {
            return Err(invalid(
                "a rejected result is unusable whole; its parts are not judged",
            ));
        }
        _ => {}
    }
    let labels: BTreeSet<i64> = x.parts.iter().map(|(l, _)| *l).collect();
    if labels.len() != x.parts.len() {
        return Err(invalid("parts names a part twice; a part takes one reason"));
    }
    let set = match x.set {
        Some(s) => s.to_string(),
        None => d
            .part_sets
            .as_array()
            .and_then(|a| a.first())
            .and_then(|s| s["key"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "whole".to_string()),
    };
    if !x.parts.is_empty() {
        let named: Vec<i64> = labels.iter().copied().collect();
        let held = derivative::labels_held(registry.store(), d.id, &set, &named)?;
        if let Some(l) = named.iter().find(|l| !held.contains(l)) {
            return Err(invalid(format!(
                "result {} has no part {l} in its set {set}",
                d.id
            )));
        }
    }
    let now = crate::time::now_iso();
    registry.store().begin()?;
    let written = (|| -> Result<Result<Checked, String>, Error> {
        let store = registry.store();
        // the hold read again under the lock: a check made meanwhile
        // decides the result
        if !held_locked(store, &[d.id])?.is_empty() {
            return Err(refused(format!(
                "result {} is held by an open check; the campaign decides it",
                d.id
            )));
        }
        let item = store
            .insert(
                &Insert::new(
                    table("review_item"),
                    &[
                        "kind",
                        "scope",
                        "ref",
                        "evidence",
                        "status",
                        "created_at",
                        "members",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(DIRECT_KIND),
                    Param::from("derivative"),
                    Param::from(json!({"derivative_id": d.id}).to_string()),
                    Param::from(
                        json!({"axis": CHECK_AXIS, "direct": true, "part_set": set}).to_string(),
                    ),
                    Param::from("open"),
                    Param::from(now.as_str()),
                    Param::Int(1),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the review item was not written back".into()))?
            .int(0)?;
        let by = By {
            who: x.who,
            kind: x.kind,
            campaign: None,
        };
        // the result checked again: every earlier decision on its parts
        // gives way to this check's
        earlier_parts_withdrawn(registry.store(), d.id, None, &now)?;
        let decision = match decide(
            registry,
            item,
            None,
            value,
            why_of(x.reasons, x.why).as_deref(),
            by,
        )? {
            Ok(id) => id,
            Err(why) => return Ok(Err(why)),
        };
        derivative::set_check(
            registry.store(),
            d.id,
            Some(value),
            &json!(x.reasons),
            Some(decision),
            Some(&now),
        )?;
        let parts = match decide_parts(registry, item, d.id, &set, x.parts, &[], by)? {
            Ok(p) => p,
            Err(why) => return Ok(Err(why)),
        };
        Ok(Ok(Checked {
            derivative: d.id,
            decision,
            parts,
        }))
    })();
    match written {
        Ok(Ok(c)) => {
            registry.store().commit()?;
            Ok(c)
        }
        Ok(Err(why)) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(refused(why))
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(e)
        }
    }
}

/// Withdraw the check that stands on a result (section 8.5): its decision
/// and every part's stop being in force, nothing is deleted, and the status
/// columns are cleared. The review items those decisions closed stay
/// closed: a withdrawn check leaves the result unchecked, not asked again.
/// Answers the decisions withdrawn.
pub fn withdraw(
    registry: &mut Registry,
    id: i64,
    who: &str,
    kind: &str,
    why: Option<&str>,
) -> Result<Vec<i64>, Error> {
    a_persons(kind)?;
    checkable(registry.store(), id)?;
    let now = crate::time::now_iso();
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE axis = {} AND withdrawn_at IS NULL \
         AND ((scope = 'derivative' AND ref = {}) OR (scope = 'derivative_part' AND ref LIKE {})) ORDER BY id",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
    );
    let ids: Vec<i64> = store
        .query(
            &sql,
            &[
                Param::from(CHECK_AXIS),
                Param::from(id.to_string()),
                Param::from(format!("{id}:%")),
            ],
        )?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Err(refused(format!("result {id} carries no check to withdraw")));
    }
    store.begin()?;
    let written = (|| -> Result<(), Error> {
        if !held_locked(store, &[id])?.is_empty() {
            return Err(refused(format!(
                "result {id} is held by an open check; the campaign decides it"
            )));
        }
        store.update_by_ids(
            table("decision"),
            &[("withdrawn_at", Param::from(now.as_str()))],
            "id",
            &ids,
        )?;
        derivative::set_check(store, id, None, &Value::Null, None, None)?;
        derivative::clear_part_checks(store, id)?;
        Ok(())
    })();
    match written {
        Ok(()) => store.commit()?,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    }
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::Decision,
            scope: json!({"withdrawn": ids, "derivative": id, "axis": CHECK_AXIS}),
            policy: None,
            job_id: None,
            details: why.map(|w| json!({"why": w})),
        },
    )?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesions() -> Vec<Reason> {
        parse_reasons(&presets()["components"]).unwrap()
    }

    #[test]
    fn the_question_s_reasons_are_held_to_section_6_1() {
        assert_eq!(lesions().len(), 7);
        assert_eq!(parse_reasons(&presets()["labels"]).unwrap().len(), 5);
        assert_eq!(parse_reasons(&presets()["whole"]).unwrap().len(), 3);
        let bad = |v: Value| parse_reasons(&v).unwrap_err().to_string();
        assert!(bad(json!([])).contains("one to 7"));
        let eight: Vec<Value> = (0..8)
            .map(|i| json!({"id": format!("r{}", "a".repeat(i + 1)), "words": "W"}))
            .collect();
        assert!(bad(json!(eight)).contains("one to 7"));
        assert!(bad(json!([{"id": "Under", "words": "U"}])).contains("an id is"));
        assert!(bad(json!([{"id": "a".repeat(25), "words": "U"}])).contains("an id is"));
        assert!(bad(json!([{"id": "u", "words": "x".repeat(33)}])).contains("words"));
        assert!(
            bad(json!([{"id": "u", "words": "U"}, {"id": "u", "words": "V"}])).contains("twice")
        );
        assert!(bad(json!([{"id": "u", "words": "U", "needs": "picture"}])).contains("needs"));
        assert_eq!(
            parse_reasons(&json!([{"id": "u", "words": "U"}])).unwrap()[0].needs,
            "none"
        );
    }

    #[test]
    fn an_answer_is_held_to_section_6_2() {
        let r = lesions();
        let ok = |v: &str, f: Value, why: Option<&str>| fits("result", &r, Some(v), Some(&f), why);
        assert!(ok("accepted", json!({"seen": [1, 2]}), None).is_ok());
        assert!(ok("accepted", json!({"reasons": ["under"]}), None).is_err());
        assert!(ok("flagged", json!({}), None).is_err());
        assert!(
            ok(
                "flagged",
                json!({"parts": [{"label": 7, "reason": "under"}]}),
                None
            )
            .is_ok()
        );
        assert!(
            ok(
                "flagged",
                json!({"points": [{"reason": "missed", "mm": [1.0, 2.0, 3.0]}]}),
                None
            )
            .is_ok()
        );
        // a point's reason needs a point, and a point needs such a reason
        assert!(ok("flagged", json!({"reasons": ["missed"]}), None).is_err());
        assert!(
            ok(
                "flagged",
                json!({"points": [{"reason": "under", "mm": [1, 2, 3]}]}),
                None
            )
            .is_err()
        );
        assert!(
            ok(
                "flagged",
                json!({"points": [{"reason": "missed", "mm": [1, 2]}]}),
                None
            )
            .is_err()
        );
        // a note's reason needs a note
        assert!(ok("flagged", json!({"reasons": ["other"]}), None).is_err());
        assert!(ok("flagged", json!({"reasons": ["other"]}), Some("a ghost")).is_ok());
        assert!(ok("flagged", json!({"reasons": ["nope"]}), None).is_err());
        assert!(ok("rejected", json!({"reasons": ["under"]}), None).is_ok());
        assert!(
            ok(
                "rejected",
                json!({"parts": [{"label": 1, "reason": "under"}]}),
                None
            )
            .is_err()
        );
        assert!(ok("maybe", json!({}), None).is_err());
        assert!(ok("accepted", json!({"seen": [1]}), Some(&"x".repeat(2001))).is_err());
        let fifty_one: Vec<Value> = (0..51)
            .map(|i| json!({"reason": "missed", "mm": [i, 0, 0]}))
            .collect();
        assert!(ok("flagged", json!({"points": fifty_one}), None).is_err());
        assert!(
            ok(
                "flagged",
                json!({"parts": [{"label": 1, "reason": "under"}, {"label": 1, "reason": "over"}]}),
                None
            )
            .is_err()
        );
        assert!(ok("accepted", json!({"looked": true}), None).is_err());
        // at part grain a rejection is "not a lesion" and needs nothing
        assert!(fits("part", &r, Some("rejected"), None, None).is_ok());
        assert!(
            fits(
                "part",
                &r,
                Some("rejected"),
                Some(&json!({"reasons": ["under"]})),
                None
            )
            .is_err()
        );
    }

    fn answer(id: i64, item: i64, who: &str, v: &str, form: Value) -> Answer {
        Answer {
            id,
            campaign_id: 1,
            item_id: item,
            assignment_id: id,
            principal: who.into(),
            role: "rater".into(),
            round: 1,
            author_kind: "person".into(),
            value: Some(v.into()),
            form: Some(form),
            derivative_id: None,
            why: None,
            actor_detail: Value::Null,
            answered_at: String::new(),
            model_id: None,
            seconds: Some(3.0),
            suggested: None,
            changed: None,
            via: Some("claim".into()),
            unsure: false,
            derived: None,
            suggested_by: None,
            supersedes_id: None,
            superseded_by: None,
            choices: None,
        }
    }

    #[test]
    fn agreed_flags_are_the_union_and_reasons_are_compared_as_sets() {
        let a = answer(
            1,
            9,
            "a",
            "flagged",
            json!({"parts": [{"label": 7, "reason": "under"}], "seen": [1, 7]}),
        );
        let b = answer(
            2,
            9,
            "b",
            "flagged",
            json!({"parts": [{"label": 12, "reason": "not_lesion"}], "points": [{"reason": "missed", "mm": [1, 2, 3]}], "seen": [12]}),
        );
        let o = outcome(&[a.clone(), b.clone()]);
        assert_eq!(o["value"], "flagged");
        assert_eq!(
            o["form"]["parts"],
            json!([{"label": 7, "reason": "under"}, {"label": 12, "reason": "not_lesion"}])
        );
        assert_eq!(o["form"]["seen"], json!([1, 7, 12]));
        assert_eq!(o["form"]["points"].as_array().unwrap().len(), 1);
        assert_eq!(o["answers"], json!([1, 2]));
        let overlap = reasons_overlap(&[a.clone(), b.clone()], 2);
        assert_eq!(overlap["items"], 1);
        // {under} against {not_lesion, missed}: nothing shared
        assert_eq!(overlap["jaccard"], 0.0);
        let same = answer(
            3,
            9,
            "c",
            "flagged",
            json!({"parts": [{"label": 3, "reason": "under"}]}),
        );
        assert_eq!(reasons_overlap(&[a.clone(), same], 2)["jaccard"], 1.0);
        let t = tally(&[&a, &b]);
        assert_eq!(t["verdicts"]["flagged"], 2);
        assert_eq!(t["reasons"]["under"], 1);
        assert_eq!(t["reasons"]["missed"], 1);
    }

    /// The T33 pipe test: two raters flag the same part with different
    /// reasons. The union names it once, its reasons joined; an outcome
    /// made before that (the part once per reason) is read the same way,
    /// and one that cannot be read is an error, never an empty form.
    #[test]
    fn a_part_several_raters_flag_is_one_part_with_its_reasons_joined() {
        let a = answer(
            1,
            9,
            "a",
            "flagged",
            json!({"parts": [{"label": 47, "reason": "under"}, {"label": 18, "reason": "over"}], "seen": [18, 47]}),
        );
        let b = answer(
            2,
            9,
            "b",
            "flagged",
            json!({"parts": [{"label": 47, "reason": "not_lesion"}, {"label": 18, "reason": "over"}]}),
        );
        let c = answer(
            3,
            9,
            "c",
            "flagged",
            json!({"parts": [{"label": 47, "reason": "under"}]}),
        );
        let o = outcome(&[a, b, c]);
        assert_eq!(
            o["form"]["parts"],
            json!([{"label": 18, "reason": "over"}, {"label": 47, "reason": "under, not_lesion"}])
        );
        let f = outcome_form(&o["form"]).unwrap();
        assert_eq!(
            f.parts,
            vec![
                (18, "over".to_string()),
                (47, "under, not_lesion".to_string())
            ]
        );
        assert_eq!(f.all_reasons(), vec!["over", "under", "not_lesion"]);
        assert_eq!(
            f.every_reason().into_iter().collect::<Vec<_>>(),
            vec!["not_lesion", "over", "under"]
        );
        let old = json!({"parts": [
            {"label": 18, "reason": "over"},
            {"label": 47, "reason": "not_lesion"},
            {"label": 47, "reason": "under"},
            {"label": 47, "reason": "not_lesion"},
        ]});
        assert!(
            form_of(Some(&old))
                .unwrap_err()
                .to_string()
                .contains("twice")
        );
        assert_eq!(
            outcome_form(&old).unwrap().parts,
            vec![
                (18, "over".to_string()),
                (47, "not_lesion, under".to_string())
            ]
        );
        assert_eq!(outcome_form(&Value::Null).unwrap(), Form::default());
        for bad in [
            json!({"parts": [{"label": 47}]}),
            json!({"parts": "47"}),
            json!({"verdict": "flagged"}),
            json!("flagged"),
        ] {
            assert!(outcome_form(&bad).is_err(), "{bad}");
        }
        assert_eq!(
            reasons_in(" under, not_lesion ,").collect::<Vec<_>>(),
            vec!["under", "not_lesion"]
        );
    }

    #[test]
    fn a_decision_s_why_is_its_reasons_then_the_note() {
        assert_eq!(why_of(&[], None), None);
        assert_eq!(why_of(&["under".into()], None).as_deref(), Some("under"));
        assert_eq!(
            why_of(&["under".into(), "over".into()], Some(" see slice 4 ")).as_deref(),
            Some("under, over: see slice 4")
        );
    }
}
