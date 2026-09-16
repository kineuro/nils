// SPDX-License-Identifier: AGPL-3.0-only

//! Record 26 §14: the batch is the thread. A batch page reads five stages
//! off one batch row, the batch it shares a name with on the other side of
//! the thread (the pseudonymise step before a digest, or the digest after
//! a pseudonymise step), the jobs that touched its stacks and the review
//! items on them: pseudonymised, walked, digested, classified, reviewed.
//! Every number is a count; nothing here is a row of a person.

use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// One `ingest_batch` row, as the stages read it.
#[derive(Debug, Clone)]
pub(crate) struct Batch {
    pub(crate) id: i64,
    /// `digest` or `pseudonymize`; a row from before reads as a digest.
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) source_id: i64,
    pub(crate) job_id: Option<i64>,
    pub(crate) state: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) counts: Value,
}

fn select(store: &Store) -> String {
    format!(
        "SELECT id, kind, name, source_id, job_id, state, {}, {}, {} FROM {}",
        crate::text_of(store, "ingest_batch", "started_at"),
        crate::text_of(store, "ingest_batch", "finished_at"),
        crate::text_of(store, "ingest_batch", "counts"),
        store.qualified("ingest_batch"),
    )
}

fn of(r: &nils_registry::Row) -> Result<Batch, StoreError> {
    Ok(Batch {
        id: r.int(0)?,
        kind: r.opt_text(1)?.unwrap_or("digest").to_string(),
        name: r.text(2)?.to_string(),
        source_id: r.int(3)?,
        job_id: r.opt_int(4)?,
        state: r.text(5)?.to_string(),
        started_at: r.opt_text(6)?.map(str::to_string),
        finished_at: r.opt_text(7)?.map(str::to_string),
        counts: r
            .opt_text(8)?
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null),
    })
}

pub(crate) fn row(store: &mut Store, id: i64) -> Result<Option<Batch>, StoreError> {
    let sql = format!(
        "{} WHERE id = {}",
        select(store),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .as_ref()
        .map(of)
        .transpose()
}

/// The batch on the other side of the thread: for a digest, the latest
/// pseudonymise step of the same name on the same source before it; for a
/// pseudonymise step, the first digest of the same name after it.
pub(crate) fn other_side(store: &mut Store, batch: &Batch) -> Result<Option<Batch>, StoreError> {
    let d = store.dialect();
    let (kind, order, cmp) = if batch.kind == "pseudonymize" {
        ("digest", "ASC", ">")
    } else {
        ("pseudonymize", "DESC", "<")
    };
    let sql = format!(
        "{} WHERE source_id = {} AND name = {} AND kind = {} AND id {cmp} {} ORDER BY id {order} LIMIT 1",
        select(store),
        d.param(1, Type::Int),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
        d.param(4, Type::Int),
    );
    store
        .query_opt(
            &sql,
            &[
                Param::Int(batch.source_id),
                Param::from(batch.name.as_str()),
                Param::from(kind),
                Param::Int(batch.id),
            ],
        )?
        .as_ref()
        .map(of)
        .transpose()
}

fn n(counts: &Value, path: &[&str]) -> u64 {
    path.iter()
        .fold(counts, |v, k| &v[*k])
        .as_u64()
        .unwrap_or(0)
}

/// The pseudonymised stage as a pseudonymise batch's counts give it.
pub(crate) fn pseudonymised(batch: &Batch) -> Value {
    let c = &batch.counts;
    json!({
        "batch": batch.id,
        "files": n(c, &["files", "written"]) + n(c, &["files", "unchanged"]),
        "changed": n(c, &["files", "written"]),
        "held": n(c, &["files", "held"]),
        "refused": n(c, &["files", "refused"]),
        "job": batch.job_id,
        "state": batch.state,
    })
}

fn count(store: &mut Store, sql: &str, params: &[Param]) -> Result<i64, StoreError> {
    store.query(sql, params)?[0].int(0)
}

/// The five stages of a batch's thread.
pub(crate) fn stages(store: &mut Store, batch: &Batch) -> Result<Value, StoreError> {
    let other = other_side(store, batch)?;
    let (pseudonymise, digest) = if batch.kind == "pseudonymize" {
        (Some(batch.clone()), other)
    } else {
        (other, Some(batch.clone()))
    };
    let pseudonymised = pseudonymise.as_ref().map(pseudonymised);
    let (walked, digested) = match &digest {
        Some(b) => {
            let c = &b.counts;
            let d = store.dialect();
            let sessions = count(
                store,
                &format!(
                    "SELECT COUNT(DISTINCT scs.session_id) FROM {} scs JOIN {} st ON st.id = scs.study_id WHERE st.first_batch_id = {}",
                    store.qualified("session_cache_study"),
                    store.qualified("study"),
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )?;
            (
                json!({
                    "batch": b.id,
                    "files": n(c, &["seen"]),
                    "new": n(c, &["written", "ingested"]),
                    "changed": n(c, &["written", "changed"]),
                    "unchanged": n(c, &["unchanged"]),
                    "refused": n(c, &["quarantined"]),
                    "job": b.job_id,
                    "state": b.state,
                }),
                json!({
                    "batch": b.id,
                    "stacks": n(c, &["written", "stacks_created"]),
                    "sessions": sessions,
                    "subjects": n(c, &["written", "subjects_created"]),
                    "moved": n(c, &["written", "gone"]),
                    "job": b.job_id,
                }),
            )
        }
        None => (
            json!({"batch": null, "files": 0, "new": 0, "changed": 0, "unchanged": 0, "refused": 0, "job": null, "state": null}),
            json!({"batch": null, "stacks": 0, "sessions": 0, "subjects": 0, "moved": 0, "job": null}),
        ),
    };
    let (classified, reviewed) = match &digest {
        Some(b) => {
            let d = store.dialect();
            let stack = store.qualified("stack");
            let class = store.qualified("classification");
            let axis = store.qualified("classification_axis");
            let member = store.qualified("review_member");
            let item = store.qualified("review_item");
            let of = count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM {stack} WHERE first_batch_id = {}",
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )?;
            let stacks = count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM {class} c JOIN {stack} s ON s.id = c.stack_id WHERE s.first_batch_id = {}",
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )?;
            let unsure = count(
                store,
                &format!(
                    "SELECT COUNT(DISTINCT s.id) FROM {stack} s JOIN {member} m ON m.stack_id = s.id \
                     JOIN {item} i ON i.id = m.item_id WHERE s.first_batch_id = {} AND i.status IN ('open', 'staged')",
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )?;
            let packs = store.query(
                &format!(
                    "SELECT DISTINCT c.pack, c.pack_version FROM {class} c JOIN {stack} s ON s.id = c.stack_id \
                     WHERE s.first_batch_id = {} ORDER BY c.pack, c.pack_version",
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )?;
            let pack: Option<String> = packs
                .first()
                .map(|r| Ok::<_, StoreError>(format!("{} {}", r.text(0)?, r.text(1)?)))
                .transpose()?;
            let jobs: Vec<i64> = store
                .query(
                    &format!(
                        "SELECT DISTINCT c.job_id FROM {class} c JOIN {stack} s ON s.id = c.stack_id \
                         WHERE s.first_batch_id = {} ORDER BY c.job_id",
                        d.param(1, Type::Int)
                    ),
                    &[Param::Int(b.id)],
                )?
                .iter()
                .map(|r| r.int(0))
                .collect::<Result<_, _>>()?;
            let mut by_base = serde_json::Map::new();
            for r in store.query(
                &format!(
                    "SELECT a.value, COUNT(*) FROM {axis} a JOIN {stack} s ON s.id = a.stack_id \
                     WHERE s.first_batch_id = {} AND a.axis = 'base' GROUP BY a.value ORDER BY a.value",
                    d.param(1, Type::Int)
                ),
                &[Param::Int(b.id)],
            )? {
                by_base.insert(
                    r.opt_text(0)?.unwrap_or("none").to_string(),
                    json!(r.int(1)?),
                );
            }
            // the questions on the batch's stacks, and the batch's own
            let created = crate::text_of(store, "review_item", "created_at");
            let items = store.query(
                &format!(
                    "SELECT i.id, i.status, {created} FROM {item} i WHERE i.id IN (\
                       SELECT m.item_id FROM {member} m JOIN {stack} s ON s.id = m.stack_id WHERE s.first_batch_id = {}\
                     ) OR (i.scope = 'batch' AND i.ref LIKE {})",
                    d.param(1, Type::Int),
                    d.param(2, Type::Text),
                ),
                &[
                    Param::Int(b.id),
                    Param::from(format!("%\"batch_id\":{}%", b.id)),
                ],
            )?;
            let mut done = 0i64;
            let mut since: Option<String> = None;
            for r in &items {
                let status = r.text(1)?;
                if status == "open" || status == "staged" {
                    let at = r.opt_text(2)?.unwrap_or_default().to_string();
                    if since.as_ref().is_none_or(|s| at < *s) {
                        since = Some(at);
                    }
                } else {
                    done += 1;
                }
            }
            (
                json!({
                    "stacks": stacks,
                    "of": of,
                    "unsure": unsure,
                    "pack": pack,
                    "jobs": jobs,
                    "by_base": by_base,
                }),
                json!({
                    "done": done,
                    "of": items.len(),
                    "since": since,
                }),
            )
        }
        None => (
            json!({"stacks": 0, "of": 0, "unsure": 0, "pack": null, "jobs": [], "by_base": {}}),
            json!({"done": 0, "of": 0, "since": null}),
        ),
    };
    Ok(json!({
        "pseudonymised": pseudonymised,
        "walked": walked,
        "digested": digested,
        "classified": classified,
        "reviewed": reviewed,
    }))
}

/// Files per second of the last run of a kind that ended done on this
/// host, from the batch's own counts; none when there was no such run.
pub(crate) fn rate_of(store: &mut Store, kind: &str) -> Result<Option<f64>, StoreError> {
    let d = store.dialect();
    let counts = crate::text_of(store, "ingest_batch", "counts");
    let sql = format!(
        "SELECT {counts} FROM {} b JOIN {} j ON j.id = b.job_id WHERE b.kind = {} AND b.state = 'done' \
         AND j.host = {} ORDER BY b.id DESC LIMIT 1",
        store.qualified("ingest_batch"),
        store.qualified("job"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let host = nils_registry::job::hostname();
    Ok(store
        .query_opt(&sql, &[Param::from(kind), Param::from(host.as_str())])?
        .and_then(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        })
        .and_then(|c| c["files_per_s"].as_f64())
        .map(|f| (f * 10.0).round() / 10.0))
}
