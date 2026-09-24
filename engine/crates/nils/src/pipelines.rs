// SPDX-License-Identifier: AGPL-3.0-only

//! Pipelines at the keyboard, the doors and the queue (record 43 S1 to S3).
//!
//! The catalog: `nils pipeline add <nils.job.yml>` checks a descriptor
//! (`contracts/job/v1`, through `nils-pipeline`) and keeps it as the name's
//! next version, refusing an image not pinned by its registry manifest
//! digest; `list` and `show` read it back.
//!
//! The runner: `nils run <pipeline> --select selection:<name>@<v>` is one run
//! over a frozen selection, a job of kind `pipeline` that the queue's worker
//! runs one at a time (R1), queued at `POST /api/jobs` under `pipelines:work`
//! at detail quasi, since a pipeline reads pixels. In order:
//!
//! 1. The descriptor, its parameters with the defaults filled, the models
//!    and the label set it reads, the working place and a runtime (rootless
//!    podman, apptainer, docker only by an operator's choice; none is the
//!    capability off, D1), and the GPU it asks for.
//! 2. The selection frozen to a handle, which the run pins.
//! 3. The input materialised under `<working>/runs/<run>/`: a release in
//!    the BIDS layout with the picks applied (R4), or `stacks.json` naming
//!    each stack's files under the source places, mounted read-only.
//! 4. The container: no network, the input and the typed inputs read-only,
//!    one output folder, `<working>/derivatives/<pipeline>/<run>/`, and a
//!    process that is not root on the host.
//! 5. After it exits: `results.json` (or, without one, each unit's files
//!    found by the declared templates), every file hashed by the engine and
//!    registered as a derivative naming the run, the proposals passed to
//!    the review spine's hook, a `pipeline:qc` review item for each unit
//!    that failed or went unreported, and the run closed with the digest of
//!    what it said.
//!
//! The doors read the catalog and the runs; the derivatives a run made are
//! served by the derivative doors, by bytes or by a shared path (S3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nils_pipeline::descriptor::{self, Descriptor, Gpu, Layout, Level};
use nils_pipeline::runtime::{self, Choice, Detected, Invocation, Mount, Runtime};
use nils_registry::derivative::{self, Belongs};
use nils_registry::home::Home;
use nils_registry::job;
use nils_registry::pipeline::{self as rows, Pipeline, Run};
use nils_registry::schema::Type;
use nils_registry::store::Store;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::Reply;
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 4] = [
    "GET /api/pipelines",
    "GET /api/pipelines/{id}",
    "GET /api/pipeline-runs",
    "GET /api/pipeline-runs/{id}",
];

/// Where the registry keeps the operator's choice of runtime.
pub(crate) const RUNTIME_KEY: &str = "pipeline_runtime";

/// Where a run's input, typed inputs and log go under the working place.
pub(crate) const RUNS: &str = "runs";

/// The operator's choice of runtime: `auto` until one is set.
pub(crate) fn choice(registry: &mut Registry) -> Choice {
    registry
        .meta_value(RUNTIME_KEY)
        .ok()
        .flatten()
        .and_then(|v| Choice::parse(&v))
        .unwrap_or(Choice::Auto)
}

/// What the look for a runtime found, kept half a minute, since the
/// capabilities are read often and asking podman is not free.
fn detect_cached(registry: &mut Registry) -> Detected {
    static CACHE: Mutex<Option<(Instant, Choice, Detected)>> = Mutex::new(None);
    let chosen = choice(registry);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, c, d)) = cache.as_ref()
        && *c == chosen
        && at.elapsed() < Duration::from_secs(30)
    {
        return d.clone();
    }
    let d = runtime::detect(chosen, std::env::var_os("PATH").as_deref());
    *cache = Some((Instant::now(), chosen, d.clone()));
    d
}

/// What `GET /api/capabilities` says of pipelines: on when a runtime and a
/// working place are both there, off with the sentence that names the cure
/// when either is not (D1).
pub(crate) fn capability(registry: &mut Registry) -> Value {
    let d = detect_cached(registry);
    capability_of(registry.store(), &d)
}

fn capability_of(store: &mut Store, d: &Detected) -> Value {
    let place = crate::derivatives::working(store);
    let catalog = rows::list(store)
        .map(|l| l.iter().filter(|p| p.state == "active").count())
        .unwrap_or(0);
    let reason = d.reason.clone().or_else(|| {
        place
            .as_ref()
            .err()
            .map(|m| m.replacen("derivatives are off", "pipelines are off", 1))
    });
    json!({
        "enabled": d.runtime.is_some() && place.is_ok(),
        "reason": reason,
        "runtime": d.runtime.as_ref().map(|r| json!({
            "name": r.kind.name(), "version": r.version, "gpu": r.gpu,
        })),
        "choice": d.choice.name(),
        "looked": d.looked.iter().map(|(k, s)| json!({"runtime": k, "found": s})).collect::<Vec<_>>(),
        "place": place.ok().map(|p| p.name),
        "pipelines": catalog,
        "contract": nils_pipeline::CONTRACT,
        "grants": {"see": "pipelines:see", "run": "pipelines:work", "run_detail": "quasi"},
    })
}

/// A catalog entry as the doors and `--json` answer it.
pub(crate) fn pipeline_doc(p: &Pipeline) -> Value {
    let x = &p.descriptor["x-nils"];
    json!({
        "id": p.id, "name": p.name, "version": p.version, "label": p.label(),
        "tool_version": p.tool_version, "description": p.descriptor["description"],
        "image": p.image, "image_digest": p.image_digest,
        "descriptor_digest": p.descriptor_digest, "layout": p.layout, "level": p.level,
        "state": p.state, "added_by": p.added_by, "added_at": p.added_at,
        "parameters": p.descriptor["inputs"], "inputs": x["inputs"], "outputs": x["outputs"],
        "needs": x["needs"], "proposals": x["proposals"],
        "descriptor": p.descriptor,
    })
}

/// A run as the doors and `--json` answer it: the row, the pipeline by its
/// label, and the derivatives it made.
pub(crate) fn run_doc(store: &mut Store, r: &Run) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
    v["pipeline"] = json!(
        rows::get(store, r.pipeline_id)
            .ok()
            .flatten()
            .map(|p| p.label())
    );
    let made = derivative::list(
        store,
        &derivative::Filter {
            run_id: Some(r.id),
            limit: 10_000,
            ..derivative::Filter::default()
        },
    )
    .unwrap_or_default();
    v["derivatives"] = json!(made.iter().map(|d| d.id).rev().collect::<Vec<_>>());
    v
}

// ---------------------------------------------------------------- the doors

/// The reading doors: the catalog and the runs.
pub(crate) fn route(
    registry: &mut Registry,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get {
        return None;
    }
    let id_of = |s: &str, what: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, format!("a {what} is named by its id")))
    };
    let err = |e: nils_registry::Error| Reply::error(500, e.to_string());
    Some(match segs {
        ["api", "pipelines"] => (|| {
            let list = rows::list(registry.store()).map_err(err)?;
            let docs: Vec<Value> = list.iter().map(pipeline_doc).collect();
            Ok(Reply::ok(json!({
                "pipelines": docs,
                "capability": capability(registry),
            })))
        })(),
        ["api", "pipelines", id] => (|| {
            let id = id_of(id, "pipeline")?;
            let p = rows::get(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline {id}")))?;
            Ok(Reply::ok(pipeline_doc(&p)))
        })(),
        ["api", "pipeline-runs"] => (|| {
            let pipeline = query
                .get("pipeline")
                .map(|p| p.parse::<i64>())
                .transpose()
                .map_err(|_| Reply::error(400, "pipeline is a pipeline's id"))?;
            let limit = query
                .get("limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(50);
            let list = rows::runs(registry.store(), pipeline, limit).map_err(err)?;
            let store = registry.store();
            let docs: Vec<Value> = list.iter().map(|r| run_doc(store, r)).collect();
            Ok(Reply::ok(json!({ "runs": docs })))
        })(),
        ["api", "pipeline-runs", id] => (|| {
            let id = id_of(id, "run")?;
            let r = rows::run(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline run {id}")))?;
            Ok(Reply::ok(run_doc(registry.store(), &r)))
        })(),
        _ => return None,
    })
}

/// A `run` a door queues: the pipeline and the flags a caller may give,
/// and the deployment's pack directory; never a path a caller composes.
pub(crate) fn located(pack_dir: Option<&Path>, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let mut out = vec!["run".to_string()];
    let mut named = 0;
    let mut it = command.into_iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--select" | "--handle" | "--param" | "--model" | "--labels" | "--pack" => {
                let value = it
                    .next()
                    .ok_or_else(|| Reply::error(400, format!("run {arg} takes a value")))?;
                out.push(arg);
                out.push(value);
            }
            a if a.starts_with('-') => {
                return Err(Reply::error(
                    400,
                    format!(
                        "run takes --select, --handle, --param, --model, --labels and --pack, not {a}"
                    ),
                ));
            }
            _ => {
                named += 1;
                if named > 1 {
                    return Err(Reply::error(400, "run names one pipeline"));
                }
                out.push(arg);
            }
        }
    }
    if named == 0 {
        return Err(Reply::error(400, "run <pipeline>: the pipeline to run"));
    }
    if let Some(d) = pack_dir {
        out.push("--pack-dir".into());
        out.push(d.display().to_string());
    }
    Ok(out)
}

// --------------------------------------------------------- the command line

/// `nils pipeline`: the catalog and the runtime.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PipelineCommand {
    /// Add a descriptor (nils.job.yml, contracts/job/v1) to the catalog, as
    /// its name's next version; an image not pinned by its registry
    /// manifest digest is refused
    Add {
        /// The descriptor
        file: PathBuf,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The catalog, by name and version, and whether pipelines can run here
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One pipeline: its image, parameters, inputs, outputs and needs
    Show {
        /// Its id, name@version, or a name for its newest version
        pipeline: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The container runtime pipelines run under: what was found, or the
    /// operator's choice. Docker is taken only when chosen here (D18)
    Runtime {
        /// auto (rootless podman, then apptainer), podman, apptainer, docker or off
        #[arg(long, value_name = "CHOICE")]
        set: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The runs, newest first, or one run with its derivatives
    Runs {
        /// One run, by id
        id: Option<i64>,
        /// Only this pipeline's runs: its id, name@version or name
        #[arg(long, value_name = "PIPELINE")]
        pipeline: Option<String>,
        /// At most this many
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils run`: one pipeline over a frozen selection.
#[derive(Debug, clap::Args)]
pub(crate) struct RunArgs {
    /// The pipeline: its id, name@version, or a name for its newest version
    pub(crate) pipeline: String,
    /// The selection to run over, frozen now into its stacks and pinned
    #[arg(long, value_name = "selection:NAME@V", conflicts_with = "handle")]
    pub(crate) select: Option<String>,
    /// A frozen list of stacks to run over, by handle id, pinned
    #[arg(long, value_name = "ID")]
    pub(crate) handle: Option<i64>,
    /// A parameter, as id=value (repeatable); the rest take their defaults
    #[arg(long = "param", value_name = "ID=VALUE")]
    pub(crate) params: Vec<String>,
    /// A registered model the pipeline reads: its id, digest or name@version (repeatable, in the order the descriptor declares its model inputs)
    #[arg(long = "model", value_name = "MODEL")]
    pub(crate) models: Vec<String>,
    /// A label set the pipeline reads, by id
    #[arg(long = "labels", value_name = "ID")]
    pub(crate) labels: Option<i64>,
    /// The pack a selection is frozen and a bids input released under
    #[arg(long, default_value = "mri")]
    pub(crate) pack: String,
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<PathBuf>,
    /// The DICOM to NIfTI converter a bids input is released with
    #[arg(long, default_value = "dcm2niix", value_name = "PATH")]
    pub(crate) dcm2niix: PathBuf,
    /// Machine-readable output
    #[arg(long)]
    pub(crate) json: bool,
}

fn print(v: &Value) -> Result<(), Exit> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| fail(format!("will not serialize: {e}")))?
    );
    Ok(())
}

pub(crate) fn command(home: &Home, command: PipelineCommand) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    match command {
        PipelineCommand::Add { file, json } => {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| usage(format!("{}: {e}", file.display())))?;
            let d =
                descriptor::parse(&text).map_err(|e| usage(format!("{}: {e}", file.display())))?;
            let digest = d.digest();
            let who = crate::actor();
            let now = nils_registry::time::now_iso();
            let (p, fresh) = rows::add(
                registry.store(),
                &rows::New {
                    name: &d.name,
                    tool_version: &d.tool_version,
                    descriptor: &d.document,
                    descriptor_digest: &digest,
                    image: &d.image.reference,
                    image_digest: &d.image.digest,
                    layout: d.layout.name(),
                    level: d.level.name(),
                    added_by: &who,
                    added_at: &now,
                },
            )?;
            if fresh {
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineAdd,
                    json!({"pipeline": p.id, "name": p.name, "version": p.version}),
                    Some(json!({"descriptor": p.descriptor_digest, "image": p.image_digest})),
                )?;
            }
            if json {
                let mut v = pipeline_doc(&p);
                v["added"] = json!(fresh);
                return print(&v);
            }
            println!(
                "pipeline {}: {} ({}), image {}{}",
                p.id,
                p.label(),
                p.descriptor_digest,
                p.image,
                if fresh {
                    ""
                } else {
                    "; the catalog already held this descriptor"
                }
            );
            Ok(())
        }
        PipelineCommand::List { json } => {
            let list = rows::list(registry.store())?;
            let cap = {
                let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
                capability_of(registry.store(), &d)
            };
            if json {
                return print(&json!({
                    "pipelines": list.iter().map(pipeline_doc).collect::<Vec<_>>(),
                    "capability": cap,
                }));
            }
            println!(
                "{:>5}  {:<32} {:<8} {:<11} {:<7} image",
                "id", "pipeline", "layout", "level", "state"
            );
            for p in &list {
                println!(
                    "{:>5}  {:<32} {:<8} {:<11} {:<7} {}",
                    p.id,
                    p.label(),
                    p.layout,
                    p.level,
                    p.state,
                    p.image
                );
            }
            if cap["enabled"] == true {
                println!(
                    "{} pipelines; they run under {} {} into {}",
                    list.len(),
                    cap["runtime"]["name"].as_str().unwrap_or_default(),
                    cap["runtime"]["version"].as_str().unwrap_or_default(),
                    cap["place"].as_str().unwrap_or_default()
                );
            } else {
                println!(
                    "{} pipelines; {}",
                    list.len(),
                    cap["reason"].as_str().unwrap_or("pipelines are off")
                );
            }
            Ok(())
        }
        PipelineCommand::Show { pipeline, json } => {
            let p = rows::resolve(registry.store(), &pipeline)?
                .ok_or_else(|| usage(format!("no pipeline {pipeline} in the catalog")))?;
            let doc = pipeline_doc(&p);
            if json {
                return print(&doc);
            }
            println!("pipeline {}: {}", p.id, p.label());
            println!("  tool version     {}", p.tool_version);
            println!("  image            {}", p.image);
            println!("  descriptor       {}", p.descriptor_digest);
            println!("  input            {} layout, {} units", p.layout, p.level);
            println!(
                "  needs a GPU      {}",
                doc["needs"]["gpu"].as_str().unwrap_or("none")
            );
            for param in doc["parameters"].as_array().into_iter().flatten() {
                println!(
                    "  parameter        {} ({}), default {}",
                    param["id"].as_str().unwrap_or_default(),
                    param["type"].as_str().unwrap_or_default(),
                    param["default-value"]
                );
            }
            for o in doc["outputs"].as_array().into_iter().flatten() {
                println!(
                    "  output           {}: a {} at {}",
                    o["id"].as_str().unwrap_or_default(),
                    o["kind"].as_str().unwrap_or_default(),
                    o["path-template"].as_str().unwrap_or_default()
                );
            }
            println!("  added by         {} at {}", p.added_by, p.added_at);
            Ok(())
        }
        PipelineCommand::Runtime { set, json } => {
            if let Some(word) = set {
                let c = Choice::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "--set is one of {}, not {word}",
                        Choice::WORDS.join(", ")
                    ))
                })?;
                registry.set_meta(RUNTIME_KEY, c.name())?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"choice": c.name()}),
                    None,
                )?;
            }
            let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
            let cap = capability_of(registry.store(), &d);
            if json {
                return print(&cap);
            }
            println!("choice   {}", d.choice.name());
            for (k, s) in &d.looked {
                println!("  {k:<10} {s}");
            }
            match &d.runtime {
                Some(r) => println!(
                    "runtime  {} {}{}",
                    r.kind.name(),
                    r.version,
                    r.gpu
                        .as_deref()
                        .map(|g| format!(", GPU {g}"))
                        .unwrap_or_else(|| ", no GPU".into())
                ),
                None => println!("runtime  none: {}", d.reason.as_deref().unwrap_or("")),
            }
            if cap["enabled"] != true
                && let Some(r) = cap["reason"].as_str()
                && d.runtime.is_some()
            {
                println!("{r}");
            }
            Ok(())
        }
        PipelineCommand::Runs {
            id,
            pipeline,
            limit,
            json,
        } => {
            if let Some(id) = id {
                let r = rows::run(registry.store(), id)?
                    .ok_or_else(|| usage(format!("no pipeline run {id}")))?;
                let doc = run_doc(registry.store(), &r);
                if json {
                    return print(&doc);
                }
                print_run(&doc);
                return Ok(());
            }
            let pid = match pipeline {
                None => None,
                Some(name) => Some(
                    rows::resolve(registry.store(), &name)?
                        .ok_or_else(|| usage(format!("no pipeline {name} in the catalog")))?
                        .id,
                ),
            };
            let list = rows::runs(registry.store(), pid, limit)?;
            let store = registry.store();
            let docs: Vec<Value> = list.iter().map(|r| run_doc(store, r)).collect();
            if json {
                return print(&json!(docs));
            }
            println!(
                "{:>5}  {:<28} {:<9} {:<10} {:>6}  started",
                "run", "pipeline", "status", "runtime", "files"
            );
            for d in &docs {
                println!(
                    "{:>5}  {:<28} {:<9} {:<10} {:>6}  {}",
                    d["id"],
                    d["pipeline"].as_str().unwrap_or("-"),
                    d["status"].as_str().unwrap_or_default(),
                    d["runtime"].as_str().unwrap_or_default(),
                    d["derivatives"].as_array().map_or(0, Vec::len),
                    d["started_at"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

fn print_run(doc: &Value) {
    println!(
        "run {} of {}: {}",
        doc["id"],
        doc["pipeline"].as_str().unwrap_or("-"),
        doc["status"].as_str().unwrap_or_default()
    );
    println!(
        "  over             {} (handle {})",
        doc["selection"].as_str().unwrap_or("a handle"),
        doc["handle_id"]
    );
    println!("  parameters       {}", doc["params"]);
    println!(
        "  runtime          {} {} on {}, {}",
        doc["runtime"].as_str().unwrap_or_default(),
        doc["runtime_version"].as_str().unwrap_or_default(),
        doc["host"].as_str().unwrap_or_default(),
        doc["device"].as_str().unwrap_or_default()
    );
    let units = &doc["summary"]["units"];
    println!(
        "  units            {} succeeded, {} failed, {} skipped, {} unreported",
        units["succeeded"], units["failed"], units["skipped"], units["unreported"]
    );
    println!(
        "  derivatives      {}",
        doc["derivatives"].as_array().map_or(0, Vec::len)
    );
    if let Some(d) = doc["results_digest"].as_str() {
        println!("  results          {d}");
    }
    if let Some(e) = doc["error"].as_str() {
        println!("  error            {e}");
    }
}

// ------------------------------------------------------------------ the run

/// One work unit of a run.
#[derive(Debug, Clone)]
struct Unit {
    /// `sub-<s>`, `sub-<s>_ses-<t>` or `stack-<id>`.
    id: String,
    subject: Option<String>,
    session: Option<String>,
    stack_id: Option<i64>,
    series_id: Option<i64>,
    subject_id: Option<i64>,
    session_day: Option<String>,
    stacks: Vec<i64>,
}

impl Unit {
    /// What a derivative of the unit belongs to.
    fn belongs(&self) -> Option<Belongs> {
        let subject_id = self.subject_id?;
        Some(match (self.stack_id, &self.session_day) {
            (Some(stack), _) => Belongs {
                scope: "stack".into(),
                stack_id: Some(stack),
                series_id: self.series_id,
                subject_id,
                session_day: None,
            },
            (None, Some(day)) => Belongs {
                scope: "session".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: Some(day.clone()),
            },
            (None, None) => Belongs {
                scope: "subject".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: None,
            },
        })
    }

    /// The values a path template's words take for the unit.
    fn vars(&self) -> Vec<(&'static str, String)> {
        let mut v = Vec::new();
        if let Some(s) = &self.subject {
            v.push(("subject", s.clone()));
        }
        if let Some(s) = &self.session {
            v.push(("session", s.clone()));
        }
        if let Some(s) = self.stack_id {
            v.push(("stack", s.to_string()));
        }
        v
    }

    fn doc(&self) -> Value {
        json!({
            "unit": self.id, "subject": self.subject, "session": self.session,
            "stack_id": self.stack_id, "subject_id": self.subject_id,
            "session_day": self.session_day, "stacks": self.stacks,
        })
    }
}

/// The detail a queued run was given; a run from the keyboard holds every
/// class. A pipeline reads pixels, which open at detail quasi.
fn detail_allows_pixels() -> Result<(), Exit> {
    if let Ok(d) = std::env::var("NILS_JOB_DETAIL") {
        let detail = crate::grants::Detail::parse(d.trim()).unwrap_or_default();
        if detail < crate::grants::Detail::Quasi {
            return Err(usage(
                "a pipeline reads the pixels, which are quasi-identifying; the run needs detail quasi",
            ));
        }
    }
    Ok(())
}

/// Parse `id=value` words.
fn pairs(words: &[String]) -> Result<Vec<(String, String)>, Exit> {
    words
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| usage(format!("--param {w}: write it as id=value")))
        })
        .collect()
}

/// A short random word, so two registries on one host never name a
/// container alike.
fn nonce() -> String {
    let mut b = [0u8; 4];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    hex::encode(b)
}

/// The stacks of a frozen handle.
fn handle_stacks(store: &mut Store, handle: i64) -> Result<Vec<i64>, Exit> {
    let h = nils_ask::handle::get(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no handle {handle}")))?;
    if h.grain != nils_ask::ast::Grain::Stack {
        return Err(usage(format!(
            "handle {handle} is a {} set; a run is over stacks, and --select freezes a wider selection into them",
            h.grain.name()
        )));
    }
    if !h.has_rows() || h.withdrawn_at.is_some() || h.truncated {
        return Err(usage(format!(
            "handle {handle} keeps no complete list of stacks; run its question again"
        )));
    }
    let mut stacks: Vec<i64> = nils_ask::handle::keys(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    stacks.sort_unstable();
    stacks.dedup();
    Ok(stacks)
}

/// Whether a release may write `out` for a run's input: the run is running
/// and `out` is its input folder under its working place. Otherwise a
/// release writes to an export place only (Wave 5 section 10.2).
pub(crate) fn release_target(store: &mut Store, run_id: i64, out: &Path) -> Result<(), String> {
    let r = rows::run(store, run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("--into-run {run_id}: no such pipeline run"))?;
    if r.status != "running" {
        return Err(format!("--into-run {run_id}: the run is {}", r.status));
    }
    let place = r
        .place_id
        .and_then(|id| nils_registry::place::show(store, id).ok().flatten())
        .ok_or_else(|| format!("--into-run {run_id}: the run has no working place"))?;
    let want = Path::new(&place.path)
        .join(RUNS)
        .join(run_id.to_string())
        .join("input");
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(out) != canon(&want) {
        return Err(format!(
            "--into-run {run_id}: a run's input is released into its own folder, {}",
            want.display()
        ));
    }
    Ok(())
}

/// Wait for a child, beating the job's heart; a cancel kills it.
fn wait_beating(
    child: &mut std::process::Child,
    store: &mut Store,
    job_id: i64,
    progress: &Value,
) -> Result<(std::process::ExitStatus, bool), String> {
    let mut beaten = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok((status, false));
        }
        if beaten.elapsed() >= Duration::from_secs(5) {
            beaten = Instant::now();
            if let Ok(job::Asked::Cancel) = job::beat(store, job_id, Some(progress)) {
                let _ = child.kill();
                let status = child.wait().map_err(|e| e.to_string())?;
                return Ok((status, true));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The input of a run, as materialised.
struct Materialised {
    units: Vec<Unit>,
    mounts: Vec<Mount>,
    release_id: Option<i64>,
    stopped: bool,
}

/// `stacks.json`: each stack's files under the source places, mounted
/// read-only at `/source/<n>`, and the frames of a multi-frame file.
fn materialise_stacks(
    store: &mut Store,
    stacks: &[i64],
    input: &Path,
) -> Result<Materialised, String> {
    let d = store.dialect();
    let mut sources: BTreeMap<i64, (usize, String)> = BTreeMap::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut units: Vec<Unit> = Vec::new();
    let err = |e: nils_registry::Error| e.to_string();
    for &stack in stacks {
        let sql = format!(
            "SELECT st.series_id, se.subject_id, st.modality FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id = {}",
            store.qualified("stack"),
            store.qualified("series"),
            d.param(1, Type::Int)
        );
        let Some(row) = store.query_opt(&sql, &[Param::Int(stack)]).map_err(err)? else {
            return Err(format!(
                "stack {stack} of the selection is not in the registry"
            ));
        };
        let series_id = row.int(0).map_err(err)?;
        let subject_id = row.int(1).map_err(err)?;
        let modality = row.text(2).map_err(err)?.to_string();
        // the files: a multi-frame file's frames first, then the files whose
        // every frame is the stack's
        let framed = format!(
            "SELECT so.id, so.root, f.path, fr.frames FROM {} fr JOIN {} i ON i.id = fr.instance_id \
             JOIN {} f ON f.id = i.source_file_id JOIN {} so ON so.id = f.source_id \
             WHERE fr.stack_id = {} ORDER BY f.path",
            store.qualified("instance_frame"),
            store.qualified("instance"),
            store.qualified("source_file"),
            store.qualified("source"),
            d.param(1, Type::Int)
        );
        let whole = format!(
            "SELECT so.id, so.root, f.path FROM {} i JOIN {} f ON f.id = i.source_file_id \
             JOIN {} so ON so.id = f.source_id WHERE i.stack_id = {} ORDER BY f.path",
            store.qualified("instance"),
            store.qualified("source_file"),
            store.qualified("source"),
            d.param(1, Type::Int)
        );
        let mut files: Vec<Value> = Vec::new();
        let mut seen: std::collections::BTreeSet<(i64, String)> = Default::default();
        let mut add = |sources: &mut BTreeMap<i64, (usize, String)>,
                       so: i64,
                       root: &str,
                       path: &str,
                       frames: Option<&str>| {
            if !seen.insert((so, path.to_string())) {
                return;
            }
            let n = sources.len();
            let (index, _) = sources.entry(so).or_insert((n, root.to_string()));
            files.push(json!({"source": *index, "path": path, "frames": frames}));
        };
        for r in store.query(&framed, &[Param::Int(stack)]).map_err(err)? {
            add(
                &mut sources,
                r.int(0).map_err(err)?,
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?,
                Some(r.text(3).map_err(err)?),
            );
        }
        for r in store.query(&whole, &[Param::Int(stack)]).map_err(err)? {
            add(
                &mut sources,
                r.int(0).map_err(err)?,
                r.text(1).map_err(err)?,
                r.text(2).map_err(err)?,
                None,
            );
        }
        if files.is_empty() {
            return Err(format!("stack {stack} has no files the registry can read"));
        }
        let unit = format!("stack-{stack}");
        entries.push(json!({
            "unit": unit, "stack_id": stack, "series_id": series_id,
            "subject_id": subject_id, "modality": modality, "files": files,
        }));
        units.push(Unit {
            id: unit,
            subject: None,
            session: None,
            stack_id: Some(stack),
            series_id: Some(series_id),
            subject_id: Some(subject_id),
            session_day: None,
            stacks: vec![stack],
        });
    }
    let mut mounts = Vec::new();
    let mut listed = Vec::new();
    let mut by_index: Vec<(usize, String)> = sources.into_values().collect();
    by_index.sort();
    for (index, root) in by_index {
        let at = format!("/source/{index}");
        listed.push(json!({"id": index, "mount": at}));
        mounts.push(Mount {
            host: PathBuf::from(root),
            container: at,
            read_only: true,
        });
    }
    let manifest =
        json!({"contract": nils_pipeline::CONTRACT, "sources": listed, "stacks": entries});
    std::fs::write(
        input.join("stacks.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("the run's input: {e}"))?;
    Ok(Materialised {
        units,
        mounts,
        release_id: None,
        stopped: false,
    })
}

/// The bids layout: a release of the stacks with the picks applied into
/// the run's input folder, by the release machinery itself as a process of
/// its own, and the units read back from what it wrote.
#[allow(clippy::too_many_arguments)]
fn materialise_bids(
    home: &Home,
    registry: &mut Registry,
    run_id: i64,
    job_id: i64,
    level: Level,
    stacks: &[i64],
    run_dir: &Path,
    input: &Path,
    args: &RunArgs,
) -> Result<Materialised, String> {
    let chosen = run_dir.join("selection.txt");
    let list: String = stacks.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(&chosen, list).map_err(|e| format!("the run's folder: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| format!("this binary: {e}"))?;
    let log = std::fs::File::create(run_dir.join("release.log"))
        .map_err(|e| format!("the run's folder: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--registry")
        .arg(home.dir())
        .args(["release", "--out"])
        .arg(input)
        .args(["--layout", "bids", "--picked", "--select"])
        .arg(&chosen)
        .args(["--name", &format!("pipeline-run-{run_id}")])
        .args([
            "--pack",
            &args.pack,
            "--json",
            "--into-run",
            &run_id.to_string(),
        ])
        .arg("--dcm2niix")
        .arg(&args.dcm2niix)
        // the release is a job of its own, never the run's row
        .env_remove(job::ADOPT_VAR)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(log));
    if let Some(dir) = &args.pack_dir {
        cmd.arg("--pack-dir").arg(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("the release: {e}"))?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        text
    });
    let (status, stopped) = wait_beating(
        &mut child,
        registry.store(),
        job_id,
        &json!({"run": run_id, "phase": "materialise", "layout": "bids"}),
    )?;
    let text = reader.join().unwrap_or_default();
    if stopped {
        return Ok(Materialised {
            units: Vec::new(),
            mounts: Vec::new(),
            release_id: None,
            stopped: true,
        });
    }
    if !status.success() {
        let tail = std::fs::read_to_string(run_dir.join("release.log")).unwrap_or_default();
        let last = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim_start_matches("nils: ")
            .to_string();
        return Err(format!("the release of the input failed: {last}"));
    }
    let report: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("the release's report: {e}"))?;
    let release_id = report["release_id"]
        .as_i64()
        .ok_or("the release's report names no release")?;
    rows::set_input_release(registry.store(), run_id, release_id).map_err(|e| e.to_string())?;
    let store = registry.store();
    let d = store.dialect();
    let err = |e: nils_registry::Error| e.to_string();
    let sql = format!(
        "SELECT rs.stack_id, rs.dir, se.subject_id, se.study_id FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id \
         JOIN {} st ON st.id = rs.stack_id JOIN {} se ON se.id = st.series_id \
         WHERE r.id = {} ORDER BY rs.stack_id",
        store.qualified("release"),
        store.qualified("release_stack"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let placed = store.query(&sql, &[Param::Int(release_id)]).map_err(err)?;
    let labels = nils_session::labels_by_study(store, &nils_registry::session::Scheme::default())
        .unwrap_or_default();
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    for r in &placed {
        let stack = r.int(0).map_err(err)?;
        let dir = r.text(1).map_err(err)?;
        let subject_id = r.int(2).map_err(err)?;
        let study_id = r.int(3).map_err(err)?;
        let mut parts = dir.split('/');
        let Some(subject) = parts.next().and_then(|p| p.strip_prefix("sub-")) else {
            // routed to sourcedata or derivatives: not a unit's input
            continue;
        };
        let session = parts.next().and_then(|p| p.strip_prefix("ses-"));
        let (id, session) = match (level, session) {
            (Level::Session, Some(s)) => (format!("sub-{subject}_ses-{s}"), Some(s.to_string())),
            _ => (format!("sub-{subject}"), None),
        };
        let day = if session.is_some() {
            labels.get(&study_id).map(|l| l.first.to_string())
        } else {
            None
        };
        let u = units.entry(id.clone()).or_insert(Unit {
            id,
            subject: Some(subject.to_string()),
            session,
            stack_id: None,
            series_id: None,
            subject_id: Some(subject_id),
            session_day: day,
            stacks: Vec::new(),
        });
        u.stacks.push(stack);
    }
    if units.is_empty() {
        return Err(
            "the release of the input holds no subject's images: the selection's stacks were not picked, or went to sourcedata; nils pick run picks them".into(),
        );
    }
    Ok(Materialised {
        units: units.into_values().collect(),
        mounts: Vec::new(),
        release_id: Some(release_id),
        stopped: false,
    })
}

/// What became of one unit after the container exited.
struct Outcome {
    status: &'static str,
    error: Option<String>,
    metrics: Value,
    /// Files relative to the output folder.
    files: Vec<String>,
}

/// `nils run`.
pub(crate) fn run_command(home: &Home, args: RunArgs) -> Result<(), Exit> {
    detail_allows_pixels()?;
    let mut registry = crate::open(home)?;
    let p = rows::resolve(registry.store(), &args.pipeline)?.ok_or_else(|| {
        usage(format!(
            "no pipeline {} in the catalog; nils pipeline list names them",
            args.pipeline
        ))
    })?;
    if p.state != "active" {
        return Err(usage(format!("{} is {}", p.label(), p.state)));
    }
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        fail(format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        ))
    })?;
    let params = d.resolve(&pairs(&args.params)?).map_err(usage)?;

    // where it goes and what runs it; either absent is the capability off
    let place = crate::derivatives::working(registry.store())
        .map_err(|m| usage(m.replacen("derivatives are off", "pipelines are off", 1)))?;
    let detected = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
    let rt = detected
        .runtime
        .clone()
        .ok_or_else(|| usage(detected.reason.clone().unwrap_or_default()))?;
    let (device, gpu) = match (d.gpu, rt.gpu()) {
        (Gpu::None, _) | (Gpu::Optional, None) => ("cpu".to_string(), false),
        (Gpu::Optional | Gpu::Required, Some(g)) => (g.to_string(), true),
        (Gpu::Required, None) => {
            return Err(usage(format!(
                "{} needs a GPU, and {} here offers none{}",
                p.label(),
                rt.kind.name(),
                if rt.kind == runtime::Kind::Podman {
                    " (podman passes one through a CDI specification, which this host has not got)"
                } else {
                    ""
                }
            )));
        }
    };

    // the typed inputs: models, a label set, derivatives of a kind
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    if args.models.len() > model_inputs.len() {
        return Err(usage(format!(
            "{} reads {} model(s); {} were given",
            p.label(),
            model_inputs.len(),
            args.models.len()
        )));
    }
    if let Some(missing) = model_inputs
        .iter()
        .skip(args.models.len())
        .find(|t| !t.optional)
    {
        return Err(usage(format!(
            "{} needs a model for its input {}: --model <id|digest|name@version>",
            p.label(),
            missing.id
        )));
    }
    let mut models = Vec::new();
    for reference in &args.models {
        let m = nils_registry::model::resolve(registry.store(), reference)?
            .ok_or_else(|| usage(format!("no registered model answers to {reference}")))?;
        if m.state == "retired" {
            return Err(usage(format!("model {} is retired", m.label())));
        }
        models.push(m);
    }
    let label_input = d.inputs.iter().find(|t| t.ty == "label_set");
    let label_set = match (args.labels, label_input) {
        (Some(_), None) => {
            return Err(usage(format!("{} reads no label set", p.label())));
        }
        (None, Some(t)) if !t.optional => {
            return Err(usage(format!(
                "{} needs a label set for its input {}: --labels <id>",
                p.label(),
                t.id
            )));
        }
        (Some(id), Some(_)) => {
            let set = nils_registry::labels::get(registry.store(), id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no label set {id}")))?;
            if set.sealed {
                return Err(usage(format!(
                    "label set {id} was drawn from a sealed certification sample and is never training data (record 40 R3); a pipeline that reads a label set may train on it"
                )));
            }
            Some(set)
        }
        _ => None,
    };

    // the selection, frozen, and the stacks it holds
    let (handle_id, selection) = match (&args.select, args.handle) {
        (Some(spec), _) => {
            drop(registry);
            let h = crate::ask_cli::freeze_selection(
                home,
                spec,
                nils_ask::ast::Grain::Stack,
                args.pack_dir.clone(),
                &args.pack,
            )?;
            registry = crate::open(home)?;
            (h, Some(spec.clone()))
        }
        (None, Some(h)) => (h, None),
        (None, None) => {
            return Err(usage(
                "a run is over a frozen selection: --select selection:<name>@<v>, or --handle <id>",
            ));
        }
    };
    let stacks = handle_stacks(registry.store(), handle_id)?;
    if stacks.is_empty() {
        return Err(usage(
            "the selection holds no stacks; there is nothing to run",
        ));
    }

    // the job: one pipeline run at a time (R1)
    let who = crate::actor();
    let actor = nils_registry::actor::current();
    let job_id = job::claim(
        registry.store(),
        &job::Claim {
            kind: "pipeline",
            name: &p.label(),
            args: json!({
                "pipeline": p.label(), "select": args.select, "handle": handle_id,
                "params": args.params, "models": args.models, "labels": args.labels,
            }),
        },
    )
    .map_err(|e| match e {
        job::Error::Busy { .. } => Exit {
            code: crate::BUSY,
            message: e.to_string(),
        },
        other => fail(other.to_string()),
    })?;
    let params_value = Value::Object(params.clone());
    let model_ids: Vec<i64> = models.iter().map(|m| m.id).collect();
    let started = nils_registry::time::now_iso();
    let run_id = rows::start(
        registry.store(),
        &rows::NewRun {
            pipeline_id: p.id,
            job_id: Some(job_id),
            handle_id: Some(handle_id),
            selection: selection.as_deref(),
            params: &params_value,
            runtime: rt.kind.name(),
            runtime_version: &rt.version,
            host: &job::hostname(),
            device: &device,
            model_ids: &model_ids,
            label_set_id: label_set.as_ref().map(|s| s.id),
            place_id: Some(place.id),
            principal: &who,
            actor: Some(&actor),
            started_at: &started,
        },
    )?;
    let _ = job::set_arg(registry.store(), job_id, "run", json!(run_id));

    let outcome = execute(
        home,
        &mut registry,
        &Execution {
            pipeline: &p,
            descriptor: &d,
            params: &params,
            run_id,
            job_id,
            place: &place,
            runtime: &rt,
            gpu,
            stacks: &stacks,
            models: &models,
            label_set: label_set.as_ref(),
            who: &who,
            actor: &actor,
            args: &args,
        },
    );
    let (status, summary, digest, exit_code, error) = match outcome {
        Ok(o) => o,
        Err(e) => ("failed", json!({}), None, None, Some(e)),
    };
    let finished = nils_registry::time::now_iso();
    rows::finish(
        registry.store(),
        run_id,
        &rows::Finish {
            status,
            finished_at: &finished,
            exit_code,
            results_digest: digest.as_deref(),
            summary: &summary,
            error: error.as_deref(),
        },
    )?;
    nils_registry::audit::record(
        &mut registry,
        &nils_registry::audit::Entry {
            principal: &who,
            action: nils_registry::audit::Action::PipelineRun,
            scope: json!({
                "run": run_id, "pipeline": p.label(), "handle": handle_id,
                "units": summary["units"], "derivatives": summary["derivatives"],
            }),
            policy: None,
            job_id: Some(job_id),
            details: Some(json!({
                "status": status, "runtime": rt.kind.name(), "device": device,
                "results": digest, "review_items": summary["review_items"],
            })),
        },
    )?;
    let job_state = match status {
        "done" => job::State::Done,
        "cancelled" => job::State::Cancelled,
        _ => job::State::Failed,
    };
    let _ = job::set_result(
        registry.store(),
        job_id,
        &json!({"run": run_id, "status": status, "summary": summary, "results_digest": digest}),
    );
    job::finish(registry.store(), job_id, job_state, error.as_deref())
        .map_err(|e| fail(e.to_string()))?;
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| fail(format!("no pipeline run {run_id}")))?;
    let doc = run_doc(registry.store(), &r);
    if args.json {
        print(&doc)?;
    } else {
        print_run(&doc);
    }
    match status {
        "done" => Ok(()),
        "cancelled" => Err(Exit {
            code: crate::STOPPED,
            message: format!("run {run_id} was cancelled"),
        }),
        _ => Err(fail(format!(
            "run {run_id} {}",
            error.unwrap_or_else(|| "failed".into())
        ))),
    }
}

/// What a run is given once it is checked.
struct Execution<'a> {
    pipeline: &'a Pipeline,
    descriptor: &'a Descriptor,
    params: &'a serde_json::Map<String, Value>,
    run_id: i64,
    job_id: i64,
    place: &'a nils_registry::place::Place,
    runtime: &'a dyn Runtime,
    gpu: bool,
    stacks: &'a [i64],
    models: &'a [nils_registry::model::Model],
    label_set: Option<&'a nils_registry::labels::LabelSet>,
    who: &'a str,
    actor: &'a Value,
    args: &'a RunArgs,
}

/// How a run ended: its status, its summary, the digest of what it said,
/// the container's exit code and the error.
type Ended = (
    &'static str,
    Value,
    Option<String>,
    Option<i64>,
    Option<String>,
);

/// Materialise, run, and take in what the run left.
fn execute(home: &Home, registry: &mut Registry, x: &Execution<'_>) -> Result<Ended, String> {
    let p = x.pipeline;
    let d = x.descriptor;
    let working = PathBuf::from(&x.place.path);
    let run_dir = working.join(RUNS).join(x.run_id.to_string());
    let input = run_dir.join("input");
    let inputs = run_dir.join("inputs");
    let rel_out = format!("{}/{}/{}", derivative::TREE, p.name, x.run_id);
    let out = working.join(&rel_out);
    for dir in [&input, &inputs] {
        std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
    }
    if out.exists() {
        return Err(format!(
            "the output folder {rel_out} is there already; a run writes a folder of its own"
        ));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("the output folder: {e}"))?;
    rows::set_output(registry.store(), x.run_id, &rel_out).map_err(|e| e.to_string())?;
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "materialise", "layout": d.layout.name()})),
    );

    let m = match d.layout {
        Layout::Stacks => materialise_stacks(registry.store(), x.stacks, &input)?,
        Layout::Bids => materialise_bids(
            home, registry, x.run_id, x.job_id, d.level, x.stacks, &run_dir, &input, x.args,
        )?,
    };
    if m.stopped {
        return Ok((
            "cancelled",
            json!({"phase": "materialise"}),
            None,
            None,
            None,
        ));
    }

    // the typed inputs, each read-only, and the manifest
    let mut mounts = vec![
        Mount {
            host: input.clone(),
            container: "/input".into(),
            read_only: true,
        },
        Mount {
            host: inputs.clone(),
            container: "/inputs".into(),
            read_only: true,
        },
        Mount {
            host: out.clone(),
            container: "/output".into(),
            read_only: false,
        },
    ];
    mounts.extend(m.mounts.iter().cloned());
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    let models_doc: Vec<Value> = x
        .models
        .iter()
        .zip(&model_inputs)
        .map(|(model, t)| {
            json!({
                "input": t.id, "model_id": model.id, "name": model.name, "version": model.version,
                "digest": model.digest, "kind": model.kind, "card": model.card,
            })
        })
        .collect();
    let mut label_doc = Value::Null;
    if let (Some(set), Some(t)) = (x.label_set, d.inputs.iter().find(|t| t.ty == "label_set")) {
        label_doc = set.as_json();
        label_doc["input"] = json!(t.id);
        let dir = set
            .place_id
            .and_then(|id| {
                nils_registry::place::show(registry.store(), id)
                    .ok()
                    .flatten()
            })
            .zip(set.path.as_deref())
            .map(|(pl, rel)| Path::new(&pl.path).join(rel));
        if let Some(dir) = dir {
            let dir = if dir.is_file() {
                dir.parent().map(Path::to_path_buf).unwrap_or(dir)
            } else {
                dir
            };
            if dir.is_dir() {
                mounts.push(Mount {
                    host: dir,
                    container: format!("/inputs/{}", t.id),
                    read_only: true,
                });
            }
        }
    }
    let mut derivative_doc = serde_json::Map::new();
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let kind = t.ty.trim_start_matches("derivative:");
        let mut listed = Vec::new();
        for &stack in x.stacks {
            let found = derivative::list(
                registry.store(),
                &derivative::Filter {
                    kind: Some(kind),
                    stack_id: Some(stack),
                    limit: 10_000,
                    ..derivative::Filter::default()
                },
            )
            .map_err(|e| e.to_string())?;
            for row in found.into_iter().filter(|r| r.withdrawn_at.is_none()) {
                if row.place_id != x.place.id {
                    continue;
                }
                let Some(rel) = row.path.strip_prefix(&format!("{}/", derivative::TREE)) else {
                    continue;
                };
                listed.push(json!({
                    "id": row.id, "kind": row.kind, "stack_id": row.stack_id,
                    "subject_id": row.subject_id, "model_id": row.model_id,
                    "preprocess_version": row.preprocess_version,
                    "path": rel, "sha256": row.sha256,
                }));
            }
        }
        if listed.is_empty() && !t.optional {
            return Err(format!(
                "{} needs {kind} derivatives of the selection's stacks for its input {}, and none is registered",
                p.label(),
                t.id
            ));
        }
        mounts.push(Mount {
            host: working.join(derivative::TREE),
            container: format!("/inputs/{}", t.id),
            read_only: true,
        });
        derivative_doc.insert(t.id.clone(), json!(listed));
    }
    let manifest = json!({
        "contract": nils_pipeline::CONTRACT,
        "run": x.run_id,
        "pipeline": {"name": p.name, "version": p.version, "descriptor_digest": p.descriptor_digest, "image": p.image},
        "params": Value::Object(x.params.clone()),
        "level": d.level.name(),
        "layout": d.layout.name(),
        "units": m.units.iter().map(Unit::doc).collect::<Vec<_>>(),
        "models": models_doc,
        "label_set": label_doc,
        "derivatives": derivative_doc,
    });
    std::fs::write(
        inputs.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("the run's folder: {e}"))?;

    // the container
    let participants: Vec<String> = {
        let mut s: Vec<String> = m.units.iter().filter_map(|u| u.subject.clone()).collect();
        s.sort();
        s.dedup();
        s
    };
    let argv = d.argv(x.params, &participants)?;
    let user = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&out).ok().map(|m| (m.uid(), m.gid()))
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    let inv = Invocation {
        name: format!("nils-run-{}-{}", x.run_id, nonce()),
        image: p.image.clone(),
        argv,
        mounts,
        gpu: x.gpu,
        env: vec![
            ("NILS_RUN_ID".into(), x.run_id.to_string()),
            ("NILS_PIPELINE".into(), p.label()),
        ],
        user,
    };
    let progress = json!({"run": x.run_id, "phase": "run", "units": m.units.len()});
    let store = registry.store();
    let ended = runtime::run(x.runtime, &inv, &run_dir.join("log.txt"), &mut || {
        !matches!(
            job::beat(store, x.job_id, Some(&progress)),
            Ok(job::Asked::Cancel)
        )
    })
    .map_err(|e| format!("{} could not start the container: {e}", x.runtime.name()))?;
    let exit_code = ended.code.map(i64::from);
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "register"})),
    );
    if ended.stopped {
        return Ok(("cancelled", json!({"phase": "run"}), None, exit_code, None));
    }

    // what the run said, or what its templates find
    let results_path = out.join(nils_pipeline::results::FILE);
    let (reported, unreadable) = if results_path.is_file() {
        match std::fs::read_to_string(&results_path)
            .map_err(|e| e.to_string())
            .and_then(|t| nils_pipeline::results::parse(&t))
        {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, None)
    };
    let mut outcomes: Vec<(usize, Outcome)> = Vec::new();
    for (i, u) in m.units.iter().enumerate() {
        let o = if let Some(e) = &unreadable {
            Outcome {
                status: "failed",
                error: Some(e.clone()),
                metrics: json!({}),
                files: Vec::new(),
            }
        } else if let Some(r) = &reported {
            match r
                .units
                .iter()
                .find(|e| nils_pipeline::results::normalise(&e.unit_id) == u.id)
            {
                None => Outcome {
                    status: "unreported",
                    error: Some("the pipeline's results say nothing of this unit".into()),
                    metrics: json!({}),
                    files: Vec::new(),
                },
                Some(e) => {
                    let mut files = e.derivatives.clone();
                    if e.status == nils_pipeline::results::Status::Succeeded && files.is_empty() {
                        files = found_for(d, &out, u);
                    }
                    Outcome {
                        status: e.status.name(),
                        error: e.error.clone(),
                        metrics: e.metrics.clone(),
                        files: if e.status == nils_pipeline::results::Status::Succeeded {
                            files
                        } else {
                            Vec::new()
                        },
                    }
                }
            }
        } else if exit_code != Some(0) {
            Outcome {
                status: "failed",
                error: Some(format!(
                    "the container exited {} and wrote no results.json",
                    exit_code.map_or("by a signal".to_string(), |c| c.to_string())
                )),
                metrics: json!({}),
                files: Vec::new(),
            }
        } else {
            let files = found_for(d, &out, u);
            if files.is_empty() {
                Outcome {
                    status: "failed",
                    error: Some("no declared output was found for this unit".into()),
                    metrics: json!({}),
                    files,
                }
            } else {
                Outcome {
                    status: "succeeded",
                    error: None,
                    metrics: json!({}),
                    files,
                }
            }
        };
        outcomes.push((i, o));
    }

    // every file hashed by the engine and registered, naming the run
    let now = nils_registry::time::now_iso();
    let mut registered = 0usize;
    let mut bytes_total = 0u64;
    let mut refused: Vec<Value> = Vec::new();
    let mut digest_units: Vec<Value> = Vec::new();
    for (i, o) in outcomes.iter_mut() {
        let u = &m.units[*i];
        let mut hashed: Vec<Value> = Vec::new();
        let Some(belongs) = u.belongs() else {
            o.status = "failed";
            o.error = Some("the unit belongs to no subject the registry holds".into());
            continue;
        };
        let mut kept: Vec<String> = Vec::new();
        for rel in &o.files {
            let file = match nils_pipeline::files::inside(&out, rel) {
                Ok(f) => f,
                Err(e) => {
                    refused.push(json!({"unit": u.id, "why": e}));
                    continue;
                }
            };
            let (bytes, sha) =
                nils_pipeline::files::sha256_file(&file).map_err(|e| format!("{rel}: {e}"))?;
            let declared = nils_pipeline::files::which_output(&d.outputs, rel);
            let kind = declared.map_or("output", |o| o.kind.as_str());
            let media = nils_pipeline::files::media_type(
                rel,
                declared.and_then(|o| o.media_type.as_deref()),
            );
            let path = format!("{rel_out}/{rel}");
            derivative::insert_of_run(
                registry.store(),
                &derivative::New {
                    kind,
                    belongs: &belongs,
                    place_id: x.place.id,
                    path: &path,
                    bytes: bytes as i64,
                    sha256: &sha,
                    media_type: &media,
                    registered_by: x.who,
                    actor: Some(x.actor),
                    model_id: None,
                    supersedes_id: None,
                    created_at: &now,
                },
                x.run_id,
            )
            .map_err(|e| e.to_string())?;
            registered += 1;
            bytes_total += bytes;
            hashed.push(json!({"path": rel, "sha256": sha}));
            kept.push(rel.clone());
        }
        o.files = kept;
        hashed.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        digest_units.push(json!({"unit": u.id, "status": o.status, "files": hashed}));
    }
    digest_units.sort_by(|a, b| a["unit"].as_str().cmp(&b["unit"].as_str()));
    let proposals = reported
        .as_ref()
        .map(|r| r.proposals.clone())
        .unwrap_or_default();
    let results_digest = nils_pipeline::sha256(
        nils_pipeline::canonical(&json!({"units": digest_units, "proposals": proposals}))
            .as_bytes(),
    );

    // the units no one can vouch for become review items
    let mut items: Vec<i64> = Vec::new();
    let label = p.label();
    for (i, o) in &outcomes {
        if o.status != "failed" && o.status != "unreported" {
            continue;
        }
        let u = &m.units[*i];
        let id = nils_registry::review::raise_pipeline_qc(
            registry.store(),
            &nils_registry::review::PipelineQc {
                run_id: x.run_id,
                pipeline: &label,
                unit: &u.id,
                stack_id: u.stack_id,
                subject_id: u.subject_id,
                session_day: u.session_day.as_deref(),
                status: o.status,
                error: o.error.as_deref(),
                metrics: &o.metrics,
                job_id: Some(x.job_id),
            },
            &now,
        )
        .map_err(|e| e.to_string())?;
        items.push(id);
    }

    // the proposals, through the review spine's hook
    let ingested = crate::proposals::ingest(
        registry,
        &crate::proposals::From {
            run_id: x.run_id,
            pipeline: &label,
            axes: &d.proposals,
            model_ids: &x.models.iter().map(|m| m.id).collect::<Vec<_>>(),
            job_id: Some(x.job_id),
        },
        &proposals,
    )?;

    let count = |s: &str| outcomes.iter().filter(|(_, o)| o.status == s).count();
    let summary = json!({
        "units": {
            "total": m.units.len(), "succeeded": count("succeeded"), "failed": count("failed"),
            "skipped": count("skipped"), "unreported": count("unreported"),
        },
        "derivatives": registered,
        "bytes": bytes_total,
        "refused_files": refused,
        "review_items": items,
        "proposals": {
            "given": proposals.len(), "declared": ingested.declared,
            "undeclared": ingested.undeclared, "taken": ingested.taken,
        },
        "results": match (&reported, &unreadable) {
            (Some(_), _) => json!("results.json"),
            (None, Some(e)) => json!(format!("unreadable: {e}")),
            (None, None) => json!("none: found by the declared templates"),
        },
        "input_release_id": m.release_id,
        "log": format!("{RUNS}/{}/log.txt", x.run_id),
    });
    if exit_code == Some(0) {
        Ok(("done", summary, Some(results_digest), exit_code, None))
    } else {
        let error = format!(
            "the container exited {}; its log is {RUNS}/{}/log.txt in the working place {}",
            exit_code.map_or("by a signal".to_string(), |c| c.to_string()),
            x.run_id,
            x.place.name
        );
        Ok((
            "failed",
            summary,
            Some(results_digest),
            exit_code,
            Some(error),
        ))
    }
}

/// A unit's files as the declared templates find them.
fn found_for(d: &Descriptor, out: &Path, u: &Unit) -> Vec<String> {
    let vars = u.vars();
    let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut files: Vec<String> = d
        .outputs
        .iter()
        .flat_map(|o| nils_pipeline::files::found(out, &o.template, &pairs))
        .collect();
    files.sort();
    files.dedup();
    files
}
