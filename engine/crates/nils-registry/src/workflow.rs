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

/// Add a document: the name's next version, or the version that already
/// has this digest. Answers the entry and whether it is new.
pub fn add(
    store: &mut Store,
    name: &str,
    document: &Value,
    digest: &str,
    added_by: &str,
    added_at: &str,
    origin: Option<&str>,
) -> Result<(Workflow, bool), Error> {
    let versions = versions(store, name)?;
    if let Some(same) = versions.iter().find(|w| w.digest == digest) {
        return Ok((same.clone(), false));
    }
    let version = versions.iter().map(|w| w.version).max().unwrap_or(0) + 1;
    let rows = store.insert(
        &Insert::new(
            table("workflow"),
            &[
                "name", "version", "document", "digest", "state", "added_by", "added_at", "origin",
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
    )?;
    let id = rows
        .first()
        .ok_or_else(|| Error::Message("the workflow was not written back".into()))?
        .int(0)?;
    let w = get(store, id)?.ok_or_else(|| Error::Message(format!("no workflow {id}")))?;
    Ok((w, true))
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
    /// per step of the document, in order.
    pub steps: Value,
    pub summary: Value,
    pub principal: String,
    pub actor: Value,
    pub error: Option<String>,
}

const RUN_COLUMNS: [&str; 14] = [
    "id",
    "workflow_id",
    "job_id",
    "handle_id",
    "selection",
    "person",
    "status",
    "started_at",
    "finished_at",
    "steps",
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
        steps: json(r.opt_text(9)?),
        summary: json(r.opt_text(10)?),
        principal: r.text(11)?.to_string(),
        actor: json(r.opt_text(12)?),
        error: r.opt_text(13)?.map(str::to_string),
    })
}

/// A workflow run to start.
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    pub workflow_id: i64,
    pub job_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub selection: Option<&'a str>,
    pub person: &'a Value,
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
                "steps",
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
            Param::from(n.steps.to_string()),
            Param::from(n.principal),
            n.actor.map_or(Param::Null, |a| Param::from(a.to_string())),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| Error::Message("the workflow run was not written back".into()))?
        .int(0)
}

pub fn run(store: &mut Store, id: i64) -> Result<Option<Run>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE id = {}",
        select_runs(store),
        d.param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| run_of(&r))
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
    store.query(&sql, &params)?.iter().map(run_of).collect()
}

/// The steps as the chain records them, whole.
pub fn set_steps(store: &mut Store, id: i64, steps: &Value) -> Result<(), Error> {
    store.update_by_id(
        table("workflow_run"),
        &[("steps", Param::from(steps.to_string()))],
        "id",
        id,
    )?;
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
