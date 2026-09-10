// SPDX-License-Identifier: AGPL-3.0-only

//! The summary door (Wave 5 §12.1): what the registry holds, as counts.
//! Subjects, sessions and stacks, each as a total and by cohort; subjects
//! and stacks by pack version; the epoch; whether the registry is
//! synthetic; and, when a date is given, what arrived after it. Never a row
//! of a person: every number here is a count.

use nils_registry::Param;
use nils_registry::Registry;
use nils_registry::schema::Type;
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

fn count(store: &mut Store, sql: &str, params: &[Param]) -> Result<i64, StoreError> {
    store.query(sql, params)?[0].int(0)
}

/// Rows of `(key, count)` as a JSON object keyed by the text column.
fn by(store: &mut Store, sql: &str, params: &[Param]) -> Result<Value, StoreError> {
    let mut out = serde_json::Map::new();
    for r in store.query(sql, params)? {
        out.insert(r.text(0)?.to_string(), json!(r.int(1)?));
    }
    Ok(Value::Object(out))
}

/// The marker `nils synth` leaves in `registry_meta`; none on real data.
pub fn synthetic(store: &mut Store) -> Result<Option<String>, StoreError> {
    let sql = format!(
        "SELECT value FROM {} WHERE key = 'synthetic'",
        store.qualified("registry_meta")
    );
    store
        .query_opt(&sql, &[])?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()
}

/// The summary document.
pub fn document(registry: &mut Registry, since: Option<&str>) -> Result<Value, StoreError> {
    let epoch = registry.meta().epoch;
    let window = Scheme::default().window_days;
    let store = registry.store();
    let d = store.dialect();
    let q = |t: &str| store.qualified(t);
    let (subject, session, stack, series, study, member, cohort, class, handle, release, batch) = (
        q("subject"),
        q("session_cache"),
        q("stack"),
        q("series"),
        q("study"),
        q("cohort_member"),
        q("cohort"),
        q("classification"),
        q("handle"),
        q("release"),
        q("ingest_batch"),
    );
    let win = d.param(1, Type::Int);
    let window_p = [Param::Int(window)];

    let subjects = count(store, &format!("SELECT COUNT(*) FROM {subject}"), &[])?;
    let sessions = count(
        store,
        &format!("SELECT COUNT(*) FROM {session} WHERE window_days = {win}"),
        &window_p,
    )?;
    let stacks = count(store, &format!("SELECT COUNT(*) FROM {stack}"), &[])?;

    // by cohort: a subject in two cohorts counts once in each, so the sums
    // may exceed the totals; the desk reads them as memberships
    let current = format!("{member} m JOIN {cohort} c ON c.id = m.cohort_id AND m.left_at IS NULL");
    let subjects_by_cohort = by(
        store,
        &format!(
            "SELECT c.name, COUNT(DISTINCT m.subject_id) FROM {current} GROUP BY c.name ORDER BY c.name"
        ),
        &[],
    )?;
    let sessions_by_cohort = by(
        store,
        &format!(
            "SELECT c.name, COUNT(DISTINCT sc.id) FROM {current} \
             JOIN {session} sc ON sc.subject_id = m.subject_id AND sc.window_days = {win} \
             GROUP BY c.name ORDER BY c.name"
        ),
        &window_p,
    )?;
    let stacks_by_cohort = by(
        store,
        &format!(
            "SELECT c.name, COUNT(DISTINCT st.id) FROM {current} \
             JOIN {study} sy ON sy.subject_id = m.subject_id \
             JOIN {series} se ON se.study_id = sy.id \
             JOIN {stack} st ON st.series_id = se.id \
             GROUP BY c.name ORDER BY c.name"
        ),
        &[],
    )?;

    // by pack version: the stacks a version classified, and the subjects
    // those stacks belong to
    let stacks_by_pack = by(
        store,
        &format!(
            "SELECT cl.pack_version, COUNT(DISTINCT cl.stack_id) FROM {class} cl \
             GROUP BY cl.pack_version ORDER BY cl.pack_version"
        ),
        &[],
    )?;
    let subjects_by_pack = by(
        store,
        &format!(
            "SELECT cl.pack_version, COUNT(DISTINCT se.subject_id) FROM {class} cl \
             JOIN {stack} st ON st.id = cl.stack_id \
             JOIN {series} se ON se.id = st.series_id \
             GROUP BY cl.pack_version ORDER BY cl.pack_version"
        ),
        &[],
    )?;
    let cohorts = count(store, &format!("SELECT COUNT(*) FROM {cohort}"), &[])?;
    let synthetic = synthetic(store)?;

    let since_block = match since {
        None => Value::Null,
        Some(date) => {
            let p = d.param(1, Type::Text);
            let dp = [Param::from(date)];
            let subjects = count(
                store,
                &format!("SELECT COUNT(*) FROM {subject} WHERE created_at > {p}"),
                &dp,
            )?;
            let sessions = count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM {session} WHERE built_at > {p} AND window_days = {}",
                    d.param(2, Type::Int)
                ),
                &[Param::from(date), Param::Int(window)],
            )?;
            let stacks = count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM {stack} st JOIN {batch} b ON b.id = st.first_batch_id \
                     WHERE b.started_at > {p}"
                ),
                &dp,
            )?;
            let handles = count(
                store,
                &format!("SELECT COUNT(*) FROM {handle} WHERE created_at > {p}"),
                &dp,
            )?;
            let releases = count(
                store,
                &format!("SELECT COUNT(*) FROM {release} WHERE started_at > {p}"),
                &dp,
            )?;
            json!({
                "date": date,
                "subjects": subjects, "sessions": sessions, "stacks": stacks,
                "handles": handles, "releases": releases,
            })
        }
    };

    Ok(json!({
        "epoch": epoch,
        "synthetic": synthetic,
        "cohorts": cohorts,
        "subjects": { "total": subjects, "by_cohort": subjects_by_cohort, "by_pack_version": subjects_by_pack },
        "sessions": { "total": sessions, "by_cohort": sessions_by_cohort, "window_days": window },
        "stacks": { "total": stacks, "by_cohort": stacks_by_cohort, "by_pack_version": stacks_by_pack },
        "since": since_block,
    }))
}
