// SPDX-License-Identifier: AGPL-3.0-only

//! `nils model` (record 42 S2, D15): the model registry at the keyboard.
//! `register` takes a card (`contracts/model/v1/card.schema.json`) and, with
//! `--artifact`, checks the card's digest against the file or fills it in;
//! `admit` records a check; `promote` promotes a model in its task and slot
//! (since record 55 D4 retiring none); `retire`; `list` and `show`. Each
//! writes the same rows and audit rows as the doors under `/api/models`.
//! `keep` (record 50) copies a registered model's artifact into the working
//! place a run reads models from, so a pipeline's model input finds it.
//!
//! Record 55 D4 and A5: `choose` sets the model that answers a task and
//! slot, or sends it back to its default, and `chosen` lists each task's;
//! `equivalence` measures the class of the difference between a model's
//! answers on a CPU and on a GPU from two runs over one fixed set, and
//! `admit --cpu --gpu` records it with the check.
//!
//! Record 55 ruling 22 (b): `certify` records a certificate that cites the
//! graded run that measured a model, at the engine's door under a person's
//! token (`nils login`, `--token` or `NILS_TOKEN`), so the person is the
//! identity the engine verified and never a local user name.
//!
//! Record 55 ruling 31 (a): `add-claims` adds to a registered card the
//! claims it lacks (the label set it was trained on, its tested claims, its
//! speed), additions only and each audited, as `POST /api/models/{id}/claims`
//! does; `show` lists what a card gained since it was registered.

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use nils_registry::home::Home;
use nils_registry::model::{self, Error, Filter, Model};

use crate::{Exit, actor, fail, open, usage};

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum ModelCommand {
    #[command(flatten)]
    Registry(RegistryCommand),
    /// Record a certificate that cites the graded run that measured a model
    /// (record 55 ruling 22 b), at the engine's door under a person's token.
    /// It unseals nothing, so one person records it; the audit keeps it
    Certify(CertifyArgs),
}

/// The verbs that work in a registry opened here.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum RegistryCommand {
    /// Register a model by its card: identified by the digest of its
    /// canonical artifact, neither admitted nor promoted
    Register {
        /// The card, a JSON file (contracts/model/v1), or - for standard input
        #[arg(long, value_name = "FILE")]
        card: PathBuf,
        /// The artifact itself: its sha256 is the card's digest, filled in
        /// when the card has none and refused when it names another
        #[arg(long, value_name = "FILE")]
        artifact: Option<PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Record a check on a model: one that passed admits it, one that
    /// failed is kept and moves nothing
    Admit {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// The check, a JSON file (suite, passed, checks), or - for standard input
        #[arg(long, value_name = "FILE")]
        check: PathBuf,
        /// The CPU run of the equivalence set, run:<id> or a results.json;
        /// with --gpu, the measured class joins the check (record 55 A5)
        #[arg(long, value_name = "RUN", requires = "gpu")]
        cpu: Option<String>,
        /// The GPU run of the same set
        #[arg(long, value_name = "RUN", requires = "cpu")]
        gpu: Option<String>,
        /// The equivalence set's name, kept with the measure
        #[arg(long, value_name = "NAME")]
        set: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Measure the class of the difference between a model's answers on a
    /// CPU and on a GPU, from two runs over one fixed set (record 55 A5)
    Equivalence {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// The CPU run: run:<id>, or a results.json
        #[arg(long, value_name = "RUN")]
        cpu: String,
        /// The GPU run: run:<id>, or a results.json
        #[arg(long, value_name = "RUN")]
        gpu: String,
        /// The equivalence set's name, kept with the measure
        #[arg(long, value_name = "NAME")]
        set: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Choose the model that answers its task and slot, or with --default
    /// send a task back to its default: the model a recorded certificate
    /// names, else the one promoted last, else the newest admitted
    Choose {
        /// The model: its id, its digest (sha256:...) or name@version
        #[arg(required_unless_present = "default", conflicts_with = "default")]
        model: Option<String>,
        /// Send this task back to its default
        #[arg(long, requires = "task")]
        default: bool,
        /// The task, with --default
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
        /// The slot, with --default: site or cohort:<name>
        #[arg(long, value_name = "SLOT", default_value = "site")]
        slot: String,
        /// Why, in the person's own words
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Every task and slot, the model that answers it and why
    Chosen {
        /// Only this task
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Promote an admitted model in its task and slot; since record 55 D4 it
    /// retires none, and the site chooses which model answers
    Promote {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// The review item a person accepted to promote it
        #[arg(long, value_name = "ID")]
        review_item: Option<i64>,
        /// Why, in the person's own words
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Retire a model: it answers no more, and its decisions keep naming it
    Retire {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// Why, in the person's own words
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Add the claims a registered model's card lacks (record 55, ruling 31
    /// a): the label set it was trained on, its tested claims, its speed.
    /// Additions only, each audited; a claim the card states already is
    /// refused, and so is any other field
    AddClaims {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// The claims: a JSON file in the card's own shape (trained_on,
        /// tested, speed), or one holding them as `claims` beside the
        /// `model`, its `digest` and `why`; - for standard input
        #[arg(long, value_name = "FILE")]
        claims: PathBuf,
        /// Why, in the person's own words; the file's when not given
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Keep a registered model's artifact where a run finds it: a copy in
    /// the lane's output place, under derivatives/models/<name>-<version>/,
    /// registered as a derivative of kind model (record 50)
    Keep {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// The artifact itself: its sha256 must be the model's digest. It
        /// is copied, never moved
        #[arg(long, value_name = "FILE")]
        artifact: PathBuf,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The registered models, oldest first
    List {
        /// Only this task, such as axis:body_part
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
        /// Only this slot: site or cohort:<name>
        #[arg(long, value_name = "SLOT")]
        slot: Option<String>,
        /// Only this state: registered, admitted, promoted or retired
        #[arg(long, value_name = "STATE")]
        state: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One model: its card, its state, the check that admitted it, the
    /// certificate that names it and every transition
    Show {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils model certify`: the model, the graded run it cites, and its result.
#[derive(Debug, Args)]
pub(crate) struct CertifyArgs {
    /// The model the run graded: its id, its digest (sha256:...) or name@version
    model: String,
    /// The engine, http://host:port: its local port or a tunnel to it
    #[arg(long, value_name = "URL")]
    server: String,
    /// The person's token; NILS_TOKEN, then the one nils login kept, when absent
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// The evidence run's MANIFEST.json: its sha256 is cited, with the run it names
    #[arg(long, value_name = "FILE", required_unless_present = "manifest_sha256")]
    evidence: Option<PathBuf>,
    /// The manifest's sha256, where the file is not at hand; with --run
    #[arg(
        long,
        value_name = "SHA256",
        conflicts_with = "evidence",
        requires = "run"
    )]
    manifest_sha256: Option<String>,
    /// The evidence run's name; read from the manifest where it is given
    #[arg(long, value_name = "RUN")]
    run: Option<String>,
    /// The commit that registered the plan before the run was drawn
    #[arg(long, value_name = "COMMIT")]
    plan_commit: String,
    /// Where the plan is written, in a few words
    #[arg(long, value_name = "TEXT")]
    plan: Option<String>,
    /// The artifact digest of the model the run graded, sha256:...
    #[arg(long, value_name = "DIGEST")]
    model_digest: String,
    /// The bar: the most wrong the plan allowed, as a share (0.01 for 1 %)
    #[arg(long, value_name = "SHARE")]
    bar: f64,
    /// How many stacks the run graded
    #[arg(long, value_name = "N")]
    n: i64,
    /// How many of those were wrong
    #[arg(long, value_name = "N")]
    errors: i64,
    /// The bound the run reported; the engine works it out again from the counts
    #[arg(long, value_name = "SHARE")]
    bound: Option<f64>,
    /// The bound's confidence
    #[arg(long, value_name = "SHARE", default_value_t = 0.95)]
    confidence: f64,
    /// The share of stacks the model accepted
    #[arg(long, value_name = "SHARE")]
    coverage: Option<f64>,
    /// The test it graded, such as T19
    #[arg(long, value_name = "NAME")]
    test: Option<String>,
    /// The claim, in words
    #[arg(long, value_name = "TEXT")]
    claim: Option<String>,
    /// The MANIFEST.json of the run that showed the artifact registered here
    /// answers as the graded one did
    #[arg(long, value_name = "FILE", requires = "equivalence_class")]
    equivalence: Option<PathBuf>,
    /// That manifest's sha256, where the file is not at hand; with --equivalence-run
    #[arg(
        long,
        value_name = "SHA256",
        conflicts_with = "equivalence",
        requires_all = ["equivalence_run", "equivalence_class"]
    )]
    equivalence_sha256: Option<String>,
    /// That run's name, where its manifest is not at hand
    #[arg(long, value_name = "RUN", requires = "equivalence_sha256")]
    equivalence_run: Option<String>,
    /// That run's class: I, II or III
    #[arg(long, value_name = "CLASS")]
    equivalence_class: Option<String>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

fn model_err(e: Error) -> Exit {
    match e {
        Error::Store(e) => fail(e.to_string()),
        other => usage(other.to_string()),
    }
}

/// A JSON document from a file, or from standard input for `-`.
fn read_json(path: &Path, what: &str) -> Result<serde_json::Value, Exit> {
    let text = if path == Path::new("-") {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| fail(format!("reading the {what} from standard input: {e}")))?;
        text
    } else {
        std::fs::read_to_string(path).map_err(|e| usage(format!("{}: {e}", path.display())))?
    };
    serde_json::from_str(&text)
        .map_err(|e| usage(format!("the {what} is not JSON ({}): {e}", path.display())))
}

/// `sha256:` and the hex of a file's bytes, read in pieces.
fn digest_of(path: &Path) -> Result<String, Exit> {
    let mut file =
        std::fs::File::open(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| fail(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(ctx.finish().as_ref())))
}

fn resolved(registry: &mut nils_registry::Registry, reference: &str) -> Result<Model, Exit> {
    model::resolve(registry.store(), reference)?.ok_or_else(|| {
        usage(format!(
            "no registered model answers to {reference}: an id, a digest (sha256:...) or name@version"
        ))
    })
}

/// The claims a file gives (record 55 ruling 31 (a)), its digest guard and
/// its why: the file is the claims in the card's own shape, or holds them
/// as `claims` beside the `model` they are for (which must be the one
/// named), its `digest` (which the engine checks) and `why`.
fn claims_of(
    registry: &mut nils_registry::Registry,
    m: &Model,
    doc: serde_json::Value,
) -> Result<(serde_json::Value, Option<String>, Option<String>), Exit> {
    if doc.get("claims").is_none() {
        return Ok((doc, None, None));
    }
    if let Some(k) = doc
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k)
        .find(|k| !matches!(k.as_str(), "claims" | "model" | "digest" | "why"))
    {
        return Err(usage(format!(
            "the claims file holds {k}: it is the claims in the card's own shape, or claims beside model, digest and why"
        )));
    }
    let text = |k: &str| -> Result<Option<String>, Exit> {
        match &doc[k] {
            serde_json::Value::Null => Ok(None),
            serde_json::Value::String(s) => Ok(Some(s.clone())),
            other => Err(usage(format!(
                "the claims file's {k} is a text, not {other}"
            ))),
        }
    };
    let (named, digest, why) = (text("model")?, text("digest")?, text("why")?);
    if let Some(named) = named {
        let other = resolved(registry, &named)?;
        if other.id != m.id {
            return Err(usage(format!(
                "the claims are for {named}, model {} ({}), not model {} ({})",
                other.id,
                other.label(),
                m.id,
                m.label()
            )));
        }
    }
    Ok((doc["claims"].clone(), digest, why))
}

fn line(m: &Model) -> String {
    format!(
        "model {}  {}  {}  {} {}  {}  {}",
        m.id,
        m.label(),
        m.kind,
        m.task,
        m.slot,
        m.state,
        m.short_digest()
    )
}

fn print_json(doc: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(doc).unwrap_or_default());
}

pub(crate) fn model_command(home: &Home, cmd: ModelCommand) -> Result<(), Exit> {
    match cmd {
        // a certificate is recorded at the engine's door, never in a
        // registry opened here
        ModelCommand::Certify(args) => certify(&args),
        ModelCommand::Registry(cmd) => registry_command(home, cmd),
    }
}

fn registry_command(home: &Home, cmd: RegistryCommand) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let who = actor();
    match cmd {
        RegistryCommand::Register {
            card,
            artifact,
            json,
        } => {
            let mut card = read_json(&card, "card")?;
            if let Some(path) = artifact {
                let digest = digest_of(&path)?;
                match card["digest"].as_str() {
                    None => card["digest"] = serde_json::Value::from(digest),
                    Some(said) if said == digest => {}
                    Some(said) => {
                        return Err(usage(format!(
                            "the card names {said} and {} is {digest}: not the artifact the card describes",
                            path.display()
                        )));
                    }
                }
            }
            let m = model::register(&mut registry, &card, &who).map_err(model_err)?;
            if json {
                print_json(&m.to_json());
            } else {
                println!("registered {}", line(&m));
            }
            Ok(())
        }
        RegistryCommand::Admit {
            model,
            check,
            cpu,
            gpu,
            set,
            json,
        } => {
            let mut check = read_json(&check, "check")?;
            let m = resolved(&mut registry, &model)?;
            if let (Some(cpu), Some(gpu)) = (cpu, gpu) {
                let measured = measure(&mut registry, &m, &cpu, &gpu, set.as_deref())?;
                if !check.is_object() {
                    return Err(usage(
                        "the check is a JSON object: suite, passed and checks",
                    ));
                }
                check["equivalence"] = measured;
            }
            let m = model::admit(&mut registry, m.id, &check, &who).map_err(model_err)?;
            let recorded = m.check.clone().unwrap_or_default();
            if json {
                print_json(&m.to_json());
            } else if m.state == "admitted" && recorded["passed"] == true {
                println!("admitted {}", line(&m));
            } else {
                println!("the check did not pass; {} stays {}", m.label(), m.state);
                for c in recorded["checks"].as_array().into_iter().flatten() {
                    if c["passed"] == false {
                        println!(
                            "  {} failed{}",
                            c["name"].as_str().unwrap_or("?"),
                            c["why"]
                                .as_str()
                                .map(|w| format!(": {w}"))
                                .unwrap_or_default()
                        );
                    }
                }
            }
            Ok(())
        }
        RegistryCommand::Equivalence {
            model,
            cpu,
            gpu,
            set,
            json,
        } => {
            let m = resolved(&mut registry, &model)?;
            let measured = measure(&mut registry, &m, &cpu, &gpu, set.as_deref())?;
            if json {
                print_json(&measured);
            } else {
                println!(
                    "{}: class {} on {} answers ({} changed between a value and not sure, {} from one value to another; probabilities at most {} apart); its card states {}",
                    m.label(),
                    measured["class"].as_str().unwrap_or("none"),
                    measured["n"],
                    measured["changes"],
                    measured["flips"],
                    measured["max_difference"],
                    m.card["equivalence"]["class"]
                        .as_str()
                        .unwrap_or("no class"),
                );
            }
            Ok(())
        }
        RegistryCommand::Choose {
            model,
            default,
            task,
            slot,
            why,
            json,
        } => {
            let done = if default {
                let task = task.unwrap_or_default();
                model::unchoose(&mut registry, &task, &slot, &who, why.as_deref())
                    .map_err(model_err)?
            } else {
                let m = resolved(&mut registry, model.as_deref().unwrap_or_default())?;
                Some(model::choose(&mut registry, m.id, &who, why.as_deref()).map_err(model_err)?)
            };
            if json {
                print_json(&serde_json::json!({
                    "chosen": done.as_ref().map(model::Chosen::as_json),
                }));
            } else {
                match &done {
                    Some(c) => println!(
                        "{} answers {} {} ({})",
                        c.model.label(),
                        c.model.task,
                        c.model.slot,
                        chosen_words(c)
                    ),
                    None => println!("no model answers now"),
                }
            }
            Ok(())
        }
        RegistryCommand::Chosen { task, json } => {
            let doc = model::tasks_json(registry.store(), task.as_deref())?;
            if json {
                print_json(&doc);
                return Ok(());
            }
            let tasks = doc["tasks"].as_array().cloned().unwrap_or_default();
            if tasks.is_empty() {
                println!("no model is registered");
            }
            for t in &tasks {
                let task = t["task"].as_str().unwrap_or("");
                let slot = t["slot"].as_str().unwrap_or("");
                match model::chosen(registry.store(), task, slot)? {
                    Some(c) => {
                        println!("{task} {slot}  {}  ({})", c.model.label(), chosen_words(&c))
                    }
                    None => println!("{task} {slot}  no model answers"),
                }
            }
            Ok(())
        }
        RegistryCommand::Promote {
            model,
            review_item,
            why,
            json,
        } => {
            let m = resolved(&mut registry, &model)?;
            let done = model::promote(&mut registry, m.id, &who, review_item, why.as_deref())
                .map_err(model_err)?;
            if json {
                print_json(&serde_json::json!({
                    "model": done.model.to_json(),
                    "retired": null,
                    "retired_models": [],
                }));
            } else {
                println!("promoted {}", line(&done.model));
            }
            Ok(())
        }
        RegistryCommand::Retire { model, why, json } => {
            let m = resolved(&mut registry, &model)?;
            let m = model::retire(&mut registry, m.id, &who, why.as_deref()).map_err(model_err)?;
            if json {
                print_json(&m.to_json());
            } else {
                println!("retired {}", line(&m));
            }
            Ok(())
        }
        RegistryCommand::AddClaims {
            model,
            claims,
            why,
            json,
        } => {
            let doc = read_json(&claims, "claims")?;
            let m = resolved(&mut registry, &model)?;
            let (claims, digest, said) = claims_of(&mut registry, &m, doc)?;
            let why = why.or(said);
            let done = model::add_claims(
                &mut registry,
                m.id,
                &claims,
                &who,
                why.as_deref(),
                digest.as_deref(),
            )
            .map_err(model_err)?;
            if json {
                let mut doc = done.model.to_json();
                doc["added"] = serde_json::json!({"fields": done.fields, "claims": done.added});
                print_json(&doc);
            } else {
                println!("added {} to {}", done.fields.join(", "), line(&done.model));
                match done.model.missing().as_slice() {
                    [] => println!("  its card is complete"),
                    lacks => println!("  its card still lacks {}", lacks.join(", ")),
                }
            }
            Ok(())
        }
        RegistryCommand::Keep {
            model,
            artifact,
            json,
        } => {
            let m = resolved(&mut registry, &model)?;
            let (d, fresh) = keep(&mut registry, &m, &artifact, &who)?;
            if json {
                let mut doc = crate::derivatives::doc(registry.store(), &d);
                doc["kept"] = serde_json::Value::from(fresh);
                print_json(&doc);
            } else if fresh {
                println!(
                    "kept {} as derivative {} at {}, {} bytes",
                    m.label(),
                    d.id,
                    d.path,
                    d.bytes
                );
            } else {
                println!(
                    "{} is kept already, as derivative {} at {}; nothing was written",
                    m.label(),
                    d.id,
                    d.path
                );
            }
            Ok(())
        }
        RegistryCommand::List {
            task,
            slot,
            state,
            json,
        } => {
            let models = model::list(
                registry.store(),
                &Filter {
                    task: task.as_deref(),
                    slot: slot.as_deref(),
                    state: state.as_deref(),
                },
            )?;
            if json {
                print_json(&serde_json::json!({
                    "count": models.len(),
                    "models": models.iter().map(Model::to_json).collect::<Vec<_>>(),
                }));
            } else if models.is_empty() {
                println!("no model is registered");
            } else {
                for m in &models {
                    println!("{}", line(m));
                }
            }
            Ok(())
        }
        RegistryCommand::Show { model, json } => {
            let m = resolved(&mut registry, &model)?;
            let events = model::events(registry.store(), m.id)?;
            let certified = model::certified(registry.store())?;
            let additions = model::additions(&events);
            if json {
                let mut doc = m.to_json();
                doc["events"] = serde_json::Value::from(events);
                doc["additions"] = serde_json::Value::from(additions);
                doc["certificate"] = model::certificate_json(&m, &certified);
                print_json(&doc);
                return Ok(());
            }
            println!("{}", line(&m));
            println!("  digest      {}", m.digest);
            if let Some(c) = certified.get(&m.id) {
                println!("  certified   {}", c.words());
            }
            if let Some(e) = m.encoder_model_id {
                println!("  encoder     model {e}");
            }
            if let Some(t) = &m.trained_on {
                println!("  trained on  {t}");
            }
            match m.missing().as_slice() {
                [] => println!("  card        complete"),
                lacks => println!("  card        lacks {}", lacks.join(", ")),
            }
            // record 55 ruling 31 (a): what the card gained since it was
            // registered
            for a in &additions {
                let fields: Vec<&str> = a["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .collect();
                println!(
                    "  added       {} on {} by {}{}",
                    fields.join(", "),
                    a["at"].as_str().unwrap_or(""),
                    a["by"].as_str().unwrap_or(""),
                    a["why"]
                        .as_str()
                        .map(|w| format!(": {w}"))
                        .unwrap_or_default()
                );
            }
            if let Some(c) = &m.check {
                println!(
                    "  check       {} {}",
                    c["suite"].as_str().unwrap_or("?"),
                    if c["passed"] == true {
                        "passed"
                    } else {
                        "failed"
                    }
                );
            }
            for e in &events {
                println!(
                    "  {}  {} by {}",
                    e["at"].as_str().unwrap_or(""),
                    e["transition"].as_str().unwrap_or(""),
                    e["by"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
    }
}

/// An evidence run's manifest: its sha256, and the run it names.
fn manifest_of(path: &Path) -> Result<(String, Option<String>), Exit> {
    let digest = digest_of(path)?;
    let doc = read_json(path, "evidence manifest")?;
    Ok((digest, doc["run"].as_str().map(str::to_string)))
}

/// `nils model certify` (record 55 ruling 22 b): a certificate that cites
/// the graded run that measured a model, posted to the engine's door under
/// the person's token, which tells the engine who records it.
fn certify(a: &CertifyArgs) -> Result<(), Exit> {
    let (manifest, run) = match (&a.evidence, &a.manifest_sha256) {
        (Some(path), _) => {
            let (digest, named) = manifest_of(path)?;
            let run = match (named, &a.run) {
                (Some(named), Some(given)) if &named != given => {
                    return Err(usage(format!(
                        "{} is the manifest of the run {named}, not {given}",
                        path.display()
                    )));
                }
                (Some(named), _) => named,
                (None, Some(given)) => given.clone(),
                (None, None) => {
                    return Err(usage(format!(
                        "{} names no run: give it with --run",
                        path.display()
                    )));
                }
            };
            (digest, run)
        }
        (None, Some(sha)) => (sha.clone(), a.run.clone().unwrap_or_default()),
        (None, None) => {
            return Err(usage(
                "--evidence MANIFEST.json, or --manifest-sha256 with --run",
            ));
        }
    };
    let mut evidence = serde_json::json!({
        "run": run, "manifest_sha256": manifest, "plan_commit": a.plan_commit,
        "model_digest": a.model_digest,
    });
    if let Some(p) = &a.plan {
        evidence["plan"] = serde_json::json!(p);
    }
    let served = match (&a.equivalence, &a.equivalence_sha256) {
        (Some(path), _) => {
            let (digest, named) = manifest_of(path)?;
            let run = named.ok_or_else(|| usage(format!("{} names no run", path.display())))?;
            Some((run, digest))
        }
        (None, Some(sha)) => Some((a.equivalence_run.clone().unwrap_or_default(), sha.clone())),
        (None, None) => None,
    };
    match (served, &a.equivalence_class) {
        (Some((run, digest)), Some(class)) => {
            evidence["equivalence"] =
                serde_json::json!({"run": run, "manifest_sha256": digest, "class": class});
        }
        (Some(_), None) => {
            return Err(usage(
                "--equivalence-class: the class the served artifact's run showed, I, II or III",
            ));
        }
        (None, Some(_)) => {
            return Err(usage(
                "--equivalence-class goes with --equivalence MANIFEST.json, or --equivalence-sha256 and --equivalence-run",
            ));
        }
        (None, None) => {}
    }
    let mut result = serde_json::json!({
        "bar": a.bar, "n": a.n, "errors": a.errors, "confidence": a.confidence,
    });
    for (k, v) in [
        ("bound", a.bound.map(serde_json::Value::from)),
        ("coverage", a.coverage.map(serde_json::Value::from)),
        ("test", a.test.clone().map(serde_json::Value::from)),
        ("claim", a.claim.clone().map(serde_json::Value::from)),
    ] {
        if let Some(v) = v {
            result[k] = v;
        }
    }
    let token = a
        .token
        .clone()
        .or_else(|| std::env::var("NILS_TOKEN").ok().filter(|t| !t.is_empty()))
        .or_else(crate::login::saved_token)
        .ok_or_else(|| {
            usage(
                "a certificate is a person's act at the engine's door: sign in with nils login, or give --token or NILS_TOKEN",
            )
        })?;
    let door = crate::door_client::Door::new(&a.server, Some(token), 30_000)?;
    let cert = door.post(
        "/api/certificates",
        &serde_json::json!({"models": [a.model], "evidence": evidence, "result": result}),
    )?;
    if a.json {
        print_json(&cert);
    } else {
        println!(
            "recorded {}",
            cert["words"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| cert.to_string())
        );
        println!(
            "  by {}; it unseals nothing",
            cert["created_by"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}

/// Why a chosen model answers, in words.
fn chosen_words(c: &model::Chosen) -> String {
    match c.by {
        "site" => format!(
            "chosen by {}{}",
            c.chosen_by.as_deref().unwrap_or("the site"),
            c.why
                .as_deref()
                .map(|w| format!(": {w}"))
                .unwrap_or_default()
        ),
        "certificate" => format!(
            "the default: certificate {} names it",
            c.certificate_id.unwrap_or_default()
        ),
        "promoted" => "the default: the one promoted last, as no certificate names one".into(),
        _ => "the default: the newest admitted, as none is certified or promoted".into(),
    }
}

/// The proposals a run said, by `run:<id>` (read again from its output
/// folder, and its units' folders) or a `results.json`; with the run's
/// device, and the models it was given.
/// What a run said, the run it was, and the models it was given.
type Said = (
    Vec<nils_registry::proposals::Proposal>,
    serde_json::Value,
    Option<Vec<i64>>,
);

fn answers_of(registry: &mut nils_registry::Registry, source: &str) -> Result<Said, Exit> {
    let read = |path: &Path| -> Result<Vec<nils_registry::proposals::Proposal>, Exit> {
        let text =
            std::fs::read_to_string(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        let doc: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| usage(format!("{} is not JSON: {e}", path.display())))?;
        nils_registry::proposals::parse(&doc).map_err(|e| usage(e.to_string()))
    };
    let Some(id) = source.strip_prefix("run:") else {
        let path = Path::new(source);
        return Ok((
            read(path)?,
            serde_json::json!({ "file": path.file_name().map(|n| n.to_string_lossy().to_string()) }),
            None,
        ));
    };
    let id: i64 = id
        .parse()
        .map_err(|_| usage(format!("{source}: a run is run:<id>")))?;
    let store = registry.store();
    let d = store.dialect();
    let t = nils_registry::schema::table("pipeline_run");
    let sql = format!(
        "SELECT r.device, {}, r.output, p.path, r.status FROM {} r LEFT JOIN {} p ON p.id = r.place_id WHERE r.id = {}",
        d.text_of(t.column("model_ids").expect("model_ids")),
        store.qualified("pipeline_run"),
        store.qualified("place"),
        d.param(1, nils_registry::schema::Type::Int),
    );
    let row = store
        .query_opt(&sql, &[nils_registry::store::Param::Int(id)])?
        .ok_or_else(|| usage(format!("no run {id}")))?;
    let device = row.text(0)?.to_string();
    let models: Vec<i64> = row
        .opt_text(1)?
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default();
    let (Some(output), Some(place)) = (row.opt_text(2)?, row.opt_text(3)?) else {
        return Err(usage(format!("run {id} names no output folder")));
    };
    let status = row.text(4)?.to_string();
    let out = Path::new(place).join(output);
    let mut found = Vec::new();
    let mut folders = vec![out.clone()];
    if let Ok(entries) = std::fs::read_dir(&out) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|f| f.is_dir()) {
                folders.push(e.path());
            }
        }
    }
    folders.sort();
    let mut any = false;
    for f in folders {
        let results = f.join("results.json");
        if std::fs::symlink_metadata(&results).is_ok_and(|m| m.is_file()) {
            any = true;
            found.extend(read(&results)?);
        }
    }
    if !any {
        return Err(usage(format!(
            "run {id} ({status}) left no results.json in its output folder"
        )));
    }
    Ok((
        found,
        serde_json::json!({ "run": id, "device": device, "status": status }),
        Some(models),
    ))
}

/// Measure a model's CPU and GPU equivalence from two runs (record 55 A5):
/// both runs must have been given the model where they are runs of the
/// engine, the first on a CPU and the second on a GPU.
fn measure(
    registry: &mut nils_registry::Registry,
    m: &Model,
    cpu: &str,
    gpu: &str,
    set: Option<&str>,
) -> Result<serde_json::Value, Exit> {
    let (a, a_from, a_models) = answers_of(registry, cpu)?;
    let (b, b_from, b_models) = answers_of(registry, gpu)?;
    for (side, models, from, want_gpu) in [
        ("--cpu", &a_models, &a_from, false),
        ("--gpu", &b_models, &b_from, true),
    ] {
        if let Some(models) = models
            && !models.contains(&m.id)
        {
            return Err(usage(format!(
                "{side}: run {} was not given {} (model {})",
                from["run"],
                m.label(),
                m.id
            )));
        }
        if let Some(device) = from["device"].as_str() {
            let on_gpu = device != "cpu";
            if on_gpu != want_gpu {
                return Err(usage(format!(
                    "{side}: run {} ran on {device}; the CPU run is given as --cpu and the GPU run as --gpu",
                    from["run"]
                )));
            }
        }
    }
    let mut measured = model::equivalence(registry.store(), m, &a, &b).map_err(model_err)?;
    measured["set"] = serde_json::json!(set);
    measured["cpu"] = a_from;
    measured["gpu"] = b_from;
    measured["digest"] = serde_json::json!(m.digest);
    Ok(measured)
}

/// A name as one path component a runtime's mount syntax takes: letters,
/// digits, `.`, `-` and `_` kept, anything else (a `:` or a `,` above all,
/// which the mounts split on) a `_`, and never a hidden or empty name.
fn component(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean.starts_with('.') {
        format!("artifact{clean}")
    } else {
        clean
    }
}

/// Copy `from` to `to` through a temporary file beside it, hashing the
/// bytes on the way; the copy is renamed into place only when its bytes
/// hash to `hex`. Answers how many bytes it wrote.
fn copy_checked(from: &Path, to: &Path, hex: &str) -> Result<u64, Exit> {
    use std::io::Write;
    let dir = to
        .parent()
        .ok_or_else(|| fail(format!("{} has no folder", to.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let mut nonce = [0u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
        .map_err(|_| fail("no randomness for a temporary name"))?;
    let temp = dir.join(format!(".keep-{}", hex::encode(nonce)));
    let result = (|| -> Result<u64, Exit> {
        let mut src =
            std::fs::File::open(from).map_err(|e| usage(format!("{}: {e}", from.display())))?;
        let mut out =
            std::fs::File::create(&temp).map_err(|e| fail(format!("{}: {e}", temp.display())))?;
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buf = vec![0u8; 1 << 20];
        let mut total: u64 = 0;
        loop {
            let n = src
                .read(&mut buf)
                .map_err(|e| fail(format!("{}: {e}", from.display())))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            ctx.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| fail(format!("{}: {e}", temp.display())))?;
        }
        out.sync_all()
            .map_err(|e| fail(format!("{}: {e}", temp.display())))?;
        let got = hex::encode(ctx.finish().as_ref());
        if got != hex {
            return Err(fail(format!(
                "the copy of {} hashes to sha256:{got}, not sha256:{hex}; the file changed while it was copied, and nothing was kept",
                from.display()
            )));
        }
        Ok(total)
    })();
    match result {
        Ok(total) => {
            std::fs::rename(&temp, to).map_err(|e| {
                let _ = std::fs::remove_file(&temp);
                fail(format!("{}: {e}", to.display()))
            })?;
            Ok(total)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// `nils model keep` (record 50): copy a registered model's artifact into
/// the working place a run reads models from, the lane's output place, and
/// register it as a derivative of kind model and scope model, so a run
/// given the model mounts it. The file must hash to the model's digest; a
/// retired model is refused. The same bytes kept again write nothing.
/// Answers the derivative and whether this call wrote it.
fn keep(
    registry: &mut nils_registry::Registry,
    m: &Model,
    artifact: &Path,
    who: &str,
) -> Result<(nils_registry::derivative::Derivative, bool), Exit> {
    use nils_registry::derivative;
    if m.state == "retired" {
        return Err(usage(format!(
            "model {} is retired; a run is not given a retired model, so its artifact is not kept",
            m.label()
        )));
    }
    let digest = digest_of(artifact)?;
    if digest != m.digest {
        return Err(usage(format!(
            "{} is {digest} and model {} is {}: not the model's artifact",
            artifact.display(),
            m.label(),
            m.digest
        )));
    }
    let hex = digest.trim_start_matches("sha256:").to_string();
    let place = crate::pipelines::run_places(registry.store())
        .map_err(usage)?
        .output;
    let file = artifact
        .file_name()
        .map(|n| component(&n.to_string_lossy()))
        .ok_or_else(|| usage(format!("{} names no file", artifact.display())))?;
    let rel = format!(
        "{}/models/{}/{file}",
        derivative::TREE,
        component(&format!("{}-{}", m.name, m.version))
    );
    let target = Path::new(&place.path).join(&rel);
    // kept already: a live row of this model in this place, its file there
    let prior = derivative::of_model(registry.store(), m.id, place.id)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .find(|d| d.scope == "model" && d.sha256 == hex);
    if let Some(d) = prior {
        let there = Path::new(&place.path).join(&d.path);
        let whole = there.is_file() && digest_of(&there).ok().as_deref() == Some(digest.as_str());
        if whole {
            return Ok((d, false));
        }
        // the row stands and its file went: the copy is made again where
        // the row says, and no second row is written
        copy_checked(artifact, &there, &hex)?;
        return Ok((d, true));
    }
    let bytes = if target.is_file() && digest_of(&target)? == digest {
        std::fs::metadata(&target)
            .map_err(|e| fail(format!("{}: {e}", target.display())))?
            .len()
    } else {
        copy_checked(artifact, &target, &hex)?
    };
    let media_type = match Path::new(&file).extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    let now = nils_registry::time::now_iso();
    let belongs = derivative::Belongs::model();
    let id = derivative::insert(
        registry.store(),
        &derivative::New {
            kind: "model",
            belongs: &belongs,
            place_id: place.id,
            path: &rel,
            bytes: bytes as i64,
            sha256: &hex,
            media_type,
            registered_by: who,
            actor: None,
            model_id: Some(m.id),
            run_id: None,
            preprocess_version: None,
            supersedes_id: None,
            created_at: &now,
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: who,
            action: nils_registry::audit::Action::DerivativeRegister,
            scope: serde_json::json!({
                "derivative": id, "kind": "model", "scope": "model",
                "place": place.name, "model": m.id,
            }),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({"bytes": bytes, "sha256": hex, "kept": true})),
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    let d = derivative::get(registry.store(), id)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail("the derivative was not written back"))?;
    Ok((d, true))
}
