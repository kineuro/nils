// SPDX-License-Identifier: AGPL-3.0-only

//! Workflows (Wave 7b A7; record 57 P7 and P8, record 58 P5, spec section
//! 8): the catalog of `nils.workflow.yml` documents beside the pipelines,
//! the plan that checks every link before anything runs (section 8.3), the
//! run that goes per unit through the chain (section 8.4), and the doors
//! and commands of each. The document and the link planner live in
//! `nils_pipeline::workflow`; this module ties them to the catalog, the
//! pre-flight, the runner and the registry's rows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nils_pipeline::descriptor::{self, Descriptor};
use nils_pipeline::pack::PipelinePack;
use nils_pipeline::workflow::{self as doc, Member, Workflow};
use nils_registry::home::Home;
use nils_registry::pipeline as prows;
use nils_registry::workflow as rows;
use nils_registry::{Registry, Store};
use serde_json::{Map, Value, json};

use crate::serve::{Caller, Reply};
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 7] = [
    "GET /api/workflows",
    "GET /api/workflows/{id}",
    "POST /api/workflows",
    "POST /api/workflows/{id}/plan",
    "POST /api/workflows/{id}/run",
    "GET /api/workflow-runs",
    "GET /api/workflow-runs/{id}",
];

/// The policy rows of [`DOORS`]: door, writes, idempotent, cost, cap, and
/// what the assistant says while and after.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/workflows",
        false,
        false,
        "bounded",
        "the catalog",
        "Listing workflows",
        "Listed workflows",
    ),
    (
        "GET /api/workflows/{id}",
        false,
        false,
        "free",
        "one workflow",
        "Reading a workflow",
        "Read a workflow",
    ),
    (
        "POST /api/workflows",
        true,
        true,
        "free",
        "one document",
        "Adding a workflow",
        "Added a workflow",
    ),
    (
        "POST /api/workflows/{id}/plan",
        false,
        false,
        "bounded",
        "the scope's units",
        "Planning a workflow",
        "Planned a workflow",
    ),
    (
        "POST /api/workflows/{id}/run",
        true,
        false,
        "long",
        "one workflow run",
        "Starting a workflow",
        "Started a workflow",
    ),
    (
        "GET /api/workflow-runs",
        false,
        false,
        "bounded",
        "the runs",
        "Listing workflow runs",
        "Listed workflow runs",
    ),
    (
        "GET /api/workflow-runs/{id}",
        false,
        false,
        "free",
        "one workflow run",
        "Reading a workflow run",
        "Read a workflow run",
    ),
];

/// The workflows the engine ships, seeded into the catalog as the starter
/// pipelines are (`pipelines/workflows/`).
pub(crate) const CATALOG: [(&str, &str); 2] = [
    (
        "flair-lesions",
        include_str!("../../../../pipelines/workflows/flair-lesions/nils.workflow.yml"),
    ),
    (
        "cortical",
        include_str!("../../../../pipelines/workflows/cortical/nils.workflow.yml"),
    ),
];

/// Who the engine adds its shipped workflows as.
const BY: &str = "nils";

/// `nils workflow`: the catalog, the plan and the runs.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum WorkflowCommand {
    /// The workflows in the catalog, by name and version
    List {
        #[arg(long)]
        json: bool,
    },
    /// One workflow as the catalog holds it: its id, name@version, or a name for its newest version
    Show {
        workflow: String,
        #[arg(long)]
        json: bool,
    },
    /// Add a workflow folder (its nils.workflow.yml) to the catalog: the name's next version, or the version that already is this document
    Add {
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check a workflow folder offline with the loader's words; the links are checked against the catalog by plan
    Validate {
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Plan a workflow over a scope before anything runs: each link's verdict (satisfied, missing, redundant, refused, fed) and each step's pre-flight
    Plan(Box<PlanArgs>),
    /// Run a workflow over a scope, step by step per unit (section 8.4)
    Run(Box<PlanArgs>),
    /// The workflow runs, newest first, or one by id
    Runs {
        run: Option<i64>,
        #[arg(long)]
        json: bool,
    },
}

/// The scope and the person's choices a plan or a run takes.
#[derive(Debug, clap::Args)]
pub(crate) struct PlanArgs {
    /// The workflow: its id, name@version, or a name for its newest version
    pub(crate) workflow: String,
    /// The scope: a saved selection, or a dataset's or a cohort's main scans, as nils run takes it
    #[arg(
        long,
        value_name = "selection:NAME@V|main:ROLE@dataset:NAME|main@dataset:NAME",
        conflicts_with = "handle"
    )]
    pub(crate) select: Option<String>,
    /// A frozen list of stacks, by handle id
    #[arg(long, value_name = "ID")]
    pub(crate) handle: Option<i64>,
    /// The goal every step judges under, where the document names none
    #[arg(long, value_name = "fast|standard|research")]
    pub(crate) goal: Option<String>,
    /// A knob a step lets a person set, as step.knob=value (repeatable)
    #[arg(long = "knob", value_name = "STEP.KNOB=VALUE")]
    pub(crate) knobs: Vec<String>,
    /// The pack the scope is frozen under
    #[arg(long, default_value = "mri")]
    pub(crate) pack: String,
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<PathBuf>,
    #[arg(long)]
    pub(crate) json: bool,
}

fn err(e: nils_registry::Error) -> Reply {
    Reply::error(500, e.to_string())
}

/// A catalog row as the doors answer it.
pub(crate) fn workflow_doc(w: &rows::Workflow) -> Value {
    let mut v = json!({
        "id": w.id, "name": w.name, "version": w.version, "label": w.label(),
        "digest": w.digest, "state": w.state, "added_by": w.added_by, "added_at": w.added_at,
        "origin": w.origin, "contract": format!("workflow/v{}", doc::CONTRACT),
    });
    match doc::from_value(w.document.clone()) {
        Ok(d) => {
            v["workflow"] = d.doc();
        }
        Err(e) => {
            v["error"] = Value::String(e);
        }
    }
    v
}

/// The catalog by name: each name's current version (the newest active one
/// no later version replaces) and every version's standing.
fn names(list: &[rows::Workflow]) -> Value {
    let mut by_name: BTreeMap<&str, Vec<&rows::Workflow>> = BTreeMap::new();
    for w in list {
        by_name.entry(w.name.as_str()).or_default().push(w);
    }
    json!(
        by_name
            .into_iter()
            .map(|(name, mut versions)| {
                versions.sort_by_key(|w| std::cmp::Reverse(w.version));
                let standing = standings(&versions);
                let current = versions.iter().find(|w| standing[&w.id] == "current");
                json!({
                    "name": name,
                    "current": current.map(|w| w.id),
                    "current_label": current.map(|w| w.label()),
                    "versions": versions.iter().map(|w| json!({
                        "id": w.id, "version": w.version, "label": w.label(), "standing": standing[&w.id],
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()
    )
}

/// Each version's standing within its name: `current`, `selectable`,
/// `replaced` (a later version's `replaces` covers it) or `retired`.
fn standings(versions: &[&rows::Workflow]) -> BTreeMap<i64, &'static str> {
    let mut out = BTreeMap::new();
    let replaced_by: Vec<(i64, Option<Vec<i64>>)> = versions
        .iter()
        .filter(|w| w.state == "active")
        .filter_map(|w| {
            doc::from_value(w.document.clone())
                .ok()
                .and_then(|d| d.replaces.map(|r| (w.version, r.versions)))
        })
        .collect();
    let newest = versions
        .iter()
        .filter(|w| w.state == "active")
        .map(|w| w.version)
        .max();
    for w in versions {
        let s = if w.state != "active" {
            "retired"
        } else if replaced_by
            .iter()
            .any(|(v, list)| *v > w.version && list.as_ref().is_none_or(|l| l.contains(&w.version)))
        {
            "replaced"
        } else if Some(w.version) == newest {
            "current"
        } else {
            "selectable"
        };
        out.insert(w.id, s);
    }
    out
}

/// Add a document's text to the catalog.
pub(crate) fn add_text(
    registry: &mut Registry,
    text: &str,
    origin: Option<&str>,
) -> Result<Value, Exit> {
    let w = doc::parse(text).map_err(usage)?;
    let digest = w.digest();
    let by = nils_registry::principal::Principal::current().to_string();
    let now = nils_registry::time::now_iso();
    let (row, fresh) = rows::add(
        registry.store(),
        &w.name,
        &w.document,
        &digest,
        origin.map_or(by.as_str(), |_| BY),
        &now,
        origin,
    )
    .map_err(|e| fail(e.to_string()))?;
    if fresh {
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: origin.map_or(by.as_str(), |_| BY),
                action: nils_registry::audit::Action::PipelineAdd,
                scope: json!({"workflow": row.id, "name": row.name, "version": row.version}),
                policy: None,
                job_id: None,
                details: Some(json!({"digest": digest, "origin": origin, "steps": w.steps.len()})),
            },
        )
        .map_err(|e| fail(e.to_string()))?;
    }
    let mut v = workflow_doc(&row);
    v["added"] = Value::Bool(fresh);
    Ok(v)
}

/// Seed the shipped workflows the catalog lacks, as the starter pipelines
/// are seeded. Answers those added.
pub(crate) fn seed(registry: &mut Registry) -> Result<Vec<Value>, String> {
    let mut added = Vec::new();
    for (name, text) in CATALOG {
        // the same document again is the version it is: one read decides
        // (after the third review, finding 10)
        let v = add_text(registry, text, Some(rows::STARTER))
            .map_err(|e| format!("the shipped workflow {name}: {}", e.message))?;
        if v["added"] == true {
            added.push(json!({"id": v["id"], "label": v["label"]}));
        }
    }
    Ok(added)
}

fn print(v: &Value) -> Result<(), Exit> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| fail(e.to_string()))?
    );
    Ok(())
}

pub(crate) fn command(home: &Home, command: WorkflowCommand) -> Result<(), Exit> {
    if let WorkflowCommand::Validate { dir, json } = &command {
        let w = doc::load_dir(dir).map_err(usage)?;
        let v = json!({"ok": true, "name": w.name, "digest": w.digest(), "steps": w.steps.len(), "links": w.links.len(), "workflow": w.doc()});
        if *json {
            return print(&v);
        }
        println!(
            "{}: {} steps, {} links, digest {}",
            w.name,
            w.steps.len(),
            w.links.len(),
            w.digest()
        );
        return Ok(());
    }
    let mut registry = crate::open(home)?;
    match command {
        WorkflowCommand::Validate { .. } => unreachable!("matched before the registry opens"),
        WorkflowCommand::List { json } => {
            let list = rows::list(registry.store()).map_err(|e| fail(e.to_string()))?;
            if json {
                return print(&json!({
                    "workflows": list.iter().map(workflow_doc).collect::<Vec<_>>(),
                    "names": names(&list),
                }));
            }
            if list.is_empty() {
                println!("no workflows in the catalog; nils workflow add <folder> adds one");
            }
            for w in &list {
                let d = doc::from_value(w.document.clone());
                println!(
                    "{} {} ({}){}",
                    w.id,
                    w.label(),
                    d.as_ref().map_or_else(
                        |e| e.clone(),
                        |d| format!(
                            "{} steps: {}",
                            d.steps.len(),
                            d.steps
                                .iter()
                                .map(|s| s.reference())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    ),
                    if w.origin.is_some() { ", shipped" } else { "" }
                );
            }
            Ok(())
        }
        WorkflowCommand::Show { workflow, json } => {
            let w = rows::resolve(registry.store(), &workflow)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no workflow {workflow} in the catalog")))?;
            let v = workflow_doc(&w);
            if json {
                return print(&v);
            }
            println!("{} ({})", w.label(), w.digest);
            if let Ok(d) = doc::from_value(w.document.clone()) {
                println!("{}", d.what);
                for s in &d.steps {
                    println!(
                        "  step {}: {} {}{}",
                        s.id,
                        s.of,
                        s.reference(),
                        s.goal
                            .as_ref()
                            .map_or(String::new(), |g| format!(", goal {g}"))
                    );
                }
                for l in &d.links {
                    println!("  link {}", l.words());
                }
            }
            Ok(())
        }
        WorkflowCommand::Add { dir, json } => {
            let file = if dir.is_dir() {
                dir.join(doc::FILE)
            } else {
                dir.clone()
            };
            let text = std::fs::read_to_string(&file)
                .map_err(|e| usage(format!("{}: {e}", file.display())))?;
            let v = add_text(&mut registry, &text, None)
                .map_err(|e| usage(format!("{}: {}", file.display(), e.message)))?;
            if json {
                return print(&v);
            }
            println!(
                "workflow {}: {} ({}){}",
                v["id"],
                v["label"].as_str().unwrap_or_default(),
                v["digest"].as_str().unwrap_or_default(),
                if v["added"] == true {
                    ""
                } else {
                    "; the catalog already held this document"
                }
            );
            Ok(())
        }
        WorkflowCommand::Plan(args) => {
            let (w, scope) = scope_of(home, &mut registry, &args)?;
            let person = Person::of_args(&args)?;
            let v = plan(home, &mut registry, &w, &scope, &person).map_err(usage)?;
            if args.json {
                return print(&v);
            }
            print_plan(&v);
            Ok(())
        }
        WorkflowCommand::Run(args) => crate::workflow_run::command(home, registry, *args),
        WorkflowCommand::Runs { run, json } => {
            let v = match run {
                Some(id) => {
                    let r = rows::run(registry.store(), id)
                        .map_err(|e| fail(e.to_string()))?
                        .ok_or_else(|| usage(format!("no workflow run {id}")))?;
                    crate::workflow_run::run_doc(registry.store(), &r)
                }
                None => json!({"runs": rows::runs(registry.store(), None, 50)
                    .map_err(|e| fail(e.to_string()))?
                    .iter()
                    .map(|r| crate::workflow_run::run_doc(registry.store(), r))
                    .collect::<Vec<_>>()}),
            };
            if json {
                return print(&v);
            }
            match run {
                Some(_) => println!("{}", crate::workflow_run::run_text(&v)),
                None => {
                    for r in v["runs"].as_array().into_iter().flatten() {
                        println!("{}", crate::workflow_run::run_line(r));
                    }
                }
            }
            Ok(())
        }
    }
}

/// What a plan or a run is over: the frozen handle, the selection it was
/// frozen from and what the main-scan freeze said.
pub(crate) struct Scope {
    pub handle: i64,
    pub selection: Option<String>,
    pub main: Option<Value>,
    pub pack: String,
    pub pack_dir: Option<PathBuf>,
}

/// The person's choices for a plan or a run: the goal where the document
/// names none, and the knobs set per step (`step.knob=value`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Person {
    pub goal: Option<String>,
    pub knobs: BTreeMap<String, Vec<(String, String)>>,
}

impl Person {
    pub(crate) fn of_args(args: &PlanArgs) -> Result<Person, Exit> {
        let mut p = Person {
            goal: args.goal.clone(),
            knobs: BTreeMap::new(),
        };
        for k in &args.knobs {
            let (name, value) = k
                .split_once('=')
                .ok_or_else(|| usage(format!("--knob takes step.knob=value, not {k}")))?;
            let (step, knob) = name
                .split_once('.')
                .ok_or_else(|| usage(format!("--knob takes step.knob=value, not {k}")))?;
            p.knobs
                .entry(step.to_string())
                .or_default()
                .push((knob.to_string(), value.to_string()));
        }
        Ok(p)
    }

    /// From a door's body: `goal`, `knobs` {step: {knob: value}}.
    pub(crate) fn of_body(body: &Value) -> Result<Person, String> {
        let mut p = Person {
            goal: body["goal"].as_str().map(str::to_string),
            knobs: BTreeMap::new(),
        };
        match &body["knobs"] {
            Value::Null => {}
            Value::Object(steps) => {
                for (step, knobs) in steps {
                    let Some(o) = knobs.as_object() else {
                        return Err("knobs is {step: {knob: value}}".into());
                    };
                    p.knobs.insert(
                        step.clone(),
                        o.iter()
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    match v {
                                        Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    },
                                )
                            })
                            .collect(),
                    );
                }
            }
            _ => return Err("knobs is {step: {knob: value}}".into()),
        }
        Ok(p)
    }

    pub(crate) fn doc(&self) -> Value {
        json!({
            "goal": self.goal,
            "knobs": self.knobs.iter().map(|(s, ks)| (s.clone(), Value::Object(ks.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect()))).collect::<Map<_, _>>(),
        })
    }
}

/// The workflow and the frozen scope the command names.
pub(crate) fn scope_of(
    home: &Home,
    registry: &mut Registry,
    args: &PlanArgs,
) -> Result<(rows::Workflow, Scope), Exit> {
    let w = rows::resolve(registry.store(), &args.workflow)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| {
            usage(format!(
                "no workflow {} in the catalog; nils workflow list names them",
                args.workflow
            ))
        })?;
    let scope = scope_from(
        home,
        registry,
        &w,
        args.select.as_deref(),
        args.handle,
        &args.pack,
        args.pack_dir.clone(),
    )?
    .ok_or_else(|| {
        usage("a workflow is planned or run over a scope: --select selection:<name>@<v>, --select main:<role>@dataset:<name> (or main@dataset:<name> for each role the steps need), or --handle <id>")
    })?;
    Ok((w, scope))
}

/// The frozen scope a workflow is planned or run over, from what the
/// command or the door names: a selection or a scope's main scans
/// (`select`), or a handle; none where neither is named. The one place for
/// both (after review, finding 10 of the second review).
pub(crate) fn scope_from(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    select: Option<&str>,
    handle: Option<i64>,
    pack: &str,
    pack_dir: Option<PathBuf>,
) -> Result<Option<Scope>, Exit> {
    let d = doc::from_value(w.document.clone()).map_err(|e| fail(format!("{}: {e}", w.label())))?;
    // the roles the steps need, for a scope's main scans without a role
    let mut needs: Vec<String> = Vec::new();
    for s in &d.steps {
        for r in crate::preflight::needs_of(registry.store(), &s.reference()) {
            if !needs.contains(&r) {
                needs.push(r);
            }
        }
    }
    let Some((handle, main)) =
        crate::preflight::frozen_over(home, select, handle, pack_dir.clone(), pack, &needs)?
    else {
        return Ok(None);
    };
    Ok(Some(Scope {
        handle,
        selection: select.map(str::to_string),
        main,
        pack: pack.to_string(),
        pack_dir,
    }))
}

/// One member of a family a step names (Wave 7b section 7): its catalog
/// row, as the family record pins it, with its descriptor, whose typed
/// outputs the links read. The member's pack judges the member's run; the
/// plan's link verdicts read a pipeline step's pack, and a family has none.
pub(crate) struct FamilyMember {
    pub key: String,
    pub pipeline: prows::Pipeline,
    pub descriptor: Descriptor,
}

/// A step resolved against the catalog: the pipeline, its descriptor and
/// pack; or the family it names with its member rows, whose typed outputs
/// are the step's link sources; or why it is refused.
pub(crate) struct Resolved {
    pub pipeline: Option<prows::Pipeline>,
    pub descriptor: Option<Descriptor>,
    pub pack: Option<PipelinePack>,
    pub family: Option<(crate::pipeline_family::Stored, Vec<FamilyMember>)>,
    pub refusal: Option<String>,
}

impl Resolved {
    /// The descriptors the step resolves to: the pipeline's, or every
    /// member's of the family.
    pub(crate) fn descriptors(&self) -> Vec<&Descriptor> {
        match &self.family {
            Some((_, members)) => members.iter().map(|m| &m.descriptor).collect(),
            None => self.descriptor.iter().collect(),
        }
    }

    /// What the step's run is given as `nils run <this>`: the pipeline's
    /// label, or the family's name, which the run command routes to the
    /// family's run (one member run per member the units route to).
    pub(crate) fn run_label(&self) -> Option<String> {
        self.pipeline
            .as_ref()
            .map(|p| p.label())
            .or_else(|| self.family.as_ref().map(|(s, _)| s.name.clone()))
    }

    /// As the planner reads the step.
    fn label(&self, s: &doc::Step) -> String {
        match (&self.pipeline, &self.family) {
            (Some(p), _) => p.label(),
            (None, Some((f, _))) => format!("family {}", f.name),
            (None, None) => s.reference(),
        }
    }
}

/// Resolve every step of a document against the catalog (section 8.3): a
/// family the catalog does not hold, or a pipeline it does not hold, is a
/// plan-time refusal, never a load error. A family the catalog holds
/// (Wave 7b section 7) resolves to its member rows, as the family record
/// pins them.
pub(crate) fn resolve_steps(store: &mut Store, d: &Workflow) -> Vec<Resolved> {
    d.steps
        .iter()
        .map(|s| {
            let none = |refusal: String| Resolved {
                pipeline: None,
                descriptor: None,
                pack: None,
                family: None,
                refusal: Some(refusal),
            };
            if s.of == "family" {
                return match resolve_family(store, &s.name) {
                    Ok(f) => Resolved {
                        pipeline: None,
                        descriptor: None,
                        pack: None,
                        family: Some(f),
                        refusal: None,
                    },
                    Err(e) => none(e),
                };
            }
            let p = match prows::resolve(store, &s.reference()) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return none(format!("no pipeline {} in the catalog", s.reference()));
                }
                Err(e) => return none(e.to_string()),
            };
            if p.state != "active" {
                return none(format!("{} is {}", p.label(), p.state));
            }
            if let Some(e) = crate::pipeline_pack::pack_error(&p) {
                return none(e);
            }
            let descriptor = match descriptor::from_value(p.descriptor.clone()) {
                Ok(d) => d,
                Err(e) => {
                    return none(format!(
                        "{}: the catalog's descriptor no longer checks: {e}",
                        p.label()
                    ));
                }
            };
            let pack = crate::pipeline_pack::pack_of(&p);
            Resolved {
                pipeline: Some(p),
                descriptor: Some(descriptor),
                pack,
                family: None,
                refusal: None,
            }
        })
        .collect()
}

/// The family a step names, with its members' catalog rows: the registry
/// holds one version of a family, the document it was added with, so the
/// step's version is not read; a family the catalog does not hold, or one
/// whose member rows are gone or no longer read, is the step's refusal.
fn resolve_family(
    store: &mut Store,
    name: &str,
) -> Result<(crate::pipeline_family::Stored, Vec<FamilyMember>), String> {
    let stored = crate::pipeline_family::get(store, name)?.ok_or_else(|| {
        format!(
            "the family {name} is not in this catalog: no member to plan (a family is one catalog row that opens to its members, Wave 7b section 7; nils pipeline add <folder> adds one)"
        )
    })?;
    stored.members_exist(store)?;
    let mut members = Vec::new();
    for (key, id) in &stored.members {
        let p = prows::get(store, *id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("family {name}: its member {key} is not in the catalog"))?;
        if let Some(e) = crate::pipeline_pack::pack_error(&p) {
            return Err(format!("family {name}, member {key}: {e}"));
        }
        let descriptor = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
            format!(
                "family {name}, member {key} ({}): the catalog's descriptor no longer checks: {e}",
                p.label()
            )
        })?;
        members.push(FamilyMember {
            key: key.clone(),
            pipeline: p,
            descriptor,
        });
    }
    Ok((stored, members))
}

/// The members a document's steps resolve to, for the link planner.
pub(crate) fn members<'a>(d: &'a Workflow, resolved: &'a [Resolved]) -> Vec<Member<'a>> {
    d.steps
        .iter()
        .zip(resolved)
        .map(|(s, r)| Member {
            step: s,
            label: r.label(s),
            descriptors: r.descriptors(),
            pack: r.pack.as_ref(),
            refusal: r.refusal.clone(),
        })
        .collect()
}

/// A family step's pre-flight for the plan: the family's routing of the
/// scope's units under the step's goal and switches (its task among them),
/// as `nils run <family> --preflight` answers it.
fn family_preflight(
    registry: &mut Registry,
    s: &crate::pipeline_family::Stored,
    scope: &Scope,
    ask: &crate::pipeline_pack::PersonAsk,
) -> Result<Value, String> {
    let f = s.family()?;
    let person = crate::pipeline_family::person_of(f, ask, &crate::actor())?;
    let stacks =
        crate::pipelines::handle_stacks(registry.store(), scope.handle).map_err(|e| e.message)?;
    let units = crate::pipeline_family::route_stacks(registry, f, &stacks, &person)?;
    Ok(crate::pipeline_family::preflight_doc(
        s, f, &person, ask, &units,
    ))
}

/// The steps each step waits for whole, by the step's place: those whose
/// link into it feeds a typed input or a pick role of its descriptor (or
/// of every member's, for a family), since a run binds both as it starts
/// (after the third review, finding 1: a role too). The per-unit chaining
/// is for order-only links, those that meet a pack need.
pub(crate) fn waits_whole(d: &Workflow, resolved: &[Resolved]) -> Vec<Vec<String>> {
    d.steps
        .iter()
        .zip(resolved)
        .map(|(s, r)| {
            let descs = r.descriptors();
            let mut out = Vec::new();
            for l in d.links.iter().filter(|l| l.to_step == s.id) {
                let typed = descs
                    .iter()
                    .any(|desc| desc.inputs.iter().any(|t| t.id == l.input));
                let role = descs.iter().any(|desc| desc.roles.contains(&l.input));
                if (typed || role) && !out.contains(&l.from_step) {
                    out.push(l.from_step.clone());
                }
            }
            out
        })
        .collect()
}

/// The steps each step may go on without, by the step's place: those whose
/// every link into it feeds an `optional` typed input (after the third
/// review, finding 9). Where such a step ended without a run, or a unit of
/// it failed or was skipped, the unit here runs without what it fed and
/// records why.
pub(crate) fn optional_from(d: &Workflow, resolved: &[Resolved]) -> Vec<Vec<String>> {
    d.steps
        .iter()
        .zip(resolved)
        .map(|(s, r)| {
            let descs = r.descriptors();
            let optional = |input: &str| {
                !descs.is_empty()
                    && descs
                        .iter()
                        .all(|desc| desc.inputs.iter().any(|t| t.id == input && t.optional))
            };
            let froms: Vec<&String> = d
                .links
                .iter()
                .filter(|l| l.to_step == s.id)
                .map(|l| &l.from_step)
                .collect();
            let mut out = Vec::new();
            for from in froms {
                if out.contains(from) {
                    continue;
                }
                let all_optional = d
                    .links
                    .iter()
                    .filter(|l| l.to_step == s.id && &l.from_step == from)
                    .all(|l| optional(&l.input));
                if all_optional {
                    out.push(from.clone());
                }
            }
            out
        })
        .collect()
}

/// The plan's bindings of a step's typed inputs to the steps that feed
/// them (after the third review, finding 4): no run yet, so the pre-flight
/// reads each as fed by its step, never as missing.
fn planned_bound(d: &Workflow, i: usize, r: &Resolved) -> crate::pipelines::Bound {
    let s = &d.steps[i];
    let mut bound = crate::pipelines::Bound::new();
    for l in d.links.iter().filter(|l| l.to_step == s.id) {
        let typed = r
            .descriptors()
            .iter()
            .any(|desc| desc.inputs.iter().any(|t| t.id == l.input));
        let Some(from) = d.steps.iter().position(|q| q.id == l.from_step) else {
            continue;
        };
        if typed {
            bound.insert(
                l.input.clone(),
                crate::pipelines::Fed::planned(from, &l.from_step, &l.output, &l.input),
            );
        }
    }
    bound
}

/// The steps a person's knobs name, checked against the document's: a knob
/// on a step the document lacks is refused with the step ids named (after
/// review, finding 9 of the second review), at the plan and at the start.
pub(crate) fn knob_steps(d: &Workflow, person: &Person) -> Result<(), String> {
    let unknown: Vec<&str> = person
        .knobs
        .keys()
        .filter(|k| !d.steps.iter().any(|s| s.id == **k))
        .map(String::as_str)
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{}: the knobs name a step {} the workflow lacks; its steps are {}",
        d.name,
        unknown.join(", "),
        d.steps
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The knobs a person set for a step, checked against what the document
/// lets them set.
pub(crate) fn step_params(
    d: &Workflow,
    step: &doc::Step,
    person: &Person,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (k, v) in person.knobs.get(&step.id).into_iter().flatten() {
        if !step.knobs.contains(k) {
            return Err(format!(
                "{}: the step {} lets a person set {}, not {k}",
                d.name,
                step.id,
                if step.knobs.is_empty() {
                    "no knob".to_string()
                } else {
                    step.knobs.join(", ")
                }
            ));
        }
        out.push((k.clone(), v.clone()));
    }
    Ok(out)
}

/// The goal and switches a step judges under: the document's, the person's
/// goal where the document names none.
pub(crate) fn step_person(step: &doc::Step, person: &Person) -> crate::pipeline_pack::PersonAsk {
    crate::pipeline_pack::PersonAsk {
        goal: step.goal.clone().or_else(|| person.goal.clone()),
        switches: step
            .switches
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        let_run: false,
    }
}

/// `POST /api/workflows/{id}/plan` and `nils workflow plan` (section 8.3):
/// each link's verdict and each step's pre-flight over the scope, before
/// anything runs.
pub(crate) fn plan(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    scope: &Scope,
    person: &Person,
) -> Result<Value, String> {
    let d = doc::from_value(w.document.clone()).map_err(|e| format!("{}: {e}", w.label()))?;
    knob_steps(&d, person)?;
    let resolved = resolve_steps(registry.store(), &d);
    let verdicts = doc::plan_links(&d, &members(&d, &resolved));
    let whole = waits_whole(&d, &resolved);
    let pack = scope
        .pack_dir
        .clone()
        .or_else(|| crate::pack_dir(home, None).ok())
        .and_then(|dir| nils_pack::load(&dir.join(&scope.pack), None).ok());
    let mut steps = Vec::new();
    // a family step's goal and switches the family does not offer refuse
    // the plan, as `nils run <family>` refuses them (after the third review,
    // finding 5): a plan that says runnable is one the run accepts
    let mut step_refusals: Vec<String> = Vec::new();
    for (i, (s, r)) in d.steps.iter().zip(&resolved).enumerate() {
        // the knobs of every step, a family's too: a knob the step does not
        // let a person set is refused here as at the run
        let params = step_params(&d, s, person)?;
        let mut doc_ = json!({
            "step": s.id, "of": s.of, "name": s.name, "reference": s.reference(),
            "pipeline": r.run_label(),
            "pipeline_id": r.pipeline.as_ref().map(|p| p.id),
            "goal": s.goal.clone().or_else(|| person.goal.clone()),
            "switches": s.switches, "knobs": s.knobs,
            "refused": r.refusal,
            "waits_for": d.fed_by(&s.id),
            // the steps it waits for whole (a typed input or a role fed by a
            // link); the rest of its links chain per unit
            "waits_whole": whole[i],
        });
        if let Some(p) = &r.pipeline {
            let pre = crate::preflight::check(
                registry,
                &crate::preflight::Asked {
                    home: home.dir(),
                    dcm2niix: Path::new(crate::tools::DCM2NIIX),
                    pipeline: &p.label(),
                    pack: pack.as_ref(),
                    handle: scope.handle,
                    selection: scope.selection.as_deref(),
                    params: &params,
                    main: scope.main.clone(),
                    resume: None,
                    replaced: false,
                    person: step_person(s, person),
                    bound: planned_bound(&d, i, r),
                },
            );
            doc_["preflight"] = match pre {
                Ok(v) => v,
                Err(e) => json!({"error": e}),
            };
        }
        if let Some((stored, fm)) = &r.family {
            // Wave 7b section 7: a family step plans as the family's
            // pre-flight routes the scope's units, over its member rows
            doc_["family"] = json!(stored.name);
            doc_["members"] = json!(
                fm.iter()
                    .map(|m| json!({
                        "member": m.key, "pipeline": m.pipeline.label(), "pipeline_id": m.pipeline.id,
                    }))
                    .collect::<Vec<_>>()
            );
            doc_["preflight"] =
                match family_preflight(registry, stored, scope, &step_person(s, person)) {
                    Ok(v) => v,
                    Err(e) => {
                        let words = format!("step {}: {e}", s.id);
                        doc_["refused"] = json!(words);
                        step_refusals.push(words);
                        json!({"error": e})
                    }
                };
        }
        steps.push(doc_);
    }
    let counts = doc::verdict_counts(&verdicts);
    let runnable = resolved.iter().all(|r| r.refusal.is_none())
        && step_refusals.is_empty()
        && verdicts.iter().all(|v| v.verdict != "refused");
    Ok(json!({
        "workflow": w.label(), "workflow_id": w.id, "name": w.name, "version": w.version,
        "handle": scope.handle, "selection": scope.selection, "main": scope.main,
        "person": person.doc(),
        "steps": steps,
        "links": verdicts,
        "verdicts": counts,
        "runnable": runnable,
        "words": if runnable {
            format!("{} steps planned; every link holds", d.steps.len())
        } else {
            let refused: Vec<String> = resolved.iter().filter_map(|r| r.refusal.clone())
                .chain(step_refusals.iter().cloned())
                .chain(verdicts.iter().filter(|v| v.verdict == "refused").map(|v| v.words.clone()))
                .collect();
            format!("not runnable: {}", refused.join("; "))
        },
    }))
}

fn print_plan(v: &Value) {
    println!(
        "{}: {}",
        v["workflow"].as_str().unwrap_or_default(),
        v["words"].as_str().unwrap_or_default()
    );
    for l in v["links"].as_array().into_iter().flatten() {
        println!(
            "  {} {}: {}",
            l["verdict"].as_str().unwrap_or_default(),
            l["link"].as_str().unwrap_or_default(),
            l["words"].as_str().unwrap_or_default()
        );
    }
    for s in v["steps"].as_array().into_iter().flatten() {
        let pre = &s["preflight"];
        let units = &pre["units"];
        println!(
            "  step {} ({}): {}",
            s["step"].as_str().unwrap_or_default(),
            s["pipeline"].as_str().unwrap_or("refused"),
            match s["refused"].as_str() {
                Some(r) => r.to_string(),
                None if pre["error"].is_string() =>
                    pre["error"].as_str().unwrap_or_default().to_string(),
                None => format!(
                    "{} units, {} missing; judged: {} ready, {} skipped",
                    units["total"],
                    units["missing"],
                    pre["units_judged"]["ready"],
                    pre["units_judged"]["skipped"]
                ),
            }
        );
    }
}

/// The doors of the catalog and the plan: `GET /api/workflows`, `GET
/// /api/workflows/{id}`, `POST /api/workflows`, `POST
/// /api/workflows/{id}/plan`; the run and its doors are in `workflow_run`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    home: &Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    method: &str,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    Some(match segs {
        ["api", "workflows"] if get => (|| {
            let list = rows::list(registry.store()).map_err(err)?;
            Ok(Reply::ok(json!({
                "workflows": list.iter().map(workflow_doc).collect::<Vec<_>>(),
                "names": names(&list),
            })))
        })(),
        ["api", "workflows"] if post => (|| {
            let doc_ = crate::serve::json_body(body)?;
            let text = match &doc_["document"] {
                Value::String(s) => s.clone(),
                Value::Object(_) => serde_json::to_string(&doc_["document"]).unwrap_or_default(),
                _ => {
                    return Err(Reply::error(
                        400,
                        "document is the nils.workflow.yml text, or the document as JSON",
                    ));
                }
            };
            // two writers adding versions of one name at once: the one that
            // lost the version answers 409, never a constraint's words
            let v = add_text(registry, &text, None).map_err(|e| {
                let code = if e.message.contains(rows::VERSION_TAKEN) {
                    409
                } else {
                    400
                };
                Reply::error(code, e.message)
            })?;
            Ok(Reply::ok(v))
        })(),
        ["api", "workflows", id] if get => (|| {
            let w = resolve_door(registry.store(), id)?;
            Ok(Reply::ok(workflow_doc(&w)))
        })(),
        ["api", "workflows", id, "plan"] if post => (|| {
            let w = resolve_door(registry.store(), id)?;
            let doc_ = crate::serve::json_body(body)?;
            let scope = scope_of_body(home, registry, &w, &doc_, pack_dir)?;
            let person = Person::of_body(&doc_).map_err(|e| Reply::error(400, e))?;
            let mut v =
                plan(home, registry, &w, &scope, &person).map_err(|e| Reply::error(400, e))?;
            if !quasi {
                crate::pipelines::plain(&mut v);
            }
            Ok(Reply::ok(v))
        })(),
        ["api", "workflows", id, "run"] if post => {
            let _ = query;
            crate::workflow_run::run_door(home, pack_dir, registry, caller, id, body)
        }
        ["api", "workflow-runs"] if get => (|| {
            let list = rows::runs(registry.store(), None, 100).map_err(err)?;
            Ok(Reply::ok(json!({
                "runs": list.iter().map(|r| crate::workflow_run::run_doc(registry.store(), r)).collect::<Vec<_>>(),
            })))
        })(),
        ["api", "workflow-runs", id] if get => (|| {
            let id: i64 = id
                .parse()
                .map_err(|_| Reply::error(404, "a workflow run is named by its id"))?;
            let r = rows::run(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no workflow run {id}")))?;
            let mut v = crate::workflow_run::run_doc(registry.store(), &r);
            if !quasi {
                crate::pipelines::plain(&mut v);
            }
            Ok(Reply::ok(v))
        })(),
        _ => return None,
    })
}

fn resolve_door(store: &mut Store, id: &str) -> Result<rows::Workflow, Reply> {
    rows::resolve(store, id)
        .map_err(err)?
        .ok_or_else(|| Reply::error(404, format!("no workflow {id}")))
}

/// The scope a door's body names: `select` or `handle`, and `pack`.
pub(crate) fn scope_of_body(
    home: &Home,
    registry: &mut Registry,
    w: &rows::Workflow,
    body: &Value,
    pack_dir: Option<&Path>,
) -> Result<Scope, Reply> {
    scope_from(
        home,
        registry,
        w,
        body["select"].as_str(),
        body["handle"].as_i64(),
        body["pack"].as_str().unwrap_or("mri"),
        pack_dir.map(Path::to_path_buf),
    )
    .map_err(|e| Reply::error(400, e.message))?
    .ok_or_else(|| {
        Reply::error(
            400,
            "a workflow is planned or run over a scope: {select: \"selection:<name>@<v>\"}, a dataset's or a cohort's main scans {select: \"main:<role>@dataset:<name>\"} (or {select: \"main@dataset:<name>\"} for each role the steps need), or {handle: <id>}",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STACKS: &str = r#"name: stand-in
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/stand-in@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
command-line: "run [Manifest] [OutputLocation]"
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  units: apart
  outputs:
    - {id: out, kind: image, path-template: "stack-{stack}/out.nii.gz"}
"#;

    const BIDS: &str = r#"name: stand-in-bids
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/stand-in-bids@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
command-line: "run [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w, flair]}
  inputs:
    - {id: mask, type: "derivative:mask", optional: true}
    - {id: prior, type: "derivative:image"}
  outputs:
    - {id: seen, kind: output, path-template: "sub-{subject}/ses-{session}/anat/*_desc-seen_T2w.nii.gz"}
"#;

    fn resolved(text: &str) -> Resolved {
        Resolved {
            pipeline: None,
            descriptor: Some(descriptor::parse(text).unwrap()),
            pack: None,
            family: None,
            refusal: None,
        }
    }

    /// After the third review (findings 1 and 9): a step fed through a
    /// role waits for the whole step before, as one fed through a typed
    /// input does; a step whose every link from another feeds an optional
    /// input may go on without that step; an order-only link does neither.
    #[test]
    fn a_role_or_a_typed_input_waits_whole_and_an_optional_input_may_be_unfed() {
        let text = r#"nils_workflow: 1
name: waits
steps:
  - {id: a, pipeline: stand-in}
  - {id: m, pipeline: stand-in}
  - {id: o, pipeline: stand-in}
  - {id: b, pipeline: stand-in-bids}
links:
  - {from: a.out, to: b.flair}
  - {from: m.out, to: b.mask}
  - {from: o.out, to: b.nothing}
"#;
        let d = doc::parse(text).unwrap();
        let r = vec![
            resolved(STACKS),
            resolved(STACKS),
            resolved(STACKS),
            resolved(BIDS),
        ];
        let whole = waits_whole(&d, &r);
        assert_eq!(
            whole[3],
            vec!["a".to_string(), "m".to_string()],
            "{whole:?}"
        );
        assert!(whole[0].is_empty() && whole[1].is_empty() && whole[2].is_empty());
        let optional = optional_from(&d, &r);
        assert_eq!(optional[3], vec!["m".to_string()], "{optional:?}");
        // the plan's bindings: the typed input, not the role
        let bound = planned_bound(&d, 3, &r[3]);
        assert_eq!(bound.len(), 1, "{bound:?}");
        assert_eq!(bound["mask"].planned_words(), "fed by step m");
        assert!(bound["mask"].is_planned());
        // a step with a role link and a typed link from one step waits whole
        // and is not optional, since the role is not
        let text = r#"nils_workflow: 1
name: both
steps:
  - {id: a, pipeline: stand-in}
  - {id: b, pipeline: stand-in-bids}
links:
  - {from: a.out, to: b.flair}
  - {from: a.out, to: b.mask}
"#;
        let d = doc::parse(text).unwrap();
        let r = vec![resolved(STACKS), resolved(BIDS)];
        assert_eq!(waits_whole(&d, &r)[1], vec!["a".to_string()]);
        assert!(optional_from(&d, &r)[1].is_empty());
    }
}
