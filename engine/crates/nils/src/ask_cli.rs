// SPDX-License-Identifier: AGPL-3.0-only

//! `nils ask` (Wave 4b §12.3): the non-interactive runner in the same
//! binary. `run`, `explain`, `validate`, `options`, `diagnose`,
//! `describe`, `handles` and `selections` each build one JSON document and
//! print it through one renderer, so the same call reads alike whether the
//! crate answered in process (a standalone registry) or a door answered
//! over HTTP (`--server`), and the same document produces the same content
//! hash either way. `promote` and `time` are the job path's and the caps
//! measurement's. `gate` is slice 12's. A listing and a prune stay on the
//! node: they read the registry, not a door.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Subcommand};
use nils_ask::affordance::{self, Setting};
use nils_ask::ast::Ask;
use nils_ask::exec::Bounds;
use nils_ask::handle;
use nils_ask::run::{self, Request};
use nils_ask::selection;
use nils_ask::validate::{Class, Scope};
use nils_ask::{diagnose, document, parse, promote};
use nils_catalog::{Caps, Catalog};
use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::job::{self, Claim, State};
use nils_registry::session::Scheme;
use serde_json::json;

use crate::door_client::Door;
use crate::{Exit, actor, fail, open, usage};

#[derive(Debug, Subcommand)]
pub(crate) enum AskCommand {
    /// Run a document to a handle: a stored document by handle, or a file
    Run(AskRunArgs),
    /// Validate a document strictly, or repair it first, and print its hash
    Validate(AskValidateArgs),
    /// Authored text with add only repair: the repairs, the diagnosis and,
    /// when it validates, the stored document (Wave 4c section 6.4)
    Draft(AskDraftArgs),
    /// Print the SQL a document compiles to, on either dialect or both
    Explain(AskExplainArgs),
    /// The typed moves a set offers, with their templates and fillers
    Options(AskSetArgs),
    /// The funnel, the drops, the ties and the cost of a document
    Diagnose(AskDiagnoseArgs),
    /// One sentence per set, the conventions, the denominators, the mechanisms
    Describe(AskDocArgs),
    /// Result handles: list, show, export to CSV, prune
    Handles {
        #[command(subcommand)]
        command: HandlesCommand,
    },
    /// Saved asks: save, list, show
    Selections {
        #[command(subcommand)]
        command: SelectionsCommand,
    },
    /// Promote a subject grain handle into a cohort (Wave 4b section 8.3)
    Promote(AskPromoteArgs),
    /// Time the fixtures on this registry, for the caps of section 11.5
    Time(AskTimeArgs),
    /// The gate (Wave 4b section 13): every fixture of the repository
    /// against its canonical, on this registry's backend
    Gate(AskGateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AskDraftArgs {
    /// The text to draft from, YAML or JSON; `-` reads standard input
    #[arg(long, value_name = "FILE")]
    file: PathBuf,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct AskGateArgs {
    /// The pack directory
    #[arg(long, value_name = "DIR")]
    pack_dir: PathBuf,
    /// The pack, by name in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    /// The gate's own directory; the repository's when absent
    #[arg(long, value_name = "DIR")]
    gate: Option<PathBuf>,
    /// Take the canonicals from this run instead of checking against them
    #[arg(long)]
    write: bool,
    #[arg(long)]
    json: bool,
    /// The DSN of the ask doors' SELECT only role on Postgres, for the
    /// write refusal fixture (Wave 4c section 6.8); deferred when absent
    #[arg(long, value_name = "DSN")]
    ask_dsn: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HandlesCommand {
    /// Every handle this registry keeps, newest first (the node's own)
    List {
        #[arg(long)]
        withdrawn: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// One handle with its provenance and what pins it
    Show(HandleArgs),
    /// A handle's rows as CSV, from its pages, with no database driver
    Export(ExportArgs),
    /// Drop the rows of every handle unread for the retention (the node's own)
    Prune {
        #[arg(long, default_value_t = nils_ask::handle::KEEP_DAYS)]
        keep_days: i64,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SelectionsCommand {
    /// Save a document as the next version of a selection
    Save(SelectionSaveArgs),
    /// Every selection, by name (the node's own)
    List {
        #[arg(long)]
        json: bool,
    },
    /// One selection: its current version, or `name@version`
    Show(SelectionShowArgs),
}

/// Where a call is answered: this registry, or a running engine.
#[derive(Debug, Args)]
pub(crate) struct Where {
    /// Ask a running engine instead of this registry, as http://host:port
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// The bearer token the engine knows; NILS_TOKEN when absent
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// The pack directory, when this registry answers
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// The pack, by name in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Print the document as JSON, which is what the door answered
    #[arg(long)]
    json: bool,
}

/// A document to act on: a file, or one the engine already holds.
#[derive(Debug, Args)]
pub(crate) struct Which {
    /// A document file, JSON or YAML
    #[arg(long, value_name = "FILE", conflicts_with = "document")]
    file: Option<PathBuf>,
    /// A document handle
    #[arg(long, value_name = "ID")]
    document: Option<i64>,
}

#[derive(Debug, Args)]
pub(crate) struct AskDocArgs {
    #[command(flatten)]
    which: Which,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct AskValidateArgs {
    #[command(flatten)]
    which: Which,
    /// Repair what is structural first (a missing {}, a lone clause, an
    /// operator alias), then validate
    #[arg(long)]
    repair: bool,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct AskExplainArgs {
    #[command(flatten)]
    which: Which,
    /// Which text to print
    #[arg(long, default_value = "both", value_name = "sqlite|postgres|both")]
    dialect: String,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct AskSetArgs {
    #[command(flatten)]
    which: Which,
    /// The set to offer moves on; the answer's set when absent
    #[arg(long)]
    set: Option<String>,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct AskDiagnoseArgs {
    #[command(flatten)]
    which: Which,
    /// Carry the surviving subject keys through the funnel
    #[arg(long)]
    keys: bool,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct HandleArgs {
    #[arg(long, value_name = "ID")]
    handle: i64,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    #[arg(long, value_name = "ID")]
    handle: i64,
    /// Where the CSV goes; standard output when absent
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct SelectionSaveArgs {
    /// The selection's name
    #[arg(long)]
    name: String,
    #[command(flatten)]
    which: Which,
    /// A note on this version
    #[arg(long)]
    note: Option<String>,
    /// What the selection is for, on its first version
    #[arg(long)]
    description: Option<String>,
    #[command(flatten)]
    at: Where,
}

#[derive(Debug, Args)]
pub(crate) struct SelectionShowArgs {
    /// The selection, as `name` or `name@version`
    #[arg(value_name = "NAME[@VERSION]")]
    name: String,
    #[command(flatten)]
    at: Where,
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
    /// The pack directory, when this registry answers
    #[arg(long, value_name = "DIR", required_unless_present = "server")]
    pack_dir: Option<PathBuf>,
    /// The pack, by name in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    /// Run on a running engine instead of this registry, as http://host:port
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// The bearer token the engine knows; NILS_TOKEN when absent
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
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

pub(crate) fn load_pack(dir: &std::path::Path, name: &str) -> Result<nils_pack::Pack, Exit> {
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
        AskCommand::Validate(args) => validate(home, args),
        AskCommand::Draft(args) => draft(home, args),
        AskCommand::Explain(args) => explain(home, args),
        AskCommand::Options(args) => options(home, args),
        AskCommand::Diagnose(args) => diagnose_cmd(home, args),
        AskCommand::Describe(args) => describe(home, args),
        AskCommand::Handles { command } => handles(home, command),
        AskCommand::Selections { command } => selections(home, command),
        AskCommand::Promote(args) => ask_promote(home, args),
        AskCommand::Time(args) => ask_time(home, args),
        AskCommand::Gate(args) => crate::gate::gate(
            home,
            args.pack_dir,
            &args.pack,
            args.gate,
            args.write,
            args.json,
            args.ask_dsn,
        ),
    }
}

// ------------------------------------------------------- where a call lands

impl Where {
    fn door(&self) -> Result<Option<Door>, Exit> {
        let Some(url) = &self.server else {
            return Ok(None);
        };
        let token = self
            .token
            .clone()
            .or_else(|| std::env::var("NILS_TOKEN").ok().filter(|t| !t.is_empty()))
            .or_else(crate::login::saved_token);
        Door::new(url, token, Caps::default().sync_timeout_ms + 5_000).map(Some)
    }
}

/// This registry, its catalog and its pack: what answers when no door does.
struct Local {
    registry: Registry,
    catalog: Catalog,
}

fn local(home: &Home, at: &Where) -> Result<Local, Exit> {
    let dir = crate::pack_dir(home, at.pack_dir.clone()).map_err(|_| {
        usage("no packs here: pass --pack-dir DIR, or --server URL to ask a running engine")
    })?;
    let pack = load_pack(&dir, &at.pack)?;
    let mut registry = open(home)?;
    let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
    Ok(Local { registry, catalog })
}

impl Local {
    fn scheme(&mut self, ask: &Ask) -> Result<Scheme, Exit> {
        match &ask.scheme {
            Some(nils_ask::ast::SchemeRef::Name(n)) if n != "default" && n != "day" => {
                crate::stored_scheme(&mut self.registry, n)
            }
            Some(nils_ask::ast::SchemeRef::Inline(m)) => {
                Scheme::from_json(&serde_json::to_string(m).unwrap_or_default())
                    .map_err(|e| usage(format!("the inline scheme: {e}")))
            }
            _ => Ok(Scheme::default()),
        }
    }

    fn setting<'a>(&'a self, scheme: &'a Scheme, scope: &'a Scope) -> Setting<'a> {
        Setting {
            names: &self.catalog,
            scope,
            scheme,
            principal: "",
            bounds: bounds(),
            values_cap: Caps::default().options_values as usize,
        }
    }
}

/// The document a call names: a file, or one the engine holds.
enum Document {
    /// The document a file holds; boxed, because an ask is far larger than
    /// a handle and both travel in one value.
    File(Box<Ask>),
    Stored(i64),
}

impl Which {
    fn read(&self) -> Result<Document, Exit> {
        match (&self.file, self.document) {
            (Some(path), _) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| usage(format!("{}: {e}", path.display())))?;
                Ok(Document::File(Box::new(
                    parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?,
                )))
            }
            (None, Some(id)) => Ok(Document::Stored(id)),
            (None, None) => Err(usage("--file FILE or --document ID")),
        }
    }

    /// The body a door takes: the document itself, or its handle.
    fn body(&self) -> Result<serde_json::Value, Exit> {
        Ok(match self.read()? {
            Document::File(ask) => json!({"document": ask}),
            Document::Stored(id) => json!({"document_id": id}),
        })
    }
}

/// The document itself, fetching a stored one when the registry holds it.
fn ask_of(local: &mut Local, which: &Which) -> Result<Ask, Exit> {
    match which.read()? {
        Document::File(ask) => Ok(*ask),
        Document::Stored(id) => Ok(document::get(local.registry.store(), id)
            .map_err(|e| fail(e.to_string()))?
            .ok_or_else(|| usage(format!("no document {id}")))?
            .ask),
    }
}

fn print_json(doc: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(doc).unwrap_or_default());
}

fn text_of(v: &serde_json::Value) -> String {
    v.as_str()
        .map(String::from)
        .unwrap_or_else(|| v.to_string())
}

// ------------------------------------------------------------------ validate

fn validate(home: &Home, args: AskValidateArgs) -> Result<(), Exit> {
    let doc = match args.at.door()? {
        Some(door) => {
            let mut body = args.which.body()?;
            if args.repair {
                body["mode"] = json!("repair");
            }
            door.post("/api/ask/validate", &body)?
        }
        None => {
            let mut l = local(home, &args.at)?;
            let scope = scope();
            let (ask, repairs) = match (&args.which.file, args.repair) {
                (Some(path), true) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| usage(format!("{}: {e}", path.display())))?;
                    nils_ask::parse_repaired(&text)
                        .map_err(|e| usage(format!("{}: {e}", path.display())))?
                }
                _ => (ask_of(&mut l, &args.which)?, Vec::new()),
            };
            let prepared = nils_ask::prepare(ask, &l.catalog, &scope).map_err(refused)?;
            json!({
                "hash": prepared.hash,
                "warnings": prepared.validated.warnings,
                "pinned": prepared.pinned.iter().map(|(s, n, v)| json!({"set": s, "selection": n, "version": v})).collect::<Vec<_>>(),
                "repairs": repairs.iter().map(|r| json!({"path": r.path, "what": r.what})).collect::<Vec<_>>(),
                "order": prepared.validated.order,
            })
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    println!("hash {}", text_of(&doc["hash"]));
    for r in doc["repairs"].as_array().into_iter().flatten() {
        println!("repaired {}: {}", text_of(&r["path"]), text_of(&r["what"]));
    }
    for p in doc["pinned"].as_array().into_iter().flatten() {
        println!(
            "pinned   {} reads {} at version {}",
            text_of(&p["set"]),
            text_of(&p["selection"]),
            p["version"]
        );
    }
    for w in doc["warnings"].as_array().into_iter().flatten() {
        println!(
            "warning  {} at {}: {} ({})",
            text_of(&w["code"]),
            text_of(&w["path"]),
            text_of(&w["message"]),
            text_of(&w["next"])
        );
    }
    Ok(())
}

// ------------------------------------------------------------------- explain

fn explain(home: &Home, args: AskExplainArgs) -> Result<(), Exit> {
    if !matches!(args.dialect.as_str(), "sqlite" | "postgres" | "both") {
        return Err(usage(format!(
            "--dialect is sqlite, postgres or both, not {}",
            args.dialect
        )));
    }
    let doc = match args.at.door()? {
        Some(door) => door.post("/api/ask/explain", &args.which.body()?)?,
        None => {
            let mut l = local(home, &args.at)?;
            let ask = ask_of(&mut l, &args.which)?;
            let scheme = l.scheme(&ask)?;
            let scope = scope();
            let ex = run::explain(&mut l.registry, ask, &l.catalog, &scope, &scheme)
                .map_err(|e| fail(e.to_string()))?;
            serde_json::to_value(ex).unwrap_or_default()
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    println!("hash {}", text_of(&doc["hash"]));
    println!(
        "{} parameters, columns {}",
        doc["params"],
        doc["columns"]
            .as_array()
            .map(|a| a.iter().map(text_of).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    );
    for i in doc["inlined"].as_array().into_iter().flatten() {
        println!(
            "inlined {} from {} at version {}",
            text_of(&i["set"]),
            text_of(&i["name"]),
            i["version"]
        );
    }
    for dialect in ["sqlite", "postgres"] {
        if args.dialect == "both" || args.dialect == dialect {
            println!("\n-- {dialect}\n{}", text_of(&doc[dialect]));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- options

fn options(home: &Home, args: AskSetArgs) -> Result<(), Exit> {
    let doc = match args.at.door()? {
        Some(door) => {
            let mut body = args.which.body()?;
            if let Some(set) = &args.set {
                body["set"] = json!(set);
            }
            door.post("/api/ask/options", &body)?
        }
        None => {
            let mut l = local(home, &args.at)?;
            let ask = ask_of(&mut l, &args.which)?;
            let scheme = l.scheme(&ask)?;
            let epoch = l.registry.meta().epoch;
            let scope = scope();
            let setting = l.setting(&scheme, &scope);
            let opts = affordance::options(epoch, &ask, args.set.as_deref(), &setting)
                .map_err(|e| usage(e.to_string()))?;
            serde_json::to_value(opts).unwrap_or_default()
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    println!(
        "{} ({}s)   epoch {}   token {}",
        text_of(&doc["set"]),
        text_of(&doc["grain"]),
        doc["epoch"],
        text_of(&doc["token"])
    );
    println!("{}", text_of(&doc["describe"]));
    for d in doc["diagnostics"].as_array().into_iter().flatten() {
        println!(
            "warning {} at {}: {}",
            text_of(&d["code"]),
            text_of(&d["path"]),
            text_of(&d["message"])
        );
    }
    println!("\nmoves");
    for m in doc["moves"].as_array().into_iter().flatten() {
        println!("{:>4}  {}", m["id"], text_of(&m["template"]));
        for h in m["holes"].as_array().into_iter().flatten() {
            let fillers: Vec<String> = h["fillers"]
                .as_array()
                .map(|a| a.iter().take(8).map(text_of).collect())
                .unwrap_or_default();
            let more = h["fillers"].as_array().map(|a| a.len()).unwrap_or(0);
            println!(
                "        {} ({}{}){}",
                text_of(&h["name"]),
                text_of(&h["type"]),
                if h["optional"].as_bool().unwrap_or(false) {
                    ", optional"
                } else {
                    ""
                },
                if fillers.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}{}",
                        fillers.join(", "),
                        if more > fillers.len() {
                            format!(" and {} more", more - fillers.len())
                        } else {
                            String::new()
                        }
                    )
                }
            );
        }
    }
    for n in doc["next"].as_array().into_iter().flatten() {
        println!("next    {}", text_of(n));
    }
    Ok(())
}

// ------------------------------------------------------------------ diagnose

fn diagnose_cmd(home: &Home, args: AskDiagnoseArgs) -> Result<(), Exit> {
    let doc = match args.at.door()? {
        Some(door) => {
            let mut body = args.which.body()?;
            body["keys"] = json!(args.keys);
            door.post("/api/ask/diagnose", &body)?
        }
        None => {
            let mut l = local(home, &args.at)?;
            let ask = ask_of(&mut l, &args.which)?;
            let scheme = l.scheme(&ask)?;
            let scope = scope();
            let d = diagnose::diagnose(
                &mut l.registry,
                ask,
                Vec::new(),
                &l.catalog,
                &scope,
                &scheme,
                bounds(),
                args.keys,
                None,
            )
            .map_err(|e| fail(e.to_string()))?;
            serde_json::to_value(d).unwrap_or_default()
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    if !doc["valid"].as_bool().unwrap_or(false) {
        for i in doc["issues"].as_array().into_iter().flatten() {
            println!(
                "{} at {}: {}\n  {}",
                text_of(&i["code"]),
                text_of(&i["path"]),
                text_of(&i["message"]),
                text_of(&i["next"])
            );
        }
        return Err(Exit {
            code: crate::USAGE,
            message: String::new(),
        });
    }
    let cost = &doc["cost"];
    println!(
        "cost {}   {} sets, {} near, {} groups",
        text_of(&cost["class"]),
        cost["sets"],
        cost["near"],
        cost["groups"]
    );
    if let Some(why) = doc["zero_rows"].as_str() {
        println!("no rows: {why}");
    }
    println!("\nfunnel");
    for s in doc["funnel"].as_array().into_iter().flatten() {
        println!(
            "{:>28}  {:>6} rows  {:>6} subjects   {}",
            format!("{}/{}", text_of(&s["set"]), text_of(&s["stage"])),
            s["rows"],
            s["subjects"],
            if s["on_path"].as_bool().unwrap_or(false) {
                ""
            } else {
                "(a helper set)"
            }
        );
    }
    for d in doc["drops"].as_array().into_iter().flatten() {
        println!(
            "drop   {}: {} to {} rows, {} unknown",
            text_of(&d["clause"]),
            d["before"],
            d["after"],
            d["nulls"]
        );
    }
    for t in doc["ties"].as_array().into_iter().flatten() {
        println!("ties   {} rows in {}", t[1], text_of(&t[0]));
    }
    for c in doc["coarse"].as_array().into_iter().flatten() {
        println!("coarse {} rows in {}", c[1], text_of(&c[0]));
    }
    for w in doc["warnings"].as_array().into_iter().flatten() {
        println!(
            "warning {}: {}",
            text_of(&w["code"]),
            text_of(&w["message"])
        );
    }
    for n in doc["next"].as_array().into_iter().flatten() {
        println!("next   {}", text_of(n));
    }
    Ok(())
}

// ------------------------------------------------------------------ describe

fn describe(home: &Home, args: AskDocArgs) -> Result<(), Exit> {
    let doc = match args.at.door()? {
        Some(door) => door.post("/api/ask/describe", &args.which.body()?)?,
        None => {
            let mut l = local(home, &args.at)?;
            let ask = ask_of(&mut l, &args.which)?;
            let scheme = l.scheme(&ask)?;
            let scope = scope();
            let setting = l.setting(&scheme, &scope);
            let d = affordance::describe(&ask, &setting).map_err(|e| usage(e.to_string()))?;
            serde_json::to_value(d).unwrap_or_default()
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    for s in doc["sets"].as_array().into_iter().flatten() {
        println!("{}", text_of(&s[1]));
    }
    println!("\nconventions");
    for c in doc["conventions"].as_array().into_iter().flatten() {
        println!("  {}", text_of(c));
    }
    if let Some(d) = doc["denominators"].as_array().filter(|a| !a.is_empty()) {
        println!("denominators");
        for x in d {
            println!("  {} counts {}", text_of(&x[0]), text_of(&x[1]));
        }
    }
    if let Some(m) = doc["mechanisms"].as_array().filter(|a| !a.is_empty()) {
        println!("mechanisms");
        for x in m {
            println!(
                "  {} chose {} by {}",
                text_of(&x[0]),
                text_of(&x[1]),
                text_of(&x[2])
            );
        }
    }
    println!("disclosure {}", text_of(&doc["disclosure"]));
    println!("{}", text_of(&doc["answer"]));
    Ok(())
}

// ------------------------------------------------------------------- handles

fn handles(home: &Home, cmd: HandlesCommand) -> Result<(), Exit> {
    match cmd {
        HandlesCommand::List {
            withdrawn,
            limit,
            json,
        } => {
            let mut registry = open(home)?;
            let rows =
                handle::list(registry.store(), withdrawn).map_err(|e| fail(e.to_string()))?;
            if json {
                let doc: Vec<serde_json::Value> = rows
                    .iter()
                    .take(limit)
                    .map(|h| serde_json::to_value(h).unwrap_or_default())
                    .collect();
                print_json(&json!({"count": rows.len(), "handles": doc}));
                return Ok(());
            }
            for h in rows.iter().take(limit) {
                println!(
                    "{:>6}  {:<24}  {:<8}  {:>7} rows  {}  {}",
                    h.id,
                    h.name.clone().unwrap_or_else(|| "-".into()),
                    h.grain.name(),
                    h.row_count,
                    if h.has_rows() { "kept   " } else { "dropped" },
                    h.created_at
                );
            }
            Ok(())
        }
        HandlesCommand::Show(args) => {
            let doc = match args.at.door()? {
                Some(door) => door.get(&format!("/api/ask/handles/{}", args.handle))?,
                None => {
                    let mut registry = open(home)?;
                    let h = handle::get(registry.store(), args.handle)
                        .map_err(|e| fail(e.to_string()))?
                        .ok_or_else(|| usage(format!("no handle {}", args.handle)))?;
                    let pins = handle::pinned_by(registry.store(), args.handle)
                        .map_err(|e| fail(e.to_string()))?;
                    let pages = handle::page_count(registry.store(), args.handle)
                        .map_err(|e| fail(e.to_string()))?;
                    let reads = handle::read_count(registry.store(), args.handle)
                        .map_err(|e| fail(e.to_string()))?;
                    let mut v = serde_json::to_value(&h).unwrap_or_default();
                    v["pinned_by"] = json!(pins);
                    v["pages"] = json!(pages);
                    v["reads"] = json!(reads);
                    v
                }
            };
            if args.at.json {
                print_json(&doc);
                return Ok(());
            }
            println!(
                "handle {}   {}   {} rows   {} pages   {}",
                doc["id"],
                text_of(&doc["grain"]),
                doc["row_count"],
                doc["pages"],
                doc["name"]
                    .as_str()
                    .map(|n| format!("named {n}"))
                    .unwrap_or_else(|| "unnamed".into())
            );
            println!(
                "hash {}   epoch {}   scheme {}   {}",
                doc["content_hash"]
                    .as_str()
                    .unwrap_or("none: the answer was capped"),
                doc["epoch"],
                text_of(&doc["scheme_digest"]),
                text_of(&doc["disclosure"])
            );
            println!(
                "{} at {} on {}   pack {}",
                text_of(&doc["principal"]),
                text_of(&doc["created_at"]),
                text_of(&doc["node"]),
                text_of(&doc["pack_version"])
            );
            let columns: Vec<String> = doc["columns"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| format!("{} ({})", text_of(&c["name"]), text_of(&c["type"])))
                        .collect()
                })
                .unwrap_or_default();
            println!("columns {}", columns.join(", "));
            for p in doc["pinned_by"].as_array().into_iter().flatten() {
                println!("pinned by {}", text_of(p));
            }
            if let Some(w) = doc["withdrawn_at"].as_str() {
                println!(
                    "withdrawn {} by {}: {}",
                    w,
                    text_of(&doc["withdrawn_by"]),
                    text_of(&doc["withdrawn_why"])
                );
            }
            Ok(())
        }
        HandlesCommand::Export(args) => export(home, args),
        HandlesCommand::Prune { keep_days, json } => {
            let mut registry = open(home)?;
            let now = nils_registry::time::now_iso();
            let pruned = handle::prune(registry.store(), &now, keep_days)
                .map_err(|e| fail(e.to_string()))?;
            if json {
                print_json(&serde_json::to_value(&pruned).unwrap_or_default());
                return Ok(());
            }
            println!(
                "dropped the rows of {} handles unread for {keep_days} days",
                pruned.dropped.len()
            );
            for (id, why) in &pruned.pinned {
                println!("kept {id}: {}", why.join(", "));
            }
            Ok(())
        }
    }
}

/// A handle's rows as CSV, read from its stored pages: no compiler, no
/// statement, no database driver (§15, slice 10).
fn export(home: &Home, args: ExportArgs) -> Result<(), Exit> {
    let (columns, pages): (Vec<String>, Vec<Vec<Vec<serde_json::Value>>>) = match args.at.door()? {
        Some(door) => {
            let head = door.get(&format!("/api/ask/handles/{}", args.handle))?;
            let columns = head["columns"]
                .as_array()
                .map(|a| a.iter().map(|c| text_of(&c["name"])).collect())
                .unwrap_or_default();
            let n = head["pages"].as_i64().unwrap_or(0);
            let mut pages = Vec::new();
            for page in 0..n {
                let doc = door.get(&format!(
                    "/api/ask/handles/{}/rows?page={page}",
                    args.handle
                ))?;
                pages.push(
                    serde_json::from_value(doc["rows"].clone())
                        .map_err(|e| fail(format!("page {page}: {e}")))?,
                );
            }
            (columns, pages)
        }
        None => {
            let mut registry = open(home)?;
            let h = handle::get(registry.store(), args.handle)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no handle {}", args.handle)))?;
            if !h.has_rows() {
                return Err(usage(format!(
                    "handle {} has expired: its rows were dropped by retention and only its metadata, hash and ask remain",
                    args.handle
                )));
            }
            let n = handle::page_count(registry.store(), args.handle)
                .map_err(|e| fail(e.to_string()))?;
            let mut pages = Vec::new();
            for page in 0..n {
                if let Some(rows) = handle::page(registry.store(), args.handle, page)
                    .map_err(|e| fail(e.to_string()))?
                {
                    pages.push(rows);
                }
            }
            // Wave 4c §6.1: an export is a read, audited like a page.
            let columns: Vec<String> = h.columns.iter().map(|c| c.name.clone()).collect();
            let total: usize = pages.iter().map(Vec::len).sum();
            let epoch = registry.meta().epoch;
            handle::read_audit(
                registry.store(),
                &principal(),
                args.handle,
                &columns,
                total,
                Some("nils ask handles export"),
                epoch,
            )
            .map_err(|e| fail(e.to_string()))?;
            (columns, pages)
        }
    };
    let mut writer: Box<dyn std::io::Write> = match &args.out {
        Some(path) => Box::new(
            std::fs::File::create(path).map_err(|e| usage(format!("{}: {e}", path.display())))?,
        ),
        None => Box::new(std::io::stdout()),
    };
    let mut csv = csv::Writer::from_writer(&mut writer);
    csv.write_record(&columns)
        .map_err(|e| fail(format!("the header: {e}")))?;
    let mut rows = 0;
    for page in &pages {
        for row in page {
            let cells: Vec<String> = row
                .iter()
                .map(|c| match c {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            csv.write_record(&cells)
                .map_err(|e| fail(format!("a row: {e}")))?;
            rows += 1;
        }
    }
    csv.flush().map_err(|e| fail(e.to_string()))?;
    drop(csv);
    if let Some(path) = &args.out {
        println!("{rows} rows to {}", path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------- selections

fn selections(home: &Home, cmd: SelectionsCommand) -> Result<(), Exit> {
    match cmd {
        SelectionsCommand::Save(args) => {
            let doc = match args.at.door()? {
                Some(door) => {
                    let mut body = args.which.body()?;
                    if let Some(n) = &args.note {
                        body["note"] = json!(n);
                    }
                    if let Some(d) = &args.description {
                        body["description"] = json!(d);
                    }
                    door.put(&format!("/api/ask/selections/{}", args.name), &body)?
                }
                None => {
                    let mut l = local(home, &args.at)?;
                    let ask = ask_of(&mut l, &args.which)?;
                    let scope = scope();
                    let prepared = nils_ask::prepare(ask, &l.catalog, &scope).map_err(refused)?;
                    let saved = selection::save(
                        &mut l.registry,
                        &args.name,
                        &prepared.ask,
                        &prepared.hash,
                        &principal(),
                        args.note.as_deref(),
                        args.description.as_deref(),
                    )
                    .map_err(|e| usage(e.to_string()))?;
                    serde_json::to_value(saved).unwrap_or_default()
                }
            };
            if args.at.json {
                print_json(&doc);
                return Ok(());
            }
            println!(
                "{} at version {}   hash {}   epoch {}",
                text_of(&doc["name"]),
                doc["version"],
                text_of(&doc["hash"]),
                doc["epoch"]
            );
            Ok(())
        }
        SelectionsCommand::List { json } => {
            let mut registry = open(home)?;
            let rows = selection::list(registry.store()).map_err(|e| fail(e.to_string()))?;
            if json {
                print_json(&serde_json::to_value(&rows).unwrap_or_default());
                return Ok(());
            }
            for s in &rows {
                println!(
                    "{:<28}  version {:<4}  {}  {}",
                    s.name,
                    s.current_version,
                    s.owner,
                    s.cohort_id
                        .map(|c| format!("the source ask of cohort {c}"))
                        .unwrap_or_default()
                );
            }
            Ok(())
        }
        SelectionsCommand::Show(args) => {
            let doc = match args.at.door()? {
                Some(door) => door.get(&format!("/api/ask/selections/{}", args.name))?,
                None => {
                    let (name, version) = match args.name.rsplit_once('@') {
                        Some((n, v)) => (
                            n.to_string(),
                            Some(
                                v.parse::<u64>()
                                    .map_err(|_| usage(format!("{v} is not a version")))?,
                            ),
                        ),
                        None => (args.name.clone(), None),
                    };
                    let mut registry = open(home)?;
                    let v = selection::get(registry.store(), &name, version)
                        .map_err(|e| fail(e.to_string()))?
                        .ok_or_else(|| usage(format!("no selection {}", args.name)))?;
                    serde_json::to_value(v).unwrap_or_default()
                }
            };
            if args.at.json {
                print_json(&doc);
                return Ok(());
            }
            println!(
                "{} version {} of {}   hash {}   {} by {}",
                text_of(&doc["name"]),
                doc["version"],
                doc["current_version"],
                text_of(&doc["hash"]),
                text_of(&doc["created_at"]),
                text_of(&doc["actor"])
            );
            if let Some(note) = doc["note"].as_str() {
                println!("note {note}");
            }
            println!(
                "{}",
                nils_ask::to_yaml(
                    &serde_json::from_value(doc["ask"].clone())
                        .map_err(|e| fail(format!("the stored ask: {e}")))?
                )
                .map_err(|e| fail(e.to_string()))?
            );
            Ok(())
        }
    }
}

/// A refused document, with every issue and the call that settles it.
fn refused(e: nils_ask::Error) -> Exit {
    match e {
        nils_ask::Error::Invalid(issues) => {
            let mut message = String::from("the document is refused:");
            for i in &issues {
                message.push_str(&format!(
                    "\n  {} at {}: {} ({})",
                    i.code.name(),
                    i.path,
                    i.message,
                    i.next
                ));
            }
            usage(message)
        }
        other => usage(other.to_string()),
    }
}

fn ask_run(home: &Home, args: AskRunArgs) -> Result<(), Exit> {
    // A running engine answers the same call, and leaves the same handle.
    if let Some(url) = &args.server {
        let at = Where {
            server: Some(url.clone()),
            token: args.token.clone(),
            pack_dir: None,
            pack: args.pack.clone(),
            json: args.json,
        };
        let door = at.door()?.expect("a server was named");
        let which = Which {
            file: args.file.clone(),
            document: args.document,
        };
        let mut body = which.body()?;
        if let Some(n) = &args.name {
            body["name"] = json!(n);
        }
        body["keep"] = json!(args.keep);
        let doc = door.post("/api/ask/run", &body)?;
        if args.json {
            print_json(&doc);
        } else {
            println!(
                "nils ask run   handle {}   {} rows   hash {}   {}",
                doc["handle"],
                doc["row_count"],
                text_of(&doc["hash"]),
                if doc["truncated"].as_bool().unwrap_or(false) {
                    "truncated"
                } else {
                    "complete"
                }
            );
        }
        return Ok(());
    }
    let dir = crate::pack_dir(home, args.pack_dir.clone()).map_err(|_| {
        usage("no packs here: pass --pack-dir DIR, or --server URL to run on a running engine")
    })?;
    let mut registry = open(home)?;
    let pack = load_pack(&dir, &args.pack)?;
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
    // Wave 4c §6.1: under a claim a door queued, the roles the door
    // recorded decide the scope and the projection, never the worker's own;
    // from a terminal, the operator at the keyboard holds every class.
    let queued_roles = std::env::var("NILS_JOB_ROLES")
        .ok()
        .filter(|r| !r.is_empty());
    let (scope, may_project_raw) = match &queued_roles {
        Some(list) => {
            let roles: Vec<crate::serve::Role> = list
                .split(',')
                .filter_map(crate::serve::Role::parse)
                .collect();
            (
                crate::ask_doors::scope_of_roles(&roles),
                crate::ask_doors::may_project_raw_of_roles(&roles)
                    && std::env::var("NILS_JOB_RAW").ok().as_deref() == Some("1"),
            )
        }
        None => (scope(), true),
    };
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
            may_project_raw,
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
            // Wave 4c §6.1: the job row carries what it produced.
            job::set_result(registry.store(), job, &doc).map_err(|e| fail(e.to_string()))?;
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
            // Wave 4c section 6.1: the job row carries what it produced.
            job::set_result(
                registry.store(),
                job,
                &serde_json::to_value(&p).unwrap_or_default(),
            )
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

/// `nils ask draft`: the draft affordance, in process or at a door.
fn draft(home: &Home, args: AskDraftArgs) -> Result<(), Exit> {
    let text = if args.file.to_str() == Some("-") {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)
            .map_err(|e| usage(format!("standard input: {e}")))?;
        t
    } else {
        std::fs::read_to_string(&args.file)
            .map_err(|e| usage(format!("{}: {e}", args.file.display())))?
    };
    let doc = match args.at.door()? {
        Some(door) => door.post("/api/ask/draft", &json!({"text": text}))?,
        None => {
            let mut registry = open(home)?;
            let dir = crate::pack_dir(home, args.at.pack_dir.clone()).map_err(|_| {
                usage("no packs here: pass --pack-dir DIR, when drafting in process")
            })?;
            let pack = load_pack(&dir, &args.at.pack)?;
            let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
            let scope = scope();
            let scheme = Scheme::default();
            let s = nils_ask::affordance::Setting {
                names: &catalog,
                scope: &scope,
                scheme: &scheme,
                principal: &principal(),
                bounds: bounds(),
                values_cap: Caps::default().options_values as usize,
            };
            let d = nils_ask::affordance::draft(&mut registry, &text, &s, None)
                .map_err(|e| fail(e.to_string()))?;
            serde_json::to_value(d).unwrap_or_default()
        }
    };
    if args.at.json {
        print_json(&doc);
        return Ok(());
    }
    match doc["document"].as_i64() {
        Some(id) => println!(
            "nils ask draft   document {id}   hash {}   {} repairs",
            text_of(&doc["hash"]),
            doc["repairs"].as_array().map_or(0, Vec::len)
        ),
        None => println!(
            "nils ask draft   not valid   {} repairs   {} issues",
            doc["repairs"].as_array().map_or(0, Vec::len),
            doc["diagnosis"]["issues"].as_array().map_or(0, Vec::len)
        ),
    }
    Ok(())
}
