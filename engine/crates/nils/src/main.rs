// SPDX-License-Identifier: AGPL-3.0-only

//! `nils`, the binary: the command line, configuration, `custody` and the output
//! formatting (`docs/specs/wave1-parse-and-digest.md`, §3 and §13).
//!
//! Slice 6 of the build has `init`, `key`, `digest` (writing, `--dry-run` or
//! `--describe`, with `--identity-rule`, stoppable with one signal and
//! abandonable with two), `status`, `linkage` (`import`, `id-type`, `show`,
//! `link`, `unlink`, `purge`), `quarantine list`, `review list | show` and
//! `custody`; `review apply` is Wave 4's and `doctor` arrives with the slice
//! that gives it something to do (§14).
//!
//! Exit codes: 0 done; 1 the command failed; 2 the arguments or the
//! configuration are wrong; 3 another job holds the registry; 130 the run
//! was stopped by a signal (§10: what was read is written, and the report
//! says so).

use std::fs;
use std::io::{self, Read as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use nils_digest::{Cancel, Cancelled, DigestError, Filter, Report, Rule, Settings};
use nils_registry::home::{
    Config, DSN_ENV, Home, InitOptions, LINKAGE_DB, REGISTRY_DB, REGISTRY_ENV,
};
use nils_registry::keys::strip_newline;
use nils_registry::linkage::{self, ImportError, ImportRow, Subkeys};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};

const FAILED: u8 = 1;
const USAGE: u8 = 2;
const BUSY: u8 = 3;
const STOPPED: u8 = 130;

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
    /// The files a digest refused, with the reason (§5.3)
    Quarantine {
        #[command(subcommand)]
        command: QuarantineCommand,
    },
    /// The review items: what a run left for a person to decide (§11)
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// Every store this registry keeps: where, what it holds, how long, and the command that changes it
    Custody {
        /// Machine-readable output
        #[arg(long, conflicts_with = "markdown")]
        json: bool,
        /// The table as a Markdown page, for the deployment's record
        #[arg(long)]
        markdown: bool,
    },
}

#[derive(Debug, Subcommand)]
enum QuarantineCommand {
    /// The quarantined files, by batch and path
    List {
        /// Only the files of this batch
        #[arg(long, value_name = "ID")]
        batch: Option<i64>,
        /// Only this class (not_dicom, missing_uid, ...)
        #[arg(long, value_name = "CLASS")]
        class: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ReviewCommand {
    /// The review items, oldest first
    List {
        /// Only this kind (ingest.quarantine, identity.collision)
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Only this status (open, accepted, rejected, superseded)
        #[arg(long, value_name = "STATUS")]
        status: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One review item in full
    Show {
        id: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
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
    /// Which file names are candidates: all, dcm, no-ext, a glob, or a comma-separated union of those
    #[arg(
        long,
        default_value = "all",
        value_name = "all|dcm|no-ext|<glob>[,...]"
    )]
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
    /// Delete the identifiers and linkages of one subject, or of every subject
    Purge {
        /// The subject's code
        #[arg(long, value_name = "CODE", conflicts_with = "all")]
        subject: Option<String>,
        /// Every subject
        #[arg(long)]
        all: bool,
        /// Confirm without asking
        #[arg(long)]
        yes: bool,
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
        Command::Quarantine { command } => quarantine_command(&home, command),
        Command::Review { command } => review_command(&home, command),
        Command::Custody { json, markdown } => custody(&home, json, markdown),
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
    let cancel = stop_on_signal()?;
    let result = if settings.dry_run {
        nils_digest::digest_with(&settings, None, &cancel)
    } else {
        let mut registry = open(home)?;
        nils_digest::digest_with(&settings, Some(&mut registry), &cancel)
    };
    match result {
        Ok(report) => {
            print_report(&report, settings.json)?;
            match report.cancelled {
                None => Ok(()),
                Some(Cancelled::Stopped) => Err(Exit {
                    code: STOPPED,
                    message: "stopped: what was read is written; run again to go on".into(),
                }),
                Some(Cancelled::Aborted) => Err(Exit {
                    code: STOPPED,
                    message: "aborted: the batch in flight was abandoned; run again to go on"
                        .into(),
                }),
            }
        }
        Err(e @ DigestError::Busy { .. }) => Err(Exit {
            code: BUSY,
            message: e.to_string(),
        }),
        Err(e) => Err(fail(e.to_string())),
    }
}

/// The token a run is asked to stop through (§10): one signal asks for a
/// stop, a second for an abort. SIGINT and SIGTERM both count.
fn stop_on_signal() -> Result<Cancel, Exit> {
    let cancel = Cancel::new();
    let token = cancel.clone();
    let mut asked = 0u8;
    ctrlc::set_handler(move || {
        token.request();
        asked = asked.saturating_add(1);
        match asked {
            1 => eprintln!(
                "nils: stopping: what is read is written, then the job ends \
                 (a second signal abandons the batch in flight)"
            ),
            2 => eprintln!("nils: aborting: the batch in flight is abandoned"),
            _ => {}
        }
    })
    .map_err(|e| fail(format!("cannot handle signals: {e}")))?;
    Ok(cancel)
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

    // what ran besides digests (a digest is its batch below): purges so far
    let others_sql = format!(
        "SELECT id, kind, name, state, host, {}, {}, {} FROM {} WHERE kind <> 'digest' AND state <> 'running' ORDER BY id DESC LIMIT 10",
        text_of(store, "job", "started_at"),
        text_of(store, "job", "finished_at"),
        text_of(store, "job", "args"),
        store.qualified("job")
    );
    let others: Vec<serde_json::Value> = store
        .query(&others_sql, &[])?
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "id": r.int(0)?,
                "kind": r.text(1)?,
                "name": r.opt_text(2)?,
                "state": r.text(3)?,
                "host": r.opt_text(4)?,
                "started_at": r.opt_text(5)?,
                "finished_at": r.opt_text(6)?,
                "args": r.opt_text(7)?.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()),
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
        "other_jobs": others,
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
    match meta.pseudonym_scheme {
        // v0's code is the whole digest in hex; no display length applies
        Scheme::Blake2b8 => println!(
            "  pseudonyms       {} from key {}, 16 hex characters",
            meta.pseudonym_scheme, meta.pseudonym_key
        ),
        Scheme::Blake2b32 => println!(
            "  pseudonyms       {} from key {}, {} characters shown",
            meta.pseudonym_scheme, meta.pseudonym_key, meta.display_length
        ),
    }
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
    if !others.is_empty() {
        println!("other jobs (last {})", others.len());
    }
    for j in &others {
        println!(
            "  job {}   {} {}   {} on {}   finished {}   {}",
            j["id"],
            s(&j["kind"]),
            s(&j["name"]),
            s(&j["state"]),
            s(&j["host"]),
            s(&j["finished_at"]),
            j["args"]
        );
    }
    println!("batches (last {})", batches.len());
    if batches.is_empty() {
        println!("  none");
    } else {
        println!(
            "  {:>5}  {:<9} {:<28} {:<21} {:<21} {:>6} {:>10} {:>10} {:>10}",
            "id", "state", "name", "started", "finished", "epoch", "seen", "parsed", "ingested"
        );
    }
    for b in &batches {
        println!(
            "  {:>5}  {:<9} {:<28} {:<21} {:<21} {:>6} {:>10} {:>10} {:>10}",
            n_i64(&b["id"]),
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
        LinkageCommand::Purge { subject, all, yes } => {
            purge(&mut registry, &mut store, subject.as_deref(), all, yes)
        }
    }
}

/// `nils linkage purge`: what it would delete is said first, and nothing is
/// deleted without a yes, on the command line or at a terminal. The purge
/// is recorded as a job row, so that `custody` and the record agree. The
/// registry's subjects stay, and so does the read audit; a purged identifier
/// is filed again only when its file is parsed again (changed, or new), never
/// by a digest that finds the file unchanged.
fn purge(
    registry: &mut Registry,
    store: &mut Store,
    subject: Option<&str>,
    all: bool,
    yes: bool,
) -> Result<(), Exit> {
    let (target, id, what) = match (subject, all) {
        (Some(code), false) => {
            let id = subject_of(registry, code)?;
            let identities = linkage::identities_of_subjects(store, &[id])?.len();
            let linkages = linkage::linkages_of(store, id)?.len();
            (
                format!("subject {code}"),
                Some(id),
                format!(
                    "{identities} identifier(s) and {linkages} linkage(s), open or reversed, of subject {code}"
                ),
            )
        }
        (None, true) => {
            let h = linkage::holdings(store)?;
            (
                "every subject".to_string(),
                None,
                format!(
                    "{} identifier(s) of {} subject(s) and {} open linkage(s)",
                    h.identities, h.subjects, h.linkages
                ),
            )
        }
        _ => return Err(usage("purge takes --subject <code> or --all")),
    };
    if !yes
        && !confirm(&format!(
            "purge {what}? The read audit stays. Type yes to go on: "
        ))?
    {
        println!("nothing purged");
        return Ok(());
    }
    let purged = linkage::purge(store, id)?;
    let now = nils_registry::time::now_iso();
    let args = serde_json::json!({
        "subject": subject,
        "all": all,
        "actor": actor(),
        "identities": purged.identities,
        "linkages": purged.linkages,
    });
    let host = nils_digest::digest::hostname();
    registry.store().insert(
        &Insert::new(
            table("job"),
            &[
                "kind",
                "name",
                "args",
                "state",
                "pid",
                "host",
                "started_at",
                "heartbeat_at",
                "finished_at",
            ],
        ),
        &[vec![
            Param::from("linkage-purge"),
            Param::from(target.as_str()),
            Param::from(args.to_string()),
            Param::from("done"),
            Param::from(i64::from(std::process::id())),
            Param::from(host.as_str()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
            Param::from(now.as_str()),
        ]],
    )?;
    println!(
        "purged {} identifier(s) and {} linkage(s) of {target}; the read audit and the registry's subjects stay, and a file parsed again files its identifier again (an unchanged file does not)",
        purged.identities, purged.linkages
    );
    Ok(())
}

/// A yes at a terminal; without one, a refusal that names `--yes`.
fn confirm(prompt: &str) -> Result<bool, Exit> {
    use std::io::{IsTerminal, Write as _};
    if !io::stdin().is_terminal() {
        return Err(usage("not at a terminal: add --yes to confirm"));
    }
    print!("{prompt}");
    io::stdout().flush().ok();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| fail(format!("cannot read the answer: {e}")))?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}

/// `nils quarantine list`: the paths a digest refused, with the class and
/// the detail, joined to the root of their source.
fn quarantine_command(home: &Home, command: QuarantineCommand) -> Result<(), Exit> {
    let QuarantineCommand::List { batch, class, json } = command;
    let mut registry = open(home)?;
    let store = registry.store();
    let d = store.dialect();
    let mut sql = format!(
        "SELECT f.batch_id, f.reason, s.root, f.path, f.detail, {} FROM {} AS f JOIN {} AS s ON s.id = f.source_id WHERE f.status = 'quarantined'",
        text_of(store, "source_file", "seen_at").replace("seen_at", "f.seen_at"),
        store.qualified("source_file"),
        store.qualified("source")
    );
    let mut params = Vec::new();
    if let Some(id) = batch {
        params.push(Param::Int(id));
        sql.push_str(&format!(
            " AND f.batch_id = {}",
            d.param(params.len(), Type::Int)
        ));
    }
    if let Some(c) = &class {
        params.push(Param::from(c.as_str()));
        sql.push_str(&format!(
            " AND f.reason = {}",
            d.param(params.len(), Type::Text)
        ));
    }
    sql.push_str(" ORDER BY f.batch_id, f.path");
    let files: Vec<serde_json::Value> = store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            let path = PathBuf::from(r.text(2)?).join(r.text(3)?);
            Ok(serde_json::json!({
                "batch_id": r.opt_int(0)?,
                "class": r.opt_text(1)?.unwrap_or("-"),
                "path": path.display().to_string(),
                "detail": r.opt_text(4)?,
                "seen_at": r.opt_text(5)?,
            }))
        })
        .collect::<Result<_, nils_registry::Error>>()?;
    if json {
        let doc = serde_json::json!({ "count": files.len(), "files": files });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    println!(
        "nils quarantine   registry {}   {} file(s){}{}",
        home.dir().display(),
        files.len(),
        batch.map(|b| format!("   batch {b}")).unwrap_or_default(),
        class.map(|c| format!("   class {c}")).unwrap_or_default()
    );
    if !files.is_empty() {
        println!("  {:>5}  {:<14} path", "batch", "class");
    }
    for f in &files {
        let detail = match f["detail"].as_str() {
            Some(d) if !d.is_empty() => format!("   ({d})"),
            _ => String::new(),
        };
        println!(
            "  {:>5}  {:<14} {}{detail}",
            n_i64(&f["batch_id"]),
            s(&f["class"]),
            s(&f["path"])
        );
    }
    Ok(())
}

/// `nils review list | show`: the rows of `review_item`.
fn review_command(home: &Home, command: ReviewCommand) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let d = store.dialect();
    let columns = format!(
        "id, kind, scope, status, actor, {}, {}, {}, {}, {}",
        text_of(store, "review_item", "created_at"),
        text_of(store, "review_item", "decided_at"),
        text_of(store, "review_item", "ref"),
        text_of(store, "review_item", "evidence"),
        text_of(store, "review_item", "decision"),
    );
    let item_of = |r: &nils_registry::Row| -> Result<serde_json::Value, nils_registry::Error> {
        let json = |i: usize| -> Result<serde_json::Value, nils_registry::Error> {
            Ok(r.opt_text(i)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(serde_json::Value::Null))
        };
        Ok(serde_json::json!({
            "id": r.int(0)?,
            "kind": r.text(1)?,
            "scope": r.text(2)?,
            "status": r.text(3)?,
            "actor": r.opt_text(4)?,
            "created_at": r.opt_text(5)?,
            "decided_at": r.opt_text(6)?,
            "ref": json(7)?,
            "evidence": json(8)?,
            "decision": json(9)?,
        }))
    };
    match command {
        ReviewCommand::List { kind, status, json } => {
            let mut sql = format!(
                "SELECT {columns} FROM {} WHERE 1 = 1",
                store.qualified("review_item")
            );
            let mut params = Vec::new();
            if let Some(k) = &kind {
                params.push(Param::from(k.as_str()));
                sql.push_str(&format!(
                    " AND kind = {}",
                    d.param(params.len(), Type::Text)
                ));
            }
            if let Some(st) = &status {
                params.push(Param::from(st.as_str()));
                sql.push_str(&format!(
                    " AND status = {}",
                    d.param(params.len(), Type::Text)
                ));
            }
            sql.push_str(" ORDER BY id");
            let items: Vec<serde_json::Value> = store
                .query(&sql, &params)?
                .iter()
                .map(item_of)
                .collect::<Result<_, nils_registry::Error>>()?;
            if json {
                let doc = serde_json::json!({ "count": items.len(), "items": items });
                println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
                return Ok(());
            }
            println!(
                "nils review   registry {}   {} item(s){}{}",
                home.dir().display(),
                items.len(),
                kind.map(|k| format!("   kind {k}")).unwrap_or_default(),
                status
                    .map(|st| format!("   status {st}"))
                    .unwrap_or_default()
            );
            if !items.is_empty() {
                println!(
                    "  {:>4}  {:<20} {:<10} {:<8} {:<21} about",
                    "id", "kind", "status", "scope", "created"
                );
            }
            for it in &items {
                println!(
                    "  {:>4}  {:<20} {:<10} {:<8} {:<21} {}",
                    n_i64(&it["id"]),
                    s(&it["kind"]),
                    s(&it["status"]),
                    s(&it["scope"]),
                    s(&it["created_at"]),
                    about(it)
                );
            }
            Ok(())
        }
        ReviewCommand::Show { id, json } => {
            let sql = format!(
                "SELECT {columns} FROM {} WHERE id = {}",
                store.qualified("review_item"),
                d.param(1, Type::Int)
            );
            let row = store
                .query_opt(&sql, &[Param::Int(id)])?
                .ok_or_else(|| fail(format!("no review item {id}")))?;
            let item = item_of(&row)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&item).unwrap_or_default()
                );
                return Ok(());
            }
            println!(
                "review item {id}   {}   {}",
                s(&item["kind"]),
                s(&item["status"])
            );
            println!("  scope      {}", s(&item["scope"]));
            println!("  about      {}", about(&item));
            println!("  created    {}", s(&item["created_at"]));
            println!("  ref        {}", item["ref"]);
            println!("  evidence   {}", item["evidence"]);
            if !item["decided_at"].is_null() {
                println!(
                    "  decided    {} by {}   {}",
                    s(&item["decided_at"]),
                    s(&item["actor"]),
                    item["decision"]
                );
            }
            Ok(())
        }
    }
}

/// One line on what a review item is about, from its `ref` and evidence:
/// counts and codes, never an identifier.
fn about(item: &serde_json::Value) -> String {
    let r = &item["ref"];
    let e = &item["evidence"];
    match s(&item["kind"]) {
        "ingest.quarantine" => format!(
            "batch {}, class {}, {} file(s)",
            r["batch_id"],
            s(&r["class"]),
            e["count"]
        ),
        "identity.collision" => format!(
            "subject {} under {} ({}), batch {}",
            s(&r["code"]),
            s(&e["id_type"]),
            s(&e["reason"]),
            e["batch_id"]
        ),
        _ => r.to_string(),
    }
}

/// `nils custody`: every store the registry keeps (C38), as the same table
/// the documentation carries, with this home's paths and counts.
fn custody(home: &Home, json: bool, markdown: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let config = registry.config().clone();
    let meta = registry.meta().clone();
    let keys = registry.keys();
    let key_list = keys.list()?;
    let count = |store: &mut Store, sql: &str| -> Result<i64, Exit> {
        Ok(store.query(sql, &[])?[0].int(0)?)
    };
    let store = registry.store();
    let count_of = |store: &mut Store, t: &str, filter: &str| -> Result<i64, Exit> {
        let sql = format!("SELECT COUNT(*) FROM {}{filter}", store.qualified(t));
        count(store, &sql)
    };
    let subjects = count_of(store, "subject", "")?;
    let studies = count_of(store, "study", "")?;
    let series = count_of(store, "series", "")?;
    let instances = count_of(store, "instance", "")?;
    let source_files = count_of(store, "source_file", "")?;
    let quarantined = count_of(store, "source_file", " WHERE status = 'quarantined'")?;
    let classes_sql = format!(
        "SELECT COUNT(DISTINCT reason) FROM {} WHERE status = 'quarantined'",
        store.qualified("source_file")
    );
    let classes = count(store, &classes_sql)?;
    let open_items = count_of(store, "review_item", " WHERE status = 'open'")?;
    let jobs = count_of(store, "job", "")?;
    let batches = count_of(store, "ingest_batch", "")?;
    let schema = store.schema().map(str::to_string);
    let linkage_holdings = linkage::holdings(&mut registry.open_linkage()?)?;

    let dir = home.dir();
    let sqlite = config.backend == Backend::Sqlite;
    let db_files = |name: &str| -> Vec<serde_json::Value> {
        [
            String::new(),
            "-wal".into(),
            "-shm".into(),
            "-journal".into(),
        ]
        .iter()
        .map(|suffix| dir.join(format!("{name}{suffix}")))
        .filter(|p| p.exists())
        .map(|p| file_entry(&p))
        .collect()
    };
    let dsn_note = match config.backend {
        Backend::Sqlite => None,
        Backend::Postgres => Some(match std::env::var(DSN_ENV) {
            Ok(v) if !v.is_empty() => format!("{} (from {DSN_ENV})", redact_dsn(&v)),
            _ => match &config.dsn {
                Some(v) => format!("{} (from nils.toml)", redact_dsn(v)),
                None => "no dsn set".to_string(),
            },
        }),
    };
    let where_db = |file: &str, schema: &str| -> String {
        if sqlite {
            format!(
                "{}, mode 600 (SQLite keeps {file}-wal and {file}-shm beside it while a connection is open)",
                dir.join(file).display()
            )
        } else {
            format!(
                "postgres schema {schema} in the database of {}",
                dsn_note.as_deref().unwrap_or("-")
            )
        }
    };
    let delete_db = |file: &str, schema: &str| -> String {
        if sqlite {
            format!(
                "remove {} (nils has no command for it)",
                dir.join(file).display()
            )
        } else {
            format!("DROP SCHEMA {schema} CASCADE (nils has no command for it)")
        }
    };
    let linkage_schema = config.linkage_schema();
    let registry_schema = schema.clone().unwrap_or_else(|| config.schema.clone());

    let stores = vec![
        serde_json::json!({
            "store": "configuration",
            "what": "nils.toml: the backend, the Postgres dsn if written there, the schema, the key store path",
            "where": home.config_path().display().to_string(),
            "files": [file_entry(&home.config_path())],
            "holds": ["technical", "secret: a password in the dsn, when one is written there instead of NILS_DSN"],
            "counts": {},
            "kept": "until removed",
            "commands": { "read": ["nils status"], "change": ["edit the file"], "export": [], "delete": "remove the file" },
        }),
        serde_json::json!({
            "store": "registry",
            "what": "the pseudonymous catalogue: subjects, studies, series, stacks, instances, source files, diagnostics, review items, jobs and batches",
            "where": where_db(REGISTRY_DB, &registry_schema),
            "files": if sqlite { db_files(REGISTRY_DB) } else { Vec::new() },
            "holds": [
                "quasi-identifying: birth dates, sex, study dates and times, station and institution names, descriptions and comments, source paths",
                "technical: everything else the catalogue declares",
            ],
            "counts": { "subjects": subjects, "studies": studies, "series": series, "instances": instances, "source_files": source_files },
            "kept": "until deleted; nothing expires on its own, and a run marks files that vanished as gone instead of deleting their rows",
            "commands": {
                "read": ["nils status [--batch <id>]", "nils quarantine list", "nils review list"],
                "change": ["nils digest <root>"],
                "export": ["none in Wave 1: the file (or the schema) is the export"],
                "delete": delete_db(REGISTRY_DB, &registry_schema),
            },
        }),
        serde_json::json!({
            "store": "linkage store",
            "what": "the identifiers behind the codes, encrypted under the registry's key; the linkages between subjects; the audit of every read",
            "where": where_db(LINKAGE_DB, &linkage_schema),
            "files": if sqlite { db_files(LINKAGE_DB) } else { Vec::new() },
            "holds": [
                "identifying: the identifiers (encrypted) and their keyed lookups",
                "technical: the linkages, the id types, the read audit (actor, time, why, identity id)",
            ],
            "counts": {
                "identities": linkage_holdings.identities,
                "subjects": linkage_holdings.subjects,
                "open_linkages": linkage_holdings.linkages,
                "audited_reads": linkage_holdings.reads,
            },
            "kept": "until purged; a purged identifier is filed again only when its file is parsed again (changed, or new), not by a digest that finds the file unchanged",
            "commands": {
                "read": ["nils linkage show <code> [--why <text>] (every read is audited)"],
                "change": ["nils digest <root>", "nils linkage import <csv>", "nils linkage link | unlink", "nils linkage id-type add"],
                "export": ["none in Wave 1"],
                "delete": "nils linkage purge --subject <code> | --all (the read audit and the id types stay)",
            },
        }),
        serde_json::json!({
            "store": "key store",
            "what": format!("the pseudonym key ({} for this registry) and any other key added", meta.pseudonym_key),
            "where": format!("{}, mode 700, one file per key, mode 600", keys.dir().display()),
            "files": key_files(&keys, &key_list),
            "holds": ["secret: the key bytes; whoever holds the registry's key can derive its codes and read its linkage store"],
            "counts": { "keys": key_list.len(), "in_use": meta.pseudonym_key },
            "kept": "until removed; the key the registry names cannot be removed while it names it",
            "commands": {
                "read": ["nils key list (names, lengths and fingerprints, never the bytes)"],
                "change": ["nils key add <name>"],
                "export": ["copy the file (that is the backup the key needs)"],
                "delete": "nils key remove <name>",
            },
        }),
        serde_json::json!({
            "store": "quarantine list",
            "what": "the files a digest refused, each with its class and detail, and one review item per batch and class",
            "where": "rows of source_file (status quarantined) and review_item (kind ingest.quarantine) in the registry",
            "files": [],
            "holds": ["quasi-identifying: the file paths", "technical: the class, the detail, the counts"],
            "counts": { "files": quarantined, "classes": classes, "open_review_items": open_items },
            "kept": "a file's row until the file changes or a run reads it again with --retry-quarantine; the review items until decided (review apply is Wave 4's)",
            "commands": {
                "read": ["nils quarantine list [--batch <id>] [--class <c>]", "nils review list [--kind ingest.quarantine]", "nils review show <id>"],
                "change": ["nils digest <root> --retry-quarantine"],
                "export": ["nils quarantine list --json"],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "job records",
            "what": "every run and purge: its arguments, host and pid, progress, counts and outcome",
            "where": "rows of job and ingest_batch in the registry",
            "files": [],
            "holds": ["quasi-identifying: the root path in a run's arguments", "technical: the counts, the host name, the pid, the times, the outcome"],
            "counts": { "jobs": jobs, "batches": batches },
            "kept": "until deleted with the registry",
            "commands": {
                "read": ["nils status [--batch <id>]"],
                "change": [],
                "export": ["nils status --json", "nils status --batch <id> --json"],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "logs",
            "what": "none: progress is printed to stderr and not stored; the counts of a run are its batch record",
            "where": "nowhere",
            "files": [],
            "holds": [],
            "counts": {},
            "kept": "not kept",
            "commands": { "read": [], "change": [], "export": [], "delete": "nothing to delete" },
        }),
    ];

    let doc = serde_json::json!({
        "home": dir.display().to_string(),
        "backend": config.backend.name(),
        "registry_id": meta.registry_id,
        "stores": stores,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    if markdown {
        print!("{}", custody_markdown(&doc));
        return Ok(());
    }
    println!(
        "nils custody   registry {}   backend {}",
        dir.display(),
        config.backend.name()
    );
    println!("every store this registry keeps; nothing is retained that is not listed here");
    for st in doc["stores"].as_array().unwrap() {
        println!();
        println!("{}", s(&st["store"]));
        println!("  what      {}", s(&st["what"]));
        let files = st["files"].as_array().unwrap();
        if files.is_empty() {
            println!("  where     {}", s(&st["where"]));
        }
        for (i, f) in files.iter().enumerate() {
            let label = if i == 0 { "where" } else { "" };
            let size = if f["kind"] == "directory" {
                "directory".to_string()
            } else {
                format!(
                    "{} bytes",
                    f["bytes"]
                        .as_u64()
                        .map(nils_digest::report::thousands)
                        .unwrap_or_default()
                )
            };
            let mode = f["mode"]
                .as_str()
                .map(|m| format!(", mode {m}"))
                .unwrap_or_default();
            println!("  {label:<9} {}   {size}{mode}", s(&f["path"]));
        }
        for h in st["holds"].as_array().unwrap() {
            println!("  holds     {}", s(h));
        }
        let counts = st["counts"].as_object().unwrap();
        if !counts.is_empty() {
            let parts: Vec<String> = counts
                .iter()
                .map(|(k, v)| match v.as_u64() {
                    Some(n) => format!("{} {}", nils_digest::report::thousands(n), counted(k, n)),
                    None => format!("{} {}", k.replace('_', " "), s(v)),
                })
                .collect();
            println!("  now       {}", parts.join(", "));
        }
        println!("  kept      {}", s(&st["kept"]));
        let c = &st["commands"];
        for (label, key) in [("read", "read"), ("change", "change"), ("export", "export")] {
            let list = c[key].as_array().unwrap();
            if !list.is_empty() {
                println!(
                    "  {label:<9} {}",
                    list.iter().map(s).collect::<Vec<_>>().join("; ")
                );
            }
        }
        println!("  delete    {}", s(&c["delete"]));
    }
    Ok(())
}

/// The custody table as a Markdown page: the record a deployment keeps
/// (C38), without the live counts and sizes that would date it by the hour.
fn custody_markdown(doc: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut page = String::new();
    let _ = writeln!(page, "# Custody\n");
    let _ = writeln!(
        page,
        "Every store the registry at `{}` keeps (backend {}), rendered by `nils custody --markdown`: \
         where it lives, which classes of data it holds (§4.3 of the Wave 1 specification), how long \
         it is kept, and the command that reads, changes, exports or deletes it. Every command named \
         here exists, and nothing is retained that this page does not show. `nils custody` prints the \
         same table with the files and counts of the moment; `--json` is the machine-readable form.\n",
        s(&doc["home"]),
        s(&doc["backend"])
    );
    let home = s(&doc["home"]);
    // a table cell: commands and paths in code font, the column bar escaped
    let cell = |text: &str| -> String {
        let mut out = match text.strip_prefix("nils ") {
            Some(_) => match text.split_once(" (") {
                Some((cmd, note)) => format!("`{cmd}` ({note}"),
                None => format!("`{text}`"),
            },
            None => text.to_string(),
        };
        if !home.is_empty() && out.contains(home) && !out.starts_with('`') {
            let mut fenced = String::new();
            let mut rest = out.as_str();
            while let Some(at) = rest.find(home) {
                fenced.push_str(&rest[..at]);
                let path = &rest[at..];
                let end = path.find([',', ' ', ')']).unwrap_or(path.len());
                fenced.push('`');
                fenced.push_str(&path[..end]);
                fenced.push('`');
                rest = &path[end..];
            }
            fenced.push_str(rest);
            out = fenced;
        }
        out.replace('|', "\\|")
    };
    for st in doc["stores"].as_array().unwrap() {
        let _ = writeln!(page, "## {}\n", s(&st["store"]));
        let _ = writeln!(page, "| | |\n|---|---|");
        let _ = writeln!(page, "| what | {} |", cell(s(&st["what"])));
        let _ = writeln!(page, "| where | {} |", cell(s(&st["where"])));
        let holds: Vec<String> = st["holds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| cell(s(v)))
            .collect();
        let _ = writeln!(
            page,
            "| holds | {} |",
            if holds.is_empty() {
                "nothing".to_string()
            } else {
                holds.join("<br>")
            }
        );
        let _ = writeln!(page, "| kept | {} |", cell(s(&st["kept"])));
        let c = &st["commands"];
        for key in ["read", "change", "export"] {
            let list: Vec<String> = c[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| cell(s(v)))
                .collect();
            let _ = writeln!(
                page,
                "| {key} | {} |",
                if list.is_empty() {
                    "no command".to_string()
                } else {
                    list.join("<br>")
                }
            );
        }
        let _ = writeln!(page, "| delete | {} |\n", cell(s(&c["delete"])));
    }
    page
}

/// A count's noun, singular when the count is one: `source_files` is one
/// source file, `studies` one study, `series` stays.
fn counted(key: &str, n: u64) -> String {
    let words = key.replace('_', " ");
    if n == 1 {
        let (head, last) = match words.rsplit_once(' ') {
            Some((h, l)) => (format!("{h} "), l),
            None => (String::new(), words.as_str()),
        };
        let one = if last == "series" {
            last.to_string()
        } else if let Some(stem) = last.strip_suffix("ies") {
            format!("{stem}y")
        } else if last.ends_with("ches") || last.ends_with("sses") {
            last[..last.len() - 2].to_string()
        } else {
            last.strip_suffix('s').unwrap_or(last).to_string()
        };
        return format!("{head}{one}");
    }
    words
}

/// One file of a store: its path, size and mode.
fn file_entry(path: &std::path::Path) -> serde_json::Value {
    let meta = fs::metadata(path).ok();
    let mode = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.as_ref()
                .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
        }
        #[cfg(not(unix))]
        {
            None::<String>
        }
    };
    let directory = meta.as_ref().is_some_and(|m| m.is_dir());
    serde_json::json!({
        "path": path.display().to_string(),
        "kind": if directory { "directory" } else { "file" },
        "bytes": if directory { None } else { meta.as_ref().map(|m| m.len()) },
        "mode": mode,
    })
}

/// The key store's files: one per key.
fn key_files(
    keys: &nils_registry::KeyStore,
    list: &[nils_registry::keys::KeyInfo],
) -> Vec<serde_json::Value> {
    let mut files = vec![file_entry(keys.dir())];
    files.extend(list.iter().map(|k| file_entry(&keys.dir().join(&k.name))));
    files
}

/// A connection string with its password replaced.
fn redact_dsn(dsn: &str) -> String {
    let Some(scheme_end) = dsn.find("://") else {
        return "<dsn>".to_string();
    };
    let rest = &dsn[scheme_end + 3..];
    let Some(at) = rest.rfind('@') else {
        return dsn.to_string();
    };
    let userinfo = &rest[..at];
    let user = userinfo.split(':').next().unwrap_or("");
    let redacted = if userinfo.contains(':') {
        format!("{user}:***")
    } else {
        user.to_string()
    };
    format!("{}://{redacted}{}", &dsn[..scheme_end], &rest[at..])
}

fn n_i64(v: &serde_json::Value) -> String {
    match v.as_i64() {
        Some(n) => n.to_string(),
        None => "-".to_string(),
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
                "imported {} row(s) as {}: {} subject(s) created, {} identifier(s) filed, {} already filed{}",
                report.rows,
                args.id_type,
                report.subjects_created,
                report.identities_added,
                report.unchanged,
                match report.second_identifiers {
                    0 => String::new(),
                    n => format!("; {n} of them a further identifier of a subject that had one"),
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_names_its_noun() {
        assert_eq!(counted("subjects", 1), "subject");
        assert_eq!(counted("subjects", 2), "subjects");
        assert_eq!(counted("studies", 1), "study");
        assert_eq!(counted("series", 1), "series");
        assert_eq!(counted("batches", 1), "batch");
        assert_eq!(counted("classes", 1), "class");
        assert_eq!(counted("source_files", 1), "source file");
        assert_eq!(counted("open_review_items", 1), "open review item");
        assert_eq!(counted("open_review_items", 0), "open review items");
        assert_eq!(counted("identities", 1), "identity");
    }

    #[test]
    fn a_dsn_loses_its_password_and_nothing_else() {
        assert_eq!(
            redact_dsn("postgres://nils:s3cret@localhost:5432/nils_test"),
            "postgres://nils:***@localhost:5432/nils_test"
        );
        assert_eq!(
            redact_dsn("postgres://nils@db.example/nils"),
            "postgres://nils@db.example/nils"
        );
        assert_eq!(
            redact_dsn("postgresql://u:p%40ss@h/d?sslmode=require"),
            "postgresql://u:***@h/d?sslmode=require"
        );
        assert_eq!(redact_dsn("host=db user=nils password=x"), "<dsn>");
        assert_eq!(
            redact_dsn("postgres://localhost/nils"),
            "postgres://localhost/nils"
        );
    }
}
