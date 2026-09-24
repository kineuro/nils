// SPDX-License-Identifier: AGPL-3.0-only

//! The review spine (Wave 4a §10.2, C5 and C15): one shape for every
//! review surface. A question about a rule, an origin or a study is one
//! item with n members and not n items; a decision applied to a group is
//! one row with its scope; a decision may be staged and committed later,
//! and withdrawn either way; and a decision has a rank, person over agent
//! over model, and survives a re-classification, which emits new items and
//! never overwrites what a person said.
//!
//! What is measured first (§10.1) is why: at the pack's thresholds the
//! nmosd corpus asks nothing and the mixed corpus asks 77 items in 9
//! groups; with every axis at 0.95 the two ask 10,791 and 10,725 items,
//! which are 34 and 108 questions.

use std::collections::{BTreeMap, BTreeSet};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    Refused(String),
    /// The caller's kind may not do it: an agent or a model where only a
    /// person may.
    Forbidden(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Refused(m) | Error::Forbidden(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

/// The rank of an author kind (C15): a person over an agent over a model.
/// A decision of a higher rank on the same key wins whatever was written
/// later; among equals the later one wins.
pub fn rank(author_kind: &str) -> u8 {
    match author_kind {
        "person" => 3,
        "agent" => 2,
        "model" => 1,
        _ => 0,
    }
}

/// A run's per-stack questions, keyed by (kind, value, tier): the evidence
/// of the first, then each member's (item id, stack id, evidence text).
type Groups = BTreeMap<(String, String, String), (serde_json::Value, Vec<(i64, i64, String)>)>;

/// What the classifier's per-stack questions of one run collapse into.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Grouped {
    /// Group items written.
    pub items: i64,
    /// Members under them, which is the per-stack questions there were.
    pub members: i64,
    /// The distinct stacks those members stand on. One stack can carry
    /// several questions, so this is never the membership count and is the
    /// only one of the three that can be held against a count of stacks.
    pub stacks: i64,
}

/// Collapse the open per-stack axis questions a run raised into one item
/// per (kind, value, tier), with the stacks as members (§10.2). The
/// per-stack rows are the run's own, never seen by anyone, and go; what an
/// earlier run asked about the same stacks was superseded by the run
/// itself.
pub fn group_run(store: &mut Store, job_id: i64) -> Result<Grouped, Error> {
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT id, kind, {}, {} FROM {} WHERE job_id = {} AND scope = 'stack' AND status = 'open' \
         ORDER BY id",
        d.text_of(t.column("ref").expect("ref")),
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    let rows = store.query(&sql, &[Param::Int(job_id)])?;
    if rows.is_empty() {
        return Ok(Grouped::default());
    }
    let mut groups: Groups = BTreeMap::new();
    for r in &rows {
        let id = r.int(0)?;
        let kind = r.text(1)?.to_string();
        let reference: serde_json::Value = r
            .opt_text(2)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(serde_json::Value::Null);
        let evidence_text = r.opt_text(3)?.unwrap_or("{}").to_string();
        let evidence: serde_json::Value =
            serde_json::from_str(&evidence_text).unwrap_or(serde_json::Value::Null);
        let stack = reference["stack_id"].as_i64().unwrap_or(0);
        let value = evidence["value"]
            .as_str()
            .map(str::to_string)
            .or_else(|| evidence["decision"].as_str().map(str::to_string))
            .unwrap_or_default();
        // A disagreement is named by both answers: a body part decided as
        // spine over a brain is not the same question as one decided as
        // spine over a neck, and a group that mixed them would ask one
        // question about two rules.
        let value = match evidence["other"].as_str() {
            Some(other) => format!("{value} over {other}"),
            None => value,
        };
        let tier = evidence["tier"].as_str().unwrap_or("").to_string();
        groups
            .entry((kind, value, tier))
            .or_insert_with(|| (evidence.clone(), Vec::new()))
            .1
            .push((id, stack, evidence_text));
    }
    let now = now_iso();
    let mut out = Grouped::default();
    let mut on: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for ((kind, value, tier), (first, members)) in groups {
        let key = format!("{kind}|{value}|{tier}");
        let mut evidence = first;
        if let serde_json::Value::Object(m) = &mut evidence {
            m.insert("members".into(), serde_json::json!(members.len()));
            m.insert("group".into(), serde_json::json!(key));
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
                        "job_id",
                        "members",
                        "group_key",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(kind.as_str()),
                    Param::from("group"),
                    Param::from(serde_json::json!({"group": key}).to_string()),
                    Param::from(evidence.to_string()),
                    Param::from("open"),
                    Param::from(now.as_str()),
                    Param::Int(job_id),
                    Param::Int(members.len() as i64),
                    Param::from(key.as_str()),
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the group item was not written back".into()))?
            .int(0)?;
        let rows: Vec<Vec<Param>> = members
            .iter()
            .map(|(_, stack, ev)| {
                vec![
                    Param::Int(item),
                    Param::Int(*stack),
                    Param::from(ev.as_str()),
                ]
            })
            .collect();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(table("review_member"), &["item_id", "stack_id", "evidence"]),
                chunk,
            )?;
        }
        let ids: Vec<String> = members.iter().map(|(id, _, _)| id.to_string()).collect();
        for chunk in ids.chunks(500) {
            store.execute(
                &format!(
                    "DELETE FROM {} WHERE id IN ({})",
                    store.qualified("review_item"),
                    chunk.join(", ")
                ),
                &[],
            )?;
        }
        out.items += 1;
        out.members += members.len() as i64;
        on.extend(members.iter().map(|(_, stack, _)| *stack));
    }
    out.stacks = on.len() as i64;
    Ok(out)
}

/// One member of a grouped item.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub stack_id: i64,
    pub evidence: serde_json::Value,
    pub decided_at: Option<String>,
}

pub fn members(store: &mut Store, item: i64) -> Result<Vec<Member>, Error> {
    let d = store.dialect();
    let t = table("review_member");
    let sql = format!(
        "SELECT stack_id, {}, {} FROM {} WHERE item_id = {} ORDER BY stack_id",
        d.text_of(t.column("evidence").expect("evidence")),
        d.text_of(t.column("decided_at").expect("decided_at")),
        store.qualified("review_member"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(item)])?
        .iter()
        .map(|r| {
            Ok(Member {
                stack_id: r.int(0)?,
                evidence: r
                    .opt_text(1)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(serde_json::Value::Null),
                decided_at: r.opt_text(2)?.map(str::to_string),
            })
        })
        .collect()
}

/// One review item, the fields the spine reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub kind: String,
    pub scope: String,
    pub status: String,
    pub reference: serde_json::Value,
    pub evidence: serde_json::Value,
    pub members: i64,
}

pub fn item(store: &mut Store, id: i64) -> Result<Option<Item>, Error> {
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT id, kind, scope, status, {}, {}, members FROM {} WHERE id = {}",
        d.text_of(t.column("ref").expect("ref")),
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| {
            Ok(Item {
                id: r.int(0)?,
                kind: r.text(1)?.to_string(),
                scope: r.text(2)?.to_string(),
                status: r.text(3)?.to_string(),
                reference: json(r.opt_text(4)?),
                evidence: json(r.opt_text(5)?),
                members: r.opt_int(6)?.unwrap_or(0),
            })
        })
        .transpose()
}

/// Who decided.
#[derive(Debug, Clone)]
pub struct Author<'a> {
    pub who: &'a str,
    /// `person`, `agent` or `model`.
    pub kind: &'a str,
    pub version: Option<&'a str>,
    /// The registered model, when the author is a model (record 42, D15).
    pub model: Option<i64>,
}

/// The one verb (§10.2): a decision applied to an item.
#[derive(Debug, Clone)]
pub struct Apply<'a> {
    pub item: i64,
    /// For a grouped item, one member to decide on its own; none means
    /// every member, as one decision with the group's scope.
    pub member: Option<i64>,
    /// How far a member's answer reaches: `stack`, `series`, `subject` or
    /// `origin`. A group decision is scoped to the group.
    pub scope: &'a str,
    /// The value, or none for "the axis has no value here".
    pub value: Option<&'a str>,
    pub author: Author<'a>,
    /// Written but not in force until committed.
    pub stage: bool,
    pub why: Option<&'a str>,
    /// The campaign whose answer this is, when one closed into it (record
    /// 42 S1).
    pub campaign: Option<i64>,
}

/// What an apply did.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The registered model that answered, when a model did.
    pub model: Option<crate::model::Model>,
    pub decision: i64,
    pub axis: String,
    /// The scope the decision was written at, and what it names.
    pub scope: String,
    pub reference: String,
    /// The items closed (or staged) by it.
    pub closed: Vec<i64>,
    /// Members decided by it, for a grouped item.
    pub members: i64,
    pub staged: bool,
}

/// Which stack, series, subject or origin a member's answer names.
fn resolve_scope(store: &mut Store, scope: &str, stack: i64) -> Result<(String, String), Error> {
    let d = store.dialect();
    match scope {
        "stack" => Ok(("stack".into(), stack.to_string())),
        "series" | "subject" => {
            let column = if scope == "series" {
                "k.series_id"
            } else {
                "r.subject_id"
            };
            let sql = format!(
                "SELECT {column} FROM {} AS k JOIN {} AS r ON r.id = k.series_id WHERE k.id = {}",
                store.qualified("stack"),
                store.qualified("series"),
                d.param(1, Type::Int)
            );
            let found = store
                .query_opt(&sql, &[Param::Int(stack)])?
                .ok_or_else(|| refused(format!("stack {stack} is not in the registry")))?;
            Ok((scope.to_string(), found.int(0)?.to_string()))
        }
        "origin" => {
            let sql = format!(
                "SELECT manufacturer FROM {} WHERE stack_id = {}",
                store.qualified("stack_fingerprint"),
                d.param(1, Type::Int)
            );
            let made_by = store
                .query_opt(&sql, &[Param::Int(stack)])?
                .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
                .filter(|m| !m.is_empty())
                .ok_or_else(|| {
                    refused(format!(
                        "stack {stack} names no manufacturer, so there is no origin to decide about"
                    ))
                })?;
            Ok((
                "origin".to_string(),
                format!("manufacturer={}", made_by.to_lowercase()),
            ))
        }
        other => Err(refused(format!(
            "{other} is not a scope: stack, series, subject or origin"
        ))),
    }
}

/// Apply a decision (§10.2). One row at its scope, the earlier decision on
/// the same key withdrawn rather than overwritten, the item closed with
/// the same words (or staged), and the audit row with the epoch, all in one
/// transaction.
pub fn apply(registry: &mut Registry, a: &Apply<'_>) -> Result<Applied, Error> {
    registry.store().begin()?;
    match apply_within(registry, a) {
        Ok(applied) => {
            registry.store().commit()?;
            Ok(applied)
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(e)
        }
    }
}

/// [`apply`] inside a transaction the caller holds, so that many decisions
/// are written as one act or none (an import); the caller commits, or
/// rolls back and re-reads the registry's epoch.
pub fn apply_within(registry: &mut Registry, a: &Apply<'_>) -> Result<Applied, Error> {
    apply_as(registry, a, None)
}

/// [`apply_within`] on one axis of an item that asks several (record 45):
/// a campaign's `axes` item, or System 1's `classify.asked`, whose evidence
/// names them in `axes`. The decision is the axis's, at the stack's scope,
/// and it answers every open question about that axis on the stack; the
/// item itself stays as it was, since its other axes are answered by their
/// own decisions, and whoever writes the last closes it.
pub fn apply_axis_within(
    registry: &mut Registry,
    a: &Apply<'_>,
    axis: &str,
) -> Result<Applied, Error> {
    apply_as(registry, a, Some(axis))
}

/// One answer to an item that asks several axes, a decision per axis, in
/// one transaction with the item closed (record 45 R5: choosing one of
/// System 1's candidates answers every axis at once). `values` names each
/// axis with its value, none for "no value here"; the whole is refused and
/// nothing written when any axis is.
pub fn apply_values(
    registry: &mut Registry,
    a: &Apply<'_>,
    values: &[(String, Option<String>)],
) -> Result<Vec<Applied>, Error> {
    if values.is_empty() {
        return Err(refused("name a value for each axis the item asks"));
    }
    registry.store().begin()?;
    let written = (|| -> Result<Vec<Applied>, Error> {
        let mut out = Vec::new();
        for (axis, value) in values {
            let one = Apply {
                value: value.as_deref(),
                ..a.clone()
            };
            out.push(apply_as(registry, &one, Some(axis))?);
        }
        let staged = out.iter().any(|x| x.staged);
        let decisions: serde_json::Map<String, serde_json::Value> = out
            .iter()
            .map(|x| (x.axis.clone(), serde_json::json!(x.decision)))
            .collect();
        let answer = serde_json::json!({
            "decisions": decisions, "actor": a.author.who, "author_kind": a.author.kind,
            "model_id": a.author.model, "why": a.why, "staged": staged,
        });
        let first = out.first().map(|x| x.decision).unwrap_or_default();
        close_item(
            registry.store(),
            a.item,
            if staged { "staged" } else { "accepted" },
            a.author.who,
            &answer,
            first,
            &now_iso(),
        )?;
        Ok(out)
    })();
    match written {
        Ok(out) => {
            registry.store().commit()?;
            Ok(out)
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(e)
        }
    }
}

fn apply_as(
    registry: &mut Registry,
    a: &Apply<'_>,
    one_of: Option<&str>,
) -> Result<Applied, Error> {
    if !["person", "agent", "model"].contains(&a.author.kind) {
        return Err(refused(format!(
            "an author is a person, an agent or a model, not {}",
            a.author.kind
        )));
    }
    // Record 42 S2 (D15): a model's answer names a registered model that is
    // admitted or promoted, and carries that model's version, not one the
    // caller typed. Record 42 R6: it is evidence until a person commits it,
    // so it is always staged.
    let model = match (a.author.kind, a.author.model) {
        ("model", Some(id)) => Some(crate::model::author(registry.store(), id).map_err(
            |e| match e {
                crate::model::Error::Store(s) => Error::Store(s),
                other => refused(other.to_string()),
            },
        )?),
        ("model", None) => {
            return Err(refused(
                "a model's decision names the registered model that answered (D15): nils model list",
            ));
        }
        (_, Some(_)) => {
            return Err(refused(format!(
                "a {} is not a model; only a model's decision names a model",
                a.author.kind
            )));
        }
        _ => None,
    };
    if let (Some(m), Some(v)) = (&model, a.author.version)
        && v != m.version
    {
        return Err(refused(format!(
            "model {} is {} at version {}, not {v}",
            m.id, m.name, m.version
        )));
    }
    let version = model
        .as_ref()
        .map(|m| m.version.as_str())
        .or(a.author.version);
    // R6, which the wave's ruling extends to agents: a model's or an
    // agent's answer is evidence until a person commits it.
    let stage = a.stage || model.is_some() || a.author.kind != "person";
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let Some(it) = item(store, a.item)? else {
        return Err(refused(format!("no review item {}", a.item)));
    };
    let axis = match one_of {
        // an item that asks several axes is answered one axis at a time
        Some(want) => {
            let asks = it.evidence["axes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str())
                .any(|x| x == want)
                || it.evidence["axis"].as_str() == Some(want);
            if !asks {
                return Err(refused(format!(
                    "review item {} does not ask about {want}",
                    it.id
                )));
            }
            if it.scope == "group" {
                return Err(refused(format!(
                    "review item {} is a group; one of its axes is answered on a stack",
                    it.id
                )));
            }
            want.to_string()
        }
        None => it.evidence["axis"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                refused(format!(
                    "review item {} is a {}, which is not a question about an axis",
                    it.id, it.kind
                ))
            })?,
    };
    // A model answers the task it was registered for, and no other.
    if let Some(m) = &model
        && m.task != format!("axis:{axis}")
    {
        return Err(refused(format!(
            "model {} ({}) answers {}, and review item {} asks about the axis {axis}",
            m.id,
            m.label(),
            m.task,
            it.id
        )));
    }
    if it.status != "open" && it.status != "staged" {
        return Err(refused(format!(
            "review item {} is already {}; decide the axis again with a new run",
            it.id, it.status
        )));
    }
    // Where the decision is written.
    let (scope, reference, member_stack): (String, String, Option<i64>) = match it.scope.as_str() {
        "group" => match a.member {
            None => {
                if a.scope != "stack" {
                    return Err(refused(
                        "a decision on a whole group is scoped to the group; name --member to reach wider from one stack",
                    ));
                }
                ("group".to_string(), it.id.to_string(), None)
            }
            Some(stack) => {
                let known = members(store, it.id)?.iter().any(|m| m.stack_id == stack);
                if !known {
                    return Err(refused(format!(
                        "stack {stack} is not a member of review item {}",
                        it.id
                    )));
                }
                let (s, r) = resolve_scope(store, a.scope, stack)?;
                (s, r, Some(stack))
            }
        },
        _ => {
            let stack = it.reference["stack_id"]
                .as_i64()
                .ok_or_else(|| refused(format!("review item {} names no stack", it.id)))?;
            let (s, r) = resolve_scope(store, a.scope, stack)?;
            (s, r, None)
        }
    };
    let now = now_iso();
    let d = store.dialect();
    // C15: a person over an agent over a model. A decision in force on this
    // key by a higher rank is not overridden by a lower one; it is refused,
    // and the lower author is told who decided.
    let standing = format!(
        "SELECT author_kind, actor FROM {} WHERE scope = {} AND ref = {} AND axis = {} \
         AND withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL) \
         ORDER BY id DESC LIMIT 1",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
    );
    if let Some(r) = store.query_opt(
        &standing,
        &[
            Param::from(scope.as_str()),
            Param::from(reference.as_str()),
            Param::from(axis.as_str()),
        ],
    )? {
        let held_kind = r.opt_text(0)?.unwrap_or("person").to_string();
        let held_by = r.opt_text(1)?.unwrap_or("").to_string();
        if rank(&held_kind) > rank(a.author.kind) {
            return Err(refused(format!(
                "{} at {scope} {reference} was decided by a {held_kind} ({held_by}); a {} does not override that (C15). Withdraw it first if it was wrong.",
                axis, a.author.kind
            )));
        }
    }
    let written = (|| -> Result<(i64, Vec<i64>, i64), Error> {
        // The earlier decision on the same key gives way to this one; it is
        // withdrawn, never deleted. A staged one replaces only staged ones.
        let withdraw = format!(
            "UPDATE {} SET withdrawn_at = {} WHERE scope = {} AND ref = {} AND axis = {} \
             AND withdrawn_at IS NULL AND {}",
            store.qualified("decision"),
            d.param(1, Type::Timestamp),
            d.param(2, Type::Text),
            d.param(3, Type::Text),
            d.param(4, Type::Text),
            if stage {
                "staged_at IS NOT NULL AND committed_at IS NULL"
            } else {
                "1 = 1"
            }
        );
        store.execute(
            &withdraw,
            &[
                Param::from(now.as_str()),
                Param::from(scope.as_str()),
                Param::from(reference.as_str()),
                Param::from(axis.as_str()),
            ],
        )?;
        // Wave 4c §5.5: the actor object beside the author, absent being its own value.
        let actor_detail = crate::actor::current();
        let decision = store
            .insert(
                &Insert::new(
                    table("decision"),
                    &[
                        "scope",
                        "ref",
                        "axis",
                        "value",
                        "actor",
                        "author_kind",
                        "author_version",
                        "actor_detail",
                        "why",
                        "decided_at",
                        "staged_at",
                        "committed_at",
                        "epoch_staged",
                        "model_id",
                        "campaign_id",
                        "committed_by",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(scope.as_str()),
                    Param::from(reference.as_str()),
                    Param::from(axis.as_str()),
                    a.value.map_or(Param::Null, Param::from),
                    Param::from(a.author.who),
                    Param::from(a.author.kind),
                    version.map_or(Param::Null, Param::from),
                    Param::from(actor_detail.to_string()),
                    a.why.map_or(Param::Null, Param::from),
                    Param::from(now.as_str()),
                    if stage {
                        Param::from(now.as_str())
                    } else {
                        Param::Null
                    },
                    if stage {
                        Param::Null
                    } else {
                        Param::from(now.as_str())
                    },
                    if stage {
                        Param::Int(epoch)
                    } else {
                        Param::Null
                    },
                    a.author.model.map_or(Param::Null, Param::Int),
                    a.campaign.map_or(Param::Null, Param::Int),
                    // Written in force, it is in force by its own author.
                    if stage {
                        Param::Null
                    } else {
                        Param::from(a.author.who)
                    },
                ]],
            )?
            .first()
            .ok_or_else(|| StoreError::Message("the decision was not written back".into()))?
            .int(0)?;
        let answer = serde_json::json!({
            "axis": axis,
            "value": a.value,
            "actor": a.author.who,
            "author_kind": a.author.kind,
            "model_version": version,
            "model_id": a.author.model,
            "campaign_id": a.campaign,
            "actor_detail": actor_detail,
            "why": a.why,
            "decision": decision,
            "staged": stage,
        });
        let status = if stage { "staged" } else { "accepted" };
        let mut closed = Vec::new();
        let mut decided_members = 0i64;
        match member_stack {
            Some(stack) => {
                // One member: mark it, and close the item when the last
                // member is decided.
                store.execute(
                    &format!(
                        "UPDATE {} SET decided_at = {} WHERE item_id = {} AND stack_id = {}",
                        store.qualified("review_member"),
                        d.param(1, Type::Timestamp),
                        d.param(2, Type::Int),
                        d.param(3, Type::Int)
                    ),
                    &[
                        Param::from(now.as_str()),
                        Param::Int(it.id),
                        Param::Int(stack),
                    ],
                )?;
                decided_members = 1;
                let left = store
                    .query_opt(
                        &format!(
                            "SELECT COUNT(*) FROM {} WHERE item_id = {} AND decided_at IS NULL",
                            store.qualified("review_member"),
                            d.param(1, Type::Int)
                        ),
                        &[Param::Int(it.id)],
                    )?
                    .map(|r| r.int(0))
                    .transpose()?
                    .unwrap_or(0);
                if left == 0 {
                    close_item(store, it.id, status, a.author.who, &answer, decision, &now)?;
                    closed.push(it.id);
                }
            }
            None => {
                if it.scope == "group" {
                    decided_members = it.members;
                    store.execute(
                        &format!(
                            "UPDATE {} SET decided_at = {} WHERE item_id = {} AND decided_at IS NULL",
                            store.qualified("review_member"),
                            d.param(1, Type::Timestamp),
                            d.param(2, Type::Int)
                        ),
                        &[Param::from(now.as_str()), Param::Int(it.id)],
                    )?;
                    close_item(store, it.id, status, a.author.who, &answer, decision, &now)?;
                    closed.push(it.id);
                } else {
                    // A per-stack item: every open question about this axis
                    // on this stack is answered by the one decision.
                    let t = table("review_item");
                    let same = format!(
                        "SELECT id, {} FROM {} WHERE status = 'open' AND scope = 'stack' AND kind LIKE {}",
                        d.text_of(t.column("ref").expect("ref")),
                        store.qualified("review_item"),
                        d.param(1, Type::Text),
                    );
                    let found = store.query(&same, &[Param::from(format!("{axis}:%"))])?;
                    for r in &found {
                        let its: serde_json::Value = r
                            .opt_text(1)?
                            .and_then(|t| serde_json::from_str(t).ok())
                            .unwrap_or(serde_json::Value::Null);
                        if its == it.reference {
                            let id = r.int(0)?;
                            close_item(store, id, status, a.author.who, &answer, decision, &now)?;
                            closed.push(id);
                        }
                    }
                }
            }
        }
        Ok((decision, closed, decided_members))
    })();
    let (decision, closed, decided_members) = written?;
    audit::record_judging(
        registry,
        &Entry {
            principal: a.author.who,
            action: Action::Decision,
            scope: serde_json::json!({
                "review_item": a.item, "scope": scope, "ref": reference, "axis": axis,
                "closed": closed.len(), "members": decided_members, "decision": decision,
            }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({
                "value": a.value, "author_kind": a.author.kind,
                "model_version": version, "model_id": a.author.model,
                "campaign_id": a.campaign, "why": a.why, "staged": stage,
            })),
        },
        !stage,
    )?;
    Ok(Applied {
        model,
        decision,
        axis,
        scope,
        reference,
        closed,
        members: decided_members,
        staged: stage,
    })
}

fn close_item(
    store: &mut Store,
    id: i64,
    status: &str,
    who: &str,
    answer: &serde_json::Value,
    decision: i64,
    now: &str,
) -> Result<(), StoreError> {
    let d = store.dialect();
    let close = format!(
        "UPDATE {} SET status = {}, decided_at = {}, actor = {}, decision = {}, decision_id = {} \
         WHERE id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Text),
        d.param(4, Type::Json),
        d.param(5, Type::Int),
        d.param(6, Type::Int),
    );
    store.execute(
        &close,
        &[
            Param::from(status),
            Param::from(now),
            Param::from(who),
            Param::from(answer.to_string()),
            Param::Int(decision),
            Param::Int(id),
        ],
    )?;
    Ok(())
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Committed {
    pub decisions: Vec<i64>,
    pub items: i64,
}

/// Commit staged decisions: the one named, or every one. A staged
/// decision carries the epoch it was staged at; if the registry moved on
/// since, the commit is refused unless `anyway`, because what the person
/// looked at is not what is there now (v0's drift signature).
pub fn commit(
    registry: &mut Registry,
    decision: Option<i64>,
    anyway: bool,
    who: &str,
) -> Result<Committed, Error> {
    commit_as(registry, decision, anyway, who, "person")
}

/// [`commit`], by a committer of a kind: a person, an agent or a model.
/// Record 42 R6: a model's answer is put in force only by a person, so an
/// agent or a model that would commit one is refused.
pub fn commit_as(
    registry: &mut Registry,
    decision: Option<i64>,
    anyway: bool,
    who: &str,
    kind: &str,
) -> Result<Committed, Error> {
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let d = store.dialect();
    let mut sql = format!(
        "SELECT id, epoch_staged, author_kind FROM {} WHERE staged_at IS NOT NULL AND committed_at IS NULL \
         AND withdrawn_at IS NULL",
        store.qualified("decision")
    );
    let mut params = Vec::new();
    if let Some(id) = decision {
        params.push(Param::Int(id));
        sql.push_str(&format!(" AND id = {}", d.param(1, Type::Int)));
    }
    let rows: Vec<(i64, Option<i64>, String)> = store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            Ok((
                r.int(0)?,
                r.opt_int(1)?,
                r.opt_text(2)?.unwrap_or("").to_string(),
            ))
        })
        .collect::<Result<_, StoreError>>()?;
    let ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    only_a_person_commits(store, &ids, kind)?;
    let staged: Vec<(i64, Option<i64>)> = rows.into_iter().map(|(id, e, _)| (id, e)).collect();
    if staged.is_empty() {
        return Err(refused(match decision {
            Some(id) => format!("decision {id} is not staged"),
            None => "nothing is staged".to_string(),
        }));
    }
    let drifted: Vec<i64> = staged
        .iter()
        .filter(|(_, e)| e.is_some_and(|e| e != epoch))
        .map(|(id, _)| *id)
        .collect();
    if !drifted.is_empty() && !anyway {
        return Err(refused(format!(
            "the registry moved on since decision(s) {} were staged (epoch {} now); look again, or commit --anyway",
            drifted
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            epoch
        )));
    }
    let now = now_iso();
    let mut out = Committed::default();
    let ids: Vec<i64> = staged.iter().map(|(id, _)| *id).collect();
    store.begin()?;
    match put_in_force(store, &ids, who, &now) {
        Ok(items) => out.items = items,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    }
    store.commit()?;
    out.decisions = ids;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::Decision,
            scope: serde_json::json!({ "committed": out.decisions, "items": out.items }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({ "anyway": anyway })),
        },
    )?;
    Ok(out)
}

/// Put staged decisions in force, by who commits them, in a few statements
/// whatever their number: the decisions, then the items they staged, which
/// are accepted now (the item's decision JSON names the decision). Answers
/// how many items were accepted. The caller holds the transaction.
fn put_in_force(store: &mut Store, ids: &[i64], who: &str, now: &str) -> Result<i64, StoreError> {
    store.update_by_ids(
        table("decision"),
        &[
            ("committed_at", Param::from(now)),
            ("committed_by", Param::from(who)),
        ],
        "id",
        ids,
    )?;
    let mut items = 0i64;
    for chunk in ids.chunks(500) {
        let sql = format!(
            "UPDATE {} SET status = 'accepted' WHERE status = 'staged' AND decision_id IN ({})",
            store.qualified("review_item"),
            id_list(chunk)
        );
        items += store.execute(&sql, &[])? as i64;
    }
    Ok(items)
}

/// Ids as a list inside `IN (...)`: integers only, so nothing a caller said
/// reaches the statement.
fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Record 42 R6, which the ruling on wave 42 extends to agents: a model's
/// or an agent's answer is put in force by a person, so a committer who is
/// an agent or a model is refused when any of the staged decisions it would
/// commit carries one. Nothing is committed then.
fn only_a_person_commits(store: &mut Store, ids: &[i64], kind: &str) -> Result<(), Error> {
    if kind == "person" || ids.is_empty() {
        return Ok(());
    }
    let by_model = needs_a_person(store, ids)?;
    if by_model.is_empty() {
        return Ok(());
    }
    Err(refused(format!(
        "decision(s) {} carry a model's or an agent's answer, which a person puts in force (record 42 R6); {} does not",
        by_model
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        if kind == "agent" {
            "an agent"
        } else {
            "a model"
        }
    )))
}

/// The decisions among `ids` that carry a model's or an agent's answer: a
/// model or an agent is their author, they name a registered model, or a
/// campaign closed an item into them that a model or an agent answered (a
/// person and a model or an agent who agreed, record 42 R6), found by the
/// campaign item's link to the decision or by the review item the decision
/// answered, so the rule holds even where the first is missing.
pub fn needs_a_person(store: &mut Store, ids: &[i64]) -> Result<Vec<i64>, StoreError> {
    let mut out = Vec::new();
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id FROM {} WHERE id IN ({list}) AND (author_kind IN ('model', 'agent') OR model_id IS NOT NULL) \
             UNION SELECT i.decision_id FROM {} i JOIN {} a ON a.item_id = i.id \
             WHERE i.decision_id IN ({list}) AND a.author_kind IN ('model', 'agent') \
             UNION SELECT r.decision_id FROM {} r JOIN {} i ON i.review_item_id = r.id \
             JOIN {} a ON a.item_id = i.id \
             WHERE r.decision_id IN ({list}) AND a.author_kind IN ('model', 'agent')",
            store.qualified("decision"),
            store.qualified("campaign_item"),
            store.qualified("campaign_answer"),
            store.qualified("review_item"),
            store.qualified("campaign_item"),
            store.qualified("campaign_answer"),
        );
        for r in store.query(&sql, &[])? {
            out.push(r.int(0)?);
        }
        // record 45: an axes item closes into a decision per axis and links
        // one of them; the others name their campaign and their stack, and
        // the item of that campaign on that stack says who answered it
        let sql = format!(
            "SELECT id, campaign_id, ref FROM {} WHERE id IN ({list}) AND campaign_id IS NOT NULL AND scope = 'stack'",
            store.qualified("decision"),
        );
        let mut by_item: BTreeMap<(i64, i64), Vec<i64>> = BTreeMap::new();
        for r in store.query(&sql, &[])? {
            if let Ok(stack) = r.text(2)?.parse::<i64>() {
                by_item
                    .entry((r.int(1)?, stack))
                    .or_default()
                    .push(r.int(0)?);
            }
        }
        let campaigns: BTreeSet<i64> = by_item.keys().map(|(c, _)| *c).collect();
        if !campaigns.is_empty() {
            let sql = format!(
                "SELECT DISTINCT i.campaign_id, i.stack_id FROM {} i JOIN {} a ON a.item_id = i.id \
                 WHERE i.campaign_id IN ({}) AND i.stack_id IS NOT NULL AND a.author_kind IN ('model', 'agent')",
                store.qualified("campaign_item"),
                store.qualified("campaign_answer"),
                campaigns
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for r in store.query(&sql, &[])? {
                if let Some(ids) = by_item.get(&(r.int(0)?, r.int(1)?)) {
                    out.extend(ids);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Which staged decisions a commit by filter takes (record 42 S6, v0's
/// commit by minimum confidence; record 45 E3, v0's "Commit all N
/// matching" of a change matrix).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommitFilter {
    /// Only decisions whose confidence is at least this: the agreement of
    /// the campaign item that closed into it, else the `confidence` its
    /// review item's evidence names. A decision with no confidence is left.
    pub min_confidence: Option<f64>,
    /// Only decisions a campaign's close staged.
    pub campaign: Option<i64>,
    /// Only decisions a registered model authored, named by its id, its
    /// digest or `name@version`.
    pub model: Option<String>,
    /// Only decisions on this axis; `from` and `to` are values of it.
    pub axis: Option<String>,
    /// Only where the axis holds this value now, on each stack the decision
    /// reaches: the decision in force there, else the classifier's value. A
    /// multi-valued axis is its values joined by commas; empty for none.
    pub from: Option<String>,
    /// Only decisions that set this value; empty for "no value here".
    pub to: Option<String>,
    /// Only on these stacks.
    pub stacks: Option<Vec<i64>>,
    /// Each name a value of the axis goes by, its label or the form the
    /// classifier stores, to its identity, from the served pack: `from`,
    /// `to`, what a decision says and what a stack holds are compared as
    /// identities, so `T2*w` and `T2starw` are one value. Empty compares
    /// them as written.
    pub names: BTreeMap<String, String>,
}

impl CommitFilter {
    /// A value as a set of identities.
    fn set_of(&self, v: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = value_set(v)
            .into_iter()
            .map(|x| self.names.get(&x).cloned().unwrap_or(x))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn names_nothing(&self) -> bool {
        self.min_confidence.is_none()
            && self.campaign.is_none()
            && self.model.is_none()
            && self.axis.is_none()
            && self.from.is_none()
            && self.to.is_none()
            && self.stacks.is_none()
    }
}

/// What a commit by filter did, and what it left staged.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommittedPart {
    pub decisions: Vec<i64>,
    pub items: i64,
    /// Staged decisions the filter left as they were.
    pub left: i64,
    /// Group decisions only part of whose stacks the filter names: the part
    /// was written as decisions of those stacks by the same author and put
    /// in force, and the group's decision stays staged for the rest.
    pub split: Vec<i64>,
}

/// A value as a set of values: a multi-valued axis is stored joined by
/// commas, and none is the empty set.
fn value_set(v: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = v
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One staged decision as a commit by filter reads it.
struct Staged {
    id: i64,
    epoch: Option<i64>,
    scope: String,
    reference: String,
    axis: String,
    value: Option<String>,
    model_id: Option<i64>,
    campaign_id: Option<i64>,
    actor: String,
    author_kind: String,
    author_version: Option<String>,
    actor_detail: Option<String>,
    why: Option<String>,
}

/// The stacks each decision reaches: its own, its group's members, its
/// series' or its subject's; none for a decision about a machine.
fn reached(store: &mut Store, staged: &[&Staged]) -> Result<BTreeMap<i64, Vec<i64>>, StoreError> {
    let mut out: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let groups: Vec<i64> = staged
        .iter()
        .filter(|s| s.scope == "group")
        .filter_map(|s| s.reference.parse().ok())
        .collect();
    let mut members: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for chunk in groups.chunks(500) {
        let sql = format!(
            "SELECT item_id, stack_id FROM {} WHERE item_id IN ({}) ORDER BY stack_id",
            store.qualified("review_member"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            members.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
    }
    let wide = staged
        .iter()
        .any(|s| s.scope == "series" || s.scope == "subject");
    let mut by_series: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut by_subject: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    if wide {
        let sql = format!(
            "SELECT k.id, k.series_id, r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id",
            store.qualified("stack"),
            store.qualified("series")
        );
        for r in store.query(&sql, &[])? {
            by_series.entry(r.int(1)?).or_default().push(r.int(0)?);
            by_subject.entry(r.int(2)?).or_default().push(r.int(0)?);
        }
    }
    for s in staged {
        let id: Option<i64> = s.reference.parse().ok();
        let stacks = match s.scope.as_str() {
            "stack" => id.into_iter().collect(),
            "group" => id
                .and_then(|i| members.get(&i).cloned())
                .unwrap_or_default(),
            "series" => id
                .and_then(|i| by_series.get(&i).cloned())
                .unwrap_or_default(),
            "subject" => id
                .and_then(|i| by_subject.get(&i).cloned())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        out.insert(s.id, stacks);
    }
    Ok(out)
}

/// What an axis holds now on each of `stacks`: the decision in force,
/// resolved as the classifier resolves them, else the classifier's value,
/// each as its set of values (none for nothing).
pub fn holds_now(
    store: &mut Store,
    axis: &str,
    stacks: &[i64],
) -> Result<BTreeMap<i64, Vec<String>>, Error> {
    let mut out: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let decided = crate::labels::decision_labels(
        store,
        &crate::labels::DecisionQuery {
            axis,
            stacks: Some(stacks),
            ..Default::default()
        },
    )
    .map_err(|e| match e {
        crate::labels::Error::Store(s) => Error::Store(s),
        other => refused(other.to_string()),
    })?;
    for l in decided {
        if let Some(stack) = l.stack_id {
            out.insert(stack, value_set(l.value.as_deref()));
        }
    }
    let rest: Vec<i64> = stacks
        .iter()
        .copied()
        .filter(|s| !out.contains_key(s))
        .collect();
    let d = store.dialect();
    for chunk in rest.chunks(500) {
        let sql = format!(
            "SELECT stack_id, value FROM {} WHERE axis = {} AND stack_id IN ({})",
            store.qualified("classification_axis"),
            d.param(1, Type::Text),
            id_list(chunk)
        );
        let mut rows: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for r in store.query(&sql, &[Param::from(axis)])? {
            let e = rows.entry(r.int(0)?).or_default();
            e.extend(value_set(r.opt_text(1)?));
        }
        for s in chunk {
            let mut v = rows.remove(s).unwrap_or_default();
            v.sort();
            v.dedup();
            out.insert(*s, v);
        }
    }
    Ok(out)
}

/// One stack's part of a staged group decision, written as a staged
/// decision of that stack by the group's author, with its model, campaign
/// and actor, so the commit that follows puts it in force (record 45 E3).
/// The rules of the spine hold (C15): a decision in force there by a higher
/// rank leaves the stack with the group, and the earlier one on the same key
/// gives way, withdrawn and never deleted. The group's item and its own
/// decision stay as they are, staged for the rest.
fn split_off(
    store: &mut Store,
    s: &Staged,
    stack: i64,
    why: &str,
    epoch: i64,
    now: &str,
) -> Result<Option<i64>, Error> {
    let d = store.dialect();
    let key = [
        Param::from("stack"),
        Param::from(stack.to_string()),
        Param::from(s.axis.as_str()),
    ];
    let standing = format!(
        "SELECT author_kind FROM {} WHERE scope = {} AND ref = {} AND axis = {} \
         AND withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL) \
         ORDER BY id DESC LIMIT 1",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
    );
    if let Some(r) = store.query_opt(&standing, &key)?
        && rank(r.opt_text(0)?.unwrap_or("person")) > rank(&s.author_kind)
    {
        return Ok(None);
    }
    let withdraw = format!(
        "UPDATE {} SET withdrawn_at = {} WHERE scope = {} AND ref = {} AND axis = {} AND withdrawn_at IS NULL",
        store.qualified("decision"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        d.param(4, Type::Text),
    );
    store.execute(
        &withdraw,
        &[
            Param::from(now),
            key[0].clone(),
            key[1].clone(),
            key[2].clone(),
        ],
    )?;
    let id = store
        .insert(
            &Insert::new(
                table("decision"),
                &[
                    "scope",
                    "ref",
                    "axis",
                    "value",
                    "actor",
                    "author_kind",
                    "author_version",
                    "actor_detail",
                    "why",
                    "decided_at",
                    "staged_at",
                    "epoch_staged",
                    "model_id",
                    "campaign_id",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from("stack"),
                Param::from(stack.to_string()),
                Param::from(s.axis.as_str()),
                s.value.as_deref().map_or(Param::Null, Param::from),
                Param::from(s.actor.as_str()),
                Param::from(s.author_kind.as_str()),
                s.author_version.as_deref().map_or(Param::Null, Param::from),
                s.actor_detail.as_deref().map_or(Param::Null, Param::from),
                Param::from(why),
                Param::from(now),
                Param::from(now),
                Param::Int(epoch),
                s.model_id.map_or(Param::Null, Param::Int),
                s.campaign_id.map_or(Param::Null, Param::Int),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the decision was not written back".into()))?
        .int(0)?;
    Ok(Some(id))
}

/// Commit the part of the staged decisions a filter names, in one
/// transaction, with the drift check [`commit`] makes: the rest stay
/// staged. A filter that names nothing is refused, so that a commit of
/// everything is always said as such. The committer is of a kind, as in
/// [`commit_as`]: an agent or a model is refused when the part holds a
/// model's answer (record 42 R6), and then nothing is committed.
///
/// Record 45 E3: `model`, `axis`, `from`, `to` and `stacks`. `from` and
/// `stacks` are held against every stack a decision reaches. A decision all
/// of whose stacks pass is committed whole; a group's decision only some of
/// whose members pass is split, its passing members each written as a
/// decision of that stack by the group's author through [`apply_within`]
/// and put in force, the group's own decision left staged for the rest; a
/// series' or a subject's that reaches past the filter is left.
pub fn commit_where(
    registry: &mut Registry,
    filter: &CommitFilter,
    anyway: bool,
    who: &str,
    kind: &str,
) -> Result<CommittedPart, Error> {
    if filter.names_nothing() {
        return Err(refused(
            "a commit by filter names a minimum confidence, a campaign, a model, an axis, from, to or stacks; commit --all commits everything",
        ));
    }
    if (filter.from.is_some() || filter.to.is_some()) && filter.axis.is_none() {
        return Err(refused(
            "from and to are values of an axis; name the axis beside them",
        ));
    }
    if filter.stacks.as_ref().is_some_and(Vec::is_empty) {
        return Err(refused("stacks names no stack"));
    }
    let model_id = match &filter.model {
        Some(r) => Some(
            crate::model::resolve(registry.store(), r)?
                .ok_or_else(|| refused(format!("no registered model answers to {r}")))?
                .id,
        ),
        None => None,
    };
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let staged: Vec<Staged> = store
        .query(
            &format!(
                "SELECT id, epoch_staged, scope, ref, axis, value, model_id, campaign_id, actor, author_kind, why, \
                 author_version, {} FROM {} WHERE staged_at IS NOT NULL AND committed_at IS NULL \
                 AND withdrawn_at IS NULL ORDER BY id",
                store
                    .dialect()
                    .text_of(table("decision").column("actor_detail").expect("actor_detail")),
                store.qualified("decision")
            ),
            &[],
        )?
        .iter()
        .map(|r| {
            Ok(Staged {
                id: r.int(0)?,
                epoch: r.opt_int(1)?,
                scope: r.text(2)?.to_string(),
                reference: r.text(3)?.to_string(),
                axis: r.text(4)?.to_string(),
                value: r.opt_text(5)?.map(str::to_string),
                model_id: r.opt_int(6)?,
                campaign_id: r.opt_int(7)?,
                actor: r.text(8)?.to_string(),
                author_kind: r.text(9)?.to_string(),
                why: r.opt_text(10)?.map(str::to_string),
                author_version: r.opt_text(11)?.map(str::to_string),
                actor_detail: r.opt_text(12)?.map(str::to_string),
            })
        })
        .collect::<Result<_, StoreError>>()?;
    // what each staged decision's confidence and campaign are, read for
    // all of them at once: the confidence its first review item names,
    // and the agreement and campaign of the last campaign item closed into
    // it, which wins; a decision a campaign's close wrote names its
    // campaign itself too (an axes item closes into several)
    let t = table("review_item");
    let d = store.dialect();
    let mut confidence: BTreeMap<i64, f64> = BTreeMap::new();
    let mut campaign_of: BTreeMap<i64, i64> = staged
        .iter()
        .filter_map(|s| s.campaign_id.map(|c| (s.id, c)))
        .collect();
    let all: Vec<i64> = staged.iter().map(|s| s.id).collect();
    for chunk in all.chunks(500) {
        let sql = format!(
            "SELECT decision_id, {} FROM {} WHERE decision_id IN ({}) ORDER BY id DESC",
            d.text_of(t.column("evidence").expect("evidence")),
            store.qualified("review_item"),
            id_list(chunk)
        );
        // the lowest id is read last, so it is the one kept
        for r in store.query(&sql, &[])? {
            let evidence: serde_json::Value = r
                .opt_text(1)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(serde_json::Value::Null);
            match evidence["confidence"].as_f64() {
                Some(c) => confidence.insert(r.int(0)?, c),
                None => confidence.remove(&r.int(0)?),
            };
        }
        let sql = format!(
            "SELECT decision_id, campaign_id, agreement FROM {} WHERE decision_id IN ({}) ORDER BY id",
            store.qualified("campaign_item"),
            id_list(chunk)
        );
        // the highest id is read last, so it is the one kept
        let mut agreement: BTreeMap<i64, Option<f64>> = BTreeMap::new();
        for r in store.query(&sql, &[])? {
            let id = r.int(0)?;
            campaign_of.insert(id, r.int(1)?);
            agreement.insert(id, r.opt_double(2)?);
        }
        for (id, a) in agreement {
            if let Some(a) = a {
                confidence.insert(id, a);
            }
        }
    }
    let to = filter.to.as_deref().map(|v| filter.set_of(Some(v)));
    let from = filter.from.as_deref().map(|v| filter.set_of(Some(v)));
    let wanted: Option<BTreeSet<i64>> = filter.stacks.as_ref().map(|s| s.iter().copied().collect());
    // the decisions the filter names as decisions, before their stacks
    let named: Vec<&Staged> = staged
        .iter()
        .filter(|s| {
            filter
                .min_confidence
                .is_none_or(|min| confidence.get(&s.id).is_some_and(|c| *c >= min))
                && filter
                    .campaign
                    .is_none_or(|want| campaign_of.get(&s.id) == Some(&want))
                && model_id.is_none_or(|m| s.model_id == Some(m))
                && filter.axis.as_deref().is_none_or(|a| s.axis == a)
                && to
                    .as_ref()
                    .is_none_or(|t| filter.set_of(s.value.as_deref()) == *t)
        })
        .collect();
    let per_stack = from.is_some() || wanted.is_some();
    let reach = if per_stack {
        reached(store, &named)?
    } else {
        BTreeMap::new()
    };
    let now_holds = match (&from, filter.axis.as_deref()) {
        (Some(_), Some(axis)) => {
            let every: Vec<i64> = reach
                .values()
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            holds_now(store, axis, &every)?
        }
        _ => BTreeMap::new(),
    };
    let passes = |stack: &i64| -> bool {
        wanted.as_ref().is_none_or(|w| w.contains(stack))
            && from.as_ref().is_none_or(|f| {
                now_holds
                    .get(stack)
                    .is_some_and(|v| filter.set_of(Some(&v.join(","))) == *f)
            })
    };
    let mut whole: Vec<&Staged> = Vec::new();
    let mut parts: Vec<(&Staged, Vec<i64>)> = Vec::new();
    for s in &named {
        if !per_stack {
            whole.push(s);
            continue;
        }
        let stacks = reach.get(&s.id).cloned().unwrap_or_default();
        let taking: Vec<i64> = stacks.iter().filter(|x| passes(x)).copied().collect();
        if !stacks.is_empty() && taking.len() == stacks.len() {
            whole.push(s);
        } else if !taking.is_empty() && s.scope == "group" {
            parts.push((s, taking));
        }
    }
    let mut out = CommittedPart::default();
    let touched: Vec<i64> = whole
        .iter()
        .map(|s| s.id)
        .chain(parts.iter().map(|(s, _)| s.id))
        .collect();
    only_a_person_commits(store, &touched, kind)?;
    let drifted: Vec<i64> = whole
        .iter()
        .copied()
        .chain(parts.iter().map(|(s, _)| *s))
        .filter(|s| s.epoch.is_some_and(|e| e != epoch))
        .map(|s| s.id)
        .collect();
    if !drifted.is_empty() && !anyway {
        return Err(refused(format!(
            "the registry moved on since decision(s) {} were staged (epoch {} now); look again, or commit --anyway",
            drifted
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            epoch
        )));
    }
    if touched.is_empty() {
        out.left = staged.len() as i64;
        return Ok(out);
    }
    let now = now_iso();
    registry.store().begin()?;
    let written = (|| -> Result<(Vec<i64>, Vec<i64>, i64), Error> {
        let mut ids: Vec<i64> = whole.iter().map(|s| s.id).collect();
        let mut split = Vec::new();
        for (s, stacks) in &parts {
            let why = format!(
                "part of staged decision {} (group), committed by a filter{}",
                s.id,
                s.why
                    .as_deref()
                    .map(|w| format!("; {w}"))
                    .unwrap_or_default()
            );
            let mut took = false;
            for stack in stacks {
                if let Some(id) = split_off(registry.store(), s, *stack, &why, epoch, &now)? {
                    ids.push(id);
                    took = true;
                }
            }
            if took {
                split.push(s.id);
            }
        }
        let items = put_in_force(registry.store(), &ids, who, &now)?;
        Ok((ids, split, items))
    })();
    let (ids, split, items) = match written {
        Ok(w) => w,
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            return Err(e);
        }
    };
    registry.store().commit()?;
    out.items = items;
    out.left = staged.len() as i64 - whole.len() as i64;
    out.decisions = ids;
    out.split = split;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::Decision,
            scope: serde_json::json!({
                "committed": out.decisions, "items": out.items, "left": out.left, "split": out.split,
            }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({
                "anyway": anyway, "min_confidence": filter.min_confidence, "campaign": filter.campaign,
                "model": model_id, "axis": filter.axis, "from": filter.from, "to": filter.to,
                "stacks": filter.stacks.as_ref().map(Vec::len),
            })),
        },
    )?;
    Ok(out)
}

/// Withdraw a decision, staged or committed: it stops being in force, the
/// items it closed open again, and nothing is deleted.
pub fn withdraw(registry: &mut Registry, decision: i64, who: &str) -> Result<i64, Error> {
    withdraw_as(registry, decision, who, "person")
}

/// [`withdraw`], by a withdrawer of a kind. A person withdraws any
/// decision; an agent or a model only one its own principal staged as that
/// kind and nobody put in force, since a decision in force is a person's
/// to take back (record 42 R6).
pub fn withdraw_as(
    registry: &mut Registry,
    decision: i64,
    who: &str,
    kind: &str,
) -> Result<i64, Error> {
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT withdrawn_at IS NOT NULL, author_kind, actor, \
         CASE WHEN staged_at IS NOT NULL AND committed_at IS NULL THEN 1 ELSE 0 END \
         FROM {} WHERE id = {}",
        store.qualified("decision"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(decision)])? else {
        return Err(refused(format!("no decision {decision}")));
    };
    if kind != "person" {
        let own = row.opt_text(1)? == Some(kind) && row.opt_text(2)? == Some(who);
        if !own || row.int(3)? != 1 {
            return Err(Error::Forbidden(format!(
                "decision {decision} is withdrawn by a person; {} withdraws only a decision it staged itself and nobody put in force",
                if kind == "agent" {
                    "an agent"
                } else {
                    "a model"
                }
            )));
        }
    }
    if row.int(0)? != 0 {
        return Err(refused(format!("decision {decision} is already withdrawn")));
    }
    let now = now_iso();
    store.begin()?;
    let written = (|| -> Result<i64, StoreError> {
        store.update_by_id(
            table("decision"),
            &[("withdrawn_at", Param::from(now.as_str()))],
            "id",
            decision,
        )?;
        let sql = format!(
            "UPDATE {} SET status = 'open', decided_at = NULL, decision = NULL, decision_id = NULL \
             WHERE status IN ('accepted', 'staged') AND decision_id = {}",
            store.qualified("review_item"),
            d.param(1, Type::Int)
        );
        let reopened = store.execute(&sql, &[Param::Int(decision)])?;
        // Members decided by it are undecided again.
        let sql = format!(
            "UPDATE {} SET decided_at = NULL WHERE item_id IN (SELECT id FROM {} WHERE status = 'open' AND scope = 'group')",
            store.qualified("review_member"),
            store.qualified("review_item")
        );
        store.execute(&sql, &[])?;
        Ok(reopened as i64)
    })();
    let reopened = match written {
        Ok(n) => n,
        Err(e) => {
            store.rollback().ok();
            return Err(e.into());
        }
    };
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::Decision,
            scope: serde_json::json!({ "withdrawn": decision, "reopened": reopened }),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    Ok(reopened)
}

// ------------------------------------------------------------ identity items

/// Record 26 §4: the pseudonymiser holds the files of an identifier the
/// linkage store does not know, and asks once per dataset and shape of
/// identifier (digits as 9, letters as A), with the files held and when the
/// first was seen as the evidence; never a value. The item is keyed
/// `place:<id>|shape:<shape>` so a later run finds it, and a run that finds
/// nothing held under the shape closes it.
pub const UNMAPPED_KIND: &str = "identity.unmapped";

/// Record 26 §4: a subject the pseudonymiser made from an identifier no
/// map named, coded anyway under the key: one open item per subject, the
/// files written under it as evidence, so a person knows the subject is
/// provisional until a map names it. A merge of the subject closes it.
pub const PROVISIONAL_KIND: &str = "identity.provisional";

/// The status a run gives an item it answers itself: a later run asked
/// again and found nothing held under the shape, so there is nothing left
/// to decide. The review-item contract's word for that is superseded.
pub const RESOLVED: &str = "superseded";

/// What groups the open `identity.unmapped` item of a dataset and shape,
/// so a later run finds it by an exact text and not by matching JSON.
pub fn unmapped_key(place_id: i64, shape: &str) -> String {
    format!("place:{place_id}|shape:{shape}")
}

/// What groups the open `identity.provisional` item of a subject.
pub fn provisional_key(subject_id: i64) -> String {
    format!("subject:{subject_id}")
}

/// The open item of a kind under a group key, if there is one: its id and
/// its evidence as written.
fn open_item(
    store: &mut Store,
    kind: &str,
    key: &str,
) -> Result<Option<(i64, serde_json::Value)>, StoreError> {
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT id, {} FROM {} WHERE kind = {} AND group_key = {} AND status = 'open' ORDER BY id DESC LIMIT 1",
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let Some(r) = store.query_opt(&sql, &[Param::from(kind), Param::from(key)])? else {
        return Ok(None);
    };
    let id = r.int(0)?;
    let evidence = r
        .opt_text(1)?
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(Some((id, evidence)))
}

/// Bring an open item up to date: its evidence, its members and the job
/// that last asked.
fn refresh_item(
    store: &mut Store,
    id: i64,
    evidence: &serde_json::Value,
    members: i64,
    job_id: Option<i64>,
) -> Result<(), StoreError> {
    store.update_by_id(
        table("review_item"),
        &[
            ("evidence", Param::from(evidence.to_string())),
            ("members", Param::Int(members)),
            ("job_id", job_id.map_or(Param::Null, Param::Int)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// Open one item of a kind under a group key. Answers its id.
#[allow(clippy::too_many_arguments)]
fn open_new(
    store: &mut Store,
    kind: &str,
    scope: &str,
    key: &str,
    reference: &serde_json::Value,
    evidence: &serde_json::Value,
    members: i64,
    job_id: Option<i64>,
    now: &str,
) -> Result<i64, StoreError> {
    let rows = store.insert(
        &Insert::new(
            table("review_item"),
            &[
                "kind",
                "scope",
                "ref",
                "evidence",
                "status",
                "created_at",
                "job_id",
                "members",
                "group_key",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(kind),
            Param::from(scope),
            Param::from(reference.to_string()),
            Param::from(evidence.to_string()),
            Param::from("open"),
            Param::from(now),
            job_id.map_or(Param::Null, Param::Int),
            Param::Int(members),
            Param::from(key),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| StoreError::Message("the review item was not written back".into()))?
        .int(0)
}

/// The held files of one dataset and shape, as the pseudonymiser counts them.
#[derive(Debug, Clone)]
pub struct Unmapped<'a> {
    pub place_id: i64,
    pub place: &'a str,
    pub shape: &'a str,
    pub id_type: &'a str,
    pub files: i64,
    /// When the first file under the shape was held.
    pub first_seen: &'a str,
    pub batch_id: Option<i64>,
    pub job_id: Option<i64>,
}

/// Open, or bring up to date, the one `identity.unmapped` item of a dataset
/// and shape: an open one gets the new count, batch and job, else one is
/// opened at batch scope with `{place_id, place, shape, id_type}` as its
/// reference and `{files, first_seen, batch_id, shape, id_type}` as its
/// evidence. Answers the item's id.
pub fn raise_unmapped(store: &mut Store, u: &Unmapped<'_>, now: &str) -> Result<i64, StoreError> {
    let key = unmapped_key(u.place_id, u.shape);
    let reference = serde_json::json!({
        "place_id": u.place_id, "place": u.place, "shape": u.shape, "id_type": u.id_type,
    });
    let evidence = serde_json::json!({
        "files": u.files,
        "first_seen": u.first_seen,
        "batch_id": u.batch_id,
        "shape": u.shape,
        "id_type": u.id_type,
    });
    if let Some((id, _)) = open_item(store, UNMAPPED_KIND, &key)? {
        refresh_item(store, id, &evidence, u.files, u.job_id)?;
        return Ok(id);
    }
    open_new(
        store,
        UNMAPPED_KIND,
        "batch",
        &key,
        &reference,
        &evidence,
        u.files,
        u.job_id,
        now,
    )
}

/// Close the item of a dataset and shape, once a run holds nothing under
/// it: the run answered the question itself. Answers whether one was open.
pub fn close_unmapped(
    store: &mut Store,
    place_id: i64,
    shape: &str,
    now: &str,
) -> Result<bool, StoreError> {
    let key = unmapped_key(place_id, shape);
    let Some((id, _)) = open_item(store, UNMAPPED_KIND, &key)? else {
        return Ok(false);
    };
    store.update_by_id(
        table("review_item"),
        &[
            ("status", Param::from(RESOLVED)),
            ("decided_at", Param::from(now)),
        ],
        "id",
        id,
    )?;
    Ok(true)
}

/// The shapes a dataset has an open `identity.unmapped` item for.
pub fn open_unmapped_shapes(store: &mut Store, place_id: i64) -> Result<Vec<String>, StoreError> {
    let d = store.dialect();
    let t = table("review_item");
    let sql = format!(
        "SELECT {} FROM {} WHERE kind = {} AND status = 'open' AND group_key LIKE {}",
        d.text_of(t.column("ref").expect("ref")),
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let prefix = format!("place:{place_id}|shape:%");
    store
        .query(&sql, &[Param::from(UNMAPPED_KIND), Param::from(prefix)])?
        .iter()
        .filter_map(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|v| v["shape"].as_str().map(str::to_string))
        })
        .map(Ok)
        .collect()
}

/// A subject coded from an identifier the map did not know.
#[derive(Debug, Clone)]
pub struct Provisional<'a> {
    pub subject_id: i64,
    pub code: &'a str,
    pub id_type: &'a str,
    pub shape: &'a str,
    pub place_id: i64,
    pub place: &'a str,
    /// The files written under the subject by this run, counted up on an
    /// item already open.
    pub files: i64,
    pub batch_id: Option<i64>,
    pub job_id: Option<i64>,
}

/// One `identity.provisional` item per subject: opened when the
/// pseudonymiser makes the subject, with `{subject_id, code}` as its
/// reference and `{files, id_type, shape, place_id, place, batch_id}` as
/// its evidence, the files counted up by a later run; never an identifier.
/// Answers the item's id.
pub fn raise_provisional(
    store: &mut Store,
    p: &Provisional<'_>,
    now: &str,
) -> Result<i64, StoreError> {
    let key = provisional_key(p.subject_id);
    let reference = serde_json::json!({ "subject_id": p.subject_id, "code": p.code });
    if let Some((id, mut evidence)) = open_item(store, PROVISIONAL_KIND, &key)? {
        let before = evidence["files"].as_i64().unwrap_or(0);
        evidence["files"] = serde_json::json!(before + p.files);
        evidence["batch_id"] = serde_json::json!(p.batch_id);
        refresh_item(store, id, &evidence, 1, p.job_id)?;
        return Ok(id);
    }
    let evidence = serde_json::json!({
        "files": p.files,
        "id_type": p.id_type,
        "shape": p.shape,
        "place_id": p.place_id,
        "place": p.place,
        "batch_id": p.batch_id,
    });
    open_new(
        store,
        PROVISIONAL_KIND,
        "subject",
        &key,
        &reference,
        &evidence,
        1,
        p.job_id,
        now,
    )
}

/// Close the open `identity.provisional` item of a subject, when the
/// subject was merged: the person is now known under the canonical code.
/// Returns how many closed (one, or none).
pub fn close_provisional(
    store: &mut Store,
    subject_id: i64,
    actor: &str,
    decision: &serde_json::Value,
) -> Result<u64, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = 'superseded', decided_at = {}, actor = {}, decision = {} \
         WHERE kind = {} AND status = 'open' AND group_key = {}",
        store.qualified("review_item"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Text),
        d.param(3, Type::Json),
        d.param(4, Type::Text),
        d.param(5, Type::Text)
    );
    store.execute(
        &sql,
        &[
            Param::from(now_iso()),
            Param::from(actor),
            Param::from(decision.to_string()),
            Param::from(PROVISIONAL_KIND),
            Param::from(provisional_key(subject_id)),
        ],
    )
}

/// A study row that holds no series. The row is a fact in a file, so it
/// stays; what stops is the session made from it, and this says that the
/// row is there and empty. v0 kept an ingest-conflict table and v1 kept the
/// conflict itself, in silence: this is the silence ending.
pub const EMPTY_STUDY_KIND: &str = "identity.empty_study";

/// A subject that owns series and no study of its own, so it is on no
/// timeline: it gets no session, no `sub-` path and appears in no ask. It
/// happens where the files of one series name a patient their study does
/// not, which the digest counts as `series_multi_study` and nothing else
/// says.
pub const NO_STUDY_KIND: &str = "identity.no_study";

/// What groups the open `identity.empty_study` item of a study.
pub fn empty_study_key(study_id: i64) -> String {
    format!("study:{study_id}")
}

/// What groups the open `identity.no_study` item of a subject.
pub fn no_study_key(subject_id: i64) -> String {
    format!("subject:{subject_id}")
}

/// One open `identity.empty_study` item per study, brought up to date by a
/// later run. `ref` is `{study_id, subject_id, code}` and the evidence says
/// the day the study carries, so a person can tell an empty row from the
/// study it was meant to be.
pub fn raise_empty_study(
    store: &mut Store,
    study_id: i64,
    subject_id: i64,
    code: &str,
    day: Option<&str>,
    now: &str,
) -> Result<i64, StoreError> {
    let key = empty_study_key(study_id);
    let reference =
        serde_json::json!({ "study_id": study_id, "subject_id": subject_id, "code": code });
    let evidence = serde_json::json!({ "series": 0, "day": day, "session": false });
    if let Some((id, _)) = open_item(store, EMPTY_STUDY_KIND, &key)? {
        refresh_item(store, id, &evidence, 1, None)?;
        return Ok(id);
    }
    open_new(
        store,
        EMPTY_STUDY_KIND,
        "study",
        &key,
        &reference,
        &evidence,
        1,
        None,
        now,
    )
}

/// One open `identity.no_study` item per subject, with the series and the
/// instances it owns counted up by a later run.
pub fn raise_no_study(
    store: &mut Store,
    subject_id: i64,
    code: &str,
    series: i64,
    instances: i64,
    now: &str,
) -> Result<i64, StoreError> {
    let key = no_study_key(subject_id);
    let reference = serde_json::json!({ "subject_id": subject_id, "code": code });
    let evidence = serde_json::json!({ "series": series, "instances": instances, "studies": 0 });
    if let Some((id, _)) = open_item(store, NO_STUDY_KIND, &key)? {
        refresh_item(store, id, &evidence, 1, None)?;
        return Ok(id);
    }
    open_new(
        store,
        NO_STUDY_KIND,
        "subject",
        &key,
        &reference,
        &evidence,
        1,
        None,
        now,
    )
}

/// Close every open item of a kind whose group key this run did not find
/// again: the row it spoke about has series now, or the subject has a study,
/// so there is nothing left to decide. `keep` is every key still true.
pub fn close_resolved(
    store: &mut Store,
    kind: &str,
    keep: &[String],
    actor: &str,
    decision: &serde_json::Value,
) -> Result<u64, StoreError> {
    let d = store.dialect();
    let mut params: Vec<Param> = vec![
        Param::from(now_iso()),
        Param::from(actor),
        Param::from(decision.to_string()),
        Param::from(kind),
    ];
    let mut held = String::new();
    if !keep.is_empty() {
        let holes: Vec<String> = keep
            .iter()
            .enumerate()
            .map(|(i, _)| d.param(params.len() + i + 1, Type::Text))
            .collect();
        held = format!(" AND group_key NOT IN ({})", holes.join(", "));
        params.extend(keep.iter().map(|k| Param::from(k.as_str())));
    }
    let sql = format!(
        "UPDATE {} SET status = '{RESOLVED}', decided_at = {}, actor = {}, decision = {} \
         WHERE kind = {} AND status = 'open'{held}",
        store.qualified("review_item"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Text),
        d.param(3, Type::Json),
        d.param(4, Type::Text),
    );
    store.execute(&sql, &params)
}

// ---------------------------------------------------------------- pick items

/// Record 42 S3: a session whose pick is not to be trusted without a look,
/// because its two best candidates are too close, the winner is rare in its
/// population, or nothing was eligible (v0's `needs_check`, study 1.4). One
/// open item per pick model, role and occasion; a later run brings it up to
/// date or closes it, and a person's pick answers it.
pub const PICK_BORDER_KIND: &str = "pick.border";

/// What groups the open `pick.border` item of one role on one occasion.
pub fn pick_border_key(model: &str, role: &str, subject_id: i64, day: &str) -> String {
    format!("pick:{model}|role:{role}|subject:{subject_id}|day:{day}")
}

/// One occasion a pick run thought worth a person's eye.
#[derive(Debug, Clone)]
pub struct PickBorder<'a> {
    pub model: &'a str,
    pub role: &'a str,
    pub subject_id: i64,
    pub day: &'a str,
    /// The run's reasons, by name (`too_close`, `rare`, `nothing_eligible`).
    pub borders: &'a [&'a str],
    /// What the run wrote, when it wrote anything.
    pub pick_id: Option<i64>,
    pub score: Option<f64>,
    pub margin: Option<f64>,
    pub runner_up_score: Option<f64>,
    /// Every candidate's stacks and score, best first.
    pub considered: &'a serde_json::Value,
    pub job_id: Option<i64>,
}

/// Raise or refresh the open `pick.border` item of one occasion. Scope
/// `subject`, since an occasion is derived and has no row; `ref` names the
/// subject, the day, the role and the model, and the evidence is the run's
/// numbers, never a value of a person.
pub fn raise_pick_border(
    store: &mut Store,
    b: &PickBorder<'_>,
    now: &str,
) -> Result<i64, StoreError> {
    let key = pick_border_key(b.model, b.role, b.subject_id, b.day);
    let reference = serde_json::json!({
        "subject_id": b.subject_id, "session_day": b.day, "role": b.role, "model": b.model,
    });
    let candidates = b.considered.as_array().map_or(0, Vec::len) as i64;
    let evidence = serde_json::json!({
        "borders": b.borders, "pick_id": b.pick_id, "score": b.score, "margin": b.margin,
        "runner_up_score": b.runner_up_score, "candidates": candidates,
        "considered": b.considered,
    });
    if let Some((id, _)) = open_item(store, PICK_BORDER_KIND, &key)? {
        refresh_item(store, id, &evidence, candidates.max(1), b.job_id)?;
        return Ok(id);
    }
    open_new(
        store,
        PICK_BORDER_KIND,
        "subject",
        &key,
        &reference,
        &evidence,
        candidates.max(1),
        b.job_id,
        now,
    )
}

/// Record 43 S2: a unit of a pipeline run that failed, or that the run's
/// results did not name (D9's QC hook, its minimal form: thresholds on
/// metrics are Wave 7's). One item per run and unit.
pub const PIPELINE_QC_KIND: &str = "pipeline:qc";

/// What groups the item of one unit of one run.
pub fn pipeline_qc_key(run_id: i64, unit: &str) -> String {
    format!("run:{run_id}|unit:{unit}")
}

/// One unit a run could not vouch for.
#[derive(Debug, Clone)]
pub struct PipelineQc<'a> {
    pub run_id: i64,
    /// `name@version`.
    pub pipeline: &'a str,
    /// `sub-<s>`, `sub-<s>_ses-<t>` or `stack-<id>`, or `run` for the run's
    /// own files.
    pub unit: &'a str,
    pub stack_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub session_day: Option<&'a str>,
    /// `failed`, `unreported` for a unit the results did not name, or
    /// `refused` for a file the engine would not take (record 43 review).
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub metrics: &'a serde_json::Value,
    pub job_id: Option<i64>,
}

/// Raise the `pipeline:qc` item of one unit of a run: scope `run`, `ref`
/// naming the run, the pipeline and the unit with its registry ids, the
/// evidence the unit's status, error and metrics. A second call for the
/// same run and unit brings the open item up to date.
pub fn raise_pipeline_qc(
    store: &mut Store,
    q: &PipelineQc<'_>,
    now: &str,
) -> Result<i64, StoreError> {
    let key = pipeline_qc_key(q.run_id, q.unit);
    let reference = serde_json::json!({
        "run_id": q.run_id, "pipeline": q.pipeline, "unit": q.unit,
        "stack_id": q.stack_id, "subject_id": q.subject_id, "session_day": q.session_day,
    });
    let evidence = serde_json::json!({
        "status": q.status, "error": q.error, "metrics": q.metrics,
    });
    if let Some((id, _)) = open_item(store, PIPELINE_QC_KIND, &key)? {
        refresh_item(store, id, &evidence, 1, q.job_id)?;
        return Ok(id);
    }
    open_new(
        store,
        PIPELINE_QC_KIND,
        "run",
        &key,
        &reference,
        &evidence,
        1,
        q.job_id,
        now,
    )
}

/// Close the open `pick.border` item of one occasion, if there is one:
/// `superseded` when a later run found nothing to doubt, `accepted` when a
/// person's pick answered it. Answers how many were closed.
pub fn close_pick_border(
    store: &mut Store,
    key: &str,
    status: &str,
    actor: &str,
    decision: &serde_json::Value,
) -> Result<u64, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, decided_at = {}, actor = {}, decision = {} \
         WHERE kind = {} AND status = 'open' AND group_key = {}",
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Text),
        d.param(4, Type::Json),
        d.param(5, Type::Text),
        d.param(6, Type::Text),
    );
    store.execute(
        &sql,
        &[
            Param::from(status),
            Param::from(now_iso()),
            Param::from(actor),
            Param::from(decision.to_string()),
            Param::from(PICK_BORDER_KIND),
            Param::from(key),
        ],
    )
}

#[cfg(test)]
mod identity_items {
    use super::*;
    use crate::migrate::{self, Kind};

    fn store() -> Store {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        store
    }

    fn status_of(store: &mut Store, id: i64) -> String {
        store
            .query_opt(
                &format!("SELECT status FROM review_item WHERE id = {id}"),
                &[],
            )
            .unwrap()
            .unwrap()
            .text(0)
            .unwrap()
            .to_string()
    }

    fn unmapped<'a>(place_id: i64, shape: &'a str, files: i64, batch: Option<i64>) -> Unmapped<'a> {
        Unmapped {
            place_id,
            place: "ward-a",
            shape,
            id_type: "patient-id",
            files,
            first_seen: "t0",
            batch_id: batch,
            job_id: batch,
        }
    }

    fn provisional<'a>(
        subject_id: i64,
        code: &'a str,
        files: i64,
        batch: Option<i64>,
    ) -> Provisional<'a> {
        Provisional {
            subject_id,
            code,
            id_type: "patient-id",
            shape: "AAA-9999",
            place_id: 3,
            place: "ward-a",
            files,
            batch_id: batch,
            job_id: None,
        }
    }

    #[test]
    fn an_unmapped_shape_is_one_item_per_dataset_counted_up_and_closed_by_a_run() {
        let mut store = store();
        let first =
            raise_unmapped(&mut store, &unmapped(7, "999999-9999", 3, Some(1)), "t1").unwrap();
        let again =
            raise_unmapped(&mut store, &unmapped(7, "999999-9999", 5, Some(2)), "t2").unwrap();
        assert_eq!(first, again, "one open item per dataset and shape");
        let other = raise_unmapped(&mut store, &unmapped(7, "AAA-999", 1, Some(2)), "t2").unwrap();
        assert_ne!(first, other);
        let elsewhere =
            raise_unmapped(&mut store, &unmapped(8, "999999-9999", 1, None), "t2").unwrap();
        assert_ne!(first, elsewhere);
        let mut shapes = open_unmapped_shapes(&mut store, 7).unwrap();
        shapes.sort();
        assert_eq!(shapes, ["999999-9999", "AAA-999"]);
        let item = item(&mut store, first).unwrap().unwrap();
        assert_eq!(item.kind, UNMAPPED_KIND);
        assert_eq!(item.scope, "batch");
        assert_eq!(item.reference["place"], "ward-a");
        assert_eq!(item.reference["id_type"], "patient-id");
        assert_eq!(item.evidence["files"], 5);
        assert_eq!(item.evidence["first_seen"], "t0");
        assert_eq!(item.evidence["batch_id"], 2);
        assert_eq!(item.members, 5);
        let key = store
            .query_opt(
                &format!("SELECT group_key, job_id FROM review_item WHERE id = {first}"),
                &[],
            )
            .unwrap()
            .unwrap();
        assert_eq!(key.text(0).unwrap(), "place:7|shape:999999-9999");
        assert_eq!(key.int(1).unwrap(), 2);
        assert!(close_unmapped(&mut store, 7, "999999-9999", "t3").unwrap());
        assert_eq!(status_of(&mut store, first), RESOLVED);
        assert!(!close_unmapped(&mut store, 7, "999999-9999", "t3").unwrap());
        assert_eq!(open_unmapped_shapes(&mut store, 7).unwrap(), ["AAA-999"]);
        // held again after a close: a new item, the old one stays closed
        let later =
            raise_unmapped(&mut store, &unmapped(7, "999999-9999", 2, Some(3)), "t4").unwrap();
        assert_ne!(later, first);
    }

    #[test]
    fn a_provisional_subject_is_one_item_whose_files_count_up_and_a_merge_closes_it() {
        let mut store = store();
        let first =
            raise_provisional(&mut store, &provisional(42, "abc", 10, Some(1)), "t1").unwrap();
        let again =
            raise_provisional(&mut store, &provisional(42, "abc", 4, Some(2)), "t2").unwrap();
        assert_eq!(first, again);
        let item = item(&mut store, first).unwrap().unwrap();
        assert_eq!(item.kind, PROVISIONAL_KIND);
        assert_eq!(item.scope, "subject");
        assert_eq!(item.reference["subject_id"], 42);
        assert_eq!(item.evidence["files"], 14);
        assert_eq!(item.evidence["batch_id"], 2);
        assert_eq!(item.evidence["shape"], "AAA-9999");
        assert_eq!(item.members, 1);
        let rendered = item.evidence.to_string() + &item.reference.to_string();
        assert!(!rendered.contains("199"), "{rendered}");
        let other = raise_provisional(&mut store, &provisional(43, "def", 1, None), "t2").unwrap();
        assert_ne!(first, other);
        assert_eq!(
            close_provisional(
                &mut store,
                42,
                "anna@lab",
                &serde_json::json!({"merged_into": "sub-y"})
            )
            .unwrap(),
            1
        );
        assert_eq!(
            close_provisional(&mut store, 42, "anna@lab", &serde_json::json!({})).unwrap(),
            0
        );
        let row = store
            .query_opt(
                "SELECT status, actor, decision FROM review_item WHERE id = ?",
                &[Param::Int(first)],
            )
            .unwrap()
            .unwrap();
        assert_eq!(row.text(0).unwrap(), "superseded");
        assert_eq!(row.text(1).unwrap(), "anna@lab");
        assert!(row.text(2).unwrap().contains("sub-y"));
        assert_eq!(status_of(&mut store, other), "open");
    }
}
