// SPDX-License-Identifier: AGPL-3.0-only

//! One spelling of a value in a row.
//!
//! A value of a classification axis has several names: its identity, the
//! identities it had before a rename, its label, and what a row stores for
//! it. The pack says which of them a row stores, and the pack's resolver
//! reads any of them as the value. The registry holds no pack, so it is
//! handed the pack's answer as data ([`Spellings`]): every other name of
//! every value with what a row stores for it. With it
//!
//! - a decision is written in the stored form whoever gave it and however
//!   they spelled it ([`crate::review`], through [`crate::Registry::spellings`]);
//! - a registry is checked for rows, decisions and staged answers that hold
//!   another spelling ([`find`]), and
//! - they are moved to the stored form ([`repair`]).
//!
//! **What a repair moves.** Rows of `classification_axis`, and `value` and
//! `rules_in_view` of `decision` (in force, staged or withdrawn), then the
//! stacks' rows of the read model `stack_axis`. A text that names no value
//! is no other spelling of one, and is left as it is.
//!
//! **What it leaves and reports apart.** A row of a stack of a sample
//! sealed now, and a decision that reaches one: nothing is written on a
//! sealed stack until a certificate unseals it. A label set is a file with
//! a digest and is not in these tables: a set keeps the spelling it was
//! written with, and the sets of an axis that had another spelling are
//! counted so that their reader knows.
//!
//! **Cost.** The other names are a few hundred texts. Rows are found
//! through the key on (axis, value), one statement per axis with the texts
//! as a list, so a registry with nothing astray answers after a probe per
//! text however many rows it has, and one with rows astray pays for those
//! rows alone. Decisions are read once, grouped by their text.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::schema::Type;
use crate::store::{Error, Param, Store};

/// Every name of every axis value with what a row stores for it and the
/// value's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spellings {
    /// By axis: a text that is not the stored form, and the stored form.
    other: BTreeMap<String, BTreeMap<String, String>>,
    /// By axis: a text that is not the identity of the value it names, and
    /// the identity.
    ids: BTreeMap<String, BTreeMap<String, String>>,
}

impl Spellings {
    /// From (axis, text, stored, identity) as the pack's resolver answers
    /// them, for every name of every value.
    pub fn new(names: impl IntoIterator<Item = (String, String, String, String)>) -> Spellings {
        let mut out = Spellings::default();
        for (axis, text, stored, id) in names {
            if text != id {
                out.ids
                    .entry(axis.clone())
                    .or_default()
                    .insert(text.clone(), id);
            }
            if text != stored {
                out.other.entry(axis).or_default().insert(text, stored);
            }
        }
        out
    }

    /// The identity of the value a row's text names on an axis; the text
    /// where it is the identity already or names no value.
    pub fn id<'a>(&'a self, axis: &str, text: &'a str) -> &'a str {
        self.ids
            .get(axis)
            .and_then(|names| names.get(text))
            .map_or(text, String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.other.is_empty() && self.ids.is_empty()
    }

    /// The axes that have another spelling of a value.
    pub fn axes(&self) -> impl Iterator<Item = &str> {
        self.other.keys().map(String::as_str)
    }

    /// What a row stores for one value's text; the text where it is the
    /// stored form already or names no value.
    pub fn stored_part<'a>(&'a self, axis: &str, part: &'a str) -> &'a str {
        self.other
            .get(axis)
            .and_then(|names| names.get(part))
            .map_or(part, String::as_str)
    }

    /// A row's or a decision's whole text in the stored form: each of its
    /// comma-joined values, in the order given, a value named twice kept
    /// once.
    pub fn stored(&self, axis: &str, text: &str) -> String {
        if !self.other.contains_key(axis) {
            return text.to_string();
        }
        let mut out: Vec<&str> = Vec::new();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let stored = self.stored_part(axis, part);
            if !out.contains(&stored) {
                out.push(stored);
            }
        }
        if out.is_empty() {
            return text.to_string();
        }
        out.join(",")
    }

    /// Whether two texts name the same values of an axis, whatever they are
    /// spelled as and in whatever order.
    pub fn same(&self, axis: &str, a: &str, b: &str) -> bool {
        let set = |text: &str| {
            let mut words: Vec<String> = self
                .stored(axis, text)
                .split(',')
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect();
            words.sort();
            words.dedup();
            words
        };
        set(a) == set(b)
    }
}

/// The suffix of a kind key whose contrast was given; a kind key is
/// `<dimension> <technique>[ +C]` (the main-scan picks' kinds).
const KIND_CONTRAST: &str = " +C";

impl Spellings {
    /// A main-scan picks' kind key with its technique written as a row
    /// stores it: `2D TIRM +C` becomes `2D IRTSE +C`. A technique that
    /// names no value stays as written.
    pub fn stored_kind_key(&self, key: &str) -> String {
        let (rest, contrast) = match key.strip_suffix(KIND_CONTRAST) {
            Some(r) => (r, true),
            None => (key, false),
        };
        let (dim, technique) = match rest.split_once(' ') {
            Some((d, t))
                if d.len() >= 2
                    && d.ends_with('D')
                    && d[..d.len() - 1].chars().all(|c| c.is_ascii_digit()) =>
            {
                (Some(d), t)
            }
            _ => (None, rest),
        };
        let technique = self.stored_part("technique", technique);
        let mut out = match dim {
            Some(d) => format!("{d} {technique}"),
            None => technique.to_string(),
        };
        if contrast {
            out.push_str(KIND_CONTRAST);
        }
        out
    }

    /// A saved main-scan rules document with every kind key of its
    /// `kinds_in_order` and `not_used` in the stored form; a key two
    /// spellings make one is kept once, at its first place. None where
    /// nothing changes.
    pub fn stored_pick_rules(&self, rules: &Value) -> Option<Value> {
        let mut out = rules.clone();
        let roles = out.get_mut("roles")?.as_object_mut()?;
        let mut changed = false;
        for role in roles.values_mut() {
            for field in ["kinds_in_order", "not_used"] {
                let Some(list) = role.get_mut(field).and_then(Value::as_array_mut) else {
                    continue;
                };
                let mut now: Vec<Value> = Vec::with_capacity(list.len());
                for k in list.iter() {
                    let k = match k.as_str() {
                        Some(t) => Value::String(self.stored_kind_key(t)),
                        None => k.clone(),
                    };
                    if !now.contains(&k) {
                        now.push(k);
                    }
                }
                if now != *list {
                    *list = now;
                    changed = true;
                }
            }
        }
        changed.then_some(out)
    }
}

/// Where a registry's spellings come from: known, or asked for when the
/// first decision is written.
#[derive(Clone, Default)]
pub(crate) enum Source {
    #[default]
    Unset,
    Lazy(Arc<dyn Fn() -> Option<Spellings> + Send + Sync>),
    Known(Arc<Spellings>),
}

impl Source {
    pub(crate) fn get(&mut self) -> Arc<Spellings> {
        match self {
            Source::Known(s) => s.clone(),
            Source::Unset => Arc::new(Spellings::default()),
            Source::Lazy(ask) => {
                let known = Arc::new(ask().unwrap_or_default());
                *self = Source::Known(known.clone());
                known
            }
        }
    }
}

/// One text found that is not the stored form of the value it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub axis: String,
    pub text: String,
    pub stored: String,
    /// How many hold it and may be rewritten.
    pub open: i64,
    /// How many hold it on a stack of a sample sealed now (a row), or
    /// reach one (a decision): left as they are.
    pub sealed: i64,
}

impl Stray {
    pub fn as_json(&self) -> Value {
        json!({
            "axis": self.axis, "text": self.text, "stored": self.stored,
            "open": self.open, "sealed": self.sealed,
        })
    }
}

/// What a registry holds in another spelling than the stored one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// Rows of `classification_axis`.
    pub rows: Vec<Stray>,
    /// Decisions by their `value`: in force, staged or withdrawn.
    pub decisions: Vec<Stray>,
    /// Decisions by the rules' answer they were made in view of.
    pub in_view: Vec<Stray>,
    /// Of the decisions, the staged ones not yet committed (a model's
    /// proposal waiting for a person among them), open and sealed.
    pub staged: i64,
    /// Label sets of an axis found astray: files under a digest, which
    /// keep the spelling they were written with.
    pub label_sets: i64,
    /// Saved versions of a scope's main-scan rules that name a kind by a
    /// technique's other spelling.
    pub pick_rules: i64,
}

impl Found {
    pub fn clean(&self) -> bool {
        self.rows.is_empty()
            && self.decisions.is_empty()
            && self.in_view.is_empty()
            && self.pick_rules == 0
    }

    /// The count astray by axis: (rows, decisions), sealed ones with them.
    pub fn by_axis(&self) -> BTreeMap<String, (i64, i64)> {
        let mut out: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for s in &self.rows {
            out.entry(s.axis.clone()).or_default().0 += s.open + s.sealed;
        }
        for s in self.decisions.iter().chain(&self.in_view) {
            out.entry(s.axis.clone()).or_default().1 += s.open + s.sealed;
        }
        out
    }

    pub fn as_json(&self) -> Value {
        let list = |v: &[Stray]| v.iter().map(Stray::as_json).collect::<Vec<_>>();
        json!({
            "clean": self.clean(),
            "rows": list(&self.rows),
            "decisions": list(&self.decisions),
            "rules_in_view": list(&self.in_view),
            "staged": self.staged,
            "label_sets": self.label_sets,
            "pick_rules": self.pick_rules,
            "by_axis": self.by_axis().into_iter().map(|(axis, (rows, decisions))| {
                (axis, json!({"rows": rows, "decisions": decisions}))
            }).collect::<serde_json::Map<String, Value>>(),
        })
    }
}

/// What a repair wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Repaired {
    /// Rows of `classification_axis` given the stored form.
    pub rows: u64,
    /// Rows removed because their stack held the value in the stored form
    /// already.
    pub rows_merged: u64,
    /// Decisions whose `value` was given the stored form.
    pub decisions: u64,
    /// Decisions whose `rules_in_view` was.
    pub in_view: u64,
    /// Saved versions of main-scan rules whose kind keys were.
    pub pick_rules: u64,
    /// Stacks whose row of `stack_axis` was made again; every stack where
    /// the whole read model was.
    pub stacks: u64,
    pub rebuilt: bool,
}

impl Repaired {
    pub fn as_json(&self) -> Value {
        json!({
            "rows": self.rows, "rows_merged": self.rows_merged,
            "decisions": self.decisions, "rules_in_view": self.in_view,
            "pick_rules": self.pick_rules,
            "stacks": self.stacks, "read_model_rebuilt": self.rebuilt,
        })
    }
}

/// The condition that a row's stack is of a sample sealed now.
fn sealed_stack(store: &Store, stack: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM {} z WHERE z.stack_id = {stack} AND z.unsealed_at IS NULL)",
        store.qualified("sealed_stack")
    )
}

/// The texts of one axis as a parameter list, with the axis first.
fn text_list(store: &Store, axis: &str, texts: &[&String]) -> (String, Vec<Param>) {
    let d = store.dialect();
    let mut params = vec![Param::from(axis)];
    let mut marks = Vec::with_capacity(texts.len());
    for (i, t) in texts.iter().enumerate() {
        marks.push(d.param(i + 2, Type::Text));
        params.push(Param::from(t.as_str()));
    }
    (marks.join(", "), params)
}

/// How many texts go into one statement's list.
const LIST: usize = 400;

/// The rows of `classification_axis` that hold another spelling, by axis
/// and text. One statement an axis, through the key on (axis, value).
fn stray_rows(store: &mut Store, spellings: &Spellings) -> Result<Vec<Stray>, Error> {
    let mut out = Vec::new();
    let d = store.dialect();
    for (axis, names) in &spellings.other {
        let texts: Vec<&String> = names.keys().collect();
        for chunk in texts.chunks(LIST) {
            let (list, params) = text_list(store, axis, chunk);
            let sql = format!(
                "SELECT a.value, COUNT(*), SUM(CASE WHEN {sealed} THEN 1 ELSE 0 END) \
                 FROM {table} a WHERE a.axis = {axis_p} AND a.value IN ({list}) GROUP BY a.value \
                 ORDER BY a.value",
                sealed = sealed_stack(store, "a.stack_id"),
                table = store.qualified("classification_axis"),
                axis_p = d.param(1, Type::Text),
            );
            for r in store.query(&sql, &params)? {
                let text = r.text(0)?.to_string();
                let all = r.int(1)?;
                let sealed = r.opt_int(2)?.unwrap_or(0);
                out.push(Stray {
                    axis: axis.clone(),
                    stored: names[&text].clone(),
                    text,
                    open: all - sealed,
                    sealed,
                });
            }
        }
    }
    Ok(out)
}

/// One decision that holds another spelling in a column.
struct StrayDecision {
    id: i64,
    axis: String,
    text: String,
    stored: String,
    staged: bool,
    sealed: bool,
}

/// The decisions that hold another spelling in `column` (`value` or
/// `rules_in_view`): the texts are read grouped, the stored form of each is
/// worked out here (a multi-valued axis's text is comma-joined, which no
/// list of single names matches), and the decisions of a text astray are
/// read by it. A decision that reaches a stack of a sample sealed now is
/// marked.
fn stray_decisions(
    store: &mut Store,
    spellings: &Spellings,
    column: &str,
) -> Result<Vec<StrayDecision>, Error> {
    let d = store.dialect();
    let table = store.qualified("decision");
    let mut out: Vec<StrayDecision> = Vec::new();
    for axis in spellings.other.keys() {
        let sql = format!(
            "SELECT {column} FROM {table} WHERE axis = {} AND {column} IS NOT NULL GROUP BY {column}",
            d.param(1, Type::Text)
        );
        let mut astray: Vec<(String, String)> = Vec::new();
        for r in store.query(&sql, &[Param::from(axis.as_str())])? {
            let text = r.text(0)?;
            let stored = spellings.stored(axis, text);
            if stored != text {
                astray.push((text.to_string(), stored));
            }
        }
        for (text, stored) in astray {
            let sql = format!(
                "SELECT id, scope, ref, CASE WHEN staged_at IS NOT NULL AND committed_at IS NULL \
                 AND withdrawn_at IS NULL THEN 1 ELSE 0 END FROM {table} \
                 WHERE axis = {} AND {column} = {} ORDER BY id",
                d.param(1, Type::Text),
                d.param(2, Type::Text)
            );
            let rows = store.query(
                &sql,
                &[Param::from(axis.as_str()), Param::from(text.as_str())],
            )?;
            let mut keys: Vec<(i64, String, String, bool)> = Vec::with_capacity(rows.len());
            for r in &rows {
                keys.push((
                    r.int(0)?,
                    r.text(1)?.to_string(),
                    r.text(2)?.to_string(),
                    r.int(3)? == 1,
                ));
            }
            let named: Vec<(i64, &str, &str)> = keys
                .iter()
                .map(|(id, scope, reference, _)| (*id, scope.as_str(), reference.as_str()))
                .collect();
            let reach = crate::review::reached_keys(store, &named)?;
            let every: Vec<i64> = reach.values().flatten().copied().collect();
            let sealed = sealed_among(store, &every)?;
            for (id, _, _, staged) in &keys {
                out.push(StrayDecision {
                    id: *id,
                    axis: axis.clone(),
                    text: text.clone(),
                    stored: stored.clone(),
                    staged: *staged,
                    sealed: reach
                        .get(id)
                        .is_some_and(|stacks| stacks.iter().any(|s| sealed.contains(s))),
                });
            }
        }
    }
    Ok(out)
}

/// Of these stacks, the ones of a sample sealed now.
fn sealed_among(store: &mut Store, stacks: &[i64]) -> Result<BTreeSet<i64>, Error> {
    let mut out = BTreeSet::new();
    let ids: Vec<i64> = stacks
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT stack_id FROM {} WHERE stack_id IN ({}) AND unsealed_at IS NULL",
            store.qualified("sealed_stack"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Decisions astray as counts by axis and text.
fn counted(decisions: &[StrayDecision]) -> Vec<Stray> {
    let mut by: BTreeMap<(String, String), Stray> = BTreeMap::new();
    for s in decisions {
        let e = by
            .entry((s.axis.clone(), s.text.clone()))
            .or_insert_with(|| Stray {
                axis: s.axis.clone(),
                text: s.text.clone(),
                stored: s.stored.clone(),
                open: 0,
                sealed: 0,
            });
        if s.sealed {
            e.sealed += 1;
        } else {
            e.open += 1;
        }
    }
    by.into_values().collect()
}

/// The count of rows of `classification_axis` astray, by axis: the line a
/// status shows. The rows alone, which is the part that is one probe per
/// text; [`find`] reads the decisions as well.
pub fn count_rows(
    store: &mut Store,
    spellings: &Spellings,
) -> Result<BTreeMap<String, i64>, Error> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for s in stray_rows(store, spellings)? {
        *out.entry(s.axis).or_default() += s.open + s.sealed;
    }
    Ok(out)
}

/// Everything the registry holds in another spelling than the stored one.
/// Reads only.
pub fn find(store: &mut Store, spellings: &Spellings) -> Result<Found, Error> {
    let (found, _, _) = found(store, spellings)?;
    Ok(found)
}

fn found(
    store: &mut Store,
    spellings: &Spellings,
) -> Result<(Found, Vec<StrayDecision>, Vec<StrayDecision>), Error> {
    let rows = stray_rows(store, spellings)?;
    let decisions = stray_decisions(store, spellings, "value")?;
    let in_view = stray_decisions(store, spellings, "rules_in_view")?;
    let astray: BTreeSet<&str> = rows
        .iter()
        .map(|s| s.axis.as_str())
        .chain(decisions.iter().map(|s| s.axis.as_str()))
        .collect();
    let mut label_sets = 0;
    let d = store.dialect();
    for axis in astray {
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE what = {} OR what = {}",
            store.qualified("label_set"),
            d.param(1, Type::Text),
            d.param(2, Type::Text)
        );
        label_sets += store
            .query_opt(
                &sql,
                &[Param::from(axis), Param::from(format!("axis:{axis}"))],
            )?
            .map_or(Ok(0), |r| r.int(0))?;
    }
    let out = Found {
        rows,
        staged: decisions.iter().filter(|s| s.staged).count() as i64,
        decisions: counted(&decisions),
        in_view: counted(&in_view),
        label_sets,
        pick_rules: stray_pick_rules(store, spellings)?.len() as i64,
    };
    Ok((out, decisions, in_view))
}

/// The saved versions of main-scan rules whose kind keys are astray, with
/// the document they become: (scope kind, scope id, version, rules).
fn stray_pick_rules(
    store: &mut Store,
    spellings: &Spellings,
) -> Result<Vec<(String, i64, i64, Value)>, Error> {
    if !spellings.other.contains_key("technique") {
        return Ok(Vec::new());
    }
    let d = store.dialect();
    let t = crate::schema::table("pick_rules");
    let sql = format!(
        "SELECT scope_kind, scope_id, version, {} FROM {}",
        d.text_of(t.column("rules").expect("pick_rules.rules")),
        store.qualified("pick_rules")
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let Some(rules) = r
            .opt_text(3)?
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
        else {
            continue;
        };
        if let Some(now) = spellings.stored_pick_rules(&rules) {
            out.push((r.text(0)?.to_string(), r.int(1)?, r.int(2)?, now));
        }
    }
    Ok(out)
}

/// Above this many stacks the whole read model is made again instead of
/// the stacks' rows one list at a time.
const REBUILD_ABOVE: usize = 50_000;

/// Move every row and decision astray to the stored form, except on a
/// sealed stack, and bring `stack_axis` in line. Runs inside the caller's
/// transaction; answers what was found before and what was written.
pub fn repair(store: &mut Store, spellings: &Spellings) -> Result<(Found, Repaired), Error> {
    let (before, decisions, in_view) = found(store, spellings)?;
    let mut done = Repaired::default();
    let d = store.dialect();
    let table = store.qualified("classification_axis");
    let open_row = format!("NOT {}", sealed_stack(store, &format!("{table}.stack_id")));

    // the stacks whose rows move, read before they do
    let mut stacks: BTreeSet<i64> = BTreeSet::new();
    let mut every_stack = false;
    for s in before.rows.iter().filter(|s| s.open > 0) {
        if every_stack {
            break;
        }
        let sql = format!(
            "SELECT DISTINCT stack_id FROM {table} WHERE axis = {} AND value = {} AND {open_row}",
            d.param(1, Type::Text),
            d.param(2, Type::Text)
        );
        for r in store.query(
            &sql,
            &[Param::from(s.axis.as_str()), Param::from(s.text.as_str())],
        )? {
            stacks.insert(r.int(0)?);
        }
        every_stack = stacks.len() > REBUILD_ABOVE;
    }

    for s in before.rows.iter().filter(|s| s.open > 0) {
        let params = [
            Param::from(s.axis.as_str()),
            Param::from(s.text.as_str()),
            Param::from(s.stored.as_str()),
        ];
        // a stack that holds the value in the stored form already keeps
        // that row: (stack, axis, value) is one row
        let merge = format!(
            "DELETE FROM {table} WHERE axis = {a} AND value = {t} AND {open_row} AND EXISTS \
             (SELECT 1 FROM {table} b WHERE b.stack_id = {table}.stack_id AND b.axis = {table}.axis \
             AND b.value = {st})",
            a = d.param(1, Type::Text),
            t = d.param(2, Type::Text),
            st = d.param(3, Type::Text),
        );
        done.rows_merged += store.execute(&merge, &params)?;
        // the parameters in the order the statement names them: SQLite's
        // are not numbered
        let update = format!(
            "UPDATE {table} SET value = {st} WHERE axis = {a} AND value = {t} AND {open_row}",
            st = d.param(1, Type::Text),
            a = d.param(2, Type::Text),
            t = d.param(3, Type::Text),
        );
        done.rows += store.execute(
            &update,
            &[
                Param::from(s.stored.as_str()),
                Param::from(s.axis.as_str()),
                Param::from(s.text.as_str()),
            ],
        )?;
    }

    let decision = store.qualified("decision");
    for (column, astray, count) in [
        ("value", &decisions, &mut done.decisions),
        ("rules_in_view", &in_view, &mut done.in_view),
    ] {
        let mut by_stored: BTreeMap<(&str, &str), Vec<i64>> = BTreeMap::new();
        for s in astray.iter().filter(|s| !s.sealed) {
            by_stored
                .entry((s.axis.as_str(), s.stored.as_str()))
                .or_default()
                .push(s.id);
        }
        for ((_, stored), ids) in by_stored {
            for chunk in ids.chunks(500) {
                let sql = format!(
                    "UPDATE {decision} SET {column} = {} WHERE id IN ({})",
                    d.param(1, Type::Text),
                    id_list(chunk)
                );
                *count += store.execute(&sql, &[Param::from(stored)])?;
            }
        }
    }

    // saved main-scan rules: a kind is named by its technique's stored word
    for (kind, scope, version, rules) in stray_pick_rules(store, spellings)? {
        let sql = format!(
            "UPDATE {} SET rules = {}, digest = {} WHERE scope_kind = {} AND scope_id = {} \
             AND version = {}",
            store.qualified("pick_rules"),
            d.param(1, Type::Json),
            d.param(2, Type::Text),
            d.param(3, Type::Text),
            d.param(4, Type::Int),
            d.param(5, Type::Int),
        );
        done.pick_rules += store.execute(
            &sql,
            &[
                Param::from(rules.to_string()),
                Param::from(crate::pick_rules::digest(&rules)),
                Param::from(kind),
                Param::Int(scope),
                Param::Int(version),
            ],
        )?;
    }

    if every_stack {
        done.stacks = crate::stack_axis::rebuild(store)?;
        done.rebuilt = true;
    } else if !stacks.is_empty() {
        let ids: Vec<i64> = stacks.into_iter().collect();
        crate::stack_axis::refresh(store, &ids)?;
        done.stacks = ids.len() as u64;
    }
    Ok((before, done))
}
