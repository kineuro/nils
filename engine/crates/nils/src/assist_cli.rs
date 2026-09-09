// SPDX-License-Identifier: AGPL-3.0-only

//! `nils assist ask "words" [--base ID] --assistant URL` (Wave 4c §9.11,
//! §9.15): the third surface of the ask-help station. The words go to the
//! assistant's headless run door, the verdict comes back, and with
//! `--server` the engine prints the document's describe sentence, its diff
//! against the base and the handle. The engine never parses a sentence
//! itself; the assistant does, and this verb only carries.

use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::door_client::Door;
use crate::{Exit, fail, usage};

#[derive(Debug, Subcommand)]
pub(crate) enum AssistCommand {
    /// Words to a document through the ask-help station, or one step of a document tuned
    Ask(AssistAskArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AssistAskArgs {
    /// The words, as a person would say them
    words: String,
    /// A stored document the words refine, by handle
    #[arg(long, value_name = "ID")]
    base: Option<i64>,
    /// The assistant's URL, as the desk reaches it
    #[arg(long, value_name = "URL")]
    assistant: String,
    /// The person's token: the assistant carries it to the engine (NILS_TOKEN when absent)
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// The engine, for the describe sentence and the diff; the assistant's answer alone when absent
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// Seconds to wait for the run to settle
    #[arg(long, default_value_t = 240)]
    wait: u64,
    /// Print the verdict as JSON, which is what the assistant answered
    #[arg(long)]
    json: bool,
}

pub(crate) fn assist_command(command: AssistCommand) -> Result<(), Exit> {
    match command {
        AssistCommand::Ask(args) => ask(args),
    }
}

fn token_of(args: &AssistAskArgs) -> Option<String> {
    args.token
        .clone()
        .or_else(|| std::env::var("NILS_TOKEN").ok().filter(|t| !t.is_empty()))
        .or_else(crate::login::saved_token)
}

fn ask(args: AssistAskArgs) -> Result<(), Exit> {
    let token = token_of(&args);
    let assistant = Door::new(&args.assistant, token.clone(), 30_000)?;
    let message = match args.base {
        Some(base) => format!(
            "{} (the base document is {base}; refine it rather than start over)",
            args.words
        ),
        None => args.words.clone(),
    };
    let started = assistant.post("/stations/ask-help/runs", &json!({"message": message}))?;
    let run = started["run"]
        .as_str()
        .ok_or_else(|| fail(format!("the assistant did not start a run: {started}")))?
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(args.wait);
    let mut state = started;
    while !matches!(
        state["state"].as_str(),
        Some("settled" | "failed" | "aborted")
    ) {
        if Instant::now() > deadline {
            return Err(fail(format!(
                "run {run} did not settle within {} s; it goes on at the assistant, GET {}/runs/{run}",
                args.wait, args.assistant
            )));
        }
        std::thread::sleep(Duration::from_secs(3));
        state = assistant.get(&format!("/runs/{run}"))?;
    }
    if state["state"] != "settled" {
        return Err(fail(format!(
            "run {run} {}: {}",
            state["state"].as_str().unwrap_or("ended"),
            state["error"].as_str().unwrap_or("no reason given")
        )));
    }
    let verdict = match assistant.get(&format!("/runs/{run}/verdict")) {
        Ok(v) => v,
        Err(_) => {
            // the run ended without settling: the reply names the terminal reason
            let terminal = state["reply"]["metadata"]["terminal"]
                .as_str()
                .unwrap_or("no reason recorded")
                .to_string();
            let text = state["reply"]["text"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            return Err(fail(format!(
                "run {run} ended without a verdict: {terminal}{}",
                if text.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; the station's last words: {}",
                        text.chars().take(240).collect::<String>()
                    )
                }
            )));
        }
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&verdict).unwrap_or_default()
        );
        return Ok(());
    }
    let result = &verdict["result"];
    let document = result["document"]
        .as_i64()
        .ok_or_else(|| fail(format!("the verdict names no document: {verdict}")))?;
    println!("{}", result["sentence"].as_str().unwrap_or(""));
    if let Some(url) = &args.server {
        let engine = Door::new(url, token, 30_000)?;
        let described = engine.post("/api/ask/describe", &json!({"document_id": document}))?;
        for set in described["sets"].as_array().into_iter().flatten() {
            if let Some(sentence) = set.get(1).and_then(Value::as_str) {
                println!("  {sentence}");
            }
        }
        if let Some(base) = args.base {
            let diff = engine.post(
                "/api/ask/diff",
                &json!({"a": {"document_id": base}, "b": {"document_id": document}}),
            )?;
            if diff["same"] == true {
                println!("no change against document {base}");
            } else {
                for c in diff["changes"].as_array().into_iter().flatten() {
                    println!(
                        "  {} {} {}: {} -> {}",
                        c["set"].as_str().unwrap_or(""),
                        c["part"].as_str().unwrap_or(""),
                        c["kind"].as_str().unwrap_or(""),
                        c["before"],
                        c["after"]
                    );
                }
            }
        }
    }
    println!(
        "document {document}   hash {}   station {}   {}",
        result["hash"].as_str().unwrap_or("?"),
        verdict["station"].as_str().unwrap_or("ask-help"),
        verdict["terminal"].as_str().unwrap_or("settled")
    );
    for choice in result["choices"].as_array().into_iter().flatten() {
        println!("choice: {choice}");
    }
    if verdict["result"].is_null() {
        return Err(usage("the verdict carried no result"));
    }
    Ok(())
}
