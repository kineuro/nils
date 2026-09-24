// SPDX-License-Identifier: AGPL-3.0-only

//! `nils model` (record 42 S2, D15): the model registry at the keyboard.
//! `register` takes a card (`contracts/model/v1/card.schema.json`) and, with
//! `--artifact`, checks the card's digest against the file or fills it in;
//! `admit` records a check; `promote` puts a model in its task and slot and
//! retires the one before it; `retire`; `list` and `show`. Each writes the
//! same rows and audit rows as the doors under `/api/models`.

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use nils_registry::home::Home;
use nils_registry::model::{self, Error, Filter, Model};

use crate::{Exit, actor, fail, open, usage};

#[derive(Debug, Subcommand)]
pub(crate) enum ModelCommand {
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
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Promote an admitted model in its task and slot; the model promoted
    /// there before is retired
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
    /// One model: its card, its state, the check that admitted it and every
    /// transition
    Show {
        /// The model: its id, its digest (sha256:...) or name@version
        model: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
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
    let mut registry = open(home)?;
    let who = actor();
    match cmd {
        ModelCommand::Register {
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
        ModelCommand::Admit { model, check, json } => {
            let check = read_json(&check, "check")?;
            let m = resolved(&mut registry, &model)?;
            let m = model::admit(&mut registry, m.id, &check, &who).map_err(model_err)?;
            if json {
                print_json(&m.to_json());
            } else if m.state == "admitted" && check["passed"] == true {
                println!("admitted {}", line(&m));
            } else {
                println!("the check did not pass; {} stays {}", m.label(), m.state);
            }
            Ok(())
        }
        ModelCommand::Promote {
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
                    "retired": done.retired.as_ref().map(Model::to_json),
                }));
            } else {
                println!("promoted {}", line(&done.model));
                if let Some(old) = &done.retired {
                    println!("retired {}", line(old));
                }
            }
            Ok(())
        }
        ModelCommand::Retire { model, why, json } => {
            let m = resolved(&mut registry, &model)?;
            let m = model::retire(&mut registry, m.id, &who, why.as_deref()).map_err(model_err)?;
            if json {
                print_json(&m.to_json());
            } else {
                println!("retired {}", line(&m));
            }
            Ok(())
        }
        ModelCommand::List {
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
        ModelCommand::Show { model, json } => {
            let m = resolved(&mut registry, &model)?;
            let events = model::events(registry.store(), m.id)?;
            if json {
                let mut doc = m.to_json();
                doc["events"] = serde_json::Value::from(events);
                print_json(&doc);
                return Ok(());
            }
            println!("{}", line(&m));
            println!("  digest      {}", m.digest);
            if let Some(e) = m.encoder_model_id {
                println!("  encoder     model {e}");
            }
            if let Some(t) = &m.trained_on {
                println!("  trained on  {t}");
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
