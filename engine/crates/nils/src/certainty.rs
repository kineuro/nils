// SPDX-License-Identifier: AGPL-3.0-only

//! How sure a sort is of a dataset (record 55 H2, round 4): the card's line
//! "120 scans · 112 sure · 8 need a look [Review 8]". A dataset's stacks
//! are the ones its digests created first, as the sources door counts
//! them; a stack is sure once it is sorted and needs no look. Every number
//! here is a count, and the Review button opens the items about the
//! dataset through `dataset=` on the review doors.
//!
//! **Need a look, one definition** (record 55 H3 and record 56,
//! 2026-10-09). A scan needs a look while a question the sort itself asks
//! about it waits for a person, open or staged: the sort's own questions,
//! and no other. [`asker`] says who asks a question and [`Asks`] reads the
//! waiting questions of one asker. Every count of a look reads them through
//! [`Asks::sort`], so the numbers agree wherever they are shown: the
//! sources door's totals (`to_sort`, `sure`, `need_a_look`) and each recent
//! digest's `to_sort`, which the card reads; the Data page's summary
//! (`need_a_look`, `sure`, `look_kinds` and the sorted step's `look`) and a
//! cohort's sorted step; the kinds the scans doors give each scan
//! (`questions`); and the Grid's `look` on subjects and visits, with its
//! filter and its order.
//!
//! Who asks what:
//!
//! - **the sort**: an axis that matters with no answer (`<axis>:missing`,
//!   all a sort raises by itself since record 55 H3), a person's own
//!   threshold (`<axis>:low_confidence`), a person's decision the rules
//!   disagree with (`<axis>:decision`), what an engine before record 55 H3
//!   asked of its rules (`<axis>:conflict`), a constraint of the pack the
//!   rules' answer breaks (`classify.excluded`, `classify.implied`) and
//!   System 1's question (`classify.asked`); never one about an axis an
//!   operation owns;
//! - **an operation of its own** (record 56: body part, post-contrast):
//!   every question about an axis it owns, from its model's run, a pass or
//!   a sort before record 56, counted as that step's `look`;
//! - **a pass** of the sort (a neighbour vote, `<axis>:vote`, or a session
//!   pass, `<axis>:session`) about another axis, counted as the sorted
//!   step's `passes`, beside its `look` and never in it;
//! - **a model's run** about another axis: its group (`<axis>:model`) and
//!   its disagreement with a person's decision (an `<axis>:decision` whose
//!   evidence names the model as its source), Review's alone;
//! - **a pick run**: its borders (`pick.border`), counted under the main
//!   scans;
//! - anything else (an identity, a read, a release, a pipeline's check),
//!   Review's alone.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nils_registry::place::Place;
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::Value;

/// The statuses of a question that still waits for a person: open, or
/// staged and not yet committed.
pub(crate) const WAITING: &str = "'open', 'staged'";

/// The questions a sort raises where the rules' answer breaks one of the
/// pack's constraints (record 48), one per broken constraint.
const BROKEN: [&str; 2] = ["classify.excluded", "classify.implied"];

fn ids_in(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("'{}'", w.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Who asks a question, and so where it is counted (the module's
/// documentation says where each is).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asker {
    /// The sort: its questions alone make a scan need a look.
    Sort,
    /// An operation of its own, by the name of its step.
    Operation(&'static str),
    /// A pass of the sort, about an axis no operation owns.
    Pass,
    /// A model's run, about an axis no operation owns.
    Model,
    /// A pick run's border.
    Picks,
    /// Anything else: an identity, a read, a release, a pipeline's check.
    Other,
}

/// Who asks a question of this kind. `by_model` says an `<axis>:decision`
/// came from a model's run (its evidence's `source`), which the kind alone
/// does not tell.
pub(crate) fn asker(kind: &str, by_model: bool) -> Asker {
    if kind == nils_registry::review::PICK_BORDER_KIND {
        return Asker::Picks;
    }
    if kind == nils_registry::asked::KIND || BROKEN.contains(&kind) {
        return Asker::Sort;
    }
    let Some((axis, why)) = kind.split_once(':') else {
        return Asker::Other;
    };
    if let Some(step) = crate::operations::owner(axis) {
        return Asker::Operation(step);
    }
    match why {
        "missing" | "low_confidence" | "conflict" => Asker::Sort,
        "decision" if by_model => Asker::Model,
        "decision" => Asker::Sort,
        "vote" | "session" => Asker::Pass,
        "model" => Asker::Model,
        _ => Asker::Other,
    }
}

/// The scans some questions ask about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Asked {
    /// The scans one of the questions asks about.
    pub(crate) scans: i64,
    /// The scans by the kind of the question. A scan asked two questions
    /// counts under each, so these may add up to more than `scans`.
    pub(crate) kinds: BTreeMap<String, i64>,
}

/// The questions of one asker that wait for a person, as every count of
/// them reads them: the condition that picks them out of `review_item`
/// (aliased `ri`), and the ones about one stack, each with its stack. A
/// grouped question asks about each of its members, a question about one
/// stack about the stack its reference names.
#[derive(Debug, Clone, Default)]
pub(crate) struct Asks {
    /// None where no question of the asker waits.
    cond: Option<String>,
    single: Vec<(String, i64)>,
}

impl Asks {
    /// The sort's own questions that wait for a person: what makes a scan
    /// need a look, everywhere.
    pub(crate) fn sort(store: &mut Store) -> Result<Asks, StoreError> {
        Asks::of(store, WAITING, &|a| a == Asker::Sort)
    }

    /// The questions of these statuses (a comma list of quoted words) whose
    /// asker `keep` keeps.
    pub(crate) fn of(
        store: &mut Store,
        statuses: &str,
        keep: &dyn Fn(Asker) -> bool,
    ) -> Result<Asks, StoreError> {
        let item = store.qualified("review_item");
        // the kinds that wait, each kept or not by its asker; a decision
        // whose asker its evidence decides is read item by item
        let mut kinds = Vec::new();
        let mut undecided = Vec::new();
        for r in store.query(
            &format!("SELECT DISTINCT ri.kind FROM {item} ri WHERE ri.status IN ({statuses})"),
            &[],
        )? {
            let kind = r.text(0)?.to_string();
            let (rules, model) = (keep(asker(&kind, false)), keep(asker(&kind, true)));
            if rules != model {
                undecided.push(kind.clone());
            }
            if rules {
                kinds.push(kind);
            }
        }
        let (mut dropped, mut added) = (Vec::new(), Vec::new());
        if !undecided.is_empty() {
            for r in store.query(
                &format!(
                    "SELECT ri.id, ri.kind, {} FROM {item} ri WHERE ri.status IN ({statuses}) \
                     AND ri.kind IN ({})",
                    crate::text_of(store, "review_item", "evidence"),
                    quoted(&undecided)
                ),
                &[],
            )? {
                let evidence: Value = r
                    .opt_text(2)?
                    .and_then(|t| serde_json::from_str(t).ok())
                    .unwrap_or(Value::Null);
                if evidence["source"] != "model" {
                    continue;
                }
                if keep(asker(r.text(1)?, false)) {
                    dropped.push(r.int(0)?);
                } else {
                    added.push(r.int(0)?);
                }
            }
        }
        let mut picks = Vec::new();
        if !kinds.is_empty() {
            let mut p = format!("ri.kind IN ({})", quoted(&kinds));
            if !dropped.is_empty() {
                p.push_str(&format!(" AND ri.id NOT IN ({})", ids_in(&dropped)));
            }
            picks.push(p);
        }
        if !added.is_empty() {
            picks.push(format!("ri.id IN ({})", ids_in(&added)));
        }
        if picks.is_empty() {
            return Ok(Asks::default());
        }
        let cond = format!("ri.status IN ({statuses}) AND ({})", picks.join(" OR "));
        // a question about one stack names it in its reference, which the
        // two backends spell apart as text: read and matched here, not in SQL
        let mut single = Vec::new();
        for r in store.query(
            &format!(
                "SELECT ri.kind, {} FROM {item} ri WHERE ri.scope = 'stack' AND {cond}",
                crate::text_of(store, "review_item", "ref"),
            ),
            &[],
        )? {
            let reference: Value = r
                .opt_text(1)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            if let Some(stack) = reference["stack_id"].as_i64() {
                single.push((r.text(0)?.to_string(), stack));
            }
        }
        Ok(Asks {
            cond: Some(cond),
            single,
        })
    }

    /// The scans of a scope these questions ask about, and how many by
    /// kind: `holds` is the condition a stack aliased `x` meets to be in it.
    pub(crate) fn count(&self, store: &mut Store, holds: &str) -> Result<Asked, StoreError> {
        Ok(self
            .counted(store, holds, None)?
            .remove(&0)
            .unwrap_or_default())
    }

    /// The same for the stacks each of `batches` created first, by batch.
    pub(crate) fn by_first_batch(
        &self,
        store: &mut Store,
        batches: &[i64],
    ) -> Result<BTreeMap<i64, Asked>, StoreError> {
        if batches.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.counted(
            store,
            &format!("x.first_batch_id IN ({})", ids_in(batches)),
            Some("x.first_batch_id"),
        )
    }

    /// The scans of a scope these questions ask about, by `key` (an
    /// integer of the stack `x`), or all under 0.
    fn counted(
        &self,
        store: &mut Store,
        holds: &str,
        key: Option<&str>,
    ) -> Result<BTreeMap<i64, Asked>, StoreError> {
        let mut out: BTreeMap<i64, Asked> = BTreeMap::new();
        let Some(cond) = &self.cond else {
            return Ok(out);
        };
        let (member, item, stack) = (
            store.qualified("review_member"),
            store.qualified("review_item"),
            store.qualified("stack"),
        );
        let pick = key.unwrap_or("0");
        let (by, by_kind) = match key {
            Some(k) => (format!(" GROUP BY {k}"), format!(" GROUP BY {k}, ri.kind")),
            None => (String::new(), " GROUP BY ri.kind".to_string()),
        };
        // the grouped questions, under each of their members in scope
        let joined = format!(
            "FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id WHERE {cond} AND {holds}"
        );
        for r in store.query(
            &format!("SELECT {pick}, COUNT(DISTINCT rm.stack_id) {joined}{by}"),
            &[],
        )? {
            out.entry(r.int(0)?).or_default().scans += r.int(1)?;
        }
        for r in store.query(
            &format!("SELECT {pick}, ri.kind, COUNT(DISTINCT rm.stack_id) {joined}{by_kind}"),
            &[],
        )? {
            *out.entry(r.int(0)?)
                .or_default()
                .kinds
                .entry(r.text(1)?.to_string())
                .or_insert(0) += r.int(2)?;
        }
        if self.single.is_empty() {
            return Ok(out);
        }
        // the questions about one stack: the stacks in scope, under their
        // key, and the kinds a grouped question counts them under already
        let named: Vec<i64> = self
            .single
            .iter()
            .map(|(_, s)| *s)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut inside: HashMap<i64, i64> = HashMap::new();
        let mut grouped: HashSet<(i64, String)> = HashSet::new();
        for chunk in named.chunks(500) {
            for r in store.query(
                &format!(
                    "SELECT x.id, {pick} FROM {stack} x WHERE x.id IN ({}) AND {holds}",
                    ids_in(chunk)
                ),
                &[],
            )? {
                inside.insert(r.int(0)?, r.int(1)?);
            }
            for r in store.query(
                &format!(
                    "SELECT DISTINCT rm.stack_id, ri.kind FROM {member} rm \
                     JOIN {item} ri ON ri.id = rm.item_id WHERE rm.stack_id IN ({}) AND {cond}",
                    ids_in(chunk)
                ),
                &[],
            )? {
                grouped.insert((r.int(0)?, r.text(1)?.to_string()));
            }
        }
        let held: HashSet<i64> = grouped.iter().map(|(s, _)| *s).collect();
        let (mut scans, mut kinds) = (HashSet::new(), HashSet::new());
        for (kind, s) in &self.single {
            let Some(k) = inside.get(s) else {
                continue;
            };
            let at = out.entry(*k).or_default();
            if !held.contains(s) && scans.insert(*s) {
                at.scans += 1;
            }
            let pair = (*s, kind.clone());
            if !grouped.contains(&pair) && kinds.insert(pair) {
                *at.kinds.entry(kind.clone()).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// The stacks of a scope these questions ask about: `holds` is the
    /// condition a stack aliased `x` meets to be in it.
    pub(crate) fn stacks_in(
        &self,
        store: &mut Store,
        holds: &str,
    ) -> Result<BTreeSet<i64>, StoreError> {
        let mut out = BTreeSet::new();
        let Some(cond) = &self.cond else {
            return Ok(out);
        };
        let (member, item, stack) = (
            store.qualified("review_member"),
            store.qualified("review_item"),
            store.qualified("stack"),
        );
        for r in store.query(
            &format!(
                "SELECT DISTINCT rm.stack_id FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
                 JOIN {stack} x ON x.id = rm.stack_id WHERE {cond} AND {holds}"
            ),
            &[],
        )? {
            out.insert(r.int(0)?);
        }
        let named: Vec<i64> = self
            .single
            .iter()
            .map(|(_, s)| *s)
            .filter(|s| !out.contains(s))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for chunk in named.chunks(500) {
            for r in store.query(
                &format!(
                    "SELECT x.id FROM {stack} x WHERE x.id IN ({}) AND {holds}",
                    ids_in(chunk)
                ),
                &[],
            )? {
                out.insert(r.int(0)?);
            }
        }
        Ok(out)
    }

    /// The kinds of these questions on each of `stacks`, sorted and each
    /// once: grouped questions through their members, and questions about
    /// one stack through their reference.
    pub(crate) fn of_stacks(
        &self,
        store: &mut Store,
        stacks: &[i64],
    ) -> Result<HashMap<i64, BTreeSet<String>>, StoreError> {
        let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
        let Some(cond) = &self.cond else {
            return Ok(out);
        };
        let (member, item) = (
            store.qualified("review_member"),
            store.qualified("review_item"),
        );
        for chunk in stacks.chunks(500) {
            for r in store.query(
                &format!(
                    "SELECT rm.stack_id, ri.kind FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
                     WHERE {cond} AND rm.stack_id IN ({})",
                    ids_in(chunk)
                ),
                &[],
            )? {
                out.entry(r.int(0)?)
                    .or_default()
                    .insert(r.text(1)?.to_string());
            }
        }
        let wanted: HashSet<i64> = stacks.iter().copied().collect();
        for (kind, s) in &self.single {
            if wanted.contains(s) {
                out.entry(*s).or_default().insert(kind.clone());
            }
        }
        Ok(out)
    }
}

/// The scans of a scope a pass's question waits on a person for, counted
/// as the sorted step's `passes` and never as a look.
pub(crate) fn passes(store: &mut Store, holds: &str) -> Result<i64, StoreError> {
    Ok(Asks::of(store, WAITING, &|a| a == Asker::Pass)?
        .count(store, holds)?
        .scans)
}

/// The stacks of `stacks` that the sources (a comma list of `source`
/// ids) hold a file of (record 55, 2026-10-10).
fn stacks_of_sources(
    store: &mut Store,
    stacks: &[i64],
    sources: &str,
) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    let held = crate::operations::held_by(store, "x", sources);
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT x.id FROM {} x WHERE x.id IN ({}) AND {held}",
            store.qualified("stack"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

/// What the sources door says of a dataset's certainty, and what the Data
/// page's summary says with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Certainty {
    /// The stacks that need a look: a question of the sort's waits on a
    /// person for each.
    pub(crate) to_sort: i64,
    /// Sorted stacks that need no look.
    pub(crate) sure: i64,
    /// Stacks no sort has judged yet.
    pub(crate) unsorted: i64,
    /// The stacks that need a look, by the kind of the sort's question
    /// (`base:missing`, `classify.asked`, ...). A stack asked two questions
    /// counts under each, so these may add up to more than `to_sort`.
    pub(crate) need_a_look: BTreeMap<String, i64>,
    /// Record 55 H3 (2026-10-09): what the sort noted that is information
    /// and not a question, by kind: the split note (a series split into
    /// stacks of one image each), on the sorted stacks it holds for. Never
    /// counted in `to_sort` or `need_a_look`.
    pub(crate) noted: BTreeMap<String, i64>,
}

/// The certainty of the stacks the sources (a comma list of `source` ids)
/// hold a file of, `stacks` of them in all, with the sort's questions read
/// once by the caller ([`Asks::sort`]).
pub(crate) fn of_sources(
    store: &mut Store,
    asks: &Asks,
    sources: &str,
    stacks: i64,
) -> Result<Certainty, StoreError> {
    let holds = crate::operations::Scope::Sources(sources).holds(store);
    let look = asks.count(store, &holds)?;
    let q = |t: &str| store.qualified(t);
    let (stack, class) = (q("stack"), q("classification"));
    let of_source = format!("WHERE {holds}");
    let sorted = store.query(
        &format!(
            "SELECT COUNT(*) FROM {stack} x {of_source} \
             AND EXISTS (SELECT 1 FROM {class} cl WHERE cl.stack_id = x.id)"
        ),
        &[],
    )?[0]
        .int(0)?;
    // the split note, by the one test the sort writes it by, over the
    // fingerprints of the sorted stacks of a split series
    let mut split = 0i64;
    for r in store.query(
        &format!(
            "SELECT f.stacks_in_series, f.n_instances FROM {stack} x \
             JOIN {} f ON f.stack_id = x.id {of_source} \
             AND f.split_reason IS NOT NULL AND f.split_reason <> '' \
             AND EXISTS (SELECT 1 FROM {class} cl WHERE cl.stack_id = x.id)",
            store.qualified("stack_fingerprint")
        ),
        &[],
    )? {
        let n =
            |i: usize| -> Result<Option<f64>, StoreError> { Ok(r.opt_int(i)?.map(|v| v as f64)) };
        if let (Some(stacks_in_series), Some(images)) = (n(0)?, n(1)?)
            && nils_classify::classify::is_split_note(stacks_in_series, images)
        {
            split += 1;
        }
    }
    let mut noted = BTreeMap::new();
    if split > 0 {
        noted.insert(nils_classify::classify::SPLIT_NOTE.to_string(), split);
    }
    Ok(Certainty {
        to_sort: look.scans,
        sure: (sorted - look.scans).max(0),
        unsorted: (stacks - sorted).max(0),
        need_a_look: look.kinds,
        noted,
    })
}

/// The review items about a dataset (record 55 H2, round 4: the card's
/// Review button), of one status or of every status: an item with a
/// member stack the dataset holds a file of; an item about one
/// stack, or one series, of the dataset; an item about a batch of the
/// dataset; and an item about a subject alone (a pick border, an unmapped
/// or provisional subject) whose subject has a stack of the dataset.
pub(crate) fn items_of(
    store: &mut Store,
    place: &Place,
    status: Option<&str>,
) -> Result<HashSet<i64>, StoreError> {
    let ids = crate::sources::source_ids(store, place)?;
    let mut out = HashSet::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let sources = ids_in(&ids);
    let d = store.dialect();
    let (filter, params): (String, Vec<Param>) = match status {
        Some(s) => (
            format!(" AND ri.status = {}", d.param(1, Type::Text)),
            vec![Param::from(s)],
        ),
        None => (String::new(), Vec::new()),
    };
    let q = |t: &str| store.qualified(t);
    let (stack, batch, member, item, series) = (
        q("stack"),
        q("ingest_batch"),
        q("review_member"),
        q("review_item"),
        q("series"),
    );
    let held = crate::operations::held_by(store, "x", &sources);
    for r in store.query(
        &format!(
            "SELECT DISTINCT rm.item_id FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id WHERE {held}{filter}"
        ),
        &params,
    )? {
        out.insert(r.int(0)?);
    }
    // the items that are no group, by what their `ref` names
    let rows = store.query(
        &format!(
            "SELECT ri.id, {} FROM {item} ri WHERE ri.scope <> 'group'{filter}",
            crate::text_of(store, "review_item", "ref"),
        ),
        &params,
    )?;
    let mut named: Vec<(i64, Value)> = Vec::with_capacity(rows.len());
    let (mut stacks, mut serieses, mut batches) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut subjects_asked = false;
    for r in &rows {
        let reference: Value = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if let Some(s) = reference["stack_id"].as_i64() {
            stacks.insert(s);
        } else if let Some(s) = reference["series_id"].as_i64() {
            serieses.insert(s);
        } else if let Some(b) = reference["batch_id"].as_i64() {
            batches.insert(b);
        } else if reference["subject_id"].is_i64() {
            subjects_asked = true;
        }
        named.push((r.int(0)?, reference));
    }
    let stacks = stacks_of_sources(store, &stacks.into_iter().collect::<Vec<_>>(), &sources)?;
    let mut series_in: BTreeSet<i64> = BTreeSet::new();
    let serieses: Vec<i64> = serieses.into_iter().collect();
    for chunk in serieses.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT x.series_id FROM {stack} x WHERE x.series_id IN ({}) AND {held}",
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            series_in.insert(r.int(0)?);
        }
    }
    let mut batches_in: BTreeSet<i64> = BTreeSet::new();
    let batches: Vec<i64> = batches.into_iter().collect();
    for chunk in batches.chunks(500) {
        let sql = format!(
            "SELECT id FROM {batch} WHERE id IN ({}) AND source_id IN ({sources})",
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            batches_in.insert(r.int(0)?);
        }
    }
    let mut subjects_in: BTreeSet<i64> = BTreeSet::new();
    if subjects_asked {
        let sql = format!(
            "SELECT DISTINCT se.subject_id FROM {stack} x JOIN {series} se ON se.id = x.series_id \
             WHERE {held}"
        );
        for r in store.query(&sql, &[])? {
            if let Some(s) = r.opt_int(0)? {
                subjects_in.insert(s);
            }
        }
    }
    for (id, reference) in named {
        let about = if let Some(s) = reference["stack_id"].as_i64() {
            stacks.contains(&s)
        } else if let Some(s) = reference["series_id"].as_i64() {
            series_in.contains(&s)
        } else if let Some(b) = reference["batch_id"].as_i64() {
            batches_in.contains(&b)
        } else if let Some(s) = reference["subject_id"].as_i64() {
            subjects_in.contains(&s)
        } else {
            false
        };
        if about {
            out.insert(id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{Asker, asker, ids_in};

    #[test]
    fn an_id_list_is_the_ids_joined() {
        assert_eq!(ids_in(&[1, 2]), "1, 2");
    }

    #[test]
    fn a_look_is_the_sort_s_own_question_and_every_other_is_counted_where_it_belongs() {
        for kind in [
            "base:missing",
            "technique:low_confidence",
            "modifier:conflict",
            "base:decision",
            "classify.asked",
            "classify.excluded",
            "classify.implied",
        ] {
            assert_eq!(asker(kind, false), Asker::Sort, "{kind}");
        }
        // a model's disagreement with a person's decision is the model's
        assert_eq!(asker("base:decision", true), Asker::Model);
        assert_eq!(asker("base:model", false), Asker::Model);
        // an operation owns every question about its axes, whoever asks it
        for (kind, step) in [
            ("body_part:low_confidence", "body_part"),
            ("body_part:missing", "body_part"),
            ("body_region:conflict", "body_part"),
            ("body_part:model", "body_part"),
            ("body_part:vote", "body_part"),
            ("post_contrast:missing", "post_contrast"),
            ("post_contrast:session", "post_contrast"),
            ("post_contrast:decision", "post_contrast"),
        ] {
            assert_eq!(asker(kind, false), Asker::Operation(step), "{kind}");
        }
        assert_eq!(
            asker("body_part:decision", true),
            Asker::Operation("body_part")
        );
        // a pass's question about another axis is the pass's
        assert_eq!(asker("base:vote", false), Asker::Pass);
        assert_eq!(asker("modifier:session", false), Asker::Pass);
        assert_eq!(asker("pick.border", false), Asker::Picks);
        for kind in [
            "identity.unmapped",
            "identity.provisional",
            "ingest.quarantine",
            "pipeline:qc",
            "release.no_task",
            "split:one_image_per_stack",
            "session.moved",
            "system1:unsure",
        ] {
            assert_eq!(asker(kind, false), Asker::Other, "{kind}");
        }
    }
}
