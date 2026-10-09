// SPDX-License-Identifier: AGPL-3.0-only

//! How sure a sort is of a dataset (record 55 H2, round 4): the card's line
//! "120 scans · 112 sure · 8 need a look [Review 8]". A dataset's stacks
//! are the ones its digests created first, as the sources door counts
//! them. A stack needs a look while an open or staged review item asks
//! about it, as a member of a grouped question or as the stack a
//! stack-scoped question names (System 1's); it is sure once it is sorted
//! and no such item asks about it. Every number here is a count, and the
//! Review button opens the same items through `dataset=` on the review
//! doors.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use nils_registry::place::Place;
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::Value;

/// The statuses that still wait for a person.
const OPEN: &str = "ri.status IN ('open', 'staged')";

fn ids_in(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The stacks of `stacks` that the sources (a comma list of `source`
/// ids) created first.
fn stacks_of_sources(
    store: &mut Store,
    stacks: &[i64],
    sources: &str,
) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT x.id FROM {} x JOIN {} b ON b.id = x.first_batch_id \
             WHERE x.id IN ({}) AND b.source_id IN ({sources})",
            store.qualified("stack"),
            store.qualified("ingest_batch"),
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

/// The open or staged questions about one stack each, which name their
/// stack in `ref` and have no member rows: (kind, stack).
fn stack_scoped(store: &mut Store) -> Result<Vec<(String, i64)>, StoreError> {
    let sql = format!(
        "SELECT ri.kind, {} FROM {} ri WHERE ri.scope = 'stack' AND {OPEN}",
        crate::text_of(store, "review_item", "ref"),
        store.qualified("review_item")
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let reference: Value = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if let Some(stack) = reference["stack_id"].as_i64() {
            out.push((r.text(0)?.to_string(), stack));
        }
    }
    Ok(out)
}

/// What the sources door says of a dataset's certainty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Certainty {
    /// Stacks an open or staged review item asks about.
    pub(crate) to_sort: i64,
    /// Sorted stacks no open or staged review item asks about.
    pub(crate) sure: i64,
    /// Stacks no sort has judged yet.
    pub(crate) unsorted: i64,
    /// The stacks that need a look, by the kind of the question
    /// (`body_part:low_confidence`, `axis_conflict`, ...). A stack asked
    /// two questions counts under each, so these may add up to more than
    /// `to_sort`.
    pub(crate) need_a_look: BTreeMap<String, i64>,
    /// Record 55 H3 (2026-10-09): what the sort noted that is information
    /// and not a question, by kind: the split note (a series split into
    /// stacks of one image each), on the sorted stacks it holds for. Never
    /// counted in `to_sort` or `need_a_look`.
    pub(crate) noted: BTreeMap<String, i64>,
}

/// The certainty of the stacks the sources (a comma list of `source` ids)
/// created first, `stacks` of them in all.
pub(crate) fn of_sources(
    store: &mut Store,
    sources: &str,
    stacks: i64,
) -> Result<Certainty, StoreError> {
    let q = |t: &str| store.qualified(t);
    let (stack, batch, member, item, class) = (
        q("stack"),
        q("ingest_batch"),
        q("review_member"),
        q("review_item"),
        q("classification"),
    );
    let of_source =
        format!("JOIN {batch} b ON b.id = x.first_batch_id WHERE b.source_id IN ({sources})");
    // the members of the grouped questions, as `to_sort` always counted
    let members = store.query(
        &format!(
            "SELECT COUNT(DISTINCT rm.stack_id) FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id {of_source} AND {OPEN}"
        ),
        &[],
    )?[0]
        .int(0)?;
    let mut need_a_look: BTreeMap<String, i64> = BTreeMap::new();
    for r in store.query(
        &format!(
            "SELECT ri.kind, COUNT(DISTINCT rm.stack_id) FROM {member} rm \
             JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id {of_source} AND {OPEN} GROUP BY ri.kind"
        ),
        &[],
    )? {
        need_a_look.insert(r.text(0)?.to_string(), r.int(1)?);
    }
    // the questions about one stack each, of this dataset's stacks
    let scoped = stack_scoped(store)?;
    let asked: Vec<i64> = scoped
        .iter()
        .map(|(_, s)| *s)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mine = stacks_of_sources(store, &asked, sources)?;
    let mut by_kind: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for (kind, s) in &scoped {
        if mine.contains(s) {
            by_kind.entry(kind.clone()).or_default().insert(*s);
        }
    }
    for (kind, on) in by_kind {
        *need_a_look.entry(kind).or_insert(0) += on.len() as i64;
    }
    // of those, the stacks no grouped question holds already
    let mine: Vec<i64> = mine.into_iter().collect();
    let mut held: BTreeSet<i64> = BTreeSet::new();
    for chunk in mine.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT rm.stack_id FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             WHERE rm.stack_id IN ({}) AND {OPEN}",
            ids_in(chunk)
        );
        for r in store.query(&sql, &[])? {
            held.insert(r.int(0)?);
        }
    }
    let to_sort = members + mine.iter().filter(|s| !held.contains(s)).count() as i64;
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
        to_sort,
        sure: (sorted - to_sort).max(0),
        unsorted: (stacks - sorted).max(0),
        need_a_look,
        noted,
    })
}

/// The review items about a dataset (record 55 H2, round 4: the card's
/// Review button), of one status or of every status: an item with a
/// member stack the dataset's digests created first; an item about one
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
    for r in store.query(
        &format!(
            "SELECT DISTINCT rm.item_id FROM {member} rm JOIN {item} ri ON ri.id = rm.item_id \
             JOIN {stack} x ON x.id = rm.stack_id JOIN {batch} b ON b.id = x.first_batch_id \
             WHERE b.source_id IN ({sources}){filter}"
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
            "SELECT DISTINCT x.series_id FROM {stack} x JOIN {batch} b ON b.id = x.first_batch_id \
             WHERE x.series_id IN ({}) AND b.source_id IN ({sources})",
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
             JOIN {batch} b ON b.id = x.first_batch_id WHERE b.source_id IN ({sources})"
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
    use super::ids_in;

    #[test]
    fn an_id_list_is_the_ids_joined() {
        assert_eq!(ids_in(&[1, 2]), "1, 2");
    }
}
