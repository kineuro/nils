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
            // record 26 §14: the subjects whose files this batch holds,
            // whoever made them. A map makes the subjects of a dataset and
            // the digest meets them, so counting what the digest created
            // answers nothing for exactly the dataset that has a map.
            let subjects = count(
                store,
                &format!(
                    "SELECT COUNT(DISTINCT se.subject_id) FROM {} f \
                     JOIN {} i ON i.id = f.instance_id JOIN {} se ON se.id = i.series_id \
                     JOIN {} su ON su.id = se.subject_id \
                     WHERE f.batch_id = {} AND su.merged_into IS NULL",
                    store.qualified("source_file"),
                    store.qualified("instance"),
                    store.qualified("series"),
                    store.qualified("subject"),
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
                    "subjects": subjects,
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

/// The files a run read or wrote, as its own counts record them: the
/// digest's `seen`, the pseudonymiser's `files.seen`.
fn files_of(counts: &Value) -> u64 {
    n(counts, &["seen"]).max(n(counts, &["files", "seen"]))
}

/// The files a run must have read or written for its rate to say anything
/// about the machine rather than about the cost of starting a run.
const ENOUGH: u64 = 100;

/// The runs of a kind a rate looks back over.
const RUNS: usize = 50;

/// What this machine does per second at a step, `{files_per_s, files}`: the
/// last run of the kind that ended done here and read or wrote at least a
/// hundred files, else the largest run there was; null where there was no
/// run at all. The files it was measured over come with it, because a run
/// of twenty files times the cost of starting a run and says nothing about
/// the disk (record 26 §14).
pub(crate) fn rate_of(store: &mut Store, kind: &str) -> Result<Value, StoreError> {
    let d = store.dialect();
    let counts = crate::text_of(store, "ingest_batch", "counts");
    let sql = format!(
        "SELECT {counts} FROM {} b JOIN {} j ON j.id = b.job_id WHERE b.kind = {} AND b.state = 'done' \
         AND j.host = {} ORDER BY b.id DESC LIMIT {RUNS}",
        store.qualified("ingest_batch"),
        store.qualified("job"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let host = nils_registry::job::hostname();
    let rows = store.query(&sql, &[Param::from(kind), Param::from(host.as_str())])?;
    let mut runs: Vec<(u64, f64)> = Vec::with_capacity(rows.len());
    for r in &rows {
        let Some(counts) = r
            .opt_text(0)?
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
        else {
            continue;
        };
        if let Some(rate) = counts["files_per_s"].as_f64() {
            runs.push((files_of(&counts), rate));
        }
    }
    Ok(match pick(&runs) {
        Some((files, rate)) => json!({
            "files_per_s": (rate * 10.0).round() / 10.0,
            "files": files,
        }),
        None => Value::Null,
    })
}

/// The run a rate is taken from, given the runs newest first: the first
/// that read or wrote enough files, else the largest of them.
fn pick(runs: &[(u64, f64)]) -> Option<(u64, f64)> {
    runs.iter()
        .find(|(files, _)| *files >= ENOUGH)
        .or_else(|| runs.iter().max_by_key(|(files, _)| *files))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::{files_of, pick};
    use serde_json::json;

    #[test]
    fn a_rate_comes_from_a_run_big_enough_to_mean_something() {
        // record 26 §14: a resume run that wrote ten of a thousand files
        // said 80,995 files a second, which is the cost of starting a run
        // and not the disk. The newest run of a hundred files or more wins.
        let runs = [(20, 80_995.0), (960, 9_964.1), (800, 17_755.5)];
        assert_eq!(pick(&runs), Some((960, 9_964.1)));
        // with none that big, the largest there was, so a small archive
        // still says something
        let small = [(20, 80_995.0), (40, 3_115.0)];
        assert_eq!(pick(&small), Some((40, 3_115.0)));
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn the_files_of_a_run_are_the_ones_its_own_counts_name() {
        // the digest counts what it saw at the top; the pseudonymiser
        // counts its files under `files`
        assert_eq!(files_of(&json!({"seen": 960, "files_per_s": 1.0})), 960);
        assert_eq!(files_of(&json!({"files": {"seen": 160}})), 160);
        assert_eq!(files_of(&json!({})), 0);
    }
}
