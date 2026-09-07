// SPDX-License-Identifier: AGPL-3.0-only

//! `nils ask`, the part the job path needs (Wave 4b §12.3, slice 9): `run`
//! a document to a handle, `promote` a handle into a cohort, and `time`
//! the fixtures for the caps of §11.5. The rest of the runner (explain,
//! validate, options, diagnose, describe, handles, selections, gate) is
//! slice 10's.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Subcommand};
use nils_ask::exec::Bounds;
use nils_ask::run::{self, Request};
use nils_ask::validate::{Class, Scope};
use nils_ask::{diagnose, document, parse, promote};
use nils_catalog::{Caps, Catalog};
use nils_registry::home::Home;
use nils_registry::job::{self, Claim, State};
use nils_registry::session::Scheme;
use serde_json::json;

use crate::{Exit, actor, fail, open, usage};

#[derive(Debug, Subcommand)]
pub(crate) enum AskCommand {
    /// Run a document to a handle: a stored document by handle, or a file
    Run(AskRunArgs),
    /// Promote a subject grain handle into a cohort (Wave 4b section 8.3)
    Promote(AskPromoteArgs),
    /// Time the fixtures on this registry, for the caps of section 11.5
    Time(AskTimeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AskRunArgs {
    /// A document handle, from POST /api/ask/documents
    #[arg(long, value_name = "ID", conflicts_with = "file")]
    document: Option<i64>,
    /// A document file, JSON or YAML
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
    /// A name for the handle; a named handle stores its ask
    #[arg(long)]
    name: Option<String>,
    /// Honour the document's `keep`
    #[arg(long)]
    keep: bool,
    /// The pack directory
    #[arg(long, value_name = "DIR")]
    pack_dir: PathBuf,
    /// The pack, by name in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Print the outcome as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AskPromoteArgs {
    /// The handle, a complete subject grain one
    #[arg(long, value_name = "ID")]
    handle: i64,
    /// The cohort's name
    #[arg(long)]
    cohort: String,
    /// Open the cohort when it does not exist
    #[arg(long)]
    create: bool,
    /// Why, on every interval opened
    #[arg(long)]
    reason: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AskTimeArgs {
    #[arg(long, value_name = "DIR")]
    pack_dir: PathBuf,
    #[arg(long, default_value = "mri")]
    pack: String,
    /// The fixture directory, `*.ask.yml`
    #[arg(long, value_name = "DIR")]
    fixtures: PathBuf,
    /// Runs per fixture
    #[arg(long, default_value_t = 5)]
    runs: usize,
}

fn principal() -> String {
    std::env::var("NILS_PRINCIPAL")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(actor)
}

fn load_pack(dir: &std::path::Path, name: &str) -> Result<nils_pack::Pack, Exit> {
    nils_pack::load(&dir.join(name), None)
        .map_err(|e| usage(format!("the pack {name} in {}: {e}", dir.display())))
}

fn bounds() -> Bounds {
    let caps = Caps::default();
    Bounds {
        timeout_ms: caps.sync_timeout_ms,
        max_rows: caps.sync_max_rows,
        max_bytes: caps.sync_max_bytes,
    }
}

/// A verb of the command line holds every class: it runs as the operator
/// at the keyboard, or as the worker under the principal that queued it.
fn scope() -> Scope {
    Scope {
        federated: false,
        classes: [Class::QuasiIdentifying, Class::Sensitive]
            .into_iter()
            .collect(),
    }
}

pub(crate) fn ask_command(home: &Home, cmd: AskCommand) -> Result<(), Exit> {
    match cmd {
        AskCommand::Run(args) => ask_run(home, args),
        AskCommand::Promote(args) => ask_promote(home, args),
        AskCommand::Time(args) => ask_time(home, args),
    }
}

fn ask_run(home: &Home, args: AskRunArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let pack = load_pack(&args.pack_dir, &args.pack)?;
    let who = principal();
    let ask = match (&args.document, &args.file) {
        (Some(id), _) => {
            document::get(registry.store(), *id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no document {id}")))?
                .ask
        }
        (None, Some(path)) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| usage(format!("{}: {e}", path.display())))?;
            parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?
        }
        (None, None) => return Err(usage("--document ID or --file FILE")),
    };
    let scheme = match &ask.scheme {
        Some(nils_ask::ast::SchemeRef::Name(n)) if n != "default" && n != "day" => {
            crate::stored_scheme(&mut registry, n)?
        }
        Some(nils_ask::ast::SchemeRef::Inline(m)) => {
            Scheme::from_json(&serde_json::to_string(m).unwrap_or_default())
                .map_err(|e| usage(format!("the inline scheme: {e}")))?
        }
        _ => Scheme::default(),
    };
    let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
    let job_name = args
        .name
        .clone()
        .or_else(|| ask.name.clone())
        .unwrap_or_else(|| "ask".to_string());
    let job = job::claim(
        registry.store(),
        &Claim {
            kind: "ask",
            name: &job_name,
            args: json!({"document": args.document, "file": args.file, "name": args.name, "keep": args.keep}),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    let scope = scope();
    let pack_version = pack.version.to_string();
    let node = job::hostname();
    let outcome = run::run(
        &mut registry,
        Request {
            ask,
            names: &catalog,
            scope: &scope,
            principal: &who,
            node: &node,
            pack_version: Some(&pack_version),
            scheme: &scheme,
            bounds: bounds(),
            page_rows: Caps::default().page_rows as usize,
            name: args.name.as_deref(),
            keep: args.keep,
            after: None,
            limit: None,
            may_project_raw: true,
            purpose: Some("nils ask run"),
            reader: None,
        },
    );
    match outcome {
        Ok(out) => {
            job::finish(registry.store(), job, State::Done, None)
                .map_err(|e| fail(e.to_string()))?;
            let doc = json!({
                "handle": out.handle.id,
                "hash": out.hash,
                "grain": out.handle.grain,
                "row_count": out.handle.row_count,
                "content_hash": out.handle.content_hash,
                "truncated": out.answer.truncated,
                "columns": out.answer.columns,
                "kept": out.kept.iter().map(|k| k.id).collect::<Vec<_>>(),
                "drift": out.drift,
                "job": job,
            });
            if args.json {
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
            } else {
                println!(
                    "nils ask run   handle {}   {} rows   hash {}   {}",
                    out.handle.id,
                    out.handle.row_count,
                    out.hash,
                    if out.answer.truncated {
                        "truncated"
                    } else {
                        "complete"
                    }
                );
            }
            Ok(())
        }
        Err(e) => {
            let _ = job::finish(registry.store(), job, State::Failed, Some(&e.to_string()));
            Err(fail(e.to_string()))
        }
    }
}

fn ask_promote(home: &Home, args: AskPromoteArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let who = principal();
    let job = job::claim(
        registry.store(),
        &Claim {
            kind: "ask",
            name: &args.cohort,
            args: json!({"handle": args.handle, "cohort": args.cohort, "create": args.create}),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    match promote::promote(
        &mut registry,
        args.handle,
        &args.cohort,
        &who,
        args.reason.as_deref(),
        args.create,
    ) {
        Ok(p) => {
            job::finish(registry.store(), job, State::Done, None)
                .map_err(|e| fail(e.to_string()))?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
            } else {
                println!(
                    "nils ask promote   cohort {}   added {}   already {}   epoch {}{}",
                    p.cohort,
                    p.added,
                    p.already,
                    p.epoch,
                    if p.source_moved {
                        "   the source ask has moved"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        Err(e) => {
            let _ = job::finish(registry.store(), job, State::Failed, Some(&e.to_string()));
            Err(fail(e.to_string()))
        }
    }
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn ask_time(home: &Home, args: AskTimeArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let pack = load_pack(&args.pack_dir, &args.pack)?;
    let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
    let scope = scope();
    let scheme = Scheme::default();
    let who = principal();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&args.fixtures)
        .map_err(|e| usage(format!("{}: {e}", args.fixtures.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".ask.yml"))
        .collect();
    files.sort();
    let mut shapes = Vec::new();
    for path in &files {
        let text =
            std::fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        let ask = parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().trim_end_matches(".ask.yml").to_string())
            .unwrap_or_default();
        let mut times: Vec<u128> = Vec::new();
        let mut rows = 0;
        let mut truncated = false;
        let mut error: Option<String> = None;
        for _ in 0..args.runs.max(1) {
            let started = Instant::now();
            let result = run::run(
                &mut registry,
                Request {
                    ask: ask.clone(),
                    names: &catalog,
                    scope: &scope,
                    principal: &who,
                    node: "time",
                    pack_version: None,
                    scheme: &scheme,
                    bounds: bounds(),
                    page_rows: 200,
                    name: None,
                    keep: false,
                    after: None,
                    limit: None,
                    may_project_raw: false,
                    purpose: None,
                    reader: None,
                },
            );
            times.push(started.elapsed().as_millis());
            match result {
                Ok(out) => {
                    rows = out.answer.rows.len();
                    truncated = out.answer.truncated;
                }
                Err(e) => error = Some(e.to_string()),
            }
        }
        times.sort_unstable();
        let started = Instant::now();
        let diagnosed = diagnose::diagnose(
            &mut registry,
            ask.clone(),
            Vec::new(),
            &catalog,
            &scope,
            &scheme,
            bounds(),
            false,
            None,
        );
        let diagnose_ms = started.elapsed().as_millis();
        shapes.push(json!({
            "fixture": name,
            "runs": times.len(),
            "p50_ms": percentile(&times, 50.0),
            "p95_ms": percentile(&times, 95.0),
            "rows": rows,
            "truncated": truncated,
            "error": error,
            "diagnose_ms": diagnose_ms,
            "diagnose_stages": diagnosed.map(|d| d.funnel.len()).unwrap_or(0),
        }));
    }
    let doc = json!({
        "backend": format!("{:?}", registry.config().backend).to_lowercase(),
        "runs": args.runs,
        "caps": Caps::default(),
        "shapes": shapes,
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    Ok(())
}
