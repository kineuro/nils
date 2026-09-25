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

/// How a run stands. `partial` (record 43): the container exited 0 and
/// some of its units failed or went unreported, which are review items.
/// `interrupted` (record 49 A1): the engine that ran it went away with
/// units in flight, and the lane takes it up again.
pub const RUN_STATUSES: [&str; 6] = [
    "running",
    "done",
    "partial",
    "failed",
    "cancelled",
    "interrupted",
];

/// How a unit of a run stands (record 49 A1): queued until the lane starts
/// it, running while its container runs, registering while what it left is
/// taken in (a resume takes it in again, and runs nothing), over once that
/// is done, whatever its outcome.
pub const UNIT_STATES: [&str; 4] = ["queued", "running", "registering", "over"];

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
    /// `starter` for a version the engine seeded (record 49 A4), none for
    /// one a person added.
    pub origin: Option<String>,
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

const COLUMNS: [&str; 14] = [
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
    "origin",
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
        origin: r.opt_text(13)?.map(str::to_string),
    })
}

/// The origin a seeded entry carries (record 49 A4).
pub const STARTER: &str = "starter";

/// Mark an entry as the engine's own starter version.
pub fn set_origin(store: &mut Store, id: i64, origin: &str) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET origin = {} WHERE id = {}",
        store.qualified("pipeline"),
        d.param(1, Type::Text),
        d.param(2, Type::Int)
    );
    store.execute(&sql, &[Param::from(origin), Param::Int(id)])?;
    Ok(())
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
    /// together | apart; none on a run from before record 49, which ran
    /// its units together.
    pub units: Option<String>,
    pub resumes: Option<i64>,
    pub threshold: Option<f64>,
    /// Record 49 R7: the working place of its scratch, where it is not
    /// `place_id`'s; none on a run from before, which kept both there.
    pub scratch_place_id: Option<i64>,
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
    /// The scratch's working place, where it is not `place_id`.
    pub scratch_place_id: Option<i64>,
    pub principal: &'a str,
    pub actor: Option<&'a Value>,
    pub started_at: &'a str,
}

const RUN_COLUMNS: [&str; 28] = [
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
    "units",
    "resumes",
    "threshold",
    "scratch_place_id",
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
        units: r.opt_text(24)?.map(str::to_string),
        resumes: r.opt_int(25)?,
        threshold: r.opt_double(26)?,
        scratch_place_id: r.opt_int(27)?,
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
                "scratch_place_id",
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
            n.scratch_place_id.map_or(Param::Null, Param::Int),
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
    // record 43: the release says it is a run's input, which the history
    // leaves out unless asked
    store.update_by_id(
        table("release"),
        &[("purpose", Param::from(RUN_INPUT))],
        "id",
        release_id,
    )?;
    Ok(())
}

/// The purpose of a release a run materialised its bids input by.
pub const RUN_INPUT: &str = "run_input";

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

/// How a run meets its units and the threshold its caller gave, which a
/// resume keeps (record 49 A1).
pub fn set_units(
    store: &mut Store,
    id: i64,
    units: &str,
    threshold: Option<f64>,
) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_run"),
        &[
            ("units", Param::from(units)),
            ("threshold", threshold.map_or(Param::Null, Param::Double)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// A run taken up again under a new job: running, its end undone, one more
/// resume counted.
pub fn resume(store: &mut Store, id: i64, job_id: i64) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = 'running', job_id = {}, finished_at = NULL, error = NULL, \
         resumes = COALESCE(resumes, 0) + 1 WHERE id = {}",
        store.qualified("pipeline_run"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store.execute(&sql, &[Param::Int(job_id), Param::Int(id)])?;
    Ok(())
}

/// Mark a running run whose engine went away as interrupted, so the lane
/// queues it once to be taken up again. Answers whether this call did.
pub fn interrupt(store: &mut Store, id: i64, why: &str) -> Result<bool, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = 'interrupted', error = {} WHERE id = {} AND status = 'running'",
        store.qualified("pipeline_run"),
        d.param(1, Type::Text),
        d.param(2, Type::Int)
    );
    Ok(store.execute(&sql, &[Param::from(why), Param::Int(id)])? == 1)
}

/// The runs in a status, oldest first.
pub fn runs_in(store: &mut Store, status: &str) -> Result<Vec<Run>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE status = {} ORDER BY id",
        select_runs(store),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(status)])?
        .iter()
        .map(run_of)
        .collect()
}

/// One unit of a run, as the lane schedules it (record 49 A1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unit {
    pub id: i64,
    pub run_id: i64,
    pub unit: String,
    pub position: i64,
    pub state: String,
    pub attempts: i64,
    pub cores: Option<i64>,
    pub memory_mb: Option<i64>,
    pub gpu_card: Option<i64>,
    pub gpu_memory_mb: Option<i64>,
    pub device: Option<String>,
    pub container: Option<String>,
    pub pid: Option<i64>,
    pub host: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub outcome: Value,
}

const UNIT_COLUMNS: [&str; 18] = [
    "id",
    "run_id",
    "unit",
    "position",
    "state",
    "attempts",
    "cores",
    "memory_mb",
    "gpu_card",
    "gpu_memory_mb",
    "device",
    "container",
    "pid",
    "host",
    "started_at",
    "finished_at",
    "exit_code",
    "outcome",
];

fn unit_of(r: &Row) -> Result<Unit, Error> {
    Ok(Unit {
        id: r.int(0)?,
        run_id: r.int(1)?,
        unit: r.text(2)?.to_string(),
        position: r.int(3)?,
        state: r.text(4)?.to_string(),
        attempts: r.int(5)?,
        cores: r.opt_int(6)?,
        memory_mb: r.opt_int(7)?,
        gpu_card: r.opt_int(8)?,
        gpu_memory_mb: r.opt_int(9)?,
        device: r.opt_text(10)?.map(str::to_string),
        container: r.opt_text(11)?.map(str::to_string),
        pid: r.opt_int(12)?,
        host: r.opt_text(13)?.map(str::to_string),
        started_at: r.opt_text(14)?.map(str::to_string),
        finished_at: r.opt_text(15)?.map(str::to_string),
        exit_code: r.opt_int(16)?,
        outcome: json(r.opt_text(17)?),
    })
}

/// The units of a run, in their order.
pub fn units(store: &mut Store, run_id: i64) -> Result<Vec<Unit>, Error> {
    let d = store.dialect();
    let t = table("pipeline_unit");
    let cols: Vec<String> = UNIT_COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a pipeline_unit column")))
        .collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE run_id = {} ORDER BY position, id",
        cols.join(", "),
        store.qualified("pipeline_unit"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(run_id)])?
        .iter()
        .map(unit_of)
        .collect()
}

/// Write the units of a run that it has no row for yet, queued, in the
/// order given; the rows it has stand as they are. Answers every unit.
pub fn ensure_units(store: &mut Store, run_id: i64, names: &[String]) -> Result<Vec<Unit>, Error> {
    let have: std::collections::BTreeSet<String> =
        units(store, run_id)?.into_iter().map(|u| u.unit).collect();
    let rows: Vec<Vec<Param>> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !have.contains(*n))
        .map(|(i, n)| {
            vec![
                Param::Int(run_id),
                Param::from(n.as_str()),
                Param::Int(i as i64),
                Param::from("queued"),
                Param::Int(0),
            ]
        })
        .collect();
    for chunk in rows.chunks(500) {
        store.insert(
            &Insert::new(
                table("pipeline_unit"),
                &["run_id", "unit", "position", "state", "attempts"],
            ),
            chunk,
        )?;
    }
    units(store, run_id)
}

/// What a unit holds while it runs.
#[derive(Debug, Clone)]
pub struct UnitStart<'a> {
    pub cores: i64,
    pub memory_mb: i64,
    pub gpu_card: Option<i64>,
    pub gpu_memory_mb: Option<i64>,
    pub device: &'a str,
    pub container: &'a str,
    pub pid: Option<i64>,
    pub host: &'a str,
    pub started_at: &'a str,
}

/// A unit started: running, one more attempt, with what it holds.
pub fn unit_started(store: &mut Store, id: i64, s: &UnitStart<'_>) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET state = 'running', attempts = attempts + 1, cores = {}, memory_mb = {}, \
         gpu_card = {}, gpu_memory_mb = {}, device = {}, container = {}, pid = {}, host = {}, \
         started_at = {}, finished_at = NULL, exit_code = NULL, outcome = NULL WHERE id = {}",
        store.qualified("pipeline_unit"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
        d.param(5, Type::Text),
        d.param(6, Type::Text),
        d.param(7, Type::Int),
        d.param(8, Type::Text),
        d.param(9, Type::Timestamp),
        d.param(10, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::Int(s.cores),
            Param::Int(s.memory_mb),
            s.gpu_card.map_or(Param::Null, Param::Int),
            s.gpu_memory_mb.map_or(Param::Null, Param::Int),
            Param::from(s.device),
            Param::from(s.container),
            s.pid.map_or(Param::Null, Param::Int),
            Param::from(s.host),
            Param::from(s.started_at),
            Param::Int(id),
        ],
    )?;
    Ok(())
}

/// A unit over: its exit code and what became of it.
pub fn unit_over(
    store: &mut Store,
    id: i64,
    exit_code: Option<i64>,
    outcome: &Value,
    finished_at: &str,
) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_unit"),
        &[
            ("state", Param::from("over")),
            ("exit_code", exit_code.map_or(Param::Null, Param::Int)),
            ("outcome", Param::from(outcome.to_string())),
            ("finished_at", Param::from(finished_at)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// A unit whose container ended: what it left is being taken in, and its
/// exit code is kept for a resume that takes it in again.
pub fn unit_registering(store: &mut Store, id: i64, exit_code: Option<i64>) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_unit"),
        &[
            ("state", Param::from("registering")),
            ("exit_code", exit_code.map_or(Param::Null, Param::Int)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// A unit that was in flight queued again, holding nothing: how a stopped
/// run gives its running units back, and a resume reruns them.
pub fn unit_requeued(store: &mut Store, id: i64) -> Result<(), Error> {
    store.update_by_id(
        table("pipeline_unit"),
        &[
            ("state", Param::from("queued")),
            ("gpu_card", Param::Null),
            ("gpu_memory_mb", Param::Null),
            ("pid", Param::Null),
            ("container", Param::Null),
        ],
        "id",
        id,
    )?;
    Ok(())
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
                scratch_place_id: None,
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

    /// Record 49 A1: a run's units are written once, started and ended one
    /// by one, and a unit in flight is queued again with nothing held.
    #[test]
    fn a_run_s_units_are_written_once_and_taken_up_again() {
        let mut store = store();
        let d = serde_json::json!({});
        let (p, _) = add(&mut store, &new(&d, "sha256:1")).unwrap();
        let params = serde_json::json!({});
        let id = start(
            &mut store,
            &NewRun {
                pipeline_id: p.id,
                job_id: Some(1),
                handle_id: None,
                selection: None,
                params: &params,
                runtime: "podman",
                runtime_version: "5",
                host: "b",
                device: "cpu",
                model_ids: &[],
                label_set_id: None,
                place_id: None,
                scratch_place_id: None,
                principal: "ops@lab",
                actor: None,
                started_at: "2026-09-24T00:00:00Z",
            },
        )
        .unwrap();
        set_units(&mut store, id, "apart", Some(0.9)).unwrap();
        let names: Vec<String> = ["stack-1", "stack-2", "stack-3"].map(String::from).to_vec();
        let us = ensure_units(&mut store, id, &names).unwrap();
        assert_eq!(us.len(), 3);
        assert!(us.iter().all(|u| u.state == "queued" && u.attempts == 0));
        // written once: a second call adds nothing
        assert_eq!(ensure_units(&mut store, id, &names).unwrap(), us);
        fn start_of(c: &str) -> UnitStart<'_> {
            UnitStart {
                cores: 2,
                memory_mb: 2048,
                gpu_card: Some(1),
                gpu_memory_mb: Some(4096),
                device: "cuda:x",
                container: c,
                pid: Some(4242),
                host: "b",
                started_at: "2026-09-24T00:00:01Z",
            }
        }
        unit_started(&mut store, us[0].id, &start_of("nils-run-1-a")).unwrap();
        unit_started(&mut store, us[1].id, &start_of("nils-run-1-b")).unwrap();
        unit_over(
            &mut store,
            us[0].id,
            Some(0),
            &serde_json::json!({"status": "succeeded"}),
            "2026-09-24T00:00:02Z",
        )
        .unwrap();
        unit_requeued(&mut store, us[1].id).unwrap();
        let now = units(&mut store, id).unwrap();
        assert_eq!(now[0].state, "over");
        assert_eq!(now[0].outcome["status"], "succeeded");
        assert_eq!(now[0].gpu_card, Some(1));
        assert_eq!((now[1].state.as_str(), now[1].attempts), ("queued", 1));
        assert_eq!((now[1].gpu_card, now[1].pid), (None, None));
        let r = run(&mut store, id).unwrap().unwrap();
        assert_eq!(
            (r.units.as_deref(), r.threshold),
            (Some("apart"), Some(0.9))
        );
        assert!(interrupt(&mut store, id, "gone").unwrap());
        assert!(!interrupt(&mut store, id, "gone").unwrap());
        assert_eq!(runs_in(&mut store, "interrupted").unwrap().len(), 1);
        resume(&mut store, id, 9).unwrap();
        let r = run(&mut store, id).unwrap().unwrap();
        assert_eq!(
            (r.status.as_str(), r.job_id, r.resumes),
            ("running", Some(9), Some(1))
        );
    }
}
