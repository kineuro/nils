// SPDX-License-Identifier: AGPL-3.0-only

//! Blind first where a read trains or measures (record 55 D6; spec §11.4).
//!
//! - **A mode per question.** A review question about a stack's axis is
//!   `routine`, where both suggestions and both reasons show at once, or
//!   `blind_first`, where the person answers from the evidence (the image,
//!   the header text, the fields) and the suggestions show only after a
//!   provisional answer. In the queue a random share of questions is
//!   `blind_first`; the share is a site setting, one in five by default
//!   ([`DEFAULT_SHARE`]). The mode is drawn once, when the question is
//!   first read, from a site salt and the question's id, and kept
//!   ([`mode_of`]).
//! - **Campaigns** whose answers train a model or measure an error
//!   (purpose `train`, `measure` or `audit`) are blind first always: a
//!   rater is shown an item's suggestion only after answering it, and an
//!   audit campaign's answers never train ([`PURPOSES`]).
//! - **Every answer records whether it was given blind and the time it
//!   took.** A queue decision is marked by [`mark_decision`]: blind where it
//!   is the provisional answer the person gave before any suggestion showed
//!   (they kept it after seeing them), not blind otherwise. A campaign's
//!   answer is blind where the rater had not been shown the item's
//!   suggestion ([`campaign_answer_blind`]).
//! - **Only blind answers become training labels** (spec §16.2): the
//!   training labels leave out decisions marked not blind, and a label set
//!   with an answer that was not blind, or of an audit, is not training
//!   data (`labels`). The rest are decisions.

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// The two modes.
pub const ROUTINE: &str = "routine";
pub const BLIND_FIRST: &str = "blind_first";

/// One question in five is blind first unless the site says otherwise
/// (spec §18).
pub const DEFAULT_SHARE: f64 = 0.2;

/// The registry settings the share and its salt are kept under.
pub const SHARE_KEY: &str = "review_blind_share";
pub const SALT_KEY: &str = "review_blind_salt";

/// The salt a mode is drawn with where the site set none.
pub const DEFAULT_SALT: &str = "nils-review";

/// What a campaign's answers are for. `decide` (the default) closes into
/// decisions as before; `train` and `measure` are read blind first always;
/// `audit` is read blind and its answers never train.
pub const PURPOSES: [&str; 4] = ["decide", "train", "measure", "audit"];

/// Whether a purpose is read blind first always.
pub fn blind_purpose(purpose: &str) -> bool {
    matches!(purpose, "train" | "measure" | "audit")
}

/// Whether a review question of this kind is one a mode is drawn for: a
/// question about an axis of a stack, the kind a read can train or
/// measure. Anything else (an identity, a pick border, a pipeline's
/// check) is routine.
pub fn asks_an_axis(kind: &str) -> bool {
    if kind == crate::asked::KIND {
        return true;
    }
    let Some((axis, why)) = kind.split_once(':') else {
        return false;
    };
    !axis.is_empty()
        && axis.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        && matches!(
            why,
            "missing"
                | "disagree"
                | "unsure"
                | "model"
                | "decision"
                | "low_confidence"
                | "conflict"
        )
}

/// The share of questions drawn blind first, from a setting's text; the
/// default where it is missing or not a share.
pub fn share_of(text: Option<&str>) -> f64 {
    text.and_then(|t| t.trim().parse::<f64>().ok())
        .filter(|s| (0.0..=1.0).contains(s))
        .unwrap_or(DEFAULT_SHARE)
}

/// Whether a question is drawn blind first: the first eight bytes of the
/// sha256 of the salt and the question's id, as a number below one, under
/// the share. The same question always draws the same.
pub fn draws_blind(salt: &str, item: i64, share: f64) -> bool {
    let h = ring::digest::digest(
        &ring::digest::SHA256,
        format!("{salt}:review:{item}").as_bytes(),
    );
    let mut b = [0u8; 8];
    b.copy_from_slice(&h.as_ref()[..8]);
    (u64::from_be_bytes(b) as f64 / u64::MAX as f64) < share
}

/// A question's mode, drawn now and kept where it has none yet. A question
/// that asks no axis is routine and nothing is kept.
pub fn mode_of(
    store: &mut Store,
    item: i64,
    kind: &str,
    salt: &str,
    share: f64,
) -> Result<&'static str, StoreError> {
    if !asks_an_axis(kind) {
        return Ok(ROUTINE);
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT mode FROM {} WHERE id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    if let Some(r) = store.query_opt(&sql, &[Param::Int(item)])?
        && let Some(m) = r.opt_text(0)?
    {
        return Ok(if m == BLIND_FIRST {
            BLIND_FIRST
        } else {
            ROUTINE
        });
    }
    let mode = if draws_blind(salt, item, share) {
        BLIND_FIRST
    } else {
        ROUTINE
    };
    store.execute(
        &format!(
            "UPDATE {} SET mode = {} WHERE id = {} AND mode IS NULL",
            store.qualified("review_item"),
            d.param(1, Type::Text),
            d.param(2, Type::Int)
        ),
        &[Param::from(mode), Param::Int(item)],
    )?;
    Ok(mode)
}

/// Set a question's mode, as a campaign that adopts it for a read that
/// trains or measures does.
pub fn set_mode(store: &mut Store, item: i64, mode: &str) -> Result<(), StoreError> {
    store.update_by_id(
        table("review_item"),
        &[("mode", Param::from(mode))],
        "id",
        item,
    )?;
    Ok(())
}

/// Note that a person opened a question, the first time only: the time an
/// answer took is measured from here. Answers when they first opened it.
pub fn read(store: &mut Store, item: i64, principal: &str) -> Result<String, StoreError> {
    let d = store.dialect();
    let t = table("review_read");
    let sql = format!(
        "SELECT {} FROM {} WHERE item_id = {} AND principal = {}",
        d.text_of(t.column("first_read_at").expect("first_read_at")),
        store.qualified("review_read"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    if let Some(r) = store.query_opt(&sql, &[Param::Int(item), Param::from(principal)])? {
        return Ok(r.text(0)?.to_string());
    }
    let now = now_iso();
    store.insert(
        &Insert::new(t, &["item_id", "principal", "first_read_at"]),
        &[vec![
            Param::Int(item),
            Param::from(principal),
            Param::from(now.as_str()),
        ]],
    )?;
    Ok(now)
}

/// Seconds since a person first opened a question, none where they never
/// did through a door that notes it.
pub fn seconds_since_read(
    store: &mut Store,
    item: i64,
    principal: &str,
) -> Result<Option<f64>, StoreError> {
    let d = store.dialect();
    let t = table("review_read");
    let sql = format!(
        "SELECT {} FROM {} WHERE item_id = {} AND principal = {}",
        d.text_of(t.column("first_read_at").expect("first_read_at")),
        store.qualified("review_read"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(item), Param::from(principal)])? else {
        return Ok(None);
    };
    let then = crate::time::secs_of(r.text(0)?);
    let now = crate::time::secs_of(&now_iso());
    Ok(match (then, now) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) as f64),
        _ => None,
    })
}

/// Note that the suggestions of a question were shown to a person.
pub fn revealed(store: &mut Store, item: i64, principal: &str) -> Result<(), StoreError> {
    read(store, item, principal)?;
    let d = store.dialect();
    store.execute(
        &format!(
            "UPDATE {} SET revealed_at = {} WHERE item_id = {} AND principal = {} AND revealed_at IS NULL",
            store.qualified("review_read"),
            d.param(1, Type::Timestamp),
            d.param(2, Type::Int),
            d.param(3, Type::Text)
        ),
        &[
            Param::from(now_iso().as_str()),
            Param::Int(item),
            Param::from(principal),
        ],
    )?;
    Ok(())
}

/// Whether a question's suggestions were shown to a person (record 56
/// §6.3: a blind-first question answered in a grid that never showed them
/// is answered blind).
pub fn was_revealed(store: &mut Store, item: i64, principal: &str) -> Result<bool, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE item_id = {} AND principal = {} AND revealed_at IS NOT NULL",
        store.qualified("review_read"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(item), Param::from(principal)])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0)
        > 0)
}

/// A provisional answer to a blind-first question.
#[derive(Debug, Clone, PartialEq)]
pub struct Provisional {
    pub answer: Value,
    pub seconds: Option<f64>,
    pub answered_at: String,
}

/// The provisional answer a person gave a question, if any.
pub fn provisional(
    store: &mut Store,
    item: i64,
    principal: &str,
) -> Result<Option<Provisional>, StoreError> {
    let d = store.dialect();
    let t = table("review_provisional");
    let sql = format!(
        "SELECT {}, seconds, {} FROM {} WHERE item_id = {} AND principal = {}",
        d.text_of(t.column("answer").expect("answer")),
        d.text_of(t.column("answered_at").expect("answered_at")),
        store.qualified("review_provisional"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    store
        .query_opt(&sql, &[Param::Int(item), Param::from(principal)])?
        .map(|r| {
            Ok(Provisional {
                answer: r
                    .opt_text(0)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null),
                seconds: r.opt_double(1)?,
                answered_at: r.text(2)?.to_string(),
            })
        })
        .transpose()
}

/// Keep a person's provisional answer to a blind-first question, given
/// before any suggestion showed; one per person, never replaced (the
/// first blind answer is the blind one). Answers it, with the seconds since
/// they first opened the question.
pub fn give_provisional(
    store: &mut Store,
    item: i64,
    principal: &str,
    answer: &Value,
) -> Result<Provisional, StoreError> {
    if let Some(p) = provisional(store, item, principal)? {
        return Ok(p);
    }
    let seconds = seconds_since_read(store, item, principal)?;
    let now = now_iso();
    store.insert(
        &Insert::new(
            table("review_provisional"),
            &["item_id", "principal", "answer", "seconds", "answered_at"],
        ),
        &[vec![
            Param::Int(item),
            Param::from(principal),
            Param::from(answer.to_string()),
            seconds.map_or(Param::Null, Param::Double),
            Param::from(now.as_str()),
        ]],
    )?;
    Ok(Provisional {
        answer: answer.clone(),
        seconds,
        answered_at: now,
    })
}

/// Whether a final answer is the provisional one: the same value, or the
/// same value on every axis it names.
pub fn same_answer(provisional: &Value, given: &Value) -> bool {
    let norm = |v: &Value| -> Value {
        match v {
            Value::Array(list) => {
                let mut words: Vec<String> = list
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                words.sort();
                json!(words)
            }
            Value::String(s) if s.is_empty() => Value::Null,
            other => other.clone(),
        }
    };
    if provisional["values"].is_object() || given["values"].is_object() {
        let (a, b) = (&provisional["values"], &given["values"]);
        let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
            return false;
        };
        return a.len() == b.len()
            && a.iter()
                .all(|(k, v)| b.get(k).is_some_and(|w| norm(v) == norm(w)));
    }
    let nothing = |v: &Value| v["nothing"].as_bool().unwrap_or(false);
    if nothing(provisional) || nothing(given) {
        return nothing(provisional) && nothing(given);
    }
    norm(&provisional["value"]) == norm(&given["value"])
}

/// Mark the decisions an answer wrote: whether it was given blind, the
/// seconds it took and the question's mode.
pub fn mark_decision(
    store: &mut Store,
    decisions: &[i64],
    blind: bool,
    seconds: Option<f64>,
    mode: &str,
) -> Result<(), StoreError> {
    for id in decisions {
        store.update_by_id(
            table("decision"),
            &[
                ("blind", Param::Int(i64::from(blind))),
                ("seconds", seconds.map_or(Param::Null, Param::Double)),
                ("mode", Param::from(mode)),
            ],
            "id",
            *id,
        )?;
    }
    Ok(())
}

// ----------------------------------------------------------- campaigns

/// A campaign's purpose; `decide` where none was said.
pub fn purpose(store: &mut Store, campaign: i64) -> Result<String, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT purpose FROM {} WHERE id = {}",
        store.qualified("campaign"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(campaign)])?
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
        .unwrap_or_else(|| "decide".to_string()))
}

/// Set a campaign's purpose, said once when it is made.
pub fn set_purpose(store: &mut Store, campaign: i64, purpose: &str) -> Result<(), String> {
    if !PURPOSES.contains(&purpose) {
        return Err(format!("purpose: {}, not {purpose}", PURPOSES.join(", ")));
    }
    store
        .update_by_id(
            table("campaign"),
            &[("purpose", Param::from(purpose))],
            "id",
            campaign,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether a rater of a campaign was shown an item's suggestion.
pub fn campaign_revealed(
    store: &mut Store,
    item: i64,
    principal: &str,
) -> Result<bool, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT 1 FROM {} WHERE item_id = {} AND principal = {}",
        store.qualified("campaign_reveal"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(item), Param::from(principal)])?
        .is_some())
}

/// Note that a rater of a campaign was shown an item's suggestion.
pub fn campaign_reveal(
    store: &mut Store,
    campaign: i64,
    item: i64,
    principal: &str,
) -> Result<(), StoreError> {
    if campaign_revealed(store, item, principal)? {
        return Ok(());
    }
    store.insert(
        &Insert::new(
            table("campaign_reveal"),
            &["campaign_id", "item_id", "principal", "revealed_at"],
        ),
        &[vec![
            Param::Int(campaign),
            Param::Int(item),
            Param::from(principal),
            Param::from(now_iso().as_str()),
        ]],
    )?;
    Ok(())
}

/// Whether a campaign answer is blind: the campaign shows no suggestion,
/// or the rater had not been shown this item's when they answered.
pub fn campaign_answer_blind(
    store: &mut Store,
    shows_suggestions: bool,
    item: i64,
    principal: &str,
    purpose: &str,
) -> Result<bool, StoreError> {
    if !shows_suggestions {
        return Ok(true);
    }
    if blind_purpose(purpose) {
        return Ok(!campaign_revealed(store, item, principal)?);
    }
    // a routine campaign shows its suggestion beside the item at once
    Ok(false)
}

/// Mark a campaign's answer blind or not.
pub fn mark_answer(store: &mut Store, answer: i64, blind: bool) -> Result<(), StoreError> {
    store.update_by_id(
        table("campaign_answer"),
        &[("blind", Param::Int(i64::from(blind)))],
        "id",
        answer,
    )?;
    Ok(())
}

/// How many answers of a campaign were not given blind.
pub fn answers_not_blind(store: &mut Store, campaign: i64) -> Result<i64, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE campaign_id = {} AND blind = 0",
        store.qualified("campaign_answer"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(campaign)])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

/// Counts of the decisions people gave through Review and the campaigns,
/// by whether they were given blind, for the round's report.
pub fn decision_counts(store: &mut Store) -> Result<Value, StoreError> {
    let sql = format!(
        "SELECT CASE WHEN blind IS NULL THEN 'unmarked' WHEN blind = 1 THEN 'blind' ELSE 'not_blind' END, \
         COUNT(*), AVG(seconds) FROM {} WHERE withdrawn_at IS NULL AND author_kind IN ('person', 'agent') \
         GROUP BY 1",
        store.qualified("decision")
    );
    let mut out = serde_json::Map::new();
    for r in store.query(&sql, &[])? {
        out.insert(
            r.text(0)?.to_string(),
            json!({"decisions": r.int(1)?, "mean_seconds": r.opt_double(2)?}),
        );
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_is_drawn_only_for_a_question_about_an_axis() {
        for kind in [
            "base:missing",
            "base:disagree",
            "technique:unsure",
            "body_part:model",
            "classify.asked",
        ] {
            assert!(asks_an_axis(kind), "{kind}");
        }
        for kind in [
            "identity.unmapped",
            "pick.border",
            "pipeline:qc",
            "campaign.check",
            "split:one_image_per_stack",
        ] {
            assert!(!asks_an_axis(kind), "{kind}");
        }
    }

    #[test]
    fn the_share_draws_about_one_in_five_and_always_the_same() {
        let n = 20_000;
        let blind = (1..=n).filter(|i| draws_blind("salt", *i, 0.2)).count();
        let share = blind as f64 / n as f64;
        assert!((share - 0.2).abs() < 0.01, "{share}");
        assert_eq!(draws_blind("salt", 7, 0.2), draws_blind("salt", 7, 0.2));
        assert!(!draws_blind("salt", 7, 0.0));
        assert!(draws_blind("salt", 7, 1.0));
        assert_eq!(share_of(None), DEFAULT_SHARE);
        assert_eq!(share_of(Some("0.5")), 0.5);
        assert_eq!(share_of(Some("2")), DEFAULT_SHARE);
    }

    #[test]
    fn a_kept_answer_is_the_provisional_one() {
        assert!(same_answer(
            &json!({"value": "T1w"}),
            &json!({"value": "T1w", "scope": "stack"})
        ));
        assert!(!same_answer(
            &json!({"value": "T1w"}),
            &json!({"value": "T2w"})
        ));
        assert!(same_answer(
            &json!({"nothing": true}),
            &json!({"nothing": true})
        ));
        assert!(!same_answer(
            &json!({"nothing": true}),
            &json!({"value": "T2w"})
        ));
        assert!(same_answer(
            &json!({"values": {"base": "T1w", "modifier": ["FLAIR", "FatSat"]}}),
            &json!({"values": {"modifier": ["FatSat", "FLAIR"], "base": "T1w"}})
        ));
        assert!(!same_answer(
            &json!({"values": {"base": "T1w"}}),
            &json!({"values": {"base": "T1w", "modifier": null}})
        ));
    }

    #[test]
    fn a_purpose_that_trains_or_measures_is_read_blind_first() {
        assert!(blind_purpose("train"));
        assert!(blind_purpose("measure"));
        assert!(blind_purpose("audit"));
        assert!(!blind_purpose("decide"));
    }
}
