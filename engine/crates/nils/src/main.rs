// SPDX-License-Identifier: AGPL-3.0-only

//! `nils`, the binary: the command line, configuration, `custody` and the output
//! formatting (`docs/specs/wave1-parse-and-digest.md`, §3 and §13).
//!
//! Slice 2 of the build has `digest --dry-run` and `digest --describe`; the
//! writer, `init`, `key`, `status` and `custody` arrive with the slices that
//! give them something to do (§14).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use nils_digest::{Filter, Settings};

/// NILS digests DICOM into a registry: one binary, on a laptop or a server.
#[derive(Debug, Parser)]
#[command(name = "nils", version, about, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Walk a tree of DICOM files, read every header and digest it into the registry
    Digest(DigestArgs),
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
    /// Which file names are candidates: all, dcm, no-ext, or a glob
    #[arg(long, default_value = "all", value_name = "all|dcm|no-ext|<glob>")]
    files: String,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Digest(args) => digest(args),
    }
}

fn digest(args: DigestArgs) -> ExitCode {
    let mut settings = Settings::new(args.root);
    settings.dry_run = args.dry_run;
    settings.json = args.json;
    if let Some(name) = args.name {
        settings.name = name;
    }
    if let Some(workers) = args.workers {
        if workers == 0 {
            eprintln!("nils digest: --workers must be at least 1");
            return ExitCode::from(2);
        }
        settings.workers = workers;
    }
    settings.filter = match Filter::parse(&args.files) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("nils digest: --files {}: {e}", args.files);
            return ExitCode::from(2);
        }
    };

    if args.describe {
        print!("{}", settings.describe());
        return ExitCode::SUCCESS;
    }
    if !settings.dry_run {
        eprintln!("nils digest: the writer lands in slice 3; run with --dry-run");
        return ExitCode::from(2);
    }
    match nils_digest::dry_run(&settings) {
        Ok(report) => {
            if settings.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("nils digest: cannot render the report: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                print!("{report}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nils digest: {e}");
            ExitCode::FAILURE
        }
    }
}
