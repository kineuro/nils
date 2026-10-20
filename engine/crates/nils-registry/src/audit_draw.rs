// SPDX-License-Identifier: AGPL-3.0-only

//! The blind audit of accepted scans (record 55 D7, record 56 section 4.2;
//! spec §11.5).
//!
//! Every round, a random share (1 % by default, [`DEFAULT_SHARE`]) of the
//! round's accepted stacks is drawn by a seeded draw keyed by DICOM
//! identity, the study and series instance UIDs and the stack's key in its
//! series, so the same seed over the same frame draws the same stacks in any
//! registry ([`draw`]). The draw records its round, its seed and its frame
//! (how many stacks, and the sha256 of their identities in order), and the
//! rules' answer each drawn stack was accepted with.
//!
//! The drawn stacks are read blind in an audit campaign (purpose `audit`,
//! suggesting nothing, closing into nothing): its answers are marked never
//! to train. Its raters are agents outside the engine, and their
//! disagreements are settled by an agent too (ruling 13 of record 55); no
//! person reads. From the settled answers [`grade`] computes the round's
//! accepted error and its exact one-sided Clopper-Pearson bound, kept on
//! the draw beside its round.
//!
//! Record 58 P2 (Wave 7b spec §13): the monthly audit draws a fixed
//! number of stacks (150) from those accepted since the last draw
//! ([`Size::Count`], [`choose_count`]); the raters are two local models
//! reading through the campaign doors, and a person settles in verify mode
//! every stack where a rater's answer differs from the accepted answer and
//! a seeded share (10 %) of the stacks where both agree with it
//! ([`to_settle`]). A disagreement between the raters alone decides
//! nothing. The settled answers are the round's, and a stack not settled
//! stands as the raters' agreement with the accepted answer
//! ([`settled_reads`]). The grade counts by vendor and acquisition year and
//! holds the registered rule (at most [`MAX_WRONG`] wrong); rounds pool
//! ([`pool`]).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The share of a round's accepted stacks an audit reads (spec §18).
pub const DEFAULT_SHARE: f64 = 0.01;

/// The share of the stacks where both raters agree with the accepted
/// answer that a person settles beside every difference (spec §13.3).
pub const SETTLE_SHARE: f64 = 0.10;

/// The registered rule of a round (record 58 P2, spec §20): it passes with
/// at most one wrong.
pub const MAX_WRONG: u64 = 1;

/// The monthly audit's count (spec §20).
pub const MONTHLY_COUNT: usize = 150;

/// The confidence of the bound: 95 %, one-sided (a 1 % claim needs 299
/// accepted stacks read with no error).
pub const CONFIDENCE: f64 = 0.95;

/// One stack of a round's frame: its id now, its DICOM identity, and the
/// rules' answer it was accepted with, by axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Framed {
    pub stack_id: i64,
    pub study_uid: String,
    pub series_uid: String,
    pub stack_key: String,
    pub accepted: Value,
    /// Record 58 P2: its vendor (the maker's name folded to one word) and
    /// its acquisition year, which the round's report counts by; none
    /// where the registry cannot tell.
    pub vendor: Option<String>,
    pub year: Option<i64>,
}

impl Framed {
    fn identity(&self) -> String {
        format!("{}|{}|{}", self.study_uid, self.series_uid, self.stack_key)
    }
}

/// One draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub id: i64,
    pub round_id: i64,
    pub seed: String,
    pub share: f64,
    pub frame_count: i64,
    pub frame_digest: String,
    pub drawn: i64,
    pub campaign_id: Option<i64>,
    pub created_by: String,
    pub created_at: String,
    pub grade: Value,
    pub graded_at: Option<String>,
    pub graded_by: Option<String>,
    /// Record 58 P2: the count asked of a draw by count (none for a draw by
    /// share) and the time its frame's stacks were accepted after.
    pub count: Option<i64>,
    pub since: Option<String>,
    /// The verify campaign a person settles in, with the seed and share of
    /// the agreements it holds.
    pub settle_campaign_id: Option<i64>,
    pub settle_seed: Option<String>,
    pub settle_share: Option<f64>,
    /// The rule review a round over the rule opened.
    pub review_campaign_id: Option<i64>,
}

impl Draw {
    /// Whether the draw has been settled: in a campaign, or, where no rater
    /// differed and the seeded share of the agreements was none, with no
    /// campaign at all (the seed and share are kept, the campaign is not).
    pub fn settled(&self) -> bool {
        self.settle_campaign_id.is_some() || self.settle_seed.is_some()
    }

    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "round_id": self.round_id, "seed": self.seed, "share": self.share,
            "count": self.count, "since": self.since,
            "frame": {"stacks": self.frame_count, "digest": self.frame_digest},
            "drawn": self.drawn, "campaign_id": self.campaign_id,
            "campaigns": {"raters": self.campaign_id, "settle": self.settle_campaign_id,
                          "rule_review": self.review_campaign_id},
            "settle": self.settled().then(|| json!({
                "seed": self.settle_seed, "share": self.settle_share,
            })),
            "created_by": self.created_by, "created_at": self.created_at,
            "grade": self.grade, "graded_at": self.graded_at, "graded_by": self.graded_by,
            "trains": false,
        })
    }
}

/// How many a draw takes: a share of its frame (Wave 7a, a round's 1 %) or
/// a fixed count (Wave 7b, the monthly 150).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Share(f64),
    Count(usize),
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// The order a seed draws a frame in: by the sha256 of the seed and each
/// stack's DICOM identity, never by a registry id.
fn rank(seed: &str, f: &Framed) -> String {
    sha256_hex(format!("{seed}:audit:{}", f.identity()).as_bytes())
}

/// How many stacks a share of a frame is: at least one of a frame that
/// holds any, rounded up.
pub fn how_many(share: f64, frame: usize) -> usize {
    if frame == 0 {
        return 0;
    }
    ((share * frame as f64).ceil() as usize).clamp(1, frame)
}

/// A frame in the order a seed draws it, keyed by `what` and each stack's
/// DICOM identity alone.
fn ranked<'a>(seed: &str, what: &str, frame: &'a [Framed]) -> Vec<&'a Framed> {
    let mut ranked: Vec<(String, &Framed)> = frame
        .iter()
        .map(|f| {
            let r = if what == "audit" {
                rank(seed, f)
            } else {
                sha256_hex(format!("{seed}:{what}:{}", f.identity()).as_bytes())
            };
            (r, f)
        })
        .collect();
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.identity().cmp(&b.1.identity()))
    });
    ranked.into_iter().map(|(_, f)| f).collect()
}

/// The stacks a seed draws from a frame, in the order drawn.
pub fn choose<'a>(seed: &str, share: f64, frame: &'a [Framed]) -> Vec<&'a Framed> {
    let mut out = ranked(seed, "audit", frame);
    out.truncate(how_many(share, frame.len()));
    out
}

/// Record 58 P2 (spec §13.1): a fixed number of stacks a seed draws from a
/// frame, in the order drawn; the whole frame where it holds fewer. The
/// same order as [`choose`], so a seed draws the same stacks in any
/// registry whatever their ids.
pub fn choose_count<'a>(seed: &str, count: usize, frame: &'a [Framed]) -> Vec<&'a Framed> {
    let mut out = ranked(seed, "audit", frame);
    out.truncate(count.min(frame.len()));
    out
}

/// The frame's digest: the sha256 of its identities, sorted, one a line.
pub fn frame_digest(frame: &[Framed]) -> String {
    let mut ids: Vec<String> = frame.iter().map(Framed::identity).collect();
    ids.sort();
    let mut text = ids.join("\n");
    text.push('\n');
    format!("sha256:{}", sha256_hex(text.as_bytes()))
}

/// Draw and keep a round's audit sample, by a share of its frame or a
/// count, with the time its frame's stacks were accepted after. Refused
/// for a share outside (0, 1], a count of none or an empty frame.
pub fn draw(
    store: &mut Store,
    round_id: i64,
    seed: &str,
    size: Size,
    since: Option<&str>,
    frame: &[Framed],
    who: &str,
) -> Result<(Draw, Vec<Framed>), String> {
    match size {
        Size::Share(share) if !(share > 0.0 && share <= 1.0) => {
            return Err(format!("share: above 0 and at most 1, not {share}"));
        }
        Size::Count(0) => return Err("count: at least one stack".into()),
        _ => {}
    }
    if seed.trim().is_empty() {
        return Err("seed: a word the draw is made from, kept with it".into());
    }
    if frame.is_empty() {
        return Err(match since {
            Some(t) => format!(
                "no stack was accepted since {t} to draw from: none was sorted under the round's rules version after it and left without a question"
            ),
            None => "the round has no accepted stacks to draw from: none was sorted under its rules version and left without a question".into(),
        });
    }
    let (chosen, share, count): (Vec<Framed>, f64, Option<i64>) = match size {
        Size::Share(share) => (
            choose(seed, share, frame).into_iter().cloned().collect(),
            share,
            None,
        ),
        Size::Count(n) => {
            let c: Vec<Framed> = choose_count(seed, n, frame).into_iter().cloned().collect();
            let share = c.len() as f64 / frame.len() as f64;
            (c, share, Some(n as i64))
        }
    };
    let digest = frame_digest(frame);
    let now = now_iso();
    let e = |e: StoreError| e.to_string();
    store.begin().map_err(e)?;
    let written = (|| -> Result<i64, StoreError> {
        let id = store
            .insert(
                &Insert::new(
                    table("audit_draw"),
                    &[
                        "round_id",
                        "seed",
                        "share",
                        "frame_count",
                        "frame_digest",
                        "drawn",
                        "created_by",
                        "created_at",
                        "count",
                        "since",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::Int(round_id),
                    Param::from(seed),
                    Param::Double(share),
                    Param::Int(frame.len() as i64),
                    Param::from(digest.as_str()),
                    Param::Int(chosen.len() as i64),
                    Param::from(who),
                    Param::from(now.as_str()),
                    count.map_or(Param::Null, Param::Int),
                    since.map_or(Param::Null, Param::from),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the draw was not written back".into()))?
            .int(0)?;
        let rows: Vec<Vec<Param>> = chosen
            .iter()
            .map(|f| {
                vec![
                    Param::Int(id),
                    Param::Int(f.stack_id),
                    Param::from(f.study_uid.as_str()),
                    Param::from(f.series_uid.as_str()),
                    Param::from(f.stack_key.as_str()),
                    Param::from(f.accepted.to_string()),
                    f.vendor.as_deref().map_or(Param::Null, Param::from),
                    f.year.map_or(Param::Null, Param::Int),
                ]
            })
            .collect();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(
                    table("audit_draw_stack"),
                    &[
                        "draw_id",
                        "stack_id",
                        "study_uid",
                        "series_uid",
                        "stack_key",
                        "accepted",
                        "vendor",
                        "year",
                    ],
                ),
                chunk,
            )?;
        }
        Ok(id)
    })();
    let id = match written {
        Ok(id) => {
            store.commit().map_err(e)?;
            id
        }
        Err(err) => {
            store.rollback().ok();
            return Err(err.to_string());
        }
    };
    let d = get(store, id).map_err(e)?.ok_or("the draw was not kept")?;
    Ok((d, chosen))
}

fn select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("audit_draw");
    format!(
        "SELECT id, round_id, seed, share, frame_count, frame_digest, drawn, campaign_id, created_by, {}, {}, {}, graded_by, count, {}, settle_campaign_id, settle_seed, settle_share, review_campaign_id FROM {}",
        d.text_of(t.column("created_at").expect("created_at")),
        d.text_of(t.column("grade").expect("grade")),
        d.text_of(t.column("graded_at").expect("graded_at")),
        d.text_of(t.column("since").expect("since")),
        store.qualified("audit_draw"),
    )
}

fn of_row(r: &Row) -> Result<Draw, StoreError> {
    Ok(Draw {
        id: r.int(0)?,
        round_id: r.int(1)?,
        seed: r.text(2)?.to_string(),
        share: r.double(3)?,
        frame_count: r.int(4)?,
        frame_digest: r.text(5)?.to_string(),
        drawn: r.int(6)?,
        campaign_id: r.opt_int(7)?,
        created_by: r.text(8)?.to_string(),
        created_at: r.text(9)?.to_string(),
        grade: r
            .opt_text(10)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null),
        graded_at: r.opt_text(11)?.map(str::to_string),
        graded_by: r.opt_text(12)?.map(str::to_string),
        count: r.opt_int(13)?,
        since: r.opt_text(14)?.map(str::to_string),
        settle_campaign_id: r.opt_int(15)?,
        settle_seed: r.opt_text(16)?.map(str::to_string),
        settle_share: r.opt_double(17)?,
        review_campaign_id: r.opt_int(18)?,
    })
}

/// The newest draw by count of a round, with when it was made: where the
/// round's next draw by count starts its frame when no time is given (spec
/// §13.1, "since the last draw"). A draw by share, or a draw of another
/// round, does not move it. The frame compares against the stored time
/// (see `Cutoff` in the door), never this text read back.
pub fn last_count_draw(store: &mut Store, round: i64) -> Result<Option<(i64, String)>, StoreError> {
    let d = store.dialect();
    let t = table("audit_draw");
    let sql = format!(
        "SELECT id, {} FROM {} WHERE round_id = {} AND count IS NOT NULL ORDER BY id DESC LIMIT 1",
        d.text_of(t.column("created_at").expect("created_at")),
        store.qualified("audit_draw"),
        d.param(1, Type::Int),
    );
    store
        .query_opt(&sql, &[Param::Int(round)])?
        .map(|r| Ok((r.int(0)?, r.text(1)?.to_string())))
        .transpose()
}

pub fn get(store: &mut Store, id: i64) -> Result<Option<Draw>, StoreError> {
    let sql = format!(
        "{} WHERE id = {}",
        select(store),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| of_row(&r))
        .transpose()
}

/// Every draw, newest first; a round's alone when one is given.
pub fn list(store: &mut Store, round: Option<i64>) -> Result<Vec<Draw>, StoreError> {
    let by = match round {
        Some(r) => format!(" WHERE round_id = {r}"),
        None => String::new(),
    };
    let sql = format!("{}{by} ORDER BY id DESC", select(store));
    store.query(&sql, &[])?.iter().map(of_row).collect()
}

/// The draw an audit campaign reads.
pub fn of_campaign(store: &mut Store, campaign: i64) -> Result<Option<Draw>, StoreError> {
    let sql = format!(
        "{} WHERE campaign_id = {}",
        select(store),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(campaign)])?
        .map(|r| of_row(&r))
        .transpose()
}

/// The stacks of a draw, in the order drawn.
pub fn stacks(store: &mut Store, draw: i64) -> Result<Vec<Framed>, StoreError> {
    let d = store.dialect();
    let t = table("audit_draw_stack");
    let sql = format!(
        "SELECT stack_id, study_uid, series_uid, stack_key, {}, vendor, year FROM {} WHERE draw_id = {} ORDER BY id",
        d.text_of(t.column("accepted").expect("accepted")),
        store.qualified("audit_draw_stack"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(draw)])?
        .iter()
        .map(|r| {
            Ok(Framed {
                stack_id: r.int(0)?,
                study_uid: r.text(1)?.to_string(),
                series_uid: r.text(2)?.to_string(),
                stack_key: r.text(3)?.to_string(),
                accepted: r
                    .opt_text(4)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null),
                vendor: r.opt_text(5)?.map(str::to_string),
                year: r.opt_int(6)?,
            })
        })
        .collect()
}

/// Tie a draw to the campaign that reads it.
pub fn set_campaign(store: &mut Store, draw: i64, campaign: i64) -> Result<(), StoreError> {
    store.update_by_id(
        table("audit_draw"),
        &[("campaign_id", Param::Int(campaign))],
        "id",
        draw,
    )?;
    Ok(())
}

/// Record 58 P2: tie a draw to the verify campaign its differences and a
/// seeded share of its agreements are settled in. With no campaign, the
/// draw is settled empty: every rater agreed and the share drawn none, so
/// nothing is left to a person and every stack stands as the raters'
/// agreement.
pub fn set_settle(
    store: &mut Store,
    draw: i64,
    campaign: Option<i64>,
    seed: &str,
    share: f64,
) -> Result<(), StoreError> {
    store.update_by_id(
        table("audit_draw"),
        &[
            (
                "settle_campaign_id",
                campaign.map_or(Param::Null, Param::Int),
            ),
            ("settle_seed", Param::from(seed)),
            ("settle_share", Param::Double(share)),
        ],
        "id",
        draw,
    )?;
    Ok(())
}

/// Record 58 P2: tie a draw to the rule review its round over the rule
/// opened.
pub fn set_review(store: &mut Store, draw: i64, campaign: i64) -> Result<(), StoreError> {
    store.update_by_id(
        table("audit_draw"),
        &[("review_campaign_id", Param::Int(campaign))],
        "id",
        draw,
    )?;
    Ok(())
}

// ------------------------------------------------------------ the settle

/// One rater's answer on a drawn stack, by axis.
pub type Answered = BTreeMap<String, Value>;

/// What goes to the settle (spec §13.3): every stack where a rater's
/// answer differs from the accepted answer on any audited axis, and the
/// seeded share of those where every rater agrees with it, each in the
/// order drawn; with the stacks whose raters have not all answered yet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToSettle {
    pub differ: Vec<i64>,
    /// How many stacks every rater agrees on with the accepted answer.
    pub agree: usize,
    /// The agreements the seed drew to be settled too.
    pub agreements_drawn: Vec<i64>,
    pub waiting: Vec<i64>,
}

impl ToSettle {
    /// The stacks the settle campaign holds, in the order drawn.
    pub fn stacks(&self, drawn: &[Framed]) -> Vec<i64> {
        let mut keep: std::collections::BTreeSet<i64> = self.differ.iter().copied().collect();
        keep.extend(self.agreements_drawn.iter().copied());
        drawn
            .iter()
            .map(|f| f.stack_id)
            .filter(|s| keep.contains(s))
            .collect()
    }
}

/// Whether an answer agrees with what was accepted on every axis the draw
/// keeps an accepted answer for (a cannot tell, or an axis left out, is a
/// difference).
pub fn agrees(accepted: &Value, answer: &Answered) -> bool {
    accepted.as_object().is_none_or(|m| {
        m.iter()
            .all(|(axis, v)| comparable(answer.get(axis).unwrap_or(&Value::Null)) == comparable(v))
    })
}

/// Which drawn stacks a person settles, from the raters' answers (stack to
/// each rater's answer), once `raters` raters have answered every stack.
/// The agreements are drawn by the sha256 of the seed and each stack's
/// DICOM identity: `share` of them, rounded up, none for a share of none.
pub fn to_settle(
    seed: &str,
    share: f64,
    drawn: &[Framed],
    answers: &BTreeMap<i64, Vec<Answered>>,
    raters: usize,
) -> ToSettle {
    let mut out = ToSettle::default();
    let mut agreeing: Vec<Framed> = Vec::new();
    for f in drawn {
        let list = answers.get(&f.stack_id).map(Vec::as_slice).unwrap_or(&[]);
        if list.len() < raters.max(1) {
            out.waiting.push(f.stack_id);
        } else if list.iter().all(|a| agrees(&f.accepted, a)) {
            agreeing.push(f.clone());
        } else {
            out.differ.push(f.stack_id);
        }
    }
    out.agree = agreeing.len();
    let n = if share > 0.0 {
        how_many(share.min(1.0), agreeing.len())
    } else {
        0
    };
    let chosen: std::collections::BTreeSet<i64> = ranked(seed, "settle", &agreeing)
        .into_iter()
        .take(n)
        .map(|f| f.stack_id)
        .collect();
    out.agreements_drawn = agreeing
        .iter()
        .map(|f| f.stack_id)
        .filter(|s| chosen.contains(s))
        .collect();
    out
}

/// Each rater's answer on a stack: the rater, the answer's text, and the
/// answer by axis.
pub type RatersAnswers = BTreeMap<i64, Vec<(String, String, Answered)>>;

/// The raters' answers now in a draw's raters' campaign, by stack: each
/// rater's latest answer (never an adjudicator's), read by axis.
pub fn raters_answers(
    store: &mut Store,
    campaign: i64,
) -> Result<RatersAnswers, crate::campaign::Error> {
    let stack_of: BTreeMap<i64, i64> = crate::campaign::items(store, campaign)?
        .into_iter()
        .filter_map(|i| i.stack_id.map(|s| (i.id, s)))
        .collect();
    let mut latest: BTreeMap<(i64, String), (String, Answered)> = BTreeMap::new();
    for a in crate::campaign::current_answers(store, campaign)? {
        if a.role != "rater" {
            continue;
        }
        let (Some(stack), Some(text)) = (stack_of.get(&a.item_id), a.value.as_ref()) else {
            continue;
        };
        let read: Answered = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m.into_iter().collect(),
            _ => continue,
        };
        latest.insert((*stack, a.principal.clone()), (text.clone(), read));
    }
    let mut out: RatersAnswers = BTreeMap::new();
    for ((stack, who), (text, read)) in latest {
        out.entry(stack).or_default().push((who, text, read));
    }
    Ok(out)
}

/// The settled reads of a settled draw (spec §13.3): a stack the settle
/// campaign holds reads as its settled outcome, with the planted axis left
/// out where the person kept a planted value (a value known to be wrong;
/// the stack counts as not settled where nothing else is left); a stack it
/// does not hold stands as the raters' agreement with the accepted answer.
/// With what the grade says of the settle.
pub fn settled_reads(store: &mut Store, d: &Draw) -> Result<(Vec<Read>, Value), String> {
    let e = |e: &dyn std::fmt::Display| e.to_string();
    if !d.settled() {
        return Err(format!("audit draw {} is not settled", d.id));
    }
    let drawn = stacks(store, d.id).map_err(|x| e(&x))?;
    let Some(campaign) = d.settle_campaign_id else {
        // settled empty: no person had anything to settle, every stack stands
        let reads: Vec<Read> = drawn
            .iter()
            .map(|f| Read {
                stack_id: f.stack_id,
                settled: f
                    .accepted
                    .as_object()
                    .map(|m| m.clone().into_iter().collect())
                    .unwrap_or_default(),
                stood: true,
            })
            .collect();
        return Ok((
            reads,
            json!({
                "campaign_id": Value::Null, "closed": true, "items": 0, "settled": 0,
                "stood": drawn.len(), "planted_kept": 0,
                "seed": d.settle_seed, "share": d.settle_share,
            }),
        ));
    };
    let c = crate::campaign::get(store, campaign)
        .map_err(|x| e(&x))?
        .ok_or_else(|| format!("no campaign {campaign}"))?;
    let question = c.question().map_err(|x| e(&x))?;
    let items = crate::campaign::items(store, campaign).map_err(|x| e(&x))?;
    let mut by_stack: BTreeMap<i64, Answered> = BTreeMap::new();
    for l in crate::labels::campaign_labels(store, campaign, crate::labels::Of::Outcomes)
        .map_err(|x| e(&x))?
    {
        if let Some(s) = l.stack_id {
            by_stack
                .entry(s)
                .or_default()
                .insert(l.what.clone(), l.value.map_or(Value::Null, Value::String));
        }
    }
    // every plant of the campaign, read once
    let plants: BTreeMap<i64, crate::verify::Planted> = crate::verify::planted(store, campaign)
        .map_err(|x| e(&x))?
        .into_iter()
        .map(|p| (p.item_id, p))
        .collect();
    let mut held: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    let mut kept_plants = 0u64;
    for it in &items {
        let Some(s) = it.stack_id else { continue };
        held.insert(s);
        let settled = it.outcome["value"].as_str();
        if settled.is_none() {
            by_stack.remove(&s);
            continue;
        }
        if let Some(axis) = plants
            .get(&it.id)
            .and_then(|p| crate::verify::kept_plant_of(&question, p, settled))
        {
            kept_plants += 1;
            if let Some(m) = by_stack.get_mut(&s) {
                m.remove(&axis);
            }
        }
    }
    let audited: Vec<String> = drawn
        .first()
        .and_then(|f| f.accepted.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let mut reads = Vec::new();
    let (mut settled_n, mut stood) = (0u64, 0u64);
    for f in &drawn {
        if held.contains(&f.stack_id) {
            let Some(mut m) = by_stack.remove(&f.stack_id) else {
                continue;
            };
            // the axes the audit asked, never what the pack derives
            if !audited.is_empty() {
                m.retain(|k, _| audited.contains(k));
            }
            if m.is_empty() {
                continue;
            }
            settled_n += 1;
            reads.push(Read {
                stack_id: f.stack_id,
                settled: m,
                stood: false,
            });
        } else {
            stood += 1;
            reads.push(Read {
                stack_id: f.stack_id,
                settled: f
                    .accepted
                    .as_object()
                    .map(|m| m.clone().into_iter().collect())
                    .unwrap_or_default(),
                stood: true,
            });
        }
    }
    Ok((
        reads,
        json!({
            "campaign_id": campaign, "closed": c.status == "closed",
            "items": items.len(), "settled": settled_n,
            "stood": stood, "planted_kept": kept_plants,
            "seed": d.settle_seed, "share": d.settle_share,
        }),
    ))
}

// ------------------------------------------------------------ the grade

/// The probability of at most `k` successes in `n` trials of probability
/// `p`, summed in log space.
fn binomial_cdf(k: u64, n: u64, p: f64) -> f64 {
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return if k >= n { 1.0 } else { 0.0 };
    }
    let (lp, lq) = (p.ln(), (1.0 - p).ln());
    let mut log_choose = 0.0f64; // ln C(n, 0)
    let mut sum = 0.0f64;
    for i in 0..=k.min(n) {
        if i > 0 {
            log_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        sum += (log_choose + i as f64 * lp + (n - i) as f64 * lq).exp();
    }
    sum.min(1.0)
}

/// The exact one-sided Clopper-Pearson upper bound on a share wrong, with
/// `k` wrong of `n`, at `confidence`: the `p` at which seeing at most `k`
/// wrong has probability `1 - confidence`. One for `n` of none.
pub fn clopper_pearson_upper(k: u64, n: u64, confidence: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    if k >= n {
        return 1.0;
    }
    let alpha = 1.0 - confidence;
    let (mut lo, mut hi) = (k as f64 / n as f64, 1.0f64);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if binomial_cdf(k, n, mid) > alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// A value as answers and the rules' answers are compared: its words in
/// order, a list or a comma list alike; none for no value.
pub fn comparable(v: &Value) -> Option<String> {
    let words: Vec<String> = match v {
        Value::Null => Vec::new(),
        Value::Array(list) => list
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Value::String(s) if s.trim_start().starts_with('[') => {
            serde_json::from_str::<Vec<String>>(s).unwrap_or_else(|_| vec![s.clone()])
        }
        Value::String(s) => s.split(',').map(|w| w.trim().to_string()).collect(),
        other => vec![other.to_string()],
    };
    let mut words: Vec<String> = words.into_iter().filter(|w| !w.is_empty()).collect();
    words.sort();
    words.dedup();
    (!words.is_empty()).then(|| words.join(","))
}

/// The words a settled answer uses for "cannot tell".
pub const CANT_TELL: [&str; 2] = ["cant_tell", "can't tell"];

/// One stack's settled answer, by axis, against what was accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub stack_id: i64,
    pub settled: BTreeMap<String, Value>,
    /// Record 58 P2: the read stands as the raters' agreement with the
    /// accepted answer, which no person settled.
    pub stood: bool,
}

/// The registered rule over `wrong` of a round or a pool: passed with at
/// most [`MAX_WRONG`] wrong, failed over it, and not yet known while a
/// stack is still to be settled and the rule still holds.
pub fn rule(wrong: u64, not_settled: u64) -> Value {
    let passed = if wrong > MAX_WRONG {
        json!(false)
    } else if not_settled > 0 {
        Value::Null
    } else {
        json!(true)
    };
    json!({"max_wrong": MAX_WRONG, "passed": passed})
}

fn count_in(map: &mut serde_json::Map<String, Value>, key: &str, field: &str, by: u64) {
    let e = map
        .entry(key.to_string())
        .or_insert_with(|| json!({"drawn": 0, "read": 0, "wrong": 0}));
    e[field] = json!(e[field].as_u64().unwrap_or(0) + by);
}

/// Grade a draw from its settled answers: a stack is wrong when any axis
/// its settled answer names differs from the rules' accepted answer; a
/// stack whose settled answer says it cannot tell on an axis, or that has
/// no settled answer yet, is not graded and is counted apart. Kept on the
/// draw, with who graded it and when. Record 58 P2: counted by vendor and
/// acquisition year (`unknown` where the registry cannot tell), with the
/// registered rule, the wrong stacks, and `settle` (what the settle says,
/// null for a draw settled by an adjudicator as in Wave 7a).
pub fn grade(
    store: &mut Store,
    draw: &Draw,
    reads: &[Read],
    campaign_closed: bool,
    settle: Value,
    who: &str,
) -> Result<Value, StoreError> {
    let drawn = stacks(store, draw.id)?;
    let by_stack: BTreeMap<i64, &Read> = reads.iter().map(|r| (r.stack_id, r)).collect();
    let (mut n, mut k) = (0u64, 0u64);
    let (mut unsettled, mut cant_tell) = (0u64, 0u64);
    let mut wrong_axes: BTreeMap<String, u64> = BTreeMap::new();
    let mut vendors = serde_json::Map::new();
    let mut years = serde_json::Map::new();
    let mut wrong_stacks: Vec<Value> = Vec::new();
    for f in &drawn {
        let vendor = f.vendor.clone().unwrap_or_else(|| "unknown".into());
        let year = f
            .year
            .map_or_else(|| "unknown".to_string(), |y| y.to_string());
        count_in(&mut vendors, &vendor, "drawn", 1);
        count_in(&mut years, &year, "drawn", 1);
        let Some(read) = by_stack.get(&f.stack_id) else {
            unsettled += 1;
            continue;
        };
        if read
            .settled
            .values()
            .any(|v| v.as_str().is_some_and(|s| CANT_TELL.contains(&s)))
        {
            cant_tell += 1;
            continue;
        }
        n += 1;
        count_in(&mut vendors, &vendor, "read", 1);
        count_in(&mut years, &year, "read", 1);
        let mut axes: Vec<&str> = Vec::new();
        for (axis, v) in &read.settled {
            if comparable(v) != comparable(&f.accepted[axis]) {
                axes.push(axis);
                *wrong_axes.entry(axis.clone()).or_default() += 1;
            }
        }
        if !axes.is_empty() {
            k += 1;
            count_in(&mut vendors, &vendor, "wrong", 1);
            count_in(&mut years, &year, "wrong", 1);
            wrong_stacks.push(json!({"stack_id": f.stack_id, "axes": axes}));
        }
    }
    let doc = json!({
        "read": n, "wrong": k,
        "error": if n > 0 { json!(k as f64 / n as f64) } else { Value::Null },
        "bound": clopper_pearson_upper(k, n, CONFIDENCE),
        "confidence": CONFIDENCE, "method": "exact one-sided Clopper-Pearson",
        "wrong_by_axis": wrong_axes, "not_settled": unsettled, "cant_tell": cant_tell,
        "drawn": drawn.len(), "campaign_closed": campaign_closed,
        "by_vendor": vendors, "by_year": years,
        "rule": rule(k, unsettled),
        "wrong_stacks": wrong_stacks,
        "settle": settle,
        "trains": false,
    });
    store.update_by_id(
        table("audit_draw"),
        &[
            ("grade", Param::from(doc.to_string())),
            ("graded_at", Param::from(now_iso().as_str())),
            ("graded_by", Param::from(who)),
        ],
        "id",
        draw.id,
    )?;
    Ok(doc)
}

/// Record 58 P2 (spec §13.4): several graded rounds pooled, six of 150 to
/// 900 stacks, with the same fields as a round's grade: the sums, the
/// exact bound over the pool, the counts by vendor and year, and the rule
/// over the pool, with the rounds over it by draw. Refused for a draw not
/// graded, or one named twice.
pub fn pool(draws: &[Draw]) -> Result<Value, String> {
    if draws.is_empty() {
        return Err("ids: the graded draws to pool, by id".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let (mut n, mut k, mut unsettled, mut cant_tell, mut drawn) = (0u64, 0, 0, 0, 0);
    let mut wrong_axes: BTreeMap<String, u64> = BTreeMap::new();
    let mut vendors = serde_json::Map::new();
    let mut years = serde_json::Map::new();
    let (mut over, mut unknown) = (Vec::new(), Vec::new());
    for d in draws {
        if !seen.insert(d.id) {
            return Err(format!("audit draw {} is named twice", d.id));
        }
        let g = &d.grade;
        if g.is_null() {
            return Err(format!("audit draw {} is not graded yet", d.id));
        }
        let u = |f: &str| g[f].as_u64().unwrap_or(0);
        n += u("read");
        k += u("wrong");
        unsettled += u("not_settled");
        cant_tell += u("cant_tell");
        drawn += u("drawn");
        for (a, c) in g["wrong_by_axis"].as_object().into_iter().flatten() {
            *wrong_axes.entry(a.clone()).or_default() += c.as_u64().unwrap_or(0);
        }
        for (into, from) in [(&mut vendors, &g["by_vendor"]), (&mut years, &g["by_year"])] {
            for (key, c) in from.as_object().into_iter().flatten() {
                for field in ["drawn", "read", "wrong"] {
                    count_in(into, key, field, c[field].as_u64().unwrap_or(0));
                }
            }
        }
        // a round graded before the rule: its own wrong count says
        let passed = match &g["rule"]["passed"] {
            Value::Bool(b) => Some(*b),
            Value::Null if g["rule"].is_null() => {
                (u("not_settled") == 0 || u("wrong") > MAX_WRONG).then(|| u("wrong") <= MAX_WRONG)
            }
            _ => None,
        };
        match passed {
            Some(false) => over.push(d.id),
            None => unknown.push(d.id),
            Some(true) => {}
        }
    }
    let mut rule = rule(k, unsettled);
    rule["rounds"] = json!(draws.len());
    rule["rounds_over"] = json!(over);
    rule["rounds_not_known"] = json!(unknown);
    Ok(json!({
        "draws": draws.iter().map(|d| d.id).collect::<Vec<_>>(),
        "read": n, "wrong": k,
        "error": if n > 0 { json!(k as f64 / n as f64) } else { Value::Null },
        "bound": clopper_pearson_upper(k, n, CONFIDENCE),
        "confidence": CONFIDENCE, "method": "exact one-sided Clopper-Pearson",
        "wrong_by_axis": wrong_axes, "not_settled": unsettled, "cant_tell": cant_tell,
        "drawn": drawn, "by_vendor": vendors, "by_year": years,
        "rule": rule,
        "trains": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(i: i64) -> Framed {
        Framed {
            stack_id: i,
            study_uid: format!("1.2.{}", i / 10),
            series_uid: format!("1.2.{}.{}", i / 10, i),
            stack_key: "0".into(),
            accepted: json!({"base": "T1w"}),
            vendor: None,
            year: None,
        }
    }

    #[test]
    fn the_bound_is_the_exact_clopper_pearson() {
        // a 1 % claim needs 299 with none wrong, and about 474 with one
        assert!(clopper_pearson_upper(0, 299, 0.95) < 0.01);
        assert!(clopper_pearson_upper(0, 298, 0.95) > 0.01);
        assert!(clopper_pearson_upper(1, 474, 0.95) < 0.01);
        assert!(clopper_pearson_upper(1, 470, 0.95) > 0.01);
        // 0 of n: 1 - 0.05^(1/n)
        let n = 100u64;
        let exact = 1.0 - 0.05f64.powf(1.0 / n as f64);
        assert!((clopper_pearson_upper(0, n, 0.95) - exact).abs() < 1e-9);
        assert_eq!(clopper_pearson_upper(0, 0, 0.95), 1.0);
    }

    #[test]
    fn a_draw_is_keyed_by_dicom_identity_and_its_seed() {
        let frame: Vec<Framed> = (1..=1000).map(framed).collect();
        let a = choose("round-1", 0.01, &frame);
        assert_eq!(a.len(), 10);
        // the same seed over the same identities draws the same stacks,
        // whatever the registry ids and the order they come in
        let mut moved: Vec<Framed> = frame
            .iter()
            .rev()
            .map(|f| Framed {
                stack_id: f.stack_id + 50_000,
                ..f.clone()
            })
            .collect();
        moved.rotate_left(17);
        let b = choose("round-1", 0.01, &moved);
        let ids = |v: &[&Framed]| v.iter().map(|f| f.identity()).collect::<Vec<_>>();
        assert_eq!(ids(&a), ids(&b));
        assert_eq!(frame_digest(&frame), frame_digest(&moved));
        // another seed draws others
        let c = choose("round-2", 0.01, &frame);
        assert_ne!(ids(&a), ids(&c));
        assert_eq!(how_many(0.01, 50), 1);
        assert_eq!(how_many(0.01, 0), 0);
        assert_eq!(how_many(0.01, 1001), 11);
    }

    #[test]
    fn a_draw_by_count_takes_the_same_stacks_by_seed_and_identity() {
        let frame: Vec<Framed> = (1..=1000).map(framed).collect();
        let a = choose_count("month-1", MONTHLY_COUNT, &frame);
        assert_eq!(a.len(), 150);
        let mut moved: Vec<Framed> = frame
            .iter()
            .map(|f| Framed {
                stack_id: f.stack_id * 7 + 3,
                ..f.clone()
            })
            .collect();
        moved.reverse();
        let b = choose_count("month-1", MONTHLY_COUNT, &moved);
        let ids = |v: &[&Framed]| v.iter().map(|f| f.identity()).collect::<Vec<_>>();
        assert_eq!(ids(&a), ids(&b));
        // the count's order is the share's: the first ten of the 150 are
        // the 1 % a share would draw
        assert_eq!(ids(&a[..10]), ids(&choose("month-1", 0.01, &frame)));
        assert_ne!(ids(&a), ids(&choose_count("month-2", 150, &frame)));
        // a frame smaller than the count is drawn whole
        assert_eq!(choose_count("month-1", 150, &frame[..40]).len(), 40);
    }

    fn answer(base: &str) -> Answered {
        [("base".to_string(), json!(base))].into_iter().collect()
    }

    #[test]
    fn the_settle_holds_every_difference_and_a_seeded_share_of_the_agreements() {
        let frame: Vec<Framed> = (1..=150).map(framed).collect();
        let mut answers: BTreeMap<i64, Vec<Answered>> = BTreeMap::new();
        for f in &frame {
            let (a, b) = match f.stack_id {
                // one rater differs; both differ alike; both differ apart
                3 => ("T1w", "T2w"),
                9 => ("PDw", "PDw"),
                27 => ("T2w", "PDw"),
                _ => ("T1w", "T1w"),
            };
            answers.insert(f.stack_id, vec![answer(a), answer(b)]);
        }
        // a cannot tell is a difference too
        answers.insert(40, vec![answer("cant_tell"), answer("T1w")]);
        let s = to_settle("seed-x", SETTLE_SHARE, &frame, &answers, 2);
        assert_eq!(s.differ, vec![3, 9, 27, 40]);
        assert_eq!(s.agree, 146);
        assert_eq!(s.agreements_drawn.len(), 15, "10 % of 146, rounded up");
        assert!(s.agreements_drawn.iter().all(|x| !s.differ.contains(x)));
        assert!(s.waiting.is_empty());
        assert_eq!(s.stacks(&frame).len(), 19);
        // the same seed settles the same agreements; another seed others
        let again = to_settle("seed-x", SETTLE_SHARE, &frame, &answers, 2);
        assert_eq!(again, s);
        let other = to_settle("seed-y", SETTLE_SHARE, &frame, &answers, 2);
        assert_ne!(other.agreements_drawn, s.agreements_drawn);
        // a stack one rater has not read waits
        answers.get_mut(&5).unwrap().pop();
        let w = to_settle("seed-x", SETTLE_SHARE, &frame, &answers, 2);
        assert_eq!(w.waiting, vec![5]);
        // a share of none settles the differences alone
        assert!(
            to_settle("seed-x", 0.0, &frame, &answers, 2)
                .agreements_drawn
                .is_empty()
        );
    }

    fn graded(id: i64, read: u64, wrong: u64, vendor: &str) -> Draw {
        Draw {
            id,
            round_id: 1,
            seed: "s".into(),
            share: 0.1,
            frame_count: 1500,
            frame_digest: "sha256:x".into(),
            drawn: read as i64,
            campaign_id: Some(id),
            created_by: "ops".into(),
            created_at: "2026-11-01T00:00:00Z".into(),
            grade: json!({
                "read": read, "wrong": wrong, "not_settled": 0, "cant_tell": 0, "drawn": read,
                "wrong_by_axis": {"base": wrong},
                "by_vendor": {vendor: {"drawn": read, "read": read, "wrong": wrong}},
                "by_year": {"2024": {"drawn": read, "read": read, "wrong": wrong}},
                "rule": rule(wrong, 0),
            }),
            graded_at: None,
            graded_by: None,
            count: Some(150),
            since: None,
            settle_campaign_id: None,
            settle_seed: None,
            settle_share: None,
            review_campaign_id: None,
        }
    }

    #[test]
    fn six_rounds_pool_to_nine_hundred_with_the_rule() {
        assert_eq!(rule(1, 0)["passed"], true);
        assert_eq!(rule(2, 0)["passed"], false);
        assert_eq!(rule(0, 3)["passed"], Value::Null);
        assert_eq!(rule(2, 3)["passed"], false);
        let draws: Vec<Draw> = (1..=6)
            .map(|i| {
                graded(
                    i,
                    150,
                    if i == 4 { 2 } else { 0 },
                    if i % 2 == 0 { "ge" } else { "siemens" },
                )
            })
            .collect();
        let p = pool(&draws).unwrap();
        assert_eq!(p["read"], 900);
        assert_eq!(p["wrong"], 2);
        assert_eq!(p["drawn"], 900);
        assert_eq!(p["by_vendor"]["ge"]["read"], 450);
        assert_eq!(p["by_vendor"]["ge"]["wrong"], 2);
        assert_eq!(p["by_year"]["2024"]["read"], 900);
        assert_eq!(p["rule"]["rounds_over"], json!([4]));
        assert_eq!(p["rule"]["passed"], false);
        let exact = clopper_pearson_upper(2, 900, CONFIDENCE);
        assert!((p["bound"].as_f64().unwrap() - exact).abs() < 1e-12);
        // 0 wrong in 900 bounds the error at about 0.33 %
        assert!((clopper_pearson_upper(0, 900, 0.95) - 0.0033).abs() < 0.0001);
        assert!(pool(&[draws[0].clone(), draws[0].clone()]).is_err());
        let mut ungraded = draws[1].clone();
        ungraded.grade = Value::Null;
        assert!(pool(&[ungraded]).is_err());
    }

    #[test]
    fn values_compare_as_words_in_order() {
        assert_eq!(
            comparable(&json!("FLAIR,FatSat")),
            comparable(&json!(["FatSat", "FLAIR"]))
        );
        assert_eq!(comparable(&json!("[\"b\",\"a\"]")), Some("a,b".into()));
        assert_eq!(comparable(&json!(null)), None);
        assert_eq!(comparable(&json!("")), None);
        assert_eq!(comparable(&json!([])), None);
    }
}
