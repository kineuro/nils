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

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

mod serve;
use nils_digest::{Cancel, Cancelled, DigestError, Filter, Report, Rule, Settings};
use nils_registry::day::Day;
use nils_registry::home::{
    Config, DSN_ENV, Home, InitOptions, LINKAGE_DB, REGISTRY_DB, REGISTRY_ENV,
};
use nils_registry::keys::strip_newline;
use nils_registry::linkage::{self, ImportError, ImportRow, Subkeys};
use nils_registry::schema::{Type, table};
use nils_registry::session;
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
    /// Derive the per-stack values a classifier reads, once, and store them
    Fingerprint(FingerprintArgs),
    /// Judge every stack with a pack, and write the verdict with its evidence
    Classify(ClassifyArgs),
    /// Why one stack was judged the way it was: the axes, the evidence, the decisions
    Explain {
        /// The stack's id
        stack: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Classification packs: what is installed, what one says, whether it loads
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },
    /// The registry: its metadata, the running jobs, the last batches
    Status(StatusArgs),
    /// The jobs: every verb that runs longer than a second is one, with a
    /// heartbeat and its progress; list, show, cancel, resume, queue and
    /// work them (Wave 4a section 9.1)
    #[command(subcommand)]
    Jobs(JobsCommand),
    /// The audit log: who did what, to which scope, when, under which
    /// policy (Wave 4a section 9.2)
    #[command(subcommand)]
    Audit(AuditCommand),
    /// The one door: the HTTP API over this registry, one route per
    /// operation, jobs for anything heavy (Wave 4a section 11)
    Serve(ServeArgs),
    /// What private elements an archive carries, by creator, so an allowlist
    /// is chosen from the data rather than from a chair (§8.4)
    Private(PrivateArgs),
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
    /// What a selection would release, without releasing it: each item and
    /// how it resolved, and what it reaches (Wave 4a section 8)
    Select(SelectArgs),
    /// Select, de-identify and write a dataset out, recording the policy it applied (§8)
    Release(Box<ReleaseArgs>),
    /// How a dataset physically leaves: encrypted archives, checksummed and
    /// recorded against the release (§11)
    #[command(subcommand)]
    Handover(HandoverCommand),
    /// The clinical layer: the vocabulary of diseases and observation kinds (Wave 4a section 7.1)
    #[command(subcommand)]
    Clinical(ClinicalCommand),
    /// Which stack stands for each session's role, with the evidence that chose it (§10)
    Pick {
        #[command(subcommand)]
        command: PickCommand,
    },
    /// The occasions a subject came in, derived from a scheme and never stored (§5)
    Session {
        #[command(subcommand)]
        command: SessionCommand,
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

#[derive(Debug, Args)]
struct ReleaseArgs {
    /// Where the tree is written
    #[arg(long, value_name = "DIR", required_unless_present_any = ["history", "withdraw"])]
    out: Option<PathBuf>,
    /// What every version of this dataset did, and write nothing (§8.6)
    #[arg(long)]
    history: bool,
    /// What to call this release, on its row and in its report
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// What happens to every date: as they are, moved by one offset per
    /// subject, or the year only
    #[arg(long, default_value = "keep", value_name = "keep|shift|year")]
    dates: String,
    /// What happens to UIDs. Remapping is keyed and deterministic, so two
    /// releases of overlapping selections agree
    #[arg(long, default_value = "remap", value_name = "remap|preserve")]
    uids: String,
    /// The arc new UIDs hang from. The default is DICOM's UUID arc, which is
    /// legal and needs no registration
    #[arg(long, value_name = "OID")]
    uid_root: Option<String>,
    /// Which categories of element to remove; all of them by default
    #[arg(long, value_name = "patient,trial,provider,institution,times")]
    categories: Option<String>,
    /// What to do with a stack whose file says nothing about text in its
    /// pixels. Holding is the default, because a release is a thing that leaves
    #[arg(long = "on-unknown", default_value = "hold", value_name = "hold|write")]
    on_unknown: String,
    /// Only these subjects, by code
    #[arg(long, value_name = "CODE")]
    subject: Vec<String>,
    /// Only these sessions, as `<subject>:<label>`, by the label the scheme
    /// gives them
    #[arg(long, value_name = "CODE:LABEL")]
    session: Vec<String>,
    /// Only these stacks, by id. The grain a query returns
    #[arg(long, value_name = "ID")]
    stack: Vec<i64>,
    /// Every current member of this cohort, by name (Wave 4a section 8)
    #[arg(long, value_name = "NAME")]
    cohort: Vec<String>,
    /// Only stacks holding this value on this pack axis, as `<axis>=<value>`;
    /// several values of one axis are alternatives, several axes all hold
    #[arg(long, value_name = "AXIS=VALUE")]
    axis: Vec<String>,
    /// Read the selection from a file, or from `-` for standard input, so the
    /// answer to a query can be piped in rather than typed. JSON with
    /// `cohorts`, `subjects`, `sessions`, `stacks` and `axes`, or one item a
    /// line: a number is a stack, `@name` a cohort, `<axis>=<value>` an axis,
    /// `<subject>:<label>` a session, anything else a subject by code or by
    /// any identifier the registry resolves
    #[arg(long, value_name = "FILE")]
    select: Option<String>,
    /// Only stacks of these dispositions; by default everything but excluded
    #[arg(long, value_name = "KIND")]
    disposition: Vec<String>,
    /// Only stacks holding one of these roles
    #[arg(long, value_name = "ROLE")]
    role: Vec<String>,
    /// Only the stacks a pick chose
    #[arg(long)]
    picked: bool,
    /// Only stacks of this modality
    #[arg(long, value_name = "MR|CT|PT|...")]
    modality: Option<String>,
    /// The session scheme the tree's `ses-` directories come from
    #[arg(long, value_name = "FILE", conflicts_with = "scheme_name")]
    scheme: Option<PathBuf>,
    /// A scheme stored in this registry, by name
    #[arg(long = "scheme-name", value_name = "NAME")]
    scheme_name: Option<String>,
    /// Which layout to write: v0's grammar, which names everything, or BIDS,
    /// which names what the standard admits and routes the rest (§9)
    #[arg(long, default_value = "descriptive", value_name = "descriptive|bids")]
    layout: String,
    /// Where a localizer goes in a BIDS tree. BIDS has no word for one, and
    /// 22 percent of a clinical archive is one (§9.3)
    #[arg(
        long,
        default_value = "sourcedata",
        value_name = "sourcedata|datatype|anat|drop"
    )]
    localizers: String,
    /// Where a vendor's synthetic contrast goes. The qMRI appendix permits it
    /// in raw anat/; a purist puts every synthetic image in derivatives/
    #[arg(long, default_value = "anat", value_name = "anat|derivatives")]
    synthetic: String,
    /// The DICOM to NIfTI converter, a prerequisite of a BIDS release (§9.6)
    #[arg(long, default_value = "dcm2niix", value_name = "PATH")]
    dcm2niix: PathBuf,
    /// Write .nii rather than .nii.gz
    #[arg(long = "no-compress")]
    no_compress: bool,
    /// Whose dataset it is, for dataset_description.json. Repeatable; the
    /// default is whoever ran the release
    #[arg(long, value_name = "NAME")]
    author: Vec<String>,
    /// The pack, recorded on the release so a tree names what judged it
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Withdraw a version of this name with --why, instead of releasing: a
    /// release is never removed, only withdrawn (Wave 4a section 13.1)
    #[arg(long, value_name = "VERSION", requires = "why")]
    withdraw: Option<String>,
    /// Why a version is withdrawn
    #[arg(long, value_name = "TEXT")]
    why: Option<String>,
    /// An observation kind whose nearest value each session carries in its
    /// sessions.tsv (Wave 4a §7.4). Repeatable; the default is the
    /// vocabulary's primary kinds
    #[arg(long, value_name = "KIND")]
    observation: Vec<String>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct PrivateArgs {
    /// The tree to read
    /// The tree to survey; with --measured, the registry is read instead
    #[arg(value_name = "ROOT", required_unless_present = "measured")]
    root: Option<PathBuf>,
    /// Stop after this many files. A survey is about what is there, not about
    /// how much of it there is, and a few thousand files of an archive answer
    /// that as well as all of them
    #[arg(long, default_value_t = 20_000, value_name = "N")]
    files: usize,
    /// How many files to read at once
    #[arg(long, default_value_t = 8, value_name = "N")]
    workers: usize,
    /// The pack whose dictionary names the elements and whose ingest list
    /// says which are read already (Wave 4a section 5.3)
    #[arg(long, default_value = "mri", value_name = "PACK")]
    pack: String,
    /// Where the packs are; $NILS_PACK_DIR, else `packs/` in the registry home
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Print the elements worth ingesting as pack entries, under the rule
    /// that an element varies per acquisition, is printable and is short
    #[arg(long)]
    suggest: bool,
    /// Read what the registry's digests kept per series instead of a tree:
    /// per address, how many series hold it, in how many it varied within
    /// the series, and how many distinct values it takes across series
    /// (Wave 4a section 5.3). A value that varies inside nearly every series
    /// is a per-image value and not a parameter of the acquisition.
    #[arg(long, conflicts_with = "suggest")]
    measured: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SelectArgs {
    /// Subjects, by code or by any identifier the registry resolves
    #[arg(long, value_name = "SUBJECT")]
    subject: Vec<String>,
    /// Sessions, as `<subject>:<label>`
    #[arg(long, value_name = "SUBJECT:LABEL")]
    session: Vec<String>,
    /// Stacks, by id
    #[arg(long, value_name = "ID")]
    stack: Vec<i64>,
    /// Every current member of this cohort, by name
    #[arg(long, value_name = "NAME")]
    cohort: Vec<String>,
    /// Stacks holding this value on this pack axis, as `<axis>=<value>`
    #[arg(long, value_name = "AXIS=VALUE")]
    axis: Vec<String>,
    /// The selection from a file, or `-` for standard input, in the form
    /// `nils release --select` takes
    #[arg(long, value_name = "FILE")]
    select: Option<String>,
    /// The pack whose axes an axis item is checked against
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Address to listen on; port 0 picks a free one and prints it
    #[arg(long, default_value = "127.0.0.1:8437", value_name = "ADDR")]
    bind: String,
    /// How a caller is known: `off` (the local user, laptop mode), `token`
    /// (a bearer token names the caller; see --token) or `oidc` (the
    /// engine validates the issuer's token and maps its groups to roles;
    /// see --oidc-issuer, --oidc-audience, --oidc-jwks, --role)
    #[arg(long, default_value = "off", value_name = "off|token|oidc")]
    auth: String,
    /// The OIDC issuer, as the token's `iss` claim spells it
    #[arg(long, value_name = "URL")]
    oidc_issuer: Option<String>,
    /// The audience a token must name (`aud`)
    #[arg(long, value_name = "AUD")]
    oidc_audience: Option<String>,
    /// The issuer's JWKS document, as a file the deployment keeps current
    #[arg(long, value_name = "FILE")]
    oidc_jwks: Option<PathBuf>,
    /// The claim that carries the groups
    #[arg(long, default_value = "groups", value_name = "CLAIM")]
    oidc_groups_claim: String,
    /// A group and the role it grants, as `GROUP=reader|reviewer|operator|admin`;
    /// repeatable. A role implies the ones below it; a caller with no
    /// mapped group is a reader
    #[arg(long, value_name = "GROUP=ROLE")]
    role: Vec<String>,
    /// A token and who it names, as `TOKEN=user@node`; repeatable. Or set
    /// NILS_TOKENS to a comma-separated list of the same
    #[arg(long, value_name = "TOKEN=PRINCIPAL")]
    token: Vec<String>,
    /// Request handlers, each with a registry connection of its own
    #[arg(long, default_value = "4", value_name = "N")]
    workers: usize,
    /// The pack directory the doors read packs from
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Stop after serving this many requests (for tests)
    #[arg(long, hide = true)]
    requests: Option<usize>,
}

#[derive(Debug, Subcommand)]
enum AuditCommand {
    /// The rows, newest first
    List {
        /// Only this principal, as `user@node`
        #[arg(long, value_name = "WHO")]
        principal: Option<String>,
        /// Only this action, or every action under a dotted prefix (`linkage.`)
        #[arg(long, value_name = "ACTION")]
        action: Option<String>,
        /// From this time on, as an ISO stamp
        #[arg(long, value_name = "STAMP")]
        since: Option<String>,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// The jobs not over, newest first; --all for the finished ones too
    List {
        #[arg(long)]
        all: bool,
        /// How many to show
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// One job: its state, its progress, its error, its command line
    Show {
        id: i64,
        #[arg(long)]
        json: bool,
    },
    /// Ask a running job to stop at its next heartbeat, or drop a queued one
    Cancel { id: i64 },
    /// Run a finished, failed or cancelled job again, from the command line
    /// it recorded; every verb resumes, because nothing is in flight
    Resume { id: i64 },
    /// Put a nils command line on the queue, to be run by a worker in its
    /// turn: `nils jobs enqueue -- digest /data --name x`
    Enqueue {
        /// A name for the queued job
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// The command line, without the leading `nils`
        #[arg(trailing_var_arg = true, required = true, value_name = "ARGS")]
        command: Vec<String>,
    },
    /// Delete finished jobs older than the retention, a year by default
    /// (Wave 4a section 13.1); a running or queued job is never pruned
    Prune {
        #[arg(long, default_value = "365", value_name = "DAYS")]
        keep_days: u32,
    },
    /// Run queued jobs, oldest first, one at a time; stops when the queue
    /// is empty with --once, else waits for more
    Work {
        #[arg(long)]
        once: bool,
        /// Seconds between looks at an empty queue
        #[arg(long, default_value = "5")]
        every: u64,
    },
}

#[derive(Debug, Subcommand)]
enum ClinicalCommand {
    /// The vocabulary: the diseases, their types, and the kinds of observation
    #[command(subcommand)]
    Vocabulary(VocabularyCommand),
    /// Import a CSV under a mapping: preview what would change, then apply
    /// under your name (Wave 4a section 7.2)
    Import(ClinicalImportArgs),
    /// The cohorts and how many current members each has, which is what
    /// `--cohort` and `@name` select (Wave 4a section 8)
    #[command(subcommand)]
    Cohort(CohortCommand),
}

#[derive(Debug, Subcommand)]
enum CohortCommand {
    /// Every cohort, with its owner and its current member count
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct ClinicalImportArgs {
    /// The mapping: the target, how a row names its subject, the columns and
    /// their parsers, the key, and what to do with a row that exists
    #[arg(long, value_name = "FILE")]
    mapping: PathBuf,
    /// The CSV to import
    #[arg(long, value_name = "FILE")]
    file: PathBuf,
    /// Write; without it the import is a preview that changes nothing
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum VocabularyCommand {
    /// Load a vocabulary file into the registry, by name: what exists is
    /// updated, what is new is added, nothing is removed
    Load {
        /// The vocabulary file; by default `clinical/vocabulary.yml` in the pack directory
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Where the packs are; $NILS_PACK_DIR, else `packs/` in the registry home
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// What the registry holds
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HandoverCommand {
    /// Pack a release into encrypted archives and record what was sent
    Run(Box<HandoverArgs>),
    /// Read a handover back: every archive there, its checksum, and openable
    Verify(HandoverVerifyArgs),
    /// The handovers of this registry, with what each sent
    List(HandoverListArgs),
    /// The password of a key, for the person the archives are going to
    Password(HandoverPasswordArgs),
}

#[derive(Debug, Parser)]
struct HandoverArgs {
    /// The release to hand over, by name. Its newest finished version is the
    /// one packed, because that is the one the tree is
    #[arg(long, value_name = "NAME")]
    release: String,
    /// Where the archives go, which is not where the tree is
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    /// The key the password is derived from, by name. The password is never
    /// stored and never appears in a command line
    #[arg(long, value_name = "NAME")]
    key: String,
    /// How big an archive may get, as a number with a unit
    #[arg(long, default_value = "100GB", value_name = "SIZE")]
    chunk: String,
    /// How the chunks are filled: in the tree's order, or largest first into
    /// the first with room
    #[arg(long, default_value = "ordered", value_name = "ordered|packed")]
    strategy: String,
    /// 7z's compression level. Low by default: a released tree is mostly
    /// already-compressed NIfTI and the time costs more than the bytes
    #[arg(long, default_value_t = 1, value_name = "0-9")]
    level: u8,
    /// Write PAR2 recovery records at this percentage. Needs par2create
    #[arg(long, value_name = "PERCENT")]
    par2: Option<u8>,
    /// Do not read the archives back after writing them
    #[arg(long)]
    no_verify: bool,
    /// The archiver
    #[arg(long = "7z", default_value = "7z", value_name = "PATH")]
    sevenzip: PathBuf,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct HandoverVerifyArgs {
    /// Which handover, by its id
    #[arg(long, value_name = "ID")]
    handover: i64,
    /// The key its password was derived from
    #[arg(long, value_name = "NAME")]
    key: String,
    #[arg(long = "7z", default_value = "7z", value_name = "PATH")]
    sevenzip: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct HandoverListArgs {
    /// Only the handovers of this release
    #[arg(long, value_name = "NAME")]
    release: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct HandoverPasswordArgs {
    /// The key, by name
    #[arg(long, value_name = "NAME")]
    key: String,
}

#[derive(Debug, Subcommand)]
enum PickCommand {
    /// Choose one stack per session and role, and write why
    Run(PickArgs),
    /// The picks, with what was chosen and what was close
    List {
        /// Only this role
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,
        /// Only the ones worth a person's eye
        #[arg(long)]
        borders: bool,
        /// Only this subject
        #[arg(long, value_name = "CODE")]
        subject: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Why one pick came out the way it did: every component, and what it read
    Explain {
        /// The pick's id, from `nils pick list`
        id: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
struct PickArgs {
    /// The pack's name, looked up in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// An origin-scoped amendment to it
    #[arg(long, value_name = "FILE")]
    overlay: Option<PathBuf>,
    /// The session scheme, as a file; without one, the registry's stored
    /// scheme or the default
    #[arg(long, value_name = "FILE", conflicts_with = "scheme_name")]
    scheme: Option<PathBuf>,
    /// A scheme stored in this registry, by name
    #[arg(long = "scheme-name", value_name = "NAME")]
    scheme_name: Option<String>,
    /// Only this subject, which also narrows the population scored against
    #[arg(long, value_name = "CODE")]
    subject: Option<String>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Derive every subject's sessions and print them
    List(SessionListArgs),
    /// The schemes this registry keeps
    Scheme {
        #[command(subcommand)]
        command: SchemeCommand,
    },
}

#[derive(Debug, Args)]
struct SessionListArgs {
    /// A scheme file to derive with, instead of a stored one
    #[arg(long, value_name = "FILE", conflicts_with = "name")]
    scheme: Option<PathBuf>,
    /// A scheme stored in this registry, by name
    #[arg(long = "scheme-name", value_name = "NAME")]
    name: Option<String>,
    /// Month zero per subject, as a CSV of `code,date`, for an explicit anchor
    #[arg(long, value_name = "FILE")]
    anchors: Option<PathBuf>,
    /// Only this subject
    #[arg(long, value_name = "CODE")]
    subject: Option<String>,
    /// Only the sessions worth a look
    #[arg(long)]
    flagged: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum SchemeCommand {
    /// Keep a scheme under a name, so a labelling can be reproduced
    Add {
        /// What to call it
        name: String,
        /// The scheme's YAML
        file: PathBuf,
        /// Why this scheme exists
        #[arg(long)]
        note: Option<String>,
        /// Replace a scheme of this name
        #[arg(long)]
        force: bool,
    },
    /// The schemes this registry keeps
    List,
    /// One scheme, as the resolver will read it
    Show {
        name: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Forget a scheme
    Remove { name: String },
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
    /// Apply a decision to one item: to a whole grouped question as one
    /// row with the group's scope, or to one member with --member, reaching
    /// as wide as --scope says; --stage writes it without putting it in
    /// force until `review commit` (Wave 4a section 10.2). The one verb
    Apply(DecideArgs),
    /// The same as `apply`, under the name Wave 2 gave it
    Decide(DecideArgs),
    /// Put staged decisions in force: one by id, or every one with --all.
    /// Refused when the registry moved on since they were staged, unless
    /// --anyway
    Commit {
        /// The decision to commit
        id: Option<i64>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        anyway: bool,
    },
    /// Withdraw a decision, staged or committed: it stops being in force,
    /// the items it closed open again, and nothing is deleted
    Withdraw {
        /// The decision to withdraw
        id: i64,
    },
    /// Acknowledge one review item: the machine was right and you checked.
    /// Not a decision, and not counted as one (Wave 4a section 13.4)
    Accept {
        id: i64,
        /// A word on what was checked
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
    },
}

/// `nils review decide` (Wave 2 §8.3).
#[derive(Debug, Args)]
struct DecideArgs {
    /// The review item to answer
    id: i64,
    /// For a grouped question, one member (a stack id) to decide on its
    /// own; without it the decision is one row for the whole group
    #[arg(long, value_name = "STACK")]
    member: Option<i64>,
    /// How far a member's answer reaches: this stack, or everything of its
    /// series, its subject, or the machine that made it
    #[arg(
        long,
        default_value = "stack",
        value_name = "stack|series|subject|origin"
    )]
    scope: String,
    /// Write the decision without putting it in force; `review commit` does
    #[arg(long)]
    stage: bool,
    /// What the axis is, in the person's judgement
    #[arg(long, value_name = "VALUE", conflicts_with = "nothing")]
    value: Option<String>,
    /// The axis has no value on this stack
    #[arg(long)]
    nothing: bool,
    /// Who decided; the operating system's user by default
    #[arg(long, value_name = "WHO")]
    actor: Option<String>,
    /// What kind of author it is (§10.1). A model's answer must be
    /// distinguishable from a person's wherever it is read.
    #[arg(
        long = "as",
        default_value = "person",
        value_name = "person|agent|model"
    )]
    author_kind: String,
    /// The model's version, required when the author is a model (D15)
    #[arg(long, value_name = "VERSION")]
    model_version: Option<String>,
    /// Why, in the person's own words
    #[arg(long, value_name = "TEXT")]
    why: Option<String>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
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
    /// The pack whose ingest list says which private elements are read into
    /// the registry (Wave 4a section 5.2)
    #[arg(long, default_value = "mri", value_name = "PACK")]
    pack: String,
    /// Where the packs are; $NILS_PACK_DIR, else `packs/` in the registry home
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// Read no private element at all, whatever the pack says
    #[arg(long)]
    no_private: bool,
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
        Command::Fingerprint(args) => fingerprint(&home, args),
        Command::Classify(args) => classify(&home, args),
        Command::Explain { stack, json } => explain(&home, stack, json),
        Command::Pack { command } => pack_command(&home, command),
        Command::Status(args) => status(&home, args),
        Command::Linkage { command } => linkage_command(&home, command),
        Command::Quarantine { command } => quarantine_command(&home, command),
        Command::Review { command } => review_command(&home, command),
        Command::Private(args) => private_survey(&home, args),
        Command::Release(args) => release(&home, *args),
        Command::Handover(command) => handover_command(&home, command),
        Command::Clinical(command) => clinical_command(&home, command),
        Command::Select(args) => select_preview(&home, args),
        Command::Jobs(command) => jobs_command(&home, command),
        Command::Serve(args) => serve::serve(&home, args),
        Command::Audit(AuditCommand::List {
            principal,
            action,
            since,
            limit,
            json,
        }) => audit_list(&home, principal, action, since, limit, json),
        Command::Pick { command } => pick_command(&home, command),
        Command::Session { command } => session_command(&home, command),
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

/// `nils classify` (Wave 2 §9).
#[derive(Debug, Parser)]
struct ClassifyArgs {
    /// The pack's name, looked up in the pack directory
    #[arg(long, default_value = "mri")]
    pack: String,
    #[arg(long, value_name = "DIR")]
    pack_dir: Option<PathBuf>,
    /// An origin-scoped amendment to it
    #[arg(long, value_name = "FILE")]
    overlay: Option<PathBuf>,
    /// The run's label, recorded on the job
    #[arg(long)]
    name: Option<String>,
    /// Only stacks of this modality
    #[arg(long, value_name = "MR|CT|PT|...")]
    modality: Option<String>,
    /// Ask about every axis below this confidence, whatever the pack declares
    #[arg(long, value_name = "0..1")]
    review_below: Option<f64>,
    /// Stacks per window, one transaction each
    #[arg(long, value_name = "N")]
    window: Option<usize>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

fn classify(home: &Home, args: ClassifyArgs) -> Result<(), Exit> {
    let dir = pack_dir(home, args.pack_dir)?;
    let found = packs_in(&dir)?
        .into_iter()
        .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()))
        .ok_or_else(|| fail(format!("no pack named {} in {}", args.pack, dir.display())))?;
    let overlay = load_overlay(args.overlay.as_ref())?;
    let pack = nils_pack::load(&found, overlay.as_ref()).map_err(|e| fail(e.to_string()))?;

    let mut settings = nils_classify::Settings {
        modality: args.modality,
        ..nils_classify::Settings::default()
    };
    if let Some(n) = args.name {
        settings.name = n;
    }
    if let Some(w) = args.window {
        if w == 0 {
            return Err(usage("--window must be at least 1"));
        }
        settings.window = w;
    }
    if let Some(r) = args.review_below {
        if !(0.0..=1.0).contains(&r) {
            return Err(usage("--review-below is a confidence, between 0 and 1"));
        }
        settings.review_below = Some(r);
    }

    let cancel = stop_on_signal()?;
    let mut registry = open(home)?;
    let report = nils_classify::classify::classify(&mut registry, &pack, &settings, &cancel)
        .map_err(|e| match e {
            nils_classify::Error::Busy { .. } => Exit {
                code: BUSY,
                message: e.to_string(),
            },
            other => fail(other.to_string()),
        })?;
    if args.json {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| fail(format!("the report will not serialize: {e}")))?;
        println!("{text}");
    } else {
        print!("{report}");
    }
    if report.cancelled {
        return Err(Exit {
            code: STOPPED,
            message: "stopped: what was judged is written; run again to go on".into(),
        });
    }
    Ok(())
}

/// `nils explain` (Wave 2 §12): small, and load-bearing. It is the answer to
/// "why is this a T2w", which v0 cannot give at all.
fn explain(home: &Home, stack: i64, json: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let meta = store
        .query(
            &format!(
                "SELECT pack, pack_version, contract, overlay, review_items FROM {} WHERE stack_id = {}",
                store.qualified("classification"),
                store.dialect().param(1, nils_registry::schema::Type::Int)
            ),
            &[nils_registry::store::Param::Int(stack)],
        )
        .map_err(|e| fail(e.to_string()))?;
    let Some(m) = meta.first() else {
        return Err(fail(format!("stack {stack} has not been classified")));
    };
    let pack = format!(
        "{}@{}",
        m.text(0).map_err(|e| fail(e.to_string()))?,
        m.text(1).map_err(|e| fail(e.to_string()))?
    );
    let overlay = m
        .opt_text(3)
        .map_err(|e| fail(e.to_string()))?
        .map(str::to_string);
    let review_items = m.int(4).map_err(|e| fail(e.to_string()))?;

    let axes = store
        .query(
            &format!(
                "SELECT axis, value, confidence, tier FROM {} WHERE stack_id = {} ORDER BY axis",
                store.qualified("classification_axis"),
                store.dialect().param(1, nils_registry::schema::Type::Int)
            ),
            &[nils_registry::store::Param::Int(stack)],
        )
        .map_err(|e| fail(e.to_string()))?;
    let ev = store
        .query(
            &format!(
                "SELECT axis, value, tier, confidence, rule_set, rule, source, matched, \
                        author, author_kind FROM {} \
                 WHERE stack_id = {} ORDER BY axis, id",
                store.qualified("classification_evidence"),
                store.dialect().param(1, nils_registry::schema::Type::Int)
            ),
            &[nils_registry::store::Param::Int(stack)],
        )
        .map_err(|e| fail(e.to_string()))?;

    if json {
        let axes_json: Vec<serde_json::Value> = axes
            .iter()
            .map(|r| {
                serde_json::json!({
                    "axis": r.text(0).unwrap_or_default(),
                    "value": r.opt_text(1).ok().flatten(),
                    "confidence": r.double(2).unwrap_or(0.0),
                    "tier": r.text(3).unwrap_or_default(),
                })
            })
            .collect();
        let ev_json: Vec<serde_json::Value> = ev
            .iter()
            .map(|r| {
                serde_json::json!({
                    "axis": r.text(0).unwrap_or_default(),
                    "value": r.text(1).unwrap_or_default(),
                    "tier": r.text(2).unwrap_or_default(),
                    "confidence": r.double(3).unwrap_or(0.0),
                    "rule_set": r.text(4).unwrap_or_default(),
                    "rule": r.text(5).unwrap_or_default(),
                    "source": r.text(6).unwrap_or_default(),
                    "matched": r.opt_text(7).ok().flatten(),
                    "author": r.opt_text(8).ok().flatten(),
                    "author_kind": r.opt_text(9).ok().flatten(),
                })
            })
            .collect();
        let v = serde_json::json!({
            "stack": stack, "pack": pack, "overlay": overlay,
            "review_items": review_items, "axes": axes_json, "evidence": ev_json,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&v)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }

    println!("stack {stack}, judged by {pack}");
    if let Some(o) = &overlay {
        println!("  under overlay {o}");
    }
    for r in &axes {
        let axis = r.text(0).unwrap_or_default();
        let value = r.opt_text(1).ok().flatten().unwrap_or("");
        println!(
            "  {axis:16} {:20} {:.2}  {}",
            if value.is_empty() { "(nothing)" } else { value },
            r.double(2).unwrap_or(0.0),
            r.text(3).unwrap_or_default()
        );
        for e in ev.iter().filter(|e| e.text(0).unwrap_or_default() == axis) {
            // §10.1. A value somebody decided says who, and with what
            // standing, in the same place a rule's answer says which rule.
            // Whether a model produced it has to be readable here, or it
            // reads exactly like a rule's answer, which is v0's 4,692 body
            // parts.
            match e.opt_text(9).ok().flatten() {
                Some(kind) => println!(
                    "      a {kind}, {}, decided {} for the {}{}",
                    e.opt_text(8).ok().flatten().unwrap_or("unnamed"),
                    e.text(1).unwrap_or_default(),
                    e.text(5).unwrap_or_default(),
                    match e.opt_text(7).ok().flatten() {
                        Some(v) => format!(" (version {v})"),
                        None => String::new(),
                    }
                ),
                None => println!(
                    "{}",
                    format!(
                        "      {} said {} by {}, from {} {}",
                        e.text(4).unwrap_or_default(),
                        e.text(1).unwrap_or_default(),
                        e.text(2).unwrap_or_default(),
                        e.text(6).unwrap_or_default(),
                        e.opt_text(7).ok().flatten().unwrap_or("")
                    )
                    .trim_end()
                ),
            }
        }
    }
    if review_items > 0 {
        println!("  {review_items} review item(s) were raised for this stack");
    }
    Ok(())
}

#[derive(Debug, Subcommand)]
enum PackCommand {
    /// The packs in the pack directory, with their versions
    List {
        /// Where the packs are; $NILS_PACK_DIR, else `packs/` in the registry home
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// What one pack declares: its identity, its vocabulary, its buckets
    Show {
        /// The pack's name
        name: String,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        /// Load it under this overlay as well
        #[arg(long, value_name = "FILE")]
        overlay: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Load a pack directory and run its own corpus, saying what is wrong
    Validate {
        /// The pack directory
        dir: PathBuf,
        /// Check an overlay against it too
        #[arg(long, value_name = "FILE")]
        overlay: Option<PathBuf>,
    },
}

/// Where packs live: the flag, then the environment, then the registry home.
fn pack_dir(home: &Home, given: Option<PathBuf>) -> Result<PathBuf, Exit> {
    if let Some(d) = given {
        return Ok(d);
    }
    if let Some(d) = std::env::var_os("NILS_PACK_DIR") {
        return Ok(PathBuf::from(d));
    }
    let d = home.dir().join("packs");
    if d.is_dir() {
        return Ok(d);
    }
    Err(usage(format!(
        "no pack directory: pass --pack-dir, set NILS_PACK_DIR, or put packs in {}",
        d.display()
    )))
}

/// Every subdirectory that holds a `pack.yml`, in name order.
fn packs_in(dir: &Path) -> Result<Vec<PathBuf>, Exit> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| fail(format!("{}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("pack.yml").is_file())
        .collect();
    out.sort();
    Ok(out)
}

fn load_overlay(path: Option<&PathBuf>) -> Result<Option<nils_pack::Overlay>, Exit> {
    match path {
        None => Ok(None),
        Some(p) => nils_pack::Overlay::load(p)
            .map(Some)
            .map_err(|e| fail(e.to_string())),
    }
}

fn pack_command(home: &Home, command: PackCommand) -> Result<(), Exit> {
    match command {
        PackCommand::List { pack_dir: d, json } => {
            let dir = pack_dir(home, d)?;
            let mut rows = Vec::new();
            for p in packs_in(&dir)? {
                // A pack that will not load is listed with why, not hidden:
                // the one you need to fix is the one you cannot see.
                let (id, modality, state) = match nils_pack::load(&p, None) {
                    Ok(pack) => (
                        pack.id(),
                        pack.modality.clone(),
                        format!("{} cases", pack.cases),
                    ),
                    Err(e) => (
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        String::new(),
                        format!("will not load: {e}"),
                    ),
                };
                rows.push((id, modality, state, p));
            }
            if json {
                let v: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|(id, m, s, p)| {
                        serde_json::json!({"pack": id, "modality": m, "state": s, "dir": p})
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v)
                        .map_err(|e| fail(format!("the list will not serialize: {e}")))?
                );
            } else if rows.is_empty() {
                println!("no packs in {}", dir.display());
            } else {
                for (id, m, s, _) in &rows {
                    println!("{id:24} {m:4} {s}");
                }
            }
            Ok(())
        }

        PackCommand::Show {
            name,
            pack_dir: d,
            overlay,
            json,
        } => {
            let dir = pack_dir(home, d)?;
            let found = packs_in(&dir)?
                .into_iter()
                .find(|p| p.file_name().is_some_and(|f| f == name.as_str()))
                .ok_or_else(|| fail(format!("no pack named {name} in {}", dir.display())))?;
            let ov = load_overlay(overlay.as_ref())?;
            let pack = nils_pack::load(&found, ov.as_ref()).map_err(|e| fail(e.to_string()))?;
            if json {
                let v = serde_json::json!({
                    "pack": pack.name,
                    "version": pack.version.to_string(),
                    "contract": pack.contract,
                    "fields": pack.fields.iter().map(|(f, v)| (f.clone(), serde_json::Value::from(v.name()))).collect::<serde_json::Map<_, _>>(),
                    "modality": pack.modality,
                    "parsers": pack.parsers.iter().map(|p| serde_json::json!({
                        "name": p.name, "predicates": p.preds.len()
                    })).collect::<Vec<_>>(),
                    "flags": pack.flags.len(),
                    "axes": pack.axes.iter().map(|a| serde_json::json!({
                        "axis": a.name, "multi": a.multi, "values": a.values.len(),
                        "review_below": pack.review.below(&a.name),
                        "asks_when_missing": pack.review.asks_when_missing(&a.name),
                    })).collect::<Vec<_>>(),
                    "passes": pack.passes.iter().map(|p| serde_json::json!({
                        "pass": p.name, "kind": p.kind_name(),
                        "phase": format!("{:?}", p.phase).to_lowercase(),
                        "reference": p.reference.scope,
                    })).collect::<Vec<_>>(),
                    "rule_sets": pack.rule_sets.iter().map(|r| serde_json::json!({
                        "rule_set": r.name, "rules": r.rules.len(),
                        "decides": r.decides, "entered": r.enter_when.is_some(),
                    })).collect::<Vec<_>>(),
                    "buckets": pack.buckets,
                    "cases": pack.cases,
                    "overlay": pack.overlay,
                    "private": serde_json::json!({
                        "coverage": pack.private_coverage,
                        "ingest": pack.ingest.iter().map(|i| serde_json::json!({
                            "name": i.name, "address": i.text(), "vr": i.vr,
                            "dictionary_name": i.dictionary_name, "kind": i.kind,
                        })).collect::<Vec<_>>(),
                        "release": pack.release.iter().map(|a| a.text()).collect::<Vec<_>>(),
                        "dictionary": {
                            "creators": pack.dictionary.creators(),
                            "elements": pack.dictionary.len(),
                        },
                    }),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&v)
                        .map_err(|e| fail(format!("the pack will not serialize: {e}")))?
                );
            } else {
                println!(
                    "{} for {}, contract {}",
                    pack.id(),
                    pack.modality,
                    pack.contract
                );
                for p in &pack.parsers {
                    println!("  parser {:20} {:3} predicates", p.name, p.preds.len());
                }
                println!("  flags   {:20} {:3}", "", pack.flags.len());
                println!("  cases   {:20} {:3}", "", pack.cases);
                println!(
                    "  private {:20} {:3} ingested, {} kept on release, dictionary of {} elements from {} creators",
                    "",
                    pack.ingest.len(),
                    pack.release.len(),
                    pack.dictionary.len(),
                    pack.dictionary.creators()
                );
                for c in &pack.private_coverage {
                    println!("          {:20} covers {c}", "");
                }
                if !pack.fields.is_empty() {
                    // Wave 4a §11.2 (C27): read by nothing yet, said here so
                    // a pack author sees what the pack declares.
                    let mut by: std::collections::BTreeMap<&str, Vec<&str>> =
                        std::collections::BTreeMap::new();
                    for (field, v) in &pack.fields {
                        by.entry(v.name()).or_default().push(field.as_str());
                    }
                    for (visibility, fields) in by {
                        println!(
                            "  fields  {:20} {:3} {visibility}: {}",
                            "",
                            fields.len(),
                            fields.join(", ")
                        );
                    }
                }
                for a in &pack.axes {
                    let asked = if pack.review.asks_when_missing(&a.name) {
                        ", asked when missing"
                    } else {
                        ""
                    };
                    println!(
                        "  axis    {:20} {:3} values, asked below {:.2}{asked}",
                        a.name,
                        a.values.len(),
                        pack.review.below(&a.name)
                    );
                }
                for p in &pack.passes {
                    println!(
                        "  pass    {:20} {} against {}",
                        p.name,
                        p.kind_name(),
                        p.reference.scope
                    );
                }
                for r in &pack.rule_sets {
                    println!(
                        "  rules   {:20} {:3} rules{}",
                        r.name,
                        r.rules.len(),
                        if r.enter_when.is_some() {
                            ", entered by a condition"
                        } else {
                            ""
                        }
                    );
                }
                if pack.buckets.is_empty() {
                    println!("  buckets {:20} none open for editing", "");
                } else {
                    for (name, values) in &pack.buckets {
                        println!("  bucket  {name:20} {:3} terms", values.len());
                    }
                }
                if let Some(o) = &pack.overlay {
                    println!("  under overlay {o}");
                }
            }
            Ok(())
        }

        PackCommand::Validate { dir, overlay } => {
            let ov = load_overlay(overlay.as_ref())?;
            match nils_pack::load(&dir, ov.as_ref()) {
                Ok(pack) => {
                    println!(
                        "{} loads: {} predicates, {} flags, {} cases{}",
                        pack.id(),
                        pack.parsers.iter().map(|p| p.preds.len()).sum::<usize>(),
                        pack.flags.len(),
                        pack.cases,
                        match &pack.overlay {
                            Some(o) => format!(", under overlay {o}"),
                            None => String::new(),
                        }
                    );
                    Ok(())
                }
                Err(e) => Err(fail(e.to_string())),
            }
        }
    }
}

/// `nils fingerprint` (Wave 2 §4.3).
#[derive(Debug, Parser)]
struct FingerprintArgs {
    /// The run's label, recorded on the job
    #[arg(long)]
    name: Option<String>,
    /// Derive again for stacks that already have a fingerprint
    #[arg(long)]
    force: bool,
    /// Only stacks of this modality
    #[arg(long, value_name = "MR|CT|PT|...")]
    modality: Option<String>,
    /// Stacks per window, one transaction each
    #[arg(long, value_name = "N")]
    window: Option<usize>,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

fn fingerprint(home: &Home, args: FingerprintArgs) -> Result<(), Exit> {
    let mut settings = nils_classify::Settings {
        force: args.force,
        modality: args.modality,
        ..nils_classify::Settings::default()
    };
    if let Some(name) = args.name {
        settings.name = name;
    }
    if let Some(n) = args.window {
        if n == 0 {
            return Err(usage("--window must be at least 1"));
        }
        settings.window = n;
    }
    let cancel = stop_on_signal()?;
    let mut registry = open(home)?;
    let report = nils_classify::run(&mut registry, &settings, &cancel).map_err(|e| match e {
        nils_classify::Error::Busy { .. } => Exit {
            code: BUSY,
            message: e.to_string(),
        },
        other => fail(other.to_string()),
    })?;
    if args.json {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| fail(format!("the report will not serialize: {e}")))?;
        println!("{text}");
    } else {
        print!("{report}");
    }
    if report.cancelled {
        return Err(Exit {
            code: STOPPED,
            message: "stopped: what was derived is written; run again to go on".into(),
        });
    }
    Ok(())
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
    // Wave 4a §5.2: the private elements the pack asks for are read at
    // digest time. A pack that cannot be found is not an error unless one was
    // asked for by name or by directory, because a digest has never needed a
    // pack; the report says what was read either way.
    if !args.no_private {
        let asked = args.pack_dir.is_some() || args.pack != "mri";
        match pack_dir(home, args.pack_dir.clone()) {
            Ok(dir) => {
                let found = packs_in(&dir)?
                    .into_iter()
                    .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()));
                match found {
                    Some(found) => {
                        let pack =
                            nils_pack::load(&found, None).map_err(|e| fail(e.to_string()))?;
                        settings.ingest = pack
                            .ingest
                            .iter()
                            .map(|i| nils_dicom::private::Ingest {
                                creator: i.creator.clone(),
                                group: i.group,
                                element: i.element,
                                vr: i.vr.clone(),
                            })
                            .collect();
                        settings.ingest_from = Some(pack.id());
                    }
                    None if asked => {
                        return Err(fail(format!(
                            "no pack named {} in {}",
                            args.pack,
                            dir.display()
                        )));
                    }
                    None => {}
                }
            }
            Err(e) if asked => return Err(e),
            Err(_) => {}
        }
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
    let doc = status_doc(home, &mut registry)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    let config = registry.config().clone();
    let meta = registry.meta().clone();
    let (jobs, others, batches) = (
        doc["jobs"].as_array().cloned().unwrap_or_default(),
        doc["other_jobs"].as_array().cloned().unwrap_or_default(),
        doc["batches"].as_array().cloned().unwrap_or_default(),
    );
    status_print(
        home,
        &config,
        &meta,
        &jobs,
        &others,
        &batches,
        doc["registry"]["schema"].as_str(),
    )
}

/// The status document (`nils status --json`, `GET /api/status`).
fn status_doc(home: &Home, registry: &mut Registry) -> Result<serde_json::Value, Exit> {
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
    Ok(doc)
}

fn status_print(
    home: &Home,
    config: &nils_registry::home::Config,
    meta: &nils_registry::home::Meta,
    jobs: &[serde_json::Value],
    others: &[serde_json::Value],
    batches: &[serde_json::Value],
    schema: Option<&str>,
) -> Result<(), Exit> {
    println!(
        "nils status   registry {}   backend {}{}",
        home.dir().display(),
        config.backend.name(),
        match schema {
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
    for j in jobs {
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
    for j in others {
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
    for b in batches {
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
/// Who is acting: the principal, `user@node` (Wave 4a section 9.2, C30).
fn actor() -> String {
    nils_registry::principal::Principal::current().to_string()
}

/// One audit row (Wave 4a section 9.2), as the command line writes it.
fn audit(
    registry: &mut Registry,
    action: nils_registry::audit::Action,
    scope: serde_json::Value,
    details: Option<serde_json::Value>,
) -> Result<(), Exit> {
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &actor(),
            action,
            scope,
            policy: None,
            job_id: None,
            details,
        },
    )
    .map(|_| ())
    .map_err(|e| fail(e.to_string()))
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
            audit(
                &mut registry,
                nils_registry::audit::Action::LinkageReveal,
                serde_json::json!({ "subject": subject, "identifiers": shown.len() }),
                why.as_ref().map(|w| serde_json::json!({ "why": w })),
            )?;
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
            audit(
                &mut registry,
                nils_registry::audit::Action::LinkageLink,
                serde_json::json!({ "linkage": id, "a": subject_a, "b": subject_b }),
                Some(serde_json::json!({ "evidence": evidence })),
            )?;
            println!("linked {b} to {a} (linkage {id})");
            Ok(())
        }
        LinkageCommand::Unlink { id } => {
            if linkage::unlink(&mut store, id, &actor())? {
                audit(
                    &mut registry,
                    nils_registry::audit::Action::LinkageUnlink,
                    serde_json::json!({ "linkage": id }),
                    None,
                )?;
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
    // Recorded as a job, done (Wave 4a section 9.1): what it removed is in
    // the args, and never what the identifiers were.
    let _ = now;
    let job_id = nils_registry::job::claim(
        registry.store(),
        &nils_registry::job::Claim {
            kind: "linkage-purge",
            name: target.as_str(),
            args,
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    nils_registry::job::finish(
        registry.store(),
        job_id,
        nils_registry::job::State::Done,
        None,
    )
    .map_err(|e| fail(e.to_string()))?;
    audit(
        registry,
        nils_registry::audit::Action::LinkagePurge,
        serde_json::json!({
            "subject": subject, "all": all,
            "identities": purged.identities, "linkages": purged.linkages,
        }),
        Some(serde_json::json!({ "job": job_id })),
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
        "id, kind, scope, status, actor, {}, {}, {}, {}, {}, members, group_key, accepted_by",
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
            "members": r.opt_int(10)?,
            "group_key": r.opt_text(11)?,
            "accepted_by": r.opt_text(12)?,
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
        ReviewCommand::Apply(args) | ReviewCommand::Decide(args) => {
            drop(columns);
            review_decide(&mut registry, args)
        }
        ReviewCommand::Commit { id, all, anyway } => {
            drop(columns);
            if id.is_none() && !all {
                return Err(usage("name a decision to commit, or --all"));
            }
            let done = nils_registry::review::commit(&mut registry, id, anyway, &actor())
                .map_err(|e| fail(e.to_string()))?;
            println!(
                "committed {} decision(s), {} item(s) accepted",
                done.decisions.len(),
                done.items
            );
            Ok(())
        }
        ReviewCommand::Withdraw { id } => {
            drop(columns);
            let reopened = nils_registry::review::withdraw(&mut registry, id, &actor())
                .map_err(|e| fail(e.to_string()))?;
            println!("withdrew decision {id}; {reopened} item(s) open again");
            Ok(())
        }
        ReviewCommand::Accept { id, why } => {
            drop(columns);
            review_accept(&mut registry, id, why)
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
            if s(&item["scope"]) == "group" {
                let members =
                    nils_registry::review::members(store, id).map_err(|e| fail(e.to_string()))?;
                let decided = members.iter().filter(|m| m.decided_at.is_some()).count();
                println!(
                    "  members    {} stack(s), {} decided: {}{}",
                    members.len(),
                    decided,
                    members
                        .iter()
                        .take(20)
                        .map(|m| m.stack_id.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if members.len() > 20 { ", ..." } else { "" }
                );
            }
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

/// `nils review decide`: a person's answer to one item.
///
/// The answer is written twice on purpose. The `decision` row is what the
/// classifier reads on every later run, so the judgement outlives the pack
/// version that prompted it; the review item is closed with the same words,
/// so the queue shrinks by exactly what was answered. A decision that
/// replaces an earlier one on the same axis withdraws it rather than
/// overwriting it: nothing a person said is deleted.
/// `nils review accept` (Wave 4a section 9.2 and 13.4): "the machine was
/// right and I checked" sets `accepted_by` and `accepted_at` on the item
/// and writes no decision row, because that would inflate the count of
/// human-authored values. Its own home, its own count.
fn review_accept(registry: &mut Registry, id: i64, why: Option<String>) -> Result<(), Exit> {
    let who = actor();
    review_accept_as(registry, id, why, &who)
}

/// The acknowledgement by a named principal, which is how the door does it.
fn review_accept_as(
    registry: &mut Registry,
    id: i64,
    why: Option<String>,
    who: &str,
) -> Result<(), Exit> {
    let who = who.to_string();
    let now = nils_registry::time::now_iso();
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT status, kind FROM {} WHERE id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(id)])? else {
        return Err(usage(format!("no review item {id}")));
    };
    let status = row.text(0)?.to_string();
    let kind = row.text(1)?.to_string();
    if status != "open" {
        return Err(fail(format!("review item {id} is already {status}")));
    }
    let close = format!(
        "UPDATE {} SET status = 'accepted', accepted_by = {}, accepted_at = {} WHERE id = {}",
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Int),
    );
    store.execute(
        &close,
        &[
            Param::from(who.as_str()),
            Param::from(now.as_str()),
            Param::Int(id),
        ],
    )?;
    audit(
        registry,
        nils_registry::audit::Action::ReviewAccept,
        serde_json::json!({ "review_item": id, "kind": kind }),
        why.as_ref().map(|w| serde_json::json!({ "why": w })),
    )?;
    println!("review item {id} acknowledged by {who}; no decision was written");
    Ok(())
}

fn review_decide(registry: &mut Registry, args: DecideArgs) -> Result<(), Exit> {
    let DecideArgs {
        id,
        member,
        scope,
        value,
        nothing,
        actor,
        author_kind,
        model_version,
        why,
        stage,
        json,
    } = args;
    if value.is_none() && !nothing {
        return Err(usage(
            "say what the axis is (--value) or that it has no value here (--nothing)",
        ));
    }
    // A named actor is a principal too: a bare name is a user on this host.
    let who = actor
        .as_deref()
        .and_then(nils_registry::principal::Principal::parse)
        .map(|p| p.to_string())
        .unwrap_or_else(self::actor);
    let applied = nils_registry::review::apply(
        registry,
        &nils_registry::review::Apply {
            item: id,
            member,
            scope: &scope,
            value: value.as_deref(),
            author: nils_registry::review::Author {
                who: &who,
                kind: &author_kind,
                version: model_version.as_deref(),
            },
            stage,
            why: why.as_deref(),
        },
    )
    .map_err(|e| match e {
        nils_registry::review::Error::Refused(m) => usage(m),
        other => fail(other.to_string()),
    })?;
    let store = registry.store();
    let open_sql = format!(
        "SELECT COUNT(*) FROM {} WHERE status = 'open'",
        store.qualified("review_item")
    );
    let open = store
        .query(&open_sql, &[])?
        .first()
        .and_then(|r| r.int(0).ok())
        .unwrap_or(0);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "review_item": id, "decision": applied.decision, "axis": applied.axis,
                "value": value, "scope": applied.scope, "ref": applied.reference,
                "closed": applied.closed, "members": applied.members,
                "staged": applied.staged, "actor": who, "open_review_items": open,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} {} = {} at {} {} by {}{}; {} item(s) {}, {} open",
        if applied.staged { "staged" } else { "decided" },
        applied.axis,
        value.as_deref().unwrap_or("nothing"),
        applied.scope,
        applied.reference,
        who,
        if applied.members > 0 {
            format!(", {} member(s)", applied.members)
        } else {
            String::new()
        },
        applied.closed.len(),
        if applied.staged { "staged" } else { "closed" },
        open
    );
    Ok(())
}

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
        _ if s(&item["scope"]) == "group" => format!(
            "{} stack(s): {} = {} ({})",
            item["members"],
            s(&e["axis"]),
            s(&e["value"]),
            s(&e["tier"])
        ),
        _ => r.to_string(),
    }
}

/// `nils custody`: every store the registry keeps (C38), as the same table
/// the documentation carries, with this home's paths and counts.
fn custody(home: &Home, json: bool, markdown: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let doc = custody_doc(home, &mut registry)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return Ok(());
    }
    if markdown {
        print!("{}", custody_markdown(&doc));
        return Ok(());
    }
    custody_print(&doc)
}

/// The custody document (`nils custody --json`, `GET /api/custody`).
fn custody_doc(home: &Home, registry: &mut Registry) -> Result<serde_json::Value, Exit> {
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
    let fingerprints = count_of(store, "stack_fingerprint", "")?;
    let classified = count_of(store, "classification", "")?;
    let decisions = count_of(store, "decision", " WHERE withdrawn_at IS NULL")?;
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
    let cohorts = count_of(store, "cohort", "")?;
    let audit_rows = count_of(store, "audit", "")?;
    let members = count_of(store, "cohort_member", " WHERE left_at IS NULL")?;
    let diseases = count_of(store, "disease", "")?;
    let kinds = count_of(store, "observation_type", "")?;
    let events = count_of(store, "event", " WHERE superseded_by IS NULL")?;
    let with_birth_date = count_of(store, "subject", " WHERE birth_date IS NOT NULL")?;
    let sessions_cached = count_of(store, "session_cache", "")?;
    let handles = count_of(store, "handle", " WHERE withdrawn_at IS NULL")?;
    let handle_rows = count_of(store, "handle_member", "")?;
    let selections = count_of(store, "selection", "")?;
    let selection_versions = count_of(store, "selection_version", "")?;
    let values_sources = count_of(store, "values_source", "")?;
    let curated = count_of(store, "catalog_curation", "")?;
    let identifier_reads = count_of(store, "handle_read_audit", "")?;
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
            "owner": "the registry's operator",
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
            "owner": "the registry's operator, for the research group that owns the archive",
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
            "owner": "the registry's operator; the identifiers are the clinic's",
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
            "owner": "the registry's operator",
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
            "owner": "the registry's operator",
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
            "store": "classifications",
            "owner": "the pack's author for the rules, the reviewers for the decisions",
            "what": "what a pack decided about each stack, one row per axis, with the evidence that made it and any decision a person recorded",
            "where": "rows of stack_fingerprint, classification, classification_axis, classification_evidence and decision in the registry",
            "files": [],
            "holds": ["technical: the fields a pack reads, the axes, the tiers and confidences, the rule that fired", "a person's words: the why on a decision"],
            "counts": { "fingerprints": fingerprints, "classified": classified, "decisions": decisions },
            "kept": "until the next run of that job replaces it; a decision until withdrawn, and a withdrawn one for good",
            "commands": {
                "read": ["nils explain <stack>", "nils review list", "nils pack show <name>"],
                "change": ["nils fingerprint", "nils classify", "nils review decide <id> --value <v>"],
                "export": ["nils explain <stack> --json"],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "clinical layer",
            "owner": "the research group that owns the cohort",
            "what": "what v0 kept in a second database, in the one registry (Wave 4a section 7.1): cohorts and their members, the vocabulary of diseases and observation kinds, each subject's diseases, the events, and the subject's demographics",
            "where": where_db(REGISTRY_DB, &registry_schema),
            "files": if sqlite { db_files(REGISTRY_DB) } else { Vec::new() },
            "holds": [
                "quasi-identifying: birth dates, sex, dates of death, the dates of diagnoses, onsets, treatments and every observation",
                "clinical: diagnoses and their types, the scales and their values, the treatments",
            ],
            "counts": { "cohorts": cohorts, "members": members, "diseases": diseases, "observation_types": kinds, "events": events, "subjects_with_birth_date": with_birth_date },
            "kept": "for ever; a correction supersedes the old row and the old row stays (section 13.2)",
            "commands": {
                "read": ["nils clinical vocabulary list", "nils custody"],
                "change": ["nils clinical vocabulary load"],
                "export": ["nils release"],
                "delete": delete_db(REGISTRY_DB, &registry_schema),
            },
        }),
        serde_json::json!({
            "store": "job records",
            "owner": "the registry's operator",
            "what": "every verb that runs longer than a second, and the queue (Wave 4a section 9.1): its command line and arguments, host and pid, heartbeat, progress, counts and outcome",
            "where": "rows of job and ingest_batch in the registry",
            "files": [],
            "holds": ["quasi-identifying: the root path in a run's arguments and command line", "technical: the counts, the host name, the pid, the times, the outcome"],
            "counts": { "jobs": jobs, "batches": batches },
            "kept": "one year of finished jobs (Wave 4a section 13.1); a running or queued one until it is over",
            "commands": {
                "read": ["nils status [--batch <id>]", "nils jobs list [--all]", "nils jobs show <id>"],
                "change": ["nils jobs cancel <id>", "nils jobs enqueue -- <command>", "nils jobs work", "nils jobs resume <id>"],
                "export": ["nils status --json", "nils status --batch <id> --json", "nils jobs list --all --json"],
                "delete": "nils jobs prune [--keep-days <n>]",
            },
        }),
        serde_json::json!({
            "store": "audit log",
            "owner": "the registry's operator; read by whoever answers for the archive",
            "what": "who did what, to which scope, when, under which policy (Wave 4a section 9.2): every decision, acknowledgement, import, vocabulary load, linkage change, release and handover, as the principal user@node",
            "where": "rows of audit in the registry",
            "files": [],
            "holds": ["quasi-identifying: the principal, a release's root path", "technical: the action, the scope's ids and counts, the policy, the time; never an identifier"],
            "counts": { "rows": audit_rows },
            "kept": "for ever (Wave 4a section 13.1); nobody deletes an audit row",
            "commands": {
                "read": ["nils audit list [--principal <who>] [--action <action>] [--since <stamp>]"],
                "change": [],
                "export": ["nils audit list --json"],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "session cache",
            "owner": "the registry's operator",
            "what": "each subject's sessions under each window, built by the one resolver over the subject's whole timeline (Wave 4b section 7), with the labels a scheme gives them beside the identity",
            "where": "rows of session_cache, session_cache_study and session_label in the registry",
            "files": [],
            "holds": ["quasi-identifying: the first and last study day of each session", "technical: the surrogate id, the window, the timeline digest, the labels"],
            "counts": { "sessions": sessions_cached },
            "kept": "no retention: rebuildable, and dropped on rebuild (Wave 4b section 14.1)",
            "commands": {
                "read": ["nils session list"],
                "change": [],
                "export": [],
                "delete": "with the registry, or by the next rebuild",
            },
        }),
        serde_json::json!({
            "store": "questions and their answers",
            "owner": "the research group that asks; the data controller sets the retention",
            "what": "a saved question (a selection) with its immutable versions, the handle a question left behind with the keys it named and the pages it was read by, and an uploaded identifier list by reference (Wave 4b section 8)",
            "where": "rows of selection, selection_version, handle, handle_member, handle_page, values_source and values_member in the registry",
            "files": [],
            "holds": ["quasi-identifying: the subject keys a handle named, the dates and ages its pages carry", "technical: the question itself, its hash, its provenance (who, when, node, pack, epoch, scheme)"],
            "counts": { "selections": selections, "selection_versions": selection_versions, "handles": handles, "handle_rows": handle_rows, "values_sources": values_sources },
            "kept": "a handle's rows 90 days after its last read, longer while a release, a selection, a job or a cohort names it, then only its metadata, hash and question; a saved question for ever (Wave 4b section 14.1)",
            "commands": {
                "read": ["nils custody"],
                "change": [],
                "export": [],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "catalog curation",
            "owner": "the research group",
            "what": "what a person wrote about a catalog path: a description, a caveat, guidance for an assistant, a visibility or a class, keyed by the path so a re-sync never overwrites it (Wave 4b section 9)",
            "where": "rows of catalog_curation in the registry",
            "files": [],
            "holds": ["technical: prose about fields and kinds; never a subject's data"],
            "counts": { "paths": curated },
            "kept": "for ever (Wave 4b section 14.1)",
            "commands": {
                "read": ["nils custody"],
                "change": [],
                "export": [],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "identifier read audit",
            "owner": "the registry's operator; read by whoever answers for the archive",
            "what": "who projected identifiers through which handle, when, which columns and how many rows, at every role (Wave 4b section 9)",
            "where": "rows of handle_read_audit in the registry",
            "files": [],
            "holds": ["quasi-identifying: the principal", "technical: the handle, the columns, the row count, the epoch, the time; never an identifier"],
            "counts": { "rows": identifier_reads },
            "kept": "for ever, like the rest of the audit (Wave 4b section 14.1)",
            "commands": {
                "read": ["nils custody"],
                "change": [],
                "export": [],
                "delete": "with the registry",
            },
        }),
        serde_json::json!({
            "store": "logs",
            "owner": "the registry's operator",
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
    Ok(doc)
}

fn custody_print(doc: &serde_json::Value) -> Result<(), Exit> {
    println!(
        "nils custody   registry {}   backend {}",
        doc["home"].as_str().unwrap_or(""),
        doc["backend"].as_str().unwrap_or("")
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
        println!("  owner     {}", s(&st["owner"]));
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
        let _ = writeln!(page, "| owner | {} |", cell(s(&st["owner"])));
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
            audit(
                registry,
                nils_registry::audit::Action::LinkageImport,
                serde_json::json!({
                    "id_type": args.id_type, "rows": report.rows,
                    "subjects_created": report.subjects_created,
                    "identities_added": report.identities_added,
                    "unchanged": report.unchanged,
                    "second_identifiers": report.second_identifiers,
                }),
                None,
            )?;
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

// --------------------------------------------------------------------------
// Sessions (Wave 3 §5)
// --------------------------------------------------------------------------

fn session_command(home: &Home, command: SessionCommand) -> Result<(), Exit> {
    match command {
        SessionCommand::List(args) => session_list(home, args),
        SessionCommand::Scheme { command } => scheme_command(home, command),
    }
}

fn read_scheme(path: &Path) -> Result<session::Scheme, Exit> {
    let text =
        fs::read_to_string(path).map_err(|e| usage(format!("--scheme {}: {e}", path.display())))?;
    session::Scheme::parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))
}

fn stored_scheme(registry: &mut Registry, name: &str) -> Result<session::Scheme, Exit> {
    let store = registry.store();
    let row = store
        .query_opt(
            &format!(
                "SELECT definition FROM {} WHERE name = {}",
                store.qualified("session_scheme"),
                store.dialect().param(1, Type::Text)
            ),
            &[Param::Text(name.to_string())],
        )
        .map_err(|e| fail(e.to_string()))?;
    let Some(row) = row else {
        return Err(fail(format!(
            "no scheme called {name}; `nils session scheme list` says what there is"
        )));
    };
    let text = row.text(0).map_err(|e| fail(e.to_string()))?.to_string();
    session::Scheme::from_json(&text).map_err(|e| fail(e.to_string()))
}

fn scheme_command(home: &Home, command: SchemeCommand) -> Result<(), Exit> {
    let mut registry = open(home)?;
    match command {
        SchemeCommand::Add {
            name,
            file,
            note,
            force,
        } => {
            let scheme = read_scheme(&file)?;
            let json = serde_json::to_string(&scheme)
                .map_err(|e| fail(format!("the scheme will not serialize: {e}")))?;
            let store = registry.store();
            let exists = store
                .query_opt(
                    &format!(
                        "SELECT id FROM {} WHERE name = {}",
                        store.qualified("session_scheme"),
                        store.dialect().param(1, Type::Text)
                    ),
                    &[Param::Text(name.clone())],
                )
                .map_err(|e| fail(e.to_string()))?;
            if exists.is_some() {
                if !force {
                    return Err(fail(format!(
                        "a scheme called {name} is already kept; --force replaces it, and every \
                         label derived with the old one changes"
                    )));
                }
                store
                    .execute(
                        &format!(
                            "UPDATE {} SET definition = {}, note = {}, created_at = {}, digest = {} WHERE name = {}",
                            store.qualified("session_scheme"),
                            store.dialect().param(1, Type::Json),
                            store.dialect().param(2, Type::Text),
                            store.dialect().param(3, Type::Timestamp),
                            store.dialect().param(4, Type::Text),
                            store.dialect().param(5, Type::Text),
                        ),
                        &[
                            Param::Text(json),
                            match &note {
                                Some(n) => Param::Text(n.clone()),
                                None => Param::Null,
                            },
                            Param::Text(nils_registry::time::now_iso()),
                            Param::Text(scheme.digest()),
                            Param::Text(name.clone()),
                        ],
                    )
                    .map_err(|e| fail(e.to_string()))?;
                println!("scheme {name} replaced");
                return Ok(());
            }
            store
                .insert(
                    &Insert::new(
                        table("session_scheme"),
                        &["name", "definition", "created_at", "note", "digest"],
                    ),
                    &[vec![
                        Param::Text(name.clone()),
                        Param::Text(json),
                        Param::Text(nils_registry::time::now_iso()),
                        match &note {
                            Some(n) => Param::Text(n.clone()),
                            None => Param::Null,
                        },
                        Param::Text(scheme.digest()),
                    ]],
                )
                .map_err(|e| fail(e.to_string()))?;
            println!("scheme {name} kept");
            Ok(())
        }
        SchemeCommand::List => {
            let store = registry.store();
            let rows = store
                .query(
                    &format!(
                        "SELECT name, created_at, note FROM {} ORDER BY name",
                        store.qualified("session_scheme")
                    ),
                    &[],
                )
                .map_err(|e| fail(e.to_string()))?;
            if rows.is_empty() {
                println!("no schemes kept; `nils session list` uses the default");
                return Ok(());
            }
            for r in &rows {
                let name = r.text(0).unwrap_or_default();
                let when = r.text(1).unwrap_or_default();
                let note = r.opt_text(2).ok().flatten().unwrap_or("");
                println!("{name:20} {when:20} {note}");
            }
            Ok(())
        }
        SchemeCommand::Show { name, json } => {
            let scheme = stored_scheme(&mut registry, &name)?;
            let text = if json {
                serde_json::to_string_pretty(&scheme)
            } else {
                serde_json::to_string_pretty(&serde_json::json!({ "session": scheme }))
            }
            .map_err(|e| fail(format!("will not serialize: {e}")))?;
            println!("{text}");
            Ok(())
        }
        SchemeCommand::Remove { name } => {
            let store = registry.store();
            let n = store
                .execute(
                    &format!(
                        "DELETE FROM {} WHERE name = {}",
                        store.qualified("session_scheme"),
                        store.dialect().param(1, Type::Text)
                    ),
                    &[Param::Text(name.clone())],
                )
                .map_err(|e| fail(e.to_string()))?;
            if n == 0 {
                return Err(fail(format!("no scheme called {name}")));
            }
            println!("scheme {name} forgotten");
            Ok(())
        }
    }
}

/// One study as the resolver needs it, with the subject it belongs to.
struct Point {
    code: String,
    study: session::Study,
}

fn session_list(home: &Home, args: SessionListArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let scheme = match (&args.scheme, &args.name) {
        (Some(path), _) => read_scheme(path)?,
        (None, Some(name)) => stored_scheme(&mut registry, name)?,
        (None, None) => session::Scheme::default(),
    };
    let anchors = match &args.anchors {
        Some(path) => read_anchors(path)?,
        None => BTreeMap::new(),
    };
    // Wave 4a §7.3: a scheme anchored on a kind of event takes month zero
    // from the clinical layer, the earliest event of that kind per subject.
    let event_anchors: BTreeMap<String, Day> = match (scheme.anchor, &scheme.event) {
        (session::Anchor::Event, Some(kind_name)) => {
            let mut registry = open(home)?;
            let kind = nils_registry::clinical::kind_named(registry.store(), kind_name)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| {
                    usage(format!(
                        "session.event names {kind_name}, which is not an observation kind the \
                         registry holds; load the vocabulary, or name one of its kinds"
                    ))
                })?;
            nils_registry::clinical::anchor_events(registry.store(), kind.id)
                .map_err(|e| fail(e.to_string()))?
                .into_iter()
                .collect()
        }
        _ => BTreeMap::new(),
    };
    if scheme.anchor == session::Anchor::Explicit && anchors.is_empty() {
        return Err(usage(
            "anchor `explicit` needs --anchors FILE, a CSV of `code,date`",
        ));
    }

    let said = match &scheme.said {
        Some(spec) => Some((
            spec.segment,
            match &spec.pattern {
                Some(p) => Some(
                    regex::Regex::new(p)
                        .map_err(|e| usage(format!("session.said.pattern: {e}")))?,
                ),
                None => None,
            },
        )),
        None => None,
    };
    let points = read_points(&mut registry, args.subject.as_deref(), said.as_ref())?;

    // One subject at a time, because a scheme is a statement about a subject's
    // timeline and a label is meaningless across subjects.
    let mut by_subject: BTreeMap<String, Vec<session::Study>> = BTreeMap::new();
    for p in points {
        by_subject.entry(p.code).or_default().push(p.study);
    }

    let mut out: Vec<serde_json::Value> = Vec::new();
    let (mut n_sessions, mut n_flagged) = (0u64, 0u64);
    for (code, studies) in &by_subject {
        let anchor = match scheme.anchor {
            session::Anchor::FirstSession => studies.iter().map(|s| s.day).min(),
            session::Anchor::Explicit => anchors.get(code).copied(),
            session::Anchor::Event => event_anchors.get(code).copied(),
            // Resolved from the labels inside the resolver, and refused above.
            session::Anchor::SourceLabel => None,
        };
        for s in session::sessions(studies, anchor, &scheme) {
            n_sessions += 1;
            if s.flagged {
                n_flagged += 1;
            }
            if args.flagged && !s.flagged {
                continue;
            }
            out.push(serde_json::json!({
                "subject": code,
                "label": s.label,
                "first": s.first.to_string(),
                "last": s.last.to_string(),
                "studies": s.studies.len(),
                "months": s.months,
                "nominal": s.nominal,
                "offset_months": s.offset_months.map(|m| (m * 100.0).round() / 100.0),
                "flagged": s.flagged,
                "reason": s.reason.map(|r| r.name()),
                "has_primary": s.has_primary,
            }));
        }
    }

    if args.json {
        let doc = serde_json::json!({
            "scheme": scheme,
            "subjects": by_subject.len(),
            "sessions": n_sessions,
            "flagged": n_flagged,
            "rows": out,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&doc)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }

    // The code's length is the pseudonym scheme's, which differs per registry,
    // so the column is as wide as the codes rather than a guess that truncates.
    let wide = out
        .iter()
        .filter_map(|r| r["subject"].as_str().map(str::len))
        .max()
        .unwrap_or(0)
        .max("subject".len());
    println!(
        "{:<wide$} {:<8} {:<11} {:<11} {:>7} {:>8}  note",
        "subject", "label", "from", "to", "studies", "months"
    );
    for row in &out {
        let text = |k: &str| row[k].as_str().unwrap_or("-").to_string();
        let months = match row["offset_months"].as_f64() {
            Some(m) => format!("{m:.1}"),
            None => "-".into(),
        };
        let mut note = match row["reason"].as_str() {
            Some(r) => r.to_string(),
            None if row["months"].is_number() && row["nominal"].is_null() => "off schedule".into(),
            None => String::new(),
        };
        // §6. Said here because it is a property of the occasion, and this is
        // the command that derives occasions: a session with no primary
        // anywhere is one whose secondaries are all it has.
        if let Some(what) = match row["has_primary"].as_bool() {
            Some(false) => Some("rescue"),
            None => Some("primary unknown"),
            Some(true) => None,
        } {
            if !note.is_empty() {
                note.push_str(", ");
            }
            note.push_str(what);
        }
        let line = format!(
            "{:<wide$} {:<8} {:<11} {:<11} {:>7} {:>8}  {}",
            text("subject"),
            row["label"].as_str().unwrap_or("-"),
            text("first"),
            text("last"),
            row["studies"].as_u64().unwrap_or(0),
            months,
            note
        );
        println!("{}", line.trim_end());
    }
    let subjects = by_subject.len() as u64;
    println!(
        "{subjects} {}, {n_sessions} {}, {n_flagged} worth a look",
        counted("subjects", subjects),
        counted("sessions", n_sessions)
    );
    Ok(())
}

/// Month zero per subject, from a CSV of `code,date`.
fn read_anchors(path: &Path) -> Result<BTreeMap<String, Day>, Exit> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .map_err(|e| usage(format!("--anchors {}: {e}", path.display())))?;
    let mut out = BTreeMap::new();
    for (i, record) in reader.records().enumerate() {
        let record = record.map_err(|e| usage(format!("--anchors line {}: {e}", i + 1)))?;
        let (Some(code), Some(date)) = (record.get(0), record.get(1)) else {
            return Err(usage(format!(
                "--anchors line {}: a row is `code,date`",
                i + 1
            )));
        };
        let code = code.trim();
        // A header, if there is one.
        if i == 0 && code.eq_ignore_ascii_case("code") {
            continue;
        }
        let Some(day) = Day::parse(date) else {
            return Err(usage(format!(
                "--anchors line {}: {date} is not a date",
                i + 1
            )));
        };
        out.insert(code.to_string(), day);
    }
    Ok(out)
}

/// Every study with a date, with what the source called its visit.
///
/// A study whose date the vote could not settle is left out: it is not a point
/// on a timeline, and putting it on one would mean guessing.
fn read_points(
    registry: &mut Registry,
    subject: Option<&str>,
    said: Option<&(usize, Option<regex::Regex>)>,
) -> Result<Vec<Point>, Exit> {
    let store = registry.store();
    let (study, subject_table) = (store.qualified("study"), store.qualified("subject"));
    let mut params: Vec<Param> = Vec::new();
    let mut where_subject = String::new();
    if let Some(code) = subject {
        where_subject = format!(" AND su.code = {}", store.dialect().param(1, Type::Text));
        params.push(Param::Text(code.to_string()));
    }
    // The path is joined in only when the scheme asks for a source label: it
    // is three tables deep, and most schemes do not read it.
    let sql = if said.is_some() {
        format!(
            "SELECT su.code, st.id, COALESCE(st.date_filled, st.study_date), MIN(sf.path), \
             MAX(st.has_original_primary) \
             FROM {study} st JOIN {subject_table} su ON su.id = st.subject_id \
             JOIN {series} se ON se.study_id = st.id \
             JOIN {instance} i ON i.series_id = se.id \
             JOIN {source_file} sf ON sf.instance_id = i.id \
             WHERE COALESCE(st.date_filled, st.study_date) IS NOT NULL{where_subject} \
             GROUP BY su.code, st.id, COALESCE(st.date_filled, st.study_date) \
             ORDER BY su.code, 3, st.id",
            series = store.qualified("series"),
            instance = store.qualified("instance"),
            source_file = store.qualified("source_file"),
        )
    } else {
        format!(
            // The cast is for Postgres, which will not infer a type for a bare
            // NULL and refuses the statement rather than guessing.
            "SELECT su.code, st.id, COALESCE(st.date_filled, st.study_date), CAST(NULL AS TEXT), \
             st.has_original_primary \
             FROM {study} st JOIN {subject_table} su ON su.id = st.subject_id \
             WHERE COALESCE(st.date_filled, st.study_date) IS NOT NULL{where_subject} \
             ORDER BY su.code, 3, st.id"
        )
    };
    let rows = store
        .query(&sql, &params)
        .map_err(|e| fail(e.to_string()))?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let code = r.text(0).map_err(|e| fail(e.to_string()))?.to_string();
        let id = r.int(1).map_err(|e| fail(e.to_string()))?;
        let date = r.text(2).map_err(|e| fail(e.to_string()))?;
        let Some(day) = Day::parse(date) else {
            continue;
        };
        let path = r.opt_text(3).ok().flatten().unwrap_or("");
        out.push(Point {
            code,
            study: session::Study {
                id,
                day,
                said: said
                    .and_then(|(segment, pattern)| label_in(path, *segment, pattern.as_ref())),
                // Null is not no: a study whose stacks are not all
                // fingerprinted has not said it holds no primary.
                has_primary: r.opt_int(4).ok().flatten().map(|v| v != 0),
            },
        });
    }
    Ok(out)
}

/// The source's own label, out of one segment of a path.
///
/// The filename is dropped first, as the identity rule drops it: a segment
/// number counts directories, so that adding a file does not shift it.
fn label_in(path: &str, segment: usize, pattern: Option<&regex::Regex>) -> Option<String> {
    let dirs: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    let text = dirs.get(segment - 1)?;
    match pattern {
        None => Some((*text).to_string()),
        Some(re) => Some(re.captures(text)?.name("label")?.as_str().to_string()),
    }
}

// --------------------------------------------------------------------------
// Picks (Wave 3 §10)
// --------------------------------------------------------------------------

fn pick_command(home: &Home, command: PickCommand) -> Result<(), Exit> {
    match command {
        PickCommand::Run(args) => pick_run(home, args),
        PickCommand::List {
            role,
            borders,
            subject,
            json,
        } => pick_list(home, role, borders, subject, json),
        PickCommand::Explain { id, json } => pick_explain(home, id, json),
    }
}

fn pick_run(home: &Home, args: PickArgs) -> Result<(), Exit> {
    let dir = pack_dir(home, args.pack_dir)?;
    let found = packs_in(&dir)?
        .into_iter()
        .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()))
        .ok_or_else(|| fail(format!("no pack named {} in {}", args.pack, dir.display())))?;
    let overlay = load_overlay(args.overlay.as_ref())?;
    let pack = nils_pack::load(&found, overlay.as_ref()).map_err(|e| fail(e.to_string()))?;
    if pack.picks.is_empty() {
        return Err(fail(format!(
            "{} declares no picks, so there is nothing to choose",
            pack.id()
        )));
    }

    let mut registry = open(home)?;
    // A pick is made under a session scheme, because a session is derived and
    // the same studies are one occasion or two depending on it. The row says
    // which, so a pick made under one scheme is not mistaken for one made
    // under another.
    let scheme = match (&args.scheme, &args.scheme_name) {
        (Some(path), _) => read_scheme(path)?,
        (None, Some(name)) => stored_scheme(&mut registry, name)?,
        (None, None) => session::Scheme::default(),
    };

    let report = nils_classify::picking::run(
        &mut registry,
        &pack,
        &scheme,
        args.subject.as_deref(),
        &format!("pick:{}", pack.id()),
    )
    .map_err(|e| fail(e.to_string()))?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }
    println!("pick with {} against {}", pack.id(), report.reference);
    println!("  occasions        {:>12}", report.sessions);
    println!("  picked           {:>12}", report.written);
    println!("  nothing eligible {:>12}", report.empty);
    for (why, n) in &report.borders {
        println!("  {why:<16} {n:>12}   worth a person's eye");
    }
    println!("  {:.1} s", report.seconds);
    Ok(())
}

fn pick_list(
    home: &Home,
    role: Option<String>,
    borders: bool,
    subject: Option<String>,
    json: bool,
) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let mut where_parts = vec!["p.withdrawn_at IS NULL".to_string()];
    if let Some(r) = &role {
        where_parts.push(format!("p.role = '{}'", r.replace('\'', "''")));
    }
    if borders {
        where_parts.push("p.borders IS NOT NULL".to_string());
    }
    if let Some(c) = &subject {
        where_parts.push(format!("su.code = '{}'", c.replace('\'', "''")));
    }
    let sql = format!(
        "SELECT p.id, su.code, p.role, {}, p.score, p.margin, p.borders, p.actor, \
                p.author_kind, COUNT(ps.stack_id) \
         FROM {} p JOIN {} su ON su.id = p.subject_id \
         LEFT JOIN {} ps ON ps.pick_id = p.id \
         WHERE {} \
         GROUP BY p.id, su.code, p.role, {}, p.score, p.margin, p.borders, p.actor, p.author_kind \
         ORDER BY su.code, {}, p.role",
        text_of(store, "pick", "session_day"),
        store.qualified("pick"),
        store.qualified("subject"),
        store.qualified("pick_stack"),
        where_parts.join(" AND "),
        text_of(store, "pick", "session_day"),
        text_of(store, "pick", "session_day"),
    );
    let rows = store.query(&sql, &[]).map_err(|e| fail(e.to_string()))?;

    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.int(0).unwrap_or(0),
                    "subject": r.text(1).unwrap_or_default(),
                    "role": r.text(2).unwrap_or_default(),
                    "session": r.text(3).unwrap_or_default(),
                    "score": r.double(4).unwrap_or(0.0),
                    "margin": r.double(5).unwrap_or(0.0),
                    "borders": r.opt_text(6).ok().flatten(),
                    "actor": r.text(7).unwrap_or_default(),
                    "author_kind": r.text(8).unwrap_or_default(),
                    "stacks": r.int(9).unwrap_or(0),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&out)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }
    let wide = rows
        .iter()
        .filter_map(|r| r.text(1).ok().map(str::len))
        .max()
        .unwrap_or(0)
        .max("subject".len());
    println!(
        "{:>6}  {:<wide$} {:<6} {:<11} {:>6} {:>7} {:>7}  note",
        "id", "subject", "role", "session", "score", "margin", "stacks"
    );
    for r in &rows {
        let note = match r.opt_text(6).ok().flatten() {
            Some(b) => b.to_string(),
            None => String::new(),
        };
        // §10.1: whose pick it is, said here because a person's overrules the
        // agent's and a reader should not have to guess which they are seeing.
        let who = r.text(8).unwrap_or_default();
        let note = if who == "agent" {
            note
        } else {
            format!(
                "{note}{}by a {who}",
                if note.is_empty() { "" } else { ", " }
            )
        };
        println!(
            "{:>6}  {:<wide$} {:<6} {:<11} {:>6.3} {:>7.3} {:>7}  {}",
            r.int(0).unwrap_or(0),
            r.text(1).unwrap_or_default(),
            r.text(2).unwrap_or_default(),
            r.text(3).unwrap_or_default(),
            r.double(4).unwrap_or(0.0),
            r.double(5).unwrap_or(0.0),
            r.int(9).unwrap_or(0),
            note.trim_end()
        );
    }
    println!("{}", counted_line(rows.len() as u64, "picks"));
    Ok(())
}

fn counted_line(n: u64, noun: &str) -> String {
    format!("{n} {}", counted(noun, n))
}

/// Why one pick came out the way it did: every component, and what it read.
///
/// v0 computes the same breakdown and keeps it only in the response of the
/// request that asked, so a pick made last year cannot be explained at all.
fn pick_explain(home: &Home, id: i64, json: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT p.role, su.code, {}, p.score, p.margin, p.runner_up_score, p.borders, \
                {}, {}, p.reference, p.scheme, p.pack, p.pack_version, p.actor, p.author_kind \
         FROM {} p JOIN {} su ON su.id = p.subject_id WHERE p.id = {}",
        text_of(store, "pick", "session_day"),
        text_of(store, "pick", "parts"),
        text_of(store, "pick", "considered"),
        store.qualified("pick"),
        store.qualified("subject"),
        d.param(1, Type::Int),
    );
    let row = store
        .query_opt(&sql, &[Param::Int(id)])
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail(format!("no pick {id}")))?;
    let parts: serde_json::Value = row
        .opt_text(7)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(serde_json::Value::Null);
    let considered: serde_json::Value = row
        .opt_text(8)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(serde_json::Value::Null);
    let stacks = store
        .query(
            &format!(
                "SELECT stack_id FROM {} WHERE pick_id = {} ORDER BY stack_id",
                store.qualified("pick_stack"),
                store.dialect().param(1, Type::Int)
            ),
            &[Param::Int(id)],
        )
        .map_err(|e| fail(e.to_string()))?;
    let chosen: Vec<i64> = stacks.iter().filter_map(|r| r.int(0).ok()).collect();

    if json {
        let v = serde_json::json!({
            "id": id,
            "role": row.text(0).unwrap_or_default(),
            "subject": row.text(1).unwrap_or_default(),
            "session": row.text(2).unwrap_or_default(),
            "score": row.double(3).unwrap_or(0.0),
            "margin": row.double(4).unwrap_or(0.0),
            "runner_up_score": row.double(5).unwrap_or(0.0),
            "borders": row.opt_text(6).ok().flatten(),
            "reference": row.text(9).unwrap_or_default(),
            "scheme": row.text(10).unwrap_or_default(),
            "pack": format!("{}@{}", row.text(11).unwrap_or_default(), row.text(12).unwrap_or_default()),
            "actor": row.text(13).unwrap_or_default(),
            "author_kind": row.text(14).unwrap_or_default(),
            "stacks": chosen,
            "parts": parts,
            "considered": considered,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&v)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }

    println!(
        "pick {id}: the {} of {} on {}",
        row.text(0).unwrap_or_default(),
        row.text(1).unwrap_or_default(),
        row.text(2).unwrap_or_default()
    );
    println!(
        "  chosen           {}   scored {:.3}, ahead by {:.1}%",
        chosen
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        row.double(3).unwrap_or(0.0),
        row.double(4).unwrap_or(0.0) * 100.0
    );
    println!(
        "  by               {} ({})",
        row.text(13).unwrap_or_default(),
        row.text(14).unwrap_or_default()
    );
    // The two things v0 does not record, and without which a pick cannot be
    // reproduced: which population the cohort-relative components read, and
    // which scheme decided what the occasion was.
    println!("  against          {}", row.text(9).unwrap_or_default());
    println!("  session scheme   {}", row.text(10).unwrap_or_default());
    if let Some(b) = row.opt_text(6).ok().flatten() {
        println!("  worth a look     {b}");
    }
    if let Some(list) = parts["parts"].as_array() {
        println!("  what decided it");
        for p in list {
            println!(
                "      {:<9} {:.2} x {:.2} = {:.3}   {}",
                p["name"].as_str().unwrap_or(""),
                p["score"].as_f64().unwrap_or(0.0),
                p["weight"].as_f64().unwrap_or(0.0),
                p["score"].as_f64().unwrap_or(0.0) * p["weight"].as_f64().unwrap_or(0.0),
                p["saw"].as_str().unwrap_or("")
            );
        }
        if let Some(m) = parts["penalty"].as_f64()
            && m != 1.0
        {
            println!("      {:<9} x{m}", "penalty");
        }
    }
    if let Some(list) = considered.as_array()
        && list.len() > 1
    {
        println!("  what else was there");
        for c in list.iter().skip(1) {
            println!(
                "      {:.3}   stacks {}",
                c["score"].as_f64().unwrap_or(0.0),
                c["stacks"]
            );
        }
    }
    Ok(())
}

// --------------------------------------------------------------------------
/// The selection's items, from the flags and from `--select`, in the one
/// grammar `nils select` and `nils release` share (Wave 4a section 8).
///
/// A release takes a selection and does not compute one: what a study means
/// by its cohort is a query, and the query AST is the door for that. So this
/// reads lists, from flags or from a file, and `-` is standard input so the
/// answer to a query can be piped in rather than typed.
fn selection_items(
    subjects: &[String],
    sessions: &[String],
    stacks: &[i64],
    cohorts: &[String],
    axes: &[String],
    select: Option<&str>,
) -> Result<Vec<nils_release::select::Item>, Exit> {
    use nils_release::select;
    let mut out: Vec<select::Item> = Vec::new();
    for name in cohorts {
        out.push(select::Item::Cohort(name.clone()));
    }
    for s in subjects {
        out.push(select::Item::Subject(s.clone()));
    }
    for text in sessions {
        match select::Item::parse(text).map_err(usage)? {
            Some(item @ select::Item::Session(..)) => out.push(item),
            _ => {
                return Err(usage(format!(
                    "{text} is not a session; those are <subject>:<label>, as `nils session list` prints them"
                )));
            }
        }
    }
    for id in stacks {
        out.push(select::Item::Stack(*id));
    }
    for text in axes {
        match select::Item::parse(text).map_err(usage)? {
            Some(item @ select::Item::Axis(..)) => out.push(item),
            _ => {
                return Err(usage(format!(
                    "{text} is not an axis; those are <axis>=<value>"
                )));
            }
        }
    }
    let Some(from) = select else {
        return Ok(out);
    };
    let text = match from {
        "-" => {
            let mut buffer = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer)
                .map_err(|e| fail(format!("standard input: {e}")))?;
            buffer
        }
        path => std::fs::read_to_string(path).map_err(|e| fail(format!("{path}: {e}")))?,
    };
    out.extend(select::Item::parse_all(&text).map_err(|e| usage(format!("the selection: {e}")))?);
    Ok(out)
}

/// Resolve the items, and refuse if any did not: a release that releases
/// less than it was asked for, silently, is the failure mode this exists
/// to prevent.
fn resolve_or_refuse(
    registry: &mut Registry,
    items: &[nils_release::select::Item],
    pack: Option<&nils_pack::pack::Pack>,
) -> Result<nils_release::select::Resolved, Exit> {
    use nils_release::{run, select};
    let resolved = select::resolve(registry, items, pack).map_err(|e| match e {
        run::Error::Refused(m) => usage(m),
        other => fail(other.to_string()),
    })?;
    if !resolved.unresolved.is_empty() {
        let mut lines = vec![format!(
            "{} item(s) of the selection did not resolve:",
            resolved.unresolved.len()
        )];
        for (item, why) in &resolved.unresolved {
            lines.push(format!("  {:<24} {why}", item.describe()));
        }
        lines
            .push("nothing was released; `nils select` shows what a selection reaches".to_string());
        return Err(usage(lines.join("\n")));
    }
    Ok(resolved)
}

/// `nils audit list` (Wave 4a section 9.2).
fn audit_list(
    home: &Home,
    principal: Option<String>,
    action: Option<String>,
    since: Option<String>,
    limit: usize,
    json: bool,
) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let rows = nils_registry::audit::list(
        registry.store(),
        &nils_registry::audit::Filter {
            principal,
            action,
            since,
            limit,
        },
    )?;
    if json {
        let doc: Vec<serde_json::Value> = rows.iter().map(|r| r.as_json()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no audit rows");
        return Ok(());
    }
    for r in &rows {
        println!(
            "  {:>6}  {:<20} {:<24} {:<18} {}",
            r.id,
            &r.at[..r.at.len().min(19)],
            r.principal,
            r.action,
            r.scope
        );
    }
    Ok(())
}

/// `nils jobs`: the one job model at the command line (Wave 4a section 9.1).
fn jobs_command(home: &Home, command: JobsCommand) -> Result<(), Exit> {
    use nils_registry::job::{self, State};
    let mut registry = open(home)?;
    let store = registry.store();
    let err = |e: job::Error| fail(e.to_string());
    match command {
        JobsCommand::List { all, limit, json } => {
            let jobs = job::list(store, all, limit).map_err(err)?;
            if json {
                let doc: Vec<serde_json::Value> = jobs.iter().map(job::Job::as_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?
                );
                return Ok(());
            }
            if jobs.is_empty() {
                println!(
                    "{}",
                    if all {
                        "no jobs"
                    } else {
                        "no job is queued or running; --all lists the finished ones"
                    }
                );
                return Ok(());
            }
            println!(
                "  {:>6}  {:<16} {:<11} {:<20} {:<20} name",
                "id", "kind", "state", "started", "last heard"
            );
            for j in &jobs {
                println!(
                    "  {:>6}  {:<16} {:<11} {:<20} {:<20} {}",
                    j.id,
                    j.kind,
                    j.state.name(),
                    &j.started_at[..j.started_at.len().min(19)],
                    j.heartbeat_at
                        .as_deref()
                        .map(|t| &t[..t.len().min(19)])
                        .unwrap_or(""),
                    j.name.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
        JobsCommand::Show { id, json } => {
            let Some(j) = job::show(store, id).map_err(err)? else {
                return Err(usage(format!("no job {id}")));
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&j.as_json()).map_err(|e| fail(e.to_string()))?
                );
                return Ok(());
            }
            println!("job {}  {}  {}", j.id, j.kind, j.state.name());
            if let Some(n) = &j.name {
                println!("  name        {n}");
            }
            println!("  started     {}", j.started_at);
            if let Some(t) = &j.heartbeat_at {
                println!("  last heard  {t}");
            }
            if let Some(t) = &j.finished_at {
                println!("  finished    {t}");
            }
            if let (Some(p), Some(h)) = (j.pid, &j.host) {
                println!("  process     {p} on {h}");
            }
            if let Some(e) = &j.error {
                println!("  error       {e}");
            }
            if let Some(p) = &j.progress {
                println!("  progress    {p}");
            }
            if let Some(argv) = j.argv() {
                println!(
                    "  command     nils {}",
                    argv.iter().skip(1).cloned().collect::<Vec<_>>().join(" ")
                );
            }
            Ok(())
        }
        JobsCommand::Cancel { id } => match job::request_cancel(store, id).map_err(err)? {
            None => Err(usage(format!("no job {id}"))),
            Some(State::Cancelling) => {
                println!("job {id} is asked to stop; it does at its next heartbeat");
                Ok(())
            }
            Some(State::Cancelled) => {
                println!("job {id} was queued and is cancelled");
                Ok(())
            }
            Some(other) => {
                println!("job {id} is {}; nothing to cancel", other.name());
                Ok(())
            }
        },
        JobsCommand::Resume { id } => {
            let Some(j) = job::show(store, id).map_err(err)? else {
                return Err(usage(format!("no job {id}")));
            };
            if !j.state.is_over() {
                return Err(usage(format!(
                    "job {id} is {}; a job resumes once it is over",
                    j.state.name()
                )));
            }
            let Some(argv) = j.argv().filter(|a| a.len() > 1) else {
                return Err(usage(format!(
                    "job {id} recorded no command line; it was made before this binary or by a door"
                )));
            };
            println!("running again: nils {}", argv[1..].join(" "));
            let status = std::process::Command::new(
                std::env::current_exe().map_err(|e| fail(e.to_string()))?,
            )
            .args(&argv[1..])
            .status()
            .map_err(|e| fail(format!("could not run the job again: {e}")))?;
            match status.code() {
                Some(0) => Ok(()),
                Some(c) => Err(Exit {
                    code: u8::try_from(c).unwrap_or(FAILED),
                    message: String::new(),
                }),
                None => Err(fail("the job was stopped by a signal")),
            }
        }
        JobsCommand::Prune { keep_days } => {
            let gone = job::prune(store, keep_days).map_err(err)?;
            audit(
                &mut registry,
                nils_registry::audit::Action::JobsPrune,
                serde_json::json!({ "pruned": gone, "keep_days": keep_days }),
                None,
            )?;
            println!("pruned {gone} finished job(s) older than {keep_days} day(s)");
            Ok(())
        }
        JobsCommand::Enqueue { name, command } => {
            let id = job::enqueue(store, &command, name.as_deref(), Some(&actor())).map_err(err)?;
            println!("queued job {id}: nils {}", command.join(" "));
            Ok(())
        }
        JobsCommand::Work { once, every } => {
            let worker = job::claim(
                store,
                &job::Claim {
                    kind: "worker",
                    name: "queue",
                    args: serde_json::json!({ "once": once }),
                },
            )
            .map_err(|e| match e {
                job::Error::Busy { .. } => Exit {
                    code: BUSY,
                    message: e.to_string(),
                },
                other => fail(other.to_string()),
            })?;
            let cancel = stop_on_signal()?;
            let mut ran = 0usize;
            let outcome = loop {
                if cancel.stop() {
                    break Ok(());
                }
                if job::beat(store, worker, Some(&serde_json::json!({ "ran": ran })))
                    .map_err(err)?
                    == job::Asked::Cancel
                {
                    break Ok(());
                }
                let Some(next) = job::next_queued(store).map_err(err)? else {
                    if once {
                        break Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_secs(every.max(1)));
                    continue;
                };
                if !job::take(store, next.id).map_err(err)? {
                    continue;
                }
                let argv = next.argv().unwrap_or_default();
                println!("job {}: nils {}", next.id, argv.join(" "));
                // The queued command line names no registry; the worker's
                // is the one it runs in.
                let status = std::process::Command::new(
                    std::env::current_exe().map_err(|e| fail(e.to_string()))?,
                )
                .arg("--registry")
                .arg(home.dir())
                .args(&argv)
                .env(job::ADOPT_VAR, next.id.to_string())
                .env("NILS_PRINCIPAL", next.principal().unwrap_or(&actor()))
                .status();
                // The verb adopted the row and finished it itself; the
                // worker writes the outcome only when the verb did not.
                let (state, error) = match status {
                    Ok(s) if s.success() => (State::Done, None),
                    Ok(s) => (State::Failed, Some(format!("exit status {s}"))),
                    Err(e) => (State::Failed, Some(e.to_string())),
                };
                let now_state = job::show(store, next.id).map_err(err)?.map(|j| j.state);
                if now_state.is_some_and(|s| !s.is_over()) || state == State::Failed {
                    job::finish(store, next.id, state, error.as_deref()).map_err(err)?;
                }
                ran += 1;
            };
            let _ = job::finish(store, worker, State::Done, None);
            outcome
        }
    }
}

/// `nils select`: what a selection reaches, without releasing it.
fn select_preview(home: &Home, args: SelectArgs) -> Result<(), Exit> {
    use nils_release::{run, select};
    let items = selection_items(
        &args.subject,
        &args.session,
        &args.stack,
        &args.cohort,
        &args.axis,
        args.select.as_deref(),
    )?;
    let pack = match args.pack_dir.clone().or_else(|| pack_dir(home, None).ok()) {
        Some(dir) => packs_in(&dir)
            .ok()
            .and_then(|found| {
                found
                    .into_iter()
                    .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()))
            })
            .and_then(|found| nils_pack::load(&found, None).ok()),
        None => None,
    };
    let mut registry = open(home)?;
    let resolved = select::resolve(&mut registry, &items, pack.as_ref()).map_err(|e| match e {
        run::Error::Refused(m) => usage(m),
        other => fail(other.to_string()),
    })?;
    let preview =
        run::preview(registry.store(), &resolved.selection).map_err(|e| fail(e.to_string()))?;
    if args.json {
        let mut doc = resolved.as_json();
        doc["reaches"] = serde_json::json!({
            "subjects": preview.subjects, "studies": preview.studies,
            "stacks": preview.stacks, "files": preview.files, "bytes": preview.bytes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?
        );
        return if resolved.unresolved.is_empty() {
            Ok(())
        } else {
            Err(fail(format!(
                "{} item(s) did not resolve",
                resolved.unresolved.len()
            )))
        };
    }
    if items.is_empty() {
        println!("no items: the selection is the whole registry");
    }
    for (item, how) in &resolved.items {
        println!("  {:<24} {}", item.describe(), how.describe());
    }
    for (item, why) in &resolved.unresolved {
        println!("  {:<24} NOT RESOLVED: {why}", item.describe());
    }
    if !resolved.selection.sessions.is_empty() {
        println!(
            "  {} session(s) are matched by label once the sessions are derived under the release's scheme",
            resolved.selection.sessions.len()
        );
    }
    println!(
        "reaches  subjects {}   studies {}   stacks {}   files {}   {}",
        preview.subjects,
        preview.studies,
        preview.stacks,
        preview.files,
        size_text(preview.bytes)
    );
    if resolved.unresolved.is_empty() {
        Ok(())
    } else {
        Err(fail(format!(
            "{} item(s) did not resolve",
            resolved.unresolved.len()
        )))
    }
}

/// A size a person reads.
fn size_text(bytes: i64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{bytes} B")
    }
}

/// `nils clinical cohort list`.
fn cohort_list(home: &Home, json: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let sql = format!(
        "SELECT c.id, c.name, c.owner, c.description, \
                (SELECT COUNT(*) FROM {} m WHERE m.cohort_id = c.id AND m.left_at IS NULL) \
         FROM {} c ORDER BY c.name",
        store.qualified("cohort_member"),
        store.qualified("cohort")
    );
    let mut rows = Vec::new();
    for r in store.query(&sql, &[]).map_err(|e| fail(e.to_string()))? {
        rows.push((
            r.int(0).map_err(|e| fail(e.to_string()))?,
            r.text(1).map_err(|e| fail(e.to_string()))?.to_string(),
            r.text(2).map_err(|e| fail(e.to_string()))?.to_string(),
            r.opt_text(3)
                .map_err(|e| fail(e.to_string()))?
                .map(str::to_string),
            r.int(4).map_err(|e| fail(e.to_string()))?,
        ));
    }
    if json {
        let doc: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, name, owner, description, members)| {
                serde_json::json!({
                    "id": id, "name": name, "owner": owner,
                    "description": description, "members": members,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no cohorts; `nils clinical import` with a cohort mapping makes one");
        return Ok(());
    }
    for (_, name, owner, description, members) in &rows {
        println!(
            "  {name:<24} {members:>6} member(s)   {owner}   {}",
            description.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

/// What private elements an archive carries (§8.4).
///
/// A private element means whatever its vendor decided, and nothing in the file
/// says what. So a release drops them all and an allowlist brings back the ones
/// a pack names, and this is how that list is chosen: from the archive, by
/// counting what is in it.
///
/// **Shapes, never values.** A creator, a block, an element, how many files
/// carry it, how long it is, whether it is printable and how much it varies.
/// Enough to judge whether an element is worth keeping, and safe to carry out
/// of a private host, which a survey that quoted values would not be.
fn private_survey(home: &Home, args: PrivateArgs) -> Result<(), Exit> {
    // The pack, when there is one: its dictionary names what the survey
    // found, and its ingest list says what is read already. A survey with
    // no pack at hand still counts, it just cannot name.
    let pack = match pack_dir(home, args.pack_dir.clone()) {
        Ok(dir) => packs_in(&dir)?
            .into_iter()
            .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()))
            .map(|found| nils_pack::load(&found, None).map_err(|e| fail(e.to_string())))
            .transpose()?,
        Err(e) if args.pack_dir.is_some() => return Err(e),
        Err(_) => None,
    };
    if args.measured {
        return private_measured(home, pack.as_ref(), args.json);
    }
    let root = args
        .root
        .as_deref()
        .ok_or_else(|| usage("a ROOT to survey"))?;
    let survey = nils_dicom::survey::walk(root, args.files, args.workers);
    let rows = survey.rows();
    let name_of = |r: &nils_dicom::survey::Row| -> Option<String> {
        pack.as_ref()
            .and_then(|p| p.dictionary.lookup(&r.creator, r.group, r.element))
            .map(|e| e.name.clone())
    };
    let ingested = |r: &nils_dicom::survey::Row| -> Option<String> {
        pack.as_ref().and_then(|p| {
            p.ingest
                .iter()
                .find(|i| {
                    i.group == r.group
                        && i.element == r.element
                        && i.creator.trim().eq_ignore_ascii_case(r.creator.trim())
                })
                .map(|i| i.name.clone())
        })
    };

    if args.suggest {
        // Pack entries, under the rule of §5.3, for what the pack does not
        // read yet. Copied into a pack's `private.yml` by a person, who
        // writes the `why`: a survey can say that an element varies, and
        // only a person can say what it is.
        let mut out = String::new();
        out.push_str("# Suggested by `nils private --suggest` (Wave 4a section 5.3): every\n");
        out.push_str("# element that varied per acquisition, was printable and was short in\n");
        out.push_str(&format!(
            "# a survey of {} file(s). Each `why` is to be written by a person.\n",
            survey.files
        ));
        out.push_str("private:\n  ingest:\n");
        let mut n = 0;
        for r in &rows {
            if !r.suggested() || ingested(r).is_some() {
                continue;
            }
            n += 1;
            out.push_str(&format!("    - creator: {}\n", yaml_text(&r.creator)));
            out.push_str(&format!("      group: 0x{:04X}\n", r.group));
            out.push_str(&format!("      element: 0x{:02X}\n", r.element));
            out.push_str(&format!("      name: {}\n", r.field_name()));
            match name_of(r) {
                Some(name) => out.push_str(&format!(
                    "      why: \"varies per acquisition in {} of {} files; the dictionary calls it {}\"\n",
                    r.files,
                    survey.files,
                    name.replace('"', "'")
                )),
                None => out.push_str(&format!(
                    "      why: \"varies per acquisition in {} of {} files; the dictionary has no name for it\"\n",
                    r.files, survey.files
                )),
            }
        }
        if n == 0 {
            out.push_str("    []\n");
        }
        print!("{out}");
        return Ok(());
    }

    if args.json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "creator": r.creator,
                    "group": format!("{:04X}", r.group),
                    "element": format!("{:02X}", r.element),
                    "address": r.address(),
                    "name": name_of(r),
                    "ingested_as": ingested(r),
                    "files": r.files,
                    "vr": r.vrs,
                    "shortest": r.shortest,
                    "longest": r.longest,
                    "printable": r.printable,
                    "distinct": r.distinct,
                    "varied": r.varied,
                    "suggested": r.suggested(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "files": survey.files,
                "with_private": survey.with_private,
                "orphans": survey.orphans,
                "orphans_by_group": survey.orphans_by_group.iter()
                    .map(|(g, n)| (format!("{g:04X}"), *n))
                    .collect::<std::collections::BTreeMap<_, _>>(),
                "pack": pack.as_ref().map(|p| p.id()),
                "elements": out,
            }))
            .map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }

    println!(
        "{} file(s) read, {} with private elements, {} distinct element(s){}",
        survey.files,
        survey.with_private,
        rows.len(),
        match &pack {
            Some(p) => format!(", named by {}", p.id()),
            None => ", no pack at hand to name them".to_string(),
        }
    );
    if survey.orphans > 0 {
        // Nothing can be done with these, and saying so is the point: an
        // element in a block no creator reserved cannot be addressed by name.
        println!(
            "  {} private element(s) in a block no creator reserved, which no allowlist can keep:",
            survey.orphans
        );
        let mut groups: Vec<(&u16, &u64)> = survey.orphans_by_group.iter().collect();
        groups.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (g, n) in groups.iter().take(12) {
            println!("    group {g:04X}  {n}");
        }
    }
    let mut creator = String::new();
    for r in &rows {
        if r.creator != creator {
            println!("\n{}", r.creator);
            creator = r.creator.clone();
        }
        // `varies` is most of what decides whether an element is worth
        // keeping: one value across an archive is a property of the scanner,
        // and one per file is a property of the acquisition.
        println!(
            "  ({:04X},xx{:02X})  {:>8} files  {:<6}  {:>4}-{:<6} bytes  {}  {:<10} {}{}",
            r.group,
            r.element,
            r.files,
            r.vrs,
            r.shortest,
            r.longest,
            match r.printable {
                true => "text  ",
                false => "binary",
            },
            match (r.varied, r.distinct) {
                (true, _) => "varies".to_string(),
                (false, 1) => "one value".to_string(),
                (false, n) => format!("{n} values"),
            },
            name_of(r).unwrap_or_default(),
            match ingested(r) {
                Some(field) => format!("  [ingested as {field}]"),
                None if r.suggested() => "  [suggested]".to_string(),
                None => String::new(),
            }
        );
    }
    Ok(())
}

/// One address as the registry measured it (Wave 4a §5.3).
struct Measured {
    address: String,
    /// Series that hold it.
    series: i64,
    /// Series whose files disagreed on it.
    varied: i64,
    /// Distinct values across series, counted by hash: shapes, never values.
    distinct: usize,
}

/// What the digests kept per series, read back as shapes: which addresses,
/// in how many series, varied within how many, distinct across how many.
///
/// This is the second half of the method that chooses the list. A survey
/// says what varies across an archive; only a digest can say whether it
/// varies **inside** a series, and an element that does is a per-image value
/// (a slice position, a window) and not a parameter of the acquisition,
/// however much it varies across the archive.
fn private_measured(home: &Home, pack: Option<&nils_pack::Pack>, json: bool) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    let series: i64 = store
        .query(
            &format!("SELECT COUNT(*) FROM {}", store.qualified("series")),
            &[],
        )
        .map_err(|e| fail(e.to_string()))?
        .first()
        .and_then(|r| r.int(0).ok())
        .unwrap_or(0);
    let t = nils_registry::schema::table("series_private");
    let elements = store
        .dialect()
        .text_of(t.column("elements").expect("series_private.elements"));
    let sql = format!(
        "SELECT {elements}, varied FROM {}",
        store.qualified("series_private")
    );
    let rows = store.query(&sql, &[]).map_err(|e| fail(e.to_string()))?;
    let mut held: std::collections::BTreeMap<String, Measured> = std::collections::BTreeMap::new();
    let mut seen: std::collections::HashMap<String, std::collections::HashSet<u64>> =
        std::collections::HashMap::new();
    for r in &rows {
        let map: std::collections::BTreeMap<String, String> = r
            .text(0)
            .ok()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default();
        for (address, value) in &map {
            let m = held.entry(address.clone()).or_insert_with(|| Measured {
                address: address.clone(),
                series: 0,
                varied: 0,
                distinct: 0,
            });
            m.series += 1;
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            value.hash(&mut h);
            seen.entry(address.clone()).or_default().insert(h.finish());
        }
        if let Ok(Some(varied)) = r.opt_text(1) {
            for address in varied.split(',').filter(|a| !a.is_empty()) {
                if let Some(m) = held.get_mut(address) {
                    m.varied += 1;
                }
            }
        }
    }
    for (address, set) in seen {
        if let Some(m) = held.get_mut(&address) {
            m.distinct = set.len();
        }
    }
    let mut out: Vec<&Measured> = held.values().collect();
    out.sort_by(|a, b| b.series.cmp(&a.series).then(a.address.cmp(&b.address)));
    // The address is `GGGGxxEE CREATOR`; the pack names it from the creator.
    let split = |address: &str| -> Option<(u16, u8, String)> {
        let (tag, creator) = address.split_once(' ')?;
        let group = u16::from_str_radix(tag.get(0..4)?, 16).ok()?;
        let element = u8::from_str_radix(tag.get(6..8)?, 16).ok()?;
        Some((group, element, creator.to_string()))
    };
    let named = |address: &str| -> (Option<String>, Option<String>) {
        let Some((group, element, creator)) = split(address) else {
            return (None, None);
        };
        let Some(p) = pack else {
            return (None, None);
        };
        (
            p.dictionary
                .lookup(&creator, group, element)
                .map(|e| e.name.clone()),
            p.ingest
                .iter()
                .find(|i| {
                    i.group == group
                        && i.element == element
                        && i.creator.trim().eq_ignore_ascii_case(creator.trim())
                })
                .map(|i| i.name.clone()),
        )
    };
    if json {
        let items: Vec<serde_json::Value> = out
            .iter()
            .map(|m| {
                let (name, ingested) = named(&m.address);
                serde_json::json!({
                    "address": m.address,
                    "name": name,
                    "ingested_as": ingested,
                    "series": m.series,
                    "varied_within": m.varied,
                    "distinct_across": m.distinct,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "series": series,
                "series_with_private": rows.len(),
                "pack": pack.map(|p| p.id()),
                "elements": items,
            }))
            .map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    println!(
        "{} series, {} with private elements, {} address(es) measured{}",
        series,
        rows.len(),
        out.len(),
        match pack {
            Some(p) => format!(", named by {}", p.id()),
            None => String::new(),
        }
    );
    println!(
        "  {:<40} {:>8} {:>14} {:>9}  reading",
        "address", "series", "varied within", "distinct"
    );
    for m in &out {
        let (name, ingested) = named(&m.address);
        let share = match m.series {
            0 => 0.0,
            n => 100.0 * m.varied as f64 / n as f64,
        };
        // The reading is the rule of §5.3 applied: what the numbers say an
        // element is, for a person to confirm.
        let reading = if share >= 50.0 {
            "per image, not a parameter"
        } else if m.distinct <= 3 {
            "a constant of the scanner or the site"
        } else if share >= 5.0 {
            "mostly a parameter, varies inside some series"
        } else {
            "a parameter of the acquisition"
        };
        println!(
            "  {:<40} {:>8} {:>7} {:>5.1}% {:>9}  {}{}{}",
            m.address,
            m.series,
            m.varied,
            share,
            m.distinct,
            reading,
            match name {
                Some(n) => format!("  [{n}]"),
                None => String::new(),
            },
            match ingested {
                Some(f) => format!("  ingested as {f}"),
                None => String::new(),
            }
        );
    }
    Ok(())
}

/// A YAML scalar for a creator string, quoted when it needs to be.
fn yaml_text(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '.')
        && !s.starts_with(' ')
        && !s.ends_with(' ')
    {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

// The clinical layer (Wave 4a §7.1)
// --------------------------------------------------------------------------

fn clinical_command(home: &Home, command: ClinicalCommand) -> Result<(), Exit> {
    match command {
        ClinicalCommand::Import(args) => clinical_import(home, args),
        ClinicalCommand::Cohort(CohortCommand::List { json }) => cohort_list(home, json),
        ClinicalCommand::Vocabulary(VocabularyCommand::Load {
            file,
            pack_dir: dir,
            json,
        }) => {
            let path = match file {
                Some(f) => f,
                None => pack_dir(home, dir)?.join("clinical").join("vocabulary.yml"),
            };
            let text =
                fs::read_to_string(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let vocabulary = nils_registry::clinical::Vocabulary::parse(&text)
                .map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let mut registry = open(home)?;
            let loaded = nils_registry::clinical::load(registry.store(), &vocabulary)
                .map_err(|e| fail(e.to_string()))?;
            audit(
                &mut registry,
                nils_registry::audit::Action::VocabularyLoad,
                serde_json::json!({
                    "file": path.display().to_string(),
                    "diseases": {"added": loaded.diseases_added, "updated": loaded.diseases_updated},
                    "disease_types": {"added": loaded.disease_types_added, "updated": loaded.disease_types_updated},
                    "observation_types": {"added": loaded.observation_types_added, "updated": loaded.observation_types_updated},
                }),
                None,
            )?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "file": path.display().to_string(),
                        "diseases": {"added": loaded.diseases_added, "updated": loaded.diseases_updated},
                        "disease_types": {"added": loaded.disease_types_added, "updated": loaded.disease_types_updated},
                        "observation_types": {"added": loaded.observation_types_added, "updated": loaded.observation_types_updated},
                        "changed": loaded.changed(),
                    }))
                    .map_err(|e| fail(e.to_string()))?
                );
                return Ok(());
            }
            println!(
                "vocabulary {}: diseases {} added, {} updated; disease types {} added, {} updated; observation types {} added, {} updated{}",
                path.display(),
                loaded.diseases_added,
                loaded.diseases_updated,
                loaded.disease_types_added,
                loaded.disease_types_updated,
                loaded.observation_types_added,
                loaded.observation_types_updated,
                match loaded.changed() {
                    0 => "; nothing changed",
                    _ => "",
                }
            );
            Ok(())
        }
        ClinicalCommand::Vocabulary(VocabularyCommand::List { json }) => {
            let mut registry = open(home)?;
            let diseases = nils_registry::clinical::diseases(registry.store())
                .map_err(|e| fail(e.to_string()))?;
            let kinds = nils_registry::clinical::observation_types(registry.store())
                .map_err(|e| fail(e.to_string()))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "diseases": diseases.iter().map(|(id, d)| serde_json::json!({
                            "id": id, "name": d.name, "code": d.code, "description": d.description,
                            "types": d.types.iter().map(|t| serde_json::json!({
                                "name": t.name, "description": t.description,
                            })).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                        "observation_types": kinds.iter().map(|k| serde_json::json!({
                            "id": k.id, "name": k.name, "category": k.category,
                            "value_type": k.value_type, "unit": k.unit, "primary": k.primary,
                            "sensitive": k.sensitive,
                        })).collect::<Vec<_>>(),
                    }))
                    .map_err(|e| fail(e.to_string()))?
                );
                return Ok(());
            }
            println!(
                "{} disease(s), {} observation kind(s)",
                diseases.len(),
                kinds.len()
            );
            for (_, d) in &diseases {
                println!(
                    "  {:24} {:8} {}",
                    d.name,
                    d.code.as_deref().unwrap_or(""),
                    d.types
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            for k in &kinds {
                println!(
                    "  {:24} {:20} {:8} {}{}",
                    k.name,
                    k.category,
                    k.value_type.as_deref().unwrap_or("date"),
                    k.unit.as_deref().unwrap_or(""),
                    match (k.primary, k.sensitive) {
                        (true, true) => "  (primary, sensitive)",
                        (true, false) => "  (primary)",
                        (false, true) => "  (sensitive)",
                        (false, false) => "",
                    }
                );
            }
            Ok(())
        }
    }
}

/// One declarative importer (§7.2): a mapping and a file, previewed and then
/// applied under a principal, idempotent on the key the mapping names.
fn clinical_import(home: &Home, args: ClinicalImportArgs) -> Result<(), Exit> {
    let mapping_text = fs::read_to_string(&args.mapping)
        .map_err(|e| usage(format!("--mapping {}: {e}", args.mapping.display())))?;
    let mapping = nils_registry::import::Mapping::parse(&mapping_text)
        .map_err(|e| usage(format!("--mapping {}: {e}", args.mapping.display())))?;
    let csv_text = fs::read_to_string(&args.file)
        .map_err(|e| usage(format!("--file {}: {e}", args.file.display())))?;
    let mut registry = open(home)?;
    let who = actor();
    let report = match args.apply {
        true => nils_registry::import::apply(&mut registry, &mapping, &csv_text, &who),
        false => nils_registry::import::preview(&mut registry, &mapping, &csv_text, &who),
    }
    .map_err(|e| fail(e.to_string()))?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": report.target,
                "applied": report.applied,
                "rows": report.rows,
                "added": report.added,
                "skipped": report.skipped,
                "updated": report.updated,
                "superseded": report.superseded,
                "reviewed": report.reviewed,
                "refused": report.refused,
                "changes": report.changes(),
                "samples": report.samples.iter().map(|o| serde_json::json!({
                    "row": o.row,
                    "subject": o.subject,
                    "verdict": o.verdict.name(),
                    "detail": match &o.verdict {
                        nils_registry::import::Verdict::Refused(why) => Some(why.clone()),
                        nils_registry::import::Verdict::Reviewed { field } => Some(field.clone()),
                        _ => None,
                    },
                })).collect::<Vec<_>>(),
            }))
            .map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    println!(
        "{} {} rows into {}: {} added, {} skipped, {} updated, {} superseded, {} for review, {} refused{}",
        match report.applied {
            true => "applied",
            false => "would apply",
        },
        report.rows,
        report.target,
        report.added,
        report.skipped,
        report.updated,
        report.superseded,
        report.reviewed,
        report.refused_total(),
        match (report.applied, report.changes()) {
            (true, 0) => "; nothing changed",
            (false, 0) => "; nothing would change",
            _ => "",
        }
    );
    for (why, n) in &report.refused {
        println!("  refused {n}: {why}");
    }
    if !report.applied {
        for o in &report.samples {
            println!(
                "  row {:>5}  {:<12} {}{}",
                o.row,
                o.subject.as_deref().unwrap_or("-"),
                o.verdict.name(),
                match &o.verdict {
                    nils_registry::import::Verdict::Refused(why) => format!(": {why}"),
                    nils_registry::import::Verdict::Reviewed { field } =>
                        format!(": {field} disagrees with the registry"),
                    _ => String::new(),
                }
            );
        }
        if report.samples.len() < report.rows {
            println!("  ... and {} more", report.rows - report.samples.len());
        }
        println!("  (a preview; add --apply to write under {who})");
    }
    Ok(())
}

// The handover (Wave 3 §11)
// --------------------------------------------------------------------------

fn handover_command(home: &Home, command: HandoverCommand) -> Result<(), Exit> {
    match command {
        HandoverCommand::Run(args) => handover_run(home, *args),
        HandoverCommand::Verify(args) => handover_verify(home, args),
        HandoverCommand::List(args) => handover_list(home, args),
        HandoverCommand::Password(args) => {
            // Reading a password out is a deliberate act, so it is its own
            // verb rather than a line of some other command's output.
            let key = home
                .keys(None)
                .read(&args.key)
                .map_err(|e| fail(e.to_string()))?;
            println!("{}", nils_release::handover::password(&key));
            Ok(())
        }
    }
}

/// A size with a unit, because a chunk is quoted in gigabytes and typed once.
fn size_of(text: &str) -> Result<i64, Exit> {
    let cleaned = text.trim().to_ascii_uppercase().replace(' ', "");
    let (number, scale) = match cleaned.as_str() {
        t if t.ends_with("TB") || t.ends_with("TIB") => {
            (t.trim_end_matches(['T', 'I', 'B']), 1i64 << 40)
        }
        t if t.ends_with("GB") || t.ends_with("GIB") => {
            (t.trim_end_matches(['G', 'I', 'B']), 1i64 << 30)
        }
        t if t.ends_with("MB") || t.ends_with("MIB") => {
            (t.trim_end_matches(['M', 'I', 'B']), 1i64 << 20)
        }
        t if t.ends_with("KB") || t.ends_with("KIB") => {
            (t.trim_end_matches(['K', 'I', 'B']), 1i64 << 10)
        }
        t => (t.trim_end_matches('B'), 1),
    };
    let value: f64 = number
        .parse()
        .map_err(|_| usage(format!("{text} is not a size; try 100GB")))?;
    let bytes = (value * scale as f64) as i64;
    match bytes > 0 {
        true => Ok(bytes),
        false => Err(usage(format!("{text} is not a size a chunk can be"))),
    }
}

fn handover_run(home: &Home, args: HandoverArgs) -> Result<(), Exit> {
    use nils_release::handover::{archive, plan, run};

    let strategy = plan::Strategy::parse(&args.strategy).ok_or_else(|| {
        usage(format!(
            "--strategy is ordered or packed, not {}",
            args.strategy
        ))
    })?;
    if args.level > 9 {
        return Err(usage("--level is 0 to 9".to_string()));
    }
    let cap = size_of(&args.chunk)?;
    // §11, before anything is packed: the archiver, and the recovery records
    // if they were asked for. A handover that discovers a missing archiver
    // after writing 400 GB has written 400 GB for nothing.
    let archiver = archive::Archiver::find(&args.sevenzip).map_err(usage)?;
    let par2 = match args.par2 {
        Some(percent) => Some(archive::Par2::find(percent).map_err(usage)?),
        None => None,
    };
    let key = home
        .keys(None)
        .read(&args.key)
        .map_err(|e| fail(e.to_string()))?;
    let password = nils_release::handover::password(&key);
    let mut registry = open(home)?;

    let settings = run::Settings {
        release: &args.release,
        out: &args.out,
        archiver: &archiver,
        key_name: &args.key,
        password: &password,
        cap,
        strategy,
        level: args.level,
        par2,
        verify: !args.no_verify,
        actor: &actor(),
    };
    let report = run::run(&mut registry, &settings).map_err(|e| match e {
        run::Error::Refused(m) => usage(m),
        other => fail(other.to_string()),
    })?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return finish(&report);
    }
    println!(
        "handover {} version {} ({})",
        report.release, report.version, report.strategy
    );
    println!("  from             {}", report.root);
    println!("  into             {}", report.out);
    println!("  packed by        {}", report.tool);
    println!("  subjects         {:>12}", report.subjects);
    println!("  files            {:>12}", report.files);
    println!("  archives         {:>12}", report.archives);
    println!(
        "  {:>9.2} GiB of tree into {:.2} GiB of archives",
        report.bytes as f64 / (1u64 << 30) as f64,
        report.packed_bytes as f64 / (1u64 << 30) as f64
    );
    if report.verified > 0 {
        println!("  read back        {:>12}", report.verified);
    }
    // A file the record names and the disk does not: a handover of a tree
    // somebody edited is not a handover of the release.
    if report.missing > 0 {
        println!(
            "  {:>10}   file(s) the release wrote and the tree no longer holds",
            report.missing
        );
    }
    for why in &report.failed {
        println!("  failed           {why}");
    }
    println!("  {:.1} s", report.seconds);
    finish(&report)
}

/// A handover with a failed archive is a failed handover, whatever it wrote.
fn finish(report: &nils_release::handover::run::Report) -> Result<(), Exit> {
    match report.failed.is_empty() {
        true => Ok(()),
        false => Err(fail(format!(
            "{} archive(s) did not come out right",
            report.failed.len()
        ))),
    }
}

fn handover_verify(home: &Home, args: HandoverVerifyArgs) -> Result<(), Exit> {
    use nils_release::handover::{archive, run};

    let archiver = archive::Archiver::find(&args.sevenzip).map_err(usage)?;
    let key = home
        .keys(None)
        .read(&args.key)
        .map_err(|e| fail(e.to_string()))?;
    let password = nils_release::handover::password(&key);
    let mut registry = open(home)?;
    let report =
        run::verify(&mut registry, args.handover, &archiver, &password).map_err(|e| match e {
            run::Error::Refused(m) => usage(m),
            other => fail(other.to_string()),
        })?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return finish(&report);
    }
    println!("handover {}", report.handover_id);
    println!("  in               {}", report.out);
    println!("  archives         {:>12}", report.archives);
    println!("  read back        {:>12}", report.verified);
    for why in &report.failed {
        println!("  failed           {why}");
    }
    println!("  {:.1} s", report.seconds);
    finish(&report)
}

fn handover_list(home: &Home, args: HandoverListArgs) -> Result<(), Exit> {
    let mut registry = open(home)?;
    let store = registry.store();
    // Qualified, because `release` has a `started_at` too and an unqualified
    // one is ambiguous the moment the two are joined.
    let started = store.dialect().text_of_qualified(
        Some("h"),
        nils_registry::schema::table("handover")
            .column("started_at")
            .expect("handover.started_at is a column"),
    );
    let mut wheres = String::new();
    let mut params: Vec<nils_registry::store::Param> = Vec::new();
    if let Some(name) = &args.release {
        wheres = format!(
            " WHERE r.name = {}",
            store.dialect().param(1, nils_registry::schema::Type::Text)
        );
        params.push(nils_registry::store::Param::from(name.as_str()));
    }
    let sql = format!(
        "SELECT h.id, r.name, r.version, h.root, {started}, h.archives, h.files, \
                h.packed_bytes, h.key_name, h.error \
         FROM {} h JOIN {} r ON r.id = h.release_id{wheres} ORDER BY h.id",
        store.qualified("handover"),
        store.qualified("release"),
    );
    let rows = store
        .query(&sql, &params)
        .map_err(|e| fail(e.to_string()))?;
    if rows.is_empty() {
        println!("no handover yet");
        return Ok(());
    }
    if args.json {
        let mut out = Vec::new();
        for r in &rows {
            out.push(serde_json::json!({
                "id": r.int(0).map_err(|e| fail(e.to_string()))?,
                "release": r.text(1).map_err(|e| fail(e.to_string()))?,
                "version": r.text(2).map_err(|e| fail(e.to_string()))?,
                "archives": r.int(5).map_err(|e| fail(e.to_string()))?,
                "files": r.int(6).map_err(|e| fail(e.to_string()))?,
                "bytes": r.int(7).map_err(|e| fail(e.to_string()))?,
                "key": r.text(8).map_err(|e| fail(e.to_string()))?,
            }));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    for r in &rows {
        let id = r.int(0).map_err(|e| fail(e.to_string()))?;
        let name = r.text(1).map_err(|e| fail(e.to_string()))?;
        let version = r.text(2).map_err(|e| fail(e.to_string()))?;
        let when = r.text(4).map_err(|e| fail(e.to_string()))?;
        let when = when.split('T').next().unwrap_or(when);
        println!("{id:>4}  {name} {version}  {when}");
        println!(
            "        {} archive(s), {} file(s), {:.2} GiB, opened by key {}",
            r.int(5).map_err(|e| fail(e.to_string()))?,
            r.int(6).map_err(|e| fail(e.to_string()))?,
            r.int(7).map_err(|e| fail(e.to_string()))? as f64 / (1u64 << 30) as f64,
            r.text(8).map_err(|e| fail(e.to_string()))?,
        );
        println!(
            "        into {}",
            r.text(3).map_err(|e| fail(e.to_string()))?
        );
        // The first, and how many more: a set of a hundred archives that all
        // failed for one reason is one line, not a hundred.
        if let Ok(Some(error)) = r.opt_text(9) {
            let mut parts = error.split("; ");
            let first = parts.next().unwrap_or(error);
            match parts.count() {
                0 => println!("        failed: {first}"),
                more => println!("        failed: {first}, and {more} more"),
            }
        }
    }
    Ok(())
}

// The release (Wave 3 §8)
// --------------------------------------------------------------------------

fn release(home: &Home, args: ReleaseArgs) -> Result<(), Exit> {
    use nils_release::{dates, policy, run, tags, uid};

    if args.history {
        return history(home, &args);
    }
    if let Some(version) = &args.withdraw {
        let name = args.name.as_deref().ok_or_else(|| {
            usage("--withdraw needs --name, the release to withdraw a version of")
        })?;
        return release_withdraw(home, name, version, args.why.as_deref().unwrap_or(""));
    }
    let out = args.out.clone().expect("--out, or --history");
    let dates_policy = dates::Policy::parse(&args.dates).ok_or_else(|| {
        usage(format!(
            "--dates is keep, shift or year, not {}",
            args.dates
        ))
    })?;
    let uids = policy::Uids::parse(&args.uids)
        .ok_or_else(|| usage(format!("--uids is remap or preserve, not {}", args.uids)))?;
    let root = match &args.uid_root {
        Some(text) => uid::Root::new(text).map_err(|e| usage(e.to_string()))?,
        None => uid::Root::default(),
    };
    let policy = policy::Policy {
        dates: dates_policy,
        uids,
        root,
    };
    // §4.3, before a registry is even opened: the two policies are one, and a
    // combination that would leave the date in the UID is refused rather than
    // warned about.
    policy.check().map_err(|e| usage(e.to_string()))?;

    let categories = match &args.categories {
        None => tags::Category::every(),
        Some(text) => {
            let mut out = Vec::new();
            for name in text.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                out.push(tags::Category::parse(name).ok_or_else(|| {
                    usage(format!(
                        "{name} is not a category: patient, trial, provider, institution, times"
                    ))
                })?);
            }
            out
        }
    };

    let on_unknown = nils_release::burned::OnUnknown::parse(&args.on_unknown).ok_or_else(|| {
        usage(format!(
            "--on-unknown is hold or write, not {}",
            args.on_unknown
        ))
    })?;

    let layout = run::Layout::parse(&args.layout).ok_or_else(|| {
        usage(format!(
            "--layout is descriptive or bids, not {}",
            args.layout
        ))
    })?;
    let places = nils_release::bids::place::Options {
        localizers: nils_release::bids::place::Localizers::parse(&args.localizers).ok_or_else(
            || {
                usage(format!(
                    "--localizers is sourcedata, datatype, anat or drop, not {}",
                    args.localizers
                ))
            },
        )?,
        synthetic: nils_release::bids::place::Synthetic::parse(&args.synthetic).ok_or_else(
            || {
                usage(format!(
                    "--synthetic is anat or derivatives, not {}",
                    args.synthetic
                ))
            },
        )?,
    };
    // §9.6. Found once, before anything is written, and recorded on the run and
    // in `GeneratedBy`. v0 discovers a missing converter per stack, in a worker
    // process, as N identical failures.
    let converter = match layout {
        run::Layout::Bids => {
            Some(nils_release::bids::convert::Converter::find(&args.dcm2niix).map_err(usage)?)
        }
        run::Layout::Descriptive => None,
    };

    // Before the pack directory is taken out of `args`.
    let items = selection_items(
        &args.subject,
        &args.session,
        &args.stack,
        &args.cohort,
        &args.axis,
        args.select.as_deref(),
    )?;
    let dir = pack_dir(home, args.pack_dir)?;
    let found = packs_in(&dir)?
        .into_iter()
        .find(|p| p.file_name().is_some_and(|f| f == args.pack.as_str()))
        .ok_or_else(|| fail(format!("no pack named {} in {}", args.pack, dir.display())))?;
    let pack = nils_pack::load(&found, None).map_err(|e| fail(e.to_string()))?;

    let mut registry = open(home)?;
    // Wave 4a section 8: every item resolved up front, and a refusal that
    // names what did not, rather than a smaller release.
    let chosen = resolve_or_refuse(&mut registry, &items, Some(&pack))?.selection;
    let scheme = match (&args.scheme, &args.scheme_name) {
        (Some(path), _) => read_scheme(path)?,
        (None, Some(name)) => stored_scheme(&mut registry, name)?,
        (None, None) => session::Scheme::default(),
    };
    // The same key the pseudonyms were derived from: the release does not
    // choose a pseudonym of its own (§8.1), and its UID remapping hangs off a
    // domain of the same key so that neither can be used to reason about the
    // other.
    let key = registry.pseudonym_key().map_err(|e| fail(e.to_string()))?;

    let name = args
        .name
        .clone()
        .unwrap_or_else(nils_registry::time::now_iso);
    let settings = run::Settings {
        name: &name,
        root: &out,
        policy: &policy,
        categories,
        selection: run::Selection {
            subjects: chosen.subjects,
            sessions: chosen.sessions,
            stacks: chosen.stacks,
            dispositions: args.disposition.clone(),
            roles: args.role.clone(),
            picked_only: args.picked,
            modality: args.modality.clone(),
            axes: chosen.axes,
            cohorts: chosen.cohorts,
        },
        scheme: &scheme,
        // §8.4: dropped by default, and back only by name. The pack declares
        // the list, because which vendor element carries a gradient is
        // knowledge about scanners.
        private: &pack.release,
        on_unknown,
        actor: &actor(),
        key: &key,
        pack: &pack,
        layout,
        places,
        converter: converter.as_ref(),
        compress: !args.no_compress,
        authors: &args.author,
        observations: &args.observation,
    };
    let report = run::run(&mut registry, &settings).map_err(|e| match e {
        run::Error::Refused(m) => usage(m),
        other => fail(other.to_string()),
    })?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|e| fail(format!("will not serialize: {e}")))?
        );
        return Ok(());
    }
    println!(
        "release {} version {} ({}, {})",
        report.name, report.version, report.layout, report.policy
    );
    println!("  into             {}", report.root);
    if let Some(c) = &report.converter {
        println!("  converted by     {c}");
    }
    match &report.previous {
        Some(before) => println!("  since            {before}"),
        // The first version of a tree writes all of it, and saying so is
        // better than a line of counts that all read as "everything new".
        None => println!("  since            nothing; this is the first"),
    }
    println!("  subjects         {:>12}", report.subjects);
    println!("  stacks           {:>12}", report.stacks);
    println!("  files            {:>12}", report.files);
    println!(
        "  in the tree      {:>9.2} GiB",
        report.bytes as f64 / (1u64 << 30) as f64
    );
    // §8.6, and the first number is the one worth reading: a re-run after a
    // QC decision or a renamed technique should leave nearly everything alone.
    if report.previous.is_some() {
        println!("  this version");
        for (n, what) in [
            (report.unchanged, "stacks left alone"),
            (report.moved, "renamed, not rewritten"),
            (report.rewritten, "written again"),
            (report.added, "new"),
            (report.removed, "no longer in the release"),
        ] {
            if n > 0 {
                println!("      {n:>10}   {what}");
            }
        }
        println!("      {:>10}   files written", report.written);
    }
    // §8.4. The second number is the one worth reading: "no tag" is not "no
    // text", and an archive where most stacks are unjudgeable is a fact a
    // release should have to confront rather than average away.
    if report.burned_in > 0 || report.unjudged > 0 {
        println!("  held back");
        if report.burned_in > 0 {
            println!(
                "      {:>10}   stacks the file says carry text in their pixels",
                report.burned_in
            );
        }
        if report.unjudged > 0 {
            println!(
                "      {:>10}   stacks the file will not say either way, each a review item",
                report.unjudged
            );
        }
    }
    if !report.refused.is_empty() {
        println!("  not written");
        for (why, n) in &report.refused {
            println!("      {n:>10}   {why}");
        }
    }
    // §9.3. Less than half of a clinical archive has a BIDS name, and where
    // the rest went is the thing a reader of the tree has to be told.
    if !report.routes.is_empty() && report.layout == "bids" {
        println!("  where they went");
        for (route, n) in &report.routes {
            let what = match route.as_str() {
                "raw" => "in the tree, under a name the standard admits",
                "sourcedata" => "sourcedata/, as DICOM",
                "derivatives" => "derivatives/nils/, which BIDS has no word for",
                "beside" => "a directory of their own, in .bidsignore",
                "unofficial" => "the raw tree under a suffix BIDS lacks, in .bidsignore",
                "nowhere" => "nowhere",
                _ => route.as_str(),
            };
            println!("      {n:>10}   {what}");
        }
        for (why, n) in &report.nowhere {
            println!("      {n:>10}   nowhere: {why}");
        }
        for (why, n) in &report.unconvertible {
            println!("      {n:>10}   written as DICOM instead: {why}");
        }
        // Wave 4a §7.4: what the clinical layer put in the tree, counted.
        for (what, n) in &report.clinical {
            println!("      {n:>10}   clinical: {what}");
        }
        if report.restored > 0 {
            println!(
                "      {:>10}   written again: their files were gone from the tree",
                report.restored
            );
        }
        for (what, choice) in &report.placements {
            println!("  {what:<16} {choice}");
        }
    }
    // What was changed, by tag and action, and never what it was: an audit
    // that records what was removed is a copy of the identifiers in clear. A
    // version that wrote no file changed no tag, and says nothing here.
    if report.changes.is_empty() {
        println!("  {:.1} s", report.seconds);
        return Ok(());
    }
    println!("  what changed");
    let mut changes: Vec<(&String, &i64)> = report.changes.iter().collect();
    changes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (what, n) in changes.iter().take(8) {
        println!("      {n:>10}   {what}");
    }
    if changes.len() > 8 {
        println!("      {:>10}   more, in release_change", changes.len() - 8);
    }
    println!("  {:.1} s", report.seconds);
    Ok(())
}

/// `nils release --withdraw VERSION --why TEXT` (Wave 4a section 13.1): the
/// row stays, marked withdrawn by whom and why; the tree on disk is the
/// operator's to remove, and the handovers that shipped it stay recorded.
fn release_withdraw(home: &Home, name: &str, version: &str, why: &str) -> Result<(), Exit> {
    if why.trim().is_empty() {
        return Err(usage("--withdraw needs --why: a withdrawal has a reason"));
    }
    let who = actor();
    let mut registry = open(home)?;
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id, withdrawn_at IS NOT NULL FROM {} WHERE name = {} AND version = {}",
        store.qualified("release"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    let Some(row) = store.query_opt(&sql, &[Param::from(name), Param::from(version)])? else {
        return Err(usage(format!(
            "no release {name} version {version}; --history lists them"
        )));
    };
    let id = row.int(0)?;
    if row.int(1)? != 0 {
        return Err(fail(format!(
            "release {name} version {version} is already withdrawn"
        )));
    }
    let now = nils_registry::time::now_iso();
    store.update_by_id(
        table("release"),
        &[
            ("withdrawn_at", Param::from(now.as_str())),
            ("withdrawn_by", Param::from(who.as_str())),
            ("withdrawn_why", Param::from(why)),
        ],
        "id",
        id,
    )?;
    audit(
        &mut registry,
        nils_registry::audit::Action::ReleaseWithdraw,
        serde_json::json!({ "release": name, "version": version, "id": id }),
        Some(serde_json::json!({ "why": why })),
    )?;
    println!(
        "withdrew release {name} version {version} ({why}); the row stays, and the tree on disk is yours to remove"
    );
    Ok(())
}

/// The releases the registry holds, newest first (`GET /api/releases`).
fn releases_doc(registry: &mut Registry, limit: usize) -> Result<serde_json::Value, Exit> {
    let store = registry.store();
    let started = text_of(store, "release", "started_at");
    let withdrawn = text_of(store, "release", "withdrawn_at");
    let sql = format!(
        "SELECT id, name, version, root, {started}, files, subjects, unchanged, moved, rewritten, \
         added, removed, layout, actor, {withdrawn}, withdrawn_by, withdrawn_why FROM {} \
         ORDER BY id DESC LIMIT {}",
        store.qualified("release"),
        limit.max(1)
    );
    let rows: Vec<serde_json::Value> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "id": r.int(0)?,
                "name": r.text(1)?,
                "version": r.text(2)?,
                "root": r.text(3)?,
                "started_at": r.opt_text(4)?,
                "files": r.opt_int(5)?,
                "subjects": r.opt_int(6)?,
                "unchanged": r.opt_int(7)?,
                "moved": r.opt_int(8)?,
                "rewritten": r.opt_int(9)?,
                "added": r.opt_int(10)?,
                "removed": r.opt_int(11)?,
                "layout": r.opt_text(12)?,
                "actor": r.opt_text(13)?,
                "withdrawn_at": r.opt_text(14)?,
                "withdrawn_by": r.opt_text(15)?,
                "withdrawn_why": r.opt_text(16)?,
            }))
        })
        .collect::<Result<_, nils_registry::Error>>()?;
    Ok(serde_json::json!({ "count": rows.len(), "releases": rows }))
}

/// One row of the version history.
struct Version {
    id: i64,
    name: String,
    version: String,
    root: String,
    started: String,
    files: i64,
    /// Unchanged, moved, rewritten, added, removed, in that order.
    counts: [i64; 5],
}

/// Every version of one dataset, and what each did (§8.6).
///
/// The point of recording the changes rather than diffing two trees: a
/// recipient who took version 3 can be told what to fetch, and a version that
/// left everything alone says so.
fn history(home: &Home, args: &ReleaseArgs) -> Result<(), Exit> {
    let mut registry = home.open().map_err(|e| fail(e.to_string()))?;
    let store = registry.store();
    let mut wheres = String::new();
    let mut params: Vec<nils_registry::store::Param> = Vec::new();
    if let Some(name) = &args.name {
        wheres = format!(
            " WHERE name = {}",
            store.dialect().param(1, nils_registry::schema::Type::Text)
        );
        params.push(nils_registry::store::Param::from(name.as_str()));
    }
    // `started_at` through the dialect's own rendering: Postgres hands a
    // timestamp back in a type the store does not read as text, and a select
    // that forgets the cast fails only once a row exists.
    let started = store.dialect().text_of(
        nils_registry::schema::table("release")
            .column("started_at")
            .expect("release.started_at is a column"),
    );
    let sql = format!(
        "SELECT id, name, version, root, {started}, files, unchanged, moved, rewritten, added, \
         removed FROM {}{wheres} ORDER BY id",
        store.qualified("release"),
    );
    let rows = store
        .query(&sql, &params)
        .map_err(|e| fail(e.to_string()))?;
    if rows.is_empty() {
        println!("no release yet");
        return Ok(());
    }
    let mut versions: Vec<Version> = Vec::new();
    for r in &rows {
        let mut counts = [0i64; 5];
        for (i, into) in counts.iter_mut().enumerate() {
            *into = r.int(6 + i).map_err(|e| fail(e.to_string()))?;
        }
        versions.push(Version {
            id: r.int(0).map_err(|e| fail(e.to_string()))?,
            name: r.text(1).map_err(|e| fail(e.to_string()))?.to_string(),
            version: r.text(2).map_err(|e| fail(e.to_string()))?.to_string(),
            root: r.text(3).map_err(|e| fail(e.to_string()))?.to_string(),
            started: r.text(4).map_err(|e| fail(e.to_string()))?.to_string(),
            files: r.int(5).map_err(|e| fail(e.to_string()))?,
            counts,
        });
    }
    let mut last_name = String::new();
    for v in &versions {
        if v.name != last_name {
            println!("{}", v.name);
            println!("  into             {}", v.root);
            last_name = v.name.clone();
        }
        let when = v.started.split('T').next().unwrap_or(&v.started);
        let (version, files) = (&v.version, v.files);
        println!("  {version:<16} {when}   {files:>9} files in the tree");
        let mut said = false;
        for (n, what) in [
            (v.counts[0], "left alone"),
            (v.counts[1], "renamed"),
            (v.counts[2], "written again"),
            (v.counts[3], "new"),
            (v.counts[4], "dropped"),
        ] {
            if n > 0 {
                println!("      {n:>10}   stacks {what}");
                said = true;
            }
        }
        if !said {
            println!("      nothing changed");
        }
        let store = registry.store();
        let sql = format!(
            "SELECT action, was, now FROM {} WHERE release_id = {} ORDER BY id",
            store.qualified("release_move"),
            store.dialect().param(1, nils_registry::schema::Type::Int),
        );
        let moves = store
            .query(&sql, &[nils_registry::store::Param::Int(v.id)])
            .map_err(|e| fail(e.to_string()))?;
        // The renames and the drops, which are the ones a recipient of an
        // earlier version has to act on. A first version's list is every
        // stack in it and worth nobody's screen.
        let interesting: Vec<&nils_registry::store::Row> = moves
            .iter()
            .filter(|r| !matches!(r.text(0), Ok("added")))
            .collect();
        for r in interesting.iter().take(6) {
            let action = r.text(0).unwrap_or("");
            let was = r.opt_text(1).ok().flatten().unwrap_or("");
            match r.opt_text(2).ok().flatten() {
                Some(now) => println!("          {action:<10} {was}  ->  {now}"),
                None => println!("          {action:<10} {was}"),
            }
        }
        if interesting.len() > 6 {
            println!("          {} more, in release_move", interesting.len() - 6);
        }
    }
    Ok(())
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
