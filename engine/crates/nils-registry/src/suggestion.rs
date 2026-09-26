// SPDX-License-Identifier: AGPL-3.0-only

//! Suggestions from outside the engine (record 50 R3): a campaign's items
//! can carry an answer suggested by something other than the engine's own
//! rules, such as v0's committed body parts or a model's proposals, each
//! recorded with its author (`v0-model`, `v0-person`, a model id) and, where
//! the source gave them, a confidence per class.
//!
//! - **A suggestion is never an answer.** It is shown beside an item, and a
//!   person accepts it or corrects it; what a person accepted is the label,
//!   kept as that person's answer with the suggestion and its author beside
//!   it. Nothing here is a decision.
//! - **Blind stays blind.** A stack of a sample sealed now takes no
//!   suggestion: an import leaves it out and counts it, and every door that
//!   shows suggestions leaves out a stack read blind.
//! - **One per author.** An author's later import replaces its earlier
//!   suggestion for an item. Where several authors suggested, the latest
//!   import is shown first ([`primary`]), and a disagreement among them
//!   makes the item among the least certain.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::campaign::{self, Error, Question, Worth};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Row, Store};

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// One suggestion as a source gave it, before it is matched to an item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Given {
    /// The campaign item, where the source names it.
    pub item: Option<i64>,
    /// The stack, where the source names it.
    pub stack: Option<i64>,
    /// The series, by its SeriesInstanceUID, as v0's exports name it: every
    /// stack of the series takes the suggestion.
    pub series: Option<String>,
    /// The value of the question's one axis (or a whole answer), in any
    /// name the pack gives it; where none is given, the class of the
    /// highest confidence.
    pub value: Option<String>,
    /// A confidence per class, in any name the pack gives each.
    pub confidences: BTreeMap<String, f64>,
    /// Who suggested it, where the row says; else the import's author.
    pub author: Option<String>,
}

/// Read a tab-separated file of suggestions. The first line names the
/// columns, in any case: `item` (or `item_id`), `stack` (or `stack_id`),
/// `SeriesInstanceUID` (or `series_instance_uid`), `value`, `author`, and
/// one `p:<value>` per class for the confidences. At least one of the three
/// that name what is suggested for, and a value or a confidence. Other
/// columns (v0's date) are read past. Answers the rows and how many lines
/// were not one.
pub fn parse_tsv(text: &str) -> Result<(Vec<Given>, i64), Error> {
    let mut lines = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines
        .next()
        .ok_or_else(|| invalid("the file is empty"))?
        .split('\t')
        .map(|c| c.trim().to_string())
        .collect();
    let at = |names: &[&str]| -> Option<usize> {
        head.iter()
            .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
    };
    let item = at(&["item", "item_id"]);
    let stack = at(&["stack", "stack_id"]);
    let series = at(&["seriesinstanceuid", "series_instance_uid"]);
    let value = at(&["value"]);
    let author = at(&["author"]);
    let classes: Vec<(usize, String)> = head
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            h.strip_prefix("p:")
                .or_else(|| h.strip_prefix("P:"))
                .map(|c| (i, c.trim().to_string()))
        })
        .filter(|(_, c)| !c.is_empty())
        .collect();
    if item.is_none() && stack.is_none() && series.is_none() {
        return Err(invalid(
            "the first line names the columns, and one of them is item, stack_id or SeriesInstanceUID",
        ));
    }
    if value.is_none() && classes.is_empty() {
        return Err(invalid(
            "the first line names a value column or a p:<value> column for each class",
        ));
    }
    let mut out = Vec::new();
    let mut bad = 0;
    for line in lines {
        let cells: Vec<&str> = line.split('\t').map(str::trim).collect();
        let cell = |i: Option<usize>| -> Option<&str> {
            i.and_then(|i| cells.get(i).copied())
                .filter(|c| !c.is_empty())
        };
        let mut g = Given {
            item: None,
            stack: None,
            series: cell(series).map(str::to_string),
            value: cell(value).map(str::to_string),
            confidences: BTreeMap::new(),
            author: cell(author).map(str::to_string),
        };
        let mut ok = true;
        for (i, into) in [(item, &mut g.item), (stack, &mut g.stack)] {
            if let Some(t) = cell(i) {
                match t.parse::<i64>() {
                    Ok(n) => *into = Some(n),
                    Err(_) => ok = false,
                }
            }
        }
        for (i, class) in &classes {
            if let Some(t) = cell(Some(*i)) {
                match t.parse::<f64>() {
                    Ok(p) => {
                        g.confidences.insert(class.clone(), p);
                    }
                    Err(_) => ok = false,
                }
            }
        }
        let names_one = g.item.is_some() || g.stack.is_some() || g.series.is_some();
        let says = g.value.is_some() || !g.confidences.is_empty();
        if ok && names_one && says {
            out.push(g);
        } else {
            bad += 1;
        }
    }
    Ok((out, bad))
}

/// An import of suggestions into one campaign.
#[derive(Debug, Clone)]
pub struct Import<'a> {
    pub campaign: i64,
    pub rows: &'a [Given],
    /// The author of every row that names none.
    pub author: Option<&'a str>,
    /// What the suggestions came from, as the importer names it.
    pub source: Option<&'a str>,
    /// Who imported them.
    pub who: &'a str,
    /// Every name a value goes by, in lower case, to its identity; empty
    /// takes values as they are written.
    pub names: &'a BTreeMap<String, String>,
    pub dry_run: bool,
}

/// What an import did, as counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Imported {
    pub rows: i64,
    /// Suggestions written (or that would be, on a dry run).
    pub suggestions: i64,
    /// Of those, the ones that replaced the same author's earlier one.
    pub replaced: i64,
    /// Rows that named nothing this campaign asks.
    pub unmatched: i64,
    /// Suggestions for a stack of a sample sealed now, never kept.
    pub sealed: i64,
    /// Rows whose value, or a class of whose confidences, the question does
    /// not take, or whose confidence is not between 0 and 1.
    pub refused_values: i64,
    /// Rows with no author, where the import names none either.
    pub no_author: i64,
    pub authors: BTreeMap<String, i64>,
}

impl Imported {
    pub fn as_json(&self) -> Value {
        json!({
            "rows": self.rows, "suggestions": self.suggestions, "replaced": self.replaced,
            "unmatched": self.unmatched, "sealed": self.sealed,
            "refused_values": self.refused_values, "no_author": self.no_author,
            "authors": self.authors,
        })
    }
}

/// An author is a word: letters, digits and `._:@/+-`, a hundred at most.
fn check_author(a: &str) -> Result<String, Error> {
    let a = a.trim();
    if a.is_empty()
        || a.len() > 100
        || !a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:@/+-".contains(c))
    {
        return Err(invalid(format!(
            "{a:?} is not an author: a word such as v0-model, v0-person or a model id"
        )));
    }
    Ok(a.to_string())
}

/// A value in the pack's name for it.
fn named(names: &BTreeMap<String, String>, v: &str) -> String {
    let v = v.trim();
    names
        .get(&v.to_lowercase())
        .cloned()
        .unwrap_or_else(|| v.to_string())
}

/// The stacks of each series named, by SeriesInstanceUID.
fn stacks_of_series(
    store: &mut Store,
    uids: &[&str],
) -> Result<BTreeMap<String, Vec<i64>>, StoreError> {
    let d = store.dialect();
    let mut out: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for chunk in uids.chunks(500) {
        let marks: Vec<String> = (1..=chunk.len()).map(|i| d.param(i, Type::Text)).collect();
        let sql = format!(
            "SELECT r.series_instance_uid, k.id FROM {} k JOIN {} r ON r.id = k.series_id \
             WHERE r.series_instance_uid IN ({}) ORDER BY k.id",
            store.qualified("stack"),
            store.qualified("series"),
            marks.join(", ")
        );
        let params: Vec<Param> = chunk.iter().map(|u| Param::from(*u)).collect();
        for r in store.query(&sql, &params)? {
            out.entry(r.text(0)?.to_string())
                .or_default()
                .push(r.int(1)?);
        }
    }
    Ok(out)
}

/// One suggestion ready to write.
struct Ready {
    item: i64,
    stack: Option<i64>,
    value: String,
    author: String,
    confidences: Option<BTreeMap<String, f64>>,
    confidence: Option<f64>,
}

/// Import suggestions into an open campaign that asks an axis or an axes
/// question (record 50 R3). Each row is matched to the campaign's items by
/// its item, its stack or the stacks of its series, and its value is held
/// to the question as an answer would be: a single-axis question takes the
/// axis's value, any other axes question a whole answer. A row with
/// confidences and no value suggests the class of the highest. A stack of
/// a sample sealed now takes none. One transaction: every suggestion is
/// written or none is; with `dry_run` nothing is.
pub fn import(registry: &mut Registry, im: &Import<'_>, now: &str) -> Result<Imported, Error> {
    let c = campaign::get(registry.store(), im.campaign)?
        .ok_or_else(|| Error::NotFound(format!("no campaign {}", im.campaign)))?;
    if c.status != "open" {
        return Err(Error::Refused(format!(
            "campaign {} is {}; suggestions go to an open campaign",
            c.name, c.status
        )));
    }
    let question = c.question()?;
    if !matches!(question, Question::Axis { .. } | Question::Axes { .. }) {
        return Err(invalid(format!(
            "campaign {} asks a {} question; suggestions are of axis and axes questions",
            c.name,
            question.kind()
        )));
    }
    let default_author = im.author.map(check_author).transpose()?;
    let single = campaign::single_axis(&question);
    let vocabulary: BTreeSet<String> = match &question {
        Question::Axis { values, .. } => values.iter().cloned().collect(),
        Question::Axes { constraints, .. } => single
            .as_ref()
            .map(|a| {
                constraints["values"][a.as_str()]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    };
    let store = registry.store();
    let items = campaign::items(store, c.id)?;
    let mut by_stack: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    let mut stack_of: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    for it in &items {
        stack_of.insert(it.id, it.stack_id);
        if let Some(s) = it.stack_id {
            by_stack.entry(s).or_default().push(it.id);
        }
    }
    let uids: Vec<&str> = im
        .rows
        .iter()
        .filter_map(|g| g.series.as_deref())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let series = stacks_of_series(store, &uids)?;
    let all_stacks: Vec<i64> = by_stack.keys().copied().collect();
    let (sealed, _) = crate::labels::sealed_now(store, &all_stacks, &[])
        .map_err(|e| Error::Store(StoreError::Message(e.to_string())))?;

    let mut out = Imported {
        rows: im.rows.len() as i64,
        ..Imported::default()
    };
    let mut ready: BTreeMap<(i64, String), Ready> = BTreeMap::new();
    for g in im.rows {
        let author = match g.author.as_deref() {
            Some(a) => check_author(a)?,
            None => match &default_author {
                Some(a) => a.clone(),
                None => {
                    out.no_author += 1;
                    continue;
                }
            },
        };
        // the confidences in the pack's names, each a probability
        let mut confidences: BTreeMap<String, f64> = BTreeMap::new();
        let mut fits = true;
        for (class, p) in &g.confidences {
            let class = named(im.names, class);
            if !p.is_finite() || !(0.0..=1.0).contains(p) {
                fits = false;
            }
            if !vocabulary.is_empty() && !vocabulary.contains(&class) {
                fits = false;
            }
            confidences.insert(class, *p);
        }
        let value = match &g.value {
            Some(v) if single.is_some() => named(im.names, v),
            Some(v) => v.trim().to_string(),
            None => confidences
                .iter()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(c, _)| c.clone())
                .unwrap_or_default(),
        };
        // can't tell is a rater's answer, never a suggestion
        if value == campaign::CANT_TELL || confidences.contains_key(campaign::CANT_TELL) {
            out.refused_values += 1;
            continue;
        }
        let answer = campaign::single_answer(&question, &value).unwrap_or_else(|| value.clone());
        let kept = campaign::kept_value(&question, &answer).ok();
        let checks = question.check_value(&answer).is_ok();
        let Some(kept) = kept.filter(|_| fits && checks && !value.is_empty()) else {
            out.refused_values += 1;
            continue;
        };
        let confidence = confidences.get(&value).copied();
        // the items it is for
        let mut targets: BTreeSet<i64> = BTreeSet::new();
        if let Some(i) = g.item
            && stack_of.contains_key(&i)
        {
            targets.insert(i);
        }
        if let Some(s) = g.stack {
            targets.extend(by_stack.get(&s).into_iter().flatten());
        }
        if let Some(u) = g.series.as_deref() {
            for s in series.get(u).into_iter().flatten() {
                targets.extend(by_stack.get(s).into_iter().flatten());
            }
        }
        if targets.is_empty() {
            out.unmatched += 1;
            continue;
        }
        for item in targets {
            let stack = stack_of.get(&item).copied().flatten();
            if stack.is_some_and(|s| sealed.contains(&s)) {
                out.sealed += 1;
                continue;
            }
            ready.insert(
                (item, author.clone()),
                Ready {
                    item,
                    stack,
                    value: kept.clone(),
                    author: author.clone(),
                    confidences: (!confidences.is_empty()).then(|| confidences.clone()),
                    confidence,
                },
            );
        }
    }
    // which of these replace an author's earlier suggestion
    let held: BTreeSet<(i64, String)> = of_campaign(store, c.id)?
        .into_iter()
        .map(|s| (s.item_id, s.author))
        .collect();
    for (key, r) in &ready {
        out.suggestions += 1;
        *out.authors.entry(r.author.clone()).or_insert(0) += 1;
        if held.contains(key) {
            out.replaced += 1;
        }
    }
    if im.dry_run || ready.is_empty() {
        return Ok(out);
    }
    let d = store.dialect();
    store.begin()?;
    let written = (|| -> Result<(), Error> {
        for chunk in ready.values().collect::<Vec<_>>().chunks(200) {
            for r in chunk {
                store.execute(
                    &format!(
                        "DELETE FROM {} WHERE item_id = {} AND author = {}",
                        store.qualified("campaign_suggestion"),
                        d.param(1, Type::Int),
                        d.param(2, Type::Text)
                    ),
                    &[Param::Int(r.item), Param::from(r.author.as_str())],
                )?;
            }
            let rows: Vec<Vec<Param>> = chunk
                .iter()
                .map(|r| {
                    vec![
                        Param::Int(c.id),
                        Param::Int(r.item),
                        r.stack.map_or(Param::Null, Param::Int),
                        Param::from(r.value.as_str()),
                        Param::from(r.author.as_str()),
                        r.confidences
                            .as_ref()
                            .map_or(Param::Null, |m| Param::from(json!(m).to_string())),
                        r.confidence.map_or(Param::Null, Param::Double),
                        im.source.map_or(Param::Null, Param::from),
                        Param::from(im.who),
                        Param::from(now),
                    ]
                })
                .collect();
            store.insert(
                &Insert::new(
                    table("campaign_suggestion"),
                    &[
                        "campaign_id",
                        "item_id",
                        "stack_id",
                        "value",
                        "author",
                        "confidences",
                        "confidence",
                        "source",
                        "imported_by",
                        "imported_at",
                    ],
                ),
                &rows,
            )?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        store.rollback().ok();
        return Err(e);
    }
    store.commit()?;
    audit::record(
        registry,
        &Entry {
            principal: im.who,
            action: Action::CampaignSuggest,
            scope: json!({"campaign": c.id, "name": c.name, "suggestions": out.suggestions}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "source": im.source, "authors": out.authors, "replaced": out.replaced,
                "unmatched": out.unmatched, "sealed": out.sealed,
                "refused_values": out.refused_values,
            })),
        },
    )?;
    Ok(out)
}

/// A suggestion as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub id: i64,
    pub campaign_id: i64,
    pub item_id: i64,
    pub stack_id: Option<i64>,
    /// As an answer to the question keeps its value.
    pub value: String,
    pub author: String,
    /// `{value: p}` per class, or null.
    pub confidences: Value,
    pub confidence: Option<f64>,
    pub source: Option<String>,
    pub imported_by: String,
    pub imported_at: String,
}

impl Suggestion {
    pub fn as_json(&self) -> Value {
        json!({
            "id": self.id, "item": self.item_id, "stack": self.stack_id, "value": self.value,
            "author": self.author, "confidences": self.confidences,
            "confidence": self.confidence, "source": self.source,
            "imported_by": self.imported_by, "imported_at": self.imported_at,
        })
    }
}

const COLUMNS: [&str; 11] = [
    "id",
    "campaign_id",
    "item_id",
    "stack_id",
    "value",
    "author",
    "confidences",
    "confidence",
    "source",
    "imported_by",
    "imported_at",
];

fn suggestion_of(r: &Row) -> Result<Suggestion, StoreError> {
    Ok(Suggestion {
        id: r.int(0)?,
        campaign_id: r.int(1)?,
        item_id: r.int(2)?,
        stack_id: r.opt_int(3)?,
        value: r.text(4)?.to_string(),
        author: r.text(5)?.to_string(),
        confidences: r
            .opt_text(6)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null),
        confidence: r.opt_double(7)?,
        source: r.opt_text(8)?.map(str::to_string),
        imported_by: r.text(9)?.to_string(),
        imported_at: r.text(10)?.to_string(),
    })
}

fn select(store: &Store) -> String {
    let d = store.dialect();
    let t = table("campaign_suggestion");
    COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a declared column")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every suggestion a campaign carries, oldest first.
pub fn of_campaign(store: &mut Store, campaign: i64) -> Result<Vec<Suggestion>, Error> {
    let sql = format!(
        "SELECT {} FROM {} WHERE campaign_id = {} ORDER BY id",
        select(store),
        store.qualified("campaign_suggestion"),
        store.dialect().param(1, Type::Int)
    );
    Ok(store
        .query(&sql, &[Param::Int(campaign)])?
        .iter()
        .map(suggestion_of)
        .collect::<Result<_, _>>()?)
}

/// The suggestions of some items of a campaign, by item, the latest import
/// first.
pub fn of_items(
    store: &mut Store,
    campaign: i64,
    items: &[i64],
) -> Result<BTreeMap<i64, Vec<Suggestion>>, Error> {
    let mut out: BTreeMap<i64, Vec<Suggestion>> = BTreeMap::new();
    for chunk in items.chunks(500) {
        let ids = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {} FROM {} WHERE campaign_id = {} AND item_id IN ({ids}) ORDER BY id DESC",
            select(store),
            store.qualified("campaign_suggestion"),
            store.dialect().param(1, Type::Int)
        );
        for r in store.query(&sql, &[Param::Int(campaign)])? {
            let s = suggestion_of(&r)?;
            out.entry(s.item_id).or_default().push(s);
        }
    }
    Ok(out)
}

/// The suggestion shown first of an item's: the latest import's.
pub fn primary(list: &[Suggestion]) -> Option<&Suggestion> {
    list.iter().max_by_key(|s| s.id)
}

/// Whether the authors that suggested for an item said different values.
pub fn disagree(list: &[Suggestion]) -> bool {
    list.iter().map(|s| &s.value).collect::<BTreeSet<_>>().len() > 1
}

/// What an item's suggestions say of how sure it is: the confidence of
/// the one shown first (none where its source gave none), and whether the
/// authors disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    pub confidence: Option<f64>,
    pub disagree: bool,
}

/// [`Told`] for the items of a campaign that carry suggestions.
pub fn worth_of_items(
    store: &mut Store,
    campaign: i64,
    items: &[i64],
) -> Result<BTreeMap<i64, Told>, StoreError> {
    let found = of_items(store, campaign, items).map_err(|e| match e {
        Error::Store(s) => s,
        other => StoreError::Message(other.to_string()),
    })?;
    Ok(found
        .into_iter()
        .map(|(item, list)| {
            (
                item,
                Told {
                    confidence: primary(&list).and_then(|s| s.confidence),
                    disagree: disagree(&list),
                },
            )
        })
        .collect())
}

/// The worth of an item (record 48 R1) with what its suggestions say
/// (record 50 R7): a suggestion's confidence is how sure the item is where
/// its source gave one, and authors that disagree make it a disagreement.
pub fn merged(mine: Option<Worth>, told: Option<&Told>) -> Option<Worth> {
    let Some(t) = told else {
        return mine;
    };
    let mut w = mine.unwrap_or(Worth {
        disagree: false,
        confidence: 1.0,
        asked: false,
    });
    if let Some(c) = t.confidence {
        w.confidence = c;
    }
    w.disagree = w.disagree || t.disagree;
    Some(w)
}

/// What a campaign's suggestions hold: how many, of how many items, and
/// per author how many of each value.
pub fn summary(store: &mut Store, campaign: i64) -> Result<Value, Error> {
    let all = of_campaign(store, campaign)?;
    let items: BTreeSet<i64> = all.iter().map(|s| s.item_id).collect();
    let mut authors: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for s in &all {
        *authors
            .entry(s.author.clone())
            .or_default()
            .entry(s.value.clone())
            .or_insert(0) += 1;
    }
    Ok(json!({
        "count": all.len(),
        "items": items.len(),
        "authors": authors.iter().map(|(a, values)| json!({
            "author": a, "count": values.values().sum::<i64>(), "values": values,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_names_its_columns_and_reads_confidences_per_class() {
        let (rows, bad) = parse_tsv(
            "stack_id\tvalue\tauthor\tp:brain\tp:Brain-Neck\n12\tbrain\tv0-model\t0.9\t0.1\n13\t\t\t0.2\t0.8\nx\tbrain\n\n",
        )
        .unwrap();
        assert_eq!(bad, 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].stack, Some(12));
        assert_eq!(rows[0].author.as_deref(), Some("v0-model"));
        assert_eq!(rows[1].value, None);
        assert_eq!(rows[1].confidences["Brain-Neck"], 0.8);
    }

    #[test]
    fn a_v0_export_reads_by_its_series() {
        let (rows, bad) =
            parse_tsv("SeriesInstanceUID\tvalue\tdate\n1.2.3\tBrain\t2025-03-01\n").unwrap();
        assert_eq!(bad, 0);
        assert_eq!(rows[0].series.as_deref(), Some("1.2.3"));
        assert_eq!(rows[0].value.as_deref(), Some("Brain"));
        assert!(parse_tsv("1.2.3\tBrain\n").is_err());
        assert!(parse_tsv("stack_id\tdate\n1\tx\n").is_err());
    }

    #[test]
    fn an_author_is_a_word() {
        assert!(check_author("v0-model").is_ok());
        assert!(check_author("bodypart@3:sha256").is_ok());
        assert!(check_author("two words").is_err());
        assert!(check_author("").is_err());
    }
}
