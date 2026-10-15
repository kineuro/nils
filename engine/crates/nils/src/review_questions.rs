// SPDX-License-Identifier: AGPL-3.0-only

//! Review round 1 (record 56 §6.3, the design of
//! `studies/2026-10-09-review-round1`, slices ER1 to ER3): the few
//! questions the sort leaves to a person, each answered as a grid of its
//! scans with one key and its exceptions.
//!
//! - **The questions** (`GET /api/review/questions`): the sort's own
//!   questions that wait for a person, open or staged, by the one meaning of
//!   need a look ([`crate::certainty`]), gathered into plain questions over a
//!   dataset's, a cohort's or the registry's scans. A question is an axis
//!   and what is in dispute on it: nothing the rules found
//!   (`<axis>:missing`), two values (`<axis>:<a>|<b>`), or one answer in
//!   doubt (`<axis>:<v>?`). Each counts distinct scans, never items or
//!   memberships, and names the two answers its keys 1 and 2 give.
//! - **A question's scans** (`GET /api/review/questions/{key}/scans`): a
//!   page of them, each with its picture, NILS's answer, the rules' reason
//!   with the rules it was decided over, System 1's answer and any staged
//!   answer; a blind-first question shows none of NILS's answers until the
//!   person answers it.
//! - **An answer** (`POST /api/review/questions/{key}/answer`): one value
//!   for the scans a person saw, with its exceptions, one decision per scan
//!   through the review spine in one transaction, and then a re-sort of
//!   those scans alone, at once (record 56 §5.2: a person's answer "applies
//!   at once and re-sorts that scan").
//!
//! A question about several axes at once (System 1's `classify.asked`, a
//! broken constraint) is answered on its own page; an item an open campaign
//! holds is answered in the campaign. Both are counted, never asked here.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nils_registry::Registry;
use nils_registry::blind;
use nils_registry::opinion;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

use crate::grants::{Access, Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::viewer::Scope;

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 3] = [
    "GET /api/review/questions",
    "GET /api/review/questions/{key}/scans",
    "POST /api/review/questions/{key}/answer",
];

/// Each door's policy row, as `serve::policy` reads them.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/review/questions",
        false,
        false,
        "bounded",
        "the questions of one scope",
        "Reading the questions",
        "Read the questions",
    ),
    (
        "GET /api/review/questions/{key}/scans",
        false,
        false,
        "bounded",
        "one page of a question's scans",
        "Reading a question's scans",
        "Read a question's scans",
    ),
    (
        "POST /api/review/questions/{key}/answer",
        true,
        false,
        "bounded",
        "one answer for the scans named",
        "Answering a question",
        "Answered a question",
    ),
];

/// A page's size when none is asked for, and the most a page holds.
const PAGE: usize = 50;
const PAGE_MOST: usize = 200;

/// How long an answer waits for a sort that holds the registry before it
/// says its scans are sorted by the next one.
const RESORT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// What each door needs, read by `serve::door` before its own table.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    use Detail::Plain;
    Some(match (method, segs) {
        ("GET", ["api", "review", "questions"])
        | ("GET", ["api", "review", "questions", _, "scans"]) => (Need::One("review:see"), Plain),
        ("POST", ["api", "review", "questions", _, "answer"]) => (Need::One("review:work"), Plain),
        _ => return None,
    })
}

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

// ------------------------------------------------------------- the keys

/// What is in dispute on a question's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    /// The rules found no value.
    Missing,
    /// Two values.
    Pair,
    /// One answer, in doubt.
    Doubt,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Shape::Missing => "missing",
            Shape::Pair => "pair",
            Shape::Doubt => "doubt",
        }
    }
}

/// A question: an axis and what is in dispute on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    axis: String,
    shape: Shape,
    /// The pair in the order of their names, the one value in doubt, or
    /// none.
    values: Vec<String>,
}

impl Key {
    fn missing(axis: &str) -> Key {
        Key {
            axis: axis.to_string(),
            shape: Shape::Missing,
            values: Vec::new(),
        }
    }

    fn doubt(axis: &str, value: &str) -> Key {
        Key {
            axis: axis.to_string(),
            shape: Shape::Doubt,
            values: vec![value.to_string()],
        }
    }

    /// Two values in dispute; one where they are the same.
    fn pair(axis: &str, a: &str, b: &str) -> Key {
        if a == b {
            return Key::doubt(axis, a);
        }
        let mut values = vec![a.to_string(), b.to_string()];
        values.sort();
        Key {
            axis: axis.to_string(),
            shape: Shape::Pair,
            values,
        }
    }

    fn text(&self) -> String {
        match self.shape {
            Shape::Missing => format!("{}:missing", self.axis),
            Shape::Pair => format!("{}:{}|{}", self.axis, self.values[0], self.values[1]),
            Shape::Doubt => format!("{}:{}?", self.axis, self.values[0]),
        }
    }

    /// A key as the doors name it, or none.
    fn parse(text: &str) -> Option<Key> {
        let (axis, rest) = text.split_once(':')?;
        if axis.is_empty()
            || !axis
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        if rest == "missing" {
            return Some(Key::missing(axis));
        }
        if let Some(v) = rest.strip_suffix('?')
            && !v.is_empty()
            && !v.contains('|')
        {
            return Some(Key::doubt(axis, v));
        }
        let (a, b) = rest.split_once('|')?;
        if a.is_empty() || b.is_empty() || b.contains('|') || a == b {
            return None;
        }
        Some(Key::pair(axis, a, b))
    }
}

/// An axis as a sentence names it.
fn axis_words(axis: &str) -> String {
    axis.replace('_', " ")
}

// --------------------------------------------------------- the questions

/// One waiting question on one scan, as the doors read them.
#[derive(Debug, Clone)]
struct Asking {
    item: i64,
    kind: String,
    /// The item's status: open, or staged for a person.
    status: String,
    group: bool,
    stack: i64,
    /// The member's own evidence where it has one, else the item's.
    evidence: Value,
    /// A group's member decided on its own.
    decided: bool,
}

impl Asking {
    /// The axis and why, for a question about one axis.
    fn axis_why(&self) -> Option<(&str, &str)> {
        self.kind.split_once(':')
    }
}

fn text_of(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The question an asking is, or none where it is answered elsewhere: a
/// question about several axes (System 1's `classify.asked`, a broken
/// constraint), or a kind this grid does not ask.
fn key_of(a: &Asking) -> Option<Key> {
    let (axis, why) = a.axis_why()?;
    let ev = &a.evidence;
    // a pair, where the rules said a value; nothing the rules found
    // otherwise, whatever else said one
    let pair = |rules: Option<String>, other: Option<String>| match (rules, other) {
        (Some(r), Some(o)) => Key::pair(axis, &r, &o),
        (Some(r), None) => Key::doubt(axis, &r),
        (None, _) => Key::missing(axis),
    };
    Some(match why {
        "missing" => Key::missing(axis),
        // the gate's disagreement: the rules' answer and System 1's
        "disagree" => pair(
            text_of(&ev["rules"]["value"]).or_else(|| text_of(&ev["value"])),
            text_of(&ev["system1"]["value"]),
        ),
        // a person's decision the rules differ from
        "decision" => pair(text_of(&ev["rule"]), text_of(&ev["decision"])),
        // an older engine's conflict between two rules
        "conflict" => pair(text_of(&ev["value"]), text_of(&ev["other"])),
        // the gate's doubt, and a person's own threshold
        "unsure" | "low_confidence" => {
            match text_of(&ev["rules"]["value"]).or_else(|| text_of(&ev["value"])) {
                Some(v) => Key::doubt(axis, &v),
                None => Key::missing(axis),
            }
        }
        _ => return None,
    })
}

/// The items an open campaign holds, each with the campaign: answered
/// there, never here (record 45). Read once.
fn held_items(store: &mut Store) -> Result<HashMap<i64, (i64, String)>, StoreError> {
    let mut out = HashMap::new();
    let d = store.dialect();
    let (item, campaign, review) = (
        store.qualified("campaign_item"),
        store.qualified("campaign"),
        store.qualified("review_item"),
    );
    for r in store.query(
        &format!(
            "SELECT i.review_item_id, c.id, c.name FROM {item} i JOIN {campaign} c ON c.id = i.campaign_id \
             WHERE c.status IN ('open', 'closing') AND i.review_item_id IS NOT NULL ORDER BY c.id"
        ),
        &[],
    )? {
        out.entry(r.int(0)?)
            .or_insert((r.int(1)?, r.text(2)?.to_string()));
    }
    // a campaign's axes item answers the review items it asks
    let t = table("review_item");
    for r in store.query(
        &format!(
            "SELECT c.id, c.name, {} FROM {item} i JOIN {campaign} c ON c.id = i.campaign_id \
             JOIN {review} r ON r.id = i.review_item_id \
             WHERE c.status IN ('open', 'closing') AND r.kind = 'campaign.axes' ORDER BY c.id",
            d.text_of_qualified(Some("r"), t.column("evidence").expect("evidence")),
        ),
        &[],
    )? {
        let ev: Value = r
            .opt_text(2)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        for id in ev["asks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
        {
            out.entry(id).or_insert((r.int(0)?, r.text(1)?.to_string()));
        }
    }
    Ok(out)
}

/// What is decided on a scan's axis at the stack's scope: the decision in
/// force (its id, value and author's kind), and the newest one staged.
#[derive(Debug, Clone, Default)]
struct Decided {
    in_force: Option<(i64, Option<String>, String)>,
    staged: Option<Value>,
}

/// The decisions on these scans' axes, at the stack's scope, and those
/// staged on a whole group item, by (stack, axis).
fn decided_on(
    store: &mut Store,
    pairs: &BTreeSet<(i64, String)>,
    groups: &BTreeMap<i64, Vec<(i64, String)>>,
) -> Result<HashMap<(i64, String), Decided>, StoreError> {
    let mut out: HashMap<(i64, String), Decided> = HashMap::new();
    let stacks: BTreeSet<i64> = pairs.iter().map(|(s, _)| *s).collect();
    let axes: BTreeSet<&str> = pairs.iter().map(|(_, a)| a.as_str()).collect();
    if stacks.is_empty() {
        return Ok(out);
    }
    let quoted = |words: &mut dyn Iterator<Item = String>| {
        words
            .map(|w| format!("'{}'", w.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let axis_list = quoted(&mut axes.iter().map(|a| a.to_string()));
    let stack_list: Vec<String> = stacks.iter().map(i64::to_string).collect();
    let read = |store: &mut Store, scope: &str, refs: &[String]| -> Result<Vec<_>, StoreError> {
        let mut rows = Vec::new();
        for chunk in refs.chunks(500) {
            let sql = format!(
                "SELECT id, ref, axis, value, author_kind, actor, model_id, \
                 CASE WHEN staged_at IS NULL THEN 0 ELSE 1 END, CASE WHEN committed_at IS NULL THEN 0 ELSE 1 END \
                 FROM {} WHERE withdrawn_at IS NULL AND scope = '{scope}' AND ref IN ({}) AND axis IN ({axis_list}) ORDER BY id",
                store.qualified("decision"),
                quoted(&mut chunk.iter().cloned()),
            );
            for r in store.query(&sql, &[])? {
                rows.push((
                    r.int(0)?,
                    r.text(1)?.to_string(),
                    r.text(2)?.to_string(),
                    r.opt_text(3)?.map(str::to_string),
                    r.opt_text(4)?.unwrap_or("person").to_string(),
                    r.opt_text(5)?.unwrap_or("").to_string(),
                    r.opt_int(6)?,
                    r.int(7)? != 0,
                    r.int(8)? != 0,
                ));
            }
        }
        Ok(rows)
    };
    for (id, reference, axis, value, kind, actor, model, staged, committed) in
        read(store, "stack", &stack_list)?
    {
        let Ok(stack) = reference.parse::<i64>() else {
            continue;
        };
        let at = out.entry((stack, axis)).or_default();
        if !staged || committed {
            // the higher rank holds (C15), among equals the later one
            let rank = nils_registry::review::rank(&kind);
            let keep = at
                .in_force
                .as_ref()
                .is_none_or(|(_, _, k)| nils_registry::review::rank(k) <= rank);
            if keep {
                at.in_force = Some((id, value, kind));
            }
        } else {
            at.staged = Some(json!({
                "value": value, "decision": id, "author_kind": kind, "actor": actor, "model_id": model,
            }));
        }
    }
    // a model's or an agent's answer staged on a whole group reaches its
    // members that wait
    let group_refs: Vec<String> = groups.keys().map(i64::to_string).collect();
    if !group_refs.is_empty() {
        for (id, reference, axis, value, kind, actor, model, staged, committed) in
            read(store, "group", &group_refs)?
        {
            if !staged || committed {
                continue;
            }
            let Ok(item) = reference.parse::<i64>() else {
                continue;
            };
            for (stack, a) in groups.get(&item).into_iter().flatten() {
                if *a != axis {
                    continue;
                }
                let at = out.entry((*stack, axis.clone())).or_default();
                if at.staged.is_none() {
                    at.staged = Some(json!({
                        "value": value, "decision": id, "author_kind": kind, "actor": actor, "model_id": model,
                    }));
                }
            }
        }
    }
    Ok(out)
}

/// The sort's waiting questions about the scans of a scope, one entry a
/// question and scan: grouped questions through each member in the scope,
/// questions about one stack through their reference.
fn gather(store: &mut Store, access: &Access, scope: &Scope) -> Result<Vec<Asking>, Reply> {
    let asks = crate::certainty::Asks::sort(store).map_err(failed)?;
    let Some(cond) = asks.condition().map(str::to_string) else {
        return Ok(Vec::new());
    };
    let Some(holds) = scope.holds(store, access, "x", "xse") else {
        return Ok(Vec::new());
    };
    let d = store.dialect();
    let ri = table("review_item");
    let rm = table("review_member");
    let json_of = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or(Value::Null)
    };
    let (member, item, stack, series) = (
        store.qualified("review_member"),
        store.qualified("review_item"),
        store.qualified("stack"),
        store.qualified("series"),
    );
    let mut out = Vec::new();
    let sql = format!(
        "SELECT ri.id, ri.kind, ri.status, {}, rm.stack_id, {}, CASE WHEN rm.decided_at IS NULL THEN 0 ELSE 1 END \
         FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id JOIN {stack} x ON x.id = rm.stack_id \
         JOIN {series} xse ON xse.id = x.series_id WHERE ri.scope = 'group' AND {cond} AND {holds} \
         ORDER BY ri.id, rm.stack_id",
        d.text_of_qualified(Some("ri"), ri.column("evidence").expect("evidence")),
        d.text_of_qualified(Some("rm"), rm.column("evidence").expect("evidence")),
    );
    for r in store.query(&sql, &[]).map_err(failed)? {
        let own = json_of(r.opt_text(5).map_err(failed)?);
        out.push(Asking {
            item: r.int(0).map_err(failed)?,
            kind: r.text(1).map_err(failed)?.to_string(),
            status: r.text(2).map_err(failed)?.to_string(),
            group: true,
            stack: r.int(4).map_err(failed)?,
            evidence: if own.is_object() {
                own
            } else {
                json_of(r.opt_text(3).map_err(failed)?)
            },
            decided: r.int(6).map_err(failed)? != 0,
        });
    }
    // the questions about one stack, held to the scope's stacks
    let sql = format!(
        "SELECT ri.id, ri.kind, ri.status, {}, {} FROM {item} ri WHERE ri.scope = 'stack' AND {cond} ORDER BY ri.id",
        d.text_of_qualified(Some("ri"), ri.column("evidence").expect("evidence")),
        d.text_of_qualified(Some("ri"), ri.column("ref").expect("ref")),
    );
    let mut single = Vec::new();
    for r in store.query(&sql, &[]).map_err(failed)? {
        let reference = json_of(r.opt_text(4).map_err(failed)?);
        let Some(s) = reference["stack_id"].as_i64() else {
            continue;
        };
        single.push(Asking {
            item: r.int(0).map_err(failed)?,
            kind: r.text(1).map_err(failed)?.to_string(),
            status: r.text(2).map_err(failed)?.to_string(),
            group: false,
            stack: s,
            evidence: json_of(r.opt_text(3).map_err(failed)?),
            decided: false,
        });
    }
    if !single.is_empty() {
        let named: Vec<i64> = single
            .iter()
            .map(|a| a.stack)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut inside: HashSet<i64> = HashSet::new();
        for chunk in named.chunks(500) {
            let sql = format!(
                "SELECT x.id FROM {stack} x JOIN {series} xse ON xse.id = x.series_id WHERE x.id IN ({}) AND {holds}",
                chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for r in store.query(&sql, &[]).map_err(failed)? {
                inside.insert(r.int(0).map_err(failed)?);
            }
        }
        out.extend(single.into_iter().filter(|a| inside.contains(&a.stack)));
    }
    Ok(out)
}

/// One question's scan, as gathered.
#[derive(Debug, Clone, Default)]
struct Scan {
    /// The waiting items that ask it.
    items: BTreeSet<i64>,
    /// Their kinds.
    kinds: BTreeSet<String>,
    /// An answer a model or an agent staged on its axis.
    staged: Option<Value>,
    /// It waits for a person to confirm a staged answer.
    status_staged: bool,
    /// The items, whether each is a group's, and its kind, in the order an
    /// answer reaches them: a question about the stack alone first.
    reach: Vec<(i64, bool, String)>,
    /// The rules' own value a question about a person's decision names
    /// ("" where they gave none): what the tile showed of the rules where a
    /// decision stands in the stored classification.
    rules_named: Option<String>,
}

/// The questions of a scope, gathered: each key with its scans.
#[derive(Debug, Default)]
struct Gathered {
    questions: BTreeMap<Key, BTreeMap<i64, Scan>>,
    /// The scans of questions about several axes, by kind.
    elsewhere: BTreeMap<String, BTreeSet<i64>>,
    /// Those questions' items, by scan.
    elsewhere_items: BTreeMap<i64, BTreeSet<i64>>,
    /// The scans an open campaign holds a question of, and the campaigns.
    held: BTreeSet<i64>,
    campaigns: BTreeMap<i64, String>,
}

impl Gathered {
    /// The distinct scans a question of any kind waits on.
    fn scans(&self) -> BTreeSet<i64> {
        let mut out: BTreeSet<i64> = self.held.clone();
        for scans in self.questions.values() {
            out.extend(scans.keys());
        }
        for scans in self.elsewhere.values() {
            out.extend(scans);
        }
        out
    }
}

/// Gather the scope's questions: each scan under the question its waiting
/// item asks, a group's member decided on its own in force left out, an
/// item a campaign holds counted as held.
fn questions_of(
    registry: &mut Registry,
    access: &Access,
    scope: &Scope,
) -> Result<Gathered, Reply> {
    let store = registry.store();
    let askings = gather(store, access, scope)?;
    let held = held_items(store).map_err(failed)?;
    // what is decided on each (stack, axis), to tell a member decided in
    // force from one staged, and to name a staged answer
    let mut pairs: BTreeSet<(i64, String)> = BTreeSet::new();
    let mut groups: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
    for a in &askings {
        if let Some((axis, _)) = a.axis_why() {
            pairs.insert((a.stack, axis.to_string()));
            if a.group {
                groups
                    .entry(a.item)
                    .or_default()
                    .push((a.stack, axis.to_string()));
            }
        }
    }
    let decided = decided_on(store, &pairs, &groups).map_err(failed)?;
    let mut out = Gathered::default();
    for a in askings {
        if let Some((cid, name)) = held.get(&a.item) {
            out.held.insert(a.stack);
            out.campaigns.insert(*cid, name.clone());
            continue;
        }
        let Some(key) = key_of(&a) else {
            out.elsewhere
                .entry(a.kind.clone())
                .or_default()
                .insert(a.stack);
            out.elsewhere_items
                .entry(a.stack)
                .or_default()
                .insert(a.item);
            continue;
        };
        let on = decided
            .get(&(a.stack, key.axis.clone()))
            .cloned()
            .unwrap_or_default();
        // a member a person decided on its own no longer waits; one a model
        // or an agent decided waits, staged, for a person to confirm it
        if a.decided && on.in_force.is_some() {
            continue;
        }
        let scan = out
            .questions
            .entry(key)
            .or_default()
            .entry(a.stack)
            .or_default();
        scan.items.insert(a.item);
        scan.kinds.insert(a.kind.clone());
        if a.status == "staged" || on.staged.is_some() {
            scan.status_staged = true;
        }
        if scan.staged.is_none() {
            scan.staged = on.staged.clone();
        }
        if a.axis_why().is_some_and(|(_, why)| why == "decision") && scan.rules_named.is_none() {
            scan.rules_named = Some(text_of(&a.evidence["rule"]).unwrap_or_default());
        }
        // a question about the stack alone first: its decision closes the
        // stack's other questions on the axis with it
        let entry = (a.item, a.group, a.kind.clone());
        if a.group {
            scan.reach.push(entry);
        } else {
            scan.reach.insert(0, entry);
        }
    }
    Ok(out)
}

/// The words of a question and why it is asked, from the kinds behind it.
fn words_of(
    key: &Key,
    kinds: &BTreeMap<String, i64>,
    values: &[String],
    s1: bool,
) -> (String, String) {
    let axis = axis_words(&key.axis);
    let why_of = |w: &str| {
        kinds
            .keys()
            .any(|k| k.split_once(':').is_some_and(|(_, x)| x == w))
    };
    match key.shape {
        Shape::Missing => (
            format!("Which {axis}?"),
            format!("The rules found no {axis} for these scans."),
        ),
        Shape::Pair => {
            let (a, b) = match values {
                [a, b, ..] => (a.clone(), b.clone()),
                _ => (key.values[0].clone(), key.values[1].clone()),
            };
            let mut why = Vec::new();
            if why_of("disagree") {
                why.push("The rules and System 1 disagree.");
            }
            if why_of("decision") {
                why.push("A person's answer differs from the rules.");
            }
            if why_of("conflict") {
                why.push("Two rules disagree.");
            }
            (format!("{a} or {b}?"), why.join(" "))
        }
        Shape::Doubt => {
            let mut why = Vec::new();
            if why_of("unsure") {
                why.push(if s1 {
                    "System 1 agrees, below its threshold."
                } else {
                    "System 1 has no answer on these scans yet."
                });
            }
            if why_of("low_confidence") {
                why.push("The rules are not sure.");
            }
            (format!("{}?", key.values[0]), why.join(" "))
        }
    }
}

/// An axis's values as a person chooses from them: `value` the form the
/// axis stores, its name and its label.
fn vocabulary(pack: Option<&nils_pack::Pack>, axis: &str) -> Vec<Value> {
    let Some(a) = pack.and_then(|p| p.axes.iter().find(|a| a.name == axis)) else {
        return Vec::new();
    };
    (0..a.values.len())
        .map(|i| {
            json!({
                "value": a.stored(i), "name": a.values[i].id, "label": a.values[i].label,
            })
        })
        .collect()
}

/// The form an axis stores a value in, from its stored form, its name or
/// its label; none for a value the axis does not have. A multi-valued
/// axis takes several, joined by commas.
fn stored_value(pack: &nils_pack::Pack, axis: &str, given: &str) -> Option<String> {
    let a = pack.axes.iter().find(|a| a.name == axis)?;
    let one = |w: &str| -> Option<String> {
        let w = w.trim();
        (0..a.values.len())
            .find(|&i| a.stored(i) == w || a.values[i].id == w || a.values[i].label == w)
            .map(|i| a.stored(i).to_string())
    };
    if a.multi {
        let parts: Option<Vec<String>> = given.split(',').map(one).collect();
        let parts = parts?;
        (!parts.is_empty()).then(|| parts.join(","))
    } else {
        one(given)
    }
}

/// NILS's answer on an axis of each of `stacks`, as stored, null where none.
fn answers_on(
    store: &mut Store,
    stacks: &[i64],
    axis: &str,
) -> Result<HashMap<i64, Option<String>>, StoreError> {
    let all = nils_registry::round::rules_answers(store, stacks)?;
    Ok(stacks
        .iter()
        .map(|s| {
            (
                *s,
                all.get(s)
                    .and_then(|a| a.get(axis))
                    .and_then(|(v, _)| v.clone()),
            )
        })
        .collect())
}

/// System 1's chosen model and its newest answers on an axis of `stacks`.
fn system1_on(
    store: &mut Store,
    stacks: &[i64],
    axis: &str,
) -> Result<
    (
        Option<nils_registry::model::Model>,
        HashMap<i64, opinion::Opinion>,
    ),
    StoreError,
> {
    let Some(chosen) = opinion::chosen(store)? else {
        return Ok((None, HashMap::new()));
    };
    let said = opinion::newest(store, stacks, Some(chosen.model.id))?;
    let out = said
        .into_iter()
        .filter_map(|(s, mut by)| by.remove(axis).map(|o| (s, o)))
        .collect();
    Ok((Some(chosen.model), out))
}

/// The two answers a question's keys 1 and 2 give: the pair, NILS's more
/// frequent answer first; else the answers staged and System 1's, the most
/// frequent first; else the values the axis holds most often in the scope,
/// then anywhere; else its first values. A question read blind first
/// (`neutral`) takes the axis's first two values in its vocabulary's order,
/// never one staged, System 1's or the rules', which would give NILS's
/// answer away before the person gave theirs (spec 11.4).
#[allow(clippy::too_many_arguments)]
fn keys_of(
    store: &mut Store,
    access: &Access,
    scope: &Scope,
    key: &Key,
    scans: &BTreeMap<i64, Scan>,
    nils: &HashMap<i64, Option<String>>,
    s1: &HashMap<i64, opinion::Opinion>,
    vocabulary: &[Value],
    neutral: bool,
) -> Result<Vec<String>, StoreError> {
    if neutral {
        return Ok(vocabulary
            .iter()
            .filter_map(|v| text_of(&v["value"]))
            .take(2)
            .collect());
    }
    if key.shape == Shape::Pair {
        let count = |v: &str| {
            scans
                .keys()
                .filter(|s| nils.get(s).cloned().flatten().as_deref() == Some(v))
                .count()
        };
        let mut pair = key.values.clone();
        pair.sort_by(|a, b| count(b).cmp(&count(a)).then_with(|| a.cmp(b)));
        return Ok(pair);
    }
    let mut out: Vec<String> = key.values.clone();
    let mut tally: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (s, scan) in scans {
        if let Some(v) = scan.staged.as_ref().and_then(|x| text_of(&x["value"])) {
            tally.entry(v).or_default().0 += 1;
        }
        if let Some(v) = s1
            .get(s)
            .and_then(|o| o.value.clone())
            .filter(|v| !v.is_empty())
        {
            tally.entry(v).or_default().1 += 1;
        }
    }
    let mut ranked: Vec<(String, (usize, usize))> = tally.into_iter().collect();
    ranked.sort_by(|a, b| {
        (b.1.0, b.1.1)
            .cmp(&(a.1.0, a.1.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    for (v, _) in ranked {
        if out.len() >= 2 {
            break;
        }
        if !out.contains(&v) {
            out.push(v);
        }
    }
    if out.len() < 2
        && let Some(holds) = scope.holds(store, access, "x", "xse")
    {
        let sql = format!(
            "SELECT ca.value, COUNT(DISTINCT ca.stack_id) FROM {} ca JOIN {} x ON x.id = ca.stack_id \
             JOIN {} xse ON xse.id = x.series_id WHERE ca.axis = {} AND ca.value IS NOT NULL AND ca.value <> '' \
             AND {holds} GROUP BY ca.value ORDER BY 2 DESC, 1 LIMIT 4",
            store.qualified("classification_axis"),
            store.qualified("stack"),
            store.qualified("series"),
            store.dialect().param(1, Type::Text),
        );
        for r in store.query(&sql, &[Param::from(key.axis.as_str())])? {
            let v = r.text(0)?.to_string();
            if out.len() < 2 && !out.contains(&v) {
                out.push(v);
            }
        }
    }
    // then the values the axis holds most often anywhere
    if out.len() < 2 {
        let sql = format!(
            "SELECT value, COUNT(*) FROM {} WHERE axis = {} AND value IS NOT NULL AND value <> '' \
             GROUP BY value ORDER BY 2 DESC, 1 LIMIT 4",
            store.qualified("classification_axis"),
            store.dialect().param(1, Type::Text),
        );
        for r in store.query(&sql, &[Param::from(key.axis.as_str())])? {
            let v = r.text(0)?.to_string();
            if out.len() < 2 && !out.contains(&v) {
                out.push(v);
            }
        }
    }
    for v in vocabulary.iter().filter_map(|v| text_of(&v["value"])) {
        if out.len() >= 2 {
            break;
        }
        if !out.contains(&v) {
            out.push(v);
        }
    }
    Ok(out)
}

/// The scope a door names: `dataset` or `cohort`, or the registry.
fn scope_of(
    registry: &mut Registry,
    dataset: Option<&str>,
    cohort: Option<&str>,
) -> Result<Scope, Reply> {
    match (
        dataset.filter(|d| !d.is_empty()),
        cohort.filter(|c| !c.is_empty()),
    ) {
        (Some(_), Some(_)) => Err(Reply::error(
            400,
            "a question is read for a dataset or a cohort, not both",
        )),
        (Some(d), None) => Scope::dataset(registry, d),
        (None, Some(c)) => Scope::cohort(registry, c),
        (None, None) => Scope::registry(registry),
    }
}

/// What the scope says of itself on a reply.
fn scope_json(scope: &Scope) -> Value {
    scope.as_json()
}

/// One question's row on the list.
#[allow(clippy::too_many_arguments)]
fn row_of(
    store: &mut Store,
    access: &Access,
    scope: &Scope,
    key: &Key,
    scans: &BTreeMap<i64, Scan>,
    pack: Option<&nils_pack::Pack>,
    modes: &HashMap<i64, &'static str>,
) -> Result<Value, StoreError> {
    let stacks: Vec<i64> = scans.keys().copied().collect();
    let nils = answers_on(store, &stacks, &key.axis)?;
    let (model, s1) = system1_on(store, &stacks, &key.axis)?;
    let vocab = vocabulary(pack, &key.axis);
    let neutral = read_blind(key, scans.values(), modes);
    let values = keys_of(
        store, access, scope, key, scans, &nils, &s1, &vocab, neutral,
    )?;
    let mut kinds: BTreeMap<String, i64> = BTreeMap::new();
    for scan in scans.values() {
        for k in &scan.kinds {
            *kinds.entry(k.clone()).or_insert(0) += 1;
        }
    }
    let (words, reason) = words_of(key, &kinds, &values, model.is_some() && !s1.is_empty());
    Ok(json!({
        "key": key.text(),
        "axis": key.axis,
        "shape": key.shape.name(),
        "values": values,
        "words": words,
        "reason": reason,
        "scans": scans.len(),
        "staged": scans.values().filter(|s| s.status_staged).count(),
        "kinds": kinds,
    }))
}

// ------------------------------------------------------- blind first

/// Each question's mode on these scans, drawn where none is kept yet
/// (spec 11.4: one in five by default).
fn modes_of<'a>(
    store: &mut Store,
    scans: impl Iterator<Item = &'a Scan>,
    salt: &str,
    share: f64,
) -> Result<HashMap<i64, &'static str>, StoreError> {
    let mut out = HashMap::new();
    for scan in scans {
        for (item, _, kind) in &scan.reach {
            if !out.contains_key(item) {
                out.insert(*item, blind::mode_of(store, *item, kind, salt, share)?);
            }
        }
    }
    Ok(out)
}

/// Whether a question can be read blind at all: only one whose words name
/// no answer. A pair (`T2w or FLAIR?`) or a value in doubt (`T2w?`) names
/// the rules' answer in the question itself, so none of its scans is read
/// blind, whatever their mode.
fn may_be_blind(key: &Key) -> bool {
    key.shape == Shape::Missing
}

/// Whether a question is read blind first: its words name no answer and a
/// question on one of its scans was drawn blind first. Its keys are then
/// neutral.
fn read_blind<'a>(
    key: &Key,
    mut scans: impl Iterator<Item = &'a Scan>,
    modes: &HashMap<i64, &'static str>,
) -> bool {
    may_be_blind(key)
        && scans.any(|s| {
            s.items
                .iter()
                .any(|i| modes.get(i) == Some(&blind::BLIND_FIRST))
        })
}

/// What one scan's tile shows a person, decided once from every question
/// waiting on it: its mode (blind first where any of them is), and whether
/// NILS's answers are hidden. They are hidden where the question may be
/// read blind, a question on the scan drawn blind first has no provisional
/// answer from the person yet, and none of the scan's questions has shown
/// them its suggestions (in the queue, or beside the scan in the dataset
/// viewer, whose names say NILS's answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tile {
    mode: &'static str,
    hidden: bool,
}

fn tile_of(
    store: &mut Store,
    key: &Key,
    scan: &Scan,
    modes: &HashMap<i64, &'static str>,
    principal: &str,
) -> Result<Tile, StoreError> {
    let first: Vec<i64> = scan
        .items
        .iter()
        .copied()
        .filter(|i| modes.get(i) == Some(&blind::BLIND_FIRST))
        .collect();
    let mode = if first.is_empty() {
        blind::ROUTINE
    } else {
        blind::BLIND_FIRST
    };
    if !may_be_blind(key) || first.is_empty() {
        return Ok(Tile {
            mode,
            hidden: false,
        });
    }
    for item in &scan.items {
        if blind::was_revealed(store, *item, principal)? {
            return Ok(Tile {
                mode,
                hidden: false,
            });
        }
    }
    let mut hidden = false;
    for item in &first {
        if blind::provisional(store, *item, principal)?.is_none() {
            hidden = true;
            break;
        }
    }
    Ok(Tile { mode, hidden })
}

/// The rules' answer on a stack's axis as a tile shows it now: the stored
/// classification's value, as the last sort wrote it with the pack, the
/// overlay and the adopted scoped changes it applied ("" where it gave
/// none), the value a sort compares a person's decision with. Where a
/// decision already stands in its place, the rules' value the question
/// about that decision names (`named`). None where neither says it: the
/// stack was never sorted, or a decision stands and no question names the
/// rules' value.
pub(crate) fn rules_seen(
    store: &mut Store,
    stack: i64,
    axis: &str,
    named: Option<&str>,
) -> Result<Option<String>, StoreError> {
    let d = store.dialect();
    let sorted = store.query_opt(
        &format!(
            "SELECT 1 FROM {} WHERE stack_id = {}",
            store.qualified("classification"),
            d.param(1, Type::Int)
        ),
        &[Param::Int(stack)],
    )?;
    if sorted.is_none() {
        return Ok(None);
    }
    let rows = store.query(
        &format!(
            "SELECT value, tier FROM {} WHERE stack_id = {} AND axis = {} ORDER BY id",
            store.qualified("classification_axis"),
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        ),
        &[Param::Int(stack), Param::from(axis)],
    )?;
    let mut values = Vec::new();
    for r in &rows {
        if r.text(1)? == "decision" {
            return Ok(named.map(str::to_string));
        }
        if let Some(v) = r.opt_text(0)?.map(str::trim).filter(|v| !v.is_empty()) {
            values.push(v.to_string());
        }
    }
    Ok(Some(values.join(",")))
}

/// `GET /api/review/questions`: the scope's questions, the most scans
/// first.
fn list(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let scope = scope_of(
        registry,
        query.get("dataset").map(String::as_str),
        query.get("cohort").map(String::as_str),
    )?;
    let only = match query.get("stack").filter(|s| !s.is_empty()) {
        Some(s) => Some(
            s.parse::<i64>()
                .map_err(|_| Reply::error(400, "stack is a scan's id"))?,
        ),
        None => None,
    };
    let gathered = questions_of(registry, &caller.access, &scope)?;
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let (share, salt) = crate::review_rule::blind_setting(registry)?;
    let store = registry.store();
    // record 56 section 6.3, finding of 2026-10-15: read for one scan, the
    // questions are read beside it in the dataset viewer, whose names and
    // frame say NILS's answer. Each question on the scan is noted as shown
    // to the caller, so no answer to it is ever a blind label.
    if let Some(s) = only {
        let mut items: BTreeSet<i64> = BTreeSet::new();
        for scans in gathered.questions.values() {
            if let Some(scan) = scans.get(&s) {
                items.extend(scan.items.iter().copied());
            }
        }
        items.extend(gathered.elsewhere_items.get(&s).into_iter().flatten());
        for item in items {
            blind::revealed(store, item, &caller.principal).map_err(failed)?;
        }
    }
    let mut rows = Vec::new();
    let mut vocabularies = serde_json::Map::new();
    for (key, scans) in &gathered.questions {
        if only.is_some_and(|s| !scans.contains_key(&s)) {
            continue;
        }
        let modes = modes_of(store, scans.values(), &salt, share).map_err(failed)?;
        let mut row = row_of(
            store,
            &caller.access,
            &scope,
            key,
            scans,
            pack.as_deref(),
            &modes,
        )
        .map_err(failed)?;
        // the scan's own mode on the question, as its tile would say it
        if let Some(scan) = only.and_then(|s| scans.get(&s)) {
            let tile = tile_of(store, key, scan, &modes, &caller.principal).map_err(failed)?;
            row["mode"] = json!(tile.mode);
            row["blind"] = json!(tile.hidden);
        }
        rows.push(row);
        if !vocabularies.contains_key(&key.axis) {
            vocabularies.insert(
                key.axis.clone(),
                json!(vocabulary(pack.as_deref(), &key.axis)),
            );
        }
    }
    rows.sort_by(|a, b| {
        b["scans"]
            .as_i64()
            .cmp(&a["scans"].as_i64())
            .then_with(|| a["key"].as_str().cmp(&b["key"].as_str()))
    });
    let elsewhere: Vec<Value> = gathered
        .elsewhere
        .iter()
        .filter(|(_, scans)| only.is_none_or(|s| scans.contains(&s)))
        .map(|(kind, scans)| {
            let (words, page) = match kind.as_str() {
                nils_registry::asked::KIND => ("System 1 asks about a whole scan.", "asked"),
                "classify.excluded" | "classify.implied" => (
                    "The rules' answer breaks one of the pack's constraints.",
                    "queue",
                ),
                _ => ("Waiting in the queue.", "queue"),
            };
            json!({"kind": kind, "scans": scans.len(), "words": words, "page": page})
        })
        .collect();
    let mut doc = json!({
        "scope": scope_json(&scope),
        "scans": gathered.scans().len(),
        "questions": rows,
        "vocabularies": vocabularies,
        "elsewhere": elsewhere,
        "held": {
            "scans": gathered.held.len(),
            "campaigns": gathered.campaigns.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        },
    });
    if let Some(s) = only {
        doc["stack"] = json!(s);
    }
    Ok(Reply::ok(doc))
}

// ------------------------------------------------------ a question's scans

/// The facts of each scan a tile shows: its subject, its visit and day,
/// shaped below detail quasi as the dataset scans door shapes them.
fn facts(
    store: &mut Store,
    stacks: &[i64],
    quasi: bool,
) -> Result<HashMap<i64, Value>, StoreError> {
    use nils_registry::session::{Naming, Scheme};
    let mut out = HashMap::new();
    if stacks.is_empty() {
        return Ok(out);
    }
    let d = store.dialect();
    let study_t = table("study");
    let date = |alias: &str, column: &str| {
        d.text_of_qualified(Some(alias), study_t.column(column).expect("study column"))
    };
    let day = format!(
        "COALESCE({}, {})",
        date("sy", "date_filled"),
        date("sy", "study_date")
    );
    let mut rows = Vec::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT st.id, su.id, su.code, sy.id, {day}, f.text_series_description, st.orientation, st.n_instances \
             FROM {} st JOIN {} se ON se.id = st.series_id JOIN {} sy ON sy.id = se.study_id \
             JOIN {} su ON su.id = se.subject_id LEFT JOIN {} f ON f.stack_id = st.id WHERE st.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("study"),
            store.qualified("subject"),
            store.qualified("stack_fingerprint"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[])? {
            rows.push((
                r.int(0)?,
                r.int(1)?,
                r.opt_text(2)?.map(str::to_string),
                r.int(3)?,
                r.opt_text(4)?.map(str::to_string),
                r.opt_text(5)?.map(str::to_string),
                r.opt_text(6)?.map(str::to_string),
                r.opt_int(7)?,
            ));
        }
    }
    // the sessions of their studies, as the dataset scans door names them
    let scheme = Scheme::default();
    let window = nils_registry::cohort::built_window(store)?.unwrap_or(scheme.window_days);
    let digest = scheme.digest();
    let dated = matches!(scheme.naming, Naming::Date);
    let studies: BTreeSet<i64> = rows.iter().map(|r| r.3).collect();
    let mut sessions: HashMap<(i64, i64), (i64, Option<String>)> = HashMap::new();
    for chunk in studies.iter().copied().collect::<Vec<_>>().chunks(500) {
        let sql = format!(
            "SELECT scs.study_id, sc.subject_id, sc.id, sl.label FROM {} scs \
             JOIN {} sc ON sc.id = scs.session_id \
             LEFT JOIN {} sl ON sl.session_id = sc.id AND sl.scheme_digest = '{digest}' \
             WHERE scs.window_days = {window} AND scs.study_id IN ({}) ORDER BY sc.id",
            store.qualified("session_cache_study"),
            store.qualified("session_cache"),
            store.qualified("session_label"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[])? {
            sessions
                .entry((r.int(0)?, r.int(1)?))
                .or_insert((r.int(2)?, r.opt_text(3)?.map(str::to_string)));
        }
    }
    let shaped = |v: Option<String>, hide: bool| match v {
        Some(v) if hide => Value::from(crate::scans::shape(&v)),
        Some(v) => Value::from(v),
        None => Value::Null,
    };
    for (stack, subject, code, study, day, description, orientation, images) in rows {
        let session = sessions.get(&(study, subject)).cloned();
        out.insert(
            stack,
            json!({
                "stack": stack,
                "subject": {"id": subject, "code": shaped(code, !quasi)},
                "session": session.map(|(id, label)| json!({"id": id, "label": shaped(label, !quasi && dated)})),
                "study": study,
                "series_description": description,
                "orientation": orientation,
                "images": images,
                "day": shaped(day, !quasi),
            }),
        );
    }
    Ok(out)
}

/// The page asked for: `page` from 1 and `limit` up to [`PAGE_MOST`].
fn paging(query: &HashMap<String, String>) -> Result<(usize, usize), Reply> {
    let page = match query.get("page").filter(|p| !p.is_empty()) {
        Some(p) => p
            .parse::<usize>()
            .ok()
            .filter(|p| *p >= 1)
            .ok_or_else(|| Reply::error(400, "page is a number from 1"))?,
        None => 1,
    };
    let limit = match query.get("limit").filter(|l| !l.is_empty()) {
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|l| *l >= 1)
            .ok_or_else(|| Reply::error(400, "limit is a number from 1"))?
            .min(PAGE_MOST),
        None => PAGE,
    };
    Ok((page, limit))
}

fn key_named(text: &str) -> Result<Key, Reply> {
    Key::parse(text).ok_or_else(|| {
        Reply::error(
            400,
            format!(
                "{text} names no question: a key is <axis>:missing, <axis>:<a>|<b> or <axis>:<value>?"
            ),
        )
    })
}

/// `GET /api/review/questions/{key}/scans`: one page of a question's
/// scans, each with what a tile shows.
fn scans(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    key_text: &str,
    query: &HashMap<String, String>,
) -> Result<Reply, Reply> {
    let key = key_named(key_text)?;
    let scope = scope_of(
        registry,
        query.get("dataset").map(String::as_str),
        query.get("cohort").map(String::as_str),
    )?;
    let (page, limit) = paging(query)?;
    let mut gathered = questions_of(registry, &caller.access, &scope)?;
    let Some(asked) = gathered.questions.remove(&key) else {
        return Err(Reply::error(
            404,
            format!("no scan of {} waits on {}", scope.name(), key.text()),
        ));
    };
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack);
    let (share, salt) = crate::review_rule::blind_setting(registry)?;
    let store = registry.store();
    let all: Vec<i64> = asked.keys().copied().collect();
    // each question's mode, drawn the first time it is read
    let modes = modes_of(store, asked.values(), &salt, share).map_err(failed)?;
    let row = row_of(
        store,
        &caller.access,
        &scope,
        &key,
        &asked,
        pack.as_deref(),
        &modes,
    )
    .map_err(failed)?;
    // the order: the subject, the day, the stack
    let mut known = facts(store, &all, quasi).map_err(failed)?;
    let mut order: Vec<i64> = all.clone();
    let sort_key = |s: &i64, known: &HashMap<i64, Value>| {
        let f = known.get(s).cloned().unwrap_or(Value::Null);
        (
            f["subject"]["code"].as_str().unwrap_or("").to_string(),
            f["day"].as_str().unwrap_or("").to_string(),
            *s,
        )
    };
    order.sort_by_key(|s| sort_key(s, &known));
    let total = order.len();
    let pages = total.div_ceil(limit).max(1);
    let shown: Vec<i64> = order
        .iter()
        .skip((page - 1) * limit)
        .take(limit)
        .copied()
        .collect();
    // the time an answer takes is counted from a question's first read
    let items: BTreeSet<i64> = shown
        .iter()
        .filter_map(|s| asked.get(s))
        .flat_map(|scan| scan.items.iter().copied())
        .collect();
    for item in &items {
        blind::read(store, *item, &caller.principal).map_err(failed)?;
    }
    // each scan's tile, decided once from every question on it: what the
    // answer to it is later marked by
    let mut shown_tiles: HashMap<i64, Tile> = HashMap::new();
    for s in &shown {
        if let Some(scan) = asked.get(s) {
            let tile = tile_of(store, &key, scan, &modes, &caller.principal).map_err(failed)?;
            shown_tiles.insert(*s, tile);
        }
    }
    let nils = answers_on(store, &shown, &key.axis).map_err(failed)?;
    let (model, s1) = system1_on(store, &shown, &key.axis).map_err(failed)?;
    let certificate = match &model {
        Some(m) => opinion::confirmed(store, m.id).map_err(failed)?,
        None => None,
    };
    let mut doc = json!({
        "scans": shown.iter().map(|s| known.remove(s).unwrap_or_else(|| json!({"stack": s}))).collect::<Vec<_>>(),
    });
    crate::scans::with_names(registry, pack.as_deref(), &mut doc)?;
    crate::scans::with_pictures(registry, caller, &doors.home, &mut doc)?;
    let store = registry.store();
    let stored_pairs = nils_registry::round::rules_answers(store, &shown).map_err(failed)?;
    let mut tiles = Vec::new();
    for mut tile in doc["scans"].as_array().cloned().unwrap_or_default() {
        let stack = tile["stack"].as_i64().unwrap_or_default();
        let Some(scan) = asked.get(&stack) else {
            continue;
        };
        let Tile { mode, hidden } = shown_tiles.get(&stack).copied().unwrap_or(Tile {
            mode: blind::ROUTINE,
            hidden: false,
        });
        tile["items"] = json!(scan.items);
        tile["status"] = json!(if scan.status_staged { "staged" } else { "open" });
        tile["mode"] = json!(mode);
        tile["blind"] = json!(hidden);
        if hidden {
            for k in [
                "nils", "other", "rules", "system1", "staged", "name", "bids", "axes",
            ] {
                tile[k] = Value::Null;
            }
            tiles.push(tile);
            continue;
        }
        let answer = nils.get(&stack).cloned().flatten();
        tile["nils"] = json!(answer);
        tile["other"] = match key.shape {
            Shape::Pair => json!(
                key.values
                    .iter()
                    .find(|v| Some(v.as_str()) != answer.as_deref())
                    .cloned()
            ),
            _ => Value::Null,
        };
        let why = crate::reader::why(store, stack, pack.as_deref(), quasi, false)
            .map_err(failed)?
            .unwrap_or(Value::Null);
        tile["rules"] = crate::review_rule::rules_reason(&why, &key.axis);
        // what the pack's ranking decided over on an axis it decided to be
        // nothing, which the reader keeps beside the axes
        if tile["rules"]["overridden"].is_null() && why["overridden"][&key.axis].is_array() {
            tile["rules"]["overridden"] = why["overridden"][&key.axis].clone();
        }
        tile["system1"] = match s1.get(&stack) {
            None => Value::Null,
            Some(o) => {
                let held = stored_pairs
                    .get(&stack)
                    .and_then(|a| a.get(&key.axis))
                    .and_then(|(v, _)| v.clone());
                json!({
                    "value": o.value, "p": o.p, "reliability": o.reliability,
                    "agrees": o.agrees(held.as_deref()),
                    "model": model.as_ref().map(nils_registry::model::Model::named),
                })
            }
        };
        tile["staged"] = scan.staged.clone().unwrap_or(Value::Null);
        tiles.push(tile);
    }
    let status = match (&model, certificate) {
        (None, _) => "no_model",
        (Some(_), Some(_)) => "confirmed",
        (Some(_), None) => "not_confirmed",
    };
    Ok(Reply::ok(json!({
        "key": key.text(),
        "axis": key.axis,
        "scope": scope_json(&scope),
        "shape": key.shape.name(),
        "words": row["words"],
        "reason": row["reason"],
        "values": row["values"],
        "vocabulary": vocabulary(pack.as_deref(), &key.axis),
        "system1": {
            "status": status,
            "model": model.as_ref().map(nils_registry::model::Model::named),
            "why": match status {
                "no_model" => "no System 1 model is chosen for its task",
                "not_confirmed" => "System 1 is not yet confirmed: no recorded certificate names it; its answer is a research model's",
                _ => "System 1 is confirmed by a recorded certificate",
            },
        },
        "total": total,
        "page": page,
        "pages": pages,
        "limit": limit,
        "scans": tiles,
        "pictures": doc["pictures"],
    })))
}

// --------------------------------------------------------------- answers

/// One scan's answer: a value, or nothing on the axis.
#[derive(Debug, Clone, PartialEq)]
enum Given {
    Value(String),
    Nothing,
    Skip,
}

impl Given {
    fn as_json(&self) -> Value {
        match self {
            Given::Value(v) => json!({"value": v}),
            Given::Nothing => json!({"nothing": true}),
            Given::Skip => json!({"skip": true}),
        }
    }
}

/// An answer's value from a body or an exception: `value`, `nothing: true`
/// or (for an exception) `skip: true`, each held to the axis's values.
fn given_of(doc: &Value, pack: &nils_pack::Pack, axis: &str, skip: bool) -> Result<Given, Reply> {
    let nothing = doc["nothing"].as_bool() == Some(true);
    let skipped = skip && doc["skip"].as_bool() == Some(true);
    let value = doc.get("value").filter(|v| !v.is_null());
    match (value, nothing, skipped) {
        (Some(v), false, false) => {
            let text = v
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| Reply::error(400, "value is one of the axis's values, as text"))?;
            stored_value(pack, axis, text)
                .map(Given::Value)
                .ok_or_else(|| Reply::error(400, format!("{text} is not a value of {axis}")))
        }
        (None, true, false) => Ok(Given::Nothing),
        (None, false, true) => Ok(Given::Skip),
        (None, false, false) => Err(Reply::error(
            400,
            if skip {
                "an exception names its value, nothing: true or skip: true"
            } else {
                "an answer names its value, or nothing: true"
            },
        )),
        _ => Err(Reply::error(
            400,
            "one of value, nothing: true or skip: true, not two",
        )),
    }
}

fn review_refusal(e: nils_registry::review::Error) -> Reply {
    match e {
        nils_registry::review::Error::Refused(m) => Reply::gated(409, m),
        nils_registry::review::Error::Forbidden(m) => Reply::error(403, m),
        other => Reply::error(500, other.to_string()),
    }
}

/// The ids as words for a refusal, the first five.
fn named(ids: &[i64]) -> String {
    let mut words: Vec<String> = ids.iter().take(5).map(i64::to_string).collect();
    if ids.len() > 5 {
        words.push(format!("and {} more", ids.len() - 5));
    }
    words.join(", ")
}

/// `POST /api/review/questions/{key}/answer`: the decisions, in one
/// transaction, then the re-sort of their scans.
fn answer(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    key_text: &str,
    body: &str,
) -> Result<Reply, Reply> {
    let key = key_named(key_text)?;
    let doc = json_body(body)?;
    let scope = scope_of(registry, doc["dataset"].as_str(), doc["cohort"].as_str())?;
    let pack = crate::reader::served_pack(doors.pack_dir.as_deref(), &doors.ask_pack).ok_or_else(
        || {
            Reply::error(
                409,
                "no pack is served here, so an answer cannot be held to the axis's values",
            )
        },
    )?;
    let main = given_of(&doc, &pack, &key.axis, false)?;
    let mut gathered = questions_of(registry, &caller.access, &scope)?;
    let asked = gathered.questions.remove(&key).unwrap_or_default();
    // the scans the answer covers: those the person saw, else every one
    let covered: Vec<i64> = match doc.get("stacks").filter(|v| !v.is_null()) {
        Some(v) => {
            let list = v
                .as_array()
                .ok_or_else(|| Reply::error(400, "stacks is a list of scan ids"))?;
            let mut out = Vec::new();
            for s in list {
                let id = s
                    .as_i64()
                    .ok_or_else(|| Reply::error(400, "stacks is a list of scan ids"))?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            out
        }
        None => asked.keys().copied().collect(),
    };
    if covered.is_empty() {
        return Err(Reply::error(
            404,
            format!("no scan of {} waits on {}", scope.name(), key.text()),
        ));
    }
    let held: Vec<i64> = covered
        .iter()
        .copied()
        .filter(|s| gathered.held.contains(s) && !asked.contains_key(s))
        .collect();
    if !held.is_empty() {
        let names: Vec<String> = gathered
            .campaigns
            .iter()
            .map(|(id, n)| format!("{n} ({id})"))
            .collect();
        return Err(Reply::error(
            409,
            format!(
                "an open campaign holds the question on scan {}: it is answered in the campaign ({})",
                named(&held),
                names.join(", ")
            ),
        ));
    }
    let gone: Vec<i64> = covered
        .iter()
        .copied()
        .filter(|s| !asked.contains_key(s))
        .collect();
    if !gone.is_empty() {
        return Err(Reply::error(
            409,
            format!(
                "{} does not wait on scan {} in {} now; it may have been answered: read the question again",
                key.text(),
                named(&gone),
                scope.name()
            ),
        ));
    }
    // the exceptions, each a scan the answer covers
    let mut each: BTreeMap<i64, Given> = covered.iter().map(|s| (*s, main.clone())).collect();
    for e in doc["except"].as_array().into_iter().flatten() {
        let stack = e["stack"]
            .as_i64()
            .ok_or_else(|| Reply::error(400, "an exception names its scan as stack"))?;
        if !each.contains_key(&stack) {
            return Err(Reply::error(
                400,
                format!("scan {stack} is set apart, but the answer does not cover it"),
            ));
        }
        each.insert(stack, given_of(e, &pack, &key.axis, true)?);
    }
    let (kind, version, model) = crate::serve::author_at_apply(registry, caller, &doc)?;
    let principal = caller.principal.as_str();
    // what each scan's tile showed as the person answered, decided once a
    // scan from every question on it, before anything is written: its
    // mode, whether NILS's answers were hidden, and the rules' answer it
    // showed, as stored
    let (share, salt) = crate::review_rule::blind_setting(registry)?;
    let store = registry.store();
    let answered: Vec<&Scan> = each
        .iter()
        .filter(|(_, g)| **g != Given::Skip)
        .map(|(s, _)| &asked[s])
        .collect();
    let modes = modes_of(store, answered.iter().copied(), &salt, share).map_err(failed)?;
    let mut tiles: HashMap<i64, (Tile, Option<String>)> = HashMap::new();
    for (stack, given) in &each {
        if *given == Given::Skip {
            continue;
        }
        let scan = &asked[stack];
        let tile = tile_of(store, &key, scan, &modes, principal).map_err(failed)?;
        let seen =
            rules_seen(store, *stack, &key.axis, scan.rules_named.as_deref()).map_err(failed)?;
        tiles.insert(*stack, (tile, seen));
    }
    // the decisions, one a scan, in one transaction
    struct Wrote {
        stack: i64,
        given: Given,
        decision: i64,
        /// Every decision written for the scan, one a question it closed,
        /// the last in force.
        decisions: Vec<i64>,
        items: Vec<(i64, String)>,
        staged: bool,
    }
    registry.store().begin().map_err(failed)?;
    let written = (|| -> Result<(Vec<Wrote>, BTreeSet<i64>), Reply> {
        let mut out = Vec::new();
        let mut closed: BTreeSet<i64> = BTreeSet::new();
        for (stack, given) in &each {
            let value = match given {
                Given::Skip => continue,
                Given::Value(v) => Some(v.as_str()),
                Given::Nothing => None,
            };
            let scan = &asked[stack];
            let mut wrote: Option<Wrote> = None;
            for (item, group, kind_of) in &scan.reach {
                // a question about the stack alone closed by the decision
                // just written is answered with it
                if !group && closed.contains(item) {
                    continue;
                }
                let applied = nils_registry::review::apply_within(
                    registry,
                    &nils_registry::review::Apply {
                        item: *item,
                        member: group.then_some(*stack),
                        scope: "stack",
                        value,
                        author: nils_registry::review::Author {
                            who: principal,
                            kind,
                            version,
                            model,
                        },
                        stage: false,
                        why: None,
                        campaign: None,
                    },
                )
                .map_err(review_refusal)?;
                closed.extend(applied.closed.iter().copied());
                match &mut wrote {
                    Some(w) => {
                        w.decision = applied.decision;
                        w.decisions.push(applied.decision);
                        w.items.push((*item, kind_of.clone()));
                    }
                    None => {
                        wrote = Some(Wrote {
                            stack: *stack,
                            given: given.clone(),
                            decision: applied.decision,
                            decisions: vec![applied.decision],
                            items: vec![(*item, kind_of.clone())],
                            staged: applied.staged,
                        });
                    }
                }
            }
            if let Some(w) = wrote {
                out.push(w);
            }
        }
        Ok((out, closed))
    })();
    let (wrote, closed) = match written {
        Ok(w) => {
            registry.store().commit().map_err(failed)?;
            w
        }
        Err(e) => {
            registry.store().rollback().ok();
            registry.refresh_meta().ok();
            return Err(e);
        }
    };
    // each scan's decisions marked once, from what its tile showed: its
    // mode, the seconds since its first question was first read, and
    // whether the answer was given blind (spec 11.4: a blind label is never
    // written where NILS's answer was on screen)
    let store = registry.store();
    let mut blind_given = 0usize;
    for w in &wrote {
        let scan = &asked[&w.stack];
        let (tile, seen) = tiles.get(&w.stack).cloned().unwrap_or((
            Tile {
                mode: blind::ROUTINE,
                hidden: false,
            },
            None,
        ));
        let mut seconds: Option<f64> = None;
        let mut read_all = true;
        for item in &scan.items {
            match blind::seconds_since_read(store, *item, principal).map_err(failed)? {
                Some(t) => seconds = Some(seconds.map_or(t, |s: f64| s.max(t))),
                None => read_all = false,
            }
        }
        let blind_now = if tile.hidden {
            // hidden on the tile, and every question on the scan read
            // through a door that hid NILS's answers (none was shown since:
            // a shown one leaves the tile not hidden)
            read_all
        } else if tile.mode == blind::BLIND_FIRST {
            // shown: blind only where every question on the scan is blind
            // first and the answer is the provisional one the person gave
            // each before its suggestions showed, as the queue marks it
            let mut all = true;
            for item in &scan.items {
                let kept = modes.get(item) == Some(&blind::BLIND_FIRST)
                    && blind::provisional(store, *item, principal)
                        .map_err(failed)?
                        .is_some_and(|p| blind::same_answer(&p.answer, &w.given.as_json()));
                if !kept {
                    all = false;
                    break;
                }
            }
            all
        } else {
            false
        };
        blind::mark_decision(store, &w.decisions, blind_now, seconds, tile.mode).map_err(failed)?;
        if blind_now {
            blind_given += 1;
        }
        // record 56 section 5.2: the rules' answer the person saw, exactly
        // as the tile showed it, so a sort does not ask about the decision
        // again until the rules move from it; none where it was hidden
        if !(tile.hidden && blind_now)
            && let Some(rules) = &seen
        {
            for d in &w.decisions {
                nils_registry::review::note_rules_in_view(store, *d, rules).map_err(failed)?;
            }
        }
    }
    let stacks: Vec<i64> = wrote.iter().map(|w| w.stack).collect();
    let resort = resort(doors, registry, caller, &scope, &key, &stacks);
    let left = questions_of(registry, &caller.access, &scope)?
        .questions
        .get(&key)
        .map_or(0, BTreeMap::len);
    let skipped: Vec<i64> = each
        .iter()
        .filter(|(_, g)| **g == Given::Skip)
        .map(|(s, _)| *s)
        .collect();
    Ok(Reply::ok(json!({
        "key": key.text(),
        "axis": key.axis,
        "scope": scope_json(&scope),
        "answered": wrote.len(),
        "decisions": wrote.iter().map(|w| json!({
            "stack": w.stack,
            "value": match &w.given { Given::Value(v) => json!(v), _ => Value::Null },
            "decision": w.decision,
            "items": w.items.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            "staged": w.staged,
        })).collect::<Vec<_>>(),
        "closed": closed,
        "skipped": skipped,
        "staged": wrote.iter().any(|w| w.staged),
        "blind": blind_given,
        "resort": resort.0,
        "picks": resort.1,
        "left": left,
    })))
}

/// Re-sort the scans an answer decided, at once (record 56 §5.2): a
/// classify job over those stacks alone, by the pack each was sorted with;
/// the gate again over those of a dataset flagged as damaged; and the pick
/// run a sort is followed by, queued where the caller may run one. A sort
/// that holds the registry past a short wait leaves them to the next.
fn resort(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    key: &Key,
    stacks: &[i64],
) -> (Value, Value) {
    if stacks.is_empty() {
        return (
            json!({"job": null, "state": "done", "stacks": 0, "why": "no scan was answered"}),
            json!({"why": "no scan was answered"}),
        );
    }
    let started = std::time::Instant::now();
    // the pack, its version and the overlay each stack was last sorted with
    let mut by_pack: BTreeMap<(String, String, Option<String>), Vec<i64>> = BTreeMap::new();
    {
        let store = registry.store();
        for chunk in stacks.chunks(500) {
            let sql = format!(
                "SELECT stack_id, pack, pack_version, overlay FROM {} WHERE stack_id IN ({})",
                store.qualified("classification"),
                chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            match store.query(&sql, &[]) {
                Ok(rows) => {
                    for r in rows {
                        if let (Ok(s), Ok(p), Ok(v), Ok(o)) =
                            (r.int(0), r.text(1), r.text(2), r.opt_text(3))
                        {
                            by_pack
                                .entry((p.to_string(), v.to_string(), o.map(str::to_string)))
                                .or_default()
                                .push(s);
                        }
                    }
                }
                Err(e) => {
                    return (
                        json!({"job": null, "state": "failed", "stacks": 0, "why": e.to_string()}),
                        json!({"why": "the scans were not re-sorted"}),
                    );
                }
            }
        }
    }
    let mut jobs = Vec::new();
    let mut states = Vec::new();
    let mut why = Vec::new();
    let mut questions: BTreeMap<String, i64> = BTreeMap::new();
    let mut sorted = 0i64;
    let mut picks = Vec::new();
    for ((name, sorted_version, overlay), list) in by_pack {
        let pack = match pack_as_sorted(doors, registry, &name, &sorted_version, overlay.as_deref())
        {
            Ok(p) => p,
            Err(refused) => {
                states.push("refused");
                why.push(format!(
                    "{} of them not re-sorted: {refused}; the decisions stand, and the next sort re-sorts them",
                    list.len()
                ));
                continue;
            }
        };
        let settings = nils_classify::Settings {
            name: format!("re-sort after {} answered {}", caller.principal, key.text()),
            stacks: nils_classify::job::Stacks::Only(std::sync::Arc::new(
                list.iter().copied().collect(),
            )),
            scope: vec![format!(
                "review answer {}: {} scans",
                key.text(),
                list.len()
            )],
            ..nils_classify::Settings::default()
        };
        let cancel = nils_digest::Cancel::new();
        let mut ran = nils_classify::classify::classify(registry, &pack, &settings, &cancel);
        while matches!(ran, Err(nils_classify::Error::Busy { .. }))
            && started.elapsed() < RESORT_WAIT
        {
            std::thread::sleep(std::time::Duration::from_millis(200));
            ran = nils_classify::classify::classify(registry, &pack, &settings, &cancel);
        }
        match ran {
            Ok(report) => {
                jobs.push(report.job_id);
                states.push("done");
                sorted += report.written;
                // a question the re-sort asked of them, by kind
                match asked_by(registry.store(), report.job_id) {
                    Ok(asked) => {
                        for (k, n) in asked {
                            *questions.entry(k).or_insert(0) += n;
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "nils: the questions re-sort job {} asked could not be counted: {e}",
                            report.job_id
                        );
                        why.push(format!(
                            "the questions re-sort job {} asked could not be counted: {e}",
                            report.job_id
                        ));
                    }
                }
                // the pick run a sort is followed by, for their subjects
                picks.push(picks_after(doors, registry, caller, report.job_id, &name));
            }
            Err(nils_classify::Error::Busy { job_id, .. }) => {
                states.push("waiting");
                why.push(format!(
                    "sort job {job_id} holds the registry; the decisions stand, and the next sort re-sorts these scans"
                ));
            }
            Err(e) => {
                states.push("failed");
                why.push(format!("the re-sort failed: {e}; the decisions stand"));
            }
        }
    }
    // the gate again over the scans of a dataset flagged as damaged; an
    // answer about one dataset whose headers are not reads no other's flag
    let flagged = match scope {
        Scope::Dataset { place, sources } => {
            crate::review_rule::damaged(registry.store(), place, sources).unwrap_or(true)
        }
        _ => true,
    };
    let gate = if flagged {
        crate::review_rule::after_opinions(
            &doors.home,
            registry,
            stacks,
            &caller.principal,
            doors.pack_dir.as_deref(),
        )
    } else {
        json!({"rounds": [], "why": "the dataset's headers are not flagged as damaged, so the rules decide"})
    };
    let state = if states.contains(&"failed") {
        "failed"
    } else if states.contains(&"refused") {
        "refused"
    } else if states.contains(&"waiting") {
        "waiting"
    } else {
        "done"
    };
    let resort = json!({
        "job": jobs.first(),
        "jobs": jobs,
        "state": state,
        "stacks": sorted,
        "seconds": (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        "questions": questions,
        "gate": gate,
        "why": if why.is_empty() { Value::Null } else { json!(why.join("; ")) },
    });
    let picks = match picks.len() {
        0 => json!({"why": "no pick run follows a re-sort that did not run"}),
        1 => picks.remove(0),
        _ => json!({"runs": picks}),
    };
    (resort, picks)
}

/// The pack a stack was last sorted with, loaded to sort it again the same
/// way: the same pack, at the same version, under the same overlay. The
/// adopted scoped changes it was sorted with (`proposal:<id>`) every sort
/// applies by itself, to the stacks their scope holds. Refused, with why,
/// where the pack served here is not that version or the overlay is not
/// one the registry keeps: a re-sort by another pack would judge the scan
/// by rules it was not sorted with.
fn pack_as_sorted(
    doors: &Doors,
    registry: &mut Registry,
    name: &str,
    version: &str,
    overlay: Option<&str>,
) -> Result<nils_pack::Pack, String> {
    let dir = doors
        .pack_dir
        .as_ref()
        .ok_or_else(|| format!("pack {name} is not served here"))?;
    // an overlay the sort was read under, by its `name@version`; scoped
    // changes are named `proposal:<id>` and come back by themselves
    let named = overlay
        .map(str::trim)
        .filter(|o| !o.is_empty() && !o.split(',').all(|p| p.trim().starts_with("proposal:")));
    let document = match named {
        None => None,
        Some(id) => {
            let kept = nils_registry::overlay::list(registry.store())
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|o| format!("{}@{}", o.name, o.version) == id)
                .ok_or_else(|| {
                    format!("they were sorted under overlay {id}, which the registry does not keep")
                })?;
            Some(
                nils_pack::Overlay::parse(
                    &format!("overlay {}", kept.id),
                    &kept.document.to_string(),
                )
                .map_err(|e| format!("overlay {id} does not load: {e}"))?,
            )
        }
    };
    let pack = nils_pack::load(&dir.join(name), document.as_ref())
        .map_err(|e| format!("pack {name} is not served here ({e})"))?;
    let served = pack.version.to_string();
    if served != version {
        return Err(format!(
            "they were sorted with pack {name} {version}, and the pack served here is {served}"
        ));
    }
    Ok(pack)
}

/// The questions a sort job asked, by kind: its own groups and items.
fn asked_by(store: &mut Store, job: i64) -> Result<BTreeMap<String, i64>, StoreError> {
    let mut out = BTreeMap::new();
    for r in store.query(
        &format!(
            "SELECT kind, COUNT(*) FROM {} WHERE job_id = {} AND status = 'open' AND scope = 'stack' GROUP BY kind",
            store.qualified("review_item"),
            store.dialect().param(1, Type::Int)
        ),
        &[Param::Int(job)],
    )? {
        *out.entry(r.text(0)?.to_string()).or_insert(0) += r.int(1)?;
    }
    for r in store.query(
        &format!(
            "SELECT kind, CAST(SUM(members) AS BIGINT) FROM {} WHERE job_id = {} AND status = 'open' AND scope = 'group' GROUP BY kind",
            store.qualified("review_item"),
            store.dialect().param(1, Type::Int)
        ),
        &[Param::Int(job)],
    )? {
        *out.entry(r.text(0)?.to_string()).or_insert(0) += r.opt_int(1)?.unwrap_or(0);
    }
    Ok(out)
}

/// The pick run after a re-sort, for the subjects it judged, queued under
/// the caller where they may run one; why not otherwise.
fn picks_after(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    job: i64,
    pack: &str,
) -> Value {
    if !crate::chain::pack_has_picks(doors.pack_dir.as_deref(), pack) {
        return json!({"why": format!("pack {pack} declares no picks")});
    }
    let sorted = match crate::pick_after::sorted_by(registry.store(), job) {
        Ok(s) => s,
        Err(e) => return json!({"why": e.to_string()}),
    };
    if sorted.subjects.is_empty() {
        return json!({"why": "the re-sort judged no subject a pick run follows"});
    }
    let mut command: Vec<String> = ["pick", "run", "--after-sort"]
        .iter()
        .map(|w| w.to_string())
        .collect();
    command.push(job.to_string());
    command.extend(["--pack".to_string(), pack.to_string()]);
    if let Some(dir) = &doors.pack_dir {
        command.extend(["--pack-dir".to_string(), dir.display().to_string()]);
    }
    if let Some((grant, detail)) = crate::serve::verb_needs(&command)
        && (!caller.access.holds(grant) || caller.access.detail < detail)
    {
        return json!({"why": format!("a pick run needs {grant}; the next sort's pick run picks for these scans")});
    }
    match nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(&format!("picks after re-sort job {job}")),
        Some(&caller.principal),
        crate::serve::queued_by(caller),
    ) {
        Ok(id) => json!({"job": id, "after": job}),
        Err(e) => json!({"why": e.to_string()}),
    }
}

/// The doors' routes, before the review doors' own table, which would take
/// `questions` for an item's id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
    quasi: bool,
) -> Option<Result<Reply, Reply>> {
    Some(match (method, segs) {
        ("GET", ["api", "review", "questions"]) => list(doors, registry, caller, query),
        ("GET", ["api", "review", "questions", key, "scans"]) => {
            scans(doors, registry, caller, quasi, key, query)
        }
        ("POST", ["api", "review", "questions", key, "answer"]) => {
            answer(doors, registry, caller, key, body)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry in `dir`, on SQLite or, with a DSN, in a schema of its own.
    fn registry(dir: &std::path::Path, postgres: Option<(&str, &str)>) -> Registry {
        use nils_registry::{Backend, Home, InitOptions, Scheme};
        let home = Home::new(dir);
        home.keys(None).add("nils", b"a test passphrase").unwrap();
        home.init(&InitOptions {
            backend: if postgres.is_some() {
                Backend::Postgres
            } else {
                Backend::Sqlite
            },
            dsn: postgres.map(|(dsn, _)| dsn.to_string()),
            schema: postgres.map(|(_, schema)| schema.to_string()),
            scheme: Scheme::DEFAULT,
            key: "nils".into(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap()
    }

    /// The questions a sort job asked are counted by kind, a group by its
    /// members, on both backends: Postgres sums an integer column as a
    /// NUMERIC, which is cast back to a whole number.
    fn questions_asked_are_counted(postgres: Option<&str>) {
        use nils_registry::store::Insert;
        let dir = std::env::temp_dir().join(format!(
            "nils-review-questions-asked-{}-{}",
            postgres.is_some(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let schema = "nils_review_questions_asked";
        if let Some(dsn) = postgres {
            Store::connect_postgres(dsn, schema)
                .unwrap()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .unwrap();
        }
        let mut r = registry(&dir.join("reg"), postgres.map(|d| (d, schema)));
        let store = r.store();
        let row = |kind: &str, scope: &str, status: &str, job: i64, members: Option<i64>| {
            vec![
                Param::from(kind),
                Param::from(scope),
                Param::from(r#"{"stack_id": 1}"#),
                Param::from("{}"),
                Param::from(status),
                Param::from("2026-10-15T10:00:00Z"),
                Param::Int(job),
                members.map_or(Param::Null, Param::Int),
            ]
        };
        store
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
                    ],
                ),
                &[
                    row("base:missing", "group", "open", 7, Some(3)),
                    row("base:missing", "group", "open", 7, Some(2)),
                    row("base:missing", "stack", "open", 7, None),
                    row("base:decision", "stack", "open", 7, None),
                    // another job's, and one answered
                    row("base:missing", "group", "open", 8, Some(9)),
                    row("base:missing", "group", "decided", 7, Some(4)),
                ],
            )
            .unwrap();
        let asked = asked_by(store, 7).unwrap();
        assert_eq!(
            asked,
            [
                ("base:decision".to_string(), 1),
                ("base:missing".to_string(), 6)
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        );
        assert!(asked_by(store, 9).unwrap().is_empty());
        drop(r);
        if let Some(dsn) = postgres {
            Store::connect_postgres(dsn, schema)
                .unwrap()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_questions_a_re_sort_asked_are_counted() {
        questions_asked_are_counted(None);
    }

    #[test]
    fn the_questions_a_re_sort_asked_are_counted_on_postgres() {
        match std::env::var("NILS_TEST_POSTGRES_DSN") {
            Ok(dsn) if !dsn.is_empty() => questions_asked_are_counted(Some(&dsn)),
            _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped"),
        }
    }

    /// A question read blind first is neutral: its keys are the axis's
    /// first values, never what the rules, a stage or System 1 say; and a
    /// question whose words name a value is never read blind.
    #[test]
    fn a_blind_first_question_gives_nothing_away() {
        let scan = |items: &[i64]| Scan {
            items: items.iter().copied().collect(),
            ..Scan::default()
        };
        let modes: HashMap<i64, &'static str> = [(1, blind::BLIND_FIRST), (2, blind::ROUTINE)]
            .into_iter()
            .collect();
        let missing = Key::missing("base");
        assert!(read_blind(
            &missing,
            [scan(&[2]), scan(&[1, 2])].iter(),
            &modes
        ));
        assert!(!read_blind(&missing, [scan(&[2])].iter(), &modes));
        for named in [Key::doubt("base", "T2w"), Key::pair("base", "FLAIR", "T2w")] {
            assert!(
                !read_blind(&named, [scan(&[1])].iter(), &modes),
                "{named:?}"
            );
            assert!(!may_be_blind(&named));
        }
    }

    #[test]
    fn a_key_reads_as_it_is_written() {
        for text in [
            "base:missing",
            "base:FLAIR|T2w",
            "base:T2*w?",
            "body_part:neck|spine",
        ] {
            let k = Key::parse(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(k.text(), text);
        }
        // a pair is named in the order of its values' names, whichever came first
        assert_eq!(
            Key::parse("base:T2w|FLAIR").unwrap().text(),
            "base:FLAIR|T2w"
        );
        assert_eq!(
            Key::pair("base", "T2w", "FLAIR"),
            Key::pair("base", "FLAIR", "T2w")
        );
        // the same value twice is one value in doubt
        assert_eq!(Key::pair("base", "T2w", "T2w").text(), "base:T2w?");
        for bad in [
            "",
            "base",
            "base:",
            "Base:missing",
            "base:a|",
            "base:|b",
            "base:a|b|c",
            "base:a|a",
            "base:?",
        ] {
            assert!(Key::parse(bad).is_none(), "{bad}");
        }
    }

    fn asking(kind: &str, evidence: Value) -> Asking {
        Asking {
            item: 1,
            kind: kind.to_string(),
            status: "open".into(),
            group: false,
            stack: 7,
            evidence,
            decided: false,
        }
    }

    #[test]
    fn each_kind_is_the_question_it_asks() {
        let key = |kind: &str, ev: Value| key_of(&asking(kind, ev)).map(|k| k.text());
        assert_eq!(
            key("base:missing", json!({"axis": "base"})).as_deref(),
            Some("base:missing")
        );
        // the gate: the rules' answer against System 1's
        assert_eq!(
            key(
                "base:disagree",
                json!({"rules": {"value": "T2w"}, "system1": {"value": "FLAIR"}})
            )
            .as_deref(),
            Some("base:FLAIR|T2w")
        );
        // the rules found nothing and System 1 says a value: nothing found
        assert_eq!(
            key(
                "base:disagree",
                json!({"rules": {"value": null}, "system1": {"value": "FLAIR"}})
            )
            .as_deref(),
            Some("base:missing")
        );
        assert_eq!(
            key("base:unsure", json!({"rules": {"value": "T2w"}})).as_deref(),
            Some("base:T2w?")
        );
        assert_eq!(
            key("base:decision", json!({"rule": "T2w", "decision": "FLAIR"})).as_deref(),
            Some("base:FLAIR|T2w")
        );
        assert_eq!(
            key(
                "body_part:conflict",
                json!({"value": "spine", "other": "neck"})
            )
            .as_deref(),
            Some("body_part:neck|spine")
        );
        assert_eq!(
            key("base:low_confidence", json!({"value": "T1w"})).as_deref(),
            Some("base:T1w?")
        );
        // several axes at once are answered on their own page
        assert_eq!(key("classify.asked", json!({"axes": ["base"]})), None);
        assert_eq!(key("classify.excluded", json!({})), None);
    }

    #[test]
    fn the_words_say_the_question_and_why() {
        let kinds: BTreeMap<String, i64> = [("base:missing".to_string(), 7)].into_iter().collect();
        let (w, r) = words_of(&Key::missing("base"), &kinds, &[], false);
        assert_eq!(w, "Which base?");
        assert_eq!(r, "The rules found no base for these scans.");
        let kinds: BTreeMap<String, i64> = [
            ("base:disagree".to_string(), 3),
            ("base:decision".to_string(), 1),
        ]
        .into_iter()
        .collect();
        let (w, r) = words_of(
            &Key::pair("base", "FLAIR", "T2w"),
            &kinds,
            &["T2w".into(), "FLAIR".into()],
            true,
        );
        assert_eq!(w, "T2w or FLAIR?");
        assert_eq!(
            r,
            "The rules and System 1 disagree. A person's answer differs from the rules."
        );
        let kinds: BTreeMap<String, i64> = [("body_part:conflict".to_string(), 1)]
            .into_iter()
            .collect();
        let (w, _) = words_of(&Key::missing("body_part"), &kinds, &[], false);
        assert_eq!(w, "Which body part?");
    }
}
