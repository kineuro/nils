// SPDX-License-Identifier: AGPL-3.0-only

//! The review rule's rounds and its gate (record 55 D3, record 56 sections
//! 2 and 3; spec §11.2).
//!
//! **A round** is a pair of a rules version and a System 1 version: the
//! pack and its version the stacks were sorted with, and the System 1
//! model chosen for the task (none while no model is chosen). A new rules
//! release or a new chosen model starts a new round, and the gate's
//! thresholds, as the model's card set them, are frozen with the pair.
//! Every audit draw ([`crate::audit_draw`]) is of one round.
//!
//! **The gate is chosen per dataset, not per scan** (record 56, section 3):
//!
//! - on a dataset with normal headers the rules decide, and Review holds
//!   only the axes that matter they left empty; System 1's opinion is a
//!   second opinion beside those questions and sends nothing there;
//! - on a dataset flagged as having damaged or foreign headers, the full
//!   gate applies to every axis the model's card gives thresholds for: the
//!   rules' answer is **accepted** when System 1's answer equals it, its
//!   probability is at least λ1 and the rules' reliability on that header
//!   is at least λ2. An accepted answer stays the rules' decision and the
//!   round records both authors (the rules' version and System 1's model).
//!   Every other stack and axis is a review question, `<axis>:disagree`
//!   where the two answers differ and `<axis>:unsure` where they agree
//!   below a threshold.
//!
//! The gate on flagged datasets waits for System 1 to be confirmed: until
//! a recorded certificate names the chosen model (D2) it raises nothing,
//! and a gate asked for says so. An axis a person decided is never asked
//! again, and a stack of a sample sealed now never becomes a review item.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::model::Model;
use crate::opinion::{self, Opinion};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The two kinds of question the gate raises, by their suffix.
pub const DISAGREE: &str = "disagree";
pub const UNSURE: &str = "unsure";

/// The outcomes of the gate on one stack and axis.
pub const OUTCOMES: [&str; 4] = ["accepted", DISAGREE, UNSURE, "decided"];

/// One round.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub id: i64,
    pub pack: String,
    pub pack_version: String,
    pub model_id: Option<i64>,
    /// `{axis: {lambda1, lambda2}}`, from the model's card.
    pub gate: Value,
    /// Whether a recorded certificate named the model when the round began.
    pub confirmed: bool,
    pub started_at: String,
    pub started_by: String,
}

impl Round {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "pack": self.pack, "pack_version": self.pack_version,
            "rules": format!("{}@{}", self.pack, self.pack_version),
            "model_id": self.model_id, "gate": self.gate, "confirmed": self.confirmed,
            "started_at": self.started_at, "started_by": self.started_by,
        })
    }

    /// λ1 and λ2 per axis.
    pub fn thresholds(&self) -> BTreeMap<String, (f64, f64)> {
        let mut out = BTreeMap::new();
        for (axis, t) in self.gate.as_object().into_iter().flatten() {
            if let (Some(l1), Some(l2)) = (t["lambda1"].as_f64(), t["lambda2"].as_f64()) {
                out.insert(axis.clone(), (l1, l2));
            }
        }
        out
    }
}

fn select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("review_round");
    format!(
        "SELECT id, pack, pack_version, model_id, {}, confirmed, {}, started_by FROM {}",
        d.text_of(t.column("gate").expect("gate")),
        d.text_of(t.column("started_at").expect("started_at")),
        store.qualified("review_round"),
    )
}

fn of_row(r: &Row) -> Result<Round, StoreError> {
    Ok(Round {
        id: r.int(0)?,
        pack: r.text(1)?.to_string(),
        pack_version: r.text(2)?.to_string(),
        model_id: r.opt_int(3)?,
        gate: r
            .opt_text(4)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_else(|| json!({})),
        confirmed: r.opt_int(5)?.unwrap_or(0) != 0,
        started_at: r.text(6)?.to_string(),
        started_by: r.text(7)?.to_string(),
    })
}

/// Every round, newest first.
pub fn list(store: &mut Store) -> Result<Vec<Round>, StoreError> {
    let sql = format!("{} ORDER BY id DESC", select(store));
    store.query(&sql, &[])?.iter().map(of_row).collect()
}

pub fn get(store: &mut Store, id: i64) -> Result<Option<Round>, StoreError> {
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

/// The round of this rules version and the System 1 model chosen now,
/// begun here when it is new. A round of a model whose certificate was
/// recorded later is marked confirmed then, since its thresholds are the
/// same pair's.
pub fn current(
    store: &mut Store,
    pack: &str,
    pack_version: &str,
    who: &str,
) -> Result<Round, StoreError> {
    let chosen = opinion::chosen(store)?.map(|c| c.model);
    let model_id = chosen.as_ref().map(|m| m.id);
    let by_model = match model_id {
        Some(id) => format!("model_id = {id}"),
        None => "model_id IS NULL".to_string(),
    };
    let d = store.dialect();
    let sql = format!(
        "{} WHERE pack = {} AND pack_version = {} AND {by_model} ORDER BY id DESC LIMIT 1",
        select(store),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let confirmed = match model_id {
        Some(id) => opinion::confirmed(store, id)?.is_some(),
        None => false,
    };
    if let Some(r) = store.query_opt(&sql, &[Param::from(pack), Param::from(pack_version)])? {
        let mut round = of_row(&r)?;
        if confirmed && !round.confirmed {
            store.update_by_id(
                table("review_round"),
                &[("confirmed", Param::Int(1))],
                "id",
                round.id,
            )?;
            round.confirmed = true;
        }
        return Ok(round);
    }
    let gate = match &chosen {
        Some(m) => gate_of(m),
        None => json!({}),
    };
    let now = now_iso();
    let id = store
        .insert(
            &Insert::new(
                table("review_round"),
                &[
                    "pack",
                    "pack_version",
                    "model_id",
                    "gate",
                    "confirmed",
                    "started_at",
                    "started_by",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(pack),
                Param::from(pack_version),
                model_id.map_or(Param::Null, Param::Int),
                Param::from(gate.to_string()),
                Param::Int(i64::from(confirmed)),
                Param::from(now.as_str()),
                Param::from(who),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the round was not written back".into()))?
        .int(0)?;
    Ok(Round {
        id,
        pack: pack.to_string(),
        pack_version: pack_version.to_string(),
        model_id,
        gate,
        confirmed,
        started_at: now,
        started_by: who.to_string(),
    })
}

/// A card's thresholds as a round keeps them.
pub fn gate_of(m: &Model) -> Value {
    let mut out = serde_json::Map::new();
    for (axis, (l1, l2)) in opinion::thresholds(m) {
        out.insert(axis, json!({"lambda1": l1, "lambda2": l2}));
    }
    Value::Object(out)
}

// ------------------------------------------------ the damaged-header flag

/// A dataset's damaged-header flag as a person set it (record 56 section
/// 3): `{damaged, why, by, at}`, or none where nobody did.
pub fn headers_flag(store: &mut Store, place_id: i64) -> Result<Option<Value>, StoreError> {
    let d = store.dialect();
    let t = table("place");
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        d.text_of(t.column("headers").expect("headers")),
        store.qualified("place"),
        d.param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(place_id)])?
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
        .and_then(|t| serde_json::from_str(&t).ok()))
}

/// Set or clear a dataset's flag. `damaged` none clears it, so the signs
/// decide again.
pub fn set_headers_flag(
    store: &mut Store,
    place_id: i64,
    damaged: Option<bool>,
    why: Option<&str>,
    who: &str,
) -> Result<Option<Value>, StoreError> {
    let doc = damaged.map(|d| json!({"damaged": d, "why": why, "by": who, "at": now_iso()}));
    store.update_by_id(
        table("place"),
        &[(
            "headers",
            doc.as_ref()
                .map_or(Param::Null, |v| Param::from(v.to_string())),
        )],
        "id",
        place_id,
    )?;
    Ok(doc)
}

// ------------------------------------------------------------- the gate

/// What the gate did over some stacks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gated {
    /// Outcomes by name, counted per stack and axis.
    pub outcomes: BTreeMap<String, i64>,
    /// Stacks every gated axis of which was accepted.
    pub accepted_stacks: i64,
    pub stacks: i64,
    /// Questions raised or brought up to date, and closed as answered by
    /// the gate now accepting.
    pub raised: Vec<i64>,
    pub closed: i64,
    /// Stacks left out: no opinion of the round's model, sealed now.
    pub skipped: BTreeMap<String, i64>,
    /// Why nothing was gated, where nothing was.
    pub waiting: Option<String>,
}

impl Gated {
    pub fn to_json(&self) -> Value {
        json!({
            "outcomes": self.outcomes, "accepted_stacks": self.accepted_stacks,
            "stacks": self.stacks, "raised": self.raised, "closed": self.closed,
            "skipped": self.skipped, "waiting": self.waiting,
        })
    }
}

/// Per stack, per axis: the rules' value and whether a person decided it.
pub type RulesAnswers = BTreeMap<i64, BTreeMap<String, (Option<String>, bool)>>;

/// The rules' answer on each axis of each stack: its value (values joined
/// in order for a multi-valued axis, none for "no value"), and whether a
/// person's or an agent's decision set it.
pub fn rules_answers(store: &mut Store, stacks: &[i64]) -> Result<RulesAnswers, StoreError> {
    let mut rows: BTreeMap<i64, BTreeMap<String, (BTreeSet<String>, bool)>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, axis, value, tier FROM {} WHERE stack_id IN ({list}) ORDER BY id",
            store.qualified("classification_axis")
        );
        for r in store.query(&sql, &[])? {
            let e = rows
                .entry(r.int(0)?)
                .or_default()
                .entry(r.text(1)?.to_string())
                .or_default();
            if let Some(v) = r.opt_text(2)?.filter(|v| !v.is_empty()) {
                e.0.insert(v.to_string());
            }
            if r.text(3)? == "decision" {
                e.1 = true;
            }
        }
    }
    Ok(rows
        .into_iter()
        .map(|(s, axes)| {
            (
                s,
                axes.into_iter()
                    .map(|(a, (vals, decided))| {
                        let v = (!vals.is_empty())
                            .then(|| vals.into_iter().collect::<Vec<_>>().join(","));
                        (a, (v, decided))
                    })
                    .collect(),
            )
        })
        .collect())
}

/// The kind of the question the gate raises on an axis.
pub fn kind(axis: &str, why: &str) -> String {
    format!("{axis}:{why}")
}

fn key(axis: &str, stack: i64) -> String {
    format!("gate:{axis}:stack:{stack}")
}

/// Run the gate over stacks of a dataset flagged as having damaged headers
/// (record 56 section 3), in the round given: each stack and axis the
/// round's thresholds cover gets its outcome, kept in `gate_outcome`, and a
/// question where it is not accepted; an earlier question the gate raised
/// on a stack and axis it now accepts is closed as superseded. Nothing is
/// gated in a round with no model or none confirmed, and the answer says
/// why. The caller holds no transaction; this writes in one.
pub fn gate(
    store: &mut Store,
    round: &Round,
    stacks: &[i64],
    who: &str,
) -> Result<Gated, StoreError> {
    let mut out = Gated::default();
    let Some(model) = round.model_id else {
        out.waiting = Some(
            "no System 1 model is chosen, so the gate has nothing to compare the rules with".into(),
        );
        return Ok(out);
    };
    if !round.confirmed {
        out.waiting = Some(format!(
            "System 1 (model {model}) is not confirmed: no recorded certificate names it, and the gate on flagged datasets waits for that (record 56, D2)"
        ));
        return Ok(out);
    }
    let thresholds = round.thresholds();
    if thresholds.is_empty() {
        out.waiting = Some(format!(
            "model {model}'s card sets no gate thresholds (gate.axes), so nothing can be accepted by the gate"
        ));
        return Ok(out);
    }
    let (sealed, _) = crate::labels::sealed_now(store, stacks, &[])
        .map_err(|e| StoreError::Message(e.to_string()))?;
    let opinions = opinion::newest(store, stacks, Some(model))?;
    let rules = rules_answers(store, stacks)?;
    store.begin()?;
    let written = (|| -> Result<(), StoreError> {
        let now = now_iso();
        for &stack in stacks {
            if sealed.contains(&stack) {
                *out.skipped.entry("sealed".into()).or_default() += 1;
                continue;
            }
            let Some(said) = opinions.get(&stack) else {
                *out.skipped.entry("no opinion".into()).or_default() += 1;
                continue;
            };
            out.stacks += 1;
            let mut all = true;
            for (axis, &(l1, l2)) in &thresholds {
                let (rule, decided) = rules
                    .get(&stack)
                    .and_then(|a| a.get(axis))
                    .cloned()
                    .unwrap_or((None, false));
                let o = said.get(axis);
                let outcome = outcome_of(rule.as_deref(), decided, o, l1, l2);
                *out.outcomes.entry(outcome.to_string()).or_default() += 1;
                if outcome != "accepted" && outcome != "decided" {
                    all = false;
                }
                let item = match outcome {
                    DISAGREE | UNSURE => Some(raise(
                        store,
                        round,
                        stack,
                        axis,
                        outcome,
                        rule.as_deref(),
                        o,
                        l1,
                        l2,
                        &now,
                    )?),
                    _ => {
                        out.closed += close(store, stack, axis, who, &now)?;
                        None
                    }
                };
                if let Some(id) = item {
                    out.raised.push(id);
                }
                record(
                    store,
                    round,
                    stack,
                    axis,
                    outcome,
                    rule.as_deref(),
                    o,
                    item,
                    &now,
                )?;
            }
            if all {
                out.accepted_stacks += 1;
            }
        }
        Ok(())
    })();
    match written {
        Ok(()) => store.commit()?,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    }
    Ok(out)
}

/// The gate's word on one stack and axis.
pub fn outcome_of(
    rule: Option<&str>,
    decided: bool,
    o: Option<&Opinion>,
    lambda1: f64,
    lambda2: f64,
) -> &'static str {
    if decided {
        return "decided";
    }
    let Some(o) = o else {
        return UNSURE;
    };
    // the model's answer against the rules' as it read them (its own
    // vocabulary), else against the stored value
    if !o.agrees(rule) {
        return DISAGREE;
    }
    let reliable = o.reliability.is_some_and(|r| r >= lambda2);
    if o.p >= lambda1 && reliable {
        "accepted"
    } else {
        UNSURE
    }
}

#[allow(clippy::too_many_arguments)]
fn raise(
    store: &mut Store,
    round: &Round,
    stack: i64,
    axis: &str,
    why: &str,
    rule: Option<&str>,
    o: Option<&Opinion>,
    lambda1: f64,
    lambda2: f64,
    now: &str,
) -> Result<i64, StoreError> {
    let k = kind(axis, why);
    let group = key(axis, stack);
    let evidence = json!({
        "gate": {"round_id": round.id, "rules": format!("{}@{}", round.pack, round.pack_version),
                 "model_id": round.model_id, "lambda1": lambda1, "lambda2": lambda2},
        "axis": axis,
        "rules": {"value": rule, "as_read": o.and_then(|o| o.rules_read.clone())},
        "system1": o.map(Opinion::as_json),
        "value": rule,
        "confidence": o.map(|o| o.p),
    });
    // a question of the other kind on the same stack and axis is replaced
    let other = kind(axis, if why == DISAGREE { UNSURE } else { DISAGREE });
    if let Some((id, _)) = crate::review::open_item(store, &other, &group)? {
        supersede(store, id, now)?;
    }
    if let Some((id, _)) = crate::review::open_item(store, &k, &group)? {
        crate::review::refresh_item(store, id, &evidence, 1, None)?;
        return Ok(id);
    }
    crate::review::open_new(
        store,
        &k,
        "stack",
        &group,
        &json!({"stack_id": stack}),
        &evidence,
        1,
        None,
        now,
    )
}

fn supersede(store: &mut Store, id: i64, now: &str) -> Result<(), StoreError> {
    store.update_by_id(
        table("review_item"),
        &[
            ("status", Param::from("superseded")),
            ("decided_at", Param::from(now)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// Close the gate's open questions on a stack and axis the gate accepts
/// now. Answers how many it closed.
fn close(
    store: &mut Store,
    stack: i64,
    axis: &str,
    _who: &str,
    now: &str,
) -> Result<i64, StoreError> {
    let group = key(axis, stack);
    let mut n = 0;
    for why in [DISAGREE, UNSURE] {
        if let Some((id, _)) = crate::review::open_item(store, &kind(axis, why), &group)? {
            supersede(store, id, now)?;
            n += 1;
        }
    }
    Ok(n)
}

#[allow(clippy::too_many_arguments)]
fn record(
    store: &mut Store,
    round: &Round,
    stack: i64,
    axis: &str,
    outcome: &str,
    rule: Option<&str>,
    o: Option<&Opinion>,
    item: Option<i64>,
    now: &str,
) -> Result<(), StoreError> {
    let d = store.dialect();
    store.execute(
        &format!(
            "DELETE FROM {} WHERE round_id = {} AND stack_id = {} AND axis = {}",
            store.qualified("gate_outcome"),
            d.param(1, Type::Int),
            d.param(2, Type::Int),
            d.param(3, Type::Text),
        ),
        &[Param::Int(round.id), Param::Int(stack), Param::from(axis)],
    )?;
    store.insert(
        &Insert::new(
            table("gate_outcome"),
            &[
                "round_id",
                "stack_id",
                "axis",
                "outcome",
                "rules_value",
                "model_value",
                "p",
                "reliability",
                "review_item",
                "at",
            ],
        ),
        &[vec![
            Param::Int(round.id),
            Param::Int(stack),
            Param::from(axis),
            Param::from(outcome),
            rule.map_or(Param::Null, Param::from),
            o.and_then(|o| o.value.clone())
                .map_or(Param::Null, Param::Text),
            o.map_or(Param::Null, |o| Param::Double(o.p)),
            o.and_then(|o| o.reliability)
                .map_or(Param::Null, Param::Double),
            item.map_or(Param::Null, Param::Int),
            Param::from(now),
        ]],
    )?;
    Ok(())
}

/// The gate's outcomes in a round, counted per stack and axis, and the
/// stacks whose every gated axis was accepted.
pub fn outcomes(store: &mut Store, round: i64) -> Result<(BTreeMap<String, i64>, i64), StoreError> {
    let d = store.dialect();
    let mut by: BTreeMap<String, i64> = BTreeMap::new();
    for r in store.query(
        &format!(
            "SELECT outcome, COUNT(*) FROM {} WHERE round_id = {} GROUP BY outcome",
            store.qualified("gate_outcome"),
            d.param(1, Type::Int)
        ),
        &[Param::Int(round)],
    )? {
        by.insert(r.text(0)?.to_string(), r.int(1)?);
    }
    let accepted = store
        .query_opt(
            &format!(
                "SELECT COUNT(*) FROM (SELECT stack_id FROM {} WHERE round_id = {} GROUP BY stack_id \
                 HAVING SUM(CASE WHEN outcome IN ('accepted', 'decided') THEN 0 ELSE 1 END) = 0) a",
                store.qualified("gate_outcome"),
                d.param(1, Type::Int)
            ),
            &[Param::Int(round)],
        )?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0);
    Ok((by, accepted))
}

/// The stacks the gate holds back in a round: some gated axis is neither
/// accepted nor a person's.
pub fn held_back(store: &mut Store, round: i64) -> Result<BTreeSet<i64>, StoreError> {
    let d = store.dialect();
    store
        .query(
            &format!(
                "SELECT DISTINCT stack_id FROM {} WHERE round_id = {} AND outcome NOT IN ('accepted', 'decided')",
                store.qualified("gate_outcome"),
                d.param(1, Type::Int)
            ),
            &[Param::Int(round)],
        )?
        .iter()
        .map(|r| r.int(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(value: Option<&str>, p: f64, reliability: Option<f64>) -> Opinion {
        Opinion {
            id: 1,
            stack_id: 1,
            axis: "base".into(),
            model_id: 1,
            run_id: None,
            value: value.map(str::to_string),
            p,
            reliability,
            fields: Value::Null,
            probabilities: Value::Null,
            note: None,
            rules_read: None,
            created_at: String::new(),
        }
    }

    fn read_as(value: Option<&str>, read: Option<&str>) -> Opinion {
        Opinion {
            rules_read: Some(read.map(str::to_string)),
            ..said(value, 0.97, Some(0.99))
        }
    }

    /// After D2: the model answers in its own vocabulary and says how it
    /// read the rules; a vendor's name the pack stores is the same answer
    /// where the model read it so.
    #[test]
    fn the_gate_compares_the_model_with_the_rules_as_it_read_them() {
        let (l1, l2) = (0.5, 0.9);
        // the pack stores SPACE, the model says 3D-TSE and read the rules so
        let space = read_as(Some("3D-TSE"), Some("3D-TSE"));
        assert_eq!(
            outcome_of(Some("SPACE"), false, Some(&space), l1, l2),
            "accepted"
        );
        // T2*w stored, T2starw in the model's words
        let star = read_as(Some("T2starw"), Some("T2starw"));
        assert_eq!(
            outcome_of(Some("T2*w"), false, Some(&star), l1, l2),
            "accepted"
        );
        // the rules decided no base, the model says null and read it so
        let none = read_as(None, None);
        assert_eq!(
            outcome_of(Some("none"), false, Some(&none), l1, l2),
            "accepted"
        );
        assert_eq!(outcome_of(None, false, Some(&none), l1, l2), "accepted");
        // its own reading differs: a disagreement, whatever is stored
        let other = read_as(Some("SS-TSE"), Some("3D-TSE"));
        assert_eq!(
            outcome_of(Some("SPACE"), false, Some(&other), l1, l2),
            DISAGREE
        );
        // with no reading, the stored value is compared, as before
        assert_eq!(
            outcome_of(
                Some("SPACE"),
                false,
                Some(&said(Some("3D-TSE"), 0.97, Some(0.99))),
                l1,
                l2
            ),
            DISAGREE
        );
    }

    #[test]
    fn the_gate_accepts_only_agreement_above_both_thresholds() {
        let (l1, l2) = (0.9, 0.95);
        let ok = said(Some("T1w"), 0.97, Some(0.99));
        assert_eq!(
            outcome_of(Some("T1w"), false, Some(&ok), l1, l2),
            "accepted"
        );
        // another value is a disagreement, however sure
        assert_eq!(outcome_of(Some("T2w"), false, Some(&ok), l1, l2), DISAGREE);
        assert_eq!(outcome_of(None, false, Some(&ok), l1, l2), DISAGREE);
        // agreement below either threshold, or with no reliability, is unsure
        let low = said(Some("T1w"), 0.8, Some(0.99));
        assert_eq!(outcome_of(Some("T1w"), false, Some(&low), l1, l2), UNSURE);
        let unreliable = said(Some("T1w"), 0.97, Some(0.5));
        assert_eq!(
            outcome_of(Some("T1w"), false, Some(&unreliable), l1, l2),
            UNSURE
        );
        let none = said(Some("T1w"), 0.97, None);
        assert_eq!(outcome_of(Some("T1w"), false, Some(&none), l1, l2), UNSURE);
        // no opinion is unsure; a person's decision is never asked again
        assert_eq!(outcome_of(Some("T1w"), false, None, l1, l2), UNSURE);
        assert_eq!(outcome_of(Some("T1w"), true, Some(&low), l1, l2), "decided");
        // both saying nothing agree
        let empty = said(None, 0.99, Some(0.99));
        assert_eq!(outcome_of(None, false, Some(&empty), l1, l2), "accepted");
    }
}
