// SPDX-License-Identifier: AGPL-3.0-only

//! Label sets (record 42 S7, C7): decisions leave the registry as labelled
//! data with their provenance. A set is one `labels.tsv`, a row per label
//! naming its stack, what was labelled, the value, who said so and the
//! decision, campaign and model behind it, beside a `provenance.json`; its
//! digest covers the canonical `labels.tsv`, so the same state gives the
//! same digest and one new decision changes it. The set's row pins the
//! handle it covers.
//!
//! Record 40 R3: a sample drawn and sealed for certification is never
//! training data. An operator seals the sample ([`seal`]); a set any of
//! whose items is of a sealed sample carries `sealed`, which the registry
//! works out and no caller sets, and [`usable_for_training`] refuses it.
//!
//! R5: v0's human body-part labels come in as person decisions marked as
//! imported from v0, with the date v0 gave them ([`import_v0`]). A label
//! set is not a database migration: v0's databases migrate nowhere.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::review;
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};
use crate::time::now_iso;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    Invalid(String),
    NotFound(String),
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Invalid(m) | Error::NotFound(m) | Error::Refused(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

impl From<crate::campaign::Error> for Error {
    fn from(e: crate::campaign::Error) -> Error {
        match e {
            crate::campaign::Error::Store(s) => Error::Store(s),
            crate::campaign::Error::NotFound(m) => Error::NotFound(m),
            crate::campaign::Error::Invalid(m) => Error::Invalid(m),
            crate::campaign::Error::Refused(m) | crate::campaign::Error::Forbidden(m) => {
                Error::Refused(m)
            }
        }
    }
}

/// One label.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Label {
    pub stack_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
    /// The axis, or the question a campaign asked.
    pub what: String,
    pub value: Option<String>,
    pub derivative_id: Option<i64>,
    pub author_kind: String,
    pub author: String,
    pub decision_id: Option<i64>,
    pub campaign_id: Option<i64>,
    pub model_id: Option<i64>,
    /// For a set of a campaign's answers, the answer.
    pub answer_id: Option<i64>,
}

/// The columns of `labels.tsv`, in order.
pub const COLUMNS: [&str; 12] = [
    "stack_id",
    "subject_id",
    "session_day",
    "what",
    "value",
    "derivative_id",
    "author_kind",
    "author",
    "decision_id",
    "campaign_id",
    "model_id",
    "answer_id",
];

fn cell(v: Option<String>) -> String {
    v.map(|s| s.replace(['\t', '\n', '\r'], " "))
        .unwrap_or_default()
}

/// The canonical `labels.tsv`: the header, then one line per label in the
/// order of its stack, subject, day, what, decision and answer, so the same
/// labels always give the same bytes.
pub fn tsv(labels: &[Label]) -> String {
    let mut sorted: Vec<&Label> = labels.iter().collect();
    sorted.sort_by(|a, b| {
        (
            a.stack_id,
            a.subject_id,
            &a.session_day,
            &a.what,
            a.decision_id,
            a.answer_id,
            a.derivative_id,
        )
            .cmp(&(
                b.stack_id,
                b.subject_id,
                &b.session_day,
                &b.what,
                b.decision_id,
                b.answer_id,
                b.derivative_id,
            ))
    });
    let mut out = COLUMNS.join("\t");
    out.push('\n');
    for l in sorted {
        let line = [
            cell(l.stack_id.map(|v| v.to_string())),
            cell(l.subject_id.map(|v| v.to_string())),
            cell(l.session_day.clone()),
            cell(Some(l.what.clone())),
            cell(l.value.clone()),
            cell(l.derivative_id.map(|v| v.to_string())),
            cell(Some(l.author_kind.clone())),
            cell(Some(l.author.clone())),
            cell(l.decision_id.map(|v| v.to_string())),
            cell(l.campaign_id.map(|v| v.to_string())),
            cell(l.model_id.map(|v| v.to_string())),
            cell(l.answer_id.map(|v| v.to_string())),
        ];
        out.push_str(&line.join("\t"));
        out.push('\n');
    }
    out
}

// ------------------------------------------------------ labels of decisions

/// Which decisions a set is made of.
#[derive(Debug, Clone, Default)]
pub struct DecisionQuery<'a> {
    pub axis: &'a str,
    /// The stacks a frozen selection named; every stack with a decision
    /// when none.
    pub stacks: Option<&'a [i64]>,
    /// The author kinds kept (`person`, `agent`, `model`); every kind when
    /// empty. Held against the decision in force, never a losing one.
    pub authors: &'a [String],
    /// Only decisions a campaign's close wrote.
    pub campaign: Option<i64>,
    /// Staged decisions are not in force and are left out unless asked for.
    pub staged_too: bool,
}

/// How narrow a scope is, for the resolution: a stack's own decision over a
/// group's, a group's over a series', a series' over a subject's.
fn narrowness(scope: &str) -> u8 {
    match scope {
        "stack" => 4,
        "group" => 3,
        "series" => 2,
        "subject" => 1,
        _ => 0,
    }
}

struct Standing {
    id: i64,
    scope: String,
    value: Option<String>,
    actor: String,
    author_kind: String,
    model_id: Option<i64>,
    campaign_id: Option<i64>,
}

/// The decision in force on each stack for one axis, the way the classifier
/// resolves them (C15): the highest rank, then the narrowest scope, then the
/// latest. A decision about a machine (origin scope) is a rule about the
/// machine rather than a label of a stack, and is not exported.
pub fn decision_labels(store: &mut Store, q: &DecisionQuery<'_>) -> Result<Vec<Label>, Error> {
    if q.axis.is_empty() {
        return Err(Error::Invalid("a label set names its axis".into()));
    }
    for a in q.authors {
        if !["person", "agent", "model"].contains(&a.as_str()) {
            return Err(Error::Invalid(format!(
                "{a} is not an author kind: person, agent or model"
            )));
        }
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT id, scope, ref, value, actor, author_kind, \
         CASE WHEN staged_at IS NOT NULL AND committed_at IS NULL THEN 1 ELSE 0 END, \
         model_id, campaign_id \
         FROM {} WHERE axis = {} AND withdrawn_at IS NULL ORDER BY id",
        store.qualified("decision"),
        d.param(1, Type::Text)
    );
    let rows = store.query(&sql, &[Param::from(q.axis)])?;
    let mut decisions: Vec<(Standing, String)> = Vec::new();
    for r in &rows {
        if r.int(6)? == 1 && !q.staged_too {
            continue;
        }
        let scope = r.text(1)?.to_string();
        if narrowness(&scope) == 0 {
            continue;
        }
        decisions.push((
            Standing {
                id: r.int(0)?,
                scope,
                value: r.opt_text(3)?.map(str::to_string),
                actor: r.text(4)?.to_string(),
                author_kind: r.text(5)?.to_string(),
                model_id: r.opt_int(7)?,
                campaign_id: r.opt_int(8)?,
            },
            r.text(2)?.to_string(),
        ));
    }
    // what each scope reaches
    let needs_tree = decisions
        .iter()
        .any(|(s, _)| s.scope == "series" || s.scope == "subject");
    let mut tree: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    let mut by_series: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut by_subject: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let tree_sql = format!(
        "SELECT k.id, k.series_id, r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id",
        store.qualified("stack"),
        store.qualified("series")
    );
    for r in store.query(&tree_sql, &[])? {
        let (stack, series, subject) = (r.int(0)?, r.int(1)?, r.int(2)?);
        tree.insert(stack, (series, subject));
        if needs_tree {
            by_series.entry(series).or_default().push(stack);
            by_subject.entry(subject).or_default().push(stack);
        }
    }
    // the members of every group a decision answers, read at once
    let groups: Vec<i64> = decisions
        .iter()
        .filter(|(s, _)| s.scope == "group")
        .filter_map(|(_, r)| r.parse::<i64>().ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut members: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for chunk in groups.chunks(500) {
        let sql = format!(
            "SELECT item_id, stack_id FROM {} WHERE item_id IN ({}) ORDER BY item_id, stack_id",
            store.qualified("review_member"),
            join_ids(chunk)
        );
        for r in store.query(&sql, &[])? {
            members.entry(r.int(0)?).or_default().push(r.int(1)?);
        }
    }
    let mut best: BTreeMap<i64, usize> = BTreeMap::new();
    for (i, (s, reference)) in decisions.iter().enumerate() {
        let reached: Vec<i64> = match s.scope.as_str() {
            "stack" => reference.parse::<i64>().ok().into_iter().collect(),
            "group" => reference
                .parse::<i64>()
                .ok()
                .and_then(|item| members.get(&item).cloned())
                .unwrap_or_default(),
            "series" => reference
                .parse::<i64>()
                .ok()
                .and_then(|id| by_series.get(&id).cloned())
                .unwrap_or_default(),
            "subject" => reference
                .parse::<i64>()
                .ok()
                .and_then(|id| by_subject.get(&id).cloned())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let key = |j: usize| {
            let (x, _) = &decisions[j];
            (review::rank(&x.author_kind), narrowness(&x.scope), x.id)
        };
        for stack in reached {
            match best.get(&stack) {
                Some(&j) if key(j) >= key(i) => {}
                _ => {
                    best.insert(stack, i);
                }
            }
        }
    }
    let wanted: Option<BTreeSet<i64>> = q.stacks.map(|s| s.iter().copied().collect());
    let from_campaign = campaign_of_decisions(store)?;
    let mut out = Vec::new();
    for (stack, i) in best {
        if wanted.as_ref().is_some_and(|w| !w.contains(&stack)) {
            continue;
        }
        let (s, _) = &decisions[i];
        if !q.authors.is_empty() && !q.authors.contains(&s.author_kind) {
            continue;
        }
        let campaign = s.campaign_id.or_else(|| from_campaign.get(&s.id).copied());
        if q.campaign.is_some() && campaign != q.campaign {
            continue;
        }
        out.push(Label {
            stack_id: Some(stack),
            subject_id: tree.get(&stack).map(|(_, subject)| *subject),
            session_day: None,
            what: q.axis.to_string(),
            value: s.value.clone(),
            derivative_id: None,
            author_kind: s.author_kind.clone(),
            author: s.actor.clone(),
            decision_id: Some(s.id),
            campaign_id: campaign,
            model_id: s.model_id,
            answer_id: None,
        });
    }
    Ok(out)
}

/// The campaign each decision a close wrote belongs to.
fn campaign_of_decisions(store: &mut Store) -> Result<BTreeMap<i64, i64>, StoreError> {
    let sql = format!(
        "SELECT decision_id, campaign_id FROM {} WHERE decision_id IS NOT NULL",
        store.qualified("campaign_item")
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.int(1)?)))
        .collect()
}

// ------------------------------------------------------ labels of a campaign

/// What a campaign's set holds: the outcome of every resolved item, or
/// every answer (for an agreement study, or a two-rater reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Of {
    Outcomes,
    Answers,
}

impl Of {
    pub fn name(self) -> &'static str {
        match self {
            Of::Outcomes => "outcomes",
            Of::Answers => "answers",
        }
    }
}

/// A campaign's labels. An outcome that became a decision is that
/// decision, with its author; one that closed into nothing is the
/// adjudicator's, or the raters' who agreed, and a derivative outcome is a
/// row per file.
pub fn campaign_labels(store: &mut Store, campaign: i64, of: Of) -> Result<Vec<Label>, Error> {
    use crate::campaign;
    let c = campaign::get(store, campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {campaign}")))?;
    let question = c.question()?;
    let what = question.what();
    let items: BTreeMap<i64, campaign::Item> = campaign::items(store, campaign)?
        .into_iter()
        .map(|i| (i.id, i))
        .collect();
    let answers = campaign::answers(store, campaign)?;
    // the answers of each item, grouped once
    let mut of_items: BTreeMap<i64, Vec<&campaign::Answer>> = BTreeMap::new();
    for a in &answers {
        of_items.entry(a.item_id).or_default().push(a);
    }
    // record 45: an axes question is labelled axis by axis
    if let campaign::Question::Axes { axes, constraints } = &question {
        return axes_labels(store, campaign, axes, constraints, &items, &answers, of);
    }
    let mut out = Vec::new();
    match of {
        Of::Answers => {
            for a in &answers {
                let it = &items[&a.item_id];
                out.push(Label {
                    stack_id: it.stack_id,
                    subject_id: it.subject_id,
                    session_day: it.session_day.clone(),
                    what: what.clone(),
                    value: a
                        .value
                        .clone()
                        .or_else(|| a.form.as_ref().map(|f| f.to_string())),
                    derivative_id: a.derivative_id,
                    author_kind: a.author_kind.clone(),
                    author: a.principal.clone(),
                    decision_id: None,
                    campaign_id: Some(campaign),
                    model_id: a.model_id,
                    answer_id: Some(a.id),
                });
            }
        }
        Of::Outcomes => {
            // the decisions the resolved items became, read at once
            let decided: Vec<i64> = items
                .values()
                .filter(|i| i.state == "resolved")
                .filter_map(|i| i.decision_id)
                .collect();
            let mut decisions: BTreeMap<i64, (Option<String>, String, String, Option<i64>)> =
                BTreeMap::new();
            for chunk in decided.chunks(500) {
                let sql = format!(
                    "SELECT id, value, actor, author_kind, model_id FROM {} WHERE id IN ({})",
                    store.qualified("decision"),
                    join_ids(chunk)
                );
                for r in store.query(&sql, &[])? {
                    decisions.insert(
                        r.int(0)?,
                        (
                            r.opt_text(1)?.map(str::to_string),
                            r.text(2)?.to_string(),
                            r.text(3)?.to_string(),
                            r.opt_int(4)?,
                        ),
                    );
                }
            }
            for it in items.values().filter(|i| i.state == "resolved") {
                let base = Label {
                    stack_id: it.stack_id,
                    subject_id: it.subject_id,
                    session_day: it.session_day.clone(),
                    what: what.clone(),
                    campaign_id: Some(campaign),
                    ..Label::default()
                };
                if let Some(decision) = it.decision_id {
                    if let Some((value, actor, kind, model)) = decisions.get(&decision) {
                        out.push(Label {
                            value: value.clone(),
                            author: actor.clone(),
                            author_kind: kind.clone(),
                            model_id: *model,
                            decision_id: Some(decision),
                            ..base
                        });
                    }
                    continue;
                }
                let of_item: Vec<&campaign::Answer> =
                    of_items.get(&it.id).cloned().unwrap_or_default();
                let settled: Vec<&campaign::Answer> =
                    match of_item.iter().rev().find(|a| a.role == "adjudicator") {
                        Some(a) => vec![*a],
                        None => of_item
                            .iter()
                            .copied()
                            .filter(|a| a.role == "rater")
                            .collect(),
                    };
                let authors: Vec<&str> = settled.iter().map(|a| a.principal.as_str()).collect();
                let kinds: BTreeSet<&str> =
                    settled.iter().map(|a| a.author_kind.as_str()).collect();
                let kind = if kinds.len() == 1 {
                    kinds.iter().next().copied().unwrap_or("person").to_string()
                } else {
                    kinds.into_iter().collect::<Vec<_>>().join("+")
                };
                let value = it.outcome["value"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| {
                        let f = &it.outcome["form"];
                        (!f.is_null()).then(|| f.to_string())
                    });
                let files: Vec<i64> = match it.outcome["derivatives"].as_array() {
                    Some(_) if settled.iter().any(|a| a.role == "adjudicator") => {
                        settled.iter().filter_map(|a| a.derivative_id).collect()
                    }
                    Some(list) => list.iter().filter_map(Value::as_i64).collect(),
                    None => Vec::new(),
                };
                // one model settled it: its answer names it
                let models: BTreeSet<i64> = settled.iter().filter_map(|a| a.model_id).collect();
                let base = Label {
                    value,
                    author_kind: kind,
                    author: authors.join("+"),
                    model_id: (settled.len() == 1)
                        .then(|| models.first().copied())
                        .flatten(),
                    ..base
                };
                if files.is_empty() {
                    out.push(base);
                } else {
                    for f in files {
                        out.push(Label {
                            derivative_id: Some(f),
                            ..base.clone()
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// An axes campaign's labels, one row per axis: every rater's answer on
/// each axis, or what each resolved item came to on each, the decision it
/// closed into where it closed into one.
fn axes_labels(
    store: &mut Store,
    campaign: i64,
    axes: &[String],
    constraints: &Value,
    items: &BTreeMap<i64, crate::campaign::Item>,
    answers: &[crate::campaign::Answer],
    of: Of,
) -> Result<Vec<Label>, Error> {
    use crate::campaign;
    let joined = |j: &campaign::Joint, axis: &str| -> Option<String> {
        j.get(axis).filter(|v| !v.is_empty()).map(|v| v.join(","))
    };
    let mut out = Vec::new();
    match of {
        Of::Answers => {
            for a in answers {
                let it = &items[&a.item_id];
                let Some(Ok(j)) = a
                    .value
                    .as_deref()
                    .map(|v| campaign::joint_of(axes, constraints, v))
                else {
                    continue;
                };
                for axis in axes {
                    out.push(Label {
                        stack_id: it.stack_id,
                        subject_id: it.subject_id,
                        what: axis.clone(),
                        value: joined(&j, axis),
                        author_kind: a.author_kind.clone(),
                        author: a.principal.clone(),
                        campaign_id: Some(campaign),
                        model_id: a.model_id,
                        answer_id: Some(a.id),
                        ..Label::default()
                    });
                }
            }
        }
        Of::Outcomes => {
            for it in items.values().filter(|i| i.state == "resolved") {
                let decided = it.outcome["decisions"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let ids: Vec<i64> = decided.values().filter_map(Value::as_i64).collect();
                let mut rows: BTreeMap<i64, (Option<String>, String, String, Option<i64>)> =
                    BTreeMap::new();
                if !ids.is_empty() {
                    let sql = format!(
                        "SELECT id, value, actor, author_kind, model_id FROM {} WHERE id IN ({})",
                        store.qualified("decision"),
                        join_ids(&ids)
                    );
                    for r in store.query(&sql, &[])? {
                        rows.insert(
                            r.int(0)?,
                            (
                                r.opt_text(1)?.map(str::to_string),
                                r.text(2)?.to_string(),
                                r.text(3)?.to_string(),
                                r.opt_int(4)?,
                            ),
                        );
                    }
                }
                let j = it.outcome["value"]
                    .as_str()
                    .and_then(|v| campaign::joint_of(axes, constraints, v).ok())
                    .unwrap_or_default();
                let settled: Vec<&campaign::Answer> = answers
                    .iter()
                    .filter(|a| a.item_id == it.id)
                    .filter(|a| {
                        it.outcome["answers"]
                            .as_array()
                            .is_some_and(|l| l.iter().any(|x| x.as_i64() == Some(a.id)))
                    })
                    .collect();
                for axis in axes {
                    let base = Label {
                        stack_id: it.stack_id,
                        subject_id: it.subject_id,
                        what: axis.clone(),
                        campaign_id: Some(campaign),
                        ..Label::default()
                    };
                    match decided.get(axis).and_then(Value::as_i64) {
                        Some(d) => {
                            if let Some((value, actor, kind, model)) = rows.get(&d) {
                                out.push(Label {
                                    value: value.clone(),
                                    author: actor.clone(),
                                    author_kind: kind.clone(),
                                    model_id: *model,
                                    decision_id: Some(d),
                                    ..base
                                });
                            }
                        }
                        None => out.push(Label {
                            value: joined(&j, axis),
                            author: settled
                                .iter()
                                .map(|a| a.principal.as_str())
                                .collect::<Vec<_>>()
                                .join("+"),
                            author_kind: settled
                                .iter()
                                .map(|a| a.author_kind.as_str())
                                .collect::<BTreeSet<_>>()
                                .into_iter()
                                .collect::<Vec<_>>()
                                .join("+"),
                            ..base
                        }),
                    }
                }
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------- the sets

/// A set to record, once its files are written.
#[derive(Debug, Clone)]
pub struct NewSet<'a> {
    pub name: &'a str,
    /// The name's next version ([`next_version`]); a version taken is
    /// refused, so two sets never share a name and a version.
    pub version: i64,
    /// decisions | outcomes | answers | imported
    pub kind: &'a str,
    pub what: &'a str,
    pub source: Value,
    pub campaign_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub pack_version: Option<&'a str>,
    pub scheme_digest: Option<&'a str>,
    pub sealed: bool,
    pub rows: i64,
    pub digest: &'a str,
    pub place_id: Option<i64>,
    pub path: Option<&'a str>,
    pub created_by: &'a str,
}

/// A set as read.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelSet {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub kind: String,
    pub what: String,
    pub source: Value,
    pub campaign_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub epoch: i64,
    pub pack_version: Option<String>,
    pub scheme_digest: Option<String>,
    pub sealed: bool,
    pub rows: i64,
    pub digest: String,
    pub place_id: Option<i64>,
    pub path: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

impl LabelSet {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "version": self.version, "kind": self.kind, "what": self.what,
            "source": self.source, "campaign_id": self.campaign_id, "handle_id": self.handle_id,
            "epoch": self.epoch, "pack_version": self.pack_version,
            "scheme_digest": self.scheme_digest, "sealed": self.sealed, "rows": self.rows,
            "digest": self.digest, "place_id": self.place_id, "path": self.path,
            "created_by": self.created_by, "created_at": self.created_at,
            "training": if self.sealed {
                "refused: drawn from a sealed certification sample (record 40 R3)"
            } else {
                "allowed"
            },
        })
    }
}

const SET_COLUMNS: [&str; 18] = [
    "id",
    "name",
    "kind",
    "what",
    "source",
    "campaign_id",
    "handle_id",
    "epoch",
    "pack_version",
    "scheme_digest",
    "sealed",
    "rows",
    "digest",
    "place_id",
    "path",
    "created_by",
    "created_at",
    "version",
];

fn select_sets(store: &Store) -> String {
    let d = store.dialect();
    let t = table("label_set");
    SET_COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a label_set column")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn set_of(r: &Row) -> Result<LabelSet, StoreError> {
    Ok(LabelSet {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        kind: r.text(2)?.to_string(),
        what: r.text(3)?.to_string(),
        source: r
            .opt_text(4)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null),
        campaign_id: r.opt_int(5)?,
        handle_id: r.opt_int(6)?,
        epoch: r.int(7)?,
        pack_version: r.opt_text(8)?.map(str::to_string),
        scheme_digest: r.opt_text(9)?.map(str::to_string),
        sealed: r.int(10)? != 0,
        rows: r.int(11)?,
        digest: r.text(12)?.to_string(),
        place_id: r.opt_int(13)?,
        path: r.opt_text(14)?.map(str::to_string),
        created_by: r.text(15)?.to_string(),
        created_at: r.text(16)?.to_string(),
        version: r.int(17)?,
    })
}

/// The version a set written now under a name would be: one past the
/// name's last.
pub fn next_version(store: &mut Store, name: &str) -> Result<i64, Error> {
    let sql = format!(
        "SELECT MAX(version) FROM {} WHERE name = {}",
        store.qualified("label_set"),
        store.dialect().param(1, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::from(name)])?
        .and_then(|r| r.opt_int(0).ok().flatten())
        .unwrap_or(0)
        + 1)
}

/// Record a set whose files were written, and audit it.
pub fn record(registry: &mut Registry, n: &NewSet<'_>) -> Result<LabelSet, Error> {
    let epoch = registry.meta().epoch;
    let now = now_iso();
    let store = registry.store();
    let id = store
        .insert(
            &Insert::new(
                table("label_set"),
                &[
                    "name",
                    "version",
                    "kind",
                    "what",
                    "source",
                    "campaign_id",
                    "handle_id",
                    "epoch",
                    "pack_version",
                    "scheme_digest",
                    "sealed",
                    "rows",
                    "digest",
                    "place_id",
                    "path",
                    "created_by",
                    "created_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(n.name),
                Param::Int(n.version),
                Param::from(n.kind),
                Param::from(n.what),
                Param::from(n.source.to_string()),
                n.campaign_id.map_or(Param::Null, Param::Int),
                n.handle_id.map_or(Param::Null, Param::Int),
                Param::Int(epoch),
                n.pack_version.map_or(Param::Null, Param::from),
                n.scheme_digest.map_or(Param::Null, Param::from),
                Param::Int(i64::from(n.sealed)),
                Param::Int(n.rows),
                Param::from(n.digest),
                n.place_id.map_or(Param::Null, Param::Int),
                n.path.map_or(Param::Null, Param::from),
                Param::from(n.created_by),
                Param::from(now.as_str()),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the label set was not written back".into()))?
        .int(0)?;
    audit::record(
        registry,
        &Entry {
            principal: n.created_by,
            action: if n.kind == "imported" {
                Action::LabelsImport
            } else {
                Action::LabelsExport
            },
            scope: json!({
                "label_set": id, "name": n.name, "version": n.version, "kind": n.kind, "what": n.what,
                "rows": n.rows, "campaign": n.campaign_id, "handle": n.handle_id,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({"digest": n.digest, "sealed": n.sealed, "place": n.place_id})),
        },
    )?;
    get(registry.store(), id)?.ok_or_else(|| Error::NotFound(format!("no label set {id}")))
}

pub fn get(store: &mut Store, id: i64) -> Result<Option<LabelSet>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_sets(store),
        store.qualified("label_set"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| set_of(&r))
        .transpose()?)
}

/// Every set, newest first.
pub fn list(store: &mut Store) -> Result<Vec<LabelSet>, Error> {
    let sql = format!(
        "SELECT {} FROM {} ORDER BY id DESC",
        select_sets(store),
        store.qualified("label_set")
    );
    Ok(store
        .query(&sql, &[])?
        .iter()
        .map(set_of)
        .collect::<Result<_, _>>()?)
}

/// The sets recorded with a digest, `sha256:` before it or not.
pub fn by_digest(store: &mut Store, digest: &str) -> Result<Vec<LabelSet>, Error> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    let sql = format!(
        "SELECT {} FROM {} WHERE digest = {} ORDER BY id",
        select_sets(store),
        store.qualified("label_set"),
        store.dialect().param(1, Type::Text)
    );
    Ok(store
        .query(&sql, &[Param::from(hex)])?
        .iter()
        .map(set_of)
        .collect::<Result<_, _>>()?)
}

/// The set, when a training tool may learn from it: a set drawn from a
/// sealed certification sample is refused (record 40 R3), since a model
/// fitted on the sample that certifies it certifies nothing. A set is held
/// to the seal it was written under and to the samples sealed since, read
/// from its `labels.tsv` where the file is still there.
///
/// Record 48 R2: a sample stays sealed only until the certificate it was
/// drawn for is recorded and the sample unsealed by it ([`unseal`]). A set
/// written while its items were sealed is read again against the seals in
/// force now, from its `labels.tsv`; one whose file is gone cannot be read
/// again, and stays refused.
pub fn usable_for_training(store: &mut Store, id: i64) -> Result<LabelSet, Error> {
    let set = get(store, id)?.ok_or_else(|| Error::NotFound(format!("no label set {id}")))?;
    let refuse = |why: String| {
        Err(Error::Refused(format!(
            "label set {id} {why}, and is not training data (records 40 R3 and 48 R2)"
        )))
    };
    let text = set
        .path
        .as_deref()
        .and_then(|p| std::fs::read(std::path::Path::new(p).join("labels.tsv")).ok());
    let now_sealed = match text {
        // the file is read only where it is the one the set recorded
        Some(bytes) => {
            if sha256_hex(&bytes) != set.digest.trim_start_matches("sha256:") {
                return refuse("has a labels.tsv that is not the one its digest names".into());
            }
            let Ok(text) = String::from_utf8(bytes) else {
                return refuse("has a labels.tsv that does not read".into());
            };
            let rows = keys_of_tsv(&text);
            if text.lines().next() != Some(COLUMNS.join("\t").as_str())
                || rows.len() as i64 != set.rows
                || rows
                    .iter()
                    .any(|l| l.stack_id.is_none() && l.subject_id.is_none())
            {
                return refuse("has a labels.tsv that does not read as its rows".into());
            }
            sealed_among(store, &rows)?
        }
        // gone: the seal it was written under stands, and the stacks of the
        // frozen list it pinned are held to the seals in force; with
        // neither, nothing says it is clear
        None => {
            if set.sealed {
                return refuse(
                    "was written from a sealed sample and its labels.tsv is gone, so it cannot be read again".into(),
                );
            }
            match set.handle_id {
                Some(h) => {
                    let stacks = handle_stacks(store, h)?;
                    if stacks.is_empty() {
                        return refuse(
                            "pins a list that keeps no stacks, and its labels.tsv is gone".into(),
                        );
                    }
                    !sealed_now(store, &stacks, &[])?.0.is_empty()
                }
                None => {
                    return refuse(
                        "has no labels.tsv where it was written and pins no list, so nothing says it holds no sealed item".into(),
                    );
                }
            }
        }
    };
    if now_sealed {
        return refuse(
            "holds items of a sealed certification sample until its certificate is recorded and the sample unsealed".into(),
        );
    }
    Ok(set)
}

/// The stacks a handle of stacks keeps; none for a handle of another grain.
fn handle_stacks(store: &mut Store, handle: i64) -> Result<Vec<i64>, Error> {
    let grain = store
        .query_opt(
            &format!(
                "SELECT grain FROM {} WHERE id = {}",
                store.qualified("handle"),
                store.dialect().param(1, Type::Int)
            ),
            &[Param::Int(handle)],
        )?
        .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string));
    if grain.as_deref() != Some("stack") {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT key FROM {} WHERE handle_id = {} ORDER BY position",
        store.qualified("handle_member"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(handle)])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?)
}

/// The stack and subject of each row of a `labels.tsv`.
fn keys_of_tsv(text: &str) -> Vec<Label> {
    text.lines()
        .skip(1)
        .map(|line| {
            let mut cells = line.split('\t');
            let stack_id = cells.next().and_then(|c| c.parse::<i64>().ok());
            let subject_id = cells.next().and_then(|c| c.parse::<i64>().ok());
            Label {
                stack_id,
                subject_id,
                ..Label::default()
            }
        })
        .collect()
}

// ------------------------------------------------------- sealed samples

/// What a seal did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sealed {
    pub sample: String,
    /// Record 48 R2: the sample's digest, which a certificate names.
    pub digest: String,
    /// Stacks sealed now, and stacks the sample held sealed already.
    pub stacks: i64,
    pub already: i64,
}

impl Sealed {
    pub fn as_json(&self) -> Value {
        json!({"sample": self.sample, "digest": self.digest, "stacks": self.stacks, "already": self.already})
    }
}

/// Record 40 R3: seal a sample drawn for certification, before anyone
/// looks. Every stack of it is kept by the registry as sealed, with its
/// subject, so that a label set holding any of them is sealed and never
/// training data. An operator's act: the seal is never a caller's flag on
/// a set. Sealing a sample again adds what it did not hold; nothing is
/// ever unsealed.
pub fn seal(
    registry: &mut Registry,
    sample: &str,
    handle_id: Option<i64>,
    stacks: &[i64],
    who: &str,
) -> Result<Sealed, Error> {
    let sample = sample.trim();
    if sample.is_empty() {
        return Err(Error::Invalid(
            "a sealed sample names what it was drawn as".into(),
        ));
    }
    if stacks.is_empty() {
        return Err(Error::Invalid("the sample names no stack".into()));
    }
    let now = now_iso();
    let store = registry.store();
    store.begin()?;
    let done = (|| -> Result<Sealed, Error> {
        let d = store.dialect();
        let mut subject_of: BTreeMap<i64, i64> = BTreeMap::new();
        let wanted: BTreeSet<i64> = stacks.iter().copied().collect();
        let list: Vec<i64> = wanted.iter().copied().collect();
        for chunk in list.chunks(500) {
            let sql = format!(
                "SELECT k.id, r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id WHERE k.id IN ({})",
                store.qualified("stack"),
                store.qualified("series"),
                join_ids(chunk)
            );
            for r in store.query(&sql, &[])? {
                subject_of.insert(r.int(0)?, r.int(1)?);
            }
        }
        if let Some(missing) = wanted.iter().find(|k| !subject_of.contains_key(k)) {
            return Err(Error::NotFound(format!("no stack {missing}")));
        }
        let sql = format!(
            "SELECT stack_id FROM {} WHERE sample = {}",
            store.qualified("sealed_stack"),
            d.param(1, Type::Text)
        );
        let held: BTreeSet<i64> = store
            .query(&sql, &[Param::from(sample)])?
            .iter()
            .map(|r| r.int(0))
            .collect::<Result<_, _>>()?;
        let rows: Vec<Vec<Param>> = subject_of
            .iter()
            .filter(|(k, _)| !held.contains(k))
            .map(|(k, subject)| {
                vec![
                    Param::from(sample),
                    Param::Int(*k),
                    Param::Int(*subject),
                    handle_id.map_or(Param::Null, Param::Int),
                    Param::from(who),
                    Param::from(now.as_str()),
                ]
            })
            .collect();
        for chunk in rows.chunks(500) {
            store.insert(
                &Insert::new(
                    table("sealed_stack"),
                    &[
                        "sample",
                        "stack_id",
                        "subject_id",
                        "handle_id",
                        "sealed_by",
                        "sealed_at",
                    ],
                ),
                chunk,
            )?;
        }
        Ok(Sealed {
            sample: sample.to_string(),
            digest: String::new(),
            stacks: rows.len() as i64,
            already: (subject_of.len() - rows.len()) as i64,
        })
    })();
    let mut sealed = match done {
        Ok(s) => s,
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    };
    store.commit()?;
    sealed.digest = sample_digest(registry.store(), &sealed.sample)?;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::LabelsSeal,
            scope: json!({"sample": sealed.sample, "handle": handle_id, "stacks": sealed.stacks}),
            policy: None,
            job_id: None,
            details: Some(json!({"already": sealed.already})),
        },
    )?;
    Ok(sealed)
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether any of these labels is an item of a sealed sample: its stack is
/// sealed, or, for a session's label that names no stack, a stack of its
/// subject is. A stack a certificate unsealed (record 48 R2) is no longer
/// sealed by that sample, though its row stays.
pub fn sealed_among(store: &mut Store, labels: &[Label]) -> Result<bool, Error> {
    let stacks: BTreeSet<i64> = labels.iter().filter_map(|l| l.stack_id).collect();
    let subjects: BTreeSet<i64> = labels
        .iter()
        .filter(|l| l.stack_id.is_none())
        .filter_map(|l| l.subject_id)
        .collect();
    for (column, ids) in [("stack_id", stacks), ("subject_id", subjects)] {
        let ids: Vec<i64> = ids.into_iter().collect();
        for chunk in ids.chunks(500) {
            let sql = format!(
                "SELECT 1 FROM {} WHERE {column} IN ({}) AND unsealed_at IS NULL LIMIT 1",
                store.qualified("sealed_stack"),
                join_ids(chunk)
            );
            if store.query_opt(&sql, &[])?.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The stacks and the subjects sealed now, of the ones given: what a batch
/// may not accept in one move and a training view leaves out.
pub fn sealed_now(
    store: &mut Store,
    stacks: &[i64],
    subjects: &[i64],
) -> Result<(BTreeSet<i64>, BTreeSet<i64>), Error> {
    let mut out = (BTreeSet::new(), BTreeSet::new());
    for (column, ids, into) in [
        ("stack_id", stacks, &mut out.0),
        ("subject_id", subjects, &mut out.1),
    ] {
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for chunk in ids.chunks(500) {
            let sql = format!(
                "SELECT DISTINCT {column} FROM {} WHERE {column} IN ({}) AND unsealed_at IS NULL",
                store.qualified("sealed_stack"),
                join_ids(chunk)
            );
            for r in store.query(&sql, &[])? {
                into.insert(r.int(0)?);
            }
        }
    }
    Ok(out)
}

/// Record 48 R2: the labels a training tool may learn from, of these: every
/// one whose item is not of a sample sealed now. Answers what is kept and
/// how many were left out.
pub fn drop_sealed(store: &mut Store, labels: Vec<Label>) -> Result<(Vec<Label>, usize), Error> {
    let stacks: Vec<i64> = labels.iter().filter_map(|l| l.stack_id).collect();
    let subjects: Vec<i64> = labels
        .iter()
        .filter(|l| l.stack_id.is_none())
        .filter_map(|l| l.subject_id)
        .collect();
    let (sealed_stacks, sealed_subjects) = sealed_now(store, &stacks, &subjects)?;
    let before = labels.len();
    let kept: Vec<Label> = labels
        .into_iter()
        .filter(|l| match l.stack_id {
            Some(k) => !sealed_stacks.contains(&k),
            None => !l.subject_id.is_some_and(|s| sealed_subjects.contains(&s)),
        })
        .collect();
    let dropped = before - kept.len();
    Ok((kept, dropped))
}

/// Record 48 R2: the development labels a training tool reads: the
/// decisions in force on an axis, or on every axis a decision names, by the
/// author kinds given (a person's when none is), leaving out every item of
/// a sample sealed now. Answers the labels and how many were left out.
/// Staged decisions are not in force and never train.
pub fn training_labels(
    store: &mut Store,
    axis: Option<&str>,
    stacks: Option<&[i64]>,
    authors: &[String],
    campaign: Option<i64>,
) -> Result<(Vec<Label>, usize), Error> {
    let axes: Vec<String> = match axis {
        Some(a) => vec![a.to_string()],
        None => store
            .query(
                &format!(
                    "SELECT DISTINCT axis FROM {} WHERE withdrawn_at IS NULL ORDER BY axis",
                    store.qualified("decision")
                ),
                &[],
            )?
            .iter()
            .map(|r| r.text(0).map(str::to_string))
            .collect::<Result<_, _>>()?,
    };
    let person = ["person".to_string()];
    let authors = if authors.is_empty() {
        &person[..]
    } else {
        authors
    };
    let mut rows = Vec::new();
    for a in &axes {
        rows.extend(decision_labels(
            store,
            &DecisionQuery {
                axis: a,
                stacks,
                authors,
                campaign,
                staged_too: false,
            },
        )?);
    }
    drop_sealed(store, rows)
}

// ------------------------------------------------------ the certificates

/// Record 48 R2: what a certification measured on a sealed sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Certificate {
    pub id: i64,
    pub sample: String,
    pub model_ids: Vec<i64>,
    pub result: Value,
    pub created_by: String,
    pub created_at: String,
}

impl Certificate {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "sample": self.sample, "model_ids": self.model_ids,
            "result": self.result, "created_by": self.created_by, "created_at": self.created_at,
        })
    }
}

fn certificate_of(r: &Row) -> Result<Certificate, StoreError> {
    Ok(Certificate {
        id: r.int(0)?,
        sample: r.text(1)?.to_string(),
        model_ids: r
            .opt_text(2)?
            .and_then(|t| serde_json::from_str::<Vec<i64>>(t).ok())
            .unwrap_or_default(),
        result: r
            .opt_text(3)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null),
        created_by: r.text(4)?.to_string(),
        created_at: r.text(5)?.to_string(),
    })
}

fn select_certificates(store: &Store) -> String {
    let d = store.dialect();
    let t = table("certificate");
    [
        "id",
        "sample",
        "model_ids",
        "result",
        "created_by",
        "created_at",
    ]
    .iter()
    .map(|c| d.text_of(t.column(c).expect("a certificate column")))
    .collect::<Vec<_>>()
    .join(", ")
}

pub fn certificate(store: &mut Store, id: i64) -> Result<Option<Certificate>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE id = {}",
        select_certificates(store),
        store.qualified("certificate"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| certificate_of(&r))
        .transpose()?)
}

/// Every certificate, newest first.
pub fn certificates(store: &mut Store) -> Result<Vec<Certificate>, Error> {
    let sql = format!(
        "SELECT {} FROM {} ORDER BY id DESC",
        select_certificates(store),
        store.qualified("certificate")
    );
    Ok(store
        .query(&sql, &[])?
        .iter()
        .map(certificate_of)
        .collect::<Result<_, _>>()?)
}

/// The samples sealed under a name, with how many of their stacks are
/// sealed now and how many a certificate unsealed.
pub fn sample_counts(store: &mut Store, sample: &str) -> Result<(i64, i64), Error> {
    let mut out = [0i64; 2];
    for (i, filter) in ["unsealed_at IS NULL", "unsealed_at IS NOT NULL"]
        .iter()
        .enumerate()
    {
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE sample = {} AND {filter}",
            store.qualified("sealed_stack"),
            store.dialect().param(1, Type::Text)
        );
        out[i] = store
            .query_opt(&sql, &[Param::from(sample)])?
            .map(|r| r.int(0))
            .transpose()?
            .unwrap_or(0);
    }
    Ok((out[0], out[1]))
}

/// Record 48 R2: the digest of a sealed sample, `sha256:` over its stack
/// ids in order, one a line: what a certificate's result names, so the
/// result is held to the very stacks that were sealed.
pub fn sample_digest(store: &mut Store, sample: &str) -> Result<String, Error> {
    let sql = format!(
        "SELECT stack_id FROM {} WHERE sample = {} ORDER BY stack_id",
        store.qualified("sealed_stack"),
        store.dialect().param(1, Type::Text)
    );
    let mut text = String::new();
    for r in store.query(&sql, &[Param::from(sample)])? {
        text.push_str(&r.int(0)?.to_string());
        text.push('\n');
    }
    Ok(format!("sha256:{}", sha256_hex(text.as_bytes())))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Record 48 R2: record what a certification measured on a sealed sample:
/// the models it certified and the result as the certifying tool gave it.
/// Refused for a sample nobody sealed, and for a model not registered.
pub fn record_certificate(
    registry: &mut Registry,
    sample: &str,
    model_ids: &[i64],
    result: &Value,
    who: &str,
    author_kind: &str,
) -> Result<Certificate, Error> {
    if author_kind != "person" {
        return Err(Error::Refused(format!(
            "a certificate is a person's act, and {who} acts as a {author_kind}"
        )));
    }
    let sample = sample.trim();
    if sample.is_empty() {
        return Err(Error::Invalid(
            "a certificate names the sealed sample it measured".into(),
        ));
    }
    if model_ids.is_empty() {
        return Err(Error::Invalid(
            "a certificate names the registered models it certified".into(),
        ));
    }
    if !result.is_object() {
        return Err(Error::Invalid(
            "result: what the certification measured, a JSON object".into(),
        ));
    }
    let store = registry.store();
    let (sealed, unsealed) = sample_counts(store, sample)?;
    if sealed + unsealed == 0 {
        return Err(Error::NotFound(format!(
            "no sample {sample} was sealed (nils labels seal); a certificate measures a sealed sample"
        )));
    }
    for id in model_ids {
        if crate::model::get(store, *id)?.is_none() {
            return Err(Error::NotFound(format!("no registered model {id}")));
        }
    }
    // the result names what it measured and what it found: the sample and
    // its digest, the risk it certifies, how many it read and how many of
    // those the models got wrong
    if result["sample"].as_str() != Some(sample) {
        return Err(Error::Invalid(format!(
            "result.sample names the sealed sample it measured, {sample}"
        )));
    }
    let digest = sample_digest(store, sample)?;
    if result["sample_digest"].as_str() != Some(digest.as_str()) {
        return Err(Error::Invalid(format!(
            "result.sample_digest is the digest of the sealed sample's stacks, as nils labels seal gave it; {sample}'s is not the one given"
        )));
    }
    if !result["risk"].as_f64().is_some_and(|r| r > 0.0 && r < 1.0) {
        return Err(Error::Invalid(
            "result.risk: the risk the certificate holds the models to, between 0 and 1".into(),
        ));
    }
    let n = result["n"].as_i64().filter(|n| *n > 0).ok_or_else(|| {
        Error::Invalid("result.n: how many stacks of the sample were read".into())
    })?;
    if n > sealed + unsealed {
        return Err(Error::Invalid(format!(
            "result.n is {n}, and {sample} holds {} stacks",
            sealed + unsealed
        )));
    }
    if !result["errors"]
        .as_i64()
        .is_some_and(|e| (0..=n).contains(&e))
    {
        return Err(Error::Invalid(
            "result.errors: how many of those the models got wrong, from 0 to n".into(),
        ));
    }
    let now = now_iso();
    let id = store
        .insert(
            &Insert::new(
                table("certificate"),
                &["sample", "model_ids", "result", "created_by", "created_at"],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(sample),
                Param::from(json!(model_ids).to_string()),
                Param::from(result.to_string()),
                Param::from(who),
                Param::from(now.as_str()),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the certificate was not written back".into()))?
        .int(0)?;
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::LabelsCertificate,
            scope: json!({"certificate": id, "sample": sample, "models": model_ids}),
            policy: None,
            job_id: None,
            details: None,
        },
    )?;
    certificate(registry.store(), id)?
        .ok_or_else(|| Error::NotFound(format!("no certificate {id}")))
}

/// What an unseal did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Unsealed {
    pub sample: String,
    pub certificate: i64,
    /// Stacks unsealed now, and those the certificate had unsealed before.
    pub stacks: i64,
    pub already: i64,
}

impl Unsealed {
    pub fn as_json(&self) -> Value {
        json!({
            "sample": self.sample, "certificate": self.certificate,
            "stacks": self.stacks, "already": self.already,
        })
    }
}

/// The samples whose stacks a label set holds, sealed now or unsealed
/// since, read from its `labels.tsv`.
pub fn samples_of_set(store: &mut Store, set: &LabelSet) -> Result<Vec<String>, Error> {
    let text = set
        .path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(std::path::Path::new(p).join("labels.tsv")).ok())
        .ok_or_else(|| {
            Error::Refused(format!(
                "label set {}'s labels.tsv is not where it was written, so the samples it holds cannot be read",
                set.id
            ))
        })?;
    let rows = keys_of_tsv(&text);
    let stacks: Vec<i64> = rows
        .iter()
        .filter_map(|l| l.stack_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let subjects: Vec<i64> = rows
        .iter()
        .filter(|l| l.stack_id.is_none())
        .filter_map(|l| l.subject_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for (column, ids) in [("stack_id", stacks), ("subject_id", subjects)] {
        for chunk in ids.chunks(500) {
            let sql = format!(
                "SELECT DISTINCT sample FROM {} WHERE {column} IN ({})",
                store.qualified("sealed_stack"),
                join_ids(chunk)
            );
            for r in store.query(&sql, &[])? {
                out.insert(r.text(0)?.to_string());
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// Record 48 R2: unseal a sealed sample once the certificate it was drawn
/// for is recorded. Every stack of the sample is marked unsealed by the
/// certificate, the row kept as history, so a label set holding them may
/// train the next model; the next certificate draws a fresh sample.
/// Refused without a certificate, or with one recorded for another sample.
pub fn unseal(
    registry: &mut Registry,
    sample: &str,
    certificate_id: i64,
    who: &str,
    author_kind: &str,
) -> Result<Unsealed, Error> {
    if author_kind != "person" {
        return Err(Error::Refused(format!(
            "unsealing a certified sample is a person's act, and {who} acts as a {author_kind}"
        )));
    }
    let store = registry.store();
    let cert = certificate(store, certificate_id)?.ok_or_else(|| {
        Error::Refused(format!(
            "no certificate {certificate_id}: a sealed sample is unsealed only once the certificate it was drawn for is recorded"
        ))
    })?;
    if cert.created_by == who {
        return Err(Error::Refused(format!(
            "{who} recorded certificate {certificate_id}; another person unseals its sample"
        )));
    }
    if cert.sample != sample.trim() {
        return Err(Error::Refused(format!(
            "certificate {certificate_id} measured {}, not {}; a certificate unseals only its own sample",
            cert.sample,
            sample.trim()
        )));
    }
    let (sealed, unsealed) = sample_counts(store, &cert.sample)?;
    if sealed + unsealed == 0 {
        return Err(Error::NotFound(format!(
            "no sample {} was sealed",
            cert.sample
        )));
    }
    let now = now_iso();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET unsealed_at = {}, unsealed_by = {}, certificate_id = {} \
         WHERE sample = {} AND unsealed_at IS NULL",
        store.qualified("sealed_stack"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
        d.param(4, Type::Text),
    );
    store.execute(
        &sql,
        &[
            Param::from(now.as_str()),
            Param::from(who),
            Param::Int(certificate_id),
            Param::from(cert.sample.as_str()),
        ],
    )?;
    let done = Unsealed {
        sample: cert.sample.clone(),
        certificate: certificate_id,
        stacks: sealed,
        already: unsealed,
    };
    audit::record(
        registry,
        &Entry {
            principal: who,
            action: Action::LabelsUnseal,
            scope: json!({"sample": done.sample, "certificate": certificate_id, "stacks": done.stacks}),
            policy: None,
            job_id: None,
            details: Some(json!({"already": done.already})),
        },
    )?;
    Ok(done)
}

// ---------------------------------------------------------- v0's labels

/// One label of v0's, as its export gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct V0Label {
    pub series_instance_uid: String,
    pub value: String,
    /// When v0's person labelled it: a day or an instant.
    pub date: String,
}

/// Read v0's export: tab-separated `SeriesInstanceUID`, value and date, a
/// header line allowed. Answers the labels and how many lines were not one.
pub fn parse_v0(text: &str) -> (Vec<V0Label>, i64) {
    let mut out = Vec::new();
    let mut bad = 0;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').map(str::trim).collect();
        if n == 0
            && parts
                .first()
                .is_some_and(|p| p.eq_ignore_ascii_case("SeriesInstanceUID"))
        {
            continue;
        }
        match parts.as_slice() {
            [uid, value, date, ..] if !uid.is_empty() && !value.is_empty() => out.push(V0Label {
                series_instance_uid: uid.to_string(),
                value: value.to_string(),
                date: date.to_string(),
            }),
            _ => bad += 1,
        }
    }
    (out, bad)
}

/// A day or an instant as the instant a decision carries.
fn instant_of(date: &str) -> Option<String> {
    let d = date.trim();
    let day = d.get(..10)?;
    let bytes = day.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && day
            .chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shaped {
        return None;
    }
    if d.len() == 10 {
        return Some(format!("{day}T00:00:00Z"));
    }
    crate::time::secs_of(d).map(crate::time::iso_of)
}

/// What an import did, as counts: never a UID.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Imported {
    pub labels: i64,
    pub series_matched: i64,
    pub series_unmatched: i64,
    pub stacks: i64,
    pub decisions: Vec<i64>,
    /// Stacks where a person's decision in this registry already stands,
    /// which an old label from v0 does not override.
    pub held: i64,
    /// Stacks that already carry the same imported label.
    pub already: i64,
    pub refused_values: i64,
    pub bad_dates: i64,
}

impl Imported {
    pub fn as_json(&self) -> Value {
        json!({
            "labels": self.labels, "series_matched": self.series_matched,
            "series_unmatched": self.series_unmatched, "stacks": self.stacks,
            "decisions": self.decisions.len(), "held": self.held, "already": self.already,
            "refused_values": self.refused_values, "bad_dates": self.bad_dates,
        })
    }
}

/// The review item kind an imported label is answered through, so that it
/// takes the one write path every decision takes.
pub const IMPORT_KIND: &str = "labels.import";

/// The mark an imported decision carries as its author's version, and its
/// author: v0's people are not known by name here.
pub const IMPORTED_V0: &str = "imported:v0";

/// Import v0's human labels of one axis (R5): each label maps by its
/// SeriesInstanceUID to the stacks of that series and becomes a person's
/// decision on each, marked `imported:v0`, dated as v0 dated it. A stack
/// where a person's decision already stands keeps it, whatever scope the
/// decision was written at: the stack's own, its group's, its series' or
/// its subject's. Values are taken in lower case and held to `allowed`
/// when it names any. The import is one transaction: every label is
/// written or none is. With `dry_run` nothing is written.
pub fn import_v0(
    registry: &mut Registry,
    labels: &[V0Label],
    axis: &str,
    allowed: &[String],
    who: &str,
    dry_run: bool,
) -> Result<Imported, Error> {
    if axis.is_empty() {
        return Err(Error::Invalid("an import names its axis".into()));
    }
    let out = Imported {
        labels: labels.len() as i64,
        ..Imported::default()
    };
    // What stands is read inside the import's transaction, so a decision a
    // second writer commits meanwhile is seen: SQLite's immediate
    // transaction holds the database, and on Postgres the decision table is
    // held against writers until the import ends.
    let store = registry.store();
    store.begin()?;
    let done = import_within(registry, labels, axis, allowed, who, dry_run, out);
    match done {
        Ok(out) if !dry_run && !out.decisions.is_empty() => {
            registry.store().commit()?;
            Ok(out)
        }
        Ok(out) => {
            registry.store().rollback().ok();
            Ok(out)
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            Err(e)
        }
    }
}

fn import_within(
    registry: &mut Registry,
    labels: &[V0Label],
    axis: &str,
    allowed: &[String],
    who: &str,
    dry_run: bool,
    mut out: Imported,
) -> Result<Imported, Error> {
    let store = registry.store();
    if matches!(store, Store::Postgres { .. }) {
        store.batch(&format!(
            "LOCK TABLE {} IN SHARE ROW EXCLUSIVE MODE",
            store.qualified("decision")
        ))?;
    }
    let tree = stacks_of_series(store, labels)?;
    let mut standing = Standings::read(store, axis)?;
    let now = now_iso();
    // what to write: the stacks, in order, with the value and the date
    let mut writes: Vec<(i64, String, String)> = Vec::new();
    for l in labels {
        let value = l.value.trim().to_lowercase();
        if !allowed.is_empty() && !allowed.contains(&value) {
            out.refused_values += 1;
            continue;
        }
        let Some(at) = instant_of(&l.date) else {
            out.bad_dates += 1;
            continue;
        };
        let Some(stacks) = tree.get(l.series_instance_uid.as_str()) else {
            out.series_unmatched += 1;
            continue;
        };
        out.series_matched += 1;
        for &(stack, series, subject) in stacks {
            out.stacks += 1;
            match standing.of(stack, series, subject, &value) {
                Held::Already => out.already += 1,
                Held::ByAPerson => out.held += 1,
                Held::Open => {
                    // a later label of the same series in this import meets
                    // this one, as it would have run after it
                    standing.imported(stack, &value);
                    writes.push((stack, value.clone(), at.clone()));
                }
            }
        }
    }
    if dry_run {
        return Ok(out);
    }
    for (stack, value, at) in &writes {
        out.decisions
            .push(import_one(registry, axis, *stack, value, at, who, &now)?);
    }
    Ok(out)
}

/// One imported label on one stack, inside the import's transaction: the
/// review item it answers, the decision through the one write path, dated
/// as v0 dated it, and the item closed by it.
fn import_one(
    registry: &mut Registry,
    axis: &str,
    stack: i64,
    value: &str,
    at: &str,
    who: &str,
    now: &str,
) -> Result<i64, Error> {
    let store = registry.store();
    let d = store.dialect();
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
                Param::from(IMPORT_KIND),
                Param::from("stack"),
                Param::from(json!({"stack_id": stack}).to_string()),
                Param::from(json!({"axis": axis, "source": "v0", "labelled": at}).to_string()),
                Param::from("open"),
                Param::from(now),
                Param::Int(1),
            ]],
        )?
        .first()
        .ok_or_else(|| StoreError::Message("the review item was not written back".into()))?
        .int(0)?;
    let why = format!("imported from v0 by {who}; labelled {}", &at[..10]);
    let applied = review::apply_within(
        registry,
        &review::Apply {
            item,
            member: None,
            scope: "stack",
            value: Some(value),
            author: review::Author {
                who: IMPORTED_V0,
                kind: "person",
                version: Some(IMPORTED_V0),
                model: None,
            },
            stage: false,
            why: Some(&why),
            campaign: None,
        },
    )
    .map_err(|e| Error::Refused(e.to_string()))?;
    let store = registry.store();
    // the date the label was made, not the day it was copied here
    store.update_by_id(
        table("decision"),
        &[("decided_at", Param::from(at))],
        "id",
        applied.decision,
    )?;
    // the item is its own kind, which `apply` does not close for a stack;
    // it is answered by the decision it carried
    let close = format!(
        "UPDATE {} SET status = 'accepted', decided_at = {}, actor = {}, decision_id = {} WHERE id = {} AND status = 'open'",
        store.qualified("review_item"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
        d.param(4, Type::Int)
    );
    store.execute(
        &close,
        &[
            Param::from(now),
            Param::from(IMPORTED_V0),
            Param::Int(applied.decision),
            Param::Int(item),
        ],
    )?;
    Ok(applied.decision)
}

/// Each series' stacks, as (stack, series, subject), by SeriesInstanceUID.
type SeriesStacks = BTreeMap<String, Vec<(i64, i64, i64)>>;

/// The stacks of each series the labels name, with the series and the
/// subject, in one read per five hundred series.
fn stacks_of_series(store: &mut Store, labels: &[V0Label]) -> Result<SeriesStacks, Error> {
    let uids: Vec<&str> = labels
        .iter()
        .map(|l| l.series_instance_uid.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let d = store.dialect();
    let mut out = SeriesStacks::new();
    for chunk in uids.chunks(500) {
        let marks: Vec<String> = (1..=chunk.len()).map(|i| d.param(i, Type::Text)).collect();
        let sql = format!(
            "SELECT r.series_instance_uid, k.id, k.series_id, r.subject_id FROM {} k JOIN {} r ON r.id = k.series_id \
             WHERE r.series_instance_uid IN ({}) ORDER BY k.id",
            store.qualified("stack"),
            store.qualified("series"),
            marks.join(", ")
        );
        let params: Vec<Param> = chunk.iter().map(|u| Param::from(*u)).collect();
        for r in store.query(&sql, &params)? {
            out.entry(r.text(0)?.to_string())
                .or_default()
                .push((r.int(1)?, r.int(2)?, r.int(3)?));
        }
    }
    Ok(out)
}

/// What stands on an axis before an import: the latest decision in force
/// on each stack at stack scope, and every key a person's decision in
/// force holds at a wider scope (series, subject, or a group's members).
struct Standings {
    /// stack -> (author kind, imported, value) of its latest in force
    stack: BTreeMap<i64, (String, bool, Option<String>)>,
    series: BTreeSet<i64>,
    subjects: BTreeSet<i64>,
    grouped: BTreeSet<i64>,
}

enum Held {
    Already,
    ByAPerson,
    Open,
}

impl Standings {
    fn read(store: &mut Store, axis: &str) -> Result<Standings, Error> {
        let sql = format!(
            "SELECT scope, ref, author_kind, author_version, value FROM {} WHERE axis = {} \
             AND withdrawn_at IS NULL AND (staged_at IS NULL OR committed_at IS NOT NULL) ORDER BY id",
            store.qualified("decision"),
            store.dialect().param(1, Type::Text)
        );
        let mut s = Standings {
            stack: BTreeMap::new(),
            series: BTreeSet::new(),
            subjects: BTreeSet::new(),
            grouped: BTreeSet::new(),
        };
        let mut groups: Vec<i64> = Vec::new();
        for r in store.query(&sql, &[Param::from(axis)])? {
            let scope = r.text(0)?;
            let Ok(reference) = r.text(1)?.parse::<i64>() else {
                continue;
            };
            let kind = r.opt_text(2)?.unwrap_or("person").to_string();
            let imported = r.opt_text(3)? == Some(IMPORTED_V0);
            let person = kind == "person" && !imported;
            match scope {
                "stack" => {
                    s.stack.insert(
                        reference,
                        (kind, imported, r.opt_text(4)?.map(str::to_string)),
                    );
                }
                "series" if person => {
                    s.series.insert(reference);
                }
                "subject" if person => {
                    s.subjects.insert(reference);
                }
                "group" if person => groups.push(reference),
                _ => {}
            }
        }
        for chunk in groups.chunks(500) {
            let sql = format!(
                "SELECT stack_id FROM {} WHERE item_id IN ({})",
                store.qualified("review_member"),
                join_ids(chunk)
            );
            for r in store.query(&sql, &[])? {
                s.grouped.insert(r.int(0)?);
            }
        }
        Ok(s)
    }

    fn of(&self, stack: i64, series: i64, subject: i64, value: &str) -> Held {
        if let Some((kind, imported, held)) = self.stack.get(&stack) {
            if *imported && held.as_deref() == Some(value) {
                return Held::Already;
            }
            if kind == "person" && !imported {
                return Held::ByAPerson;
            }
        }
        if self.series.contains(&series)
            || self.subjects.contains(&subject)
            || self.grouped.contains(&stack)
        {
            return Held::ByAPerson;
        }
        Held::Open
    }

    fn imported(&mut self, stack: i64, value: &str) {
        self.stack
            .insert(stack, ("person".to_string(), true, Some(value.to_string())));
    }
}

/// The labels an import wrote, as a set's rows.
pub fn imported_labels(store: &mut Store, axis: &str) -> Result<Vec<Label>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT d.id, d.ref, d.value, r.subject_id FROM {} d \
         LEFT JOIN {} k ON {} = k.id LEFT JOIN {} r ON r.id = k.series_id \
         WHERE d.axis = {} AND d.author_version = {} AND d.withdrawn_at IS NULL AND d.scope = 'stack' ORDER BY d.id",
        store.qualified("decision"),
        store.qualified("stack"),
        match d {
            crate::dialect::Dialect::Sqlite => "CAST(d.ref AS INTEGER)",
            crate::dialect::Dialect::Postgres => "CAST(d.ref AS BIGINT)",
        },
        store.qualified("series"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    store
        .query(&sql, &[Param::from(axis), Param::from(IMPORTED_V0)])?
        .iter()
        .map(|r| {
            Ok(Label {
                stack_id: r.text(1)?.parse::<i64>().ok(),
                subject_id: r.opt_int(3)?,
                session_day: None,
                what: axis.to_string(),
                value: r.opt_text(2)?.map(str::to_string),
                derivative_id: None,
                author_kind: "person".into(),
                author: IMPORTED_V0.into(),
                decision_id: Some(r.int(0)?),
                campaign_id: None,
                model_id: None,
                answer_id: None,
            })
        })
        .collect::<Result<_, StoreError>>()
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_v0_export_reads_with_or_without_its_header() {
        let (labels, bad) = parse_v0(
            "SeriesInstanceUID\tvalue\tdate\n1.2.3\tBrain\t2025-03-01\n\n1.2.4\tSpine\t2025-03-02T10:00:00Z\nbroken\n",
        );
        assert_eq!(labels.len(), 2);
        assert_eq!(bad, 1);
        assert_eq!(labels[0].value, "Brain");
        assert_eq!(instant_of("2025-03-01").unwrap(), "2025-03-01T00:00:00Z");
        assert_eq!(
            instant_of("2025-03-02T10:00:00Z").unwrap(),
            "2025-03-02T10:00:00Z"
        );
        assert_eq!(instant_of("yesterday"), None);
    }

    #[test]
    fn the_same_labels_give_the_same_bytes_in_any_order() {
        let a = Label {
            stack_id: Some(2),
            what: "body_part".into(),
            value: Some("brain".into()),
            author_kind: "person".into(),
            author: "anna@lab".into(),
            decision_id: Some(9),
            ..Label::default()
        };
        let b = Label {
            stack_id: Some(1),
            value: Some("spine\tneck".into()),
            decision_id: Some(4),
            ..a.clone()
        };
        let one = tsv(&[a.clone(), b.clone()]);
        assert_eq!(one, tsv(&[b, a]));
        assert!(one.starts_with("stack_id\tsubject_id\t"));
        let lines: Vec<&str> = one.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("1\t"), "{one}");
        assert!(
            lines[1].contains("spine neck"),
            "a tab in a value is a space"
        );
    }
}
