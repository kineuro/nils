// SPDX-License-Identifier: AGPL-3.0-only

//! `nils model` (record 42 S2, D15): the model registry at the keyboard.
//! `register` takes a card (`contracts/model/v1/card.schema.json`) and, with
//! `--artifact`, checks the card's digest against the file or fills it in;
//! `admit` records a check; `promote` puts a model in its task and slot and
//! retires the one before it; `retire`; `list` and `show`. Each writes the
//! same rows and audit rows as the doors under `/api/models`. `keep`
//! (record 50) copies a registered model's artifact into the working place
//! a run reads models from, so a pipeline's model input finds it.

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
        ModelCommand::Keep {
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
