// SPDX-License-Identifier: AGPL-3.0-only

//! `nils`, the binary: the command line, configuration, `custody` and the output
//! formatting (`docs/specs/wave1-parse-and-digest.md`, §3 and §13).
//!
//! Slice 4 of the build has `init`, `key`, `digest` (writing, `--dry-run` or
//! `--describe`, with `--identity-rule`), `status` and `linkage` (`import`,
//! `id-type`, `show`, `link`, `unlink`); `custody`, `quarantine`, `review`,
//! `linkage purge` and `doctor` arrive with the slices that give them
//! something to do (§14).
//!
//! Exit codes: 0 done; 1 the command failed; 2 the arguments or the
//! configuration are wrong; 3 another job holds the registry.

use std::fs;
use std::io::{self, Read as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use nils_digest::{DigestError, Filter, Report, Rule, Settings};
use nils_registry::home::{Config, Home, InitOptions, REGISTRY_ENV};
use nils_registry::keys::strip_newline;
use nils_registry::linkage::{self, ImportError, ImportRow, Subkeys};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Param, Registry, Scheme, Store};

const FAILED: u8 = 1;
const USAGE: u8 = 2;
const BUSY: u8 = 3;

/// NILS digests DICOM into a registry: one binary, on a laptop or a server.
#[derive(Debug, Parser)]
#[command(name = "nils", version, about, arg_required_else_help = true)]
struct Cli {
    /// The registry home: nils.toml, the key store and, on SQLite, the
    /// databases. NILS_REGISTRY, else the working directory.
    #[arg(long, global = true, value_name = "DIR")]
    registry: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a registry in the home: its configuration and its schema
    Init(InitArgs),
    /// The key store: the keys the pseudonyms are derived from
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    /// Walk a tree of DICOM files, read every header and digest it into the registry
    Digest(DigestArgs),
    /// The registry: its metadata, the running jobs, the last batches
    Status(StatusArgs),
    /// The linkage store: the identifiers behind the codes (§7)
    Linkage {
        #[command(subcommand)]
        command: LinkageCommand,
    },
}

#[derive(Debug, Args)]
struct InitArgs {
    /// sqlite (the databases in the home) or postgres (a dsn)
    #[arg(long, default_value = "sqlite", value_name = "sqlite|postgres")]
    backend: String,
    /// The Postgres connection string; NILS_DSN at run time when not written here
    #[arg(long, value_name = "DSN")]
    dsn: Option<String>,
    /// The Postgres schema of the registry; the linkage store lives in <schema>_linkage
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    /// The pseudonym scheme
    #[arg(
        long,
        default_value = "blake2b-32",
        value_name = "blake2b-32|blake2b-8"
    )]
    scheme: String,
    /// The name of the key in the key store the pseudonyms are derived from
    #[arg(long, value_name = "NAME")]
    key: String,
    /// Characters of the pseudonym shown (blake2b-32)
    #[arg(long, default_value_t = 12, value_name = "N")]
    display_length: usize,
    /// A JSON file with the session scheme (§12.4); the default scheme otherwise
    #[arg(long, value_name = "FILE")]
    session_scheme: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum KeyCommand {
    /// Store a key under a name, from a file or from stdin
    Add {
        name: String,
        /// Read the key from this file instead of stdin
        #[arg(long, value_name = "PATH")]
        from_file: Option<PathBuf>,
    },
    /// The keys in the store, by name, with their fingerprints
    List,
    /// Remove a key the registry does not use
    Remove { name: String },
}

#[derive(Debug, Args)]
struct DigestArgs {
    /// The directory to walk
    root: PathBuf,
    /// The batch's label; the root's basename and today's date by default
    #[arg(long)]
    name: Option<String>,
    /// Parser threads; one per core by default
    #[arg(long, value_name = "N")]
    workers: Option<usize>,
    /// Walker threads
    #[arg(long, value_name = "N")]
    walk_threads: Option<usize>,
    /// Instances per batch, one transaction each
    #[arg(long, value_name = "N")]
    batch_rows: Option<usize>,
    /// Which file names are candidates: all, dcm, no-ext, or a glob
    #[arg(long, default_value = "all", value_name = "all|dcm|no-ext|<glob>")]
    files: String,
    /// A YAML file with the identity rule (§7.3); PatientID, then StudyInstanceUID, by default
    #[arg(long, value_name = "FILE")]
    identity_rule: Option<PathBuf>,
    /// Read the quarantined files again
    #[arg(long)]
    retry_quarantine: bool,
    /// Read every file again, changed or not
    #[arg(long)]
    restart: bool,
    /// Walk and read everything, print the report, write nothing
    #[arg(long)]
    dry_run: bool,
    /// Print the effective knobs and exit
    #[arg(long)]
    describe: bool,
    /// Machine-readable output: the report as one JSON document, progress as JSON lines
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum LinkageCommand {
    /// File the identifier → code pairs of a CSV, creating the subjects the codes name
    Import(ImportArgs),
    /// The identifier types
    IdType {
        #[command(subcommand)]
        command: IdTypeCommand,
    },
    /// Decrypt the identifiers of a subject; every read is audited
    Show {
        /// The subject's code
        code: String,
        /// Why the identifiers are read; written to the audit
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
    },
    /// Record that two subjects are one person: A is canonical, B the alias
    Link {
        /// The canonical subject's code
        a: String,
        /// The alias subject's code
        b: String,
        /// What shows they are one person
        #[arg(long, value_name = "TEXT")]
        evidence: String,
    },
    /// Reverse a linkage by its id
    Unlink {
        /// The linkage's id, as link printed and show lists it
        id: i64,
    },
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// The CSV: a header row, then one identifier and its code per row
    csv: PathBuf,
    /// The type the identifiers are filed under
    #[arg(long, default_value = "patient-id", value_name = "NAME")]
    id_type: String,
    /// The header of the identifier column
    #[arg(long, default_value = "identifier", value_name = "HEADER")]
    id_column: String,
    /// The header of the code column
    #[arg(long, default_value = "code", value_name = "HEADER")]
    code_column: String,
}

#[derive(Debug, Subcommand)]
enum IdTypeCommand {
    /// Add an identifier type: lower case letters, digits and hyphens
    Add {
        name: String,
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
    },
    /// The identifier types, by id
    List,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Print one batch's report instead
    #[arg(long, value_name = "ID")]
    batch: Option<i64>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = Home::resolve(cli.registry.as_deref());
    let outcome = match cli.command {
        Command::Init(args) => init(&home, args),
        Command::Key { command } => key(&home, command),
        Command::Digest(args) => digest(&home, args),
        Command::Status(args) => status(&home, args),
        Command::Linkage { command } => linkage_command(&home, command),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(Exit { code, message }) => {
            eprintln!("nils: {message}");
            ExitCode::from(code)
        }
    }
}

/// How a command ends when it does not succeed.
struct Exit {
    code: u8,
    message: String,
}

fn fail(message: impl Into<String>) -> Exit {
    Exit {
        code: FAILED,
        message: message.into(),
    }
}

fn usage(message: impl Into<String>) -> Exit {
    Exit {
        code: USAGE,
        message: message.into(),
    }
}

impl From<nils_registry::HomeError> for Exit {
    fn from(e: nils_registry::HomeError) -> Exit {
        fail(e.to_string())
    }
}

impl From<nils_registry::KeyError> for Exit {
    fn from(e: nils_registry::KeyError) -> Exit {
        fail(e.to_string())
    }
}

impl From<nils_registry::Error> for Exit {
    fn from(e: nils_registry::Error) -> Exit {
        fail(e.to_string())
    }
}

fn init(home: &Home, args: InitArgs) -> Result<(), Exit> {
    let backend: Backend = args
        .backend
        .parse()
        .map_err(|_| usage(format!("--backend {}: sqlite or postgres", args.backend)))?;
    let scheme: Scheme = args
        .scheme
        .parse()
        .map_err(|_| usage(format!("--scheme {}: blake2b-32 or blake2b-8", args.scheme)))?;
    let session_scheme = match &args.session_scheme {
        Some(path) => Some(
            fs::read_to_string(path)
                .map_err(|e| usage(format!("--session-scheme {}: {e}", path.display())))?,
        ),
        None => None,
    };
    let opts = InitOptions {
        backend,
        dsn: args.dsn,
        schema: args.schema,
        scheme,
        key: args.key,
        display_length: args.display_length,
        session_scheme,
    };
    let registry = home.init(&opts)?;
    let meta = registry.meta();
    println!(
        "initialised {} on {}: registry {}, schema version {}, pseudonyms {} from key {}",
        home.dir().display(),
        backend.name(),
        meta.registry_id,
        meta.schema_version,
        meta.pseudonym_scheme,
        meta.pseudonym_key
    );
    Ok(())
}

fn key(home: &Home, command: KeyCommand) -> Result<(), Exit> {
    // before init the store is where init will look for it
    let config: Option<Config> = if home.exists() {
        Some(home.read_config()?)
    } else {
        None
    };
    let keys = home.keys(config.as_ref());
    match command {
        KeyCommand::Add { name, from_file } => {
            let raw = match &from_file {
                Some(path) => fs::read(path)
                    .map_err(|e| usage(format!("--from-file {}: {e}", path.display())))?,
                None => {
                    let mut buf = Vec::new();
                    io::stdin()
                        .read_to_end(&mut buf)
                        .map_err(|e| fail(format!("stdin: {e}")))?;
                    buf
                }
            };
            let (bytes, stripped) = strip_newline(&raw);
            let info = keys.add(&name, bytes)?;
            println!(
                "added key {} ({} bytes{}, fingerprint {})",
                info.name,
                info.bytes,
                if stripped {
                    ", trailing newline dropped"
                } else {
                    ""
                },
                info.fingerprint
            );
        }
        KeyCommand::List => {
            let in_use = key_in_use(home, config.as_ref());
            let list = keys.list()?;
            if list.is_empty() {
                println!("no keys in {}", keys.dir().display());
            }
            for k in list {
                let mark = if in_use.as_deref() == Some(k.name.as_str()) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{mark} {:<24} {:>3} bytes  {}",
                    k.name, k.bytes, k.fingerprint
                );
            }
        }
        KeyCommand::Remove { name } => {
            let in_use = key_in_use(home, config.as_ref());
            keys.remove(&name, in_use.as_deref())?;
            println!("removed key {name}");
        }
    }
    Ok(())
}

/// The name of the key the registry's pseudonyms come from, when there is a
/// registry to ask.
fn key_in_use(home: &Home, config: Option<&Config>) -> Option<String> {
    config?;
    home.open().ok().map(|r| r.meta().pseudonym_key.clone())
}

fn digest(home: &Home, args: DigestArgs) -> Result<(), Exit> {
    let mut settings = Settings::new(args.root);
    settings.dry_run = args.dry_run;
    settings.json = args.json;
    settings.retry_quarantine = args.retry_quarantine;
    settings.restart = args.restart;
    if let Some(name) = args.name {
        settings.name = name;
    }
    for (flag, value, slot) in [
        ("--workers", args.workers, &mut settings.workers),
        (
            "--walk-threads",
            args.walk_threads,
            &mut settings.walk_threads,
        ),
        ("--batch-rows", args.batch_rows, &mut settings.batch_rows),
    ] {
        if let Some(n) = value {
            if n == 0 {
                return Err(usage(format!("{flag} must be at least 1")));
            }
            *slot = n;
        }
    }
    settings.filter =
        Filter::parse(&args.files).map_err(|e| usage(format!("--files {}: {e}", args.files)))?;
    if let Some(path) = &args.identity_rule {
        let text = fs::read_to_string(path)
            .map_err(|e| usage(format!("--identity-rule {}: {e}", path.display())))?;
        let mut rule = Rule::parse(&text)
            .map_err(|e| usage(format!("--identity-rule {}: {e}", path.display())))?;
        rule.source = Some(path.display().to_string());
        settings.identity = rule;
    }

    if args.describe {
        print!("{}", settings.describe());
        return Ok(());
    }
    let result = if settings.dry_run {
        nils_digest::dry_run(&settings)
    } else {
        let mut registry = open(home)?;
        nils_digest::digest(&settings, &mut registry)
    };
    match result {
        Ok(report) => print_report(&report, settings.json),
        Err(e @ DigestError::Busy { .. }) => Err(Exit {
            code: BUSY,
            message: e.to_string(),
        }),
        Err(e) => Err(fail(e.to_string())),
    }
}

fn print_report(report: &Report, json: bool) -> Result<(), Exit> {
    if json {
        let text = serde_json::to_string_pretty(report)
            .map_err(|e| fail(format!("cannot render the report: {e}")))?;
        println!("{text}");
    } else {
        print!("{report}");
    }
    Ok(())
}

/// The registry of the home, with a word about where to look when there is
/// none.
fn open(home: &Home) -> Result<Registry, Exit> {
    if !home.exists() {
        return Err(usage(format!(
            "no registry in {}; run nils init there, or point --registry or {REGISTRY_ENV} at one",
            home.dir().display()
        )));
    }
    Ok(home.open()?)
}

/// A column as text on either backend, for the timestamps and the JSON.
fn text_of(store: &Store, table_name: &str, column: &str) -> String {
    let t = table(table_name);
    let c = t
        .column(column)
        .unwrap_or_else(|| panic!("{table_name}.{column} is not a column"));
    store.dialect().text_of(c)
}

fn status(home: &Home, args: StatusArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    if let Some(id) = args.batch {
        return batch_report(&mut registry, id, args.json);
    }
    let meta = registry.meta().clone();
    let config = registry.config().clone();
    let store = registry.store();

    let jobs_sql = format!(
        "SELECT id, kind, name, pid, host, {}, {}, {} FROM {} WHERE state = 'running' ORDER BY id",
        text_of(store, "job", "started_at"),
        text_of(store, "job", "heartbeat_at"),
        text_of(store, "job", "progress"),
        store.qualified("job")
    );
    let jobs: Vec<serde_json::Value> = store
        .query(&jobs_sql, &[])?
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "id": r.int(0)?,
                "kind": r.text(1)?,
                "name": r.opt_text(2)?,
                "pid": r.opt_int(3)?,
                "host": r.opt_text(4)?,
                "started_at": r.opt_text(5)?,
                "heartbeat_at": r.opt_text(6)?,
                "progress": r.opt_text(7)?.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
            }))
        })
        .collect::<Result<_, nils_registry::Error>>()?;

    let batches_sql = format!(
        "SELECT id, name, state, {}, {}, epoch_after, {} FROM {} ORDER BY id DESC LIMIT 10",
        text_of(store, "ingest_batch", "started_at"),
        text_of(store, "ingest_batch", "finished_at"),
        text_of(store, "ingest_batch", "counts"),
        store.qualified("ingest_batch")
    );
    let batches: Vec<serde_json::Value> = store
        .query(&batches_sql, &[])?
        .iter()
        .map(|r| {
            let counts = r
                .opt_text(6)?
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            let pick = |path: &[&str]| -> Option<u64> {
                let mut v = counts.as_ref()?;
                for p in path {
                    v = v.get(p)?;
                }
                v.as_u64()
            };
            Ok(serde_json::json!({
                "id": r.int(0)?,
                "name": r.text(1)?,
                "state": r.text(2)?,
                "started_at": r.opt_text(3)?,
                "finished_at": r.opt_text(4)?,
                "epoch_after": r.opt_int(5)?,
                "seen": pick(&["seen"]),
                "parsed": pick(&["parsed"]),
                "quarantined": pick(&["quarantined"]),
                "ingested": pick(&["written", "ingested"]),
            }))
        })
        .collect::<Result<_, nils_registry::Error>>()?;

    let doc = serde_json::json!({
        "registry": {
            "home": home.dir().display().to_string(),
            "backend": config.backend.name(),
            "schema": store.schema(),
            "registry_id": meta.registry_id,
            "schema_version": meta.schema_version,
            "epoch": meta.epoch,
            "created_at": meta.created_at,
            "pseudonym_scheme": meta.pseudonym_scheme.to_string(),
            "pseudonym_key": meta.pseudonym_key,
            "display_length": meta.display_length,
        },
        "jobs": jobs,
        "batches": batches,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }

    println!(
        "nils status   registry {}   backend {}{}",
        home.dir().display(),
        config.backend.name(),
        match store.schema() {
            Some(s) => format!("   schema {s}"),
            None => String::new(),
        }
    );
    println!("  registry id      {}", meta.registry_id);
    println!("  schema version   {}", meta.schema_version);
    println!("  epoch            {}", meta.epoch);
    println!("  created          {}", meta.created_at);
    println!(
        "  pseudonyms       {} from key {}, {} characters shown",
        meta.pseudonym_scheme, meta.pseudonym_key, meta.display_length
    );
    println!("running jobs");
    if jobs.is_empty() {
        println!("  none");
    }
    for j in &jobs {
        println!(
            "  job {}   {} {}   pid {} on {}   started {}   heartbeat {}",
            j["id"],
            s(&j["kind"]),
            s(&j["name"]),
            n(&j["pid"]),
            s(&j["host"]),
            s(&j["started_at"]),
            s(&j["heartbeat_at"])
        );
    }
    println!("batches (last {})", batches.len());
    if batches.is_empty() {
        println!("  none");
    } else {
        println!(
            "  {:>5}  {:<8} {:<28} {:<20} {:<20} {:>6} {:>10} {:>10} {:>10}",
            "id", "state", "name", "started", "finished", "epoch", "seen", "parsed", "ingested"
        );
    }
    for b in &batches {
        println!(
            "  {:>5}  {:<8} {:<28} {:<20} {:<20} {:>6} {:>10} {:>10} {:>10}",
            b["id"],
            s(&b["state"]),
            s(&b["name"]),
            s(&b["started_at"]),
            s(&b["finished_at"]),
            n(&b["epoch_after"]),
            n(&b["seen"]),
            n(&b["parsed"]),
            n(&b["ingested"]),
        );
    }
    Ok(())
}

/// One batch's report, as `ingest_batch.counts` recorded it.
fn batch_report(registry: &mut Registry, id: i64, json: bool) -> Result<(), Exit> {
    let store = registry.store();
    let sql = format!(
        "SELECT state, {} FROM {} WHERE id = {}",
        text_of(store, "ingest_batch", "counts"),
        store.qualified("ingest_batch"),
        store.dialect().param(1, Type::Int)
    );
    let row = store
        .query_opt(&sql, &[Param::Int(id)])?
        .ok_or_else(|| fail(format!("no batch {id}")))?;
    let state = row.text(0)?.to_string();
    let Some(counts) = row.opt_text(1)? else {
        return Err(fail(format!("batch {id} is {state} and has no report yet")));
    };
    if json {
        println!("{counts}");
        return Ok(());
    }
    let report: Report = serde_json::from_str(counts)
        .map_err(|e| fail(format!("batch {id}: the report does not parse: {e}")))?;
    print!("{report}");
    Ok(())
}

/// Who runs the command, for the audit and the linkage rows: the OS user.
fn actor() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn linkage_command(home: &Home, command: LinkageCommand) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let mut store = registry.open_linkage()?;
    match command {
        LinkageCommand::Import(args) => import(&mut registry, &mut store, args),
        LinkageCommand::IdType { command } => match command {
            IdTypeCommand::Add { name, description } => {
                let t = linkage::add_id_type(&mut store, &name, description.as_deref())?;
                println!("added id type {} (id {})", t.name, t.id);
                Ok(())
            }
            IdTypeCommand::List => {
                for t in linkage::id_types(&mut store)? {
                    println!(
                        "{:>3}  {:<24} {}",
                        t.id,
                        t.name,
                        t.description.as_deref().unwrap_or("")
                    );
                }
                Ok(())
            }
        },
        LinkageCommand::Show { code, why } => {
            let subject = subject_of(&mut registry, &code)?;
            let keys = Subkeys::derive(&registry.pseudonym_key()?);
            let shown = linkage::reveal(&mut store, &keys, subject, &actor(), why.as_deref())?;
            println!("subject {code} (id {subject})");
            if shown.is_empty() {
                println!("  no identifiers");
            }
            for r in &shown {
                println!(
                    "  {:<24} {}   (identity {}, from {})",
                    r.id_type, r.value, r.identity_id, r.source
                );
            }
            let links = linkage::linkages_of(&mut store, subject)?;
            if !links.is_empty() {
                let ids: Vec<i64> = links
                    .iter()
                    .flat_map(|l| [l.subject_a, l.subject_b])
                    .filter(|&id| id != subject)
                    .collect();
                let codes: std::collections::HashMap<i64, String> =
                    linkage::subjects_by_id(registry.store(), &ids)?
                        .into_iter()
                        .map(|s| (s.id, s.code))
                        .collect();
                let name = |id: i64| codes.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
                println!("linkages");
                for l in &links {
                    let (role, other) = if l.subject_a == subject {
                        ("canonical of", l.subject_b)
                    } else {
                        ("alias of", l.subject_a)
                    };
                    let state = match &l.reversed_at {
                        Some(at) => format!(
                            "reversed {at} by {}",
                            l.reversed_by.as_deref().unwrap_or("-")
                        ),
                        None => "open".to_string(),
                    };
                    println!(
                        "  {:>4}  {role} {}   {}   by {} at {}   {state}",
                        l.id,
                        name(other),
                        l.evidence,
                        l.actor.as_deref().unwrap_or("-"),
                        l.created_at
                    );
                }
            }
            Ok(())
        }
        LinkageCommand::Link { a, b, evidence } => {
            let subject_a = subject_of(&mut registry, &a)?;
            let subject_b = subject_of(&mut registry, &b)?;
            let id = linkage::link(&mut store, subject_a, subject_b, &evidence, &actor())?;
            println!("linked {b} to {a} (linkage {id})");
            Ok(())
        }
        LinkageCommand::Unlink { id } => {
            if linkage::unlink(&mut store, id, &actor())? {
                println!("reversed linkage {id}");
                Ok(())
            } else {
                Err(fail(format!("no open linkage {id}")))
            }
        }
    }
}

/// The subject a code names.
fn subject_of(registry: &mut Registry, code: &str) -> Result<i64, Exit> {
    linkage::subjects_by_code(registry.store(), &[code.to_string()])?
        .into_iter()
        .next()
        .map(|s| s.id)
        .ok_or_else(|| fail(format!("no subject with code {code}")))
}

/// `nils linkage import`: the CSV read whole, checked whole, then filed.
fn import(registry: &mut Registry, store: &mut Store, args: ImportArgs) -> Result<(), Exit> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_path(&args.csv)
        .map_err(|e| usage(format!("{}: {e}", args.csv.display())))?;
    let headers = reader
        .headers()
        .map_err(|e| usage(format!("{}: {e}", args.csv.display())))?
        .clone();
    let column = |name: &str, flag: &str| -> Result<usize, Exit> {
        headers.iter().position(|h| h == name).ok_or_else(|| {
            usage(format!(
                "{}: no column {name:?} in the header ({}); {flag} names it",
                args.csv.display(),
                headers.iter().collect::<Vec<_>>().join(", ")
            ))
        })
    };
    let id_col = column(&args.id_column, "--id-column")?;
    let code_col = column(&args.code_column, "--code-column")?;
    let mut rows = Vec::new();
    for (i, record) in reader.records().enumerate() {
        let line = i + 2;
        let record =
            record.map_err(|e| usage(format!("{} line {line}: {e}", args.csv.display())))?;
        rows.push(ImportRow {
            line,
            identifier: record.get(id_col).unwrap_or("").to_string(),
            code: record.get(code_col).unwrap_or("").to_string(),
        });
    }
    let keys = Subkeys::derive(&registry.pseudonym_key()?);
    match linkage::import(registry.store(), store, &keys, &args.id_type, &rows) {
        Ok(report) => {
            println!(
                "imported {} row(s) as {}: {} subject(s) created, {} identifier(s) filed, {} already filed",
                report.rows,
                args.id_type,
                report.subjects_created,
                report.identities_added,
                report.unchanged
            );
            Ok(())
        }
        Err(e @ ImportError::Faults(_)) => Err(fail(e.to_string().trim_end().to_string())),
        Err(ImportError::Store(e)) => Err(fail(e.to_string())),
    }
}

fn s(v: &serde_json::Value) -> &str {
    v.as_str().unwrap_or("-")
}

fn n(v: &serde_json::Value) -> String {
    match v.as_u64() {
        Some(n) => nils_digest::report::thousands(n),
        None => "-".to_string(),
    }
}
