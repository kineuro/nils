// SPDX-License-Identifier: AGPL-3.0-only

//! `nils ask-scale`: the ask on a registry of production size. `build`
//! makes the scale registry of `nils_synth::build_scale` in an empty
//! registry, with its sessions and, on Postgres, its statistics; `time`
//! compiles and runs every document of a fixture directory on it, a few
//! times each, and prints how long the statement took and what it
//! answered: the rows, the subjects, and the content hash of the answer,
//! and how long the funnel a draft counts took under a draft's time, in
//! one pass per set and by a statement per stage.
//!
//! The answers are the reference a change to the compiler is held to: the
//! generator is code and a seed, so a registry built again holds the same
//! rows, and `--record FILE` keeps what this build answered per backend
//! while `--reference FILE` fails where an answer differs from what was
//! kept. A measurement and a check run by hand; hidden from the help.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Subcommand};
use nils_ask::compile::{Context, compile};
use nils_ask::exec::{self, Bounds};
use nils_ask::validate::{Class, Scope};
use nils_ask::{diagnose, parse, prepare};
use nils_catalog::{Caps, Catalog};
use nils_registry::Backend;
use nils_registry::home::Home;
use nils_registry::session::Scheme;
use serde_json::{Value, json};

use crate::{Exit, fail, open, usage};

#[derive(Debug, Subcommand)]
pub(crate) enum AskScaleCommand {
    /// Build the scale registry into an empty registry, with its sessions
    Build(BuildArgs),
    /// Time every document of a fixture directory and say what each answered
    Time(Box<TimeArgs>),
}

#[derive(Debug, Args)]
pub(crate) struct BuildArgs {
    /// The seed; the same seed builds the same registry on either backend
    #[arg(long, default_value_t = 7)]
    seed: u64,
    /// Subjects in all; 5,900 is a registry of some 490,000 stacks
    #[arg(long, default_value_t = 5_900)]
    subjects: usize,
}

#[derive(Debug, Args)]
pub(crate) struct TimeArgs {
    #[arg(long, value_name = "DIR")]
    pack_dir: PathBuf,
    #[arg(long, default_value = "mri")]
    pack: String,
    /// The fixture directory, `*.ask.yml`; a `*.plain.ask.yml` is read at
    /// detail plain, every other with quasi identifying fields raw
    #[arg(long, value_name = "DIR")]
    fixtures: PathBuf,
    /// Only the fixtures whose name holds this text
    #[arg(long, value_name = "TEXT")]
    only: Option<String>,
    /// Runs per fixture
    #[arg(long, default_value_t = 5)]
    runs: usize,
    /// The time a statement may take, in milliseconds
    #[arg(long, default_value_t = 120_000)]
    timeout_ms: u64,
    /// Keep what each fixture answered on this backend in this file
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,
    /// Fail where a fixture's answer differs from what this file keeps
    #[arg(long, value_name = "FILE")]
    reference: Option<PathBuf>,
    /// Write each fixture's statement beside its name in this directory
    #[arg(long, value_name = "DIR")]
    sql: Option<PathBuf>,
    /// Keep each fixture's whole diagnosis on this backend in this file:
    /// the funnel's stages, the drops, the ties and the coarse dates,
    /// counted with no time for the whole
    #[arg(long, value_name = "FILE")]
    record_funnel: Option<PathBuf>,
    /// Fail where a fixture's whole diagnosis differs from what this file
    /// keeps
    #[arg(long, value_name = "FILE")]
    reference_funnel: Option<PathBuf>,
    /// Count each fixture's diagnosis a second time with a statement per
    /// stage, and fail where the two differ
    #[arg(long)]
    check_funnel: bool,
    /// How many readers a funnel's statements are counted on beside the
    /// registry's own connection, as the draft door counts them
    #[arg(long, default_value_t = 4)]
    beside: usize,
}

pub(crate) fn ask_scale(home: &Home, cmd: AskScaleCommand) -> Result<(), Exit> {
    match cmd {
        AskScaleCommand::Build(args) => build(home, args),
        AskScaleCommand::Time(args) => time(home, *args),
    }
}

fn build(home: &Home, args: BuildArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let started = Instant::now();
    let manifest = nils_synth::build_scale(
        &mut registry,
        &nils_synth::ScalePlan {
            seed: args.seed,
            subjects: args.subjects,
        },
    )?;
    let scheme = Scheme::default();
    let anchors = nils_session::Anchors::resolve(&mut registry, &scheme, BTreeMap::new())
        .map_err(|e| fail(e.to_string()))?;
    let sessions = nils_session::ensure(&mut registry, &scheme, &anchors, None, false)
        .map_err(|e| fail(e.to_string()))?;
    // a registry filled at once has no statistics yet, where one that grew
    // has the ones autovacuum gathered: a plan is timed on those
    if registry.config().backend == Backend::Postgres {
        registry.store().batch("ANALYZE")?;
    }
    let mut doc = serde_json::to_value(&manifest).map_err(|e| fail(e.to_string()))?;
    doc["sessions"] = json!(sessions.sessions);
    doc["built_in_s"] = json!(started.elapsed().as_secs());
    println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    Ok(())
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn time(home: &Home, args: TimeArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let pack = crate::ask_cli::load_pack(&args.pack_dir, &args.pack)?;
    let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
    let scheme = Scheme::default();
    let backend = format!("{:?}", registry.config().backend).to_lowercase();
    let read = |path: &PathBuf| -> Result<Value, Exit> {
        let text =
            std::fs::read_to_string(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        serde_json::from_str(&text).map_err(|e| usage(format!("{}: {e}", path.display())))
    };
    let reference = args.reference.as_ref().map(&read).transpose()?;
    let funnel_reference = args.reference_funnel.as_ref().map(&read).transpose()?;
    let mut funnels = serde_json::Map::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&args.fixtures)
        .map_err(|e| usage(format!("{}: {e}", args.fixtures.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".ask.yml"))
        .filter(|p| {
            args.only
                .as_ref()
                .is_none_or(|o| p.to_string_lossy().contains(o.as_str()))
        })
        .collect();
    files.sort();
    let mut shapes = Vec::new();
    let mut answers = serde_json::Map::new();
    let mut differ: Vec<String> = Vec::new();
    for path in &files {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().trim_end_matches(".ask.yml").to_string())
            .unwrap_or_default();
        let text =
            std::fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        let ask = parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        let drafted = ask.clone();
        // a verb of the command line holds every class; a fixture named
        // plain is read as a caller with none, so what is held back is timed
        let scope = if name.ends_with(".plain") {
            Scope::default()
        } else {
            Scope {
                federated: false,
                classes: [Class::QuasiIdentifying, Class::Sensitive]
                    .into_iter()
                    .collect(),
                unsealed: false,
            }
        };
        let prepared = prepare(ask, &catalog, &scope).map_err(|e| usage(format!("{name}: {e}")))?;
        let store = registry.store();
        let ctx = Context {
            names: &catalog,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: scheme.window_days,
            scheme_digest: scheme.digest(),
            after: None,
            limit: None,
            withhold_sealed: !scope.unsealed,
        };
        let compiled = compile(&prepared.ask, &prepared.validated, &ctx)
            .map_err(|e| usage(format!("{name}: {e}")))?;
        if let Some(dir) = &args.sql {
            std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let file = dir.join(format!("{name}.{backend}.sql"));
            let params: Vec<String> = compiled.params.iter().map(|p| format!("{p:?}")).collect();
            std::fs::write(
                &file,
                format!("{}\n-- {}\n", compiled.sql, params.join(", ")),
            )
            .map_err(|e| fail(format!("{}: {e}", file.display())))?;
        }
        let mut times: Vec<u128> = Vec::new();
        let mut answer: Option<Value> = None;
        let mut error: Option<String> = None;
        for _ in 0..args.runs.max(1) {
            let started = Instant::now();
            let result = exec::run(
                store,
                &compiled,
                Bounds {
                    timeout_ms: args.timeout_ms,
                    max_rows: 5_000_000,
                    max_bytes: 4 * 1024 * 1024 * 1024,
                },
            );
            times.push(started.elapsed().as_millis());
            match result {
                Ok(a) => {
                    // a count answers its rows and subjects; a list answers
                    // as many rows as it has, and its hash says which
                    let counted = compiled.columns == ["rows", "subjects"];
                    let cell = |i: usize| -> Value {
                        a.rows
                            .first()
                            .and_then(|r| r.0.get(i))
                            .map(nils_ask::handle::cell_json)
                            .unwrap_or(Value::Null)
                    };
                    answer = Some(json!({
                        "rows": if counted { cell(0) } else { json!(a.rows.len()) },
                        "subjects": if counted { cell(1) } else { Value::Null },
                        "hash": a.content_hash,
                        "held": a.rows_held || !a.held_back.is_empty(),
                    }));
                }
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        times.sort_unstable();
        // the funnel a draft counts, under the door's bounds and the time
        // a draft gives all its counts together: how long it took, and
        // whether every count was made within it
        let caps = Caps::default();
        let door = Bounds {
            timeout_ms: caps.sync_timeout_ms,
            max_rows: caps.sync_max_rows,
            max_bytes: caps.sync_max_bytes,
        };
        // counted in one pass per set, as a draft counts it, and by a
        // statement per stage, each the least of the runs
        let mut funnel = json!({});
        // as a draft counts it: one pass per set, and the statements that
        // are left at the same time on readers beside; then the same on
        // one connection, and by a statement per stage
        let mut beside: Vec<nils_registry::Store> = (0..args.beside)
            .map_while(|_| registry.open_ask_reader(None).ok())
            .collect();
        for (how, ms, whole) in [
            (2, "ms", "whole"),
            (1, "alone_ms", "alone_whole"),
            (0, "by_stage_ms", "by_stage_whole"),
        ] {
            let mut least: Option<(u128, Value)> = None;
            for _ in 0..args.runs.max(1) {
                let started = Instant::now();
                let counted = if how > 0 {
                    diagnose::diagnose_beside(
                        &mut registry,
                        drafted.clone(),
                        Vec::new(),
                        &catalog,
                        &scope,
                        &scheme,
                        door,
                        false,
                        None,
                        Some(caps.draft_timeout_ms),
                        if how == 2 { &mut beside[..] } else { &mut [] },
                    )
                } else {
                    diagnose::diagnose_by_stage(
                        &mut registry,
                        drafted.clone(),
                        Vec::new(),
                        &catalog,
                        &scope,
                        &scheme,
                        door,
                        false,
                        None,
                        Some(caps.draft_timeout_ms),
                    )
                };
                let took = started.elapsed().as_millis();
                let said = match counted {
                    Ok(d) => json!(d.unavailable.is_none()),
                    Err(e) => json!(e.to_string()),
                };
                if least.as_ref().is_none_or(|(l, _)| took < *l) {
                    least = Some((took, said));
                }
            }
            if let Some((took, said)) = least {
                funnel[ms] = json!(took);
                funnel[whole] = said;
            }
        }
        // the diagnosis whole, with no time for all its counts: what a
        // change to how the funnel is counted may not change
        if args.record_funnel.is_some() || funnel_reference.is_some() || args.check_funnel {
            let whole = diagnose::diagnose_beside(
                &mut registry,
                parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?,
                Vec::new(),
                &catalog,
                &scope,
                &scheme,
                Bounds {
                    timeout_ms: args.timeout_ms,
                    max_rows: caps.sync_max_rows,
                    max_bytes: caps.sync_max_bytes,
                },
                false,
                None,
                None,
                &mut beside[..],
            );
            let whole = match whole {
                Ok(d) => serde_json::to_value(&d).unwrap_or(Value::Null),
                Err(e) => json!({"error": e.to_string()}),
            };
            if let Some(kept) = funnel_reference
                .as_ref()
                .map(|r| &r[&backend][&name])
                .filter(|kept| !kept.is_null())
            {
                let same = *kept == whole;
                if !same {
                    differ.push(format!(
                        "{name}: the diagnosis differs: kept {kept}, counted {whole}"
                    ));
                }
                funnel["same_as_reference"] = json!(same);
            }
            if args.check_funnel {
                let by_stage = diagnose::diagnose_by_stage(
                    &mut registry,
                    parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?,
                    Vec::new(),
                    &catalog,
                    &scope,
                    &scheme,
                    Bounds {
                        timeout_ms: args.timeout_ms,
                        max_rows: caps.sync_max_rows,
                        max_bytes: caps.sync_max_bytes,
                    },
                    false,
                    None,
                    None,
                );
                let by_stage = match by_stage {
                    Ok(d) => serde_json::to_value(&d).unwrap_or(Value::Null),
                    Err(e) => json!({"error": e.to_string()}),
                };
                let same = by_stage == whole;
                if !same {
                    differ.push(format!(
                        "{name}: the diagnosis differs from a count per stage: {by_stage}, in one pass {whole}"
                    ));
                }
                funnel["same_as_by_stage"] = json!(same);
            }
            funnels.insert(name.clone(), whole);
        }
        let answered = answer.clone().unwrap_or(Value::Null);
        // a fixture the reference keeps no answer of on this backend (one
        // that did not end when the reference was taken) is held to nothing
        let same = reference
            .as_ref()
            .map(|r| &r[&backend][&name])
            .filter(|kept| !kept.is_null())
            .map(|kept| {
                let same = !answered.is_null() && *kept == answered;
                if !same {
                    differ.push(format!("{name}: kept {kept}, answered {answered}"));
                }
                same
            });
        answers.insert(name.clone(), answered.clone());
        shapes.push(json!({
            "fixture": name,
            "runs": times.len(),
            "min_ms": times.first(),
            "p50_ms": percentile(&times, 50.0),
            "max_ms": times.last(),
            "answer": answered,
            "same_as_reference": same,
            "funnel": funnel,
            "error": error,
        }));
    }
    if let Some(path) = &args.record_funnel {
        let mut kept = if path.exists() {
            read(path)?
        } else {
            json!({})
        };
        let mut mine = kept[&backend].as_object().cloned().unwrap_or_default();
        mine.extend(funnels);
        kept[&backend] = Value::Object(mine);
        std::fs::write(
            path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&kept).unwrap_or_default()
            ),
        )
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }
    if let Some(path) = &args.record {
        let mut kept = if path.exists() {
            read(path)?
        } else {
            json!({})
        };
        // a run of some fixtures keeps what the others answered before
        let mut mine = kept[&backend].as_object().cloned().unwrap_or_default();
        for (name, answer) in &answers {
            if !answer.is_null() {
                mine.insert(name.clone(), answer.clone());
            }
        }
        kept[&backend] = Value::Object(mine);
        std::fs::write(
            path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&kept).unwrap_or_default()
            ),
        )
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }
    let doc = json!({
        "backend": backend,
        "runs": args.runs,
        "timeout_ms": args.timeout_ms,
        "shapes": shapes,
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    if !differ.is_empty() {
        return Err(fail(format!(
            "{} of {} fixtures answer differently from the reference:\n{}",
            differ.len(),
            files.len(),
            differ.join("\n")
        )));
    }
    Ok(())
}
