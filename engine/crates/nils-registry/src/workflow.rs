// SPDX-License-Identifier: AGPL-3.0-only

//! Workflows in the catalog and their runs (Wave 7b A7, record 57 P7 and
//! P8): a workflow is versioned as a pipeline is (the same document again
//! is the version it is), and a workflow run holds the frozen scope, the
//! person's choices and each step's pipeline run as the chain makes it.

use serde::Serialize;
use serde_json::Value;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};

/// A version the engine seeded (`origin`).
pub const STARTER: &str = "starter";

pub const RUN_STATUSES: [&str; 6] = [
    "running",
    "done",
    "partial",
    "failed",
    "cancelled",
    "interrupted",
];

/// One workflow version as the catalog holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub document: Value,
    pub digest: String,
    pub state: String,
    pub added_by: String,
    pub added_at: String,
    pub origin: Option<String>,
}

impl Workflow {
    /// `name@version`.
    pub fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

const COLUMNS: [&str; 9] = [
    "id", "name", "version", "document", "digest", "state", "added_by", "added_at", "origin",
];

fn select(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("workflow");
    let cols: Vec<String> = COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a workflow column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("workflow")
    )
}

fn json(text: Option<&str>) -> Value {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

fn of(r: &Row) -> Result<Workflow, Error> {
    Ok(Workflow {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        version: r.int(2)?,
        document: json(r.opt_text(3)?),
        digest: r.text(4)?.to_string(),
        state: r.text(5)?.to_string(),
        added_by: r.text(6)?.to_string(),
        added_at: r.text(7)?.to_string(),
        origin: r.opt_text(8)?.map(str::to_string),
    })
}

/// The words an [`add`] answers where another writer took the version it
/// computed, every time it tried: the door answers 409 on them.
pub const VERSION_TAKEN: &str = "was taken by another writer at the same time";

/// How many times [`add`] computes the next version again after another
/// writer took it.
const ADD_TRIES: usize = 3;

/// Whether an error is the unique key refusing a row: the (name, version)
/// of a workflow another writer inserted between the read and the insert.
fn is_unique_violation(e: &Error) -> bool {
    match e {
        Error::Sqlite(rusqlite::Error::SqliteFailure(f, _)) => {
            f.code == rusqlite::ErrorCode::ConstraintViolation
        }
        Error::Postgres(e) => e
            .as_db_error()
            .is_some_and(|d| *d.code() == postgres::error::SqlState::UNIQUE_VIOLATION),
        _ => false,
    }
}

/// Add a document: the name's next version, or the version that already
/// has this digest. Answers the entry and whether it is new. Two writers
/// adding versions of one name at once (after the third review, finding
/// 8): the version is read and the row inserted in one transaction, and
/// where the unique key refuses the row because another writer took that
/// version first, the versions are read again and the insert tried again,
/// a few times, before the add answers [`VERSION_TAKEN`] in words, never
/// the constraint's.
pub fn add(
    store: &mut Store,
    name: &str,
    document: &Value,
    digest: &str,
    added_by: &str,
    added_at: &str,
    origin: Option<&str>,
) -> Result<(Workflow, bool), Error> {
    let mut version = 0;
    for _ in 0..ADD_TRIES {
        store.begin()?;
        let attempt = (|| -> Result<Result<(Workflow, bool), i64>, Error> {
            let versions = versions(store, name)?;
            if let Some(same) = versions.iter().find(|w| w.digest == digest) {
                return Ok(Ok((same.clone(), false)));
            }
            let version = versions.iter().map(|w| w.version).max().unwrap_or(0) + 1;
            let inserted = store.insert(
                &Insert::new(
                    table("workflow"),
                    &[
                        "name", "version", "document", "digest", "state", "added_by", "added_at",
                        "origin",
                    ],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from(name),
                    Param::Int(version),
                    Param::from(document.to_string()),
                    Param::from(digest),
                    Param::from("active"),
                    Param::from(added_by),
                    Param::from(added_at),
                    origin.map_or(Param::Null, Param::from),
                ]],
            );
            let rows = match inserted {
                Ok(rows) => rows,
                Err(e) if is_unique_violation(&e) => return Ok(Err(version)),
                Err(e) => return Err(e),
            };
            let id = rows
                .first()
                .ok_or_else(|| Error::Message("the workflow was not written back".into()))?
                .int(0)?;
            let w = get(store, id)?.ok_or_else(|| Error::Message(format!("no workflow {id}")))?;
            Ok(Ok((w, true)))
        })();
        match attempt {
            Ok(Ok(answer)) => {
                store.commit()?;
                return Ok(answer);
            }
            Ok(Err(taken)) => {
                // another writer took the version between the read and the
                // insert: read again
                store.rollback()?;
                version = taken;
            }
            Err(e) => {
                let _ = store.rollback();
                return Err(e);
            }
        }
    }
    Err(Error::Message(format!(
        "workflow {name}: the version {version} {VERSION_TAKEN}; try again"
    )))
}

/// Every version of one name, oldest first.
pub fn versions(store: &mut Store, name: &str) -> Result<Vec<Workflow>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE name = {} ORDER BY version",
        select(store),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(name)])?
        .iter()
        .map(of)
        .collect()
}

pub fn get(store: &mut Store, id: i64) -> Result<Option<Workflow>, Error> {
    let d = store.dialect();
    let sql = format!("{} WHERE id = {}", select(store), d.param(1, Type::Int));
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| of(&r))
        .transpose()
}

/// An entry as a person names it: its id, `name@version`, or a name, which
/// is its newest active version.
pub fn resolve(store: &mut Store, reference: &str) -> Result<Option<Workflow>, Error> {
    let reference = reference.trim();
    if let Ok(id) = reference.parse::<i64>() {
        return get(store, id);
    }
    if let Some((name, version)) = reference.rsplit_once('@') {
        let Ok(v) = version.parse::<i64>() else {
            return Ok(None);
        };
        return Ok(versions(store, name)?.into_iter().find(|w| w.version == v));
    }
    Ok(versions(store, reference)?
        .into_iter()
        .rev()
        .find(|w| w.state == "active"))
}

/// The catalog, by name and version.
pub fn list(store: &mut Store) -> Result<Vec<Workflow>, Error> {
    let sql = format!("{} ORDER BY name, version", select(store));
    store.query(&sql, &[])?.iter().map(of).collect()
}

/// One workflow run as the registry holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Run {
    pub id: i64,
    pub workflow_id: i64,
    pub job_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub selection: Option<String>,
    pub person: Value,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// `[{step, of, name, pipeline, pipeline_id, run, status, ...}]`, one
    /// per step of the document, in order: the `workflow_step` rows, each
    /// its document fields with what the chain recorded laid over them.
    pub steps: Value,
    pub summary: Value,
    pub principal: String,
    pub actor: Value,
    pub error: Option<String>,
}

const RUN_COLUMNS: [&str; 13] = [
    "id",
    "workflow_id",
    "job_id",
    "handle_id",
    "selection",
    "person",
    "status",
    "started_at",
    "finished_at",
    "summary",
    "principal",
    "actor",
    "error",
];

fn select_runs(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("workflow_run");
    let cols: Vec<String> = RUN_COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a workflow_run column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("workflow_run")
    )
}

fn run_of(r: &Row) -> Result<Run, Error> {
    Ok(Run {
        id: r.int(0)?,
        workflow_id: r.int(1)?,
        job_id: r.opt_int(2)?,
        handle_id: r.opt_int(3)?,
        selection: r.opt_text(4)?.map(str::to_string),
        person: json(r.opt_text(5)?),
        status: r.text(6)?.to_string(),
        started_at: r.text(7)?.to_string(),
        finished_at: r.opt_text(8)?.map(str::to_string),
        steps: Value::Array(Vec::new()),
        summary: json(r.opt_text(9)?),
        principal: r.text(10)?.to_string(),
        actor: json(r.opt_text(11)?),
        error: r.opt_text(12)?.map(str::to_string),
    })
}

const STEP_COLUMNS: [&str; 8] = [
    "workflow_run_id",
    "step",
    "doc",
    "run_id",
    "status",
    "error",
    "units",
    "finished_at",
];

fn select_steps(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("workflow_step");
    let cols: Vec<String> = STEP_COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a workflow_step column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("workflow_step")
    )
}

/// One step row as the `steps` value carries it: the document's fields
/// with `run`, `status`, `error`, `units` and `finished_at` laid over.
fn step_of(r: &Row) -> Result<(i64, i64, Value), Error> {
    let mut v = json(r.opt_text(2)?);
    if !v.is_object() {
        v = Value::Object(Default::default());
    }
    v["step"] = Value::from(r.int(1)?);
    v["run"] = r.opt_int(3)?.map_or(Value::Null, Value::from);
    v["status"] = Value::from(r.text(4)?);
    v["error"] = r.opt_text(5)?.map_or(Value::Null, Value::from);
    v["units"] = json(r.opt_text(6)?);
    v["finished_at"] = r.opt_text(7)?.map_or(Value::Null, Value::from);
    Ok((r.int(0)?, r.int(1)?, v))
}

/// The steps of some runs, by run, each in step order.
fn steps_of(store: &mut Store, run_ids: &[i64]) -> Result<Vec<(i64, i64, Value)>, Error> {
    if run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = run_ids.iter().map(i64::to_string).collect();
    let sql = format!(
        "{} WHERE workflow_run_id IN ({}) ORDER BY workflow_run_id, step",
        select_steps(store),
        ids.join(", ")
    );
    store.query(&sql, &[])?.iter().map(step_of).collect()
}

/// Lay the step rows into the runs' `steps`.
fn with_steps(store: &mut Store, runs: &mut [Run]) -> Result<(), Error> {
    let ids: Vec<i64> = runs.iter().map(|r| r.id).collect();
    let rows = steps_of(store, &ids)?;
    for r in runs.iter_mut() {
        r.steps = Value::Array(
            rows.iter()
                .filter(|(run, ..)| *run == r.id)
                .map(|(_, _, v)| v.clone())
                .collect(),
        );
    }
    Ok(())
}

/// A workflow run to start.
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    pub workflow_id: i64,
    pub job_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub selection: Option<&'a str>,
    pub person: &'a Value,
    /// One document a step, in order; each becomes a `workflow_step` row
    /// at status `queued`.
    pub steps: &'a Value,
    pub principal: &'a str,
    pub actor: Option<&'a Value>,
    pub started_at: &'a str,
}

pub fn start(store: &mut Store, n: &NewRun<'_>) -> Result<i64, Error> {
    let rows = store.insert(
        &Insert::new(
            table("workflow_run"),
            &[
                "workflow_id",
                "job_id",
                "handle_id",
                "selection",
                "person",
                "status",
                "started_at",
                "principal",
                "actor",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::Int(n.workflow_id),
            n.job_id.map_or(Param::Null, Param::Int),
            n.handle_id.map_or(Param::Null, Param::Int),
            n.selection.map_or(Param::Null, Param::from),
            Param::from(n.person.to_string()),
            Param::from("running"),
            Param::from(n.started_at),
            Param::from(n.principal),
            n.actor.map_or(Param::Null, |a| Param::from(a.to_string())),
        ]],
    )?;
    let id = rows
        .first()
        .ok_or_else(|| Error::Message("the workflow run was not written back".into()))?
        .int(0)?;
    let steps: Vec<Vec<Param>> = n
        .steps
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, doc)| {
            let status = doc["status"].as_str().unwrap_or("queued").to_string();
            vec![
                Param::Int(id),
                Param::Int(i as i64),
                Param::from(doc.to_string()),
                Param::from(status),
            ]
        })
        .collect();
    if !steps.is_empty() {
        store.insert(
            &Insert::new(
                table("workflow_step"),
                &["workflow_run_id", "step", "doc", "status"],
            ),
            &steps,
        )?;
    }
    Ok(id)
}

pub fn run(store: &mut Store, id: i64) -> Result<Option<Run>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE id = {}",
        select_runs(store),
        d.param(1, Type::Int)
    );
    let Some(r) = store.query_opt(&sql, &[Param::Int(id)])? else {
        return Ok(None);
    };
    let mut runs = vec![run_of(&r)?];
    with_steps(store, &mut runs)?;
    Ok(runs.pop())
}

/// One step of a run as the chain recorded it, `(step, doc)`; none where
/// the run has no such step.
pub fn step(store: &mut Store, workflow_run: i64, step: usize) -> Result<Option<Value>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE workflow_run_id = {} AND step = {}",
        select_steps(store),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(workflow_run), Param::Int(step as i64)])?
        .map(|r| step_of(&r).map(|(_, _, v)| v))
        .transpose()
}

/// The runs, newest first, of one workflow (any version of it by id) or
/// all.
pub fn runs(store: &mut Store, workflow_id: Option<i64>, limit: usize) -> Result<Vec<Run>, Error> {
    let d = store.dialect();
    let mut sql = select_runs(store);
    let mut params = Vec::new();
    if let Some(w) = workflow_id {
        params.push(Param::Int(w));
        sql.push_str(&format!(" WHERE workflow_id = {}", d.param(1, Type::Int)));
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", limit.max(1)));
    let mut runs: Vec<Run> = store
        .query(&sql, &params)?
        .iter()
        .map(run_of)
        .collect::<Result<_, _>>()?;
    with_steps(store, &mut runs)?;
    Ok(runs)
}

/// What the chain records of one step as it goes; a field left none is
/// kept as it was.
#[derive(Debug, Clone, Default)]
pub struct StepUpdate<'a> {
    pub run_id: Option<i64>,
    pub status: Option<&'a str>,
    pub error: Option<&'a str>,
    pub units: Option<&'a Value>,
    pub finished_at: Option<&'a str>,
}

/// Record what the chain learned of one step, by (run, step): its own row,
/// so two steps recording at once never write over each other.
pub fn update_step(
    store: &mut Store,
    workflow_run: i64,
    step: usize,
    u: &StepUpdate<'_>,
) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE workflow_run_id = {} AND step = {}",
        store.qualified("workflow_step"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let id = store
        .query_opt(&sql, &[Param::Int(workflow_run), Param::Int(step as i64)])?
        .ok_or_else(|| Error::Message(format!("workflow run {workflow_run} has no step {step}")))?
        .int(0)?;
    let mut fields: Vec<(&str, Param)> = Vec::new();
    if let Some(r) = u.run_id {
        fields.push(("run_id", Param::Int(r)));
    }
    if let Some(s) = u.status {
        fields.push(("status", Param::from(s)));
    }
    if let Some(e) = u.error {
        fields.push(("error", Param::from(e)));
    }
    if let Some(v) = u.units {
        fields.push(("units", Param::from(v.to_string())));
    }
    if let Some(t) = u.finished_at {
        fields.push(("finished_at", Param::from(t)));
    }
    if fields.is_empty() {
        return Ok(());
    }
    store.update_by_id(table("workflow_step"), &fields, "id", id)?;
    Ok(())
}

/// A workflow run ended.
pub fn finish(
    store: &mut Store,
    id: i64,
    status: &str,
    summary: &Value,
    error: Option<&str>,
    finished_at: &str,
) -> Result<(), Error> {
    store.update_by_id(
        table("workflow_run"),
        &[
            ("status", Param::from(status)),
            ("summary", Param::from(summary.to_string())),
            ("error", error.map_or(Param::Null, Param::from)),
            ("finished_at", Param::from(finished_at)),
        ],
        "id",
        id,
    )?;
    Ok(())
}
