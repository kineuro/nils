// SPDX-License-Identifier: AGPL-3.0-only

//! Picking main scans as a pipeline step after the sort (record 55 H2,
//! round 4). When a sort (a `classify` job) ends done, the chain queues
//! `pick run --after-sort JOB` for the subjects whose stacks it judged,
//! with the pack it sorted with and its `picks/main.yml`, unchanged and
//! tunable as before. A dataset whose `picks` is `off` keeps its subjects
//! out of it; a sort that judged nothing, or only such datasets' stacks,
//! queues nothing. A chain that already names a pick run after the sort (a
//! bring-in of a dataset that feeds a cohort) keeps its own.
//!
//! The run's job keeps its report as its result; the picks summary door
//! answers what the picks of a dataset's subjects are now, per role, with
//! the last run that decided them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_registry::job::{self, Job};
use nils_registry::place::{self, Place, Role};
use nils_registry::review;
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// The subjects a sort judged a stack of, and the datasets they came
/// through, leaving out the stacks of a dataset that turned picking after
/// a sort off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Sorted {
    pub(crate) subjects: BTreeSet<i64>,
    pub(crate) datasets: Vec<String>,
}

/// What the sort that was job `job_id` judged, as [`Sorted`].
pub(crate) fn sorted_by(store: &mut Store, job_id: i64) -> Result<Sorted, StoreError> {
    let sql = format!(
        "SELECT DISTINCT se.subject_id, b.source_id FROM {} cl \
         JOIN {} x ON x.id = cl.stack_id JOIN {} se ON se.id = x.series_id \
         JOIN {} b ON b.id = x.first_batch_id \
         WHERE cl.job_id = {} AND se.subject_id IS NOT NULL",
        store.qualified("classification"),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("ingest_batch"),
        store.dialect().param(1, Type::Int)
    );
    let rows: Vec<(i64, i64)> = store
        .query(&sql, &[Param::Int(job_id)])?
        .iter()
        .map(|r| Ok((r.int(0)?, r.int(1)?)))
        .collect::<Result<_, StoreError>>()?;
    let mut out = Sorted::default();
    if rows.is_empty() {
        return Ok(out);
    }
    // each source to the dataset whose place holds it
    let places: Vec<Place> = place::list(store)?
        .into_iter()
        .filter(|p| p.role == Role::Source && p.retired_at.is_none())
        .collect();
    let mut dataset_of: HashMap<i64, usize> = HashMap::new();
    for (i, p) in places.iter().enumerate() {
        for id in crate::sources::source_ids(store, p)? {
            dataset_of.entry(id).or_insert(i);
        }
    }
    let mut datasets: BTreeSet<String> = BTreeSet::new();
    for (subject, source) in rows {
        match dataset_of.get(&source).map(|i| &places[*i]) {
            Some(p) if !place::picks_after_sort(&p.dataset) => {}
            Some(p) => {
                out.subjects.insert(subject);
                datasets.insert(p.name.clone());
            }
            // a tree no dataset names is picked as any other
            None => {
                out.subjects.insert(subject);
            }
        }
    }
    out.datasets = datasets.into_iter().collect();
    Ok(out)
}

/// The pick run a sort that ended done is followed by, when there is one:
/// `pick run --after-sort JOB` with the sort's own pack (and pack
/// directory, where its command line named one).
pub(crate) fn step_after(store: &mut Store, job: &Job) -> Option<Vec<String>> {
    let words = job.queued()?;
    if job.kind != "classify" || words.first().map(String::as_str) != Some("classify") {
        return None;
    }
    if words.get(1).is_some_and(|w| w == "votes") {
        return None;
    }
    // a chain that names its own pick run keeps it
    if job
        .then()
        .iter()
        .any(|step| step.first().map(String::as_str) == Some("pick"))
    {
        return None;
    }
    let sorted = match sorted_by(store, job.id) {
        Ok(sorted) => sorted,
        Err(e) => {
            eprintln!(
                "nils: job {}: the stacks it sorted were not read, so no pick follows it: {e}",
                job.id
            );
            return None;
        }
    };
    if sorted.subjects.is_empty() {
        return None;
    }
    let mut step = vec![
        "pick".to_string(),
        "run".to_string(),
        "--after-sort".to_string(),
        job.id.to_string(),
    ];
    let mut rest = words.iter().skip(1);
    while let Some(w) = rest.next() {
        if matches!(w.as_str(), "--pack" | "--pack-dir")
            && let Some(v) = rest.next()
        {
            step.extend([w.clone(), v.clone()]);
        }
    }
    Some(step)
}

/// One role of the summary.
#[derive(Debug, Default)]
struct RoleSum {
    picked: i64,
    clear: i64,
    tied: i64,
    review_items: i64,
    borders: BTreeMap<String, i64>,
    /// The subjects with a pick that applies on at least one occasion.
    subjects: BTreeSet<i64>,
}

/// Whether the two best of a pick's `considered` scored the same.
fn tied(considered: &Value) -> bool {
    let Some(list) = considered.as_array() else {
        return false;
    };
    match (list.first(), list.get(1)) {
        (Some(a), Some(b)) => match (a["score"].as_f64(), b["score"].as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

/// `GET /api/picks/summary?dataset=NAME`: per role, how many of the
/// dataset's subjects' occasions have a pick that applies (`picked`), how
/// many of those nobody needs to look at (`clear`: no open border, no
/// tie), the ties, the open `pick.border` items and their borders by
/// reason (`nothing_eligible` among them, an occasion with no pick), and
/// since 2026-10-09 how many subjects have a pick on at least one occasion
/// (`subjects`); the dataset's `picks` setting, and the last pick run that
/// decided its subjects, with its report. With `scheme`, the picks made
/// under that scheme alone.
pub(crate) fn summary(
    store: &mut Store,
    dataset: &Place,
    scheme: Option<&str>,
) -> Result<Value, StoreError> {
    let subjects = crate::chain::dataset_subjects(store, dataset)?;
    let (roles, review_items) = roles_of(store, &subjects, scheme)?;
    Ok(json!({
        "dataset": dataset.name,
        "dataset_id": dataset.id,
        "picks": if place::picks_after_sort(&dataset.dataset) { "after_sort" } else { "off" },
        "subjects": subjects.len(),
        "scheme": scheme,
        "roles": roles,
        "review_items": review_items,
        "last_run": last_run(store, &format!("dataset:{}", dataset.name), Some(&dataset.name))?,
    }))
}

/// Wave 7a, the Data page (2026-10-09): `GET /api/picks/summary?cohort=NAME`,
/// the same for a cohort's open members, with the last pick run that
/// decided them (one for the cohort, or one for the whole registry). None
/// for a cohort that does not exist.
pub(crate) fn cohort_summary(
    store: &mut Store,
    name: &str,
    scheme: Option<&str>,
) -> Result<Option<Value>, StoreError> {
    let Some(subjects) = nils_registry::cohort::open_members_of(store, name)? else {
        return Ok(None);
    };
    let (roles, review_items) = roles_of(store, &subjects, scheme)?;
    Ok(Some(json!({
        "cohort": name,
        "subjects": subjects.len(),
        "scheme": scheme,
        "roles": roles,
        "review_items": review_items,
        "last_run": last_run(store, &format!("cohort:{name}"), None)?,
    })))
}

/// The roles of the summary for these subjects, and their open
/// `pick.border` items in all.
fn roles_of(
    store: &mut Store,
    subjects: &BTreeSet<i64>,
    scheme: Option<&str>,
) -> Result<(serde_json::Map<String, Value>, i64), StoreError> {
    let mut roles: BTreeMap<String, RoleSum> = BTreeMap::new();
    let mut tied_on: BTreeSet<(String, i64, String)> = BTreeSet::new();
    let mut picked_on: BTreeSet<(String, i64, String)> = BTreeSet::new();
    let ids: Vec<i64> = subjects.iter().copied().collect();
    let d = store.dialect();
    let (scheme_filter, params): (String, Vec<Param>) = match scheme {
        Some(s) => (
            format!(" AND p.scheme = {}", d.param(1, Type::Text)),
            vec![Param::from(s)],
        ),
        None => (String::new(), Vec::new()),
    };
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT p.role, p.subject_id, {}, p.author_kind, {} FROM {} p \
             WHERE p.withdrawn_at IS NULL AND p.subject_id IN ({}){scheme_filter}",
            crate::text_of(store, "pick", "session_day"),
            crate::text_of(store, "pick", "considered"),
            store.qualified("pick"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &params)? {
            let role = r.text(0)?.to_string();
            let key = (role.clone(), r.int(1)?, r.text(2)?.to_string());
            if !picked_on.insert(key.clone()) {
                continue;
            }
            let sum = roles.entry(role).or_default();
            sum.picked += 1;
            sum.subjects.insert(key.1);
            let considered: Value = r
                .opt_text(4)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            if r.text(3)? == "agent" && tied(&considered) {
                tied_on.insert(key);
            }
        }
    }
    // the open borders on the dataset's subjects' occasions
    let sql = format!(
        "SELECT {}, {} FROM {} WHERE kind = {} AND status IN ('open', 'staged')",
        crate::text_of(store, "review_item", "ref"),
        crate::text_of(store, "review_item", "evidence"),
        store.qualified("review_item"),
        d.param(1, Type::Text)
    );
    let mut bordered: BTreeSet<(String, i64, String)> = BTreeSet::new();
    for r in store.query(&sql, &[Param::from(review::PICK_BORDER_KIND)])? {
        let parse = |i: usize| -> Result<Value, StoreError> {
            Ok(r.opt_text(i)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null))
        };
        let (reference, evidence) = (parse(0)?, parse(1)?);
        let (Some(subject), Some(role)) =
            (reference["subject_id"].as_i64(), reference["role"].as_str())
        else {
            continue;
        };
        if !subjects.contains(&subject) {
            continue;
        }
        if let Some(s) = scheme
            && evidence["scheme"]["name"].as_str().is_some_and(|n| n != s)
        {
            continue;
        }
        let day = reference["session_day"].as_str().unwrap_or("").to_string();
        bordered.insert((role.to_string(), subject, day));
        let sum = roles.entry(role.to_string()).or_default();
        sum.review_items += 1;
        for b in evidence["borders"].as_array().into_iter().flatten() {
            if let Some(name) = b.as_str() {
                *sum.borders.entry(name.to_string()).or_insert(0) += 1;
            }
        }
    }
    for key in &picked_on {
        let sum = roles.entry(key.0.clone()).or_default();
        if tied_on.contains(key) {
            sum.tied += 1;
        }
        if !tied_on.contains(key) && !bordered.contains(key) {
            sum.clear += 1;
        }
    }
    let review_items: i64 = roles.values().map(|r| r.review_items).sum();
    let roles: serde_json::Map<String, Value> = roles
        .into_iter()
        .map(|(role, s)| {
            (
                role,
                json!({
                    "picked": s.picked, "clear": s.clear, "tied": s.tied,
                    "borders": s.borders, "review_items": s.review_items,
                    "subjects": s.subjects.len(),
                }),
            )
        })
        .collect();
    Ok((roles, review_items))
}

/// The newest pick run that decided these subjects, with its report: one
/// for the dataset or the cohort (`label`, `dataset:NAME` or
/// `cohort:NAME`), one after a sort that named the dataset, or one for the
/// whole registry. Null when none of the newest runs did.
fn last_run(store: &mut Store, label: &str, dataset: Option<&str>) -> Result<Value, StoreError> {
    let sql = format!(
        "SELECT id FROM {} WHERE kind = 'pick' ORDER BY id DESC LIMIT 25",
        store.qualified("job")
    );
    let ids: Vec<i64> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    for id in ids {
        let Some(j) = job::show(store, id).map_err(|e| StoreError::Message(e.to_string()))? else {
            continue;
        };
        let Some(report) = j.result.as_ref().filter(|r| r.is_object()) else {
            continue;
        };
        let covers = match report["only"].as_str() {
            None => report["reference"] == "registry",
            Some(only) => {
                only == label
                    || dataset.is_some_and(|name| {
                        report["datasets"]
                            .as_array()
                            .is_some_and(|d| d.iter().any(|n| n == name))
                    })
            }
        };
        if covers {
            return Ok(json!({
                "job": j.id, "state": j.state.name(), "finished_at": j.finished_at,
                "report": report,
            }));
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::tied;
    use serde_json::json;

    #[test]
    fn a_tie_is_two_best_with_the_same_score() {
        assert!(tied(
            &json!([{"stacks": [1], "score": 0.8}, {"stacks": [2], "score": 0.8}])
        ));
        assert!(!tied(
            &json!([{"stacks": [1], "score": 0.9}, {"stacks": [2], "score": 0.8}])
        ));
        assert!(!tied(&json!([{"stacks": [1], "score": 0.9}])));
        assert!(!tied(&json!(null)));
    }
}
