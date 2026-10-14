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

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The share of a round's accepted stacks an audit reads (spec §18).
pub const DEFAULT_SHARE: f64 = 0.01;

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
}

impl Draw {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "round_id": self.round_id, "seed": self.seed, "share": self.share,
            "frame": {"stacks": self.frame_count, "digest": self.frame_digest},
            "drawn": self.drawn, "campaign_id": self.campaign_id,
            "created_by": self.created_by, "created_at": self.created_at,
            "grade": self.grade, "graded_at": self.graded_at, "graded_by": self.graded_by,
            "trains": false,
        })
    }
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

/// The stacks a seed draws from a frame, in the order drawn.
pub fn choose<'a>(seed: &str, share: f64, frame: &'a [Framed]) -> Vec<&'a Framed> {
    let mut ranked: Vec<(String, &Framed)> = frame.iter().map(|f| (rank(seed, f), f)).collect();
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.identity().cmp(&b.1.identity()))
    });
    ranked
        .into_iter()
        .take(how_many(share, frame.len()))
        .map(|(_, f)| f)
        .collect()
}

/// The frame's digest: the sha256 of its identities, sorted, one a line.
pub fn frame_digest(frame: &[Framed]) -> String {
    let mut ids: Vec<String> = frame.iter().map(Framed::identity).collect();
    ids.sort();
    let mut text = ids.join("\n");
    text.push('\n');
    format!("sha256:{}", sha256_hex(text.as_bytes()))
}

/// Draw and keep a round's audit sample. Refused for a share outside
/// (0, 1] or an empty frame.
pub fn draw(
    store: &mut Store,
    round_id: i64,
    seed: &str,
    share: f64,
    frame: &[Framed],
    who: &str,
) -> Result<(Draw, Vec<Framed>), String> {
    if !(share > 0.0 && share <= 1.0) {
        return Err(format!("share: above 0 and at most 1, not {share}"));
    }
    if seed.trim().is_empty() {
        return Err("seed: a word the draw is made from, kept with it".into());
    }
    if frame.is_empty() {
        return Err(
            "the round has no accepted stacks to draw from: none was sorted under its rules version and left without a question".into(),
        );
    }
    let chosen: Vec<Framed> = choose(seed, share, frame).into_iter().cloned().collect();
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
        "SELECT id, round_id, seed, share, frame_count, frame_digest, drawn, campaign_id, created_by, {}, {}, {}, graded_by FROM {}",
        d.text_of(t.column("created_at").expect("created_at")),
        d.text_of(t.column("grade").expect("grade")),
        d.text_of(t.column("graded_at").expect("graded_at")),
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
    })
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
        "SELECT stack_id, study_uid, series_uid, stack_key, {} FROM {} WHERE draw_id = {} ORDER BY id",
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
}

/// Grade a draw from its settled answers: a stack is wrong when any axis
/// its settled answer names differs from the rules' accepted answer; a
/// stack whose settled answer says it cannot tell on an axis, or that has
/// no settled answer yet, is not graded and is counted apart. Kept on the
/// draw, with who graded it and when.
pub fn grade(
    store: &mut Store,
    draw: &Draw,
    reads: &[Read],
    campaign_closed: bool,
    who: &str,
) -> Result<Value, StoreError> {
    let drawn = stacks(store, draw.id)?;
    let by_stack: BTreeMap<i64, &Read> = reads.iter().map(|r| (r.stack_id, r)).collect();
    let (mut n, mut k) = (0u64, 0u64);
    let (mut unsettled, mut cant_tell) = (0u64, 0u64);
    let mut wrong_axes: BTreeMap<String, u64> = BTreeMap::new();
    for f in &drawn {
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
        let mut wrong = false;
        for (axis, v) in &read.settled {
            if comparable(v) != comparable(&f.accepted[axis]) {
                wrong = true;
                *wrong_axes.entry(axis.clone()).or_default() += 1;
            }
        }
        if wrong {
            k += 1;
        }
    }
    let doc = json!({
        "read": n, "wrong": k,
        "error": if n > 0 { json!(k as f64 / n as f64) } else { Value::Null },
        "bound": clopper_pearson_upper(k, n, CONFIDENCE),
        "confidence": CONFIDENCE, "method": "exact one-sided Clopper-Pearson",
        "wrong_by_axis": wrong_axes, "not_settled": unsettled, "cant_tell": cant_tell,
        "drawn": drawn.len(), "campaign_closed": campaign_closed,
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
