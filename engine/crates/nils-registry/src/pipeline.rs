// SPDX-License-Identifier: AGPL-3.0-only

//! Pipelines (record 43 S1 and S2): the catalog of descriptors and the runs.
//!
//! A pipeline is a `nils.job.yml` (`contracts/job/v1`), kept whole with the
//! digest of its canonical JSON, its image pinned by a registry manifest
//! digest. A name's versions are the descriptors that differ, numbered from
//! one; the same descriptor added again is the version it already is. A run
//! is one pipeline over one frozen selection, recorded with everything it
//! needs to be run again. Nothing is deleted.
//!
//! This module is the rows. Checking a descriptor is `nils-pipeline`'s, and
//! running one, with its input, its container and its outputs, the binary's.

use serde::Serialize;
use serde_json::Value;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};

/// The states of a catalog entry.
pub const STATES: [&str; 2] = ["active", "retired"];

/// The states of a run.
pub const RUN_STATUSES: [&str; 4] = ["running", "done", "failed", "cancelled"];

/// One version of a pipeline in the catalog.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pipeline {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub tool_version: String,
    pub descriptor: Value,
    pub descriptor_digest: String,
    pub image: String,
    pub image_digest: String,
    pub layout: String,
    pub level: String,
    pub state: String,
    pub added_by: String,
    pub added_at: String,
}

impl Pipeline {
    /// `name@version`.
    pub fn label(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// A descriptor to add, checked already.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub name: &'a str,
    pub tool_version: &'a str,
    pub descriptor: &'a Value,
    pub descriptor_digest: &'a str,
    pub image: &'a str,
    pub image_digest: &'a str,
    pub layout: &'a str,
    pub level: &'a str,
    pub added_by: &'a str,
    pub added_at: &'a str,
}

const COLUMNS: [&str; 13] = [
    "id",
    "name",
    "version",
    "tool_version",
    "descriptor",
    "descriptor_digest",
    "image",
    "image_digest",
    "layout",
    "level",
    "state",
    "added_by",
    "added_at",
];

fn select(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("pipeline");
    let cols: Vec<String> = COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a pipeline column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("pipeline")
    )
}

fn json(text: Option<&str>) -> Value {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

fn of(r: &Row) -> Result<Pipeline, Error> {
    Ok(Pipeline {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        version: r.int(2)?,
        tool_version: r.text(3)?.to_string(),
        descriptor: json(r.opt_text(4)?),
        descriptor_digest: r.text(5)?.to_string(),
        image: r.text(6)?.to_string(),
        image_digest: r.text(7)?.to_string(),
        layout: r.text(8)?.to_string(),
        level: r.text(9)?.to_string(),
        state: r.text(10)?.to_string(),
        added_by: r.text(11)?.to_string(),
        added_at: r.text(12)?.to_string(),
    })
}

/// Add a descriptor: the name's next version, or the version that already
/// has this digest. Answers the entry and whether it is new.
pub fn add(store: &mut Store, n: &New<'_>) -> Result<(Pipeline, bool), Error> {
    let versions = versions(store, n.name)?;
    if let Some(same) = versions
        .iter()
        .find(|p| p.descriptor_digest == n.descriptor_digest)
    {
        return Ok((same.clone(), false));
    }
    let version = versions.iter().map(|p| p.version).max().unwrap_or(0) + 1;
    let rows = store.insert(
        &Insert::new(
            table("pipeline"),
            &[
                "name",
                "version",
                "tool_version",
                "descriptor",
                "descriptor_digest",
                "image",
                "image_digest",
                "layout",
                "level",
                "state",
                "added_by",
                "added_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(n.name),
            Param::Int(version),
            Param::from(n.tool_version),
            Param::from(n.descriptor.to_string()),
            Param::from(n.descriptor_digest),
            Param::from(n.image),
            Param::from(n.image_digest),
            Param::from(n.layout),
            Param::from(n.level),
            Param::from("active"),
            Param::from(n.added_by),
            Param::from(n.added_at),
        ]],
    )?;
    let id = rows
        .first()
        .ok_or_else(|| Error::Message("the pipeline was not written back".into()))?
        .int(0)?;
    let p = get(store, id)?.ok_or_else(|| Error::Message(format!("no pipeline {id}")))?;
    Ok((p, true))
}

/// Every version of one name, oldest first.
pub fn versions(store: &mut Store, name: &str) -> Result<Vec<Pipeline>, Error> {
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

/// One entry by id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Pipeline>, Error> {
    let d = store.dialect();
    let sql = format!("{} WHERE id = {}", select(store), d.param(1, Type::Int));
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| of(&r))
        .transpose()
}

/// An entry as a person names it: its id, `name@version`, or a name, which
/// is its newest active version.
pub fn resolve(store: &mut Store, reference: &str) -> Result<Option<Pipeline>, Error> {
    let reference = reference.trim();
    if let Ok(id) = reference.parse::<i64>() {
        return get(store, id);
    }
    if let Some((name, version)) = reference.rsplit_once('@') {
        let Ok(v) = version.parse::<i64>() else {
            return Ok(None);
        };
        return Ok(versions(store, name)?.into_iter().find(|p| p.version == v));
    }
    Ok(versions(store, reference)?
        .into_iter()
        .rev()
        .find(|p| p.state == "active"))
}

/// The catalog, by name and version.
pub fn list(store: &mut Store) -> Result<Vec<Pipeline>, Error> {
    let sql = format!("{} ORDER BY name, version", select(store));
    store.query(&sql, &[])?.iter().map(of).collect()
}

/// One run as the registry holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Run {
    pub id: i64,
    pub pipeline_id: i64,
    pub job_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub selection: Option<String>,
    pub params: Value,
    pub runtime: String,
    pub runtime_version: String,
    pub host: String,
    pub device: String,
    pub model_ids: Value,
    pub label_set_id: Option<i64>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub results_digest: Option<String>,
    pub place_id: Option<i64>,
    pub output: Option<String>,
    pub input_release_id: Option<i64>,
    pub summary: Value,
    pub principal: String,
    pub actor: Value,
    pub error: Option<String>,
}

/// A run to start.
#[derive(Debug, Clone)]
pub struct NewRun<'a> {
    pub pipeline_id: i64,
    pub job_id: Option<i64>,
    pub handle_id: Option<i64>,
    pub selection: Option<&'a str>,
    pub params: &'a Value,
    pub runtime: &'a str,
    pub runtime_version: &'a str,
    pub host: &'a str,
    pub device: &'a str,
    pub model_ids: &'a [i64],
    pub label_set_id: Option<i64>,
    pub place_id: Option<i64>,
    pub principal: &'a str,
    pub actor: Option<&'a Value>,
    pub started_at: &'a str,
}

const RUN_COLUMNS: [&str; 24] = [
    "id",
    "pipeline_id",
    "job_id",
    "handle_id",
    "selection",
    "params",
    "runtime",
    "runtime_version",
    "host",
    "device",
    "model_ids",
    "label_set_id",
    "status",
    "started_at",
    "finished_at",
    "exit_code",
    "results_digest",
    "place_id",
    "output",
    "input_release_id",
    "summary",
    "principal",
    "actor",
    "error",
];

fn select_runs(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("pipeline_run");
    let cols: Vec<String> = RUN_COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a pipeline_run column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("pipeline_run")
    )
}

fn run_of(r: &Row) -> Result<Run, Error> {
    Ok(Run {
        id: r.int(0)?,
        pipeline_id: r.int(1)?,
        job_id: r.opt_int(2)?,
        handle_id: r.opt_int(3)?,
        selection: r.opt_text(4)?.map(str::to_string),
        params: json(r.opt_text(5)?),
        runtime: r.text(6)?.to_string(),
        runtime_version: r.text(7)?.to_string(),
        host: r.text(8)?.to_string(),
        device: r.text(9)?.to_string(),
        model_ids: json(r.opt_text(10)?),
        label_set_id: r.opt_int(11)?,
        status: r.text(12)?.to_string(),
        started_at: r.text(13)?.to_string(),
        finished_at: r.opt_text(14)?.map(str::to_string),
        exit_code: r.opt_int(15)?,
        results_digest: r.opt_text(16)?.map(str::to_string),
        place_id: r.opt_int(17)?,
        output: r.opt_text(18)?.map(str::to_string),
        input_release_id: r.opt_int(19)?,
        summary: json(r.opt_text(20)?),
        principal: r.text(21)?.to_string(),
        actor: json(r.opt_text(22)?),
        error: r.opt_text(23)?.map(str::to_string),
    })
}

/// Write a run as `running`. Answers its id.
pub fn start(store: &mut Store, n: &NewRun<'_>) -> Result<i64, Error> {
    let rows = store.insert(
        &Insert::new(
            table("pipeline_run"),
            &[
                "pipeline_id",
                "job_id",
                "handle_id",
                "selection",
                "params",
                "runtime",
                "runtime_version",
                "host",
                "device",
                "model_ids",
                "label_set_id",
                "status",
                "started_at",
                "place_id",
                "principal",
                "actor",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::Int(n.pipeline_id),
            n.job_id.map_or(Param::Null, Param::Int),
            n.handle_id.map_or(Param::Null, Param::Int),
            n.selection.map_or(Param::Null, Param::from),
            Param::from(n.params.to_string()),
            Param::from(n.runtime),
            Param::from(n.runtime_version),
            Param::from(n.host),
            Param::from(n.device),
            Param::from(serde_json::json!(n.model_ids).to_string()),
            n.label_set_id.map_or(Param::Null, Param::Int),
            Param::from("running"),
            Param::from(n.started_at),
            n.place_id.map_or(Param::Null, Param::Int),
            Param::from(n.principal),
            n.actor.map_or(Param::Null, |a| Param::from(a.to_string())),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| Error::Message("the run was not written back".into()))?
        .int(0)
}

/// Where a run's outputs go, under its place, once the folder is made.
pub fn set_output(store: &mut Store, id: i64, output: &str) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_run"),
        &[("output", Param::from(output))],
        "id",
        id,
    )?;
    Ok(())
}

/// The release that materialised a run's bids input.
pub fn set_input_release(store: &mut Store, id: i64, release_id: i64) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_run"),
        &[("input_release_id", Param::Int(release_id))],
        "id",
        id,
    )?;
    Ok(())
}

/// How a run ended.
#[derive(Debug, Clone)]
pub struct Finish<'a> {
    pub status: &'a str,
    pub finished_at: &'a str,
    pub exit_code: Option<i64>,
    pub results_digest: Option<&'a str>,
    pub summary: &'a Value,
    pub error: Option<&'a str>,
}

/// Close a run.
pub fn finish(store: &mut Store, id: i64, f: &Finish<'_>) -> Result<(), Error> {
    if !RUN_STATUSES.contains(&f.status) || f.status == "running" {
        return Err(Error::Message(format!(
            "{} is not how a run ends",
            f.status
        )));
    }
    store.update_by_id(
        table("pipeline_run"),
        &[
            ("status", Param::from(f.status)),
            ("finished_at", Param::from(f.finished_at)),
            ("exit_code", f.exit_code.map_or(Param::Null, Param::Int)),
            (
                "results_digest",
                f.results_digest.map_or(Param::Null, Param::from),
            ),
            ("summary", Param::from(f.summary.to_string())),
            ("error", f.error.map_or(Param::Null, Param::from)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// One run by id.
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

/// The runs, newest first, of one pipeline (any version of it by id) or
/// all.
pub fn runs(store: &mut Store, pipeline_id: Option<i64>, limit: usize) -> Result<Vec<Run>, Error> {
    let d = store.dialect();
    let mut sql = select_runs(store);
    let mut params = Vec::new();
    if let Some(p) = pipeline_id {
        params.push(Param::Int(p));
        sql.push_str(&format!(" WHERE pipeline_id = {}", d.param(1, Type::Int)));
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", limit.max(1)));
    store.query(&sql, &params)?.iter().map(run_of).collect()
}

/// How many entries and runs the registry keeps, for custody.
pub fn totals(store: &mut Store) -> Result<(i64, i64), Error> {
    let count = |store: &mut Store, t: &str| -> Result<i64, Error> {
        let sql = format!("SELECT COUNT(*) FROM {}", store.qualified(t));
        store
            .query_opt(&sql, &[])?
            .ok_or_else(|| Error::Message("no count".into()))?
            .int(0)
    };
    Ok((count(store, "pipeline")?, count(store, "pipeline_run")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};

    fn store() -> Store {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        store
    }

    fn new<'a>(descriptor: &'a Value, digest: &'a str) -> New<'a> {
        New {
            name: "n4",
            tool_version: "2.6",
            descriptor,
            descriptor_digest: digest,
            image: "antsx/ants@sha256:00",
            image_digest: "sha256:00",
            layout: "bids",
            level: "session",
            added_by: "ops@lab",
            added_at: "2026-09-24T00:00:00Z",
        }
    }

    #[test]
    fn a_name_s_versions_are_the_descriptors_that_differ() {
        let mut store = store();
        let one = serde_json::json!({"name": "n4", "a": 1});
        let two = serde_json::json!({"name": "n4", "a": 2});
        let (first, fresh) = add(&mut store, &new(&one, "sha256:1")).unwrap();
        assert!(fresh);
        assert_eq!(first.version, 1);
        let (again, fresh) = add(&mut store, &new(&one, "sha256:1")).unwrap();
        assert!(!fresh);
        assert_eq!(again.id, first.id);
        let (second, _) = add(&mut store, &new(&two, "sha256:2")).unwrap();
        assert_eq!(second.version, 2);
        assert_eq!(second.descriptor, two);
        assert_eq!(resolve(&mut store, "n4").unwrap().unwrap().id, second.id);
        assert_eq!(resolve(&mut store, "n4@1").unwrap().unwrap().id, first.id);
        assert_eq!(
            resolve(&mut store, &first.id.to_string())
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert!(resolve(&mut store, "n4@9").unwrap().is_none());
        assert!(resolve(&mut store, "other").unwrap().is_none());
        assert_eq!(list(&mut store).unwrap().len(), 2);
    }

    #[test]
    fn a_run_is_written_running_and_closed_with_what_it_did() {
        let mut store = store();
        let d = serde_json::json!({});
        let (p, _) = add(&mut store, &new(&d, "sha256:1")).unwrap();
        let params = serde_json::json!({"dimension": 3, "shrink": null});
        let id = start(
            &mut store,
            &NewRun {
                pipeline_id: p.id,
                job_id: Some(4),
                handle_id: Some(9),
                selection: Some("selection:every@1"),
                params: &params,
                runtime: "podman",
                runtime_version: "5.0",
                host: "b",
                device: "cpu",
                model_ids: &[2, 3],
                label_set_id: None,
                place_id: Some(1),
                principal: "ops@lab",
                actor: None,
                started_at: "2026-09-24T00:00:00Z",
            },
        )
        .unwrap();
        let r = run(&mut store, id).unwrap().unwrap();
        assert_eq!(r.status, "running");
        assert_eq!(r.params, params);
        assert_eq!(r.model_ids, serde_json::json!([2, 3]));
        set_output(&mut store, id, "derivatives/n4/1").unwrap();
        set_input_release(&mut store, id, 5).unwrap();
        assert!(
            finish(
                &mut store,
                id,
                &Finish {
                    status: "running",
                    finished_at: "x",
                    exit_code: None,
                    results_digest: None,
                    summary: &Value::Null,
                    error: None
                }
            )
            .is_err()
        );
        let summary = serde_json::json!({"units": {"succeeded": 2}});
        finish(
            &mut store,
            id,
            &Finish {
                status: "done",
                finished_at: "2026-09-24T00:01:00Z",
                exit_code: Some(0),
                results_digest: Some("sha256:ab"),
                summary: &summary,
                error: None,
            },
        )
        .unwrap();
        let r = run(&mut store, id).unwrap().unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.output.as_deref(), Some("derivatives/n4/1"));
        assert_eq!(r.input_release_id, Some(5));
        assert_eq!(r.summary, summary);
        assert_eq!(runs(&mut store, Some(p.id), 10).unwrap().len(), 1);
        assert_eq!(totals(&mut store).unwrap(), (1, 1));
    }
}
