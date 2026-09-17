// SPDX-License-Identifier: AGPL-3.0-only
//! `nils setup`: one wizard that installs every part, asks what a person
//! wants, makes what it needs, and says what this machine can do. It is the
//! other end of the one line installer: the script fetches this binary and
//! this binary does the rest.
//!
//! It is a guided wizard, not a full screen: a step at a time, numbered
//! choices with a default in brackets, and nothing done before a summary
//! has said what will be done. Piped (no terminal), it takes every default
//! and says so, so `curl ... | sh` is never stuck at a prompt.
//!
//! `NILS_NO_TTY` says there is no terminal even where one could be opened,
//! for a script that wants the defaults without saying `--yes`.
//!
//! Everything that touches a container engine, a database or the network
//! is decided by a pure function that says what the commands are; the
//! wizard then runs them. `--print` stops after saying, which is what the
//! tests read.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{Exit, fail, usage};
use crate::{tui, update};
use nils_registry::home::{Home, InitOptions};
use nils_registry::{Backend, Scheme};

/// The desk's releases, when the channel is the default one.
pub(crate) const DESK_RELEASES: &str = "https://github.com/kineuro/nils-desk/releases";

/// The images a container run pulls.
const ENGINE_IMAGE: &str = "ghcr.io/kineuro/nils";
const DESK_IMAGE: &str = "ghcr.io/kineuro/nils-desk";

/// This account, as docker wants it written. Podman remaps the user and
/// `:U` gives the container ownership of what it mounts. Docker does
/// neither: a container running as the image's own user cannot write a
/// directory this account owns, and the first thing it tries to write is
/// the registry's key. So every docker run is told to be this account.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "getuid and getgid read this process and cannot fail"
)]
fn as_this_account() -> String {
    // SAFETY: neither call takes a pointer, touches memory, or can fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    format!("{uid}:{gid}")
}

/// Windows names no such account, and the wizard refuses Windows long
/// before a container runs; this is here so the binary builds there.
#[cfg(not(unix))]
fn as_this_account() -> String {
    String::new()
}

/// `--user <this account> ` for a docker run, and nothing where there is no
/// account to name.
fn docker_user() -> String {
    let account = as_this_account();
    if account.is_empty() {
        String::new()
    } else {
        format!("--user {account} ")
    }
}

/// The tag a published image carries, for a version. A release names its
/// images after its git tag, which begins with a v; every version this
/// wizard holds has had that v taken off, so it goes back on here. One
/// place, because a tag that does not match is a silent local build.
fn image_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

/// Where the two Node parts come from, and the release tag each is taken
/// at: the Kvasir and the assistant this version of the engine was released
/// beside.
const KVASIR_REPO: &str = "https://github.com/kineuro/kvasir";
const ASSISTANT_REPO: &str = "https://github.com/kineuro/nils-assistant";
const KVASIR_REF: &str = "v1.0.0-alpha.7";
const ASSISTANT_REF: &str = "v1.0.0-alpha.25";

/// llama.cpp's server, which runs the models Kvasir downloads once an admin
/// starts one (record 24): the build this version takes, where its archives
/// are published, and the sha256 of every archive it may take. A mirror that
/// NILS_SETUP_LLAMA_RELEASES names is held to the same digests.
const LLAMA_BUILD: &str = "b10964";
const LLAMA_RELEASES: &str = "https://github.com/ggml-org/llama.cpp/releases/download";
const LLAMA_ARCHIVES: [(&str, &str); 6] = [
    (
        "ubuntu-x64",
        "9abf88aea48a55d0f80edb1ee20220b186848cca0b4e919d71518cfd7ca67443",
    ),
    (
        "ubuntu-vulkan-x64",
        "55d1e58e14c11eedea090bf088fdeefbfe7b4b09ee03bf6dba9834651769afcf",
    ),
    (
        "ubuntu-arm64",
        "5f0e9c95d970892e43380f82ebcab960edfd20a1cd0f7abffa13b29fdb924949",
    ),
    (
        "ubuntu-vulkan-arm64",
        "f7864baa0edf5a059fb42c5efb5aceb96075aa1f41e6c3142b71ca69286cb0bb",
    ),
    (
        "macos-arm64",
        "033c845c1df9bf945ff37bb193238b40910b2244be3e1e637b2ceb5878f1a6f5",
    ),
    (
        "macos-x64",
        "03430a394d0a169a5e6d8f01c09f48cf58eb026af6fc95940a4a528e2e50cf38",
    ),
];

/// The name the setup record keeps llama.cpp's build under, and its folder.
const LLAMA_PART: &str = "llama.cpp";

/// Inside a container, everything lives under one prefix.
const IN_DESK: &str = "/srv/nils/desk";

/// The Postgres a setup runs for the registry when asked to: the official
/// image with its major version pinned, since a new major version needs the
/// data upgraded, not only a new image.
const POSTGRES_IMAGE: &str = "docker.io/library/postgres:17-alpine";
const POSTGRES_MAJOR: &str = "17";
const POSTGRES_CONTAINER: &str = "nils-postgres";
const IN_POSTGRES: &str = "/var/lib/postgresql/data";

/// What Kvasir and the assistant run in beside the engine and the desk.
/// Neither has an image of its own: both are built on this machine, and their
/// directories are mounted into Node's image at the same paths, so every path
/// their configuration names means the same inside as out.
const NODE_IMAGE: &str = "docker.io/library/node:22-trixie-slim";

/// The address a rootless podman pod reaches this machine's own loopback at,
/// through pasta, and so a model server listening on 127.0.0.1 here.
const HOST_LOOPBACK_IN_POD: &str = "169.254.1.2";

/// How a container names this machine: podman maps it for the pod, docker's
/// is its bridge, which a server listening only on 127.0.0.1 does not answer.
fn host_from_container(runtime: Runtime) -> Option<&'static str> {
    match runtime {
        Runtime::Podman => Some("host.containers.internal"),
        Runtime::Docker => Some("host.docker.internal"),
        Runtime::Machine => None,
    }
}

/// A model address as Kvasir must dial it from where it runs: on this
/// machine as it was typed, in a container with this machine's loopback named
/// the way a container reaches it.
fn model_address_for(runtime: Runtime, url: &str) -> String {
    let Some(host) = host_from_container(runtime) else {
        return url.to_string();
    };
    for loopback in ["127.0.0.1", "localhost", "[::1]"] {
        for scheme in ["http://", "https://"] {
            let prefix = format!("{scheme}{loopback}");
            if let Some(rest) = url.strip_prefix(&prefix)
                && (rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
            {
                return format!("{scheme}{host}{rest}");
            }
        }
    }
    url.to_string()
}

/// What a person should know when Kvasir runs in a container and the model
/// server is on this machine's loopback: how it will be reached, or why it
/// will not be. Nothing when Kvasir runs on the machine or the server is
/// somewhere else.
fn model_reach_note(runtime: Runtime, url: &str, pasta: fn() -> bool) -> Option<String> {
    let dialled = model_address_for(runtime, url);
    if dialled == url {
        return None;
    }
    Some(match runtime {
        Runtime::Podman if pasta() => format!(
            "Kvasir runs in the pod and reaches this machine's own {url} as {dialled}, which \
             podman hands the pod"
        ),
        Runtime::Podman => format!(
            "podman here does not network through pasta, so the pod cannot reach this \
             machine's 127.0.0.1; start the model server on an address the pod reaches, and \
             Kvasir dials {dialled}"
        ),
        _ => format!(
            "Kvasir runs in a container and dials {dialled}, which is this machine on docker's \
             bridge; a server listening only on 127.0.0.1 does not answer there, so start it on \
             0.0.0.0 or on the bridge's address"
        ),
    })
}

/// A Postgres connection string as the engine must use it from where it
/// runs: on this machine as it was typed, in a container with this
/// machine's loopback named the way a container reaches it. Both forms the
/// driver takes are read, a URL and `key=value` pairs; anything else is left
/// as it was.
fn dsn_for(runtime: Runtime, dsn: &str) -> String {
    let Some(alias) = host_from_container(runtime) else {
        return dsn.to_string();
    };
    let loopback = |host: &str| matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1");
    if let Some(scheme_end) = dsn.find("://") {
        let (head, rest) = dsn.split_at(scheme_end + 3);
        let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(at) => authority.split_at(at + 1),
            None => ("", authority),
        };
        let host = if hostport.starts_with('[') {
            hostport.find(']').map_or(hostport, |end| &hostport[..=end])
        } else {
            hostport.split(':').next().unwrap_or(hostport)
        };
        if loopback(host) {
            return format!("{head}{userinfo}{alias}{}{tail}", &hostport[host.len()..]);
        }
        return dsn.to_string();
    }
    let mut changed = false;
    let words: Vec<String> = dsn
        .split_whitespace()
        .map(|word| match word.split_once('=') {
            Some(("host", host)) if loopback(host) => {
                changed = true;
                format!("host={alias}")
            }
            _ => word.to_string(),
        })
        .collect();
    if changed {
        words.join(" ")
    } else {
        dsn.to_string()
    }
}

/// What a person should know when the engine runs in a container and
/// Postgres is on this machine's loopback: how it is reached, or what
/// Postgres must allow. Nothing when the engine runs on the machine or the
/// database is somewhere else.
fn postgres_reach_note(runtime: Runtime, dsn: &str, pasta: fn() -> bool) -> Option<String> {
    if dsn_for(runtime, dsn) == dsn {
        return None;
    }
    Some(match runtime {
        Runtime::Podman if pasta() => "the engine runs in the pod and reaches this machine's \
             Postgres as host.containers.internal, which podman hands the pod"
            .to_string(),
        Runtime::Podman => "podman here does not network through pasta, so the pod cannot \
             reach this machine's 127.0.0.1; give the connection string an address the pod \
             reaches"
            .to_string(),
        _ => "the engine runs in a container and reaches this machine's Postgres as \
             host.docker.internal, on docker's bridge: Postgres must listen there \
             (listen_addresses) and let that network in (pg_hba.conf), not only 127.0.0.1"
            .to_string(),
    })
}

/// The connection string an install wrote into its registry, as the engine
/// uses it.
fn registry_dsn(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("registry").join("nils.toml")).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value.get("dsn")?.as_str().map(str::to_string)
}

/// The Postgres a registry is kept in, from its own configuration: the
/// connection string and the schema. None for a SQLite registry.
fn registry_backend(dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join("registry").join("nils.toml")).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    if value.get("backend")?.as_str()? != "postgres" {
        return None;
    }
    Some((
        value.get("dsn")?.as_str()?.to_string(),
        value
            .get("schema")
            .and_then(|s| s.as_str())
            .unwrap_or("nils")
            .to_string(),
    ))
}

/// An error and every cause under it, on one line: the driver's own
/// "error connecting to server" says nothing without the refusal beneath.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let said = cause.to_string();
        if !out.contains(&said) {
            out = format!("{out}: {said}");
        }
        source = cause.source();
    }
    out
}

/// The same address the other way: what a person typed, from what Kvasir
/// dials, for a machine run after a container one.
fn model_address_on_machine(url: &str) -> String {
    for host in ["host.containers.internal", "host.docker.internal"] {
        for scheme in ["http://", "https://"] {
            if let Some(rest) = url.strip_prefix(&format!("{scheme}{host}")) {
                return format!("{scheme}127.0.0.1{rest}");
            }
        }
    }
    url.to_string()
}

#[derive(Debug, Args)]
pub(crate) struct SetupArgs {
    /// What to install, as a list: engine, desk, assistant
    #[arg(long, value_name = "LIST")]
    parts: Option<String>,
    /// The one directory everything lives under
    #[arg(long, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Who may sign in: off, local or oidc
    #[arg(long, value_name = "off|local|oidc")]
    mode: Option<String>,
    /// With --mode oidc: the issuer of a provider the desk is registered at
    #[arg(long, value_name = "URL")]
    oidc_issuer: Option<String>,
    /// With --oidc-issuer: the desk's client id there
    #[arg(long, value_name = "ID")]
    oidc_client_id: Option<String>,
    /// With --oidc-issuer: a file holding the client's secret
    #[arg(long, value_name = "FILE")]
    oidc_client_secret_file: Option<PathBuf>,
    /// With --oidc-issuer: where the provider publishes its keys, when its
    /// discovery document does not say
    #[arg(long, value_name = "URL")]
    oidc_jwks: Option<String>,
    /// With --oidc-issuer: the claim that carries the entitlements
    #[arg(long, value_name = "NAME")]
    oidc_roles_claim: Option<String>,
    /// With --mode oidc: an Authentik to register the desk at
    #[arg(long, value_name = "URL")]
    authentik: Option<String>,
    /// With --authentik: a file holding an API token of it
    #[arg(long, value_name = "FILE")]
    authentik_token_file: Option<PathBuf>,
    /// With --authentik: the group whose members may use NILS
    #[arg(long, value_name = "GROUP")]
    authentik_users: Option<String>,
    /// With --authentik: the group of those who run it, as operators and admins
    #[arg(long, value_name = "GROUP")]
    authentik_admins: Option<String>,
    /// Where it runs: machine, podman or docker
    #[arg(long, value_name = "machine|podman|docker")]
    runtime: Option<String>,
    /// The registry's backend: sqlite, or postgres, set up here in a container,
    /// or with --dsn one you already run
    #[arg(long, value_name = "sqlite|postgres")]
    backend: Option<String>,
    /// The Postgres connection string, with --backend postgres
    #[arg(long, value_name = "DSN")]
    dsn: Option<String>,
    /// The Postgres schema, with --backend postgres
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    /// A directory of DICOM the engine may read
    #[arg(long, value_name = "DIR")]
    source: Option<PathBuf>,
    /// Who may reach the desk: this machine, or an address of this host
    #[arg(long, value_name = "loopback|network")]
    reach: Option<String>,
    /// The address a browser opens the desk at, where a proxy of yours
    /// answers for it: with --reach network the desk is bound for a proxy on
    /// another machine, and otherwise for one here
    #[arg(long, value_name = "URL")]
    origin: Option<String>,
    /// The registry key's passphrase, from a file instead of a prompt
    #[arg(long, value_name = "FILE")]
    key_file: Option<PathBuf>,
    /// Write and start services
    #[arg(long)]
    service: bool,
    /// Write no services
    #[arg(long, conflicts_with = "service")]
    no_service: bool,
    /// Write the services of this machine, in /etc/systemd/system, each part
    /// running as an account of its own; root's to do, and Linux only
    #[arg(long, conflicts_with = "no_service")]
    system: bool,
    /// With --system: the account a part runs as, as engine=nils; named once
    /// for each part, and nils for a part not named
    #[arg(long, value_name = "PART=ACCOUNT")]
    account: Vec<String>,
    /// With --system: the capabilities the engine's service keeps, as
    /// CAP_DAC_OVERRIDE,CAP_DAC_READ_SEARCH
    #[arg(long, value_name = "LIST")]
    capabilities: Option<String>,
    /// Take every default without asking
    #[arg(long, short = 'y')]
    yes: bool,
    /// Say what it would do, with every command and unit, and change nothing
    #[arg(long)]
    print: bool,
    /// Only bring the parts already installed up to date
    #[arg(long)]
    update: bool,
    /// Where releases come from; NILS_RELEASES sets the same thing
    #[arg(long, value_name = "URL")]
    channel: Option<String>,
}

// ---------------------------------------------------------------- the parts

/// The three things a person may install. The engine is always one of them:
/// it is the binary doing the asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Part {
    Engine,
    Desk,
    Assistant,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Engine => "engine",
            Part::Desk => "desk",
            Part::Assistant => "assistant",
        }
    }
}

/// The parts a list names, engine always among them.
pub(crate) fn parts_of(list: &str) -> Result<Vec<Part>, String> {
    let mut out = vec![Part::Engine];
    for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match word {
            "engine" => {}
            "desk" => out.push(Part::Desk),
            "assistant" => out.push(Part::Assistant),
            other => return Err(format!("{other} is not a part: engine, desk or assistant")),
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// How people sign in, which is one decision across both parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    Local,
    Oidc,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Local => "local",
            Mode::Oidc => "oidc",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Mode::Off => "nobody signs in; one person on this machine",
            Mode::Local => "the desk keeps the people and their passwords",
            Mode::Oidc => "an identity provider decides",
        }
    }

    fn parse(word: &str) -> Result<Mode, String> {
        match word.trim() {
            "off" => Ok(Mode::Off),
            "local" => Ok(Mode::Local),
            "oidc" => Ok(Mode::Oidc),
            other => Err(format!("{other} is not a mode: off, local or oidc")),
        }
    }
}

/// Where the parts run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Runtime {
    Machine,
    Podman,
    Docker,
}

impl Runtime {
    fn name(self) -> &'static str {
        match self {
            Runtime::Machine => "machine",
            Runtime::Podman => "podman",
            Runtime::Docker => "docker",
        }
    }

    fn parse(word: &str) -> Result<Runtime, String> {
        match word.trim() {
            "machine" => Ok(Runtime::Machine),
            "podman" => Ok(Runtime::Podman),
            "docker" => Ok(Runtime::Docker),
            other => Err(format!(
                "{other} is not a runtime: machine, podman or docker"
            )),
        }
    }

    fn container(self) -> bool {
        self != Runtime::Machine
    }
}

/// Who may open the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reach {
    Loopback,
    Network(String),
    /// A proxy of the site's answers for the desk at `origin`: the address a
    /// browser opens, which the desk compares a write against, signs its
    /// tokens with and sends a person back to after a sign in. Where the
    /// desk binds is a separate matter, and `network` carries it: a proxy on
    /// another machine reaches the desk on this machine's address, one here
    /// reaches it on the loopback.
    Behind {
        origin: String,
        network: bool,
    },
}

impl Reach {
    /// Whether the desk itself answers beyond this machine's own loopback,
    /// which is what a container publishes and what an open desk with no
    /// login is warned about.
    fn beyond_loopback(&self) -> bool {
        match self {
            Reach::Loopback => false,
            Reach::Network(_) => true,
            Reach::Behind { network, .. } => *network,
        }
    }

    /// The origin a proxy answers at, where one does.
    fn proxied(&self) -> Option<&str> {
        match self {
            Reach::Behind { origin, .. } => Some(origin),
            _ => None,
        }
    }
}

// --------------------------------------------- the services of this machine

/// The services of the machine rather than of the account that ran setup:
/// unit files in `/etc/systemd/system`, each part running as an account of
/// its own, and the engine keeping the capabilities it needs to read and
/// write across the filesystems a site mounts. A service of an account's own
/// cannot carry a capability at all, so this is a kind of service and not a
/// setting beside them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SystemUnits {
    /// The capabilities the engine's service keeps, by their own names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) capabilities: Vec<String>,
    /// The account each part runs as, by the part's name. A part not named
    /// here runs as [`DEFAULT_ACCOUNT`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) accounts: BTreeMap<String, String>,
}

/// The account a part runs as where an install names none for it.
const DEFAULT_ACCOUNT: &str = "nils";

/// The parts an account can be named for. Kvasir and llama.cpp are the
/// assistant's own and run as the account it runs as.
const ACCOUNT_PARTS: [&str; 3] = ["engine", "desk", "assistant"];

/// The directory the services of a machine are written into.
const SYSTEM_UNITS_DIR: &str = "/etc/systemd/system";

impl SystemUnits {
    /// The account a part runs as: the one named for it, else the default.
    pub(crate) fn account(&self, part: &str) -> &str {
        self.accounts
            .get(part)
            .map_or(DEFAULT_ACCOUNT, String::as_str)
    }

    /// Every account this install needs, each named once, in the order the
    /// parts are named.
    fn every_account(&self, parts: &[Part]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for part in parts {
            let account = self.account(part.name()).to_string();
            if !out.contains(&account) {
                out.push(account);
            }
        }
        out
    }
}

/// The accounts named on the command line, as `engine=nils`. A part that is
/// not a part, or a name that is not an account's, is refused here, before
/// anything is written.
fn accounts_given(given: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for word in given {
        let Some((part, account)) = word.split_once('=') else {
            return Err(format!(
                "{word} names no account: --account is a part and an account, as --account \
                 desk=nils-desk"
            ));
        };
        let (part, account) = (part.trim(), account.trim());
        if !ACCOUNT_PARTS.contains(&part) {
            return Err(format!(
                "{part} is not a part an account can be named for: engine, desk or assistant"
            ));
        }
        if account.is_empty() {
            return Err(format!("--account {part}= names no account"));
        }
        let named = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '$');
        if !account.chars().all(named) || account.starts_with('-') {
            return Err(format!("{account} is not the name of an account"));
        }
        out.insert(part.to_string(), account.to_string());
    }
    Ok(out)
}

/// Every capability a service can be given, so that one misspelled is a
/// sentence here rather than a service systemd refuses to start.
const CAPABILITY_NAMES: [&str; 41] = [
    "CAP_CHOWN",
    "CAP_DAC_OVERRIDE",
    "CAP_DAC_READ_SEARCH",
    "CAP_FOWNER",
    "CAP_FSETID",
    "CAP_KILL",
    "CAP_SETGID",
    "CAP_SETUID",
    "CAP_SETPCAP",
    "CAP_LINUX_IMMUTABLE",
    "CAP_NET_BIND_SERVICE",
    "CAP_NET_BROADCAST",
    "CAP_NET_ADMIN",
    "CAP_NET_RAW",
    "CAP_IPC_LOCK",
    "CAP_IPC_OWNER",
    "CAP_SYS_MODULE",
    "CAP_SYS_RAWIO",
    "CAP_SYS_CHROOT",
    "CAP_SYS_PTRACE",
    "CAP_SYS_PACCT",
    "CAP_SYS_ADMIN",
    "CAP_SYS_BOOT",
    "CAP_SYS_NICE",
    "CAP_SYS_RESOURCE",
    "CAP_SYS_TIME",
    "CAP_SYS_TTY_CONFIG",
    "CAP_MKNOD",
    "CAP_LEASE",
    "CAP_AUDIT_WRITE",
    "CAP_AUDIT_CONTROL",
    "CAP_SETFCAP",
    "CAP_MAC_OVERRIDE",
    "CAP_MAC_ADMIN",
    "CAP_SYSLOG",
    "CAP_WAKE_ALARM",
    "CAP_BLOCK_SUSPEND",
    "CAP_AUDIT_READ",
    "CAP_PERFMON",
    "CAP_BPF",
    "CAP_CHECKPOINT_RESTORE",
];

/// The capabilities named on the command line, as
/// `CAP_DAC_OVERRIDE,CAP_DAC_READ_SEARCH`. Written however a person writes
/// them, and held to the names the kernel knows.
fn capabilities_given(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for word in text
        .split([',', ' ', '\t'])
        .filter(|w| !w.trim().is_empty())
    {
        let word = word.trim().to_ascii_uppercase();
        let name = if word.starts_with("CAP_") {
            word
        } else {
            format!("CAP_{word}")
        };
        if !CAPABILITY_NAMES.contains(&name.as_str()) {
            return Err(format!(
                "{name} is not a capability a service can keep; the engine's are usually \
                 CAP_DAC_OVERRIDE and CAP_DAC_READ_SEARCH, which let it read and write across the \
                 filesystems a site mounts"
            ));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

/// The numbers an account has on this machine, asked of the system so that
/// whatever keeps the accounts answers.
fn account_ids(name: &str) -> Option<(u32, u32)> {
    let uid = run_quiet("id", &["-u", name])?.trim().parse().ok()?;
    let gid = run_quiet("id", &["-g", name])?.trim().parse().ok()?;
    Some((uid, gid))
}

/// Whether this process is root, which writing into `/etc/systemd/system`
/// and running the parts as other accounts both need.
fn am_root() -> bool {
    run_quiet("id", &["-u"]).is_some_and(|out| out.trim() == "0")
}

/// Whether systemd runs this machine, which is what takes a service of its
/// own. Asked of the machine rather than of the binary, which answers
/// wherever it is installed.
fn systemd_here() -> bool {
    cfg!(target_os = "linux") && Path::new("/run/systemd/system").exists()
}

/// Why this machine cannot be given services of its own, said so that a
/// person can act on it before anything is written. `None` where it can.
fn system_refusal(runtime: Runtime, system: &SystemUnits, parts: &[Part]) -> Option<String> {
    let missing: Vec<String> = system
        .every_account(parts)
        .into_iter()
        .filter(|name| account_ids(name).is_none())
        .collect();
    system_refusal_when(runtime, am_root(), systemd_here(), &missing)
}

/// The same, for a machine with or without each of the things it asks for,
/// so that every answer can be had on any machine.
fn system_refusal_when(
    runtime: Runtime,
    root: bool,
    systemd: bool,
    missing: &[String],
) -> Option<String> {
    if runtime.container() {
        return Some(format!(
            "--system writes a service for each part that runs on this machine, and with {} the \
             parts run in containers their own runtime keeps: install with --runtime machine, or \
             leave --system off",
            runtime.name()
        ));
    }
    if !systemd {
        return Some(
            "this machine has no systemd to take a service of its own: leave --system off, and \
             the parts are kept the way this account keeps its own"
                .to_string(),
        );
    }
    if !root {
        return Some(format!(
            "writing services into {SYSTEM_UNITS_DIR}, and running the parts as accounts of their \
             own, is root's to do: run nils setup again as root, or leave --system off and the \
             services are this account's own"
        ));
    }
    if !missing.is_empty() {
        // The accounts are the site's: it keeps them, it numbers them, and
        // it may keep them somewhere other than this machine's own files.
        // Setup names one in a unit; it never makes one.
        let (there, them) = if missing.len() == 1 {
            ("is no account", "it")
        } else {
            ("are no accounts", "them")
        };
        return Some(format!(
            "there {there} on this machine named {}, and the parts would run as {them}: a service \
             account is the site's own to make, so add {them} and run nils setup again",
            missing.join(", ")
        ));
    }
    None
}

/// Why an account or a capability named without the services of this machine
/// cannot be taken. `None` where neither was named.
fn without_system(accounts: bool, capabilities: bool) -> Option<String> {
    if capabilities {
        return Some(
            "a capability belongs to a service of this machine: a service of this account cannot \
             carry one at all, so --capabilities goes with --system"
                .to_string(),
        );
    }
    if accounts {
        return Some(
            "an account of its own belongs to a service of this machine: a service of this \
             account runs as this account, so --account goes with --system"
                .to_string(),
        );
    }
    None
}

/// An install whose services are this machine's, brought up to date or
/// repaired where they cannot be written now: said before anything is
/// touched, since the record says what the services are and this run would
/// otherwise write half an install and leave the rest as it was.
fn recorded_system_refusal(plan: &Plan) -> Option<String> {
    let system = plan.system.as_ref()?;
    if !plan.service {
        return None;
    }
    system_refusal(plan.runtime, system, &plan.parts)
}

/// Where the registry itself is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendChoice {
    Sqlite,
    Postgres { dsn: String, schema: String },
}

/// The four ports, so a second install on one machine can move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ports {
    pub(crate) engine: u16,
    pub(crate) desk: u16,
    pub(crate) kvasir: u16,
    pub(crate) assistant: u16,
    /// The Postgres a setup runs, published on this machine's loopback.
    #[serde(default = "default_postgres_port")]
    pub(crate) postgres: u16,
    /// The supervisor on this host, which the desk asks about the install.
    #[serde(default = "default_supervisor_port")]
    pub(crate) supervisor: u16,
    /// llama.cpp on this host, which runs the models Kvasir starts.
    #[serde(default = "default_llama_port")]
    pub(crate) llama: u16,
}

fn default_postgres_port() -> u16 {
    5432
}

fn default_supervisor_port() -> u16 {
    8470
}

fn default_llama_port() -> u16 {
    7110
}

impl Default for Ports {
    fn default() -> Ports {
        Ports {
            engine: 8437,
            desk: 7200,
            kvasir: 7100,
            assistant: 7300,
            postgres: default_postgres_port(),
            supervisor: default_supervisor_port(),
            llama: default_llama_port(),
        }
    }
}

/// A Postgres this setup runs in a container for the registry, and which
/// runtime runs it. Its port is the plan's; its data and its password live in
/// the base directory, beside the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagedPostgres {
    pub(crate) runtime: Runtime,
}

/// Where a Postgres could be run for the registry: in the runtime the parts
/// use, or on a machine install in podman, else docker. None when neither
/// answers.
fn managed_runtime(runtime: Runtime, podman: bool, docker: bool) -> Option<Runtime> {
    match runtime {
        Runtime::Podman | Runtime::Docker => Some(runtime),
        Runtime::Machine if podman => Some(Runtime::Podman),
        Runtime::Machine if docker => Some(Runtime::Docker),
        Runtime::Machine => None,
    }
}

// ------------------------------------------------------- what the card can do

/// What a graphics card offers, as the probe found it.
#[derive(Debug, Clone)]
pub(crate) struct Card {
    pub(crate) name: String,
    pub(crate) memory_gb: f64,
}

/// What the memory of a card means for the assistant, in the product's own
/// words. `None` is a machine with no card the probe could find, which
/// reads the same as one too small to serve a model.
pub(crate) fn card_advice(memory_gb: Option<f64>) -> Vec<String> {
    match memory_gb {
        Some(gb) if gb >= 24.0 => vec![
            "A 27B model at 4 bit fits with room for the context.".to_string(),
            "That is what the stations were written against.".to_string(),
        ],
        Some(gb) if gb >= 12.0 => vec![
            "A 7B to 14B model fits.".to_string(),
            "The stations still work, with more retries on the harder questions.".to_string(),
        ],
        _ => vec![
            "No local model worth serving.".to_string(),
            "The options are a model on another machine you can reach, a commercial provider \
             through Kvasir, the model gateway (the prompt then leaves the machine, and Kvasir \
             marks that backend remote), or no assistant at all, which costs nothing else: the \
             engine and the desk are complete without it."
                .to_string(),
        ],
    }
}

/// Ask the machine what cards it has: every NVIDIA card, else every AMD
/// card, else the unified memory of an Apple machine. Nothing here installs
/// anything.
pub(crate) fn probe_cards() -> Vec<Card> {
    if let Some(out) = run_quiet(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ],
    ) {
        let cards = nvidia_cards(&out);
        if !cards.is_empty() {
            return cards;
        }
    }
    if let Some(out) = run_quiet("rocm-smi", &["--showmeminfo", "vram", "--csv"]) {
        let cards = amd_cards(&out);
        if !cards.is_empty() {
            return cards;
        }
    }
    if cfg!(target_os = "macos") {
        let name = run_quiet("sysctl", &["-n", "machdep.cpu.brand_string"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "this machine".to_string());
        if name.contains("Apple")
            && let Some(mem) = run_quiet("sysctl", &["-n", "hw.memsize"])
            && let Ok(bytes) = mem.trim().parse::<f64>()
        {
            return vec![Card {
                name: format!("{name}, unified memory"),
                memory_gb: bytes / 1024.0 / 1024.0 / 1024.0,
            }];
        }
    }
    Vec::new()
}

/// The one card a caller that wants one reads: the card with the most
/// memory.
pub(crate) fn probe_card() -> Option<Card> {
    largest(&probe_cards())
}

/// The card with the most memory; the first of those alike.
pub(crate) fn largest(cards: &[Card]) -> Option<Card> {
    cards
        .iter()
        .fold(None, |best: Option<&Card>, c| match best {
            Some(b) if b.memory_gb >= c.memory_gb => Some(b),
            _ => Some(c),
        })
        .cloned()
}

/// Every card `nvidia-smi --query-gpu=name,memory.total
/// --format=csv,noheader,nounits` lists, one a line, in MiB; a line that is
/// not a name and a size names no card, and a size that does not read is
/// none.
fn nvidia_cards(out: &str) -> Vec<Card> {
    out.lines()
        .filter_map(|line| line.rsplit_once(','))
        .map(|(name, mib)| Card {
            name: name.trim().to_string(),
            memory_gb: mib.trim().parse::<f64>().unwrap_or(0.0) / 1024.0,
        })
        .filter(|c| !c.name.is_empty())
        .collect()
}

/// Every card `rocm-smi --showmeminfo vram --csv` names: a line whose first
/// number past a gigabyte is the card's memory, in bytes.
fn amd_cards(out: &str) -> Vec<Card> {
    out.lines()
        .filter_map(|line| {
            line.split(|c: char| !c.is_ascii_digit())
                .filter_map(|n| n.parse::<f64>().ok())
                .find(|n| *n > 1e9)
        })
        .map(|bytes| Card {
            name: "an AMD card".to_string(),
            memory_gb: bytes / 1024.0 / 1024.0 / 1024.0,
        })
        .collect()
}

/// The memory of every card, in gigabytes.
fn total_gb(cards: &[Card]) -> f64 {
    cards.iter().map(|c| c.memory_gb).sum()
}

/// The cards as the plan names them: each name once, with how many there are
/// when more than one, and the memory of them all, as in
/// `2 × NVIDIA RTX PRO 6000, 191 GB`; none without a card.
pub(crate) fn cards_words(cards: &[Card]) -> Option<String> {
    if cards.is_empty() {
        return None;
    }
    let mut names: Vec<(&str, usize)> = Vec::new();
    for c in cards {
        match names.iter_mut().find(|(n, _)| *n == c.name) {
            Some((_, count)) => *count += 1,
            None => names.push((c.name.as_str(), 1)),
        }
    }
    let mut named: Vec<String> = names
        .iter()
        .map(|(name, count)| match count {
            1 => name.to_string(),
            n => format!("{n} × {name}"),
        })
        .collect();
    let last = named.pop().unwrap_or_default();
    let all = if named.is_empty() {
        last
    } else {
        format!("{} and {last}", named.join(", "))
    };
    Some(format!("{all}, {:.0} GB", total_gb(cards).round()))
}

// ------------------------------------------------------- the model runtime

/// The llama.cpp build a machine takes (record 24), and whether a Vulkan
/// build finds the loader it needs there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Llama {
    pub(crate) variant: &'static str,
    /// False only for a Vulkan build on a Linux machine with no
    /// libvulkan.so.1, where a model runs on the processor instead.
    pub(crate) loader: bool,
}

/// The archive a machine takes by its system, its processor and whether it
/// has graphics: macOS by its processor; Linux the Vulkan build where there
/// is a card or a render node, and the CPU build otherwise. None where
/// llama.cpp publishes no build.
fn llama_variant(os: &str, arch: &str, graphics: bool) -> Option<&'static str> {
    Some(match (os, arch, graphics) {
        ("macos", "aarch64", _) => "macos-arm64",
        ("macos", "x86_64", _) => "macos-x64",
        ("linux", "x86_64", true) => "ubuntu-vulkan-x64",
        ("linux", "x86_64", false) => "ubuntu-x64",
        ("linux", "aarch64", true) => "ubuntu-vulkan-arm64",
        ("linux", "aarch64", false) => "ubuntu-arm64",
        _ => return None,
    })
}

/// Whether this machine has a render node, which gives a Vulkan build a
/// device where the card probe names none, an integrated one among them.
fn render_node() -> bool {
    std::fs::read_dir("/dev/dri").is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
    })
}

/// Whether the Vulkan loader is on this machine, as the dynamic linker lists
/// it or where the distributions put it.
fn vulkan_loader() -> bool {
    let listed = ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"]
        .iter()
        .find_map(|ldconfig| run_quiet(ldconfig, &["-p"]))
        .is_some_and(|list| list.contains("libvulkan.so.1"));
    listed
        || [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/libvulkan.so.1",
        ]
        .iter()
        .any(|path| Path::new(path).exists())
}

/// The build this machine takes, where llama.cpp publishes one for it.
fn llama_here(card: bool) -> Option<Llama> {
    let variant = llama_variant(
        std::env::consts::OS,
        std::env::consts::ARCH,
        card || render_node(),
    )?;
    Some(Llama {
        variant,
        loader: !variant.contains("vulkan") || vulkan_loader(),
    })
}

/// The variant of a build folder a record names, `<build>-<variant>`.
fn llama_recorded(path: &str) -> Option<&'static str> {
    let name = Path::new(path).file_name()?.to_str()?;
    let (_, variant) = name.split_once('-')?;
    LLAMA_ARCHIVES
        .iter()
        .map(|(v, _)| *v)
        .find(|v| *v == variant)
}

/// A build as a person reads it: what it runs a model on.
fn llama_words(variant: &str) -> &'static str {
    if variant.contains("vulkan") {
        "Vulkan"
    } else if variant.starts_with("macos") {
        "Metal"
    } else {
        "CPU"
    }
}

/// The sha256 this version takes for one variant's archive.
fn llama_digest(variant: &str) -> Option<&'static str> {
    LLAMA_ARCHIVES
        .iter()
        .find(|(v, _)| *v == variant)
        .map(|(_, digest)| *digest)
}

/// Where the archives come from: llama.cpp's releases, or a mirror of them.
fn llama_base() -> String {
    std::env::var("NILS_SETUP_LLAMA_RELEASES")
        .ok()
        .map(|base| base.trim().to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| LLAMA_RELEASES.to_string())
}

/// One variant's archive of the pinned build, under a base.
fn llama_archive(base: &str, variant: &str) -> String {
    format!(
        "{}/{LLAMA_BUILD}/llama-{LLAMA_BUILD}-bin-{variant}.tar.gz",
        base.trim_end_matches('/')
    )
}

/// Where a build is unpacked: `<dir>/llama.cpp/<build>-<variant>`.
fn llama_build_dir(dir: &Path, variant: &str) -> PathBuf {
    dir.join(LLAMA_PART)
        .join(format!("{LLAMA_BUILD}-{variant}"))
}

/// An archive unpacked into `into` without its `llama-<build>/` folder, once
/// its sha256 is `want`. Where it is not, where the archive names a path
/// outside that folder or holds no llama-server, nothing is left at `into`
/// and a build already there stays.
fn unpack_llama(bytes: &[u8], want: &str, into: &Path) -> Result<(), String> {
    let got = crate::supervise::sha256_hex(bytes);
    if got != want {
        return Err(format!(
            "its sha256 is {got}, and this version of nils takes only {want}"
        ));
    }
    let name = into
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let partial = into.with_file_name(format!(".{name}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    if let Err(e) = unpack_into(bytes, &partial) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(into);
    std::fs::rename(&partial, into).map_err(|e| {
        let _ = std::fs::remove_dir_all(&partial);
        format!("{}: {e}", into.display())
    })
}

/// The entries of a build's archive under `to`, each without the build's own
/// folder, its links kept as links.
fn unpack_into(bytes: &[u8], to: &Path) -> Result<(), String> {
    use std::path::Component;
    let top = format!("llama-{LLAMA_BUILD}");
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive.entries().map_err(|e| format!("the archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("the archive: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("the archive: {e}"))?
            .into_owned();
        let outside = || format!("the archive names {}, outside {top}/", path.display());
        let mut parts = path.components();
        if !matches!(parts.next(), Some(Component::Normal(first)) if first.to_str() == Some(top.as_str()))
        {
            return Err(outside());
        }
        let rest: PathBuf = parts.collect();
        if rest.as_os_str().is_empty() {
            continue;
        }
        if !rest.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(outside());
        }
        if entry.header().entry_type().is_hard_link() {
            return Err(format!(
                "the archive holds {} as a hard link",
                path.display()
            ));
        }
        let at = to.join(&rest);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        entry
            .unpack(&at)
            .map_err(|e| format!("{}: {e}", rest.display()))?;
    }
    if !to.join("llama-server").is_file() {
        return Err(format!("the archive holds no {top}/llama-server"));
    }
    Ok(())
}

/// The devices a build runs a model on, as `llama-server --list-devices`
/// names them; none where it names none, or says nothing within ten seconds.
fn llama_devices(server: &Path) -> Vec<String> {
    use std::io::Read as _;
    let Ok(mut child) = Command::new(server)
        .arg("--list-devices")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Vec::new();
    };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(10) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut said);
    }
    devices_listed(&said)
}

/// The device lines of `--list-devices`: those under "Available devices",
/// where a machine with none has the one line `(none)`.
fn devices_listed(said: &str) -> Vec<String> {
    said.lines()
        .skip_while(|line| !line.trim_start().starts_with("Available devices"))
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "(none)")
        .map(str::to_string)
        .collect()
}

/// Run a command for its output, or nothing at all when it is not there.
fn run_quiet(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn have(program: &str) -> bool {
    run_quiet(program, &["--version"]).is_some()
}

/// The first four words of a command, for a line that says what was done
/// without repeating a screen of mounts.
fn short(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().take(4).collect();
    format!("{} ...", words.join(" "))
}

/// Run one of the container lines this wizard prints, as it is written.
fn run_line(line: &str) -> Result<(), String> {
    let mut words = line.split_whitespace();
    let program = words.next().ok_or("an empty command")?;
    let args: Vec<&str> = words.collect();
    let out = Command::new(program)
        .args(&args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    Err(why.lines().last().unwrap_or("it failed").to_string())
}

// ------------------------------------------------------------- the screens

/// The wizard's steps, as it names them.
const STEPS: [&str; 8] = [
    "What to install",
    "Where it runs",
    "Where it lives",
    "The registry",
    "Who may sign in",
    "What it can do",
    "Keeping it running",
    "The plan",
];

/// What this machine has, asked once. The screens run the questions again
/// after every answer, and asking podman, docker and the card each time made
/// every key wait for them.
struct Facts {
    podman: bool,
    docker: Result<(), DockerAbsent>,
    /// Every card the probe found, and the one with the most memory, which
    /// is what the advice reads.
    cards: Vec<Card>,
    card: Option<Card>,
    /// The llama.cpp build this machine takes, where there is one.
    llama: Option<Llama>,
}

impl Facts {
    fn probe() -> Facts {
        let cards = probe_cards();
        let card = largest(&cards);
        Facts {
            podman: have("podman"),
            docker: docker_answers(),
            llama: llama_here(card.is_some()),
            card,
            cards,
        }
    }
}

/// What the questions came to.
enum Flow {
    Install(Box<(Plan, Answers)>),
    Update(State),
    Repair(State),
    Remove,
    /// `--print` said the plan and changed nothing.
    Printed,
    /// The plan was declined.
    Declined,
    /// This machine lacks what the plan needs, each said; nothing was placed.
    Unready(Vec<String>),
}

/// Why the questions stopped short: an error of their own, or a question
/// with no answer yet, which the screens then ask.
enum Stop {
    Exit(Exit),
    Ask(Question),
}

impl From<Exit> for Stop {
    fn from(e: Exit) -> Stop {
        Stop::Exit(e)
    }
}

impl Stop {
    fn into_exit(self) -> Exit {
        match self {
            Stop::Exit(e) => e,
            Stop::Ask(question) => fail(format!(
                "\"{}\" was asked with nothing to answer it on",
                question.text
            )),
        }
    }
}

/// A question as the screens ask it.
struct Question {
    text: String,
    ask: tui::Ask,
}

/// An answer given on a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Answer {
    Pick(usize),
    Text(String),
}

/// The answers given on the screens, and what the run of the questions under
/// way has used of them and shown.
#[derive(Default)]
struct Replay {
    answers: Vec<(String, Answer)>,
    /// Answers taken back, by where they were given, offered again when the
    /// same question comes up there.
    offered: BTreeMap<usize, (String, Answer)>,
    /// What each probe found, by how many answers had been used when it was
    /// made.
    probes: Vec<(usize, String, Box<dyn std::any::Any>)>,
    at: usize,
    step: usize,
    said: Vec<tui::Said>,
    summaries: Vec<Option<String>>,
}

impl Replay {
    /// Ready for the questions to be run again from the start.
    fn restart(&mut self) {
        self.at = 0;
        self.step = 0;
        self.said.clear();
        self.summaries = vec![None; STEPS.len()];
    }

    /// The answer given to this question when it was asked here before. A
    /// different question here means the questions went another way, and the
    /// answers from here on are no longer theirs.
    fn next(&mut self, question: &str) -> Option<Answer> {
        match self.answers.get(self.at) {
            Some((asked, answer)) if asked == question => {
                let answer = answer.clone();
                self.at += 1;
                Some(answer)
            }
            Some(_) => {
                self.forget_from(self.at);
                None
            }
            None => None,
        }
    }

    /// The answer just used, which did not fit its question, taken away.
    fn unanswer(&mut self) {
        self.at = self.at.saturating_sub(1);
        self.forget_from(self.at);
    }

    /// The answers from `position` on taken away, with what was probed for
    /// them.
    fn forget_from(&mut self, position: usize) {
        self.answers.truncate(position);
        self.probes.retain(|(at, _, _)| *at <= position);
    }
}

/// A question with the answer taken back from it given again, to keep or to
/// change.
fn offer(ask: tui::Ask, answer: &Answer) -> tui::Ask {
    match (ask, answer) {
        (tui::Ask::Pick { options, .. }, Answer::Pick(at)) if *at < options.len() => {
            tui::Ask::Pick { options, at: *at }
        }
        (
            tui::Ask::Text {
                hidden,
                default,
                required,
                ..
            },
            Answer::Text(text),
        ) => tui::Ask::Text {
            text: text.clone(),
            cursor: text.chars().count(),
            hidden,
            default,
            required,
        },
        (ask, _) => ask,
    }
}

// ------------------------------------------------------------- the console

/// The terminal, when there is one. A piped run has no prompt and takes
/// every default; that is what `curl ... | sh` does.
struct Console {
    tty: Option<BufReader<std::fs::File>>,
    stdin: bool,
    colour: bool,
    yes: bool,
    /// Whether output is a terminal wide enough to draw the checklist and
    /// the card on.
    live: bool,
    palette: tui::Palette,
    /// The checklist while an install draws one, with the stage each of its
    /// rows is.
    checklist: Option<(tui::Live, Vec<Stage>)>,
    /// What was said under the checklist that a person should read, shown
    /// once it is finished.
    later: std::cell::RefCell<Vec<String>>,
    /// Whether a failure that leaves a chosen part unusable stops the run:
    /// an install's, where nothing is running yet that a stop would strand.
    strict: std::cell::Cell<bool>,
    /// The answers given on the screens, while the questions are asked on
    /// them.
    screens: Option<std::cell::RefCell<Replay>>,
}

impl Console {
    fn new(yes: bool) -> Console {
        // NILS_NO_TTY says there is no terminal even where one could be
        // opened, which is what a script wants and what the tests assert.
        let blind = std::env::var_os("NILS_NO_TTY").is_some();
        let stdin = !blind && std::io::stdin().is_terminal();
        let tty = if stdin || blind {
            None
        } else {
            std::fs::File::open("/dev/tty").ok().map(BufReader::new)
        };
        let colour = std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
        let terminal = !blind && std::io::stdout().is_terminal();
        let live = terminal
            && std::env::var("TERM").ok().is_none_or(|t| t != "dumb")
            && tui::width(1) >= tui::CHECKLIST_WIDTH + 8;
        Console {
            tty,
            stdin,
            colour,
            yes,
            live,
            palette: tui::Palette::detect(terminal),
            checklist: None,
            later: std::cell::RefCell::new(Vec::new()),
            strict: std::cell::Cell::new(false),
            screens: None,
        }
    }

    /// Whether a person can be asked anything at all.
    fn interactive(&self) -> bool {
        !self.yes && (self.stdin || self.tty.is_some() || self.screens.is_some())
    }

    fn bold(&self, text: &str) -> String {
        if self.colour {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn dim(&self, text: &str) -> String {
        if self.colour {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn accent(&self, text: &str) -> String {
        if self.colour {
            format!("\x1b[36m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// A step begun: its heading on lines, a fresh screen on the screens.
    fn step(&self, n: usize) {
        match &self.screens {
            Some(replay) => {
                let mut replay = replay.borrow_mut();
                replay.step = n;
                replay.said.clear();
            }
            None => {
                println!();
                println!(
                    "{} {}",
                    self.accent(&format!("[{n}/{}]", STEPS.len())),
                    self.bold(STEPS.get(n.wrapping_sub(1)).copied().unwrap_or_default())
                );
            }
        }
    }

    /// What a step was answered with, for the steps down the side.
    fn said(&self, summary: &str) {
        if let Some(replay) = &self.screens {
            let mut replay = replay.borrow_mut();
            let step = replay.step;
            if let Some(slot) = step
                .checked_sub(1)
                .and_then(|i| replay.summaries.get_mut(i))
            {
                *slot = Some(summary.to_string());
            }
        }
    }

    /// A row of the plan.
    fn row(&self, key: &str, value: &str) {
        match &self.screens {
            Some(replay) => replay
                .borrow_mut()
                .said
                .push(tui::Said::Row(key.to_string(), value.to_string())),
            None => println!("  {} {value}", self.dim(&format!("{key:<12}"))),
        }
    }

    /// A heading inside a step; on a screen, the step's screen begun again
    /// under it.
    fn heading(&self, text: &str) {
        match &self.screens {
            Some(replay) => {
                let mut replay = replay.borrow_mut();
                replay.said.clear();
                replay.said.push(tui::Said::Text(text.to_string()));
            }
            None => {
                println!();
                println!("  {}", self.bold(text));
            }
        }
    }

    fn note(&self, text: &str) {
        if let Some(replay) = &self.screens {
            replay
                .borrow_mut()
                .said
                .push(tui::Said::Note(text.to_string()));
        } else if self.checklist.is_none() {
            // under a checklist, its tick says it
            println!("  {}", self.dim(text));
        }
    }

    /// A step of the work done, said as it happens; under a checklist, its
    /// tick says it.
    fn progress(&self, text: &str) {
        if self.checklist.is_none() {
            println!("  {text}");
        }
    }

    /// What the work is doing now: a note, or under a checklist the running
    /// row's few words.
    fn doing(&self, text: &str) {
        match &self.checklist {
            Some((live, _)) => live.detail(text),
            None => self.note(text),
        }
    }

    /// A line a person should read, kept until the checklist is finished.
    fn say(&self, text: &str) {
        if let Some(replay) = &self.screens {
            replay
                .borrow_mut()
                .said
                .push(tui::Said::Text(text.to_string()));
        } else if self.checklist.is_some() {
            self.later.borrow_mut().push(format!("  {text}"));
        } else {
            println!("  {text}");
        }
    }

    /// Lines a person must read while the work waits on them, such as a code
    /// to enter somewhere: under a checklist, drawn beneath its rows until
    /// they are taken away with none; otherwise said at once.
    fn show(&self, lines: &[String]) {
        match &self.checklist {
            Some((live, _)) => live.notice(lines),
            None => {
                for line in lines {
                    self.say(line);
                }
            }
        }
    }

    /// Something that did not happen, which marks the checklist's running
    /// row; the install goes on.
    /// A failure that leaves a part the person chose unusable. An install
    /// stops on it with the reason, before the rest is started, since an
    /// install that would not work is not finished; an update or a repair of
    /// one already there says it and goes on, so what still works keeps
    /// running.
    fn broken(&self, text: &str) -> Result<(), Exit> {
        if self.strict.get() {
            return Err(fail(text));
        }
        self.warn(text);
        Ok(())
    }

    fn warn(&self, text: &str) {
        match &self.checklist {
            Some((live, _)) => {
                live.falter();
                self.later
                    .borrow_mut()
                    .push(format!(" {} {text}", self.palette.bad("!")));
            }
            None => println!("  {text}"),
        }
    }

    /// A wait with nothing to run, said with its timer on a terminal.
    fn waiting(&self, label: &str, since: std::time::Instant) {
        match &self.checklist {
            Some((live, _)) => live.detail(label),
            None if std::io::stdout().is_terminal() => {
                print!(
                    "\r  {label} {}",
                    self.dim(&format!("{}s", since.elapsed().as_secs()))
                );
                let _ = std::io::stdout().flush();
            }
            None => {}
        }
    }

    /// The end of a wait, its line cleared.
    fn waited(&self) {
        if self.checklist.is_none() && std::io::stdout().is_terminal() {
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
    }

    /// Draw the checklist for these stages, on a terminal that can show it.
    fn start_checklist(&mut self, title: &str, stages: Vec<(Stage, String)>) {
        if !self.live {
            return;
        }
        println!();
        let (keys, names): (Vec<Stage>, Vec<String>) = stages.into_iter().unzip();
        self.checklist = Some((tui::Live::start(self.palette, title, names), keys));
    }

    /// A stage under way, when the checklist lists it.
    fn begin(&self, stage: Stage) {
        if let Some((live, stages)) = &self.checklist
            && let Some(at) = stages.iter().position(|s| *s == stage)
        {
            live.begin(at);
        }
    }

    /// The checklist's last drawing, then what was kept for after it.
    fn finish_checklist(&mut self, ok: bool, title: &str) {
        let Some((live, _)) = self.checklist.take() else {
            return;
        };
        live.finish(ok, title);
        let later = std::mem::take(self.later.get_mut());
        if !later.is_empty() {
            println!();
            for line in later {
                println!("{line}");
            }
        }
    }

    /// Whether the questions can be asked on screens: a person at a terminal
    /// with room for them.
    fn can_draw_screens(&self) -> bool {
        if !self.interactive() || !self.live {
            return false;
        }
        let (cols, rows) = tui::size(1);
        cols >= 60 && rows >= 18
    }

    /// Where keys come from: the terminal opened for a piped run, or
    /// standard input.
    #[cfg(unix)]
    fn input_fd(&self) -> i32 {
        use std::os::fd::AsRawFd as _;
        self.tty.as_ref().map_or(0, |tty| tty.get_ref().as_raw_fd())
    }

    #[cfg(not(unix))]
    fn input_fd(&self) -> i32 {
        0
    }

    /// One of a list: by number on lines, with the arrow keys on a screen.
    fn ask_choice(
        &mut self,
        question: &str,
        options: &[(&str, &str)],
        default: usize,
    ) -> Result<usize, Stop> {
        let Some(replay) = &self.screens else {
            return Ok(self.choice(question, options, default));
        };
        let mut replay = replay.borrow_mut();
        match replay.next(question) {
            Some(Answer::Pick(n)) if n < options.len() => {
                replay.said.push(tui::Said::Answer(
                    question.to_string(),
                    options[n].0.to_string(),
                ));
                Ok(n)
            }
            answered => {
                if answered.is_some() {
                    replay.unanswer();
                }
                Err(Stop::Ask(Question {
                    text: question.to_string(),
                    ask: tui::Ask::Pick {
                        options: options
                            .iter()
                            .map(|(title, hint)| (title.to_string(), hint.to_string()))
                            .collect(),
                        at: default.min(options.len().saturating_sub(1)),
                    },
                }))
            }
        }
    }

    fn ask_yes_no(&mut self, question: &str, default: bool) -> Result<bool, Stop> {
        if self.screens.is_none() {
            return Ok(self.yes_no(question, default));
        }
        let no = usize::from(!default);
        Ok(self.ask_choice(question, &[("Yes", ""), ("No", "")], no)? == 0)
    }

    fn ask_line(&mut self, question: &str, default: &str) -> Result<String, Stop> {
        if self.screens.is_none() {
            return Ok(self.line(question, default));
        }
        self.ask_text(question, default, false, false)
    }

    /// A passphrase, twice and hidden.
    fn ask_secret(&mut self, question: &str) -> Result<Option<String>, Stop> {
        if self.screens.is_none() {
            return Ok(self.secret(question));
        }
        self.ask_twice(question, false)
    }

    /// A password for the desk, twice and hidden, held to the desk's rule.
    fn ask_password(&mut self, question: &str) -> Result<Option<String>, Stop> {
        if self.screens.is_none() {
            return Ok(self.password(question));
        }
        self.ask_twice(question, true)
    }

    /// A key a provider gave, once and hidden; empty for none.
    fn ask_hidden_once(&mut self, question: &str) -> Result<Option<String>, Stop> {
        if self.screens.is_none() {
            return Ok(self.hidden_once(question));
        }
        let text = self.ask_text(question, "", true, false)?;
        Ok(Some(text).filter(|t| !t.is_empty()))
    }

    /// Hidden text twice on a screen. A password for the desk is held to the
    /// desk's rule when it is typed the first time, so one the desk would
    /// refuse is asked for again here, not found out once the rest is installed.
    fn ask_twice(&mut self, question: &str, password: bool) -> Result<Option<String>, Stop> {
        loop {
            let first = self.ask_text(question, "", true, true)?;
            if let Some(refused) = password.then(|| desk_password_refusal(&first)).flatten() {
                self.note(refused);
                continue;
            }
            let again = self.ask_text("and again", "", true, true)?;
            if first == again {
                return Ok(Some(first));
            }
            self.note("those differ; once more");
        }
    }

    /// Text on a screen: the default already typed, to keep or change, or
    /// hidden and typed afresh.
    fn ask_text(
        &mut self,
        question: &str,
        default: &str,
        hidden: bool,
        required: bool,
    ) -> Result<String, Stop> {
        let Some(replay) = &self.screens else {
            return Ok(self.line(question, default));
        };
        let mut replay = replay.borrow_mut();
        match replay.next(question) {
            Some(Answer::Text(text)) => {
                let text = if text.is_empty() {
                    default.to_string()
                } else {
                    text
                };
                let shown = if text.is_empty() {
                    "none".to_string()
                } else if hidden {
                    "•".repeat(text.chars().count().min(12))
                } else {
                    text.clone()
                };
                replay
                    .said
                    .push(tui::Said::Answer(question.to_string(), shown));
                Ok(text)
            }
            answered => {
                if answered.is_some() {
                    replay.unanswer();
                }
                let text = if hidden {
                    String::new()
                } else {
                    default.to_string()
                };
                Err(Stop::Ask(Question {
                    text: question.to_string(),
                    ask: tui::Ask::Text {
                        cursor: text.chars().count(),
                        text,
                        hidden,
                        default: default.to_string(),
                        required,
                    },
                }))
            }
        }
    }

    /// Something asked of the world for the answers so far, such as whether a
    /// database answers: on lines once, and on the screens once for those
    /// answers, however often the questions are run again.
    fn probe<T: Clone + 'static>(&self, key: &str, make: impl FnOnce() -> T) -> T {
        let Some(replay) = &self.screens else {
            return make();
        };
        let at = replay.borrow().at;
        let found = replay
            .borrow()
            .probes
            .iter()
            .find(|(when, what, _)| *when == at && what == key)
            .and_then(|(_, _, value)| value.downcast_ref::<T>().cloned());
        if let Some(found) = found {
            return found;
        }
        let value = make();
        replay
            .borrow_mut()
            .probes
            .push((at, key.to_string(), Box::new(value.clone())));
        value
    }

    /// Ask the questions one screen at a time, and say what each step was
    /// answered with. Where the terminal cannot be put in raw mode, they are
    /// asked on lines.
    fn screens<T>(
        &mut self,
        mut questions: impl FnMut(&mut Console) -> Result<T, Stop>,
    ) -> Result<(T, Vec<String>), Exit> {
        let raw = tui::Raw::on(self.input_fd());
        if !raw.active() {
            drop(raw);
            println!(
                "{}",
                self.bold("NILS setup: the engine, the desk and the assistant")
            );
            return questions(self)
                .map(|value| (value, Vec::new()))
                .map_err(Stop::into_exit);
        }
        let screen = tui::Screen::enter();
        let mut tty = self.tty.take();
        let outcome = self.drive(
            questions,
            &mut || match tty.as_mut() {
                Some(tty) => tui::read_keys(tty.get_mut()),
                None => tui::read_keys(&mut std::io::stdin().lock()),
            },
            &mut |lines| screen.draw(&lines),
            &|| tui::size(1),
        );
        self.tty = tty;
        drop(screen);
        drop(raw);
        outcome
    }

    /// The screens, with the keys, the drawing and the terminal's size given.
    /// The questions are run from the start with the answers so far until one
    /// has none; that one is drawn and answered, and they are run again.
    /// Going back takes the last answer away, and offers it again when the
    /// same question comes up in the same place.
    fn drive<T>(
        &mut self,
        mut questions: impl FnMut(&mut Console) -> Result<T, Stop>,
        keys: &mut dyn FnMut() -> Vec<tui::Key>,
        draw: &mut dyn FnMut(Vec<String>),
        size: &dyn Fn() -> (usize, usize),
    ) -> Result<(T, Vec<String>), Exit> {
        self.screens = Some(std::cell::RefCell::new(Replay::default()));
        let mut pending = std::collections::VecDeque::new();
        let outcome = loop {
            if let Some(replay) = &self.screens {
                replay.borrow_mut().restart();
            }
            let Question { text, ask } = match questions(self) {
                Ok(value) => break Ok(value),
                Err(Stop::Exit(e)) => break Err(e),
                Err(Stop::Ask(question)) => question,
            };
            let Some(replay) = &self.screens else {
                break Err(fail("the screens closed with a question open"));
            };
            let position = replay.borrow().answers.len();
            let mut ask = match replay.borrow().offered.get(&position) {
                Some((asked, answer)) if *asked == text => offer(ask, answer),
                _ => ask,
            };
            let taken = loop {
                {
                    let replay = replay.borrow();
                    let steps: Vec<(&str, Option<String>)> = STEPS
                        .iter()
                        .copied()
                        .zip(replay.summaries.iter().cloned())
                        .collect();
                    draw(tui::screen(
                        self.palette,
                        size(),
                        &tui::View {
                            steps: &steps,
                            current: replay.step,
                            said: &replay.said,
                            question: &text,
                            ask: &ask,
                            back: !replay.answers.is_empty(),
                        },
                    ));
                }
                if pending.is_empty() {
                    pending.extend(keys());
                }
                if let Some(key) = pending.pop_front() {
                    let outcome = ask.key(&key);
                    if outcome != tui::Outcome::Stay {
                        break outcome;
                    }
                }
            };
            let mut replay = replay.borrow_mut();
            match taken {
                tui::Outcome::Answer => {
                    let answer = match &ask {
                        tui::Ask::Pick { at, .. } => Answer::Pick(*at),
                        tui::Ask::Text { text, .. } => Answer::Text(text.clone()),
                    };
                    // the answer taken back from here, given again, keeps the
                    // ones taken back after it on offer; another does not
                    if replay
                        .offered
                        .get(&position)
                        .is_some_and(|(_, was)| *was == answer)
                    {
                        replay.offered.remove(&position);
                    } else {
                        let _ = replay.offered.split_off(&position);
                    }
                    replay.answers.push((text, answer));
                }
                tui::Outcome::Back => {
                    if let Some((asked, answer)) = replay.answers.pop() {
                        let position = replay.answers.len();
                        replay.forget_from(position);
                        replay.offered.insert(position, (asked, answer));
                    }
                }
                tui::Outcome::Quit => {
                    break Err(Exit {
                        code: crate::STOPPED,
                        message: "setup was stopped; nothing was changed".to_string(),
                    });
                }
                tui::Outcome::Stay => {}
            }
        };
        let summaries = self
            .screens
            .take()
            .map(|replay| {
                replay
                    .into_inner()
                    .summaries
                    .into_iter()
                    .flatten()
                    .collect()
            })
            .unwrap_or_default();
        outcome.map(|value| (value, summaries))
    }

    /// The services as they were found: said as they are, or under a
    /// checklist, the running row marked when one does not run and what it
    /// said kept for after.
    fn report(&self, started: &Started) {
        if self.checklist.is_none() {
            print!("{}", started.text);
            return;
        }
        for service in started.services.iter().filter(|s| !s.running) {
            self.warn(&format!("not running: {}", service.unit));
            for line in &service.said {
                self.say(&format!("  {line}"));
            }
        }
    }

    fn read_line(&mut self) -> Option<String> {
        self.read_typed_line().map(|line| line.trim().to_string())
    }

    /// A line as it was typed, only its line ending taken off: a password's
    /// spaces are part of it, as the desk's sign-in counts them.
    fn read_typed_line(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = match self.tty.as_mut() {
            Some(tty) => tty.read_line(&mut line).ok()?,
            None => std::io::stdin().read_line(&mut line).ok()?,
        };
        (read > 0).then(|| line.trim_end_matches(['\n', '\r']).to_string())
    }

    /// One of a list, by number. Returns the index chosen.
    fn choice(&mut self, question: &str, options: &[(&str, &str)], default: usize) -> usize {
        println!("  {question}");
        for (i, (title, hint)) in options.iter().enumerate() {
            let mark = if i == default { ">" } else { " " };
            println!("   {mark} {}. {}", i + 1, self.bold(title));
            if !hint.is_empty() {
                println!("        {}", self.dim(hint));
            }
        }
        if !self.interactive() {
            println!("  {}", self.dim(&format!("taking {}", options[default].0)));
            return default;
        }
        loop {
            print!("  [{}] ", default + 1);
            let _ = std::io::stdout().flush();
            match self.read_line() {
                None => return default,
                Some(answer) if answer.is_empty() => return default,
                Some(answer) => match answer.parse::<usize>() {
                    Ok(n) if n >= 1 && n <= options.len() => return n - 1,
                    _ => println!("  {}", self.dim("a number from the list")),
                },
            }
        }
    }

    /// A line of text, with a default. An empty default may come back empty.
    fn line(&mut self, question: &str, default: &str) -> String {
        // An empty default is no default, and `[]` says nothing.
        let hint = if default.is_empty() {
            ":".to_string()
        } else {
            format!(" [{default}]")
        };
        if !self.interactive() {
            println!("  {question}{}", self.dim(&hint));
            return default.to_string();
        }
        print!("  {question}{hint} ");
        let _ = std::io::stdout().flush();
        match self.read_line() {
            Some(answer) if !answer.is_empty() => answer,
            _ => default.to_string(),
        }
    }

    fn yes_no(&mut self, question: &str, default: bool) -> bool {
        let hint = if default { "Y/n" } else { "y/N" };
        if !self.interactive() {
            println!("  {question} {}", self.dim(&format!("[{hint}]")));
            return default;
        }
        loop {
            print!("  {question} [{hint}] ");
            let _ = std::io::stdout().flush();
            match self.read_line().as_deref() {
                None | Some("") => return default,
                Some("y" | "Y" | "yes") => return true,
                Some("n" | "N" | "no") => return false,
                _ => {}
            }
        }
    }

    /// A passphrase, twice, without echoing it.
    fn secret(&mut self, question: &str) -> Option<String> {
        self.hidden_twice(question, "a passphrase", false)
    }

    /// A password for the desk, twice, without echoing it, as typed and held
    /// to the desk's rule.
    fn password(&mut self, question: &str) -> Option<String> {
        self.hidden_twice(question, "a password", true)
    }

    fn hidden_twice(&mut self, question: &str, noun: &str, password: bool) -> Option<String> {
        if !self.interactive() {
            return None;
        }
        loop {
            let first = self.secret_once(question, password)?;
            if first.is_empty() {
                println!("  {}", self.dim(&format!("{noun} is needed")));
                continue;
            }
            if let Some(refused) = password.then(|| desk_password_refusal(&first)).flatten() {
                println!("  {}", self.dim(refused));
                continue;
            }
            let again = self.secret_once("and again", password)?;
            if first == again {
                return Some(first);
            }
            println!("  {}", self.dim("those differ; once more"));
        }
    }

    /// Once, hidden: a key a provider gave, which is pasted rather than
    /// chosen, so asking for it twice would only invite a second paste.
    fn hidden_once(&mut self, question: &str) -> Option<String> {
        if !self.interactive() {
            return None;
        }
        self.secret_once(question, false).filter(|s| !s.is_empty())
    }

    /// A slow thing, said in one line with a timer that runs while it
    /// works. Its own output is kept out of sight: a person installing NILS
    /// does not need a clone's object counts or a package manager's
    /// deprecation notices, and a fresh install should not open on a screen
    /// of warnings. When it fails, the end of that output is the first thing
    /// shown, because then it is the whole story.
    fn task(&self, label: &str, dir: &Path, program: &str, args: &[&str]) -> Result<(), Exit> {
        use std::sync::{Arc, Mutex};
        let started = std::time::Instant::now();
        let drawn = self.checklist.as_ref().map(|(live, _)| live);
        if let Some(checklist) = drawn {
            checklist.detail(&brief(label));
        }
        // the one line with its timer, where no checklist is drawn
        let live = drawn.is_none() && std::io::stdout().is_terminal();
        let mut child = Command::new(program)
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| fail(format!("{program}: {e}")))?;
        let heard = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut readers = Vec::new();
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let heard = Arc::clone(&heard);
            readers.push(std::thread::spawn(move || {
                let mut stream = stream;
                let mut chunk = [0u8; 4096];
                while let Ok(n) = std::io::Read::read(&mut stream, &mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut all) = heard.lock() {
                        all.extend_from_slice(&chunk[..n]);
                    }
                }
            }));
        }
        if !live && drawn.is_none() {
            println!("  {label}");
        }
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            if live {
                print!(
                    "\r  {label} {}",
                    self.dim(&format!("{}s", started.elapsed().as_secs()))
                );
                let _ = std::io::stdout().flush();
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        };
        for reader in readers {
            let _ = reader.join();
        }
        let took = started.elapsed().as_secs();
        if status.success() {
            if drawn.is_some() {
                return Ok(());
            }
            if live {
                print!("\r\x1b[2K");
            }
            println!("  {label} {}", self.dim(&format!("done in {took}s")));
            return Ok(());
        }
        let text = heard
            .lock()
            .map(|all| String::from_utf8_lossy(&all).to_string())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let tail = &lines[lines.len().saturating_sub(25)..];
        if drawn.is_some() {
            let mut later = self.later.borrow_mut();
            later.push(format!(
                "  {label} {}",
                self.dim(&format!("failed after {took}s"))
            ));
            later.extend(tail.iter().map(|line| format!("    {line}")));
        } else {
            if live {
                print!("\r\x1b[2K");
            }
            println!("  {label} {}", self.dim(&format!("failed after {took}s")));
            for line in tail {
                println!("    {line}");
            }
        }
        Err(fail(format!("{program} {} failed", args.join(" "))))
    }

    fn secret_once(&mut self, question: &str, as_typed: bool) -> Option<String> {
        print!("  {question}: ");
        let _ = std::io::stdout().flush();
        let guard = EchoOff::on(self.tty.as_ref().map(BufReader::get_ref));
        let line = if as_typed {
            self.read_typed_line()
        } else {
            self.read_line()
        };
        drop(guard);
        println!();
        line
    }
}

/// The terminal's echo, off while a passphrase is typed and on again after.
struct EchoOff {
    #[cfg(unix)]
    saved: Option<(i32, libc::termios)>,
}

impl EchoOff {
    #[cfg(unix)]
    #[allow(
        unsafe_code,
        reason = "tcgetattr and tcsetattr fill and read a plain struct through a pointer"
    )]
    fn on(tty: Option<&std::fs::File>) -> EchoOff {
        use std::os::unix::io::AsRawFd;
        let fd = match tty {
            Some(file) => file.as_raw_fd(),
            None => 0,
        };
        let mut term = std::mem::MaybeUninit::<libc::termios>::zeroed();
        // SAFETY: `fd` is an open descriptor and the struct outlives the call.
        if unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) } != 0 {
            return EchoOff { saved: None };
        }
        // SAFETY: tcgetattr returned 0, so the struct is initialised.
        let saved = unsafe { term.assume_init() };
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        // SAFETY: `quiet` is a copy of a struct the kernel just filled.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
        EchoOff {
            saved: Some((fd, saved)),
        }
    }

    #[cfg(not(unix))]
    fn on(_tty: Option<&std::fs::File>) -> EchoOff {
        EchoOff {}
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        #[allow(unsafe_code, reason = "tcsetattr puts back the struct it gave")]
        if let Some((fd, saved)) = self.saved {
            // SAFETY: `saved` is the struct tcgetattr filled for this fd.
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
        }
    }
}

// -------------------------------------------------------------- the state

/// What a setup left behind, so the next one knows what is there.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    pub(crate) dir: String,
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) runtime: String,
    #[serde(default)]
    pub(crate) service: String,
    #[serde(default)]
    pub(crate) reach: String,
    /// The address a browser opens the desk at, where a proxy answers for
    /// it; empty where the desk is opened where it binds. Recorded, so that
    /// an update and a repair write the same origin the install did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) origin: String,
    #[serde(default)]
    pub(crate) backend: String,
    #[serde(default)]
    pub(crate) at: String,
    #[serde(default)]
    pub(crate) ports: Ports,
    #[serde(default)]
    pub(crate) places: Vec<PlaceState>,
    #[serde(default)]
    pub(crate) parts: BTreeMap<String, PartState>,
    /// Programs this install put on the machine that no part names: the nils
    /// that set up a container install, the nils-desk its image was made
    /// from, and a binary a part ran from before it moved into a container.
    /// An uninstall removes these and the parts' own, and nothing else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) programs: Vec<String>,
    /// Set from the moment an install starts placing anything until it
    /// finishes, so one that stops partway is still on record for an
    /// uninstall and is started again by the next setup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) unfinished: bool,
    /// The provider the desk signs people in at, in `oidc` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) oidc: Option<OidcPlan>,
    /// The services of this machine, where the install has them: the account
    /// each part runs as and the capabilities the engine keeps. Recorded, so
    /// that an update and a repair write the services this install has
    /// rather than falling back to this account's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) system: Option<SystemUnits>,
}

impl State {
    fn keep_program(&mut self, path: &Path) {
        let path = path.display().to_string();
        if !self.programs.contains(&path) {
            self.programs.push(path);
        }
    }
}

/// One place the wizard declared, kept so a repair knows what it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlaceState {
    pub(crate) name: String,
    pub(crate) role: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PartState {
    pub(crate) version: String,
    pub(crate) path: String,
    /// How this part runs: `binary`, `podman`, `docker` or `node`.
    #[serde(default = "binary_kind")]
    pub(crate) kind: String,
}

fn binary_kind() -> String {
    "binary".to_string()
}

/// `$XDG_CONFIG_HOME/nils/setup.toml`, else `~/.config/nils/setup.toml`.
pub(crate) fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("nils").join("setup.toml")
}

pub(crate) fn read_state() -> Option<State> {
    let text = std::fs::read_to_string(state_path()).ok()?;
    toml::from_str(&text).ok()
}

fn write_state(state: &State) -> Result<PathBuf, Exit> {
    let path = state_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    }
    let text = toml::to_string(state).map_err(|e| fail(e.to_string()))?;
    std::fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    Ok(path)
}

// --------------------------------------------------------------- the plan

/// What a person answered that is not the shape of the install: the first
/// person and the model. Asked with every other question and shown on the
/// plan, so that once the person says "Do it" there is only work to watch.
/// Asked in the middle of the work instead, a person who had walked away
/// came back to a prompt, and one who stayed could not tell a question from
/// a hang.
#[derive(Default)]
struct Answers {
    first: Option<(String, String)>,
    model: Option<ModelChoice>,
    passphrase: Option<String>,
    provider: Option<Provider>,
}

/// How the desk comes to be registered at its provider.
enum Provider {
    /// Setup registers it at an Authentik, with an API token of it.
    Authentik {
        url: String,
        token: String,
        users: String,
        admins: String,
    },
    /// Registered already: the client's secret, to keep beside the desk.
    Registered { secret: Option<String> },
}

/// What the wizard decided, before it does any of it.
#[derive(Clone)]
pub(crate) struct Plan {
    pub(crate) dir: PathBuf,
    pub(crate) parts: Vec<Part>,
    pub(crate) mode: Mode,
    pub(crate) runtime: Runtime,
    pub(crate) backend: BackendChoice,
    pub(crate) ports: Ports,
    pub(crate) reach: Reach,
    pub(crate) source: Option<PathBuf>,
    /// The registry's source places, by name: the directories of DICOM added
    /// at the desk or with nils place add, which the engine is given as it is
    /// given the one setup asked for.
    pub(crate) sources: Vec<(String, PathBuf)>,
    pub(crate) registry_exists: bool,
    pub(crate) service: bool,
    pub(crate) channel: Option<String>,
    pub(crate) version: String,
    /// Whether the pod is given this machine's loopback, so Kvasir or an
    /// engine inside it reaches a model server or a Postgres listening on
    /// 127.0.0.1 here. Rootless podman with pasta, and only when something
    /// in the pod needs it.
    pub(crate) host_loopback: bool,
    /// The Postgres this setup runs for the registry, when it runs one.
    pub(crate) postgres: Option<ManagedPostgres>,
    /// The provider the desk signs people in at, in `oidc` mode, once named.
    pub(crate) oidc: Option<OidcPlan>,
    /// The llama.cpp build the models Kvasir starts run on, where the plan
    /// has the assistant and llama.cpp publishes a build for this machine.
    pub(crate) llama: Option<Llama>,
    /// The services of this machine, asked for with `--system`: `None` where
    /// the services are the account's own, which is every install on a
    /// laptop.
    pub(crate) system: Option<SystemUnits>,
}

/// The provider the desk signs people in at, as the desk, the engine and
/// Kvasir are each told of it. The client's secret is not here but in a
/// file beside the desk's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OidcPlan {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    /// Where the provider publishes the keys its tokens are signed with.
    pub(crate) jwks: String,
    /// The claim that carries the entitlements, by their own names.
    pub(crate) roles_claim: String,
    /// The scopes the desk asks for, where not the desk's own, which include
    /// the entitlements scope that a registration at Authentik makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scopes: Option<Vec<String>>,
}

/// The ways the parts can run here, in the order they are offered: the
/// machine always, then each container runtime this machine has.
fn runtime_choices(podman: bool, docker: bool) -> Vec<(Runtime, &'static str, &'static str)> {
    let mut out = vec![(
        Runtime::Machine,
        "On this machine",
        "two small binaries and a directory",
    )];
    if podman {
        out.push((
            Runtime::Podman,
            "In containers (podman)",
            "one pod holding every part, run as you, kept running by systemd",
        ));
    }
    if docker {
        out.push((
            Runtime::Docker,
            "In containers (docker)",
            "one container per part on a docker network, kept running by docker",
        ));
    }
    out
}

/// Whether docker here is docker and answers. A `docker` command that is
/// podman's compatibility wrapper is podman, which is offered as itself; one
/// whose daemon is not running, or that this account may not use, would fail
/// at the first pull.
fn docker_answers() -> Result<(), DockerAbsent> {
    let Some(version) = run_quiet("docker", &["--version"]) else {
        return Err(DockerAbsent::NotInstalled);
    };
    if version.to_lowercase().contains("podman") {
        return Err(DockerAbsent::PodmanWrapper);
    }
    match run_quiet("docker", &["version", "--format", "{{.Server.Version}}"]) {
        Some(server) if !server.trim().is_empty() => Ok(()),
        _ => Err(DockerAbsent::NoDaemon),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockerAbsent {
    NotInstalled,
    PodmanWrapper,
    NoDaemon,
}

/// Why docker is not among the choices, when that is worth a sentence.
fn docker_absent_reason(docker: Result<(), DockerAbsent>) -> Option<String> {
    match docker {
        Err(DockerAbsent::NoDaemon) => Some(
            "docker is here but does not answer, so it is not offered: its daemon is not \
             running, or this account may not use it (the docker group)"
                .to_string(),
        ),
        _ => None,
    }
}

/// Whether podman here networks a rootless pod through pasta, which is what
/// can hand the pod this machine's loopback.
fn podman_has_pasta() -> bool {
    // asked once: the screens ask their questions again after every answer
    static PASTA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PASTA.get_or_init(|| {
        run_quiet(
            "podman",
            &["info", "--format", "{{.Host.Pasta.Executable}}"],
        )
        .is_some_and(|s| !s.trim().is_empty())
    })
}

impl Plan {
    fn registry(&self) -> PathBuf {
        self.dir.join("registry")
    }

    fn desk_dir(&self) -> PathBuf {
        self.dir.join("desk")
    }

    fn desk_config(&self) -> PathBuf {
        self.desk_dir().join("nils-desk.toml")
    }

    /// Where llama.cpp and Kvasir both read the runtime's presets and key,
    /// and where llama.cpp writes its log, at the same path inside a
    /// container and out.
    fn runtime_dir(&self) -> PathBuf {
        self.dir.join("kvasir").join("runtime")
    }

    /// The data of a Postgres this setup runs, and its environment, which
    /// holds the password.
    fn postgres_dir(&self) -> PathBuf {
        self.dir.join("postgres")
    }

    fn postgres_env(&self) -> PathBuf {
        self.dir.join("postgres.env")
    }

    /// The tag the published images carry, for the version this plan holds.
    fn tag(&self) -> String {
        image_tag(&self.version)
    }

    fn has(&self, part: Part) -> bool {
        self.parts.contains(&part)
    }

    /// The directories the engine reads, each by its place's name: the one
    /// setup asked for first, under the name it declares it by, then the
    /// registry's other source places.
    fn read_from(&self) -> Vec<(String, PathBuf)> {
        let mut out: Vec<(String, PathBuf)> = self
            .source
            .iter()
            .map(|path| ("source".to_string(), path.clone()))
            .collect();
        for (name, path) in &self.sources {
            if !out.iter().any(|(n, p)| n == name || p == path) {
                out.push((name.clone(), path.clone()));
            }
        }
        out
    }
}

/// The plan a recorded setup describes, so `--update` and a repair need ask
/// nothing. What the state does not carry is what the plan does not use: a
/// Postgres connection string is only needed to make a registry, and neither
/// of those two makes one.
pub(crate) fn plan_from_state(state: &State, channel: Option<&str>) -> Plan {
    let dir = PathBuf::from(&state.dir);
    let mut parts = vec![Part::Engine];
    if state.parts.contains_key("desk") {
        parts.push(Part::Desk);
    }
    if state.parts.contains_key("assistant") {
        parts.push(Part::Assistant);
    }
    Plan {
        dir: dir.clone(),
        parts,
        mode: Mode::parse(&state.mode).unwrap_or(Mode::Off),
        runtime: Runtime::parse(&state.runtime).unwrap_or(Runtime::Machine),
        backend: match state.backend.strip_prefix("postgres:") {
            Some(schema) => BackendChoice::Postgres {
                dsn: registry_dsn(&dir).unwrap_or_default(),
                schema: schema.to_string(),
            },
            None => BackendChoice::Sqlite,
        },
        ports: state.ports,
        reach: recorded_reach(state),
        source: state
            .places
            .iter()
            .find(|p| p.name == "source" && p.role == "source")
            .map(|p| PathBuf::from(&p.path)),
        sources: registry_sources(&dir).unwrap_or_else(|| recorded_sources(&state.places)),
        registry_exists: true,
        service: !state.service.is_empty() && state.service != "none",
        channel: channel.map(str::to_string),
        version: update::VERSION.to_string(),
        host_loopback: state.runtime == "podman"
            && (state.parts.contains_key("assistant")
                || state.parts.contains_key("desk")
                || registry_dsn(&dir).is_some_and(|d| d.contains("host.containers.internal")))
            && podman_has_pasta(),
        postgres: state
            .parts
            .get("postgres")
            .and_then(|p| Runtime::parse(&p.kind).ok())
            .filter(|r| r.container())
            .map(|runtime| ManagedPostgres { runtime }),
        oidc: state.oidc.clone(),
        llama: llama_of_state(state),
        system: state.system.clone(),
    }
}

/// Where a recorded install's desk answers and where it binds: the origin a
/// proxy of the site's answers at, where one was named, and otherwise the
/// address the record keeps. Read by the plan an update and a repair are
/// made from, so that neither writes an origin the install never had.
fn recorded_reach(state: &State) -> Reach {
    let network = !matches!(state.reach.as_str(), "" | "loopback");
    match state.origin.trim() {
        "" if network => Reach::Network(state.reach.clone()),
        "" => Reach::Loopback,
        origin => Reach::Behind {
            origin: origin.to_string(),
            network,
        },
    }
}

/// The llama.cpp build of a recorded install with the assistant: the one on
/// record, or where none is on record yet, the one this machine takes.
fn llama_of_state(state: &State) -> Option<Llama> {
    let runtime = Runtime::parse(&state.runtime).unwrap_or(Runtime::Machine);
    if !state.parts.contains_key("assistant") || (runtime.container() && cfg!(target_os = "macos"))
    {
        return None;
    }
    match state
        .parts
        .get(LLAMA_PART)
        .and_then(|p| llama_recorded(&p.path))
    {
        Some(variant) => Some(Llama {
            variant,
            loader: !variant.contains("vulkan") || vulkan_loader(),
        }),
        None => llama_here(probe_card().is_some()),
    }
}

/// The source places a registry holds, by name, read as the registry stands.
/// A registry this binary would have to migrate first is not opened, and the
/// setup's own record stands in for it.
fn registry_sources(dir: &Path) -> Option<Vec<(String, PathBuf)>> {
    use nils_registry::place::{self, Role};
    let home = Home::new(dir.join("registry"));
    if !home.exists() {
        return None;
    }
    let mut store = home.open_as_it_stands().ok()?;
    let places = place::active(&mut store).ok()?;
    Some(
        places
            .into_iter()
            .filter(|p| p.role == Role::Source)
            .map(|p| (p.name, PathBuf::from(p.path)))
            .collect(),
    )
}

/// The source places a setup recorded when it last declared its places.
fn recorded_sources(places: &[PlaceState]) -> Vec<(String, PathBuf)> {
    places
        .iter()
        .filter(|p| p.role == "source")
        .map(|p| (p.name.clone(), PathBuf::from(&p.path)))
        .collect()
}

fn default_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("nils"))
        .unwrap_or_else(|| PathBuf::from("nils"))
}

/// A path with `~` for the home directory, as a person writes it.
fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

// ------------------------------------------------- ports and reachability

/// Whether anything already listens on a port of this machine.
fn port_taken(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// Where a part should listen: `None` when its port is free, else the next
/// port that nothing on this machine holds and no other part was given.
fn settle_port(port: u16, chosen: &[u16], taken: &dyn Fn(u16) -> bool) -> Option<u16> {
    let held = |p: u16| taken(p) || chosen.contains(&p);
    held(port).then(|| next_free_port(port.saturating_add(1), &held))
}

/// The first free port at or after `start`, asking `taken` about each.
pub(crate) fn next_free_port(start: u16, taken: &dyn Fn(u16) -> bool) -> u16 {
    let mut port = start;
    for _ in 0..200 {
        if !taken(port) {
            return port;
        }
        port = port.saturating_add(1);
    }
    start
}

/// This host's own address on the network, for a desk other machines open.
fn host_address() -> Option<String> {
    // The address the kernel would use to reach the world, without sending
    // anything: a connected UDP socket names its own end.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}

/// What the desk's three address keys are, for a reach and a port. The
/// binding and the origin are two answers and not one: behind a proxy the
/// desk is bound where only the proxy reaches it and answers at a name with
/// a certificate, and either address on its own would be wrong.
pub(crate) fn desk_binding(
    reach: &Reach,
    port: u16,
    container: bool,
) -> (String, String, Vec<String>) {
    // whatever it answers at, the desk is opened on this machine too: from a
    // browser here, and by whatever asks it for its keys
    let here = || {
        vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ]
    };
    match reach {
        Reach::Loopback => (
            if container {
                format!("0.0.0.0:{port}")
            } else {
                format!("127.0.0.1:{port}")
            },
            format!("http://127.0.0.1:{port}"),
            vec![format!("http://localhost:{port}")],
        ),
        Reach::Network(addr) => (
            format!("0.0.0.0:{port}"),
            format!("http://{addr}:{port}"),
            here(),
        ),
        Reach::Behind { origin, network } => (
            if container || *network {
                format!("0.0.0.0:{port}")
            } else {
                format!("127.0.0.1:{port}")
            },
            origin.clone(),
            here(),
        ),
    }
}

/// The origin a person gave, as the desk will hold it: a scheme and a host,
/// with nothing after them. It is the address a browser opens, so what is
/// wrong with one that cannot be is said in those terms.
fn origin_given(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(
            "an origin is the address a browser opens the desk at, such as \
             https://nils.example.org"
                .to_string(),
        );
    }
    if text.split_whitespace().count() > 1 {
        return Err(format!(
            "{text} has a space in it, so it is not one address"
        ));
    }
    let Some((scheme, rest)) = text.split_once("://") else {
        return Err(format!("{text} has no scheme: write https://{text}"));
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(format!(
            "{scheme} is not a scheme a browser opens the desk on: http or https"
        ));
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if let Some(at) = rest.find(['/', '?', '#']) {
        return Err(format!(
            "an origin is a scheme and a host with nothing after them, so leave off {}",
            &rest[at..]
        ));
    }
    if rest.contains('@') {
        return Err(format!(
            "an origin carries no sign in, so leave off what is before the @ in {rest}"
        ));
    }
    let (host, port) = match rest.rsplit_once(':') {
        // [::1]:7200 is a host and a port; [::1] is a host
        Some((host, port)) if !host.ends_with(']') => (host, Some(port)),
        _ => (rest, None),
    };
    if host.is_empty() {
        return Err(format!(
            "{text} names no host: write https://nils.example.org"
        ));
    }
    let named =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':');
    if !host.chars().all(named) {
        return Err(format!("{host} is not a host a browser can open"));
    }
    if let Some(port) = port
        && port.parse::<u16>().is_err()
    {
        return Err(format!("{port} is not a port: a number up to 65535"));
    }
    // a browser sends the host lower case, and the desk compares what it is
    // given with what it holds
    Ok(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        rest.to_ascii_lowercase()
    ))
}

// ------------------------------------------------------------- the places

/// One place the engine will keep, in the order they must be added: a
/// backup place exists before the registry that names it.
pub(crate) struct PlaceSpec {
    pub(crate) name: &'static str,
    pub(crate) role: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) backup: Option<&'static str>,
}

/// The places a fresh install declares: where the data is read from, where
/// the registry is, where work in flight goes, where archives go, and where
/// a release may be written.
pub(crate) fn place_specs(plan: &Plan) -> Vec<PlaceSpec> {
    let mut out = vec![PlaceSpec {
        name: "backups",
        role: "backup",
        path: plan.dir.join("backups"),
        backup: None,
    }];
    if let Some(source) = &plan.source {
        out.push(PlaceSpec {
            name: "source",
            role: "source",
            path: source.clone(),
            backup: None,
        });
    }
    out.push(PlaceSpec {
        name: "registry",
        role: "registry",
        path: plan.registry(),
        backup: Some("backups"),
    });
    out.push(PlaceSpec {
        name: "working",
        role: "working",
        path: plan.dir.join("working"),
        backup: None,
    });
    out.push(PlaceSpec {
        name: "export",
        role: "export",
        path: plan.dir.join("export"),
        backup: None,
    });
    out
}

/// One place as the command a person would type.
pub(crate) fn place_argv(spec: &PlaceSpec) -> Vec<String> {
    let mut argv = vec![
        "nils".to_string(),
        "place".to_string(),
        "add".to_string(),
        spec.name.to_string(),
        spec.path.display().to_string(),
        "--role".to_string(),
        spec.role.to_string(),
    ];
    if let Some(backup) = spec.backup {
        argv.push("--backup".to_string());
        argv.push(backup.to_string());
    }
    argv
}

// ---------------------------------------------------------- the containers

/// The engine's own arguments, wherever it runs.
fn engine_args(plan: &Plan, registry: &str, backups: &str) -> Vec<String> {
    let mut argv = vec![
        "serve".to_string(),
        "--bind".to_string(),
        if plan.runtime.container() {
            format!("0.0.0.0:{}", plan.ports.engine)
        } else {
            format!("127.0.0.1:{}", plan.ports.engine)
        },
        "--registry".to_string(),
        registry.to_string(),
        "--backup-dir".to_string(),
        backups.to_string(),
        // the jobs the desk queues, a digest or a backup, run beside the doors
        "--worker".to_string(),
    ];
    match plan.mode {
        Mode::Off => {
            argv.push("--auth".to_string());
            argv.push("off".to_string());
        }
        Mode::Local => {
            let (issuer, jwks) = desk_trust(plan);
            argv.push("--auth".to_string());
            argv.push("oidc".to_string());
            argv.push("--oidc-trust".to_string());
            argv.push(format!(
                "issuer={issuer},audience=nils,jwks={jwks},keep_subject=true"
            ));
            argv.push("--oidc-groups-claim".to_string());
            argv.push("roles".to_string());
            for role in ["reader", "reviewer", "operator", "admin"] {
                argv.push("--role".to_string());
                argv.push(format!("{role}={role}"));
            }
        }
        Mode::Oidc => {
            argv.push("--auth".to_string());
            argv.push("oidc".to_string());
            // the provider's tokens, for a command line signed in there; with
            // none named the engine refuses to start, and setup says so
            if let Some(oidc) = &plan.oidc {
                argv.push("--oidc-trust".to_string());
                argv.push(format!(
                    "issuer={},audience={},jwks={}",
                    oidc.issuer, oidc.client_id, oidc.jwks
                ));
                argv.push("--oidc-groups-claim".to_string());
                argv.push(oidc.roles_claim.clone());
                for role in ["reader", "reviewer", "operator", "admin"] {
                    argv.push("--role".to_string());
                    argv.push(format!("{role}={role}"));
                }
                // and the desk's, which it signs for a person once the
                // provider has said who they are (record 25)
                let (issuer, jwks) = desk_trust(plan);
                argv.push("--oidc-trust".to_string());
                argv.push(format!(
                    "issuer={issuer},audience=nils,jwks={jwks},keep_subject=true"
                ));
            }
        }
    }
    for (name, path) in plan.read_from() {
        argv.push("--ingest-root".to_string());
        argv.push(format!("{name}={}", path.display()));
    }
    argv
}

/// The desk as an issuer, for a part that trusts the tokens it signs: in
/// `local` mode, and in `oidc` mode for a person the provider named (record
/// 25). The issuer is what the desk writes into its tokens, which is
/// its own origin; the keys are fetched by the part, so that address is one a
/// part reaches from where it runs.
fn desk_trust(plan: &Plan) -> (String, String) {
    let (_, issuer, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    let keys = match plan.runtime {
        Runtime::Docker => format!("http://nils-desk:{}", plan.ports.desk),
        _ => format!("http://127.0.0.1:{}", plan.ports.desk),
    };
    (issuer, format!("{keys}/.well-known/jwks.json"))
}

/// How the desk's port is published: on this machine's loopback where only
/// this machine opens the desk, which includes a proxy running here, and on
/// every address where another machine reaches it.
fn desk_publish(plan: &Plan) -> String {
    let p = plan.ports.desk;
    if plan.reach.beyond_loopback() {
        format!("{p}:{p}")
    } else {
        format!("127.0.0.1:{p}:{p}")
    }
}

/// The commands a podman run is, in order, exactly as a person would type
/// them. `--print` shows these and the wizard runs them.
pub(crate) fn podman_commands(plan: &Plan) -> Vec<String> {
    let publish = desk_publish(plan);
    let mut pod = format!("podman pod create --name nils -p {publish}");
    if plan.has(Part::Assistant) {
        // Kvasir, on this machine's loopback only, for what setup asks of it
        let _ = write!(pod, " -p 127.0.0.1:{p}:{p}", p = plan.ports.kvasir);
    }
    if plan.host_loopback {
        let _ = write!(
            pod,
            " --network pasta:--map-host-loopback={HOST_LOOPBACK_IN_POD}"
        );
    }
    let mut out = vec![pod];
    // The registry and the backups are mounted at the paths they have on this
    // machine, so the places the registry records are paths the engine sees.
    let (registry, backups) = (
        plan.registry().display().to_string(),
        plan.dir.join("backups").display().to_string(),
    );
    let mut engine =
        format!("podman run -d --pod nils --name nils-engine -v {registry}:{registry}:U");
    for (_, path) in plan.read_from() {
        let _ = write!(engine, " -v {0}:{0}:ro", path.display());
    }
    let _ = write!(
        engine,
        " -v {backups}:{backups}:U {ENGINE_IMAGE}:{} {}",
        plan.tag(),
        engine_args(plan, &registry, &backups).join(" ")
    );
    out.push(engine);
    if plan.has(Part::Desk) {
        out.push(format!(
            "podman run -d --pod nils --name nils-desk -v {}:{IN_DESK}:U {DESK_IMAGE}:{} serve --config {IN_DESK}/nils-desk.toml",
            plan.desk_dir().display(),
            plan.tag()
        ));
    }
    if plan.has(Part::Assistant) {
        let (kvasir, assistant) = (plan.dir.join("kvasir"), plan.dir.join("assistant"));
        out.push(format!(
            "podman run -d --pod nils --name nils-kvasir -v {k}:{k} -w {k} {NODE_IMAGE} node dist/main.js --config kvasir.json",
            k = kvasir.display()
        ));
        out.push(format!(
            "podman run -d --pod nils --name nils-assistant --env-file {a}/assistant.env -v {a}:{a} -v {k}:{k}:ro -w {a} {NODE_IMAGE} node {entry}",
            a = assistant.display(),
            k = kvasir.display(),
            entry = assistant_entry(&assistant)
        ));
    }
    out
}

/// The same for docker, which has no pod: a network of its own, the desk
/// publishing the port, and no `:U` because it does not remap the user.
pub(crate) fn docker_commands(plan: &Plan) -> Vec<String> {
    let publish = desk_publish(plan);
    let mut out = vec!["docker network create nils".to_string()];
    let (registry, backups) = (
        plan.registry().display().to_string(),
        plan.dir.join("backups").display().to_string(),
    );
    let mut engine = format!(
        "docker run -d --network nils --name nils-engine {}{}-v {registry}:{registry}",
        docker_user(),
        if matches!(plan.backend, BackendChoice::Postgres { .. }) {
            "--add-host host.docker.internal:host-gateway "
        } else {
            ""
        },
    );
    for (_, path) in plan.read_from() {
        let _ = write!(engine, " -v {0}:{0}:ro", path.display());
    }
    let _ = write!(
        engine,
        " -v {backups}:{backups} {ENGINE_IMAGE}:{} {}",
        plan.tag(),
        engine_args(plan, &registry, &backups).join(" ")
    );
    out.push(engine);
    if plan.has(Part::Desk) {
        out.push(format!(
            "docker run -d --network nils --name nils-desk {}-p {publish} --add-host host.docker.internal:host-gateway -v {}:{IN_DESK} {DESK_IMAGE}:{} serve --config {IN_DESK}/nils-desk.toml",
            docker_user(),
            plan.desk_dir().display(),
            plan.tag()
        ));
    }
    if plan.has(Part::Assistant) {
        let (kvasir, assistant) = (plan.dir.join("kvasir"), plan.dir.join("assistant"));
        out.push(format!(
            "docker run -d --network nils --name nils-kvasir {user}-p 127.0.0.1:{p}:{p} --add-host host.docker.internal:host-gateway -v {k}:{k} -w {k} {NODE_IMAGE} node dist/main.js --config kvasir.json",
            user = docker_user(),
            p = plan.ports.kvasir,
            k = kvasir.display()
        ));
        out.push(format!(
            "docker run -d --network nils --name nils-assistant {user}--env-file {a}/assistant.env -v {a}:{a} -v {k}:{k}:ro -w {a} {NODE_IMAGE} node {entry}",
            user = docker_user(),
            a = assistant.display(),
            k = kvasir.display(),
            entry = assistant_entry(&assistant)
        ));
    }
    out
}

/// A compose file for docker, so the two containers come back at boot.
pub(crate) fn docker_compose(plan: &Plan) -> String {
    let mut out = String::from("# Written by nils setup.\nservices:\n");
    let _ = writeln!(out, "  engine:");
    let _ = writeln!(out, "    image: {ENGINE_IMAGE}:{}", plan.tag());
    let _ = writeln!(out, "    container_name: nils-engine");
    if !as_this_account().is_empty() {
        let _ = writeln!(out, "    user: \"{}\"", as_this_account());
    }
    let _ = writeln!(out, "    restart: unless-stopped");
    let (registry, backups) = (
        plan.registry().display().to_string(),
        plan.dir.join("backups").display().to_string(),
    );
    let _ = writeln!(
        out,
        "    command: {}",
        engine_args(plan, &registry, &backups).join(" ")
    );
    if matches!(plan.backend, BackendChoice::Postgres { .. }) {
        let _ = writeln!(out, "    extra_hosts:");
        let _ = writeln!(out, "      - \"host.docker.internal:host-gateway\"");
    }
    let _ = writeln!(out, "    volumes:");
    let _ = writeln!(out, "      - {registry}:{registry}");
    let _ = writeln!(out, "      - {backups}:{backups}");
    for (_, path) in plan.read_from() {
        let _ = writeln!(out, "      - {0}:{0}:ro", path.display());
    }
    if plan.has(Part::Desk) {
        let publish = desk_publish(plan);
        let _ = writeln!(out, "  desk:");
        let _ = writeln!(out, "    image: {DESK_IMAGE}:{}", plan.tag());
        let _ = writeln!(out, "    container_name: nils-desk");
        if !as_this_account().is_empty() {
            let _ = writeln!(out, "    user: \"{}\"", as_this_account());
        }
        let _ = writeln!(out, "    restart: unless-stopped");
        let _ = writeln!(out, "    depends_on: [engine]");
        // the supervisor listens on the host, which a container reaches by this name
        let _ = writeln!(out, "    extra_hosts:");
        let _ = writeln!(out, "      - \"host.docker.internal:host-gateway\"");
        let _ = writeln!(out, "    command: serve --config {IN_DESK}/nils-desk.toml");
        let _ = writeln!(out, "    ports:");
        let _ = writeln!(out, "      - \"{publish}\"");
        let _ = writeln!(out, "    volumes:");
        let _ = writeln!(out, "      - {}:{IN_DESK}", plan.desk_dir().display());
    }
    if plan.has(Part::Assistant) {
        let (kvasir, assistant) = (plan.dir.join("kvasir"), plan.dir.join("assistant"));
        let (k, a) = (kvasir.display(), assistant.display());
        let user = as_this_account();
        let _ = writeln!(out, "  kvasir:");
        let _ = writeln!(out, "    image: {NODE_IMAGE}");
        let _ = writeln!(out, "    container_name: nils-kvasir");
        if !user.is_empty() {
            let _ = writeln!(out, "    user: \"{user}\"");
        }
        let _ = writeln!(out, "    restart: unless-stopped");
        let _ = writeln!(out, "    working_dir: {k}");
        let _ = writeln!(out, "    command: node dist/main.js --config kvasir.json");
        let _ = writeln!(out, "    ports:");
        let _ = writeln!(out, "      - \"127.0.0.1:{p}:{p}\"", p = plan.ports.kvasir);
        let _ = writeln!(out, "    extra_hosts:");
        let _ = writeln!(out, "      - \"host.docker.internal:host-gateway\"");
        let _ = writeln!(out, "    volumes:");
        let _ = writeln!(out, "      - {k}:{k}");
        let _ = writeln!(out, "  assistant:");
        let _ = writeln!(out, "    image: {NODE_IMAGE}");
        let _ = writeln!(out, "    container_name: nils-assistant");
        if !user.is_empty() {
            let _ = writeln!(out, "    user: \"{user}\"");
        }
        let _ = writeln!(out, "    restart: unless-stopped");
        let _ = writeln!(out, "    depends_on: [engine, kvasir]");
        let _ = writeln!(out, "    working_dir: {a}");
        let _ = writeln!(out, "    command: node {}", assistant_entry(&assistant));
        let _ = writeln!(out, "    env_file: {a}/assistant.env");
        let _ = writeln!(out, "    volumes:");
        let _ = writeln!(out, "      - {a}:{a}");
        let _ = writeln!(out, "      - {k}:{k}:ro");
    }
    out
}

/// Podman's quadlets: systemd builds the units from these.
pub(crate) fn quadlets(plan: &Plan) -> Vec<(String, String)> {
    let publish = desk_publish(plan);
    let mut pod = format!("[Pod]\nPodName=nils\nPublishPort={publish}\n");
    if plan.has(Part::Assistant) {
        let _ = writeln!(pod, "PublishPort=127.0.0.1:{p}:{p}", p = plan.ports.kvasir);
    }
    if plan.host_loopback {
        let _ = writeln!(
            pod,
            "Network=pasta:--map-host-loopback={HOST_LOOPBACK_IN_POD}"
        );
    }
    let _ = write!(pod, "\n[Install]\nWantedBy=default.target\n");
    let mut out = vec![("nils.pod".to_string(), pod)];
    let mut engine = String::from("[Unit]\nDescription=NILS engine\n");
    if plan.postgres.map(|p| p.runtime) == Some(Runtime::Podman) {
        engine.push_str("After=nils-postgres.service\nWants=nils-postgres.service\n");
    }
    engine.push_str("\n[Container]\n");
    let _ = writeln!(engine, "Image={ENGINE_IMAGE}:{}", plan.tag());
    let _ = writeln!(engine, "Pod=nils.pod");
    let (registry, backups) = (
        plan.registry().display().to_string(),
        plan.dir.join("backups").display().to_string(),
    );
    let _ = writeln!(engine, "Volume={registry}:{registry}:U");
    let _ = writeln!(engine, "Volume={backups}:{backups}:U");
    for (_, path) in plan.read_from() {
        let _ = writeln!(engine, "Volume={0}:{0}:ro", path.display());
    }
    let _ = writeln!(
        engine,
        "Exec={}",
        engine_args(plan, &registry, &backups).join(" ")
    );
    let _ = write!(engine, "\n[Install]\nWantedBy=default.target\n");
    out.push(("nils-engine.container".to_string(), engine));
    if plan.has(Part::Desk) {
        let mut desk = String::from("[Unit]\nDescription=NILS desk\n\n[Container]\n");
        let _ = writeln!(desk, "Image={DESK_IMAGE}:{}", plan.tag());
        let _ = writeln!(desk, "Pod=nils.pod");
        let _ = writeln!(desk, "Volume={}:{IN_DESK}:U", plan.desk_dir().display());
        let _ = writeln!(desk, "Exec=serve --config {IN_DESK}/nils-desk.toml");
        let _ = write!(desk, "\n[Install]\nWantedBy=default.target\n");
        out.push(("nils-desk.container".to_string(), desk));
    }
    if plan.has(Part::Assistant) {
        let (kvasir, assistant) = (plan.dir.join("kvasir"), plan.dir.join("assistant"));
        let (k, a) = (kvasir.display(), assistant.display());
        out.push((
            "nils-kvasir.container".to_string(),
            format!(
                "[Unit]\nDescription=Kvasir, the model gateway\n\n[Container]\n\
                 Image={NODE_IMAGE}\nPod=nils.pod\nVolume={k}:{k}\nWorkingDir={k}\n\
                 Exec=node dist/main.js --config kvasir.json\n\n[Install]\nWantedBy=default.target\n"
            ),
        ));
        out.push((
            "nils-assistant.container".to_string(),
            format!(
                "[Unit]\nDescription=NILS assistant\nAfter=nils-kvasir.service nils-engine.service\n\
                 ConditionPathExists={k}/assistant.key\n\n\
                 [Container]\nImage={NODE_IMAGE}\nPod=nils.pod\nEnvironmentFile={a}/assistant.env\n\
                 Volume={a}:{a}\nVolume={k}:{k}:ro\nWorkingDir={a}\nExec=node {}\n\n\
                 [Install]\nWantedBy=default.target\n",
                assistant_entry(&assistant)
            ),
        ));
    }
    out
}

/// A Containerfile from a binary already downloaded, for a machine that
/// cannot pull the published image.
pub(crate) fn containerfile(part: &str) -> String {
    // the engine backs a Postgres registry up with pg_dump
    let pg_dump = if part == "nils" {
        "RUN apt-get update && apt-get install -y --no-install-recommends postgresql-client \\\n\
         \x20&& rm -rf /var/lib/apt/lists/*\n"
    } else {
        ""
    };
    format!(
        "# Written by nils setup, from the release binary beside it.\n\
         FROM docker.io/library/debian:trixie-slim\n\
         {pg_dump}RUN groupadd -g 1500 nils && useradd -u 1500 -g 1500 -M -d /srv/nils nils \\\n\
         \x20&& mkdir -p /srv/nils && chown -R nils:nils /srv/nils\n\
         COPY {part} /usr/local/bin/{part}\n\
         USER nils\nWORKDIR /srv/nils\nENTRYPOINT [\"{part}\"]\n"
    )
}

// ------------------------------------------------------------- the wizard

pub(crate) fn setup(args: SetupArgs) -> Result<(), Exit> {
    if cfg!(windows) {
        return Err(fail(
            "nils setup does not run on Windows yet; the documentation at \
             https://kineuro.se/nils/docs/ has the steps by hand",
        ));
    }
    let mut console = Console::new(args.yes || args.print);
    let mut existing = read_state();
    // An install that stopped partway is not one to update or repair: it is
    // started again, and its record is what an uninstall would remove.
    let restarted = existing.as_ref().is_some_and(|s| s.unfinished);
    if restarted {
        existing = None;
    }
    let facts = Facts::probe();

    let flow = if console.can_draw_screens() {
        let (flow, answered) = console
            .screens(|console| questions(console, &args, existing.as_ref(), &facts, restarted))?;
        if matches!(flow, Flow::Install(..)) && !answered.is_empty() {
            let p = console.palette;
            println!();
            println!(" {} {}", p.good("✓"), answered.join(&p.dim(" · ")));
        }
        flow
    } else {
        println!(
            "{}",
            console.bold("NILS setup: the engine, the desk and the assistant")
        );
        questions(&mut console, &args, existing.as_ref(), &facts, restarted)
            .map_err(Stop::into_exit)?
    };

    match flow {
        Flow::Install(install) => {
            let (plan, answers) = *install;
            let services = do_it(&plan, &mut console, &args, existing, false, &answers)?;
            summary(&plan, &console, &services);
            Ok(())
        }
        Flow::Update(state) => update_parts(&state, &args, &mut console),
        Flow::Repair(state) => repair(&state, &args, &mut console),
        Flow::Remove => uninstall(UninstallArgs {
            keep_data: false,
            purge: false,
            yes: false,
            print: args.print,
        }),
        Flow::Printed => Ok(()),
        Flow::Unready(missing) => Err(fail(format!(
            "nothing was changed: this machine lacks what the plan needs: {}",
            missing.join("; ")
        ))),
        Flow::Declined => {
            println!("nothing was changed");
            Ok(())
        }
    }
}

/// What a person wants, asked a step at a time: a plan and the answers to
/// install it with, or another thing to do with the setup that is there.
/// Asked on lines, or on screens, which run this again from the start after
/// every answer; so it changes nothing but what the console shows, and what
/// it asks of the world it asks through the console's probes.
fn questions(
    console: &mut Console,
    args: &SetupArgs,
    existing: Option<&State>,
    facts: &Facts,
    restarted: bool,
) -> Result<Flow, Stop> {
    if restarted {
        console.note(
            "an earlier setup stopped before it finished, so this one starts again; nils \
             uninstall removes what it placed instead",
        );
    }

    // An install that is already there: say what it is, and offer the four
    // things a person comes back for.
    if let Some(state) = existing {
        console.say(&what_is_there(state));
        let choice = if args.update {
            0
        } else if console.interactive() {
            console.ask_choice(
                "This machine already has a setup. What now?",
                &[
                    (
                        "Update everything",
                        "the newest of every part, nothing else asked",
                    ),
                    ("Change something", "the mode, the ports, what is installed"),
                    ("Add a part", "the desk or the assistant"),
                    ("Repair", "write the configuration and the services again"),
                    ("Remove", "NILS alone and keep your data, or everything"),
                ],
                0,
            )?
        } else {
            1
        };
        match choice {
            0 => return Ok(Flow::Update(state.clone())),
            3 => return Ok(Flow::Repair(state.clone())),
            4 => return Ok(Flow::Remove),
            _ => {}
        }
    } else if args.update {
        return Err(usage(format!(
            "--update needs a setup to update; none is recorded at {}",
            state_path().display()
        ))
        .into());
    }

    if !console.interactive() && !args.print {
        console.note("no terminal here, so every default is taken");
    }

    // 1. what to install
    console.step(1);
    let mut parts = match &args.parts {
        Some(list) => parts_of(list).map_err(usage)?,
        None => {
            let choices = [
                ("The engine only", "the registry and its command line"),
                (
                    "The engine and the desk",
                    "and a web application over it, for other people",
                ),
                (
                    "Everything",
                    "the assistant as well, which needs a model to talk to",
                ),
            ];
            let default = existing
                .map(
                    |s| match (s.parts.contains_key("assistant"), s.parts.len()) {
                        (true, _) => 2,
                        (false, n) if n > 1 => 1,
                        _ => 0,
                    },
                )
                .unwrap_or(1);
            match console.ask_choice("Which parts?", &choices, default)? {
                0 => vec![Part::Engine],
                1 => vec![Part::Engine, Part::Desk],
                _ => vec![Part::Engine, Part::Desk, Part::Assistant],
            }
        }
    };
    let named = parts
        .iter()
        .map(|p| p.name())
        .collect::<Vec<_>>()
        .join(", ");
    console.note(&named);
    console.said(if parts.contains(&Part::Assistant) {
        "everything"
    } else {
        &named
    });

    // 2. where it runs
    console.step(2);
    let podman = facts.podman;
    let docker = facts.docker;
    let runtime = match &args.runtime {
        Some(r) => Runtime::parse(r).map_err(usage)?,
        None => {
            if let Some(why) = docker_absent_reason(docker) {
                console.note(&why);
            }
            let choices = runtime_choices(podman, docker.is_ok());
            if choices.len() == 1 {
                console.note(
                    "no podman and no docker here, so the parts run on the machine; \
                     install either if you would rather have containers",
                );
                Runtime::Machine
            } else {
                let shown: Vec<(&str, &str)> = choices
                    .iter()
                    .map(|(_, label, said)| (*label, *said))
                    .collect();
                let default = existing
                    .and_then(|s| choices.iter().position(|(r, _, _)| r.name() == s.runtime))
                    .unwrap_or(0);
                choices[console.ask_choice("How should the parts run?", &shown, default)?].0
            }
        }
    };
    let engine_here = match runtime {
        Runtime::Machine => true,
        Runtime::Podman => podman,
        Runtime::Docker => docker.is_ok(),
    };
    if !engine_here {
        if !args.print {
            return Err(usage(format!(
                "{} is not on this machine; install it, or run with --runtime machine",
                runtime.name()
            ))
            .into());
        }
        console.note(&format!(
            "{} is not on this machine, so this is what it would run",
            runtime.name()
        ));
    }
    console.note(match runtime {
        Runtime::Machine => "on this machine",
        Runtime::Podman => "in podman containers",
        Runtime::Docker => "in docker containers",
    });
    console.said(match runtime {
        Runtime::Machine => "this machine",
        Runtime::Podman => "podman",
        Runtime::Docker => "docker",
    });

    // 3. where it lives
    console.step(3);
    let dir = match &args.dir {
        Some(d) => d.clone(),
        None => {
            let default = existing
                .map(|s| s.dir.clone())
                .unwrap_or_else(|| default_dir().display().to_string());
            expand(&console.ask_line("One directory for everything", &default)?)
        }
    };
    console.note(&format!(
        "registry, desk, backups, working, export{} under {}",
        if parts.contains(&Part::Assistant) {
            ", assistant, kvasir"
        } else {
            ""
        },
        dir.display()
    ));
    console.said(&tilde(&dir));
    let source = match &args.source {
        Some(s) => Some(s.clone()),
        None => {
            // a rerun offers the directory read now, so Enter keeps it
            let reads = existing
                .and_then(|s| {
                    s.places
                        .iter()
                        .find(|p| p.name == "source" && p.role == "source")
                })
                .map(|p| p.path.clone())
                .unwrap_or_default();
            let answer =
                console.ask_line("A directory of DICOM to read (empty for none)", &reads)?;
            let answer = answer.trim().to_string();
            (!answer.is_empty()).then(|| expand(&answer))
        }
    };

    // 4. the registry and its backend
    console.step(4);
    let home = Home::new(dir.join("registry"));
    let registry_exists = home.exists();
    let managed_here = managed_runtime(runtime, podman, docker.is_ok());
    let mut postgres: Option<ManagedPostgres> = None;
    let mut backend = if registry_exists {
        console.note(&format!(
            "a registry is already at {}; it is left alone",
            home.dir().display()
        ));
        console.said("kept as it is");
        // Its own configuration says where it is kept. A Postgres an earlier
        // install set up here, whose data and password were kept, is run
        // again for it; left as SQLite, nothing started it and the engine
        // could not reach its registry.
        match registry_backend(&dir) {
            Some((dsn, schema)) => {
                if dir.join("postgres.env").exists() {
                    match managed_here {
                        Some(rt) => {
                            postgres = Some(ManagedPostgres { runtime: rt });
                            console.note(&format!(
                                "the Postgres set up here before is run again in {}, with its data",
                                rt.name()
                            ));
                        }
                        None => console.note(
                            "the Postgres set up here before needs podman or docker to run, and \
                             neither answers",
                        ),
                    }
                }
                BackendChoice::Postgres { dsn, schema }
            }
            None => BackendChoice::Sqlite,
        }
    } else {
        console.note(
            "pseudonyms are derived from a key: the same subject under the same key gets \
             the same code, so keep its passphrase where you keep passwords",
        );
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Store {
            Sqlite,
            Here,
            Yours,
        }
        let mut offered = vec![(
            Store::Sqlite,
            "SQLite".to_string(),
            "one file in the directory above; a laptop or one server".to_string(),
        )];
        if let Some(rt) = managed_here {
            offered.push((
                Store::Here,
                "Postgres, set up here".to_string(),
                format!(
                    "run in {} beside the others, its data in {}",
                    rt.name(),
                    dir.join("postgres").display()
                ),
            ));
        }
        offered.push((
            Store::Yours,
            "A Postgres you already run".to_string(),
            "a database server, given by its connection string".to_string(),
        ));
        let store = match (&args.backend, &args.dsn) {
            (Some(b), _) if b.trim() == "sqlite" => Store::Sqlite,
            (Some(b), Some(_)) if b.trim() == "postgres" => Store::Yours,
            (Some(b), None) if b.trim() == "postgres" => {
                if managed_here.is_none() {
                    return Err(usage(
                        "no podman or docker here to run Postgres in; give --dsn for a Postgres \
                         you run, or --backend sqlite",
                    )
                    .into());
                }
                Store::Here
            }
            (Some(other), _) => {
                return Err(usage(format!("{other} is not a backend: sqlite or postgres")).into());
            }
            (None, _) => {
                let shown: Vec<(&str, &str)> = offered
                    .iter()
                    .map(|(_, label, said)| (label.as_str(), said.as_str()))
                    .collect();
                offered
                    [console.ask_choice("Where should the registry itself be kept?", &shown, 0)?]
                .0
            }
        };
        match store {
            Store::Sqlite => {
                console.said("SQLite");
                BackendChoice::Sqlite
            }
            Store::Here => {
                let rt = managed_here.unwrap_or(Runtime::Podman);
                postgres = Some(ManagedPostgres { runtime: rt });
                console.note(&format!(
                    "Postgres is run in {}, its data in {}",
                    rt.name(),
                    dir.join("postgres").display()
                ));
                console.said("Postgres here");
                // its connection string is written once its port is settled
                BackendChoice::Postgres {
                    dsn: String::new(),
                    schema: "nils".to_string(),
                }
            }
            Store::Yours => {
                'postgres: {
                    let mut dsn = match &args.dsn {
                        Some(d) => d.clone(),
                        None => console
                            .ask_line("The connection string", "postgres://nils@127.0.0.1/nils")?,
                    };
                    let schema = match &args.schema {
                        Some(s) => s.clone(),
                        None => console.ask_line("The schema", "nils")?,
                    };
                    // Tried now, from this machine, so a database that is not there
                    // is said here rather than after the plan and the images.
                    loop {
                        let refused = console.probe(&format!("postgres {dsn} {schema}"), || {
                            nils_registry::store::Store::connect_postgres(&dsn, &schema)
                                .err()
                                .map(|e| with_causes(&e))
                        });
                        let Some(why) = refused else {
                            console.note(&format!(
                                "the database at {} answered",
                                crate::redact_dsn(&dsn)
                            ));
                            break;
                        };
                        console.note(&format!(
                            "the database at {} did not answer: {why}",
                            crate::redact_dsn(&dsn)
                        ));
                        if args.print {
                            break;
                        }
                        if !console.interactive() {
                            return Err(usage(
                                "start the database, or run with --backend sqlite; nothing \
                                 was written",
                            )
                            .into());
                        }
                        if !console.ask_yes_no("Enter another connection string?", true)? {
                            // Going on with an address that did not answer
                            // only fails later, after the plan and the work.
                            console.note("the registry is kept in SQLite instead");
                            console.said("SQLite");
                            break 'postgres BackendChoice::Sqlite;
                        }
                        let again = console.ask_line("The connection string", "")?;
                        if !again.trim().is_empty() {
                            dsn = again.trim().to_string();
                        }
                    }
                    if let Some(note) = postgres_reach_note(runtime, &dsn, podman_has_pasta) {
                        console.note(&note);
                    }
                    console.said("your Postgres");
                    BackendChoice::Postgres { dsn, schema }
                }
            }
        }
    };
    let mut answers = Answers::default();
    if !registry_exists && args.key_file.is_none() && console.interactive() {
        answers.passphrase = console.ask_secret("A passphrase for the registry's key")?;
    }

    // 5. who may sign in, and who may reach the desk
    console.step(5);
    let mut mode = match &args.mode {
        Some(m) => Mode::parse(m).map_err(usage)?,
        None => {
            let choices = [
                ("Nobody", "one person on this machine, no login"),
                (
                    "The desk keeps the people",
                    "usernames and passwords the desk holds, no other service",
                ),
                (
                    "An identity provider",
                    "single sign on: Authentik, Keycloak, Entra",
                ),
            ];
            let default = existing
                .and_then(|s| Mode::parse(&s.mode).ok())
                .map_or(0, |m| match m {
                    Mode::Off => 0,
                    Mode::Local => 1,
                    Mode::Oidc => 2,
                });
            match console.ask_choice("How do people reach it?", &choices, default)? {
                0 => Mode::Off,
                1 => Mode::Local,
                _ => Mode::Oidc,
            }
        }
    };

    console.note(mode.words());

    // An address given outright is the one a browser opens, whatever the
    // desk binds: a name of the site's, with a certificate, that a proxy
    // answers for. It is refused here, before anything is written, since an
    // address a browser cannot open leaves a desk nobody can sign in to.
    let given_origin = match &args.origin {
        Some(text) => Some(origin_given(text).map_err(usage)?),
        None => None,
    };
    let recorded_origin = existing
        .map(|s| s.origin.trim().to_string())
        .filter(|o| !o.is_empty());
    let recorded_network = existing.is_some_and(|s| !matches!(s.reach.as_str(), "" | "loopback"));
    let reach_word = match args.reach.as_deref().map(str::trim) {
        None => None,
        Some("network") => Some(true),
        Some("loopback") => Some(false),
        Some(other) => {
            return Err(usage(format!("{other} is not a reach: loopback or network")).into());
        }
    };
    let mut reach = match (&given_origin, reach_word) {
        // the desk binds for a proxy on another machine only where it is
        // told to; the origin says nothing about that
        (Some(origin), given) => Reach::Behind {
            origin: origin.clone(),
            network: given.unwrap_or(false),
        },
        (None, Some(true)) => {
            Reach::Network(host_address().unwrap_or_else(|| "127.0.0.1".to_string()))
        }
        (None, Some(false)) => Reach::Loopback,
        (None, None) if !parts.contains(&Part::Desk) => Reach::Loopback,
        (None, None) => {
            let address = host_address().unwrap_or_else(|| "this host".to_string());
            let choices = [
                ("Only this machine", "the desk answers on 127.0.0.1"),
                ("This network", "other machines here can open it"),
                (
                    "Behind a proxy of yours",
                    "a name and a certificate of yours answer for it",
                ),
            ];
            // a rerun opens on what is on record, so an install behind a
            // proxy stays behind it without being asked twice
            let default = if recorded_origin.is_some() {
                2
            } else {
                usize::from(recorded_network)
            };
            match console.ask_choice("Who may open the desk?", &choices, default)? {
                0 => Reach::Loopback,
                1 => Reach::Network(address),
                _ => {
                    let mut suggested = recorded_origin.clone().unwrap_or_default();
                    let origin = loop {
                        let answer =
                            console.ask_line("The address a browser opens it at", &suggested)?;
                        match origin_given(&answer) {
                            Ok(origin) => break origin,
                            Err(refused) if console.interactive() => {
                                console.note(&refused);
                                suggested = String::new();
                            }
                            Err(refused) => return Err(usage(refused).into()),
                        }
                    };
                    let here = console
                        .ask_yes_no("Does that proxy run on this machine?", !recorded_network)?;
                    Reach::Behind {
                        origin,
                        network: !here,
                    }
                }
            }
        }
    };
    if reach.proxied().is_some() && !parts.contains(&Part::Desk) {
        console.note("the desk is not installed here, so nothing answers at that address yet");
    }
    if (reach.beyond_loopback() || reach.proxied().is_some()) && mode == Mode::Off {
        console.note(match reach.proxied() {
            Some(_) => {
                "off mode has no login, so anyone who opens that address gets the whole registry"
            }
            None => {
                "off mode has no login, so anyone on that network who finds the port gets the \
                 whole registry"
            }
        });
        if console.ask_yes_no("Keep the people in the desk instead (local mode)?", true)? {
            mode = Mode::Local;
        } else if !console.interactive() && reach.proxied().is_none() {
            reach = Reach::Loopback;
        }
    }
    console.said(match mode {
        Mode::Off => "no login",
        Mode::Local => "desk accounts",
        Mode::Oidc => "a provider",
    });

    // the first person, for a desk that keeps its own and has none yet; a
    // desk that ran with nobody signing in has its store already, and no one
    // in it
    if mode == Mode::Local
        && parts.contains(&Part::Desk)
        && !desk_has_people(&dir)
        && console.interactive()
        && console.ask_yes_no("Add the first person now, who may do everything?", true)?
    {
        // the desk's rules, held where they are asked: an answer it would refuse is asked again
        let name = loop {
            let name = console
                .ask_line("A username for them", "admin")?
                .trim()
                .to_string();
            match desk_username_refusal(&name) {
                None => break name,
                Some(refused) => console.note(refused),
            }
        };
        if let Some(password) = console.ask_password(&format!("A password for {name}"))? {
            answers.first = Some((name, password));
        }
    }

    // the provider, for a desk that signs people in through one
    let oidc = if mode == Mode::Oidc {
        choose_provider(
            console,
            args,
            existing.and_then(|s| s.oidc.clone()),
            &mut answers,
        )?
    } else {
        None
    };

    // 6. the assistant, and what this machine can do
    console.step(6);
    let card = facts.card.clone();
    match cards_words(&facts.cards) {
        Some(words) => console.note(&words),
        None => console.note("no graphics card the probe could find"),
    }
    for line in card_advice(card.as_ref().map(|c| c.memory_gb)) {
        console.say(&line);
    }
    let served = card.as_ref().map(|c| c.memory_gb).unwrap_or(0.0) >= 12.0;
    // An assistant already installed is kept unless a person says otherwise:
    // a change that took every default dropped it on a machine with no card.
    let had_assistant = existing.is_some_and(|s| s.parts.contains_key("assistant"));
    if args.parts.is_none() {
        if parts.contains(&Part::Assistant) {
            let question = if had_assistant {
                "Keep the assistant?"
            } else {
                "Install the assistant anyway?"
            };
            if !console.ask_yes_no(question, served || had_assistant)? {
                parts.retain(|p| *p != Part::Assistant);
            }
        } else if console.ask_yes_no("Add the assistant as well?", false)? {
            parts.push(Part::Assistant);
        }
    }
    if parts.contains(&Part::Assistant) && runtime.container() {
        console.note(&format!(
            "the assistant and Kvasir, the model gateway, are built from source on this machine, \
             which needs Node 22, and run in {NODE_IMAGE} beside the others"
        ));
    }
    // The model the assistant asks for is kept unless another is chosen, and
    // a rerun is how the model is changed; with none named yet, it is asked.
    let on_record = if parts.contains(&Part::Assistant) {
        model_on_record_in(&dir)
    } else {
        None
    };
    let choose = parts.contains(&Part::Assistant)
        && match &on_record {
            Some(model) => {
                let talks_to = if model == CHATGPT_WORDS {
                    "a ChatGPT subscription"
                } else {
                    model.as_str()
                };
                !console.ask_yes_no(
                    &format!("The assistant talks to {talks_to}. Keep it?"),
                    true,
                )?
            }
            None => true,
        };
    if choose {
        let chosen = choose_model(console, served, mode)?;
        if let Some(note) = model_reach_note(runtime, &chosen.url, podman_has_pasta) {
            console.note(&note);
        }
        console.said(if chosen.later {
            "a model later"
        } else if chosen.chatgpt {
            "ChatGPT"
        } else {
            &chosen.model
        });
        answers.model = Some(chosen);
    } else {
        // The install's ChatGPT subscription, kept, is chosen again: signed
        // in once Kvasir runs where it is not, which is how a sign-in that did
        // not finish is taken up by running setup again.
        if on_record.as_deref() == Some(CHATGPT_WORDS) && mode == Mode::Off {
            answers.model = Some(ModelChoice::chatgpt());
        }
        let said = match &on_record {
            Some(model) => model.clone(),
            None if parts.contains(&Part::Assistant) => "the assistant".to_string(),
            None => "no assistant".to_string(),
        };
        console.said(&said);
    }

    // ports, once the parts are settled: each part this machine will listen
    // for, and none this setup already holds, since its own running service
    // is what holds it
    let mut ports = existing.map(|s| s.ports).unwrap_or_default();
    let ours = |part: &str| existing.is_some_and(|s| s.parts.contains_key(part));
    let assistant = parts.contains(&Part::Assistant);
    // llama.cpp runs beside Kvasir wherever there is a build for this machine;
    // podman and docker on macOS run in a machine of their own, where it does not
    let llama = facts
        .llama
        .filter(|_| assistant && !(runtime.container() && cfg!(target_os = "macos")));
    let mut chosen: Vec<u16> = Vec::new();
    // a supervisor this setup started holds its own port on a rerun
    let supervised = dir.join("supervise").join("supervise.toml").exists();
    for (name, part, port, listens) in [
        ("the engine", "engine", &mut ports.engine, true),
        (
            "the desk",
            "desk",
            &mut ports.desk,
            parts.contains(&Part::Desk),
        ),
        ("Kvasir", "kvasir", &mut ports.kvasir, assistant),
        // in a container the assistant is published nowhere
        (
            "the assistant",
            "assistant",
            &mut ports.assistant,
            assistant && !runtime.container(),
        ),
        (
            "Postgres",
            "postgres",
            &mut ports.postgres,
            postgres.is_some(),
        ),
        ("the supervisor", "supervisor", &mut ports.supervisor, true),
        ("llama.cpp", LLAMA_PART, &mut ports.llama, llama.is_some()),
    ] {
        if listens
            && !ours(part)
            && !(part == "supervisor" && supervised)
            && let Some(free) = settle_port(*port, &chosen, &port_taken)
        {
            console.note(&format!("port {} is taken, so {name} takes {free}", *port));
            *port = free;
        }
        chosen.push(*port);
    }
    if postgres.is_some() && !registry_exists {
        // A password kept from an earlier install of this directory, since
        // its data was made with it; a new one otherwise. Made once for the
        // answers so far, however often the questions are run again.
        let password = console.probe("a password for Postgres", || {
            postgres_password(&dir.join("postgres.env")).unwrap_or_else(generated_passphrase)
        });
        backend = BackendChoice::Postgres {
            dsn: format!(
                "postgres://nils:{password}@127.0.0.1:{}/nils",
                ports.postgres
            ),
            schema: "nils".to_string(),
        };
    }

    // 7. services
    console.step(7);
    // The services of this machine are asked for outright and never arrived
    // at: they are written as root, into a directory of the machine's, and
    // they run the parts as accounts of their own. An install that has them
    // keeps them on a rerun, since the record says so.
    let accounts = accounts_given(&args.account).map_err(usage)?;
    let capabilities = match &args.capabilities {
        Some(text) => capabilities_given(text).map_err(usage)?,
        None => Vec::new(),
    };
    let system = match (args.system, existing.and_then(|s| s.system.clone())) {
        (false, None) => {
            if let Some(refused) = without_system(!accounts.is_empty(), !capabilities.is_empty()) {
                return Err(usage(refused).into());
            }
            None
        }
        (_, recorded) => {
            let mut system = recorded.unwrap_or_default();
            if !accounts.is_empty() {
                system.accounts = accounts;
            }
            if !capabilities.is_empty() {
                system.capabilities = capabilities;
            }
            Some(system)
        }
    };
    // What this machine cannot be given is said here, with nothing written
    // and the fix in the same sentence. `--print` changes nothing anyway, so
    // it says it and goes on to show what such an install would be.
    if let Some(system) = &system
        && let Some(refused) = system_refusal(runtime, system, &parts)
    {
        if !args.print {
            return Err(usage(refused).into());
        }
        console.note(&refused);
    }
    let wants_services = args.service || system.is_some();
    let manager = service_manager(runtime, system.is_some());
    let service = match (args.no_service, wants_services, manager) {
        (true, _, _) => false,
        // Asked for outright where nothing here can write or start them:
        // said now, with nothing changed, rather than after every file is
        // written and every part installed. `--print` changes nothing
        // anyway, and goes on to show what would be written.
        (_, true, None) if !args.print => {
            return Err(usage(no_manager_here(runtime, system.is_some())).into());
        }
        (_, true, _) => true,
        (_, _, None) => {
            console.note(&no_manager_here(runtime, system.is_some()));
            false
        }
        (_, _, Some(manager)) => {
            let before = existing.map(|s| !s.service.is_empty() && s.service != "none");
            console.ask_yes_no(
                &format!("Write and start {manager} so it comes back after a restart?"),
                before.unwrap_or(true),
            )?
        }
    };
    console.said(match (service, runtime) {
        (false, _) => "by hand",
        (true, _) if system.is_some() => "the machine's systemd",
        (true, Runtime::Machine) if cfg!(target_os = "macos") => "launchd",
        (true, Runtime::Machine) => "systemd",
        (true, Runtime::Podman) => "quadlets",
        (true, Runtime::Docker) => "compose",
    });

    let postgres_here = matches!(&backend, BackendChoice::Postgres { dsn, .. }
        if dsn_for(Runtime::Podman, dsn) != *dsn);
    // the desk in the pod reaches the supervisor on this host's loopback
    let host_loopback = runtime == Runtime::Podman
        && (parts.contains(&Part::Assistant) || parts.contains(&Part::Desk) || postgres_here)
        && podman_has_pasta();
    let sources = registry_sources(&dir).unwrap_or_else(|| {
        existing
            .map(|s| recorded_sources(&s.places))
            .unwrap_or_default()
    });
    let plan = Plan {
        dir,
        parts,
        mode,
        runtime,
        backend,
        ports,
        reach,
        source,
        sources,
        registry_exists,
        host_loopback,
        postgres,
        service,
        channel: args.channel.clone(),
        version: update::VERSION.to_string(),
        oidc,
        llama,
        system,
    };

    // 8. the summary, then the work
    console.step(8);
    for (key, value) in plan_rows(&plan) {
        console.row(key, &value);
    }
    if let Some((name, _)) = &answers.first {
        console.row("first person", &format!("{name}, who may do everything"));
    }
    if let Some(model) = &answers.model {
        let said = if model.later {
            "none named yet".to_string()
        } else if model.chatgpt {
            "your ChatGPT subscription, signed in once Kvasir runs (the prompt leaves your systems)"
                .to_string()
        } else {
            format!(
                "{} at {}{}",
                if model.model.is_empty() {
                    "the default model"
                } else {
                    &model.model
                },
                model.url,
                if model.local {
                    ""
                } else {
                    " (the prompt leaves your systems)"
                }
            )
        };
        console.row("model", &said);
    }
    // what this machine lacks for the plan, found before anything is placed
    let missing = missing_for(&plan);
    for need in &missing {
        console.row("missing", need);
    }
    if args.print {
        print!("{}", commands_text(&plan, console));
        println!();
        println!("nothing was changed");
        return Ok(Flow::Printed);
    }
    if !missing.is_empty() {
        return Ok(Flow::Unready(missing));
    }
    if console.interactive() && !console.ask_yes_no("Do it?", true)? {
        return Ok(Flow::Declined);
    }
    Ok(Flow::Install(Box::new((plan, answers))))
}

/// What is installed, where, in what mode and how it runs, on one line.
fn what_is_there(state: &State) -> String {
    let parts = state
        .parts
        .iter()
        .map(|(name, p)| format!("{name} {}", p.version))
        .collect::<Vec<_>>()
        .join(", ");
    let runtime = match state.runtime.as_str() {
        "podman" => "in podman containers",
        "docker" => "in docker containers",
        _ => "on the machine",
    };
    let service = if state.service.is_empty() || state.service == "none" {
        "started by hand".to_string()
    } else {
        format!("kept running by {}", state.service)
    };
    format!(
        "{parts} in {}, {} mode, {runtime}, {service}",
        state.dir, state.mode
    )
}

/// `--update`, and the first thing the menu offers: the parts the state
/// names, brought to the newest release, without a question.
fn update_parts(state: &State, args: &SetupArgs, console: &mut Console) -> Result<(), Exit> {
    let mut plan = plan_from_state(state, args.channel.as_deref());
    if let Some(refused) = recorded_system_refusal(&plan) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    if let Ok(newest) = update::newest_version(&update::engine_base(args.channel.as_deref())) {
        plan.version = newest;
    }
    println!();
    println!("{}", console.bold("Updating"));
    print!("{}", plan_text(&plan, console));
    if args.print {
        print!("{}", commands_text(&plan, console));
        println!();
        println!("nothing was changed");
        return Ok(());
    }
    let services = do_it(
        &plan,
        console,
        args,
        Some(state.clone()),
        true,
        &Answers::default(),
    )?;
    // A container's engine is the image, and `do_it` pulled the new tag. A
    // machine's engine is this binary, and nothing above touches it, so
    // "update everything" would have moved every part except the one a
    // person is most likely to have meant.
    if !plan.runtime.container() {
        update_engine_binary(args.channel.as_deref(), console);
    }
    summary(&plan, console, &services);
    Ok(())
}

/// The engine binary itself, moved to the newest release. It goes last,
/// because it replaces the binary doing the replacing; on unix that is a
/// rename over an open file, which the running process does not notice.
fn update_engine_binary(channel: Option<&str>, console: &mut Console) {
    let base = update::engine_base(channel);
    let Ok(wanted) = update::newest_version(&base) else {
        println!("  the newest engine release could not be read; this binary is left alone");
        return;
    };
    if !update::newer(&wanted, update::VERSION) {
        console.note(&format!("engine {} is the newest release", update::VERSION));
        return;
    }
    let Ok(me) = std::env::current_exe() else {
        println!("  this binary cannot say where it is, so it is left alone");
        return;
    };
    let me = std::fs::canonicalize(&me).unwrap_or(me);
    let file = update::file_of(&update::host_target());
    match update::fetch_checked(&base, &wanted, &file)
        .and_then(|bytes| update::install_binary(&me, &bytes))
    {
        Ok(()) => console.note(&format!(
            "engine {wanted} at {} (was {})",
            me.display(),
            update::VERSION
        )),
        Err(e) => println!("  the engine binary was left alone: {}", e.message),
    }
}

/// The menu's last offer: the configuration and the units written again from
/// what the state records, for an install whose files were lost or edited.
fn repair(state: &State, args: &SetupArgs, console: &mut Console) -> Result<(), Exit> {
    let plan = plan_from_state(state, args.channel.as_deref());
    if let Some(refused) = recorded_system_refusal(&plan) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    println!();
    println!("{}", console.bold("Repairing"));
    print!("{}", plan_text(&plan, console));
    if args.print {
        print!("{}", commands_text(&plan, console));
        println!();
        println!("nothing was changed");
        return Ok(());
    }
    for sub in ["registry", "desk", "backups", "working", "export"] {
        let path = plan.dir.join(sub);
        std::fs::create_dir_all(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }
    let mut stages = Vec::new();
    if plan.postgres.is_some() {
        stages.push((Stage::Postgres, format!("Postgres {POSTGRES_MAJOR}")));
    }
    if plan.has(Part::Desk) {
        stages.push((Stage::Desk, "desk configuration".to_string()));
    }
    if plan.has(Part::Assistant) {
        if plan.llama.is_some() {
            stages.push((Stage::Runtime, LLAMA_PART.to_string()));
        }
        stages.push((Stage::Kvasir, "Kvasir configuration".to_string()));
    }
    stages.push((Stage::Services, "services".to_string()));
    console.start_checklist("Repairing", stages);
    let mended = mend(&plan, state, console);
    console.finish_checklist(
        mended.is_ok(),
        if mended.is_ok() {
            "Repaired"
        } else {
            "Stopped"
        },
    );
    let services = mended?;
    summary(&plan, console, &services);
    Ok(())
}

/// A repair's work: the Postgres this setup runs started again, the desk's
/// configuration written and Kvasir's mended, and the services written and
/// started.
fn mend(plan: &Plan, state: &State, console: &mut Console) -> Result<Vec<Service>, Exit> {
    // what a repair places is recorded, so the services and an uninstall know it
    let mut state = state.clone();
    // A Postgres this setup runs, started again with its data and password.
    if let Some(pg) = plan.postgres {
        console.begin(Stage::Postgres);
        if let Err(e) = start_postgres(plan, pg, console) {
            console.warn(&format!("Postgres was not started: {}", e.message));
        }
    }
    if plan.has(Part::Desk) {
        console.begin(Stage::Desk);
        if let Err(e) = write_supervisor(plan) {
            console.warn(&format!("the supervisor was not set up: {}", e.message));
        }
        write_desk_config(plan)?;
        console.progress(&plan.desk_config().display().to_string());
    }
    // Kvasir's file is mended, not rewritten, and the assistant's environment
    // is written where it is missing; the models are added and the key is
    // made during the start-up below, once Kvasir answers.
    if plan.has(Part::Assistant) {
        if plan.llama.is_some() {
            console.begin(Stage::Runtime);
            let before = state.parts.get(LLAMA_PART).map(|p| p.path.clone());
            if install_llama(plan, &mut state, console).is_ok()
                && state.parts.get(LLAMA_PART).map(|p| p.path.clone()) != before
            {
                let _ = write_state(&state);
            }
        }
        console.begin(Stage::Kvasir);
        if let Err(e) = configure_kvasir(plan, console, None) {
            console.warn(&format!(
                "Kvasir's configuration was not mended: {}",
                e.message
            ));
        }
        if let Err(e) = write_assistant_env(plan, None) {
            console.warn(&format!(
                "the assistant's environment was not written: {}",
                e.message
            ));
        }
    }
    hand_over_files(plan, console);
    console.begin(Stage::Services);
    let mut services = Vec::new();
    if plan.service {
        match start_everything(plan, &state, console, None) {
            Ok(started) => {
                console.report(&started);
                services = started.services;
            }
            Err(e) => console.warn(&format!("the services were not started: {}", e.message)),
        }
    } else {
        for line in container_commands(plan) {
            console.say(&format!("run: {line}"));
        }
    }
    // systemd, podman and docker make Kvasir ready as they start, between
    // Kvasir and the assistant
    let systemd = plan.runtime == Runtime::Machine && !cfg!(target_os = "macos");
    if plan.has(Part::Assistant) && !(plan.service && (systemd || plan.runtime.container())) {
        ready_kvasir(plan, console, None)?;
    }
    start_supervisor(plan, &state, console);
    Ok(services)
}

/// Whether this account has a systemd user manager here that would take
/// units. `systemctl --user --version` answers from the binary wherever it
/// is installed, session or none, so the manager itself is asked: reading a
/// property of it needs the bus, which is there only where a session is.
fn user_session() -> bool {
    cfg!(target_os = "linux")
        && run_quiet("systemctl", &["--user", "show", "--property=Version"]).is_some()
}

/// What the plan, the record and the wizard call the services of this
/// machine.
const SYSTEM_MANAGER: &str = "systemd system units";

/// Which service manager this machine and runtime use, if any. The services
/// of this machine are systemd's own and ask nothing of any account; the
/// units of a machine or a podman install are this account's own, so a user
/// manager must be there to take them; docker's own daemon brings its
/// containers back and asks nothing of this account either.
fn service_manager(runtime: Runtime, system: bool) -> Option<&'static str> {
    if system {
        return systemd_here().then_some(SYSTEM_MANAGER);
    }
    service_manager_when(runtime, user_session())
}

/// The same, for a machine with or without a session of this account: the
/// probe is asked once and the answer read here, so that both answers can be
/// had on any machine.
fn service_manager_when(runtime: Runtime, session: bool) -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        return (runtime == Runtime::Machine).then_some("launchd agents");
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    match runtime {
        Runtime::Docker => Some("a compose file"),
        Runtime::Machine => session.then_some("systemd user units"),
        Runtime::Podman => session.then_some("podman quadlets"),
    }
}

/// Why nothing here can keep the parts running, said so that a person can
/// act on it: on Linux the units want a session of the account that runs
/// NILS, which is what a machine nobody is logged in to has not got.
fn no_manager_here(runtime: Runtime, system: bool) -> String {
    if system {
        return "this machine has no systemd to take a service of its own: leave --system off, \
                and the parts are kept the way this account keeps its own"
            .to_string();
    }
    no_manager_words(runtime, user_session())
}

fn no_manager_words(runtime: Runtime, session: bool) -> String {
    if !session && cfg!(target_os = "linux") {
        return format!(
            "this account has no systemd session on this machine, so nothing here can keep {} \
             running: sign in as the account that runs NILS, or allow it to keep services \
             without a login (loginctl enable-linger {}), and run nils setup again",
            if runtime.container() {
                "the containers"
            } else {
                "the parts"
            },
            whoami().unwrap_or_else(|| "<account>".to_string())
        );
    }
    "no service manager here, so the commands are printed instead".to_string()
}

/// The plan as a person reads it before anything happens: a row for each
/// thing it decides.
fn plan_rows(plan: &Plan) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("directory", plan.dir.display().to_string()),
        (
            "parts",
            plan.parts
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        (
            "runs",
            match plan.runtime {
                Runtime::Machine => "on this machine".to_string(),
                Runtime::Podman => format!("in podman containers, images {ENGINE_IMAGE}"),
                Runtime::Docker => format!("in docker containers, images {ENGINE_IMAGE}"),
            },
        ),
    ];
    if plan.runtime.container() && plan.has(Part::Assistant) {
        rows.push((
            "kvasir",
            format!(
                "and the assistant built here and run in {NODE_IMAGE}, their directories mounted"
            ),
        ));
    }
    if plan.has(Part::Assistant) {
        rows.push((LLAMA_PART, llama_row(plan)));
    }
    rows.push((
        "registry",
        format!(
            "{} {}",
            plan.registry().display(),
            if plan.registry_exists {
                "(already there)"
            } else {
                "(will be made)"
            }
        ),
    ));
    rows.push((
        "backend",
        match &plan.backend {
            BackendChoice::Sqlite => "sqlite".to_string(),
            BackendChoice::Postgres { schema, .. } => format!("postgres, schema {schema}"),
        },
    ));
    if let Some(pg) = plan.postgres {
        rows.push((
            "postgres",
            format!(
                "set up here in {}, on 127.0.0.1:{}, its data in {}",
                pg.runtime.name(),
                plan.ports.postgres,
                plan.postgres_dir().display()
            ),
        ));
    }
    rows.push((
        "sign in",
        format!("{} ({})", plan.mode.words(), plan.mode.name()),
    ));
    if plan.has(Part::Desk) {
        let (bind, origin, _) =
            desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
        rows.push(("desk", format!("{origin} (binds {bind})")));
        rows.push((
            "desk config",
            format!(
                "{} ({})",
                plan.desk_config().display(),
                desk_config_fate(plan).words()
            ),
        ));
    }
    rows.push(("engine port", plan.ports.engine.to_string()));
    let reads: Vec<String> = plan
        .read_from()
        .iter()
        .map(|(_, path)| path.display().to_string())
        .collect();
    if !reads.is_empty() {
        rows.push(("reads", format!("{} (read only)", reads.join(", "))));
    }
    rows.push((
        "places",
        place_specs(plan)
            .iter()
            .map(|s| format!("{} as {}", s.name, s.role))
            .collect::<Vec<_>>()
            .join(", "),
    ));
    rows.push((
        "services",
        if plan.service {
            service_manager(plan.runtime, plan.system.is_some())
                .unwrap_or("none")
                .to_string()
        } else {
            "none; the commands are printed".to_string()
        },
    ));
    if let Some(system) = &plan.system {
        rows.push((
            "accounts",
            plan.parts
                .iter()
                .map(|part| format!("{} as {}", part.name(), system.account(part.name())))
                .collect::<Vec<_>>()
                .join(", "),
        ));
        if !system.capabilities.is_empty() {
            rows.push(("engine keeps", system.capabilities.join(", ")));
        }
    }
    rows.push(("state", state_path().display().to_string()));
    rows
}

/// llama.cpp's row of the plan: the build, what it runs a model on, where it
/// listens, and what a Linux machine without a Vulkan loader should know.
fn llama_row(plan: &Plan) -> String {
    let Some(llama) = plan.llama else {
        return "no build for this machine, so a model Kvasir downloads is started with a model \
                server of your own"
            .to_string();
    };
    let mut row = format!(
        "{LLAMA_BUILD}, the {} build, runs the models Kvasir starts, on {}:{}",
        llama_words(llama.variant),
        match plan.runtime {
            Runtime::Docker => "docker's bridge",
            _ => "127.0.0.1",
        },
        plan.ports.llama
    );
    if !llama.loader {
        row.push_str(&format!("; {}", NO_VULKAN_LOADER));
    }
    row
}

/// What a Linux machine with a card and no Vulkan loader is told.
const NO_VULKAN_LOADER: &str = "this machine has no Vulkan loader, so a model runs on the \
                                processor until libvulkan1 (Debian, Ubuntu) or vulkan-loader \
                                (Fedora) is installed";

/// The plan's rows, as lines.
fn plan_text(plan: &Plan, console: &Console) -> String {
    let mut out = String::new();
    for (key, value) in plan_rows(plan) {
        let _ = writeln!(out, "  {} {value}", console.dim(&format!("{key:<12}")));
    }
    out
}

/// A line of a file, indented, without leaving a blank line full of spaces.
fn indent(line: &str) -> String {
    if line.is_empty() {
        String::new()
    } else {
        format!("    {line}")
    }
}

/// Under `--print`, the exact commands and files, so a person can read them
/// and a test can assert them.
fn commands_text(plan: &Plan, console: &Console) -> String {
    let mut out = String::new();
    match plan.runtime {
        Runtime::Machine if plan.service => {
            // Every unit the install would write, and every call it would
            // make: the services are the likeliest thing to go wrong, and
            // the services of a machine the likeliest of those.
            let state = planned_state(plan);
            let mac = cfg!(target_os = "macos");
            let mut units = if mac {
                launchd_plists(plan, &state)
            } else {
                systemd_units(plan, &state)
            };
            units.push(supervisor_service(plan, &state));
            let where_they_go = if mac {
                "~/Library/LaunchAgents".to_string()
            } else {
                units_dir(plan.system.is_some()).display().to_string()
            };
            let _ = writeln!(
                out,
                "\n{} {}",
                console.bold("  services"),
                console.dim(&where_they_go)
            );
            for (name, text) in &units {
                let _ = writeln!(out, "\n  {}", console.bold(name));
                for line in text.lines() {
                    let _ = writeln!(out, "{}", indent(line));
                }
            }
            if !mac {
                let names: Vec<String> = units
                    .iter()
                    .filter_map(|(name, _)| name.strip_suffix(".service"))
                    .map(str::to_string)
                    .collect();
                let _ = writeln!(out, "\n{}", console.bold("  then"));
                for line in unit_calls(plan, &names) {
                    let _ = writeln!(out, "    {line}");
                }
            }
        }
        Runtime::Machine => {
            let _ = writeln!(out, "\n{}", console.bold("  start it"));
            for line in start_commands(plan) {
                let _ = writeln!(out, "    {line}");
            }
        }
        Runtime::Podman => {
            let _ = writeln!(out, "\n{}", console.bold("  podman"));
            for line in podman_commands(plan) {
                let _ = writeln!(out, "    {line}");
            }
            if plan.service {
                for (name, text) in quadlets(plan) {
                    let _ = writeln!(out, "\n  {}", console.bold(&name));
                    for line in text.lines() {
                        let _ = writeln!(out, "{}", indent(line));
                    }
                }
            }
        }
        Runtime::Docker => {
            let _ = writeln!(out, "\n{}", console.bold("  docker"));
            for line in docker_commands(plan) {
                let _ = writeln!(out, "    {line}");
            }
            if plan.service {
                let _ = writeln!(out, "\n  {}", console.bold("compose.yaml"));
                for line in docker_compose(plan).lines() {
                    let _ = writeln!(out, "{}", indent(line));
                }
            }
        }
    }
    // llama.cpp runs on this machine whichever runtime the parts use
    if plan.has(Part::Assistant)
        && plan.service
        && let Some(llama) = plan.llama
    {
        let build = llama_build_dir(&plan.dir, llama.variant);
        let (name, text) = if cfg!(target_os = "macos") {
            (
                "se.kineuro.nils-llama.plist".to_string(),
                launchd_plist(
                    "se.kineuro.nils-llama",
                    &llama_argv(plan, &build, "127.0.0.1"),
                    &plan.runtime_dir().display().to_string(),
                ),
            )
        } else {
            (
                "nils-llama.service".to_string(),
                llama_unit(plan, &build, &llama_host(plan)),
            )
        };
        let _ = writeln!(out, "\n  {}", console.bold(&name));
        for line in text.lines() {
            let _ = writeln!(out, "{}", indent(line));
        }
    }
    let _ = writeln!(out, "\n{}", console.bold("  places"));
    for spec in place_specs(plan) {
        let _ = writeln!(out, "    {}", place_argv(&spec).join(" "));
    }
    out
}

/// What an install does, in the order it does it: a row of the checklist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Engine,
    NodeImage,
    DeskImage,
    Postgres,
    Registry,
    Desk,
    Runtime,
    Kvasir,
    Assistant,
    Places,
    Services,
}

/// The rows of the checklist for a plan: every stage the install passes
/// through, named as a person reads it.
fn stages(plan: &Plan, only_update: bool) -> Vec<(Stage, String)> {
    let mut out = Vec::new();
    if plan.runtime.container() {
        out.push((Stage::Engine, "engine image".to_string()));
        if plan.has(Part::Assistant) {
            out.push((Stage::NodeImage, "Node image".to_string()));
        }
        if plan.has(Part::Desk) {
            out.push((Stage::DeskImage, "desk image".to_string()));
        }
    } else {
        out.push((Stage::Engine, "rule packs".to_string()));
    }
    if plan.postgres.is_some() {
        out.push((Stage::Postgres, format!("Postgres {POSTGRES_MAJOR}")));
    }
    if !plan.registry_exists && !only_update {
        out.push((Stage::Registry, "registry".to_string()));
    }
    if plan.has(Part::Desk) {
        out.push((Stage::Desk, "desk".to_string()));
    }
    if plan.has(Part::Assistant) {
        if plan.llama.is_some() {
            out.push((Stage::Runtime, LLAMA_PART.to_string()));
        }
        out.push((Stage::Kvasir, "Kvasir".to_string()));
        out.push((Stage::Assistant, "assistant".to_string()));
    }
    out.push((Stage::Places, "places".to_string()));
    if plan.service || plan.runtime.container() {
        out.push((Stage::Services, "services".to_string()));
    }
    out
}

/// A task's label as a row of the checklist has room for: an image by its
/// own name, without the registry it comes from.
fn brief(label: &str) -> String {
    label
        .replace("ghcr.io/kineuro/", "")
        .replace("docker.io/library/", "")
}

/// A service as it was found once started: its unit or container, whether
/// it runs, and when it does not, what it said and where its log is.
struct Service {
    unit: String,
    running: bool,
    said: Vec<String>,
}

/// What starting the services left: each service as found, and the lines
/// that say so.
struct Started {
    services: Vec<Service>,
    text: String,
}

impl Started {
    fn of(services: Vec<Service>) -> Started {
        Started {
            text: services_text(&services),
            services,
        }
    }
}

/// Everything the plan said, in order.
fn do_it(
    plan: &Plan,
    console: &mut Console,
    args: &SetupArgs,
    existing: Option<State>,
    only_update: bool,
    answers: &Answers,
) -> Result<Vec<Service>, Exit> {
    let mut state = State {
        dir: plan.dir.display().to_string(),
        mode: plan.mode.name().to_string(),
        runtime: plan.runtime.name().to_string(),
        service: if plan.service {
            service_manager(plan.runtime, plan.system.is_some())
                .unwrap_or("none")
                .to_string()
        } else {
            "none".to_string()
        },
        // where the desk binds, which behind a proxy is its own answer: the
        // address a browser opens is the origin beside it
        reach: match &plan.reach {
            Reach::Loopback => "loopback".to_string(),
            Reach::Network(addr) => addr.clone(),
            Reach::Behind { network: true, .. } => {
                host_address().unwrap_or_else(|| "network".to_string())
            }
            Reach::Behind { network: false, .. } => "loopback".to_string(),
        },
        origin: plan.reach.proxied().unwrap_or_default().to_string(),
        backend: match &plan.backend {
            BackendChoice::Sqlite => "sqlite".to_string(),
            BackendChoice::Postgres { schema, .. } => format!("postgres:{schema}"),
        },
        at: nils_registry::time::now_iso(),
        ports: plan.ports,
        places: Vec::new(),
        parts: existing
            .as_ref()
            .map(|s| s.parts.clone())
            .unwrap_or_default(),
        programs: existing
            .as_ref()
            .map(|s| s.programs.clone())
            .unwrap_or_default(),
        unfinished: true,
        oidc: plan.oidc.clone(),
        system: plan.system.clone(),
    };
    let previous_parts = state.parts.clone();
    let existing_places = existing.map(|s| s.places).unwrap_or_default();

    // On record before anything is placed, and again as each thing is, so
    // an install that stops partway can still be removed and started again.
    write_state(&state)?;
    let (doing, done) = if only_update {
        ("Updating", "Updated")
    } else {
        ("Installing", "Installed")
    };
    console.start_checklist(doing, stages(plan, only_update));
    console.strict.set(!only_update);
    let placed = place(
        plan,
        console,
        args,
        &mut state,
        existing_places,
        only_update,
        answers,
    );
    console.strict.set(false);
    console.finish_checklist(
        placed.is_ok(),
        if placed.is_ok() { done } else { "Stopped" },
    );
    let services = match placed {
        Ok(services) => services,
        Err(e) => {
            let _ = write_state(&state);
            return Err(fail(format!(
                "{}\n  what was placed is on record: nils uninstall removes it, and nils setup \
                 starts again",
                e.message
            )));
        }
    };

    // A binary a part ran from before this run moved it into a container is
    // still on the machine, and still this install's to remove.
    for (name, before) in &previous_parts {
        let now = state.parts.get(name).map(|p| p.path.as_str());
        if before.kind == "binary" && now != Some(before.path.as_str()) {
            state.keep_program(Path::new(&before.path));
        }
    }
    // and one that is a part's own again is not listed twice
    let own: Vec<String> = state.parts.values().map(|p| p.path.clone()).collect();
    state.programs.retain(|p| !own.contains(p));

    state.unfinished = false;
    let path = write_state(&state)?;
    if !console.live {
        println!("  {}", path.display());
    }
    Ok(services)
}

/// The work of an install, each thing recorded in `state` as it is placed
/// and the record written again, so that where this stops the record says
/// what is there.
fn place(
    plan: &Plan,
    console: &mut Console,
    args: &SetupArgs,
    state: &mut State,
    existing_places: Vec<PlaceState>,
    only_update: bool,
    answers: &Answers,
) -> Result<Vec<Service>, Exit> {
    let checkpoint = |state: &State| {
        let _ = write_state(state);
    };
    for sub in ["registry", "desk", "backups", "working", "export"] {
        let path = plan.dir.join(sub);
        std::fs::create_dir_all(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }

    // The engine: the running binary, or an image.
    let me = std::env::current_exe()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .map_err(|e| fail(format!("this binary cannot say where it is: {e}")))?;
    let into = binary_dir(&me, plan);

    console.begin(Stage::Engine);
    if plan.runtime.container() {
        state.keep_program(&me);
        checkpoint(state);
        pull_or_build(plan, console, "nils", ENGINE_IMAGE, &me)?;
        // Taken now rather than by the first start, which a slow pull would
        // run past the time systemd gives a service to come up.
        if plan.has(Part::Assistant) {
            console.begin(Stage::NodeImage);
            if let Err(e) = console.task(
                "taking Node's image",
                &plan.dir,
                plan.runtime.name(),
                &["pull", NODE_IMAGE],
            ) {
                console.broken(&format!(
                    "{NODE_IMAGE} could not be pulled, and Kvasir and the assistant run in it: {}",
                    e.message
                ))?;
            }
        }
        if plan.has(Part::Desk) {
            console.begin(Stage::DeskImage);
            let desk_binary = match install_desk(&into, plan.channel.as_deref()) {
                Ok((_, path)) => {
                    state.keep_program(&path);
                    checkpoint(state);
                    path
                }
                Err(_) => into.join("nils-desk"),
            };
            pull_or_build(plan, console, "nils-desk", DESK_IMAGE, &desk_binary)?;
        }
    } else {
        install_packs(plan, &me, console)?;
    }

    state.parts.insert(
        "engine".to_string(),
        PartState {
            version: update::VERSION.to_string(),
            path: if plan.runtime.container() {
                format!("{ENGINE_IMAGE}:{}", plan.tag())
            } else {
                me.display().to_string()
            },
            kind: if plan.runtime.container() {
                plan.runtime.name().to_string()
            } else {
                "binary".to_string()
            },
        },
    );
    console.progress(&format!("engine {} ready", update::VERSION));
    checkpoint(state);

    // A Postgres this setup runs, up before the registry it holds.
    if let Some(pg) = plan.postgres {
        console.begin(Stage::Postgres);
        state.parts.insert(
            "postgres".to_string(),
            PartState {
                version: POSTGRES_MAJOR.to_string(),
                path: POSTGRES_IMAGE.to_string(),
                kind: pg.runtime.name().to_string(),
            },
        );
        checkpoint(state);
        start_postgres(plan, pg, console)?;
    }

    // The registry, made by the engine wherever it runs.
    let home = Home::new(plan.registry());
    if !plan.registry_exists && !only_update {
        console.begin(Stage::Registry);
        make_registry(plan, &home, console, args, answers.passphrase.as_deref())?;
    }
    // A Postgres registry is backed up with pg_dump. The engine's image
    // carries it; on the machine it has to be there already.
    if plan.runtime == Runtime::Machine
        && matches!(plan.backend, BackendChoice::Postgres { .. })
        && !have("pg_dump")
    {
        console.say("the registry's backups need pg_dump, which is not on this machine");
        console.say(&format!(
            "install the PostgreSQL {POSTGRES_MAJOR} client: postgresql-client-{POSTGRES_MAJOR} on Debian and Ubuntu"
        ));
    }

    // The desk.
    if plan.has(Part::Desk) {
        console.begin(Stage::Desk);
        if !plan.runtime.container() {
            match install_desk(&into, plan.channel.as_deref()) {
                Ok((version, path)) => {
                    console.progress(&format!("desk {version} at {}", path.display()));
                    state.parts.insert(
                        "desk".to_string(),
                        PartState {
                            version,
                            path: path.display().to_string(),
                            kind: "binary".to_string(),
                        },
                    );
                    checkpoint(state);
                }
                Err(e) => console.broken(&format!("the desk was not installed: {}", e.message))?,
            }
        } else {
            state.parts.insert(
                "desk".to_string(),
                PartState {
                    version: plan.version.clone(),
                    path: format!("{DESK_IMAGE}:{}", plan.tag()),
                    kind: plan.runtime.name().to_string(),
                },
            );
        }
    }

    // The provider, once the desk is on this machine to register itself at
    // an Authentik: what it answers is what the desk, the engine and Kvasir
    // are told.
    let registered;
    let plan = match register_desk(plan, state, answers, console) {
        Some(oidc) => {
            registered = Plan {
                oidc: Some(oidc),
                ..plan.clone()
            };
            &registered
        }
        None => plan,
    };
    state.oidc = plan.oidc.clone();
    if plan.has(Part::Desk) {
        if let Some(Provider::Registered {
            secret: Some(secret),
        }) = &answers.provider
        {
            write_secret(&plan.desk_dir().join("client-secret"), secret)?;
        }
        if let Err(e) = write_supervisor(plan) {
            console.warn(&format!("the supervisor was not set up: {}", e.message));
        }
        write_desk_config(plan)?;
        console.progress(&plan.desk_config().display().to_string());
        if plan.mode == Mode::Local {
            let desk = state
                .parts
                .get("desk")
                .filter(|p| p.kind == "binary")
                .map(|p| PathBuf::from(&p.path));
            add_first_admin(plan, desk, answers.first.as_ref(), console)?;
        }
        if plan.mode == Mode::Oidc && plan.oidc.is_none() {
            // a registration asked for and not made leaves nobody able to sign
            // in, which stops an install; a provider left for later was the
            // person's own choice, and is said
            let why =
                "the desk has no provider yet, so nobody can sign in and the engine does not start";
            if answers.provider.is_some() {
                console.broken(why)?;
            } else {
                console.warn(why);
            }
            console.say(
                "run nils setup again and name one, or register the desk at an Authentik with:",
            );
            let (_, origin, _) =
                desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
            console.say(&format!(
                "  nils-desk register --authentik https://auth.example.org --token ./api-token \\\n      --origin {origin} --allow <group> --bind reader=<group> --secret-file {}",
                plan.desk_dir().join("client-secret").display()
            ));
        }
    }

    // llama.cpp, which runs the models Kvasir starts: here before Kvasir is
    // configured, since kvasir.json names only a build that is here.
    if plan.has(Part::Assistant) && plan.llama.is_some() {
        console.begin(Stage::Runtime);
        install_llama(plan, state, console)?;
        checkpoint(state);
    }

    // The assistant and Kvasir, which are built rather than downloaded.
    if plan.has(Part::Assistant) {
        console.begin(Stage::Kvasir);
        match install_node_parts(plan, console, answers.model.as_ref()) {
            Ok(paths) => {
                for (name, path) in paths {
                    console.progress(&format!("{name} at {}", path.display()));
                    state.parts.insert(
                        name.to_string(),
                        PartState {
                            version: "from source".to_string(),
                            path: path.display().to_string(),
                            kind: "node".to_string(),
                        },
                    );
                }
                checkpoint(state);
            }
            Err(e) => console.broken(&format!("the assistant was not installed: {}", e.message))?,
        }
    }

    // The places, on an engine that keeps them.
    console.begin(Stage::Places);
    if only_update {
        state.places = existing_places;
    } else {
        state.places = declare_places(plan, &home, console)?;
    }
    checkpoint(state);

    // What each part reads and writes belongs to the account that part runs
    // as, before anything is started as that account.
    hand_over_files(plan, console);

    // Start it.
    let mut services = Vec::new();
    if plan.service {
        console.begin(Stage::Services);
        match start_everything(plan, state, console, answers.model.as_ref()) {
            Ok(started) => {
                console.report(&started);
                let stopped: Vec<&str> = started
                    .services
                    .iter()
                    .filter(|s| !s.running)
                    .map(|s| s.unit.as_str())
                    .collect();
                // what each said is kept for after the checklist, so the stop
                // only names them
                if console.strict.get() && !stopped.is_empty() {
                    return Err(fail(format!(
                        "{} did not start; what each said is above",
                        stopped.join(", ")
                    )));
                }
                services = started.services;
            }
            Err(e) => console.broken(&format!("the services were not started: {}", e.message))?,
        }
    } else if plan.runtime.container() {
        console.begin(Stage::Services);
        // No unit files were asked for, but a container still has to be
        // started, or the person is left with images and nothing running.
        // The assistant's starts once Kvasir has made its key.
        let (assistant, rest): (Vec<String>, Vec<String>) = container_commands(plan)
            .into_iter()
            .partition(|line| line.contains("--name nils-assistant"));
        let run = |line: &str| -> Result<(), Exit> {
            match run_line(line) {
                Ok(()) => console.progress(&short(line)),
                Err(e) => {
                    console.say(&format!("run: {line}"));
                    console.broken(&format!("{} did not start: {e}", short(line)))?;
                }
            }
            Ok(())
        };
        for line in &rest {
            run(line)?;
        }
        if let Some(line) = llama_command(plan, state) {
            console.say(&format!(
                "start llama.cpp, which runs the models Kvasir starts: {line}"
            ));
        }
        if !assistant.is_empty() {
            ready_kvasir(plan, console, answers.model.as_ref())?;
        }
        for line in &assistant {
            run(line)?;
        }
    }

    // The model and the assistant's key come from Kvasir. systemd, podman and
    // docker make them as they start, between Kvasir and the assistant, and
    // so did the containers just above; launchd has no Kvasir to wait for.
    let systemd = plan.runtime == Runtime::Machine && !cfg!(target_os = "macos");
    if plan.has(Part::Assistant) && plan.service && !systemd && !plan.runtime.container() {
        ready_kvasir(plan, console, answers.model.as_ref())?;
    }
    if plan.has(Part::Assistant) && !plan.service && !plan.runtime.container() {
        console.say(
            "with no services, start Kvasir yourself; then nils setup and repair adds its model \
             and makes the assistant's key",
        );
    }
    start_supervisor(plan, state, console);

    Ok(services)
}

fn container_commands(plan: &Plan) -> Vec<String> {
    match plan.runtime {
        Runtime::Podman => podman_commands(plan),
        Runtime::Docker => docker_commands(plan),
        Runtime::Machine => Vec::new(),
    }
}

/// Where a binary this wizard installs goes: beside the running one when
/// that directory takes a file, else `<dir>/bin`.
fn binary_dir(me: &Path, plan: &Plan) -> PathBuf {
    let beside = me.parent().unwrap_or(Path::new(".")).to_path_buf();
    if update::writable(&beside) {
        beside
    } else {
        plan.dir.join("bin")
    }
}

/// The image, pulled if the registry has it and built from the binary
/// beside us if it does not.
fn pull_or_build(
    plan: &Plan,
    console: &mut Console,
    part: &str,
    image: &str,
    binary: &Path,
) -> Result<(), Exit> {
    let engine = plan.runtime.name();
    let tag = format!("{image}:{}", plan.tag());
    // one line with a timer, as every slow step; a pull that fails is not
    // an error yet, since the image is then built here
    let pulled = quietly(engine, &["image", "exists", &tag])
        || console
            .task(
                &format!("taking {tag}"),
                &plan.dir,
                engine,
                &["pull", "--quiet", &tag],
            )
            .is_ok();
    if pulled {
        return Ok(());
    }
    console.note(&format!(
        "{tag} could not be pulled, so it is built here from the release binary"
    ));
    let context = plan.dir.join("images").join(part);
    std::fs::create_dir_all(&context).map_err(|e| fail(format!("{}: {e}", context.display())))?;
    std::fs::copy(binary, context.join(part))
        .map_err(|e| fail(format!("{}: {e}", binary.display())))?;
    std::fs::write(context.join("Containerfile"), containerfile(part))
        .map_err(|e| fail(format!("{}: {e}", context.display())))?;
    // the file named, since docker looks only for a Dockerfile on its own
    console.task(
        &format!("building {tag}"),
        &context,
        engine,
        &["build", "-f", "Containerfile", "-t", &tag, "."],
    )?;
    console.note(&format!("built {tag} from {}", context.display()));
    Ok(())
}

/// The password in a managed Postgres's environment file, when one is there.
fn postgres_password(env: &Path) -> Option<String> {
    let text = std::fs::read_to_string(env).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("POSTGRES_PASSWORD="))
        .map(str::to_string)
        .filter(|p| !p.is_empty())
}

/// The password of a connection string `postgres://user:password@...`.
fn dsn_password(dsn: &str) -> Option<&str> {
    let rest = dsn.split_once("://")?.1;
    let userinfo = &rest[..rest.rfind('@')?];
    userinfo.split_once(':').map(|(_, password)| password)
}

/// A managed Postgres's quadlet: standalone, not in the pod, so an engine on
/// the machine and one in a pod reach it alike on this machine's loopback.
fn postgres_quadlet(plan: &Plan) -> String {
    format!(
        "[Unit]\nDescription=Postgres for the NILS registry\n\n[Container]\n\
         Image={POSTGRES_IMAGE}\nContainerName={POSTGRES_CONTAINER}\nEnvironmentFile={env}\n\
         Volume={data}:{IN_POSTGRES}:U\nPublishPort=127.0.0.1:{port}:5432\n\n\
         [Install]\nWantedBy=default.target\n",
        env = plan.postgres_env().display(),
        data = plan.postgres_dir().display(),
        port = plan.ports.postgres,
    )
}

/// The docker run of a managed Postgres, as this account so its data is this
/// account's, restarted by docker. Published on this machine's loopback, and
/// on docker's bridge as well when the engine runs in docker and reaches it
/// there.
fn postgres_docker_run(plan: &Plan, bridge: Option<&str>) -> Vec<String> {
    let mut argv: Vec<String> = [
        "run",
        "-d",
        "--name",
        POSTGRES_CONTAINER,
        "--restart",
        "unless-stopped",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let account = as_this_account();
    if !account.is_empty() {
        argv.push("--user".to_string());
        argv.push(account);
    }
    argv.push("--env-file".to_string());
    argv.push(plan.postgres_env().display().to_string());
    argv.push("-v".to_string());
    argv.push(format!("{}:{IN_POSTGRES}", plan.postgres_dir().display()));
    argv.push("-p".to_string());
    argv.push(format!("127.0.0.1:{}:5432", plan.ports.postgres));
    if let Some(bridge) = bridge {
        argv.push("-p".to_string());
        argv.push(format!("{bridge}:{}:5432", plan.ports.postgres));
    }
    argv.push(POSTGRES_IMAGE.to_string());
    argv
}

/// Start the Postgres this setup runs and wait until it takes connections:
/// its environment written once and kept, its image taken, the container
/// started by the service manager where there is one.
fn start_postgres(plan: &Plan, pg: ManagedPostgres, console: &Console) -> Result<(), Exit> {
    let rt = pg.runtime.name();
    let data = plan.postgres_dir();
    std::fs::create_dir_all(&data).map_err(|e| fail(format!("{}: {e}", data.display())))?;
    let env = plan.postgres_env();
    if !env.exists() {
        let BackendChoice::Postgres { dsn, .. } = &plan.backend else {
            return Err(fail("a Postgres to run with no connection string for it"));
        };
        let password = dsn_password(dsn).unwrap_or_default();
        write_secret_bytes(
            &env,
            format!(
                "POSTGRES_USER=nils\nPOSTGRES_DB=nils\nPOSTGRES_PASSWORD={password}\n\
                 PGDATA={IN_POSTGRES}/pgdata\n"
            )
            .as_bytes(),
        )?;
    }
    let present = match pg.runtime {
        Runtime::Docker => quietly("docker", &["image", "inspect", POSTGRES_IMAGE]),
        _ => quietly("podman", &["image", "exists", POSTGRES_IMAGE]),
    };
    if !present {
        console.task(
            "taking Postgres's image",
            &plan.dir,
            rt,
            &["pull", "--quiet", POSTGRES_IMAGE],
        )?;
    }
    // a Postgres this setup runs is podman's, in a quadlet of this account's
    let with_systemd = plan.service && service_manager(Runtime::Podman, false).is_some();
    match pg.runtime {
        Runtime::Podman if with_systemd => {
            let dir = quadlet_dir();
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            std::fs::write(dir.join("nils-postgres.container"), postgres_quadlet(plan))
                .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            make_the_calls(&hand_units_to_systemd(&[], false, false), false)?;
            if !quietly("systemctl", &["--user", "restart", "nils-postgres"]) {
                return Err(fail(
                    "Postgres did not start; its log: journalctl --user -u nils-postgres",
                ));
            }
        }
        Runtime::Docker => {
            quietly("docker", &["rm", "-f", POSTGRES_CONTAINER]);
            let bridge = (plan.runtime == Runtime::Docker).then(|| {
                run_quiet(
                    "docker",
                    &[
                        "network",
                        "inspect",
                        "bridge",
                        "--format",
                        "{{(index .IPAM.Config 0).Gateway}}",
                    ],
                )
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "172.17.0.1".to_string())
            });
            let argv = postgres_docker_run(plan, bridge.as_deref());
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            console.task("starting Postgres", &plan.dir, "docker", &args)?;
        }
        _ => {
            quietly("podman", &["rm", "-f", POSTGRES_CONTAINER]);
            let data_mount = format!("{}:{IN_POSTGRES}:U", data.display());
            let publish = format!("127.0.0.1:{}:5432", plan.ports.postgres);
            let env_file = env.display().to_string();
            console.task(
                "starting Postgres",
                &plan.dir,
                "podman",
                &[
                    "run",
                    "-d",
                    "--name",
                    POSTGRES_CONTAINER,
                    "--env-file",
                    &env_file,
                    "-v",
                    &data_mount,
                    "-p",
                    &publish,
                    POSTGRES_IMAGE,
                ],
            )?;
        }
    }
    let started = std::time::Instant::now();
    loop {
        if quietly(
            rt,
            &[
                "exec",
                POSTGRES_CONTAINER,
                "pg_isready",
                "-h",
                "127.0.0.1",
                "-U",
                "nils",
                "-q",
            ],
        ) {
            break;
        }
        if started.elapsed().as_secs() >= 90 {
            console.waited();
            return Err(fail(format!(
                "Postgres did not take connections within 90 s; its log: {rt} logs {POSTGRES_CONTAINER}"
            )));
        }
        console.waiting("waiting for Postgres", started);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    console.waited();
    console.progress(&format!(
        "Postgres {POSTGRES_MAJOR} on 127.0.0.1:{}, its data in {}",
        plan.ports.postgres,
        data.display()
    ));
    Ok(())
}

/// A key and an empty registry, the two commands the documentation gives,
/// run here or inside the container that will use them.
fn make_registry(
    plan: &Plan,
    home: &Home,
    console: &mut Console,
    args: &SetupArgs,
    asked: Option<&str>,
) -> Result<(), Exit> {
    let passphrase = match (&args.key_file, asked) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map_err(|e| usage(format!("--key-file {}: {e}", path.display())))?,
        (None, Some(asked)) => asked.to_string(),
        (None, None) => match console.secret("A passphrase for the registry's key") {
            Some(secret) => secret,
            None => {
                let generated = generated_passphrase();
                let path = plan.dir.join("key.passphrase");
                write_secret(&path, &generated)?;
                console.say(&format!(
                    "no terminal to ask on, so a passphrase was made and written to {}",
                    path.display()
                ));
                console.say("move it into your password manager and delete the file");
                generated
            }
        },
    };

    if let BackendChoice::Postgres { dsn, schema } = &plan.backend {
        // Try the connection before anything is written, so a wrong dsn
        // costs a sentence rather than half a registry.
        if let Err(e) = nils_registry::store::Store::connect_postgres(dsn, schema) {
            return Err(fail(format!(
                "the database refused the connection: {}\n  check the connection string, that \
                 the database exists, and that the role may create a schema",
                with_causes(&e)
            )));
        }
        console.note("the database answered");
    }

    if plan.runtime.container() {
        let engine = plan.runtime.name();
        // at the path it has on this machine, as the engine's service mounts it
        let inside = plan.registry().display().to_string();
        let mount = format!(
            "{inside}:{inside}{}",
            if plan.runtime == Runtime::Podman {
                ":U"
            } else {
                ""
            }
        );
        let tag = format!("{ENGINE_IMAGE}:{}", plan.tag());
        // Docker does not remap the user, so the container has to be told
        // to be this account or it cannot write the directory it mounts.
        let account = as_this_account();
        let user: Vec<String> = if plan.runtime == Runtime::Docker && !account.is_empty() {
            vec!["--user".to_string(), account]
        } else {
            Vec::new()
        };
        // Postgres is reached from inside the container, at the address a
        // container names this machine by, and the registry is made there:
        // left out, the container made a SQLite registry beside a plan that
        // said Postgres.
        let mut network: Vec<String> = Vec::new();
        let mut backend: Vec<String> = Vec::new();
        if let BackendChoice::Postgres { dsn, schema } = &plan.backend {
            backend = vec![
                "--backend".to_string(),
                "postgres".to_string(),
                "--dsn".to_string(),
                dsn_for(plan.runtime, dsn),
                "--schema".to_string(),
                schema.clone(),
            ];
            match plan.runtime {
                Runtime::Docker => {
                    network = vec![
                        "--add-host".to_string(),
                        "host.docker.internal:host-gateway".to_string(),
                    ];
                }
                Runtime::Podman if plan.host_loopback => {
                    network = vec![
                        "--network".to_string(),
                        format!("pasta:--map-host-loopback={HOST_LOOPBACK_IN_POD}"),
                    ];
                }
                _ => {}
            }
        }
        let mut child = Command::new(engine)
            .args(["run", "--rm", "-i"])
            .args(&user)
            .args(["-v", &mount, &tag])
            .args(["--registry", &inside, "key", "add", "nils"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| fail(format!("{engine}: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{passphrase}");
        }
        // The engine's own lines about the key and the registry are the
        // machine install's too, which says them in one line; they are shown
        // only when a step fails.
        let said = |out: &std::process::Output| {
            let text = String::from_utf8_lossy(&out.stderr);
            text.lines().last().unwrap_or_default().to_string()
        };
        let added = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
        if !added.status.success() {
            return Err(fail(format!(
                "the key could not be added inside the container: {}",
                said(&added)
            )));
        }
        let made = Command::new(engine)
            .args(["run", "--rm"])
            .args(&user)
            .args(&network)
            .args(["-v", &mount, &tag])
            .args(["--registry", &inside, "init", "--key", "nils"])
            .args(&backend)
            .output()
            .map_err(|e| fail(format!("{engine}: {e}")))?;
        if !made.status.success() {
            let reach = match &plan.backend {
                BackendChoice::Postgres { dsn, .. } => {
                    postgres_reach_note(plan.runtime, dsn, podman_has_pasta)
                        .map(|note| format!("\n  {note}"))
                        .unwrap_or_default()
                }
                BackendChoice::Sqlite => String::new(),
            };
            return Err(fail(format!(
                "the registry could not be made inside the container: {}{reach}",
                said(&made)
            )));
        }
        console.progress(&format!("registry at {}", plan.registry().display()));
        return Ok(());
    }

    let (bytes, _) = nils_registry::keys::strip_newline(passphrase.as_bytes());
    home.keys(None)
        .add("nils", bytes)
        .map_err(|e| fail(e.to_string()))?;
    let opts = match &plan.backend {
        BackendChoice::Sqlite => InitOptions {
            backend: Backend::Sqlite,
            dsn: None,
            schema: None,
            scheme: Scheme::Blake2b32,
            key: "nils".to_string(),
            display_length: 12,
            session_scheme: None,
        },
        BackendChoice::Postgres { dsn, schema } => InitOptions {
            backend: Backend::Postgres,
            dsn: Some(dsn.clone()),
            schema: Some(schema.clone()),
            scheme: Scheme::Blake2b32,
            key: "nils".to_string(),
            display_length: 12,
            session_scheme: None,
        },
    };
    home.init(&opts).map_err(|e| fail(e.to_string()))?;
    console.progress(&format!("registry at {}", home.dir().display()));
    Ok(())
}

/// The places the engine now keeps, added in an order that lets the
/// registry name its backup. A place already there is left as it is, so a
/// second run of the wizard says the same thing as the first.
fn declare_places(plan: &Plan, home: &Home, console: &Console) -> Result<Vec<PlaceState>, Exit> {
    use nils_registry::place::{self, Role};
    // a place asked for and not declared is a registry that does not read
    // the directory the person named, so each failure here stops the install
    let mut registry = crate::open(home).map_err(|e| {
        fail(format!(
            "the registry did not open to declare its places: {}",
            e.message
        ))
    })?;
    let mut declared: Vec<PlaceState> = Vec::new();
    for spec in place_specs(plan) {
        let Some(role) = Role::parse(spec.role) else {
            continue;
        };
        let _ = std::fs::create_dir_all(&spec.path);
        let path = std::fs::canonicalize(&spec.path).unwrap_or_else(|_| spec.path.clone());
        let path = path.display().to_string();
        let row = PlaceState {
            name: spec.name.to_string(),
            role: spec.role.to_string(),
            path: path.clone(),
        };
        match place::by_name(registry.store(), spec.name) {
            Ok(Some(there)) => {
                // A place setup declared follows the answer given now: a
                // source named on a rerun moves the place, where keeping the
                // old path left the registry reading a directory the engine
                // was no longer given.
                let was = std::fs::canonicalize(&there.path)
                    .unwrap_or_else(|_| PathBuf::from(&there.path));
                if there.role == role && there.retired_at.is_none() && was != Path::new(&path) {
                    let probed = crate::places::probe(Path::new(&path));
                    match place::set(registry.store(), there.id, Some(&path), None, Some(&probed)) {
                        Ok(_) => {
                            let _ = crate::audit(
                                &mut registry,
                                nils_registry::audit::Action::PlaceSet,
                                serde_json::json!({"place": there.id, "name": spec.name, "moved": true}),
                                None,
                            );
                            console.progress(&format!("the {} place is now {path}", spec.name));
                            declared.push(row);
                        }
                        Err(e) => {
                            return Err(fail(format!(
                                "the {} place was not moved to {path}: {e}",
                                spec.name
                            )));
                        }
                    }
                    continue;
                }
                declared.push(PlaceState {
                    path: there.path,
                    ..row
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                return Err(fail(format!(
                    "the {} place could not be read: {e}",
                    spec.name
                )));
            }
        }
        let made = place::add(
            registry.store(),
            &place::New {
                name: spec.name,
                role,
                path: &path,
                guarantees: serde_json::json!({
                    "backup": spec.backup,
                    "snapshots": false,
                    "protected": false,
                    "fast": false,
                }),
                probed: crate::places::probe(Path::new(&path)),
                handling: serde_json::Value::Null,
                // a source place setup declares reads its folder itself
                // until a dataset is declared on it (record 26)
                dataset: serde_json::Value::Null,
            },
        );
        match made {
            Ok(id) => {
                let _ = crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PlaceAdd,
                    serde_json::json!({"place": id, "name": spec.name, "role": spec.role}),
                    None,
                );
                declared.push(row);
            }
            Err(e) => {
                return Err(fail(format!(
                    "the {} place was not declared: {e}",
                    spec.name
                )));
            }
        }
    }
    // Every other source place is kept on record too, so a later update
    // mounts a directory added at the desk even when it does not read the
    // registry.
    if let Ok(active) = place::active(registry.store()) {
        for p in active.into_iter().filter(|p| p.role == Role::Source) {
            if !declared.iter().any(|d| d.name == p.name) {
                declared.push(PlaceState {
                    name: p.name,
                    role: Role::Source.name().to_string(),
                    path: p.path,
                });
            }
        }
    }
    if !declared.is_empty() {
        console.progress(&format!("places: {}", say_places(&declared)));
    }
    Ok(declared)
}

/// The places as one line: `backups as backup, registry as registry`.
fn say_places(places: &[PlaceState]) -> String {
    places
        .iter()
        .map(|p| format!("{} as {}", p.name, p.role))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Thirty two bytes of the machine's own randomness, as text.
fn generated_passphrase() -> String {
    let mut bytes = [0u8; 24];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read as _;
        let _ = file.read_exact(&mut bytes);
    }
    hex::encode(bytes)
}

fn write_secret(path: &Path, text: &str) -> Result<(), Exit> {
    write_secret_bytes(path, format!("{text}\n").as_bytes())
}

/// A file only this user may read: mode 600 where the platform has modes.
fn write_secret_bytes(path: &Path, bytes: &[u8]) -> Result<(), Exit> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
    file.write_all(bytes)
        .map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// The desk's binary from its own releases, checked against their sums.
fn install_desk(into: &Path, channel: Option<&str>) -> Result<(String, PathBuf), Exit> {
    let base = update::desk_base(channel);
    let version = update::newest_version(&base)?;
    let target = update::host_target();
    let file = update::part_file("nils-desk", &target);
    let bytes = update::fetch_checked(&base, &version, &file)?;
    let name = if cfg!(windows) {
        "nils-desk.exe"
    } else {
        "nils-desk"
    };
    let path = into.join(name);
    update::install_binary(&path, &bytes)?;
    Ok((version, path))
}

/// The rule packs, which a machine install has no other way of getting: the
/// binary carries none and the release keeps them in one tarball beside it.
/// A container run needs none of this, because the image holds them.
///
/// They go where the engine looks by default, so that `nils digest` finds
/// them with no flag whoever runs it, not only the service this wizard
/// wrote.
fn install_packs(plan: &Plan, me: &Path, console: &mut Console) -> Result<(), Exit> {
    let dir = pack_destination(plan, me);
    let base = update::engine_base(plan.channel.as_deref());
    let version = update::newest_version(&base).unwrap_or_else(|_| update::VERSION.to_string());
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {e}", parent.display())))?;
    }
    match update::refresh_packs(&base, &version, &dir) {
        Ok(_) => {
            console.note(&format!("packs at {}", dir.display()));
            Ok(())
        }
        // The engine runs, digests and answers questions without packs, but
        // it cannot say what a scan is, which is most of what it is installed
        // for: an install stops here and says where the packs go, and an
        // update keeps the packs it had and says so.
        Err(e) => {
            console.broken(&format!(
                "the rule packs were not installed, so the engine could not say what a scan is: {e}"
            ))?;
            console.say("the engine still runs and digests; what it cannot do is classify.");
            console.say(&format!(
                "put a packs directory at {} or pass --pack-dir",
                dir.display()
            ));
            Ok(())
        }
    }
}

/// Where the packs go: the share directory of the prefix the engine binary
/// sits in, which is the place the engine looks at whoever runs it. Where
/// that prefix cannot be written, the registry's own directory, which the
/// engine looks at first of all.
fn pack_destination(plan: &Plan, me: &Path) -> PathBuf {
    if let Some(prefix) = me.parent().and_then(Path::parent) {
        let share = prefix.join("share").join("nils");
        if std::fs::create_dir_all(&share).is_ok() && update::writable(&share) {
            return share.join("packs");
        }
    }
    plan.registry().join("packs")
}

/// The desk's configuration once a plan is set: written whole where there is
/// none or where the file no longer reads as TOML, and otherwise the file on
/// disk with what setup writes set in it, keeping what a person set by hand.
fn write_desk_config(plan: &Plan) -> Result<(), Exit> {
    let path = plan.desk_config();
    let written = desk_config_text(plan);
    let text = match std::fs::read_to_string(&path) {
        Err(_) => written,
        Ok(existing) => match desk_config_merged(&existing, &written) {
            Ok(Some(merged)) => merged,
            Ok(None) => return Ok(()),
            Err(()) => written,
        },
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    }
    // it holds the supervisor's token, so only this account reads it
    write_secret_bytes(&path, text.as_bytes())
}

/// What setup keeps in the desk's configuration: the keys and the tables it
/// writes. Anything else in the file is a person's, and stays.
const DESK_MANAGED: [&str; 11] = [
    "bind",
    "origin",
    "also_origins",
    "mode",
    "store",
    "local",
    "oidc",
    "engine",
    "supervisor",
    "kvasir",
    "assistant",
];

/// The desk's configuration on disk with a plan's set in it: each key setup
/// writes replaced, each table it writes set key by key so a person's other
/// keys in it stay, and Kvasir's and the assistant's tables removed
/// with them. A `[local]` or an `[oidc]` table stays for a desk that goes
/// back, and every key setup does not write is kept, though not the file's
/// comments. `Ok(None)` when nothing setup writes has changed; an error when
/// either text does not read as TOML.
fn desk_config_merged(existing: &str, written: &str) -> Result<Option<String>, ()> {
    let mut have: toml::Table = toml::from_str(existing).map_err(|_| ())?;
    let want: toml::Table = toml::from_str(written).map_err(|_| ())?;
    let mut changed = false;
    for key in DESK_MANAGED {
        match (have.get(key).cloned(), want.get(key)) {
            (Some(toml::Value::Table(mut mine)), Some(toml::Value::Table(theirs))) => {
                for (k, v) in theirs {
                    if mine.get(k) != Some(v) {
                        mine.insert(k.clone(), v.clone());
                        changed = true;
                    }
                }
                have.insert(key.to_string(), toml::Value::Table(mine));
            }
            (mine, Some(value)) => {
                if mine.as_ref() != Some(value) {
                    have.insert(key.to_string(), value.clone());
                    changed = true;
                }
            }
            (Some(_), None) if matches!(key, "kvasir" | "assistant") => {
                have.remove(key);
                changed = true;
            }
            _ => {}
        }
    }
    if !changed {
        return Ok(None);
    }
    let body = toml::to_string(&have).map_err(|_| ())?;
    Ok(Some(format!(
        "# Written by nils setup. The documentation is at\n\
         # https://kineuro.se/nils/docs/desk/configuration/\n{body}"
    )))
}

/// Whether the desk under a directory keeps anyone who signs in with a
/// password, read from its store without changing it. A store that is not
/// there, or does not read, keeps nobody.
fn desk_has_people(dir: &Path) -> bool {
    let store = dir.join("desk").join("nils-desk.sqlite");
    if !store.exists() {
        return false;
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    conn.query_row("SELECT EXISTS(SELECT 1 FROM user)", [], |r| {
        r.get::<_, i64>(0)
    })
    .is_ok_and(|n| n != 0)
}

/// The desk's configuration as setup writes it for a plan: the mode chosen,
/// and the tables for the parts installed beside it.
fn desk_config_text(plan: &Plan) -> String {
    let (bind, origin, also) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    let engine_url = match plan.runtime {
        Runtime::Docker => format!("http://nils-engine:{}", plan.ports.engine),
        _ => format!("http://127.0.0.1:{}", plan.ports.engine),
    };
    let mut text = String::new();
    let _ = writeln!(text, "# Written by nils setup. The documentation is at");
    let _ = writeln!(text, "# https://kineuro.se/nils/docs/desk/configuration/");
    let _ = writeln!(text, "bind = \"{bind}\"");
    let _ = writeln!(text, "origin = \"{origin}\"");
    let _ = writeln!(
        text,
        "also_origins = [{}]",
        also.iter()
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(text, "mode = \"{}\"", plan.mode.name());
    let _ = writeln!(text, "store = \"nils-desk.sqlite\"");
    if plan.mode == Mode::Local {
        let _ = writeln!(text, "\n[local]");
        let _ = writeln!(text, "key = \"nils-desk.key\"");
        let _ = writeln!(text, "audience = \"nils\"");
    }
    if plan.mode == Mode::Oidc {
        match &plan.oidc {
            Some(oidc) => {
                let _ = writeln!(text, "\n[oidc]");
                let _ = writeln!(text, "issuer = \"{}\"", oidc.issuer);
                let _ = writeln!(text, "client_id = \"{}\"", oidc.client_id);
                let _ = writeln!(text, "client_secret_file = \"client-secret\"");
                let _ = writeln!(text, "roles_claim = \"{}\"", oidc.roles_claim);
                // the provider's groups, which the desk's groups follow (record 25)
                let _ = writeln!(text, "groups_claim = \"groups\"");
                if let Some(scopes) = &oidc.scopes {
                    let listed: Vec<String> = scopes.iter().map(|s| format!("\"{s}\"")).collect();
                    let _ = writeln!(text, "scopes = [{}]", listed.join(", "));
                }
            }
            None => {
                let _ = writeln!(
                    text,
                    "\n# nils-desk register --authentik ... prints this table"
                );
                let _ = writeln!(text, "# [oidc]");
                let _ = writeln!(
                    text,
                    "# issuer = \"https://auth.example.org/application/o/nils/\""
                );
                let _ = writeln!(text, "# client_id = \"...\"");
                let _ = writeln!(text, "# client_secret_file = \"client-secret\"");
            }
        }
    }
    let _ = writeln!(text, "\n[engine]");
    let _ = writeln!(text, "url = \"{engine_url}\"");
    // the supervisor on this host, which Settings reach through the desk
    if let (Some(url), Some(token)) = (supervisor_url(plan), supervisor_token(plan)) {
        let _ = writeln!(text, "\n[supervisor]");
        let _ = writeln!(text, "url = \"{url}\"");
        let _ = writeln!(text, "token = \"{token}\"");
    }
    if plan.has(Part::Assistant) {
        // In a pod every part shares one loopback; on a docker network each
        // container is reached by its name.
        let (kvasir, assistant) = match plan.runtime {
            Runtime::Docker => ("nils-kvasir", "nils-assistant"),
            _ => ("127.0.0.1", "127.0.0.1"),
        };
        let _ = writeln!(text, "\n[kvasir]");
        let _ = writeln!(text, "url = \"http://{kvasir}:{}\"", plan.ports.kvasir);
        let _ = writeln!(text, "\n[assistant]");
        let _ = writeln!(
            text,
            "url = \"http://{assistant}:{}\"",
            plan.ports.assistant
        );
    }
    text
}

/// The desk's rule for a username (nils-desk's `users::add`): letters, digits,
/// dots, dashes and underscores.
fn desk_username_refusal(name: &str) -> Option<&'static str> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    (name.is_empty() || !name.chars().all(allowed))
        .then_some("a username is letters, digits, dots, dashes and underscores")
}

/// The desk's rule for a password (nils-desk's `users::add`): at least eight
/// characters, counted as the desk counts them, in bytes.
fn desk_password_refusal(password: &str) -> Option<&'static str> {
    (password.len() < 8).then_some("a password is at least eight characters")
}

/// In local mode the desk keeps the people, and an empty desk has nobody to
/// let in. The first person is added with the name and password asked for
/// with the other questions, both held to the desk's rules there; with nobody
/// named here, the summary says how to add one when the desk still keeps
/// nobody. A first person the desk still refuses stops the install before the
/// rest is started: an install nobody can sign in to is not finished.
fn add_first_admin(
    plan: &Plan,
    desk: Option<PathBuf>,
    first: Option<&(String, String)>,
    console: &Console,
) -> Result<(), Exit> {
    let Some((name, password)) = first else {
        return Ok(());
    };
    let by_hand = format!(
        "nils-desk user add <name> --admin --config {}",
        plan.desk_config().display()
    );
    let desk = desk
        .filter(|d| d.exists())
        .or_else(|| Some(plan.dir.join("bin").join("nils-desk")).filter(|d| d.exists()))
        .unwrap_or_else(|| PathBuf::from("nils-desk"));
    // The password was asked for with the other questions, hidden and twice,
    // and goes to the desk on its standard input. Handing the desk the
    // terminal instead left a person at an empty line with no prompt, and
    // what they typed was shown as they typed it.
    let child = Command::new(&desk)
        .args(["user", "add", name, "--admin", "--config"])
        .arg(plan.desk_config())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let done = child.and_then(|mut child| {
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{password}")?;
        }
        child.wait_with_output()
    });
    match done {
        Ok(out) if out.status.success() => {
            console.progress(&format!("{name} may sign in and do everything"));
            Ok(())
        }
        Ok(out) => {
            let why = String::from_utf8_lossy(&out.stderr);
            Err(fail(format!(
                "{name} was not added, so nobody could sign in: {}. Add them with: {by_hand}",
                why.lines().last().unwrap_or("the desk refused")
            )))
        }
        Err(e) => Err(fail(format!(
            "{name} was not added, so nobody could sign in ({e}). Add them with: {by_hand}"
        ))),
    }
}

/// Where the desk signs people in, in `oidc` mode: named with the flags, kept
/// from an earlier install, or asked. An Authentik is registered during the
/// install, once the desk is on this machine to do it, so this answers no
/// provider for it yet; a provider the desk is registered at already is
/// named here with the client's secret.
fn choose_provider(
    console: &mut Console,
    args: &SetupArgs,
    kept: Option<OidcPlan>,
    answers: &mut Answers,
) -> Result<Option<OidcPlan>, Stop> {
    if let Some(url) = &args.authentik {
        let token = args
            .authentik_token_file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        match token {
            Some(token) => {
                answers.provider = Some(Provider::Authentik {
                    url: url.trim().trim_end_matches('/').to_string(),
                    token,
                    users: args
                        .authentik_users
                        .clone()
                        .unwrap_or_else(|| "nils".into()),
                    admins: args
                        .authentik_admins
                        .clone()
                        .unwrap_or_else(|| "nils-admins".into()),
                });
                return Ok(None);
            }
            None => console.note(
                "--authentik needs --authentik-token-file, a file holding an API token of that \
                 Authentik",
            ),
        }
    }
    if let (Some(issuer), Some(client_id)) = (&args.oidc_issuer, &args.oidc_client_id) {
        let secret = args
            .oidc_client_secret_file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .map(|secret| secret.trim().to_string());
        let jwks = args.oidc_jwks.clone().or_else(|| discover_jwks(issuer));
        match jwks {
            Some(jwks) => {
                answers.provider = Some(Provider::Registered { secret });
                return Ok(Some(OidcPlan {
                    issuer: issuer.trim().to_string(),
                    client_id: client_id.trim().to_string(),
                    jwks,
                    roles_claim: args
                        .oidc_roles_claim
                        .clone()
                        .unwrap_or_else(|| "roles".into()),
                    scopes: Some(plain_scopes()),
                }));
            }
            None => console.note(&format!(
                "{issuer} does not say where its keys are; name them with --oidc-jwks"
            )),
        }
    }
    if let Some(kept) = kept
        && console.ask_yes_no(
            &format!("The desk signs people in at {}. Keep it?", kept.issuer),
            true,
        )?
    {
        return Ok(Some(kept));
    }
    if !console.interactive() {
        return Ok(None);
    }
    let pick = console.ask_choice(
        "Where is the desk registered?",
        &[
            (
                "At an Authentik, by setup",
                "with an API token of it; setup makes the application and binds your groups",
            ),
            (
                "At a provider already",
                "you have the issuer, the desk's client id and its secret",
            ),
            (
                "Later",
                "nobody signs in and the engine does not start until one is named",
            ),
        ],
        0,
    )?;
    match pick {
        0 => {
            let url = console.ask_line("The Authentik's address", "https://auth.example.org")?;
            let Some(token) = console.ask_hidden_once("An API token of that Authentik")? else {
                console.note("no token was given, so the desk is not registered");
                return Ok(None);
            };
            let users = console.ask_line("The group whose members may use NILS", "nils")?;
            let admins = console.ask_line(
                "The group of those who run it, as operators and admins",
                "nils-admins",
            )?;
            answers.provider = Some(Provider::Authentik {
                url: url.trim().trim_end_matches('/').to_string(),
                token,
                users: users.trim().to_string(),
                admins: admins.trim().to_string(),
            });
            Ok(None)
        }
        1 => {
            let issuer = console.ask_line("The issuer", "")?.trim().to_string();
            let client_id = console
                .ask_line("The desk's client id", "")?
                .trim()
                .to_string();
            let secret = console.ask_hidden_once("Its client secret")?;
            let roles_claim = console
                .ask_line("The claim that carries the entitlements", "roles")?
                .trim()
                .to_string();
            let found = console.probe(&format!("jwks {issuer}"), || discover_jwks(&issuer));
            let jwks = match found {
                Some(jwks) => {
                    console.note(&format!("{issuer} publishes its keys at {jwks}"));
                    jwks
                }
                None => console
                    .ask_line("Where it publishes its keys (the JWKS address)", "")?
                    .trim()
                    .to_string(),
            };
            if issuer.is_empty() || client_id.is_empty() || jwks.is_empty() {
                console.note("an issuer, a client id and the keys are needed, so none is named");
                return Ok(None);
            }
            answers.provider = Some(Provider::Registered { secret });
            Ok(Some(OidcPlan {
                issuer,
                client_id,
                jwks,
                roles_claim,
                scopes: Some(plain_scopes()),
            }))
        }
        _ => Ok(None),
    }
}

/// The scopes the desk asks a provider for that has no entitlements scope,
/// which is one only a registration at Authentik makes.
fn plain_scopes() -> Vec<String> {
    ["openid", "profile", "email", "offline_access"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Where a provider publishes its signing keys, as its discovery document
/// says.
fn discover_jwks(issuer: &str) -> Option<String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim().trim_end_matches('/')
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .into();
    let text = agent
        .get(&url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    doc["jwks_uri"].as_str().map(str::to_string)
}

/// The desk registered at an Authentik by the desk's own register command:
/// the application, its provider and signing key, and the groups bound to
/// the entitlements, each made where it is not there yet. The client's
/// secret goes beside the desk's configuration; the API token is kept only
/// while the command runs.
fn register_desk(
    plan: &Plan,
    state: &State,
    answers: &Answers,
    console: &Console,
) -> Option<OidcPlan> {
    let Some(Provider::Authentik {
        url,
        token,
        users,
        admins,
    }) = &answers.provider
    else {
        return None;
    };
    if !plan.has(Part::Desk) {
        console.note("an Authentik registers the desk, which this setup does not install");
        return None;
    }
    let dir = plan.desk_dir();
    let _ = std::fs::create_dir_all(&dir);
    let token_file = dir.join("authentik-token");
    if let Err(e) = write_secret(&token_file, token) {
        console.warn(&format!("the desk was not registered: {}", e.message));
        return None;
    }
    let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    // in a container the desk's directory is where the desk's own container has it
    let at = if plan.runtime.container() {
        PathBuf::from(IN_DESK)
    } else {
        dir.clone()
    };
    let mut argv: Vec<String> = vec![
        "register".into(),
        "--authentik".into(),
        url.clone(),
        "--token".into(),
        at.join("authentik-token").display().to_string(),
        "--origin".into(),
        origin,
        "--secret-file".into(),
        at.join("client-secret").display().to_string(),
    ];
    for group in [users, admins] {
        argv.push("--allow".into());
        argv.push(group.clone());
    }
    for (entitlement, group) in [
        ("reader", users),
        ("reviewer", users),
        ("assist", users),
        ("reader", admins),
        ("reviewer", admins),
        ("assist", admins),
        ("operator", admins),
        ("admin", admins),
    ] {
        argv.push("--bind".into());
        argv.push(format!("{entitlement}={group}"));
    }
    let mut command = match plan.runtime {
        Runtime::Machine => Command::new(
            state
                .parts
                .get("desk")
                .filter(|p| p.kind == "binary")
                .map(|p| PathBuf::from(&p.path))
                .filter(|d| d.exists())
                .or_else(|| Some(plan.dir.join("bin").join("nils-desk")).filter(|d| d.exists()))
                .unwrap_or_else(|| PathBuf::from("nils-desk")),
        ),
        Runtime::Podman => {
            let mut c = Command::new("podman");
            c.args(["run", "--rm", "-v"])
                .arg(format!("{}:{IN_DESK}:U", dir.display()))
                .arg(format!("{DESK_IMAGE}:{}", plan.tag()));
            c
        }
        Runtime::Docker => {
            let mut c = Command::new("docker");
            c.args(["run", "--rm"]);
            let account = as_this_account();
            if !account.is_empty() {
                c.args(["--user", &account]);
            }
            c.arg("-v")
                .arg(format!("{}:{IN_DESK}", dir.display()))
                .arg(format!("{DESK_IMAGE}:{}", plan.tag()));
            c
        }
    };
    console.progress(&format!("registering the desk at {url}"));
    let ran = command.args(&argv).stdin(Stdio::null()).output();
    let _ = std::fs::remove_file(&token_file);
    match ran {
        Ok(out) if out.status.success() => {
            match registered_at(&String::from_utf8_lossy(&out.stdout)) {
                Some(oidc) => {
                    console.progress(&format!("the desk is registered at {}", oidc.issuer));
                    Some(oidc)
                }
                None => {
                    console.warn("the desk's registration named no client");
                    None
                }
            }
        }
        Ok(out) => {
            let why = String::from_utf8_lossy(&out.stderr);
            console.warn(&format!(
                "the desk was not registered: {}",
                why.lines().last().unwrap_or("the desk refused")
            ));
            None
        }
        Err(e) => {
            console.warn(&format!("the desk was not registered: {e}"));
            None
        }
    }
}

/// The provider a registration names, from the `[oidc]` table the desk's
/// register command prints; an Authentik publishes its keys under the
/// issuer's jwks/.
fn registered_at(said: &str) -> Option<OidcPlan> {
    let field = |key: &str| {
        said.lines().find_map(|line| {
            line.trim()
                .strip_prefix(key)?
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_string)
        })
    };
    let issuer = field("issuer")?;
    let client_id = field("client_id").filter(|id| !id.is_empty())?;
    Some(OidcPlan {
        jwks: format!("{issuer}jwks/"),
        issuer,
        client_id,
        roles_claim: "roles".to_string(),
        scopes: None,
    })
}

/// What setup does with the desk's configuration it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeskConfigFate {
    /// there is none, so it is written whole
    New,
    /// everything setup writes in it agrees with the plan
    Kept,
    /// what setup writes is set again, and what a person set by hand stays
    Updated,
    /// it no longer reads as TOML, so it is written whole again
    Replaced,
}

impl DeskConfigFate {
    fn words(self) -> &'static str {
        match self {
            DeskConfigFate::New => "will be written",
            DeskConfigFate::Kept => "kept as it is",
            DeskConfigFate::Updated => "updated, keeping what was set by hand",
            DeskConfigFate::Replaced => "written again, since it does not read as TOML",
        }
    }
}

/// What `write_desk_config` will do with the file on disk, said in the plan
/// before anything is changed.
fn desk_config_fate(plan: &Plan) -> DeskConfigFate {
    match std::fs::read_to_string(plan.desk_config()) {
        Err(_) => DeskConfigFate::New,
        Ok(existing) => match desk_config_merged(&existing, &desk_config_text(plan)) {
            Ok(None) => DeskConfigFate::Kept,
            Ok(Some(_)) => DeskConfigFate::Updated,
            Err(()) => DeskConfigFate::Replaced,
        },
    }
}

/// What this machine lacks for a plan, found before anything is placed, so
/// an install never stops halfway for a tool it could have named at the start.
fn missing_for(plan: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    if plan.runtime.container() && !have(plan.runtime.name()) {
        out.push(format!("{}, which runs the parts", plan.runtime.name()));
    }
    if plan.has(Part::Assistant) {
        // the assistant and Kvasir are built on this machine, whatever runs them
        if node_major() < 22 {
            out.push("Node 22 or newer, which builds the assistant and Kvasir".to_string());
        }
        for (tool, does) in [
            ("git", "takes their source"),
            ("npm", "installs their packages"),
        ] {
            if !have(tool) {
                out.push(format!("{tool}, which {does}"));
            }
        }
    }
    out
}

/// The major version of the Node on the path; 0 where there is none.
fn node_major() -> u32 {
    run_quiet("node", &["--version"])
        .unwrap_or_default()
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The ref a Node part is taken at: the release tag pinned here, or, for
/// testing in a lab before that tag exists, the ref its variable names,
/// which may be a branch.
fn source_ref(pinned: &str, named: Option<&str>) -> String {
    named
        .map(str::trim)
        .filter(|named| !named.is_empty())
        .unwrap_or(pinned)
        .to_string()
}

/// A Node part's source by the part's name: its repository, the ref it is
/// taken at, and what a person reads it as.
fn node_source(name: &str) -> Option<(&'static str, String, &'static str)> {
    let (repo, pinned, variable, said) = match name {
        "kvasir" => (KVASIR_REPO, KVASIR_REF, "NILS_SETUP_KVASIR_REF", "Kvasir"),
        "assistant" => (
            ASSISTANT_REPO,
            ASSISTANT_REF,
            "NILS_SETUP_ASSISTANT_REF",
            "the assistant",
        ),
        _ => return None,
    };
    let named = std::env::var(variable).ok();
    Some((repo, source_ref(pinned, named.as_deref()), said))
}

/// The git commands that bring a Node part's source to a ref, each as its
/// arguments. With no checkout, a clone of that ref alone. With one, from a
/// checkout of main too, the ref fetched and checked out detached, which
/// takes a tag and a branch alike.
fn source_steps(repo: &str, reference: &str, into: &Path, checked_out: bool) -> Vec<Vec<String>> {
    let words = |list: &[&str]| list.iter().map(|w| (*w).to_string()).collect::<Vec<_>>();
    if checked_out {
        vec![
            words(&["fetch", "--depth", "1", "origin", reference]),
            words(&["checkout", "--detach", "FETCH_HEAD"]),
        ]
    } else {
        let into = into.display().to_string();
        vec![words(&[
            "clone", "--depth", "1", "--branch", reference, repo, &into,
        ])]
    }
}

/// A git command's line on screen: what it does, to which part, at which ref.
fn source_label(step: &[String], said: &str, reference: &str) -> String {
    match step.first().map(String::as_str) {
        Some("checkout") => format!("checking out {said} at {reference}"),
        _ => format!("fetching {said} at {reference}"),
    }
}

/// Kvasir and the assistant: taken at their release tags and built, since
/// neither ships a binary. Anything missing is said rather than guessed at.
fn install_node_parts(
    plan: &Plan,
    console: &mut Console,
    model: Option<&ModelChoice>,
) -> Result<Vec<(&'static str, PathBuf)>, Exit> {
    if node_major() < 22 {
        return Err(fail(
            "the assistant and Kvasir, the model gateway, are built with Node 22; install it and \
             run nils setup again",
        ));
    }
    if !have("git") {
        return Err(fail(
            "git is needed to take the source of the assistant and Kvasir",
        ));
    }

    let mut out = Vec::new();
    for name in ["kvasir", "assistant"] {
        if name == "assistant" {
            console.begin(Stage::Assistant);
        }
        let Some((repo, reference, said)) = node_source(name) else {
            continue;
        };
        let into = plan.dir.join(name);
        // a checkout is brought to the release, whatever it followed before
        let checked_out = into.join(".git").exists();
        let at = if checked_out { &into } else { &plan.dir };
        for step in source_steps(repo, &reference, &into, checked_out) {
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            console.task(&source_label(&step, said, &reference), at, "git", &args)?;
        }
        console.task(
            &format!("installing {said}'s packages"),
            &into,
            "npm",
            &["ci", "--no-audit", "--no-fund", "--loglevel=error"],
        )?;
        console.task(&format!("building {said}"), &into, "npm", &["run", "build"])?;
        out.push((name, into));
    }

    configure_kvasir(plan, console, model)?;
    write_assistant_env(plan, model)?;
    Ok(out)
}

/// What the assistant will talk to, chosen by a person who may not know
/// what an OpenAI compatible address is: a model server or a provider, the
/// install's own ChatGPT subscription, or nothing yet.
struct ModelChoice {
    /// The model's OpenAI compatible address; empty for ChatGPT and for later.
    url: String,
    local: bool,
    key: Option<String>,
    /// The model's name as its server lists it, and `chatgpt` for ChatGPT.
    model: String,
    later: bool,
    /// The install's own ChatGPT subscription, signed in once Kvasir runs.
    chatgpt: bool,
}

impl ModelChoice {
    /// No model named yet.
    fn later() -> ModelChoice {
        ModelChoice {
            url: String::new(),
            local: true,
            key: None,
            model: String::new(),
            later: true,
            chatgpt: false,
        }
    }

    /// The install's own ChatGPT subscription: no address and no key, and
    /// the model the assistant names is Kvasir's ChatGPT.
    fn chatgpt() -> ModelChoice {
        ModelChoice {
            url: String::new(),
            local: false,
            key: None,
            model: CHATGPT.to_string(),
            later: false,
            chatgpt: true,
        }
    }
}

/// Kvasir's name for its ChatGPT backend and for the subscription, and so
/// the model the assistant names for it.
const CHATGPT: &str = "chatgpt";

/// The model on record, where the assistant names Kvasir's ChatGPT.
const CHATGPT_WORDS: &str = "ChatGPT subscription";

/// The address offered for a model server on this machine: SGLang's own
/// port, and the model the assistant's stations were written against.
const DEFAULT_MODEL_URL: &str = "http://127.0.0.1:30000/v1";

/// That model's name, offered where a server does not list its models.
const EXAMPLE_MODEL_ID: &str = "qwen38-27b";

/// Ask what the assistant should talk to, and what to type for it. Where the
/// address answers, the server is asked which models it serves, so the name
/// is picked from a list rather than remembered. Where nobody signs in, the
/// install's own ChatGPT subscription is offered; where people sign in, a
/// subscription is each person's own, signed in from the desk.
fn choose_model(console: &mut Console, served: bool, mode: Mode) -> Result<ModelChoice, Stop> {
    let example_model = EXAMPLE_MODEL_ID;
    console.heading("The model");
    console.note(
        "the assistant talks to a model through Kvasir, the model gateway, over the OpenAI chat \
         API, which almost every model server and provider speaks",
    );
    let subscription = mode == Mode::Off;
    if !subscription {
        console.note(
            "where people sign in, a ChatGPT subscription is each person's own: each signs in to \
             theirs from the desk",
        );
    }
    let mut options = vec![
        (
            "A model server on this machine",
            "SGLang, vLLM, llama.cpp or Ollama, already running here",
        ),
        (
            "A model server on another machine of yours",
            "the same, reached over your network; the prompt stays inside your own systems",
        ),
        (
            "A commercial provider",
            "OpenAI, OpenRouter, MiniMax or another; the prompt leaves your systems",
        ),
    ];
    if subscription {
        options.push((
            "Your ChatGPT subscription",
            "sign in with ChatGPT once setup has started Kvasir; the prompt leaves your systems",
        ));
    }
    options.push((
        "Decide later",
        "install it without a model now; run nils setup again to name one",
    ));
    let later = options.len() - 1;
    // a model named is asked one short question, and taken only once it answers
    loop {
        let pick = console.ask_choice(
            "What should it talk to?",
            &options,
            if served { 0 } else { later },
        )?;

        if pick == later {
            return Ok(model_later(console));
        }

        if subscription && pick == 3 {
            // a sign-in is made by a person, at the terminal
            if !console.interactive() {
                console.note("with nobody at the terminal to sign in, the model is left for later");
                return Ok(model_later(console));
            }
            console.note(
                "once setup has started Kvasir, it shows a link and a code: open the link, sign in \
                 with ChatGPT and enter the code",
            );
            console.note(
                "Kvasir keeps your registry's rows on your own systems unless you decide \
                 otherwise, so the questions that read the registry are not sent to ChatGPT until \
                 you open them: https://kineuro.se/nils/docs/assistant/kvasir/",
            );
            return Ok(ModelChoice::chatgpt());
        }

        let (url, local) = match pick {
            0 => {
                console.note(
                    "SGLang listens on http://127.0.0.1:30000/v1, vLLM on :8000/v1, llama.cpp on \
                 :8080/v1 and Ollama on :11434/v1",
                );
                (ask_address(console, DEFAULT_MODEL_URL)?, true)
            }
            1 => {
                console.note(
                    "the address of that server as this machine reaches it, for example \
                 http://192.168.1.20:30000/v1",
                );
                (ask_address(console, "")?, true)
            }
            _ => {
                console.note(
                    "its OpenAI compatible address: https://api.openai.com/v1, \
                 https://openrouter.ai/api/v1 or https://api.minimax.io/v1, among others",
                );
                (ask_address(console, "https://api.openai.com/v1")?, false)
            }
        };

        let key = if local {
            if console.ask_yes_no("Does that server need a key?", false)? {
                console.ask_hidden_once("Its key")?
            } else {
                None
            }
        } else {
            let key = console.ask_hidden_once("Your key from that provider")?;
            if key.is_none() {
                console.note("no key was given; the provider will refuse Kvasir until one is");
            }
            key
        };

        let listed = console.probe(&format!("models {url}"), || {
            list_models(&url, key.as_deref())
        });
        let model = match listed {
            Some(ids) if ids.len() == 1 => {
                console.note(&format!("{url} answered, serving {}", ids[0]));
                ids[0].clone()
            }
            Some(ids) if !ids.is_empty() => {
                console.note(&format!("{url} answered"));
                let shown: Vec<(&str, &str)> =
                    ids.iter().take(12).map(|id| (id.as_str(), "")).collect();
                let at = console.ask_choice("Which model?", &shown, 0)?;
                ids[at].clone()
            }
            _ => {
                console.note(&format!(
                    "{url} did not list its models; name the one to use"
                ));
                let default = if local { example_model } else { "" };
                loop {
                    let named =
                        console.ask_line("The model's name, as the server lists it", default)?;
                    if !named.trim().is_empty() || !console.interactive() {
                        break named.trim().to_string();
                    }
                    console.note("a model's name is needed, for example gpt-4.1-mini");
                }
            }
        };

        let answered = console.probe(
            &format!(
                "answers {url} {model} {}",
                key.as_deref().map_or(0, key_mark)
            ),
            || try_model(&url, key.as_deref(), &model),
        );
        match answered {
            Ok(()) => {
                console.note(&format!("{model} answered at {url}"));
                if !local {
                    console.note(
                        "Kvasir keeps your registry's rows on your own systems unless you decide \
                     otherwise, so questions that read the registry are refused by this provider \
                     until you open them: https://kineuro.se/nils/docs/assistant/kvasir/",
                    );
                }
                return Ok(ModelChoice {
                    url,
                    local,
                    key,
                    model,
                    later: false,
                    chatgpt: false,
                });
            }
            Err(why) => {
                console.note(&format!("{model} at {url} did not answer: {why}"));
                if !console.interactive() {
                    console.note("with nobody to ask, the model is left for later");
                    return Ok(model_later(console));
                }
                let next = console.ask_choice(
                    "A model is taken only once it answers. What now?",
                    &[
                        ("Name it again", "another address, key or model"),
                        (
                            "Decide later",
                            "install it without a model now; run nils setup again to name one",
                        ),
                    ],
                    0,
                )?;
                if next == 1 {
                    return Ok(model_later(console));
                }
            }
        }
    }
}

/// An http or https address, asked until one is given.
fn ask_address(console: &mut Console, default: &str) -> Result<String, Stop> {
    loop {
        let url = console
            .ask_line("Its address", default)?
            .trim()
            .trim_end_matches('/')
            .to_string();
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(url);
        }
        if !console.interactive() {
            return Ok(DEFAULT_MODEL_URL.to_string());
        }
        console.note("an address starting with http:// or https://");
    }
}

/// The models an OpenAI compatible server lists, when it answers at all.
fn list_models(url: &str, key: Option<&str>) -> Option<Vec<String>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(2)))
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .into();
    let mut request = agent.get(&format!("{url}/models"));
    if let Some(key) = key {
        request = request.header("authorization", &format!("Bearer {key}"));
    }
    let text = request.call().ok()?.body_mut().read_to_string().ok()?;
    let answer: serde_json::Value = serde_json::from_str(&text).ok()?;
    let ids: Vec<String> = answer["data"]
        .as_array()?
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    Some(ids)
}

/// No model named yet: Kvasir holds none, and the assistant answers once
/// nils setup names one, or one is added from the desk.
fn model_later(console: &mut Console) -> ModelChoice {
    console.note(
        "Kvasir holds no model for now, so the assistant does not answer yet; run nils setup \
         again to name one, or add one from the desk",
    );
    ModelChoice::later()
}

/// A mark of a key that tells one answer from another on the screens,
/// without showing or keeping the key.
fn key_mark(key: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// One short chat request to a model, the kind Kvasir will send it:
/// whether it answers, and when it does not, why, in a person's words.
fn try_model(url: &str, key: Option<&str>, model: &str) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(5)))
        .timeout_global(Some(std::time::Duration::from_secs(90)))
        .http_status_as_error(false)
        .build()
        .into();
    let asked = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "Say ready."}],
        "max_tokens": 8,
        "stream": false,
    });
    let mut request = agent
        .post(&format!("{url}/chat/completions"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", &format!("Bearer {key}"));
    }
    let mut response = request
        .send(asked.to_string())
        .map_err(|e| format!("nothing answered there ({e})"))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    model_answer(status, &text, model)
}

/// What the answer to that request says: the model answered, or why not.
fn model_answer(status: u16, text: &str, model: &str) -> Result<(), String> {
    let said: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let message = said
        .as_ref()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .or_else(|| v["detail"].as_str())
        })
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| {
            let cut: String = m.chars().take(160).collect();
            if cut.len() < m.len() {
                format!("{cut}...")
            } else {
                cut
            }
        });
    let with = |words: String| match &message {
        Some(m) => format!("{words}: {m}"),
        None => words,
    };
    match status {
        200..=299 if said.as_ref().is_some_and(|v| v["choices"].is_array()) => Ok(()),
        200..=299 => {
            Err("it answered, but not the way an OpenAI compatible server does".to_string())
        }
        401 | 403 => Err(with("the key was refused".to_string())),
        404 => Err(with(format!(
            "it serves no model named {model}, or this is not its OpenAI address"
        ))),
        429 => Err(with(
            "it refuses for now: too many requests, or no credit left".to_string(),
        )),
        _ => Err(with(format!("it answered {status}"))),
    }
}

/// The purposes the assistant uses: the host's own, from Kvasir's example,
/// and one for each station the assistant ships, read from that station's
/// own file. Kvasir refuses a purpose it was not told of, so a station
/// missing here is a station that never answers.
fn assistant_purposes(plan: &Plan, declared: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = declared.as_array().cloned().unwrap_or_default();
    let stations = plan.dir.join("assistant").join("stations");
    let mut found: Vec<(String, String)> = std::fs::read_dir(&stations)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join("station.yml")).ok()?;
            let field = |key: &str| {
                text.lines().find_map(|line| {
                    line.strip_prefix(key)
                        .and_then(|rest| rest.strip_prefix(':'))
                        .map(|value| value.trim().trim_matches('"').to_string())
                })
            };
            Some((
                field("purpose")?,
                field("content").unwrap_or_else(|| "rows".to_string()),
            ))
        })
        .collect();
    found.sort();
    for (id, content) in found {
        if out.iter().any(|p| p["id"] == id.as_str()) {
            continue;
        }
        out.push(serde_json::json!({
            "id": id,
            "app": "nils-assistant",
            "content": content,
            "kind": "foreground",
        }));
    }
    out
}

/// Kvasir's configuration: where it listens, an admin token made here, how
/// it knows its callers, and every purpose the assistant uses. It names no
/// model. Kvasir holds its models in its own database and refuses a file
/// that names any, so the model a person chose is kept beside the file until
/// Kvasir, once it runs, adds it. The file holds that token, so it is written
/// readable by nobody else.
///
/// An existing file is mended rather than rewritten, because a person may
/// have changed it by hand.
fn configure_kvasir(
    plan: &Plan,
    console: &mut Console,
    chosen: Option<&ModelChoice>,
) -> Result<(), Exit> {
    let dir = plan.dir.join("kvasir");
    let config = dir.join("kvasir.json");
    if config.exists() {
        repair_kvasir(plan, console)?;
    } else {
        let example = dir.join("kvasir.example.json");
        let text = std::fs::read_to_string(&example)
            .map_err(|e| fail(format!("{}: {e}", example.display())))?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| fail(format!("{}: {e}", example.display())))?;

        value["bind"] = serde_json::json!(kvasir_bind(plan));
        value["origin"] = serde_json::json!(format!("http://127.0.0.1:{}", plan.ports.kvasir));
        value["auth"] = kvasir_auth(plan, &generated_passphrase());
        value["purposes"] = serde_json::json!(assistant_purposes(plan, &value["purposes"]));
        // an example from before Kvasir held its models still names them
        if let Some(fields) = value.as_object_mut() {
            fields.remove("backends");
            fields.remove("oauth");
        }
        runtime_into_kvasir(&mut value, plan);

        write_secret_bytes(
            &config,
            serde_json::to_string_pretty(&value)
                .unwrap_or(text)
                .as_bytes(),
        )?;
        console.note(&format!("{} is written", config.display()));
    }
    match chosen {
        Some(chosen) if !chosen.later && !chosen.chatgpt => {
            keep_to_add(plan, vec![model_door_body(plan, chosen)])?;
            console.note(&format!(
                "Kvasir adds {} at {} once it runs",
                chosen.model, chosen.url
            ));
        }
        // ChatGPT, or no model for now, in place of a model chosen before
        // that Kvasir does not hold yet; a backend from before stays kept
        Some(chosen) => {
            let kept: Vec<serde_json::Value> = to_add(plan)
                .into_iter()
                .filter(|backend| backend["replaces"] != true)
                .collect();
            write_to_add(plan, &kept)?;
            if chosen.chatgpt {
                console.note("the ChatGPT sign-in follows once Kvasir runs");
            }
        }
        None => {}
    }
    if llama_built(plan).is_some() {
        write_runtime_files(plan)?;
    }
    Ok(())
}

/// The name the installer's token carries at Kvasir.
const INSTALLER: &str = "nils-setup:admin";

/// The id of the backend setup adds for the model a person chose, which the
/// assistant teaches on.
const MODEL_BACKEND: &str = "model";

/// How Kvasir knows its callers, following the desk's sign-in. In `off` mode
/// nobody signs in. In `local` mode it trusts the tokens the desk signs, as
/// the engine does, so a person reaches it through the desk under their own
/// roles; knowing only its own token, it refused every call the desk passed
/// on. With a provider it trusts the provider's tokens and the desk's, which
/// the desk signs for a person the provider named, once the provider is
/// named, and knows only its token until then. The installer's token is kept
/// in every mode, for what setup asks of Kvasir.
fn kvasir_auth(plan: &Plan, admin: &str) -> serde_json::Value {
    let tokens = serde_json::json!({ admin: INSTALLER });
    match plan.mode {
        Mode::Off => serde_json::json!({ "mode": "off", "tokens": tokens }),
        Mode::Local => {
            let (issuer, jwks) = desk_trust(plan);
            serde_json::json!({
                "mode": "oidc",
                "tokens": tokens,
                "trust": [{ "issuer": issuer, "audience": "nils", "jwks": jwks, "keepSubject": true }],
                "groupsClaim": "roles",
                "roles": {
                    "reader": "reader",
                    "reviewer": "reviewer",
                    "operator": "operator",
                    "admin": "admin",
                    "assist": "assist",
                },
            })
        }
        Mode::Oidc => match &plan.oidc {
            Some(oidc) => {
                let (issuer, jwks) = desk_trust(plan);
                serde_json::json!({
                    "mode": "oidc",
                    "tokens": tokens,
                    "trust": [
                        { "issuer": oidc.issuer, "audience": oidc.client_id, "jwks": oidc.jwks },
                        { "issuer": issuer, "audience": "nils", "jwks": jwks, "keepSubject": true },
                    ],
                    "groupsClaim": oidc.roles_claim,
                    "roles": {
                        "reader": "reader",
                        "reviewer": "reviewer",
                        "operator": "operator",
                        "admin": "admin",
                        "assist": "assist",
                    },
                })
            }
            None => serde_json::json!({ "mode": "token", "tokens": tokens }),
        },
    }
}

/// The model a person chose, as Kvasir's door takes it: under the id
/// `model`, at its address as Kvasir reaches it from where it runs, with
/// whether the prompt leaves this site, the model's name and its key. It is
/// marked as the wizard's, which takes the place of a `model` Kvasir holds
/// with another address or model.
fn model_door_body(plan: &Plan, chosen: &ModelChoice) -> serde_json::Value {
    let mut body = serde_json::json!({
        "id": MODEL_BACKEND,
        "baseUrl": model_address_for(plan.runtime, &chosen.url),
        "locality": if chosen.local { "local" } else { "remote" },
        "models": [chosen.model],
        "replaces": true,
    });
    if let Some(key) = &chosen.key {
        body["key"] = serde_json::json!(key);
    }
    body
}

/// The backends Kvasir is to hold, kept beside its configuration until it
/// holds each: the model chosen in the wizard, and the backends a kvasir.json
/// from before named. The file holds their keys, so it is readable by nobody
/// else, and it goes once Kvasir holds them all.
fn to_add_path(plan: &Plan) -> PathBuf {
    plan.dir.join("kvasir").join("backends-to-add.json")
}

/// The backends kept for Kvasir to add; none where there is no such file.
fn to_add(plan: &Plan) -> Vec<serde_json::Value> {
    std::fs::read_to_string(to_add_path(plan))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|kept| kept.as_array().cloned())
        .unwrap_or_default()
}

/// The backends still to add written down, and the file gone where none is
/// left.
fn write_to_add(plan: &Plan, backends: &[serde_json::Value]) -> Result<(), Exit> {
    let path = to_add_path(plan);
    if backends.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(fail(format!("{}: {e}", path.display())))
            }
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(backends).map_err(|e| fail(e.to_string()))?;
    write_secret_bytes(&path, text.as_bytes())
}

/// More backends kept for Kvasir to add, each taking the place of one kept
/// under the same id.
fn keep_to_add(plan: &Plan, more: Vec<serde_json::Value>) -> Result<(), Exit> {
    let mut kept = to_add(plan);
    for backend in more {
        kept.retain(|k| k["id"] != backend["id"]);
        kept.push(backend);
    }
    write_to_add(plan, &kept)
}

/// Where Kvasir listens. On the machine, loopback. In a container, every
/// address of the container's own network, which is what a published port
/// and the other containers reach; the port is published on this machine's
/// loopback alone.
fn kvasir_bind(plan: &Plan) -> String {
    if plan.runtime.container() {
        format!("0.0.0.0:{}", plan.ports.kvasir)
    } else {
        format!("127.0.0.1:{}", plan.ports.kvasir)
    }
}

/// Mend what an earlier version of this wizard, or of Kvasir, left in
/// kvasir.json, and say what changed.
fn repair_kvasir(plan: &Plan, console: &mut Console) -> Result<(), Exit> {
    let dir = plan.dir.join("kvasir");
    let config = dir.join("kvasir.json");
    let text =
        std::fs::read_to_string(&config).map_err(|e| fail(format!("{}: {e}", config.display())))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| fail(format!("{}: {e}", config.display())))?;
    let mut mended: Vec<String> = Vec::new();

    // Kvasir holds its models in its own database, and refuses a file that
    // still names backends or the OAuth of its first design. Each backend is
    // kept aside with its key before it leaves the file, and Kvasir adds it
    // again under the same id once it runs, so nothing a person set up is
    // lost. Its address is dialled from where Kvasir runs now, so a setup
    // changed from the machine to containers, or back, is mended here too.
    if let Some(named) = value
        .as_object_mut()
        .and_then(|fields| fields.remove("backends"))
    {
        let mut moved = Vec::new();
        for mut backend in named.as_array().cloned().unwrap_or_default() {
            if let Some(file) = backend["keyFile"].as_str().map(str::to_string) {
                match std::fs::read_to_string(dir.join(&file)) {
                    Ok(key) if !key.trim().is_empty() => {
                        backend["key"] = serde_json::json!(key.trim());
                    }
                    _ => mended.push(format!(
                        "dropped a key file that is not on this machine ({file})"
                    )),
                }
            }
            if let Some(fields) = backend.as_object_mut() {
                fields.remove("keyFile");
                fields.remove("provider");
                // a field an earlier file left empty is one Kvasir's door refuses
                fields.retain(|_, value| !value.is_null());
            }
            if let Some(url) = backend["baseUrl"].as_str().map(str::to_string) {
                backend["baseUrl"] = serde_json::json!(model_address_for(
                    plan.runtime,
                    &model_address_on_machine(&url)
                ));
            }
            moved.push(backend);
        }
        mended.push(if moved.is_empty() {
            "no longer names backends, since Kvasir holds its models itself".to_string()
        } else {
            format!(
                "no longer names its {} backend(s): Kvasir holds its models itself, and adds \
                 each again once it runs",
                moved.len()
            )
        });
        keep_to_add(plan, moved)?;
    }
    if value
        .as_object_mut()
        .and_then(|fields| fields.remove("oauth"))
        .is_some()
    {
        mended.push(
            "no longer names oauth, since a subscription is signed in through Kvasir".to_string(),
        );
    }
    // How Kvasir knows its callers follows the desk's sign-in; the tokens it
    // holds are kept, the installer's among them.
    if let Some(auth) = kvasir_auth_now(plan, &value["auth"]) {
        mended.push(format!(
            "knows its callers the way the desk signs them in ({})",
            plan.mode.name()
        ));
        value["auth"] = auth;
    }
    // Where Kvasir listens follows where it runs.
    let bind = kvasir_bind(plan);
    if value["bind"].as_str() != Some(bind.as_str()) {
        mended.push(format!("listens on {bind}, for where it runs"));
        value["bind"] = serde_json::json!(bind);
    }

    let purposes = assistant_purposes(plan, &value["purposes"]);
    let had = value["purposes"].as_array().map_or(0, Vec::len);
    if purposes.len() > had {
        mended.push(format!(
            "declared {} purpose(s) the assistant's stations use",
            purposes.len() - had
        ));
        value["purposes"] = serde_json::json!(purposes);
    }
    // where llama.cpp is, and how Kvasir in a container reaches this machine
    mended.extend(runtime_into_kvasir(&mut value, plan));

    // The key file an earlier wizard wrote for the model: its key is kept
    // for Kvasir above, or Kvasir holds it already.
    let leftover = dir.join("model.key");
    if leftover.exists() && std::fs::remove_file(&leftover).is_ok() {
        console.note(&format!(
            "{} is removed, since Kvasir keeps the model's key",
            leftover.display()
        ));
    }

    if mended.is_empty() {
        console.note(&format!(
            "{} is already there and was left alone",
            config.display()
        ));
        return Ok(());
    }
    write_secret_bytes(
        &config,
        serde_json::to_string_pretty(&value)
            .unwrap_or(text)
            .as_bytes(),
    )?;
    for line in mended {
        console.note(&format!("{}: {line}", config.display()));
    }
    Ok(())
}

/// The runtime's binary a plan names, where its build is here.
fn llama_built(plan: &Plan) -> Option<PathBuf> {
    plan.llama
        .map(|l| llama_build_dir(&plan.dir, l.variant).join("llama-server"))
        .filter(|server| server.is_file())
}

/// llama.cpp's address as Kvasir dials it from where Kvasir runs: this
/// machine's loopback, the loopback a pod is given, or docker's name for this
/// machine.
fn llama_url(plan: &Plan) -> String {
    let port = plan.ports.llama;
    match plan.runtime {
        Runtime::Machine => format!("http://127.0.0.1:{port}"),
        Runtime::Podman if plan.host_loopback => format!("http://{HOST_LOOPBACK_IN_POD}:{port}"),
        Runtime::Podman => format!("http://host.containers.internal:{port}"),
        Runtime::Docker => format!("http://host.docker.internal:{port}"),
    }
}

/// The name Kvasir in a container reaches this machine's own loopback by,
/// which it dials in place of a loopback address an admin gives it; none on
/// the machine.
fn host_alias(plan: &Plan) -> Option<String> {
    match plan.runtime {
        Runtime::Machine => None,
        Runtime::Podman if plan.host_loopback => Some(HOST_LOOPBACK_IN_POD.to_string()),
        runtime => host_from_container(runtime).map(str::to_string),
    }
}

/// kvasir.json told where llama.cpp is and how Kvasir reaches this machine,
/// the rest of it kept: `local.runtime` where the build is here, and
/// `hostAlias` where Kvasir runs in a container. What changed, in words.
fn runtime_into_kvasir(value: &mut serde_json::Value, plan: &Plan) -> Vec<String> {
    let mut said = Vec::new();
    if !value.is_object() {
        return said;
    }
    if let Some(llama) = plan.llama.filter(|_| llama_built(plan).is_some()) {
        let dir = plan.runtime_dir();
        let runtime = serde_json::json!({
            "url": llama_url(plan),
            "keyFile": dir.join("runtime.key").display().to_string(),
            "presets": dir.join("models.ini").display().to_string(),
            "log": dir.join("runtime.log").display().to_string(),
            "build": LLAMA_BUILD,
            "variant": llama.variant,
        });
        if value["local"]["runtime"] != runtime {
            if !value["local"].is_object() {
                value["local"] = serde_json::json!({});
            }
            value["local"]["runtime"] = runtime;
            said.push(format!(
                "names llama.cpp {LLAMA_BUILD}, which runs the models Kvasir starts"
            ));
        }
    }
    match host_alias(plan) {
        Some(alias) if value["hostAlias"].as_str() != Some(alias.as_str()) => {
            said.push(format!(
                "reaches this machine's own loopback as {alias}, from the container it runs in"
            ));
            value["hostAlias"] = serde_json::json!(alias);
        }
        None => {
            if let Some(fields) = value.as_object_mut()
                && fields.remove("hostAlias").is_some()
            {
                said.push("names no host alias, since it runs on the machine".to_string());
            }
        }
        _ => {}
    }
    said
}

/// The runtime's files in Kvasir's folder: the key only Kvasir and llama.cpp
/// read, made once, and the presets holding only their defaults' section
/// until Kvasir writes the models it starts into them. A file already there
/// stays.
fn write_runtime_files(plan: &Plan) -> Result<(), Exit> {
    let dir = plan.runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let key = dir.join("runtime.key");
    if !std::fs::read_to_string(&key).is_ok_and(|k| !k.trim().is_empty()) {
        write_secret(&key, &generated_passphrase())?;
    }
    let presets = dir.join("models.ini");
    if !presets.exists() {
        std::fs::write(&presets, "[*]\n")
            .map_err(|e| fail(format!("{}: {e}", presets.display())))?;
    }
    Ok(())
}

/// kvasir.json and the runtime's files brought to what a plan names, for an
/// update, which mends nothing else of Kvasir's.
fn mend_kvasir_runtime(plan: &Plan) {
    let config = plan.dir.join("kvasir").join("kvasir.json");
    if let Ok(text) = std::fs::read_to_string(&config)
        && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text)
        && !runtime_into_kvasir(&mut value, plan).is_empty()
    {
        let _ = write_secret_bytes(
            &config,
            serde_json::to_string_pretty(&value)
                .unwrap_or(text)
                .as_bytes(),
        );
    }
    if llama_built(plan).is_some() {
        let _ = write_runtime_files(plan);
    }
}

/// The build a plan names brought to this machine: its archive downloaded,
/// checked against the pinned sha256 and unpacked, unless it is here already,
/// and a build this install took before removed once it is. Answers the
/// build's folder, and whether it was taken now.
fn fetch_llama(plan: &Plan) -> Result<(PathBuf, bool), String> {
    let llama = plan
        .llama
        .ok_or_else(|| "llama.cpp publishes no build for this machine".to_string())?;
    let dir = llama_build_dir(&plan.dir, llama.variant);
    if dir.join("llama-server").is_file() {
        return Ok((dir, false));
    }
    let want = llama_digest(llama.variant)
        .ok_or_else(|| format!("no sha256 is pinned for the {} build", llama.variant))?;
    let url = llama_archive(&llama_base(), llama.variant);
    let bytes = crate::supervise::fetch(&url)?;
    unpack_llama(&bytes, want, &dir).map_err(|e| format!("{url}: {e}"))?;
    if let Ok(entries) = std::fs::read_dir(plan.dir.join(LLAMA_PART)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path != dir && llama_recorded(&path.display().to_string()).is_some() {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    Ok((dir, true))
}

/// llama.cpp placed for an install or a repair and recorded, with the devices
/// it runs a model on said. An archive that cannot be downloaded, or whose
/// sha256 is not the pinned one, stops an install, and is said on an update
/// or a repair, where Kvasir is then configured without it.
fn install_llama(plan: &Plan, state: &mut State, console: &Console) -> Result<(), Exit> {
    let Some(llama) = plan.llama else {
        return Ok(());
    };
    console.doing(&format!(
        "taking llama.cpp {LLAMA_BUILD}, the {} build",
        llama_words(llama.variant)
    ));
    let dir = match fetch_llama(plan) {
        Ok((dir, _)) => dir,
        Err(e) => {
            console.broken(&format!(
                "llama.cpp {LLAMA_BUILD}, which runs the models Kvasir starts, was not installed: {e}"
            ))?;
            console.say(&format!(
                "run nils setup again once it downloads, or point NILS_SETUP_LLAMA_RELEASES at a \
                 folder or server holding {LLAMA_BUILD}/llama-{LLAMA_BUILD}-bin-{}.tar.gz",
                llama.variant
            ));
            return Ok(());
        }
    };
    state.parts.insert(
        LLAMA_PART.to_string(),
        PartState {
            version: LLAMA_BUILD.to_string(),
            path: dir.display().to_string(),
            kind: LLAMA_PART.to_string(),
        },
    );
    console.progress(&format!("llama.cpp {LLAMA_BUILD} at {}", dir.display()));
    let devices = llama_devices(&dir.join("llama-server"));
    if devices.is_empty() {
        console.say(
            "llama.cpp finds no graphics device here, so a model Kvasir starts runs on the processor",
        );
    } else {
        console.say(&format!("llama.cpp runs a model on {}", devices.join("; ")));
    }
    if !llama.loader {
        console.say(NO_VULKAN_LOADER);
    }
    Ok(())
}

/// After `nils update` moves this binary, the setup record says so. The
/// wizard opens by naming what is installed, and it named the version the
/// install began with until something rewrote the record. Only the binary
/// the record names is written, and the answer is whether it was that one.
pub(crate) fn record_engine_version(path: &Path, version: &str) -> bool {
    let Some(mut state) = read_state() else {
        return false;
    };
    let Some(engine) = state.parts.get_mut("engine") else {
        return false;
    };
    let recorded =
        std::fs::canonicalize(&engine.path).unwrap_or_else(|_| PathBuf::from(&engine.path));
    let moved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if engine.kind != "binary" || recorded != moved {
        return false;
    }
    engine.version = version.to_string();
    let _ = write_state(&state);
    true
}

/// Everything the assistant reads from its environment, in one file its
/// service reads. Nothing here is a secret: Kvasir's key is a path.
fn write_assistant_env(plan: &Plan, chosen: Option<&ModelChoice>) -> Result<(), Exit> {
    let dir = plan.dir.join("assistant");
    let path = dir.join("assistant.env");
    // How the assistant reaches the engine and Kvasir, and where it listens,
    // follow where it runs: a pod shares one loopback, a docker network names
    // each container, and the desk in another container reaches the
    // assistant only if it listens beyond its own loopback.
    let (engine, kvasir, host) = match plan.runtime {
        Runtime::Docker => (
            format!("http://nils-engine:{}", plan.ports.engine),
            format!("http://nils-kvasir:{}", plan.ports.kvasir),
            Some("0.0.0.0"),
        ),
        _ => (
            format!("http://127.0.0.1:{}", plan.ports.engine),
            format!("http://127.0.0.1:{}", plan.ports.kvasir),
            None,
        ),
    };
    // The model the assistant asks for is the one chosen on this run, and
    // chatgpt for the install's ChatGPT subscription; with none named, the
    // assistant follows the first model Kvasir lists. A candidate it teaches
    // is served from the backend setup adds for the model chosen.
    let model = chosen.filter(|c| !c.later).map(|c| c.model.as_str());
    let teaches = chosen.is_some_and(|c| !c.later && !c.chatgpt);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        set_env_line(&mut lines, "NILS_URL", Some(&engine));
        set_env_line(&mut lines, "KVASIR_URL", Some(&kvasir));
        set_env_line(&mut lines, "HOST", host);
        // the teaching backend is the one setup adds, where none is named or
        // a model is chosen now
        let named = lines
            .iter()
            .any(|line| line.starts_with("ASSISTANT_TEACHING_BACKEND="));
        if teaches || !named {
            set_env_line(
                &mut lines,
                "ASSISTANT_TEACHING_BACKEND",
                Some(MODEL_BACKEND),
            );
        }
        if let Some(model) = model {
            set_env_line(&mut lines, "ASSISTANT_MODEL", Some(model));
        }
        let mended = format!("{}\n", lines.join("\n"));
        if mended != text {
            std::fs::write(&path, mended).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        }
        return Ok(());
    }
    let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    let mut text = String::from("# Written by nils setup.\n");
    if let Some(model) = model {
        let _ = writeln!(text, "ASSISTANT_MODEL={model}");
    }
    let _ = writeln!(text, "ASSISTANT_TEACHING_BACKEND={MODEL_BACKEND}");
    let _ = writeln!(text, "NILS_URL={engine}");
    let _ = writeln!(text, "KVASIR_URL={kvasir}");
    let _ = writeln!(
        text,
        "KVASIR_KEY_FILE={}",
        plan.dir.join("kvasir").join("assistant.key").display()
    );
    let _ = writeln!(
        text,
        "ASSISTANT_STATIONS={}",
        dir.join("stations").display()
    );
    let _ = writeln!(
        text,
        "ASSISTANT_STORE={}",
        dir.join("assistant.sqlite").display()
    );
    let _ = writeln!(
        text,
        "ASSISTANT_LEDGER={}",
        dir.join("seam.sqlite").display()
    );
    let _ = writeln!(
        text,
        "ASSISTANT_NOTES={}",
        dir.join("notes.sqlite").display()
    );
    let _ = writeln!(text, "DESK_ORIGIN={origin}");
    let _ = writeln!(text, "PORT={}", plan.ports.assistant);
    if let Some(host) = host {
        let _ = writeln!(text, "HOST={host}");
    }
    std::fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// One `KEY=value` line of an environment file set, added or taken out,
/// leaving every other line as it was.
fn set_env_line(lines: &mut Vec<String>, key: &str, value: Option<&str>) {
    let prefix = format!("{key}=");
    let at = lines.iter().position(|line| line.starts_with(&prefix));
    match (at, value) {
        (Some(i), Some(value)) => lines[i] = format!("{key}={value}"),
        (None, Some(value)) => lines.push(format!("{key}={value}")),
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, None) => {}
    }
}

/// The token this setup made for Kvasir, read back from its own file.
fn kvasir_admin_token(plan: &Plan) -> Option<String> {
    let config = kvasir_config(plan)?;
    let tokens = config["auth"]["tokens"].as_object()?;
    tokens
        .iter()
        .find(|(_, named)| named.as_str() == Some(INSTALLER))
        .or_else(|| tokens.iter().next())
        .map(|(token, _)| token.clone())
}

/// How Kvasir knows its callers as the desk signs them in now, keeping the
/// tokens it holds, the installer's among them: `None` where it already does,
/// or where the provider is not named yet.
fn kvasir_auth_now(plan: &Plan, current: &serde_json::Value) -> Option<serde_json::Value> {
    if plan.mode == Mode::Oidc && plan.oidc.is_none() {
        return None;
    }
    let mut tokens = current["tokens"].as_object().cloned().unwrap_or_default();
    let held = tokens
        .iter()
        .find(|(_, named)| named.as_str() == Some(INSTALLER))
        .map(|(token, _)| token.clone());
    let admin = held.unwrap_or_else(|| {
        let token = generated_passphrase();
        tokens.insert(token.clone(), serde_json::json!(INSTALLER));
        token
    });
    let mut auth = kvasir_auth(plan, &admin);
    auth["tokens"] = serde_json::Value::Object(tokens);
    (*current != auth).then_some(auth)
}

/// After an update, Kvasir knows its callers the way the desk signs them in
/// now: a desk that signs for the people a provider names needs Kvasir to
/// trust what it signs (record 25). Only `auth` changes, and one line says
/// so; an install without the file is left for setup.
fn mend_kvasir_auth(plan: &Plan) {
    let config = plan.dir.join("kvasir").join("kvasir.json");
    let Some(mut value) = kvasir_config(plan) else {
        return;
    };
    let Some(auth) = kvasir_auth_now(plan, &value["auth"]) else {
        return;
    };
    value["auth"] = auth;
    let Ok(text) = serde_json::to_string_pretty(&value) else {
        return;
    };
    match write_secret_bytes(&config, text.as_bytes()) {
        Ok(()) => println!(
            "Kvasir now knows its callers the way the desk signs them in ({})",
            plan.mode.name()
        ),
        Err(e) => println!("Kvasir's configuration was left alone: {}", e.message),
    }
}

/// Kvasir's configuration as setup wrote it, where it is there.
fn kvasir_config(plan: &Plan) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(plan.dir.join("kvasir").join("kvasir.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether Kvasir answers its health door, waiting for it a while. A service
/// that was started a moment ago is not up yet, and asking it once and
/// giving up is how an install ended by printing a command to run by hand
/// instead of doing the thing.
fn kvasir_up(plan: &Plan, console: &Console, seconds: u64) -> bool {
    let url = format!("http://127.0.0.1:{}/healthz", plan.ports.kvasir);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(1)))
        .build()
        .into();
    let started = std::time::Instant::now();
    let up = loop {
        if agent.get(&url).call().is_ok() {
            break true;
        }
        if started.elapsed().as_secs() >= seconds {
            break false;
        }
        console.waiting("waiting for Kvasir", started);
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    console.waited();
    up
}

/// How often a wait on Kvasir asks again: while a person signs in, and while
/// Kvasir admits a model on its own.
const POLL: std::time::Duration =
    std::time::Duration::from_millis(if cfg!(test) { 20 } else { 2_000 });

/// How long an admission may take: the suite asks the model itself for tool
/// calls, a schema and a stream.
const ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(1_800);

/// Kvasir made ready for the assistant, once it answers: the backends kept
/// for it added, the model chosen among them, since Kvasir holds its models
/// itself; a local model it does not list yet admitted, which Kvasir does on
/// its own for one just added; the install's ChatGPT subscription signed in
/// where it was chosen, or else a commercial provider that is the only model
/// given the purposes that read no rows, since a purpose goes only to a
/// local model until it is mapped; and the assistant's key, made again when
/// the stations' purposes have outgrown it. Waits for Kvasir first; where it
/// never comes up, the reason is Kvasir's own and is shown with the
/// services, so this says only what did not happen and how it will. Answers
/// whether the assistant has its key.
fn ready_kvasir(
    plan: &Plan,
    console: &Console,
    chosen: Option<&ModelChoice>,
) -> Result<bool, Exit> {
    let path = plan.dir.join("kvasir").join("assistant.key");
    let Some(admin) = kvasir_admin_token(plan) else {
        return Ok(path.exists());
    };
    if !kvasir_up(plan, console, 30) {
        let kept: Vec<String> = to_add(plan)
            .iter()
            .filter_map(|b| b["id"].as_str().map(str::to_string))
            .collect();
        if !kept.is_empty() {
            console.say("once it runs, nils setup and then repair adds them");
            console.broken(&format!(
                "Kvasir did not come up, so it does not hold {} yet",
                kept.join(", ")
            ))?;
        }
        if path.exists() {
            return Ok(true);
        }
        console.say("once it runs, nils setup and then repair makes the key");
        console.broken("Kvasir did not come up, so the assistant has no key")?;
        return Ok(false);
    }
    let kvasir = Kvasir {
        base: format!("http://127.0.0.1:{}", plan.ports.kvasir),
        admin,
    };
    // an admission record above this one is one written since this run began
    let since = kvasir
        .call("GET", "/v1/admission?limit=1", None, 10)
        .and_then(|answer| answer["records"][0]["id"].as_i64())
        .unwrap_or(0);
    let added = hold_backends(plan, &kvasir, console)?;
    admit_local_models(plan, &kvasir, console, &added, since)?;
    if plan.mode == Mode::Off && chosen.is_some_and(|c| c.chatgpt) {
        sign_in_to_chatgpt(&kvasir, console)?;
    } else {
        open_to_the_provider(&kvasir, console);
    }
    assistant_key(plan, &kvasir, console)
}

/// Kvasir's admin doors, called as the installer.
#[derive(Clone)]
struct Kvasir {
    base: String,
    admin: String,
}

impl Kvasir {
    /// A door's JSON answer, `null` for an empty one, or None where the door
    /// refused or did not answer within `seconds`.
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        seconds: u64,
    ) -> Option<serde_json::Value> {
        let (status, text) = self.send(method, path, body, seconds)?;
        if !(200..300).contains(&status) {
            return None;
        }
        if text.trim().is_empty() {
            return Some(serde_json::Value::Null);
        }
        serde_json::from_str(&text).ok()
    }

    /// A door's status with its JSON answer, `null` where there is none to
    /// read, whatever the status; None where nothing answered within
    /// `seconds`.
    fn answer(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        seconds: u64,
    ) -> Option<(u16, serde_json::Value)> {
        let (status, text) = self.send(method, path, body, seconds)?;
        Some((
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        ))
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        seconds: u64,
    ) -> Option<(u16, String)> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(seconds)))
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!("{}{path}", self.base);
        let bearer = format!("Bearer {}", self.admin);
        let sent = body.map(ToString::to_string).unwrap_or_default();
        let answered = match method {
            "GET" => agent.get(&url).header("authorization", &bearer).call(),
            "DELETE" => agent.delete(&url).header("authorization", &bearer).call(),
            "PUT" => agent
                .put(&url)
                .header("authorization", &bearer)
                .header("content-type", "application/json")
                .send(sent),
            _ => agent
                .post(&url)
                .header("authorization", &bearer)
                .header("content-type", "application/json")
                .send(sent),
        };
        let mut response = answered.ok()?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Some((status, text))
    }
}

/// The model ids a backend or a catalog names, each given as a name or as
/// an entry with its id.
fn model_ids(models: &serde_json::Value) -> Vec<String> {
    models
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().or_else(|| m["id"].as_str()).map(str::to_string))
        .collect()
}

/// Why a door refused, in Kvasir's own words where it gave some.
fn refusal(status: u16, said: &serde_json::Value) -> String {
    said["error"]["message"]
        .as_str()
        .map_or_else(|| format!("it answered {status}"), str::to_string)
}

/// Each model that did not answer Kvasir's short request, with Kvasir's
/// words for why.
fn unanswered(said: &serde_json::Value) -> String {
    let each: Vec<String> = said["error"]["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["answered"] != true)
        .map(|m| {
            format!(
                "{}: {}",
                m["id"].as_str().unwrap_or("a model"),
                m["error"]["message"].as_str().unwrap_or("no answer")
            )
        })
        .collect();
    if each.is_empty() {
        refusal(422, said)
    } else {
        each.join("; ")
    }
}

/// The backends kept for Kvasir added through its door, each held only once
/// every one of its models answered one short request from where Kvasir
/// runs. One Kvasir holds already with the same address and models is not
/// added again. The model chosen in the wizard takes the place of a `model`
/// held with another address or model; a backend a kvasir.json from before
/// named is left as Kvasir holds it. A chosen model Kvasir does not take
/// leaves the assistant with no model, which stops an install and is said on
/// an update; a backend from before that does not answer now is said. Either
/// is kept for the next run. Answers the ids added now, which Kvasir warms
/// and admits on its own.
fn hold_backends(plan: &Plan, kvasir: &Kvasir, console: &Console) -> Result<Vec<String>, Exit> {
    let kept = to_add(plan);
    if kept.is_empty() {
        return Ok(Vec::new());
    }
    let Some(held) = kvasir
        .call("GET", "/v1/backends", None, 10)
        .and_then(|answer| answer["backends"].as_array().cloned())
    else {
        console.say("nils setup and then repair adds them once Kvasir answers");
        console.broken("Kvasir did not say which models it holds, so none was added")?;
        return Ok(Vec::new());
    };
    let mut added = Vec::new();
    let mut left = Vec::new();
    let mut stop: Option<String> = None;
    for entry in kept {
        let id = entry["id"].as_str().unwrap_or_default().to_string();
        let replaces = entry["replaces"] == true;
        let mut body = entry.clone();
        if let Some(fields) = body.as_object_mut() {
            fields.remove("replaces");
        }
        let url = body["baseUrl"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let models = model_ids(&body["models"]);
        let named = models.join(", ");
        match held.iter().find(|b| b["id"] == id.as_str()) {
            Some(b)
                if b["base_url"].as_str().map(|u| u.trim_end_matches('/'))
                    == Some(url.as_str())
                    && model_ids(&b["models"]) == models =>
            {
                console.note(&format!("Kvasir holds {named} at {url} already"));
                continue;
            }
            Some(_) if !replaces => {
                console.note(&format!(
                    "Kvasir holds a backend named {id} already, so the one kvasir.json named is \
                     left as it holds it"
                ));
                continue;
            }
            // the model chosen now takes the place of the one held
            Some(_) => {
                let _ = kvasir.call("DELETE", &format!("/v1/backends/{id}"), None, 10);
            }
            None => {}
        }
        let started = std::time::Instant::now();
        let asked = kvasir.clone();
        let sent = body.clone();
        let run =
            std::thread::spawn(move || asked.answer("POST", "/v1/backends", Some(&sent), 300));
        while !run.is_finished() {
            console.waiting(&format!("Kvasir trying {named}"), started);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        console.waited();
        match run.join().ok().flatten() {
            Some((201, _)) => {
                console.progress(&format!("Kvasir holds {named} at {url}"));
                added.push(id);
            }
            Some((409, said)) => {
                console.note(&format!("Kvasir did not add {id}: {}", refusal(409, &said)));
            }
            Some((422, said)) => {
                let why = unanswered(&said);
                if replaces {
                    if let Some(note) = model_reach_note(
                        plan.runtime,
                        &model_address_on_machine(&url),
                        podman_has_pasta,
                    ) {
                        console.say(&note);
                    }
                    console.say(
                        "run nils setup again once the model server answers where Kvasir runs, \
                         or name another model",
                    );
                    stop = Some(format!(
                        "Kvasir could not reach {named} at {url} from where it runs ({why}), \
                         though setup reached it from this machine, so the assistant has no model"
                    ));
                } else {
                    console.warn(&format!(
                        "Kvasir did not take {id} back, since {named} did not answer it ({why}); \
                         nils setup and then repair tries again"
                    ));
                }
                left.push(entry);
            }
            other => {
                let why = other.map_or_else(
                    || "it did not answer".to_string(),
                    |(status, said)| refusal(status, &said),
                );
                if replaces {
                    stop = Some(format!(
                        "Kvasir did not take {named} ({why}), so the assistant has no model"
                    ));
                } else {
                    console.warn(&format!(
                        "Kvasir did not take {id} back ({why}); nils setup and then repair tries \
                         again"
                    ));
                }
                left.push(entry);
            }
        }
    }
    write_to_add(plan, &left)?;
    if let Some(why) = stop {
        console.broken(&why)?;
    }
    Ok(added)
}

/// A local model Kvasir does not list yet, admitted. Kvasir warms and admits
/// a backend added through its door on its own, so for one added now this
/// waits, for as long as admission takes, until the model is listed or its
/// admission record says it did not pass. One Kvasir held from before is run
/// through its admission door. The suite asks the model itself for tool
/// calls, a schema and a stream, so it takes a while, and a model that does
/// not pass stays unlisted, which is said.
fn admit_local_models(
    plan: &Plan,
    kvasir: &Kvasir,
    console: &Console,
    added: &[String],
    since: i64,
) -> Result<(), Exit> {
    let Some(held) = kvasir.call("GET", "/v1/backends", None, 10) else {
        return Ok(());
    };
    // what Kvasir lists now: a local model once it is admitted, and every
    // model where its gate is off
    let Some(catalog) = kvasir.call("GET", "/v1/config", None, 10) else {
        return Ok(());
    };
    let listed = model_ids(&catalog["models"]);
    for backend in held["backends"].as_array().into_iter().flatten() {
        let Some(id) = backend["id"].as_str() else {
            continue;
        };
        if backend["builtin"] == true || backend["locality"] != "local" {
            continue;
        }
        // Held at an address Kvasir does not reach from where it runs now, as
        // after a setup moved between the machine and containers: the model
        // is added again with its key, which only a person has.
        let url = backend["base_url"].as_str().unwrap_or_default();
        let dialled = model_address_for(plan.runtime, &model_address_on_machine(url));
        if !url.is_empty() && dialled != url {
            console.warn(&format!(
                "Kvasir holds {id} at {url}, which it does not reach from where it runs now; \
                 name the model again with nils setup, so Kvasir dials {dialled}"
            ));
            continue;
        }
        for name in model_ids(&backend["models"]) {
            if listed.contains(&name) {
                continue;
            }
            if added.iter().any(|a| a == id) {
                await_admission(kvasir, console, id, &name, since)?;
                continue;
            }
            if backend["health"]["warming"] == true
                && let Some(why) = backend["health"]["lastError"].as_str()
            {
                console.note(&format!(
                    "{name} does not answer Kvasir yet ({why}), so Kvasir has not admitted it; \
                     nils setup and then repair admits it once it does"
                ));
                continue;
            }
            run_admission(kvasir, console, id, &name)?;
        }
    }
    Ok(())
}

/// A wait while Kvasir admits a model it was just given, for as long as
/// admission takes: until the model is listed, or an admission record
/// written since this run began says it did not pass.
fn await_admission(
    kvasir: &Kvasir,
    console: &Console,
    backend: &str,
    name: &str,
    since: i64,
) -> Result<(), Exit> {
    let started = std::time::Instant::now();
    // None once admitted; a record that did not pass, or none where the wait ran out
    let refused: Option<Option<serde_json::Value>> = loop {
        let listed = kvasir
            .call("GET", "/v1/config", None, 10)
            .is_some_and(|catalog| model_ids(&catalog["models"]).iter().any(|m| m == name));
        if listed {
            break None;
        }
        let record = kvasir
            .call("GET", "/v1/admission?limit=50", None, 10)
            .and_then(|answer| {
                answer["records"]
                    .as_array()?
                    .iter()
                    .find(|r| {
                        r["id"].as_i64().is_some_and(|n| n > since)
                            && r["backend"] == backend
                            && r["model"] == name
                            && r["passed"] == false
                    })
                    .cloned()
            });
        if let Some(record) = record {
            break Some(Some(record));
        }
        if started.elapsed() >= ADMISSION_WAIT {
            break Some(None);
        }
        console.waiting(&format!("Kvasir admitting {name}"), started);
        std::thread::sleep(POLL);
    };
    console.waited();
    match refused {
        None => {
            console.note(&format!("Kvasir admitted {name}"));
            Ok(())
        }
        Some(record) => admission_refused(console, name, record.as_ref()),
    }
}

/// A model Kvasir held from before, run through its admission door.
fn run_admission(
    kvasir: &Kvasir,
    console: &Console,
    backend: &str,
    name: &str,
) -> Result<(), Exit> {
    let started = std::time::Instant::now();
    let asked = kvasir.clone();
    let body = serde_json::json!({ "backend": backend, "model": name });
    let run =
        std::thread::spawn(move || asked.call("POST", "/v1/admission/run", Some(&body), 1800));
    while !run.is_finished() {
        console.waiting(&format!("Kvasir admitting {name}"), started);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    console.waited();
    let record = run
        .join()
        .ok()
        .flatten()
        .and_then(|answer| answer["records"].as_array()?.first().cloned());
    match record {
        Some(r) if r["passed"] == true => {
            console.note(&format!("Kvasir admitted {name}"));
            Ok(())
        }
        other => admission_refused(console, name, other.as_ref()),
    }
}

/// A model that did not pass Kvasir's admission, with the checks it failed,
/// or whose admission did not finish.
fn admission_refused(
    console: &Console,
    name: &str,
    record: Option<&serde_json::Value>,
) -> Result<(), Exit> {
    match record {
        Some(r) => {
            let failed: Vec<&str> = r["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["passed"] == false)
                .filter_map(|c| c["name"].as_str())
                .collect();
            console.say(
                "Kvasir does not offer it until it passes; nils setup and then repair runs the \
                 suite again",
            );
            console.broken(&format!(
                "{name} did not pass Kvasir's admission ({}), so the assistant has no model it may use",
                failed.join(", ")
            ))
        }
        None => {
            console.say("nils setup and then repair runs the suite again");
            console.broken(&format!("Kvasir did not finish admitting {name}"))
        }
    }
}

/// A commercial provider as the only model: a purpose goes to a local model
/// unless it is mapped elsewhere, so with no local model every purpose was
/// refused. The purposes that read no rows of the archive are mapped to the
/// provider now. Those that read rows stay closed to it until an admin opens
/// them in the desk's settings, which records under that admin's name that
/// rows leave the site.
fn open_to_the_provider(kvasir: &Kvasir, console: &Console) {
    let Some(held) = kvasir.call("GET", "/v1/backends", None, 10) else {
        return;
    };
    // Kvasir's own ChatGPT is not a provider anyone added
    let added: Vec<&serde_json::Value> = held["backends"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["builtin"] != true)
        .collect();
    if added
        .iter()
        .any(|b| b["locality"] == "local" && !model_ids(&b["models"]).is_empty())
    {
        return;
    }
    let Some(remote) = added.iter().find(|b| b["locality"] == "remote") else {
        return;
    };
    let Some(id) = remote["id"].as_str() else {
        return;
    };
    let model = model_ids(&remote["models"])
        .into_iter()
        .next()
        .unwrap_or_else(|| id.to_string());
    open_to(kvasir, console, id, &model, false);
}

/// The purposes that read no rows of the archive mapped to a remote backend,
/// `to` in a person's words, and where the others stay said: closed to it
/// until an admin opens them in the desk's settings, never where they carry
/// identifiers, or on the local model that serves them. Only the purposes on
/// no backend yet are mapped, unless a person chose that backend for the
/// assistant, which maps every purpose of the assistant's that reads no rows.
fn open_to(kvasir: &Kvasir, console: &Console, backend: &str, to: &str, chosen: bool) {
    let Some(table) = kvasir.call("GET", "/v1/purposes", None, 10) else {
        return;
    };
    let mut opened = 0;
    let (mut closed, mut identifiers, mut local) = (Vec::new(), Vec::new(), Vec::new());
    for row in table["purposes"].as_array().into_iter().flatten() {
        let Some(purpose) = row["purpose"].as_str() else {
            continue;
        };
        if chosen && !purpose.starts_with("assistant.") {
            continue;
        }
        let on = row["backend"].as_str();
        match row["content"].as_str() {
            Some("catalog") if on == Some(backend) => {}
            Some("catalog") if on.is_none() || chosen => {
                let mapped = kvasir.call(
                    "PUT",
                    &format!("/v1/purposes/{purpose}/policy"),
                    Some(&serde_json::json!({ "backend": backend })),
                    10,
                );
                if mapped.is_some() {
                    opened += 1;
                }
            }
            Some(_) if row["locality"] == "local" => local.push(purpose),
            Some("rows") if on.is_none() => closed.push(purpose),
            Some("identifiers") if on.is_none() => identifiers.push(purpose),
            _ => {}
        }
    }
    if opened > 0 {
        console.say(&format!(
            "{opened} purpose(s) that read no rows of the archive go to {to}"
        ));
    }
    if !closed.is_empty() {
        console.say(&format!(
            "{} read rows of the archive and stay closed to {to} until an admin opens them in \
             the desk's settings",
            closed.join(", ")
        ));
    }
    if !identifiers.is_empty() {
        console.say(&format!(
            "{} carry identifiers and never leave your systems",
            identifiers.join(", ")
        ));
    }
    if !local.is_empty() {
        console.say(&format!(
            "{} stay local, on the model that serves them",
            local.join(", ")
        ));
    }
}

/// The install's ChatGPT subscription, as Kvasir says it stands.
fn subscription_state(kvasir: &Kvasir) -> Option<serde_json::Value> {
    let answer = kvasir.call("GET", "/v1/subscriptions", None, 10)?;
    answer["subscriptions"]
        .as_array()?
        .iter()
        .find(|s| s["provider"] == CHATGPT)
        .cloned()
}

/// Milliseconds since the epoch, as Kvasir writes its times.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// What a person reads to sign in to ChatGPT, in plain words: where to go,
/// the code to enter there, and how long the code works.
fn sign_in_words(waiting: &serde_json::Value, now: i64) -> Vec<String> {
    let link = waiting["verification_uri"]
        .as_str()
        .unwrap_or("ChatGPT's sign-in page");
    let code = waiting["user_code"].as_str().unwrap_or("Kvasir gave none");
    let mut out = vec![
        "Sign in to ChatGPT for the assistant:".to_string(),
        format!("open {link} in a browser, sign in, and enter the code {code}"),
    ];
    if let Some(expires) = waiting["expires_at"].as_i64() {
        let minutes = ((expires - now).max(0) + 59_999) / 60_000;
        out.push(format!("the code works for {minutes} minute(s)"));
    }
    out
}

/// The install's own ChatGPT subscription signed in, where nobody signs in
/// to the desk: Kvasir starts a sign-in with a device code, setup shows the
/// link and the code, and asks Kvasir every two seconds until it is signed
/// in, fails, or the code expires. Once it is signed in, the assistant's
/// purposes that read no rows go to it. One signed in already is kept. With
/// nobody at the terminal no sign-in is started, as with a model left for
/// later.
fn sign_in_to_chatgpt(kvasir: &Kvasir, console: &Console) -> Result<(), Exit> {
    const TO: &str = "your ChatGPT subscription";
    const LATER: &str =
        "to sign in later, run nils setup again and keep the ChatGPT subscription when it asks";
    if subscription_state(kvasir).is_some_and(|s| s["state"] == "signed_in") {
        open_to(kvasir, console, CHATGPT, TO, true);
        return Ok(());
    }
    if !console.interactive() {
        console.say(&format!(
            "with nobody at the terminal to sign in to ChatGPT, the assistant has no model yet; \
             {LATER}"
        ));
        return Ok(());
    }
    let started = kvasir.answer(
        "POST",
        &format!("/v1/subscriptions/{CHATGPT}/sign-in"),
        Some(&serde_json::json!({})),
        60,
    );
    let waiting = match started {
        Some((200, waiting)) => waiting,
        other => {
            let why = other.map_or_else(
                || "Kvasir did not answer".to_string(),
                |(status, said)| refusal(status, &said),
            );
            console.say(LATER);
            return console.broken(&format!(
                "the ChatGPT sign-in did not start ({why}), so the assistant has no model"
            ));
        }
    };
    console.show(&sign_in_words(&waiting, now_ms()));
    let expires = waiting["expires_at"].as_i64();
    let since = std::time::Instant::now();
    let outcome = loop {
        match subscription_state(kvasir) {
            Some(s) if s["state"] == "signed_in" => {
                break Ok(s["model"].as_str().map(str::to_string));
            }
            Some(s) if s["state"] == "failed" => {
                break Err(s["error"]
                    .as_str()
                    .unwrap_or("the sign-in did not finish")
                    .to_string());
            }
            Some(s) if s["state"] == "signed_out" => {
                break Err("Kvasir no longer waits for it".to_string());
            }
            _ => {}
        }
        if expires.is_some_and(|at| now_ms() > at + 5_000) {
            break Err("the code expired before it was approved".to_string());
        }
        console.waiting("waiting for you to sign in to ChatGPT", since);
        std::thread::sleep(POLL);
    };
    console.waited();
    console.show(&[]);
    match outcome {
        Ok(model) => {
            console.say(&match model {
                Some(model) => format!("signed in to ChatGPT; the assistant streams with {model}"),
                None => "signed in to ChatGPT".to_string(),
            });
            open_to(kvasir, console, CHATGPT, TO, true);
            Ok(())
        }
        Err(why) => {
            console.say(LATER);
            console.broken(&format!(
                "the ChatGPT sign-in did not finish ({why}), so the assistant has no model"
            ))
        }
    }
}

/// The assistant's key at Kvasir, for every purpose Kvasir declares for it.
/// A key made before a station was added lacks that station's purpose, and
/// Kvasir refuses the station, so a key that does not cover them all, or
/// that Kvasir no longer holds, is made again and the one it replaces is
/// revoked.
fn assistant_key(plan: &Plan, kvasir: &Kvasir, console: &Console) -> Result<bool, Exit> {
    let path = plan.dir.join("kvasir").join("assistant.key");
    let purposes: Vec<String> = kvasir_config(plan)
        .and_then(|c| {
            c["purposes"].as_array().map(|all| {
                all.iter()
                    .filter_map(|p| p["id"].as_str())
                    .filter(|id| id.starts_with("assistant."))
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default();
    // a key names its row: kvs_<id>.<secret>
    let held = std::fs::read_to_string(&path).ok().and_then(|key| {
        key.trim()
            .strip_prefix("kvs_")
            .and_then(|rest| rest.split_once('.'))
            .map(|(id, _)| id.to_string())
    });
    if path.exists() {
        let Some(keys) = kvasir.call("GET", "/v1/keys", None, 10) else {
            // a Kvasir that does not list its keys keeps the one there
            return Ok(true);
        };
        let now = now_ms();
        let row = held.as_deref().and_then(|id| {
            keys["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|k| k["id"] == id)
        });
        let covers = row.is_some_and(|k| {
            k["revokedAt"].is_null()
                && k["expiresAt"].as_i64().is_none_or(|at| at > now)
                && purposes.iter().all(|p| {
                    k["purposes"]
                        .as_array()
                        .is_some_and(|have| have.iter().any(|h| h.as_str() == Some(p.as_str())))
                })
        });
        if covers {
            return Ok(true);
        }
    }
    let body = serde_json::json!({
        "principal": "nils-assistant",
        "purposes": purposes,
        "max_class": "rows",
    });
    let minted = kvasir
        .call("POST", "/v1/keys", Some(&body), 10)
        .and_then(|answer| answer["key"].as_str().map(str::to_string));
    match minted {
        Some(key) if write_secret(&path, &key).is_ok() => {
            if let Some(old) = &held {
                let _ = kvasir.call("DELETE", &format!("/v1/keys/{old}"), None, 10);
                console
                    .note("the assistant's key is made again, for every purpose its stations use");
            } else {
                console.note(&format!("the assistant's key is at {}", path.display()));
            }
            Ok(true)
        }
        _ => {
            console.say("nils setup and then repair asks it again");
            console.broken("Kvasir would not make the assistant's key")?;
            Ok(path.exists())
        }
    }
}

// ------------------------------------------------------------- the services

/// Where an install's units live: the machine's own directory for the
/// services of this machine, and this account's directory for its own.
fn units_dir(system: bool) -> PathBuf {
    if system {
        return PathBuf::from(SYSTEM_UNITS_DIR);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("systemd")
        .join("user")
}

/// A `systemctl` call in the scope an install's units live in: the machine's
/// own manager for the services of this machine, and this account's manager
/// for its own.
fn systemctl_argv(system: bool, args: &[&str]) -> Vec<String> {
    let mut out = vec!["systemctl".to_string()];
    if !system {
        out.push("--user".to_string());
    }
    out.extend(args.iter().map(|word| (*word).to_string()));
    out
}

/// Such a call, run for its effect.
fn systemctl(system: bool, args: &[&str]) -> bool {
    let argv = systemctl_argv(system, args);
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    quietly("systemctl", &rest)
}

/// Such a call, run for what it says.
fn systemctl_says(system: bool, args: &[&str]) -> Option<String> {
    let argv = systemctl_argv(system, args);
    let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    run_quiet("systemctl", &rest)
}

/// Where a unit's log is, as a person would read it.
fn journal_line(system: bool, unit: &str) -> String {
    if system {
        format!("its log: journalctl -u {unit}")
    } else {
        format!("its log: journalctl --user -u {unit}")
    }
}

/// What a unit is wanted by: the machine's own target for a service of this
/// machine, and the account's for its own.
fn wanted_by(system: bool) -> &'static str {
    if system {
        "multi-user.target"
    } else {
        "default.target"
    }
}

fn quadlet_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("containers")
        .join("systemd")
}

/// Whatever this machine and runtime use to keep the parts running. Kvasir is
/// made ready, with the model chosen on this run, before the assistant starts.
fn start_everything(
    plan: &Plan,
    state: &State,
    console: &Console,
    chosen: Option<&ModelChoice>,
) -> Result<Started, Exit> {
    match (plan.runtime, cfg!(target_os = "macos")) {
        (Runtime::Podman, _) => {
            let dir = quadlet_dir();
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let written = quadlets(plan);
            for (name, text) in &written {
                std::fs::write(dir.join(name), text)
                    .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            }
            // a part this plan no longer has leaves no quadlet behind to start
            for name in [
                "nils-desk.container",
                "nils-kvasir.container",
                "nils-assistant.container",
            ] {
                if !written.iter().any(|(n, _)| n == name) && dir.join(name).exists() {
                    quietly(
                        "systemctl",
                        &["--user", "stop", name.trim_end_matches(".container")],
                    );
                    let _ = std::fs::remove_file(dir.join(name));
                }
            }
            // the account is lingered before this, since a machine nobody is
            // logged in as has no user manager to reload until it is
            for refused in make_the_calls(&hand_units_to_systemd(&[], false, false), false)? {
                console.warn(&format!("{refused} was refused"));
            }
            // A quadlet's unit is generated, so it is never enabled: the
            // [Install] section of the file is what systemd reads. The pod
            // is restarted, since what it publishes and how it is networked
            // are its own, and restarting it starts every container in it
            // again, from its new image tag and with rebuilt code. Each is
            // then started, which does nothing to one that is up. The
            // assistant's quadlet waits for the key Kvasir makes, so it
            // is started once that is made, as on the machine.
            quietly("systemctl", &["--user", "restart", "nils-pod"]);
            let mut units = vec!["nils-engine".to_string()];
            // llama.cpp runs on this machine, outside the pod, and is up before Kvasir
            if let Some(unit) = start_llama_unit(plan, state) {
                units.push(unit);
            }
            if plan.has(Part::Desk) {
                units.push("nils-desk".to_string());
            }
            if plan.has(Part::Assistant) {
                units.push("nils-kvasir".to_string());
            }
            for unit in &units {
                quietly("systemctl", &["--user", "start", unit]);
            }
            if plan.has(Part::Assistant) {
                ready_kvasir(plan, console, chosen)?;
                // again, since a key made again replaces the one it read
                quietly("systemctl", &["--user", "restart", "nils-assistant"]);
                units.push("nils-assistant".to_string());
            }
            Ok(Started::of(unit_report(
                &units,
                console,
                Watcher::Systemd { system: false },
            )))
        }
        (Runtime::Docker, _) => {
            let path = plan.dir.join("compose.yaml");
            std::fs::write(&path, docker_compose(plan))
                .map_err(|e| fail(format!("{}: {e}", path.display())))?;
            // Recreated, not only brought up: a Kvasir or an assistant built
            // again has the same configuration and would otherwise keep
            // running the old code. The assistant starts once Kvasir has made
            // its key. llama.cpp runs on this machine, outside docker, and is
            // up before Kvasir.
            let llama = start_llama_unit(plan, state);
            let mut services = vec!["engine"];
            if plan.has(Part::Desk) {
                services.push("desk");
            }
            if plan.has(Part::Assistant) {
                services.push("kvasir");
            }
            let mut args = vec!["compose", "up", "-d", "--force-recreate"];
            args.extend(&services);
            console.task("starting the containers", &plan.dir, "docker", &args)?;
            let mut containers: Vec<String> =
                services.iter().map(|s| format!("nils-{s}")).collect();
            if plan.has(Part::Assistant) {
                ready_kvasir(plan, console, chosen)?;
                console.task(
                    "starting the assistant",
                    &plan.dir,
                    "docker",
                    &["compose", "up", "-d", "--force-recreate", "assistant"],
                )?;
                containers.push("nils-assistant".to_string());
            }
            let mut reported = unit_report(&containers, console, Watcher::Docker);
            if let Some(unit) = llama {
                reported.extend(unit_report(
                    &[unit],
                    console,
                    Watcher::Systemd { system: false },
                ));
            }
            let mut started = Started::of(reported);
            let _ = writeln!(
                started.text,
                "  {}",
                console.dim(&format!(
                    "{} brings them back after a restart of docker",
                    path.display()
                ))
            );
            Ok(started)
        }
        (Runtime::Machine, true) => {
            let dir = std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join("Library").join("LaunchAgents"))
                .ok_or_else(|| fail("no home directory"))?;
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let mut names = Vec::new();
            for (name, text) in launchd_plists(plan, state) {
                let path = dir.join(&name);
                // An agent that is loaded already is not started again by a
                // load, so it is unloaded first; one that is not says so,
                // quietly.
                let _ = Command::new("launchctl")
                    .args(["unload"])
                    .arg(&path)
                    .output();
                std::fs::write(&path, text)
                    .map_err(|e| fail(format!("{}: {e}", path.display())))?;
                let _ = Command::new("launchctl")
                    .args(["load", "-w"])
                    .arg(&path)
                    .output();
                names.push(name);
            }
            Ok(Started {
                services: Vec::new(),
                text: format!("  services: {} in {}\n", names.join(", "), dir.display()),
            })
        }
        (Runtime::Machine, false) => {
            let system = plan.system.is_some();
            let dir = units_dir(system);
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let mut names = Vec::new();
            for (name, text) in systemd_units(plan, state) {
                std::fs::write(dir.join(&name), text)
                    .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
                names.push(name.trim_end_matches(".service").to_string());
            }
            // The account is lingered first, then the units are read and
            // enabled: on a machine nobody is logged in as there is no user
            // manager to read them until an account lingers, and this ran
            // the other way about, so nothing started and nothing said so.
            // The machine's own manager asks for no lingering. Enabling
            // prints a line for every link it makes, which is systemd's
            // business and not the person's.
            for refused in make_the_calls(&hand_units_to_systemd(&names, true, system), system)? {
                console.warn(&format!("{refused} was refused"));
            }
            // Where the parts run as accounts of their own, what each reads
            // and writes is that account's before it starts.
            hand_over_files(plan, console);
            // The assistant reads a key Kvasir makes, so everything else
            // starts first, the key is made once Kvasir answers, and the
            // assistant starts last. Started together, the assistant died
            // looking for a key that did not exist yet.
            let (assistant, rest): (Vec<&String>, Vec<&String>) =
                names.iter().partition(|u| u.as_str() == "nils-assistant");
            for unit in &rest {
                systemctl(system, &["restart", unit]);
            }
            if !assistant.is_empty() {
                ready_kvasir(plan, console, chosen)?;
                // the key Kvasir has just made is read by the account the
                // assistant runs as, not by the one that made it
                hand_over_files(plan, console);
                for unit in &assistant {
                    systemctl(system, &["restart", unit]);
                }
            }
            Ok(Started::of(unit_report(
                &names,
                console,
                Watcher::Systemd { system },
            )))
        }
    }
}

/// Run something for its effect, keeping its output to itself.
fn quietly(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// What watches the services: systemd, for units on the machine and podman's
/// quadlets, or docker, for its containers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Watcher {
    /// systemd, and whether the units are the machine's own rather than this
    /// account's.
    Systemd {
        system: bool,
    },
    Docker,
}

/// Which units are running, with the reason for any that is not. Each is
/// looked at twice, two seconds apart: a service that fails at start is
/// active for an instant and then restarting, and a single look at that
/// instant reported it running while it crashed in a loop.
fn unit_report(names: &[String], console: &Console, watcher: Watcher) -> Vec<Service> {
    let look = |unit: &str| match watcher {
        Watcher::Systemd { system } => {
            systemctl_says(system, &["is-active", unit]).is_some_and(|s| s.trim() == "active")
        }
        Watcher::Docker => run_quiet("docker", &["inspect", "-f", "{{.State.Running}}", unit])
            .is_some_and(|s| s.trim() == "true"),
    };
    console.doing("checking that the services came up");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let first: Vec<bool> = names.iter().map(|u| look(u)).collect();
    std::thread::sleep(std::time::Duration::from_secs(2));
    names
        .iter()
        .zip(first)
        .map(|(unit, was)| {
            let running = was && look(unit);
            let mut said = Vec::new();
            if !running {
                if let Some(why) = last_error(unit, watcher) {
                    // A container in a pod that did not start says only
                    // that; the reason is the pod's.
                    let pod = why
                        .contains("result 'dependency'")
                        .then(|| last_error("nils-pod", watcher))
                        .flatten();
                    said.push(format!("it said: {why}"));
                    if let Some(pod) = pod {
                        said.push(format!("the pod said: {pod}"));
                    }
                }
                said.push(match watcher {
                    Watcher::Systemd { system } => journal_line(system, unit),
                    Watcher::Docker => format!("its log: docker logs {unit}"),
                });
            }
            Service {
                unit: unit.clone(),
                running,
                said,
            }
        })
        .collect()
}

/// The services in lines: the running ones on one, and each that is not with
/// what it said.
fn services_text(services: &[Service]) -> String {
    let running: Vec<&str> = services
        .iter()
        .filter(|s| s.running)
        .map(|s| s.unit.as_str())
        .collect();
    let mut out = String::new();
    if !running.is_empty() {
        let _ = writeln!(out, "  running: {}", running.join(", "));
    }
    for service in services.iter().filter(|s| !s.running) {
        let _ = writeln!(out, "  not running: {}", service.unit);
        for line in &service.said {
            let _ = writeln!(out, "    {line}");
        }
    }
    out
}

/// The line of a unit's log most likely to say why it stopped.
fn last_error(unit: &str, watcher: Watcher) -> Option<String> {
    let log = match watcher {
        Watcher::Systemd { system } => {
            let mut args = vec!["-u", unit, "-n", "80", "--no-pager", "-o", "cat"];
            if !system {
                args.insert(0, "--user");
            }
            run_quiet("journalctl", &args)?
        }
        Watcher::Docker => {
            let out = Command::new("docker")
                .args(["logs", "--tail", "80", unit])
                .output()
                .ok()?;
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        }
    };
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    let telling = |line: &&&str| {
        let lower = line.to_lowercase();
        [
            "error", "enoent", "refused", "panicked", "cannot", "no such",
        ]
        .iter()
        .any(|word| lower.contains(word))
    };
    let pick = lines.iter().rev().find(telling).or_else(|| lines.last())?;
    let text: String = pick.trim().chars().take(160).collect();
    Some(text)
}

/// The account this process runs as, asked of the system rather than of the
/// environment: `$USER` names whoever last logged in, which under `pct exec`
/// and other contexts nobody logged in to is another account or none, and
/// the units written here belong to the account writing them.
fn whoami() -> Option<String> {
    for args in [["id", "-un"], ["id", "-u"]] {
        if let Some(out) = run_quiet(args[0], &args[1..])
            && !out.trim().is_empty()
        {
            return Some(out.trim().to_string());
        }
    }
    None
}

/// One call that hands systemd this account's units: what to run, and
/// whether an install can go on when it is refused.
#[derive(Debug)]
struct UnitCall {
    argv: Vec<String>,
    /// False where a refusal costs something short of the install: lingering
    /// is not allowed on every machine, and where it is not the services
    /// still run while the person is logged in.
    needed: bool,
}

/// What hands systemd the units just written, in the order it must happen.
/// Lingering comes first: it is what gives an account nobody is logged in as
/// a user manager at all, and every call after it needs one. Enabling is for
/// units written as files; podman's quadlets are generated from the files
/// and carry their own `[Install]` section instead.
fn hand_units_to_systemd(units: &[String], enable: bool, system: bool) -> Vec<UnitCall> {
    let mut out = Vec::new();
    // The machine's own manager is there whoever is logged in, and asks for
    // no lingering at all.
    if !system {
        let words = |argv: &[&str]| {
            argv.iter()
                .map(|w| (*w).to_string())
                .collect::<Vec<String>>()
        };
        out.push(UnitCall {
            argv: match whoami() {
                Some(me) => words(&["loginctl", "enable-linger", me.as_str()]),
                None => words(&["loginctl", "enable-linger"]),
            },
            needed: false,
        });
    }
    out.push(UnitCall {
        argv: systemctl_argv(system, &["daemon-reload"]),
        needed: true,
    });
    if enable {
        for unit in units {
            out.push(UnitCall {
                argv: systemctl_argv(system, &["enable", unit]),
                needed: false,
            });
        }
    }
    out
}

/// The calls made in the order they are given. One that is needed and is
/// refused stops the install there, with what was refused named: a daemon
/// that will not read the units means nothing of this install runs, and
/// saying so here is the difference between a sentence and an install that
/// writes every file and then reports that nothing started. The rest are
/// given back rather than swallowed, for the person to read.
fn make_the_calls(calls: &[UnitCall], system: bool) -> Result<Vec<String>, Exit> {
    let mut refused = Vec::new();
    for call in calls {
        let Some((program, args)) = call.argv.split_first() else {
            continue;
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        if quietly(program, &args) {
            continue;
        }
        if call.needed {
            let fix = if system {
                "run nils setup as root on a machine systemd runs".to_string()
            } else {
                format!(
                    "sign in as the account that runs NILS, or allow it to keep services without \
                     a login (loginctl enable-linger {})",
                    whoami().unwrap_or_else(|| "<account>".to_string())
                )
            };
            return Err(fail(format!(
                "systemd would not take the services: {} was refused, so nothing here was \
                 started; {fix}, and run nils setup again",
                call.argv.join(" ")
            )));
        }
        refused.push(call.argv.join(" "));
    }
    Ok(refused)
}

/// The lines a service of this machine carries that a service of an
/// account's own cannot: the account it runs as, the capabilities the engine
/// keeps, and, for a part that runs as another account than the engine's,
/// the home directories and the data it never reads kept out of its reach.
/// Empty for the services of this account, which is every install on a
/// laptop.
fn service_of_machine(plan: &Plan, part: &str) -> String {
    let Some(system) = &plan.system else {
        return String::new();
    };
    let account = system.account(part);
    let mut out = format!("User={account}\n");
    if part == "engine" {
        if !system.capabilities.is_empty() {
            let named = system.capabilities.join(" ");
            let _ = write!(
                out,
                "AmbientCapabilities={named}\nCapabilityBoundingSet={named}\n"
            );
        }
        return out;
    }
    // A part that does not run as the engine's account is one that never
    // reads the archive: it asks the engine at its own address. What it does
    // not read is kept out of its reach, and the home directories with it,
    // unless this install lives in one.
    if account == system.account("engine") {
        return out;
    }
    if !under_home(&plan.dir) {
        out.push_str("ProtectHome=yes\n");
    }
    for path in engine_data(plan) {
        let _ = writeln!(out, "InaccessiblePaths=-{}", path.display());
    }
    out
}

/// Whether a directory is under a home, where keeping the home directories
/// out of a service's reach would keep its own files out with them.
fn under_home(dir: &Path) -> bool {
    dir.starts_with("/home") || dir.starts_with("/root") || dir.starts_with("/Users")
}

/// What the engine alone reads: the registry, the archives and every folder
/// of DICOM it is given.
fn engine_data(plan: &Plan) -> Vec<PathBuf> {
    let mut out = vec![plan.registry(), plan.dir.join("backups")];
    for (_, path) in plan.read_from() {
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// What each part reads and writes, and the account it runs as: the
/// registry, the archives, the work in flight and what is exported are the
/// engine's; the desk's folder is the desk's; Kvasir's and the assistant's
/// are the assistant's. Nothing else is given away: the base directory
/// itself, the passphrase written where there was no terminal to ask on, and
/// the supervisor's folder stay with the account that ran setup.
///
/// Every file an install writes is written as whoever runs setup, which for
/// the services of this machine is root. A part that runs as an account of
/// its own has to be able to read its own configuration and write its own
/// store, or it starts and stops again while the install reports that it
/// finished.
fn files_of(plan: &Plan) -> Vec<(PathBuf, String)> {
    let Some(system) = &plan.system else {
        return Vec::new();
    };
    let engine = system.account("engine").to_string();
    let mut out: Vec<(PathBuf, String)> = ["registry", "backups", "working", "export"]
        .iter()
        .map(|sub| (plan.dir.join(sub), engine.clone()))
        .collect();
    if plan.has(Part::Desk) {
        out.push((plan.desk_dir(), system.account("desk").to_string()));
    }
    if plan.has(Part::Assistant) {
        let assistant = system.account("assistant").to_string();
        out.push((plan.dir.join("assistant"), assistant.clone()));
        out.push((plan.dir.join("kvasir"), assistant));
    }
    out
}

/// The files of each part given to the account that part runs as. A path
/// that is not there yet is passed over: this runs before the services start
/// and again once a part has been given what it needed to start, so a file
/// written between the two is caught by the second.
fn hand_over_files(plan: &Plan, console: &Console) {
    for (path, account) in files_of(plan) {
        if !path.exists() {
            continue;
        }
        let Some((uid, gid)) = account_ids(&account) else {
            console.warn(&format!(
                "{} was left as it is, since this machine has no {account} account",
                path.display()
            ));
            continue;
        };
        if let Err(e) = give_to(&path, uid, gid) {
            console.warn(&format!(
                "{} did not become {account}'s, so that part may not read it: {e}",
                path.display()
            ));
        }
    }
}

/// One file, or one tree, given to an account. A symbolic link is changed
/// rather than followed, so that a link into someone else's folder leaves
/// that folder alone.
fn give_to(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::lchown(path, Some(uid), Some(gid))?;
        if path.is_symlink() || !path.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(path)? {
            give_to(&entry?.path(), uid, gid)?;
        }
    }
    #[cfg(not(unix))]
    let _ = (path, uid, gid);
    Ok(())
}

/// The record an install of this plan would write, as far as the units read
/// it: the parts there would be, and what each runs from. For `--print`,
/// which writes nothing and so has no record to read.
fn planned_state(plan: &Plan) -> State {
    let me = std::env::current_exe()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|_| PathBuf::from("nils"));
    let into = binary_dir(&me, plan);
    let mut state = State {
        dir: plan.dir.display().to_string(),
        ports: plan.ports,
        system: plan.system.clone(),
        ..State::default()
    };
    let mut part = |name: &str, path: &Path, kind: &str| {
        state.parts.insert(
            name.to_string(),
            PartState {
                version: plan.version.clone(),
                path: path.display().to_string(),
                kind: kind.to_string(),
            },
        );
    };
    part("engine", &me, "binary");
    if plan.has(Part::Desk) && !plan.runtime.container() {
        part("desk", &into.join("nils-desk"), "binary");
    }
    if plan.has(Part::Assistant) {
        part("kvasir", &plan.dir.join("kvasir"), "node");
        part("assistant", &plan.dir.join("assistant"), "node");
    }
    state
}

/// What an install hands systemd once the units are written, as a person
/// would type it: the calls that give them over, then the restart of each.
fn unit_calls(plan: &Plan, names: &[String]) -> Vec<String> {
    let system = plan.system.is_some();
    let mut out: Vec<String> = hand_units_to_systemd(names, true, system)
        .iter()
        .map(|call| call.argv.join(" "))
        .collect();
    for name in names {
        out.push(systemctl_argv(system, &["restart", name]).join(" "));
    }
    out
}

/// One unit per part, for the parts that run on the machine.
pub(crate) fn systemd_units(plan: &Plan, state: &State) -> Vec<(String, String)> {
    let system = plan.system.is_some();
    let wanted = wanted_by(system);
    let engine = state
        .parts
        .get("engine")
        .map(|p| p.path.clone())
        .unwrap_or_else(|| "nils".to_string());
    // The engine connects to its registry at start, so it starts after a
    // Postgres this setup runs; a docker one is started by docker instead.
    let postgres_after = if plan.postgres.map(|p| p.runtime) == Some(Runtime::Podman) {
        "After=nils-postgres.service\nWants=nils-postgres.service\n"
    } else {
        ""
    };
    let mut out = vec![(
        "nils-engine.service".to_string(),
        format!(
            "[Unit]\nDescription=NILS engine\nAfter=network-online.target\n{postgres_after}\n[Service]\n{}\
             ExecStart={engine} {}\nRestart=on-failure\n\n[Install]\nWantedBy={wanted}\n",
            service_of_machine(plan, "engine"),
            engine_args(
                plan,
                &plan.registry().display().to_string(),
                &plan.dir.join("backups").display().to_string()
            )
            .join(" ")
        ),
    )];
    if let Some(desk) = state.parts.get("desk").filter(|d| d.kind == "binary") {
        out.push((
            "nils-desk.service".to_string(),
            format!(
                "[Unit]\nDescription=NILS desk\nAfter=nils-engine.service\n\n[Service]\n{}\
                 ExecStart={} serve --config {}\nWorkingDirectory={}\nRestart=on-failure\n\n\
                 [Install]\nWantedBy={wanted}\n",
                service_of_machine(plan, "desk"),
                desk.path,
                plan.desk_config().display(),
                plan.desk_dir().display(),
            ),
        ));
    }
    // llama.cpp is up before Kvasir, which starts models on it
    if let Some(part) = state.parts.get(LLAMA_PART) {
        out.push((
            "nils-llama.service".to_string(),
            llama_unit(plan, Path::new(&part.path), &llama_host(plan)),
        ));
    }
    if state.parts.contains_key("kvasir") {
        out.push((
            "kvasir.service".to_string(),
            format!(
                "[Unit]\nDescription=Kvasir, the model gateway\n\n[Service]\n{}\
                 ExecStart=/usr/bin/env node dist/main.js --config kvasir.json\n\
                 WorkingDirectory={}\nRestart=on-failure\n\n[Install]\nWantedBy={wanted}\n",
                service_of_machine(plan, "assistant"),
                plan.dir.join("kvasir").display()
            ),
        ));
    }
    if state.parts.contains_key("assistant") {
        let dir = plan.dir.join("assistant");
        out.push((
            "nils-assistant.service".to_string(),
            format!(
                "[Unit]\nDescription=NILS assistant\nAfter=nils-engine.service kvasir.service\n\n\
                 [Service]\n{}EnvironmentFile={}\n\
                 ExecStart=/usr/bin/env node {}\n\
                 WorkingDirectory={}\nRestart=on-failure\n\n[Install]\nWantedBy={wanted}\n",
                service_of_machine(plan, "assistant"),
                dir.join("assistant.env").display(),
                assistant_entry(&dir),
                dir.display()
            ),
        ));
    }
    out
}

/// What starts the assistant. Its own entry listens on loopback and stops
/// cleanly; Flue's, which a checkout from before it had, listens on every
/// interface.
fn assistant_entry(dir: &Path) -> &'static str {
    // A checkout not made yet will be made from main, which has the entry.
    if !dir.exists() || dir.join("bin").join("serve.mjs").exists() {
        "bin/serve.mjs"
    } else {
        "dist/app/server.mjs"
    }
}

/// Where llama.cpp listens: this machine's loopback, or for docker the
/// bridge's own address, where a container reaches the machine and a server
/// on the loopback alone does not answer it.
fn llama_host(plan: &Plan) -> String {
    match plan.runtime {
        Runtime::Docker => docker_bridge(),
        _ => "127.0.0.1".to_string(),
    }
}

/// llama.cpp's command: its server in router mode, which loads no model until
/// Kvasir asks and at most one at a time, reads Kvasir's presets and the key
/// only Kvasir holds besides it, and logs where Kvasir reads a failed load.
fn llama_argv(plan: &Plan, build: &Path, host: &str) -> Vec<String> {
    let dir = plan.runtime_dir();
    let at = |name: &str| dir.join(name).display().to_string();
    vec![
        build.join("llama-server").display().to_string(),
        "--models-preset".to_string(),
        at("models.ini"),
        "--no-models-autoload".to_string(),
        "--models-max".to_string(),
        "1".to_string(),
        "--api-key-file".to_string(),
        at("runtime.key"),
        "--host".to_string(),
        host.to_string(),
        "--port".to_string(),
        plan.ports.llama.to_string(),
        "--no-webui".to_string(),
        "--log-file".to_string(),
        at("runtime.log"),
    ]
}

/// llama.cpp's systemd user unit, on this machine whichever runtime the parts
/// use, started again when it fails.
fn llama_unit(plan: &Plan, build: &Path, host: &str) -> String {
    format!(
        "[Unit]\nDescription=llama.cpp, which runs the models Kvasir starts\n\
         After=network-online.target\n\n[Service]\n{}ExecStart={}\nWorkingDirectory={}\n\
         Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy={}\n",
        service_of_machine(plan, "assistant"),
        llama_argv(plan, build, host).join(" "),
        plan.runtime_dir().display(),
        wanted_by(plan.system.is_some())
    )
}

/// llama.cpp's command for a person to run, where no service runs it.
fn llama_command(plan: &Plan, state: &State) -> Option<String> {
    let part = state.parts.get(LLAMA_PART)?;
    Some(llama_argv(plan, Path::new(&part.path), &llama_host(plan)).join(" "))
}

/// llama.cpp's unit written and started on this machine ahead of Kvasir, for
/// an install whose parts run in containers; on the machine it is among the
/// units. None where no build is on record, and on macOS.
fn start_llama_unit(plan: &Plan, state: &State) -> Option<String> {
    if cfg!(target_os = "macos") {
        return None;
    }
    let part = state.parts.get(LLAMA_PART)?;
    let system = plan.system.is_some();
    let dir = units_dir(system);
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(
        dir.join("nils-llama.service"),
        llama_unit(plan, Path::new(&part.path), &llama_host(plan)),
    )
    .ok()?;
    systemctl(system, &["daemon-reload"]);
    systemctl(system, &["enable", "nils-llama"]);
    systemctl(system, &["restart", "nils-llama"]);
    Some("nils-llama".to_string())
}

/// A launchd agent that runs one command in a directory, started when it is
/// loaded and kept alive.
fn launchd_plist(label: &str, argv: &[String], cwd: &str) -> String {
    let args = argv
        .iter()
        .map(|a| format!("    <string>{a}</string>"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key><string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{args}\n  </array>\n\
         \x20 <key>WorkingDirectory</key><string>{cwd}</string>\n\
         \x20 <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n\
         </dict>\n</plist>\n"
    )
}

/// The same, as launchd agents, for a machine with no systemd.
pub(crate) fn launchd_plists(plan: &Plan, state: &State) -> Vec<(String, String)> {
    let plist = |label: &str, argv: Vec<String>, cwd: &str| launchd_plist(label, &argv, cwd);
    let engine = state
        .parts
        .get("engine")
        .map(|p| p.path.clone())
        .unwrap_or_else(|| "nils".to_string());
    let mut argv = vec![engine];
    argv.extend(engine_args(
        plan,
        &plan.registry().display().to_string(),
        &plan.dir.join("backups").display().to_string(),
    ));
    let mut out = vec![(
        "se.kineuro.nils-engine.plist".to_string(),
        plist(
            "se.kineuro.nils-engine",
            argv,
            &plan.dir.display().to_string(),
        ),
    )];
    if let Some(desk) = state.parts.get("desk").filter(|d| d.kind == "binary") {
        out.push((
            "se.kineuro.nils-desk.plist".to_string(),
            plist(
                "se.kineuro.nils-desk",
                vec![
                    desk.path.clone(),
                    "serve".to_string(),
                    "--config".to_string(),
                    plan.desk_config().display().to_string(),
                ],
                &plan.desk_dir().display().to_string(),
            ),
        ));
    }
    if let Some(part) = state.parts.get(LLAMA_PART) {
        out.push((
            "se.kineuro.nils-llama.plist".to_string(),
            plist(
                "se.kineuro.nils-llama",
                llama_argv(plan, Path::new(&part.path), "127.0.0.1"),
                &plan.runtime_dir().display().to_string(),
            ),
        ));
    }
    out
}

/// What to say when a desk that keeps its own people has nobody to let in
/// yet: the command that adds the first, since without one the desk opens on
/// a login nobody can pass.
fn nobody_yet(plan: &Plan) -> Option<String> {
    (plan.mode == Mode::Local && plan.has(Part::Desk) && !desk_has_people(&plan.dir)).then(|| {
        format!(
            "add the first person, who may do everything: nils-desk user add <name> --admin --config {}",
            plan.desk_config().display()
        )
    })
}

/// The last lines: what is there, where to open it, what to run. On a
/// terminal, a card.
fn summary(plan: &Plan, console: &Console, services: &[Service]) {
    if console.live {
        card_summary(plan, console, services);
        return;
    }
    let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    println!();
    println!("{}", console.bold("Installed"));
    println!("  everything under {}", plan.dir.display());
    if plan.has(Part::Desk) {
        println!("  the desk at {origin}");
    }
    println!("  the engine on port {}", plan.ports.engine);
    println!();
    if let Some(line) = nobody_yet(plan) {
        println!("{}", console.bold("Nobody can sign in yet"));
        println!("  {line}");
        println!();
    }
    if !plan.service && plan.runtime == Runtime::Machine {
        println!("{}", console.bold("Start it"));
        for command in start_commands(plan) {
            println!("  {command}");
        }
        if plan.has(Part::Assistant) {
            println!(
                "  {}",
                console.dim(
                    "once Kvasir runs, nils setup and repair adds its model and makes the \
                     assistant's key"
                )
            );
        }
        println!();
    }
    println!("{}", console.bold("Then"));
    println!("  bring data in with: nils digest <a directory of DICOM files>");
    println!("  the documentation is at https://kineuro.se/nils/docs/");
    println!("  the next version, when there is one: nils update --all");
}

/// The commands that start each part by hand, with the arguments its service
/// would have been given, so a port this setup moved and the trust a desk
/// that keeps its own people needs are in what a person copies, not only in
/// a unit.
fn start_commands(plan: &Plan) -> Vec<String> {
    let registry = plan.registry().display().to_string();
    let backups = plan.dir.join("backups").display().to_string();
    let mut out = vec![format!(
        "nils {}",
        engine_args(plan, &registry, &backups).join(" ")
    )];
    if plan.has(Part::Desk) {
        out.push(format!(
            "nils-desk serve --config {}",
            plan.desk_config().display()
        ));
    }
    if plan.has(Part::Assistant) {
        if let Some(server) = llama_built(plan) {
            let build = server.parent().map(Path::to_path_buf).unwrap_or_default();
            out.push(llama_argv(plan, &build, "127.0.0.1").join(" "));
        }
        out.push(format!(
            "(cd {} && node dist/main.js --config kvasir.json)",
            plan.dir.join("kvasir").display()
        ));
        let assistant = plan.dir.join("assistant");
        out.push(format!(
            "(cd {} && set -a && . ./assistant.env && node {})",
            assistant.display(),
            assistant_entry(&assistant)
        ));
    }
    out
}

/// The end on a terminal: the card, how to start what no service starts, and
/// what to run next.
fn card_summary(plan: &Plan, console: &Console, services: &[Service]) {
    let p = console.palette;
    let title = if services.is_empty() {
        "NILS is installed"
    } else if services.iter().all(|s| s.running) {
        "NILS is running"
    } else {
        "NILS is installed, not all of it runs"
    };
    // a long path or address is cut rather than wrapped through the frame
    let room = tui::width(1).saturating_sub(18).max(30);
    let rows: Vec<(&str, String)> = card_rows(plan)
        .into_iter()
        .map(|(key, value)| (key, tui::truncate(&value, room)))
        .collect();
    let shown: Vec<(String, bool)> = services
        .iter()
        .map(|s| (service_name(&s.unit), s.running))
        .collect();
    println!();
    for line in tui::card(p, title, &rows, &shown) {
        println!("{line}");
    }
    if let Some(line) = nobody_yet(plan) {
        println!();
        println!(" {}", p.bold("Nobody can sign in yet"));
        println!("   {line}");
    }
    if !plan.service && plan.runtime == Runtime::Machine {
        println!();
        println!(" {}", p.bold("Start it"));
        for command in start_commands(plan) {
            println!("   {command}");
        }
        if plan.has(Part::Assistant) {
            println!(
                "   {}",
                p.dim(
                    "once Kvasir runs, nils setup and repair adds its model and makes the \
                     assistant's key"
                )
            );
        }
    }
    println!();
    let next = [
        ("Next", "nils digest <dir>", "bring DICOM in"),
        ("Next", "nils update --all", "take the newest release"),
        ("Next", "nils uninstall", "remove it"),
        ("Docs", "https://kineuro.se/nils/docs/", ""),
    ];
    for line in tui::next_steps(p, &next) {
        println!("{line}");
    }
    println!();
}

/// The card's rows: where each part answers, where the registry is kept,
/// what the assistant talks to, and how it all runs.
fn card_rows(plan: &Plan) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if plan.has(Part::Desk) {
        let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
        rows.push(("desk", origin));
        rows.push(("sign in", plan.mode.words().to_string()));
    }
    rows.push((
        "engine",
        match plan.runtime {
            Runtime::Machine => format!("127.0.0.1:{}", plan.ports.engine),
            Runtime::Podman => format!("port {} inside the pod", plan.ports.engine),
            Runtime::Docker => format!("nils-engine:{} on the docker network", plan.ports.engine),
        },
    ));
    rows.push((
        "registry",
        match (&plan.backend, plan.postgres) {
            (BackendChoice::Sqlite, _) => format!("SQLite in {}", tilde(&plan.registry())),
            (BackendChoice::Postgres { .. }, Some(_)) => format!(
                "Postgres {POSTGRES_MAJOR} in {}",
                tilde(&plan.postgres_dir())
            ),
            (BackendChoice::Postgres { dsn, .. }, None) => {
                format!("Postgres at {}", crate::redact_dsn(dsn))
            }
        },
    ));
    if plan.has(Part::Assistant) {
        rows.push((
            "assistant",
            model_on_record(plan).unwrap_or_else(|| "no model named yet".to_string()),
        ));
    }
    if plan.has(Part::Assistant)
        && let Some(llama) = plan.llama
        && llama_built(plan).is_some()
    {
        rows.push((
            "models",
            format!(
                "llama.cpp {LLAMA_BUILD} ({}), started from Kvasir",
                llama_words(llama.variant)
            ),
        ));
    }
    rows.push((
        "runs",
        match (plan.service, plan.runtime) {
            (true, Runtime::Machine) if cfg!(target_os = "macos") => {
                "as launchd agents, back after a restart"
            }
            (true, Runtime::Machine) => "as systemd user units, back after a restart",
            (true, Runtime::Podman) => "in podman, back after a restart",
            (true, Runtime::Docker) => "in docker, back after a restart",
            (false, Runtime::Machine) => "started by hand",
            (false, Runtime::Podman) => "in podman, started by this setup",
            (false, Runtime::Docker) => "in docker, started by this setup",
        }
        .to_string(),
    ));
    rows.push(("directory", tilde(&plan.dir)));
    rows
}

/// The model the assistant asks for, as its environment names it: a model's
/// name, or the install's ChatGPT subscription.
fn model_on_record(plan: &Plan) -> Option<String> {
    model_on_record_in(&plan.dir)
}

/// The model the assistant of a setup directory asks for.
fn model_on_record_in(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("assistant").join("assistant.env")).ok()?;
    let named = text
        .lines()
        .find_map(|line| line.strip_prefix("ASSISTANT_MODEL="))?
        .trim();
    match named {
        "" => None,
        CHATGPT => Some(CHATGPT_WORDS.to_string()),
        model => Some(model.to_string()),
    }
}

/// A unit or a container, by the name of the part it runs.
fn service_name(unit: &str) -> String {
    match unit.strip_prefix("nils-").unwrap_or(unit) {
        "kvasir" => "Kvasir".to_string(),
        other => other.to_string(),
    }
}

/// A path as a person writes it, with `~` for the home directory.
fn tilde(path: &Path) -> String {
    match home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

// -------------------------------------------------------------- the update

/// Whether `nils setup` recorded an install for `nils update --all` to bring
/// up to date, said before anything is fetched.
pub(crate) fn setup_recorded() -> Result<(), Exit> {
    if read_state().is_some() {
        return Ok(());
    }
    Err(fail(format!(
        "no setup is recorded at {}; nils update takes the engine alone",
        state_path().display()
    )))
}

/// Every part the state file names, brought up to date in whatever way that
/// part runs: a binary is replaced, a container is a pull of the new tag, a
/// Node part is a fetch and a rebuild. One line each, and a slow step is one
/// line with a timer, as in the wizard. The engine's binary is not among
/// them: `nils update` replaces it first and hands the parts to the new
/// binary, so they move to the versions the newest release pins, and then
/// [`restart_after_update`] starts everything again from what is there.
/// The answer is whether any part changed.
pub(crate) fn update_all(channel: Option<&str>) -> Result<bool, Exit> {
    let mut state = read_state().ok_or_else(|| {
        fail(format!(
            "no setup is recorded at {}; nils update takes the engine alone",
            state_path().display()
        ))
    })?;
    let newest = update::newest_version(&update::engine_base(channel)).ok();
    let console = Console::new(true);
    let mut changed = false;

    let names: Vec<String> = state
        .parts
        .iter()
        // the engine's binary is replaced by nils update itself; its image is not
        .filter(|(name, part)| name.as_str() != "engine" || part.kind != "binary")
        // a new Postgres major version needs its data upgraded, not a pull
        .filter(|(name, part)| {
            if name.as_str() == "postgres" {
                println!("postgres: {} kept, as its data needs", part.version);
                false
            } else {
                true
            }
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in names {
        let part = state.parts[&name].clone();
        match part.kind.as_str() {
            "podman" | "docker" => {
                let Some(version) = newest.as_deref() else {
                    println!("{name}: no release to move to");
                    continue;
                };
                let image = part
                    .path
                    .rsplit_once(':')
                    .map_or(part.path.as_str(), |(i, _)| i);
                let tag = format!("{image}:{}", image_tag(version));
                if part.path == tag {
                    println!("{name}: {tag} is the newest");
                    continue;
                }
                let dir = PathBuf::from(&state.dir);
                if console
                    .task(&format!("taking {tag}"), &dir, &part.kind, &["pull", &tag])
                    .is_err()
                {
                    println!("{name}: {tag} could not be pulled");
                    continue;
                }
                state.parts.insert(
                    name.clone(),
                    PartState {
                        version: version.to_string(),
                        path: tag.clone(),
                        ..part
                    },
                );
                println!("{name}: {tag}");
                changed = true;
            }
            "node" => {
                let dir = PathBuf::from(&part.path);
                let Some((repo, reference, said)) = node_source(&name) else {
                    println!("{name}: not a part this can update");
                    continue;
                };
                let head = |dir: &Path| {
                    run_quiet(
                        "git",
                        &["-C", &dir.display().to_string(), "rev-parse", "HEAD"],
                    )
                };
                let before = head(&dir);
                // the release this version names, from a checkout of main too
                let fetched = source_steps(repo, &reference, &dir, true)
                    .iter()
                    .try_for_each(|step| {
                        let args: Vec<&str> = step.iter().map(String::as_str).collect();
                        console.task(&source_label(step, said, &reference), &dir, "git", &args)
                    });
                if let Err(e) = fetched {
                    println!("{name}: {}", e.message);
                    continue;
                }
                // Source that did not move is not built again; nils setup and
                // repair builds it regardless.
                if before.is_some() && head(&dir) == before && dir.join("dist").exists() {
                    println!("{name}: {reference} is the one built");
                    continue;
                }
                let built = console
                    .task(
                        &format!("installing {said}'s packages"),
                        &dir,
                        "npm",
                        &["ci", "--no-audit", "--no-fund", "--loglevel=error"],
                    )
                    .and_then(|()| {
                        console.task(&format!("building {said}"), &dir, "npm", &["run", "build"])
                    });
                match built {
                    Ok(()) => {
                        println!(
                            "{name}: {reference} fetched and rebuilt in {}",
                            dir.display()
                        );
                        changed = true;
                    }
                    Err(e) => println!("{name}: {}", e.message),
                }
            }
            // llama.cpp is taken again where this version pins another build
            LLAMA_PART => {
                let plan = plan_from_state(&state, channel);
                match fetch_llama(&plan) {
                    Ok((dir, taken)) => {
                        state.parts.insert(
                            name.clone(),
                            PartState {
                                version: LLAMA_BUILD.to_string(),
                                path: dir.display().to_string(),
                                kind: LLAMA_PART.to_string(),
                            },
                        );
                        mend_kvasir_runtime(&plan);
                        if taken || part.version != LLAMA_BUILD {
                            println!("{name}: {LLAMA_BUILD} in {}", dir.display());
                            changed = true;
                        } else {
                            println!("{name}: {LLAMA_BUILD} is the build this version takes");
                        }
                    }
                    Err(e) => println!("{name}: {e}"),
                }
            }
            _ => match update_binary_part(&name, &part, &update::desk_base(channel)) {
                Ok((version, said)) => {
                    println!("{name}: {said}");
                    changed |= version != part.version;
                    state
                        .parts
                        .insert(name.clone(), PartState { version, ..part });
                }
                Err(e) => println!("{name}: {}", e.message),
            },
        }
    }

    // an install from before llama.cpp takes it, where it has the assistant
    if state.parts.contains_key("assistant") && !state.parts.contains_key(LLAMA_PART) {
        let plan = plan_from_state(&state, channel);
        if plan.llama.is_some() {
            match fetch_llama(&plan) {
                Ok((dir, _)) => {
                    state.parts.insert(
                        LLAMA_PART.to_string(),
                        PartState {
                            version: LLAMA_BUILD.to_string(),
                            path: dir.display().to_string(),
                            kind: LLAMA_PART.to_string(),
                        },
                    );
                    mend_kvasir_runtime(&plan);
                    println!(
                        "{LLAMA_PART}: {LLAMA_BUILD} in {}, which runs the models Kvasir starts",
                        dir.display()
                    );
                    changed = true;
                }
                Err(e) => println!("{LLAMA_PART}: {e}"),
            }
        }
    }

    state.at = nils_registry::time::now_iso();
    write_state(&state)?;
    Ok(changed)
}

/// After an update the services run what is now installed. The units are
/// written again, since a container's names its image tag and the
/// assistant's names its entry, and everything is restarted in the order the
/// wizard starts it, with which of them are running said. An install made
/// with no services is left for the person to restart.
pub(crate) fn restart_after_update(channel: Option<&str>) {
    let Some(state) = read_state() else {
        return;
    };
    if state.service.is_empty() || state.service == "none" {
        println!("no services were written for this setup, so restart what you run yourself");
        return;
    }
    let mut plan = plan_from_state(&state, channel);
    if let Some(refused) = recorded_system_refusal(&plan) {
        println!("the services were left alone: {refused}");
        return;
    }
    if let Some(engine) = state.parts.get("engine") {
        plan.version = engine.version.clone();
    }
    // Kvasir trusts what the desk signs as the desk signs it now, before it
    // starts again (record 25)
    if plan.has(Part::Assistant) {
        mend_kvasir_auth(&plan);
    }
    let console = Console::new(true);
    println!("restarting the services");
    match start_everything(&plan, &state, &console, None) {
        Ok(started) => print!("{}", started.text),
        Err(e) => println!("the services were left alone: {}", e.message),
    }
    // and the supervisor, from the binary that is installed now
    start_supervisor(&plan, &state, &console);
}

// ------------------------------------------------------- for the supervisor

/// A part as its service manager knows it: the unit or the container, and
/// what watches it.
pub(crate) struct Unit {
    pub(crate) part: &'static str,
    pub(crate) name: String,
    /// `systemd`, `docker` or `launchd`.
    pub(crate) watcher: &'static str,
    /// Whether a systemd unit is the machine's own rather than the account's.
    pub(crate) system: bool,
}

/// The unit each part of a recorded install runs under, in the order the
/// parts start: Postgres, the engine, the desk, Kvasir, the assistant. Kvasir's
/// part is named `gateway` there, as the desk asks for it.
/// An install that runs no services has none.
pub(crate) fn service_units(state: &State) -> Vec<Unit> {
    if state.service.is_empty() || state.service == "none" {
        return Vec::new();
    }
    let runtime = Runtime::parse(&state.runtime).unwrap_or(Runtime::Machine);
    let system = state.system.is_some();
    let mut out = Vec::new();
    let mut push = |part: &'static str, name: &str, watcher: &'static str| {
        out.push(Unit {
            part,
            name: name.to_string(),
            watcher,
            system,
        });
    };
    match state.parts.get("postgres").map(|p| p.kind.as_str()) {
        Some("podman") => push("postgres", "nils-postgres", "systemd"),
        Some("docker") => push("postgres", "nils-postgres", "docker"),
        _ => {}
    }
    let has = |name: &str| state.parts.contains_key(name);
    let desk_binary = state.parts.get("desk").is_some_and(|d| d.kind == "binary");
    match (runtime, cfg!(target_os = "macos")) {
        (Runtime::Docker, _) => {
            push("engine", "nils-engine", "docker");
            if has("desk") {
                push("desk", "nils-desk", "docker");
            }
            if has(LLAMA_PART) {
                push(LLAMA_PART, "nils-llama", "systemd");
            }
            if has("kvasir") {
                push("gateway", "nils-kvasir", "docker");
            }
            if has("assistant") {
                push("assistant", "nils-assistant", "docker");
            }
        }
        (Runtime::Podman, _) => {
            push("engine", "nils-engine", "systemd");
            if has("desk") {
                push("desk", "nils-desk", "systemd");
            }
            if has(LLAMA_PART) {
                push(LLAMA_PART, "nils-llama", "systemd");
            }
            if has("kvasir") {
                push("gateway", "nils-kvasir", "systemd");
            }
            if has("assistant") {
                push("assistant", "nils-assistant", "systemd");
            }
        }
        (Runtime::Machine, true) => {
            push("engine", "se.kineuro.nils-engine", "launchd");
            if desk_binary {
                push("desk", "se.kineuro.nils-desk", "launchd");
            }
            if has(LLAMA_PART) {
                push(LLAMA_PART, "se.kineuro.nils-llama", "launchd");
            }
        }
        (Runtime::Machine, false) => {
            push("engine", "nils-engine", "systemd");
            if desk_binary {
                push("desk", "nils-desk", "systemd");
            }
            if has(LLAMA_PART) {
                push(LLAMA_PART, "nils-llama", "systemd");
            }
            if has("kvasir") {
                push("gateway", "kvasir", "systemd");
            }
            if has("assistant") {
                push("assistant", "nils-assistant", "systemd");
            }
        }
    }
    out
}

/// Whether a unit runs now: one look, for a page rather than a start.
pub(crate) fn unit_running(unit: &Unit) -> bool {
    match unit.watcher {
        "docker" => run_quiet(
            "docker",
            &["inspect", "-f", "{{.State.Running}}", &unit.name],
        )
        .is_some_and(|s| s.trim() == "true"),
        "launchd" => {
            run_quiet("launchctl", &["list", &unit.name]).is_some_and(|s| s.contains("\"PID\""))
        }
        _ => systemctl_says(unit.system, &["is-active", &unit.name])
            .is_some_and(|s| s.trim() == "active"),
    }
}

/// Where each part of a recorded install answers, and who can reach it there.
pub(crate) fn addresses(state: &State) -> Vec<serde_json::Value> {
    let runtime = Runtime::parse(&state.runtime).unwrap_or(Runtime::Machine);
    let reach = recorded_reach(state);
    let ports = state.ports;
    let inside = match runtime {
        Runtime::Docker => "inside docker only",
        Runtime::Podman => "inside the pod only",
        Runtime::Machine => "this machine only",
    };
    let named = |container: &str, port: u16| match runtime {
        Runtime::Docker => format!("{container}:{port}"),
        _ => format!("127.0.0.1:{port}"),
    };
    let mut out = Vec::new();
    if state.parts.contains_key("desk") {
        let (_, origin, _) = desk_binding(&reach, ports.desk, runtime.container());
        let who = match reach {
            Reach::Loopback => "this machine only",
            Reach::Network(_) => "this network",
            Reach::Behind { .. } => "whoever reaches that address",
        };
        out.push(serde_json::json!({ "part": "desk", "address": origin, "reach": who }));
    }
    out.push(
        serde_json::json!({ "part": "engine", "address": named("nils-engine", ports.engine), "reach": inside }),
    );
    if state.parts.contains_key("kvasir") {
        out.push(serde_json::json!({ "part": "gateway", "address": format!("127.0.0.1:{}", ports.kvasir), "reach": "this machine only" }));
    }
    if state.parts.contains_key(LLAMA_PART) {
        let (host, reach) = match runtime {
            Runtime::Docker => (docker_bridge(), "this machine and its docker containers"),
            _ => ("127.0.0.1".to_string(), "this machine only"),
        };
        out.push(serde_json::json!({ "part": LLAMA_PART, "address": format!("{host}:{}", ports.llama), "reach": reach }));
    }
    if state.parts.contains_key("assistant") {
        out.push(serde_json::json!({ "part": "assistant", "address": named("nils-assistant", ports.assistant), "reach": inside }));
    }
    if state.parts.contains_key("postgres") {
        out.push(serde_json::json!({ "part": "postgres", "address": format!("127.0.0.1:{}", ports.postgres), "reach": "this machine only" }));
    }
    out
}

/// The install as the supervisor reports it: the setup record without its
/// secrets, the parts, where each answers, and each service with whether it
/// runs.
pub(crate) fn install_doc(state: &State) -> serde_json::Value {
    let parts: serde_json::Map<String, serde_json::Value> = state
        .parts
        .iter()
        .map(|(name, p)| {
            (
                name.clone(),
                serde_json::json!({ "version": p.version, "kind": p.kind, "path": p.path }),
            )
        })
        .collect();
    let services: Vec<serde_json::Value> = service_units(state)
        .iter()
        .map(|u| {
            serde_json::json!({ "part": u.part, "unit": u.name, "watcher": u.watcher, "running": unit_running(u) })
        })
        .collect();
    serde_json::json!({
        "record": state_path().display().to_string(),
        "dir": state.dir,
        "runtime": state.runtime,
        "service": state.service,
        "reach": state.reach,
        "backend": state.backend,
        "mode": state.mode,
        "at": state.at,
        "ports": state.ports,
        "parts": parts,
        "places": state.places,
        "oidc": state.oidc.as_ref().map(|o| serde_json::json!({ "issuer": o.issuer, "client_id": o.client_id })),
        "addresses": addresses(state),
        "services": services,
        "unfinished": state.unfinished,
    })
}

/// Restart one part of a recorded install, or every part in the order they
/// start. A digest that was running resumes when the engine is back.
pub(crate) fn restart_units(state: &State, part: Option<&str>) -> Result<Vec<String>, Exit> {
    let units = service_units(state);
    if units.is_empty() {
        return Err(fail(
            "this install runs no services, so restart what you run yourself",
        ));
    }
    let chosen: Vec<&Unit> = match part {
        None | Some("all") => units.iter().collect(),
        Some(name) => {
            let found: Vec<&Unit> = units.iter().filter(|u| u.part == name).collect();
            if found.is_empty() {
                let runs: Vec<&str> = units.iter().map(|u| u.part).collect();
                return Err(usage(format!(
                    "{name} is not a part this install runs; it runs {}",
                    runs.join(", ")
                )));
            }
            found
        }
    };
    let uid = run_quiet("id", &["-u"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut done: Vec<String> = Vec::new();
    for unit in chosen {
        let ok = match unit.watcher {
            "docker" => quietly("docker", &["restart", &unit.name]),
            "launchd" => quietly(
                "launchctl",
                &["kickstart", "-k", &format!("gui/{uid}/{}", unit.name)],
            ),
            _ => systemctl(unit.system, &["restart", &unit.name]),
        };
        if !ok {
            return Err(fail(format!(
                "{} did not restart; before it, {}",
                unit.name,
                if done.is_empty() {
                    "nothing was restarted".to_string()
                } else {
                    format!("{} restarted", done.join(", "))
                }
            )));
        }
        println!("restarted {}", unit.name);
        done.push(unit.name.clone());
    }
    Ok(done)
}

/// The engine made to follow the registry: its unit or container written
/// again from the record, with every source place mounted, and only the
/// engine started again, so the desk, Kvasir and the assistant keep
/// running. A container sees only what was mounted when it started, so a
/// folder added as a source needs this before the engine can read it.
pub(crate) fn reapply_engine(state: &State) -> Result<(), Exit> {
    if state.service.is_empty() || state.service == "none" {
        return Err(fail(
            "this install runs no services; start the engine again yourself, with the folders it reads",
        ));
    }
    let mut plan = plan_from_state(state, None);
    if let Some(engine) = state.parts.get("engine") {
        plan.version = engine.version.clone();
    }
    match (plan.runtime, cfg!(target_os = "macos")) {
        (Runtime::Podman, _) => {
            let dir = quadlet_dir();
            let (name, unit) = quadlets(&plan)
                .into_iter()
                .find(|(n, _)| n == "nils-engine.container")
                .ok_or_else(|| fail("no quadlet names the engine"))?;
            std::fs::write(dir.join(&name), unit)
                .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            quietly("systemctl", &["--user", "daemon-reload"]);
            if !quietly("systemctl", &["--user", "restart", "nils-engine"]) {
                return Err(fail(
                    "the engine's container did not start again; its log: journalctl --user -u nils-engine",
                ));
            }
        }
        (Runtime::Docker, _) => {
            let path = plan.dir.join("compose.yaml");
            std::fs::write(&path, docker_compose(&plan))
                .map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let recreated = Command::new("docker")
                .args(["compose", "up", "-d", "--force-recreate", "engine"])
                .current_dir(&plan.dir)
                .status()
                .is_ok_and(|s| s.success());
            if !recreated {
                return Err(fail(
                    "the engine's container was not made again; its log: docker logs nils-engine",
                ));
            }
        }
        (Runtime::Machine, true) => {
            let dir = std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join("Library").join("LaunchAgents"))
                .ok_or_else(|| fail("no home directory"))?;
            let (name, agent) = launchd_plists(&plan, state)
                .into_iter()
                .find(|(n, _)| n == "se.kineuro.nils-engine.plist")
                .ok_or_else(|| fail("no agent names the engine"))?;
            let path = dir.join(name);
            let _ = Command::new("launchctl").arg("unload").arg(&path).output();
            std::fs::write(&path, agent).map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let _ = Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&path)
                .output();
        }
        (Runtime::Machine, false) => {
            let system = plan.system.is_some();
            let dir = units_dir(system);
            let (name, unit) = systemd_units(&plan, state)
                .into_iter()
                .find(|(n, _)| n == "nils-engine.service")
                .ok_or_else(|| fail("no unit names the engine"))?;
            std::fs::write(dir.join(&name), unit)
                .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            systemctl(system, &["daemon-reload"]);
            if !systemctl(system, &["restart", "nils-engine"]) {
                return Err(fail(format!(
                    "the engine did not start again; {}",
                    journal_line(system, "nils-engine")
                )));
            }
        }
    }
    println!("the engine reads the registry's places and was started again");
    Ok(())
}

/// Where the supervisor listens: this machine's loopback, or for docker the
/// bridge's own address, since a container reaches the host there and a
/// server on the loopback alone does not answer it.
fn supervisor_bind(plan: &Plan) -> String {
    let host = match plan.runtime {
        Runtime::Docker => docker_bridge(),
        _ => "127.0.0.1".to_string(),
    };
    format!("{host}:{}", plan.ports.supervisor)
}

/// The address of docker's own bridge on this host.
fn docker_bridge() -> String {
    run_quiet(
        "docker",
        &[
            "network",
            "inspect",
            "bridge",
            "-f",
            "{{(index .IPAM.Config 0).Gateway}}",
        ],
    )
    .map(|s| s.trim().to_string())
    .filter(|s| s.parse::<std::net::IpAddr>().is_ok())
    .unwrap_or_else(|| "172.17.0.1".to_string())
}

/// How the desk reaches the supervisor from where the desk runs; none from
/// a pod that is not given this host's loopback.
fn supervisor_url(plan: &Plan) -> Option<String> {
    let port = plan.ports.supervisor;
    match plan.runtime {
        Runtime::Machine => Some(format!("http://127.0.0.1:{port}")),
        Runtime::Podman => plan
            .host_loopback
            .then(|| format!("http://{HOST_LOOPBACK_IN_POD}:{port}")),
        Runtime::Docker => Some(format!("http://host.docker.internal:{port}")),
    }
}

fn supervisor_config(plan: &Plan) -> PathBuf {
    plan.dir.join("supervise").join("supervise.toml")
}

/// The token the desk shows the supervisor, as the supervisor's file holds it.
fn supervisor_token(plan: &Plan) -> Option<String> {
    let text = std::fs::read_to_string(supervisor_config(plan)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("tokens")?.as_table()?.keys().next().cloned()
}

/// The supervisor's file: where it listens, the one token it answers, which
/// is the desk's, and where its log and its runs go. A token made before is
/// kept, so a desk configured with it still reaches the supervisor.
fn write_supervisor(plan: &Plan) -> Result<String, Exit> {
    let path = supervisor_config(plan);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let token = supervisor_token(plan).unwrap_or_else(generated_passphrase);
    let trust = dir.join("trust.pub");
    if !trust.exists() {
        std::fs::write(&trust, "").map_err(|e| fail(format!("{}: {e}", trust.display())))?;
    }
    let text = format!(
        "# Written by nils setup. The supervisor reports this install to the desk's\n\
         # settings and restarts its parts; it answers the one token below, the desk's.\n\
         bind = \"{}\"\n\
         trust = \"trust.pub\"\n\
         log = \"supervise.log\"\n\n\
         [tokens]\n\
         \"{token}\" = \"nils-desk\"\n",
        supervisor_bind(plan)
    );
    write_secret_bytes(&path, text.as_bytes())?;
    Ok(token)
}

/// The binary a supervisor runs from: the engine's own on the machine, and
/// otherwise the nils that made the container install.
fn supervisor_binary(state: &State) -> String {
    state
        .parts
        .get("engine")
        .filter(|p| p.kind == "binary")
        .map(|p| p.path.clone())
        .or_else(|| {
            state
                .programs
                .iter()
                .find(|p| Path::new(p).file_name().is_some_and(|n| n == "nils"))
                .cloned()
        })
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "nils".to_string())
}

/// The supervisor as a service of its own, on this host and outside every
/// container, since it restarts them. Where the services are this machine's
/// it stays root's: it restarts them and replaces the binaries they run, and
/// an account that could do that is an account that runs the install.
fn supervisor_service(plan: &Plan, state: &State) -> (String, String) {
    let nils = supervisor_binary(state);
    let config = supervisor_config(plan).display().to_string();
    if cfg!(target_os = "macos") {
        let dir = plan.dir.join("supervise").display().to_string();
        return (
            "se.kineuro.nils-supervise.plist".to_string(),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\
                 \x20 <key>Label</key><string>se.kineuro.nils-supervise</string>\n\
                 \x20 <key>ProgramArguments</key>\n  <array>\n    <string>{nils}</string>\n    <string>supervise</string>\n    <string>run</string>\n    <string>--config</string>\n    <string>{config}</string>\n  </array>\n\
                 \x20 <key>WorkingDirectory</key><string>{dir}</string>\n\
                 \x20 <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n\
                 </dict>\n</plist>\n"
            ),
        );
    }
    (
        "nils-supervise.service".to_string(),
        format!(
            "[Unit]\nDescription=NILS supervisor\nAfter=network-online.target\n\n[Service]\n\
             ExecStart={nils} supervise run --config {config}\nRestart=on-failure\nRestartSec=5\n\
             # a run it started, an update among them, outlives a restart of the supervisor\n\
             KillMode=process\n\n[Install]\nWantedBy={}\n",
            wanted_by(plan.system.is_some())
        ),
    )
}

/// The supervisor written and started: its file, then its service, or the
/// command that runs it where this setup writes no services.
fn start_supervisor(plan: &Plan, state: &State, console: &Console) {
    if let Err(e) = write_supervisor(plan) {
        console.warn(&format!("the supervisor was not set up: {}", e.message));
        return;
    }
    if !plan.service {
        console.say(&format!(
            "run the supervisor, which the desk's settings read: {} supervise run --config {}",
            supervisor_binary(state),
            supervisor_config(plan).display()
        ));
        return;
    }
    let (name, unit) = supervisor_service(plan, state);
    if cfg!(target_os = "macos") {
        let Some(dir) =
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("LaunchAgents"))
        else {
            return;
        };
        let path = dir.join(&name);
        let _ = Command::new("launchctl").arg("unload").arg(&path).output();
        if std::fs::write(&path, unit).is_ok() {
            let _ = Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&path)
                .output();
        }
        return;
    }
    let system = plan.system.is_some();
    let dir = units_dir(system);
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(&name), unit))
    {
        console.warn(&format!("the supervisor's unit was not written: {e}"));
        return;
    }
    systemctl(system, &["daemon-reload"]);
    systemctl(system, &["enable", "nils-supervise"]);
    if systemctl(system, &["restart", "nils-supervise"]) {
        console.progress(&format!("the supervisor on {}", supervisor_bind(plan)));
    } else {
        console.warn(&format!(
            "the supervisor did not start; {}",
            journal_line(system, "nils-supervise")
        ));
    }
}

/// One binary part from its own releases, when a newer one is published.
fn update_binary_part(name: &str, part: &PartState, base: &str) -> Result<(String, String), Exit> {
    if name != "desk" {
        return Err(fail(format!("{name} is not a part this can update")));
    }
    let version = update::newest_version(base)?;
    let path = &part.path;
    if !update::newer(&version, &part.version) && Path::new(path).exists() {
        return Ok((
            part.version.clone(),
            format!("{} is the newest", part.version),
        ));
    }
    let file = update::part_file("nils-desk", &update::host_target());
    let bytes = update::fetch_checked(base, &version, &file)?;
    update::install_binary(Path::new(path), &bytes)?;
    Ok((version.clone(), format!("{version} at {path}")))
}

// ----------------------------------------------------------- the uninstall

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// Remove NILS and keep the data: the registry and its key, the backups,
    /// the desk's people and the assistant's history stay where they are
    #[arg(long, conflicts_with = "purge")]
    keep_data: bool,
    /// Remove NILS and every file it made, the registry's key included
    #[arg(long)]
    purge: bool,
    /// Go ahead without asking; with --purge this also skips typing the name
    #[arg(long, short = 'y')]
    yes: bool,
    /// Say what would be removed and what kept, and change nothing
    #[arg(long)]
    print: bool,
}

/// The first-party packs a release carries. A pack directory holding only
/// these was put there by an install; any other pack is a person's own and
/// is never removed.
const FIRST_PARTY_PACKS: [&str; 2] = ["mri", "clinical"];

/// Everything an uninstall would touch, gathered before anything is.
struct Removal {
    dir: PathBuf,
    runtime: String,
    /// Whether the units are the machine's own rather than this account's.
    system: bool,
    /// Unit names to stop and disable, and the files that define them.
    units: Vec<String>,
    unit_files: Vec<PathBuf>,
    /// Container names and image references, for podman or docker.
    containers: Vec<String>,
    images: Vec<String>,
    /// Programs other than this one, then this one, removed last.
    programs: Vec<PathBuf>,
    me: Option<PathBuf>,
    /// This program, when the record does not name it and so it stays.
    me_kept: Option<PathBuf>,
    /// First-party packs to remove, and a pack directory kept because it
    /// also holds a person's own.
    packs: Vec<PathBuf>,
    packs_kept: Option<PathBuf>,
    /// What building Kvasir and the assistant made.
    built: Vec<PathBuf>,
    /// Kvasir's directory, where the data stays: the models it holds and
    /// their keys, its subscriptions, its seal key and pepper, and the
    /// assistant's key go with NILS. Where everything goes, it goes with the
    /// base directory.
    kvasir: Option<PathBuf>,
    /// llama.cpp's build, which goes with NILS where the data stays; where
    /// everything goes, it goes with the base directory.
    llama: Option<PathBuf>,
    state: PathBuf,
    /// The runtime of a Postgres this setup runs, whose container goes; its
    /// data goes with the base directory, or stays with it.
    postgres: Option<String>,
}

/// What an uninstall takes: NILS alone, or NILS and its data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Leaving {
    KeepData,
    Purge,
}

pub(crate) fn uninstall(args: UninstallArgs) -> Result<(), Exit> {
    let mut console = Console::new(args.yes);
    println!("{}", console.bold("NILS uninstall"));
    let Some(state) = read_state() else {
        return remove_leftovers(&args, &mut console);
    };
    println!("  {}", describe_state(&state));

    let leaving = if args.purge {
        Leaving::Purge
    } else if args.keep_data || !console.interactive() {
        Leaving::KeepData
    } else {
        let dir = state.dir.clone();
        match console.choice(
            "What should go?",
            &[
                (
                    "NILS, keeping your data",
                    &format!(
                        "the services, the programs, the packs and Kvasir's models and keys; the \
                         registry and its key, the backups, the desk's people and the assistant's \
                         history stay in {dir}"
                    ),
                ),
                (
                    "Everything",
                    &format!(
                        "all of that and {dir} itself; the registry's key cannot be recovered"
                    ),
                ),
            ],
            0,
        ) {
            0 => Leaving::KeepData,
            _ => Leaving::Purge,
        }
    };

    let me = std::env::current_exe()
        .ok()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
    let removal = gather_removal(&state, me, leaving);

    if leaving == Leaving::Purge
        && let Err(why) = safe_to_purge(&removal.dir, home_dir().as_deref())
    {
        return Err(fail(format!(
            "{} will not be removed: {why}; run nils uninstall --keep-data, and remove \
                 what is left by hand",
            removal.dir.display()
        )));
    }

    println!();
    print!("{}", removal_text(&removal, leaving, &console));
    if args.print {
        println!();
        println!("nothing was changed");
        return Ok(());
    }

    let go = match leaving {
        Leaving::KeepData => args.yes || (console.interactive() && console.yes_no("Do it?", false)),
        Leaving::Purge if args.yes => true,
        Leaving::Purge => {
            if !console.interactive() {
                false
            } else {
                console.note(
                    "this cannot be undone: without the registry's key the same subject can never \
                     be given the same code again",
                );
                let name = removal
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let typed = console.line(&format!("Type {name} to remove everything"), "");
                typed.trim() == name && !name.is_empty()
            }
        }
    };
    if !go {
        println!("  nothing was changed");
        if !console.interactive() {
            println!("  add --yes to go ahead");
        }
        return Ok(());
    }

    println!();
    carry_out(&removal, leaving, &console);
    println!();
    println!("{}", console.bold("Removed"));
    match leaving {
        Leaving::KeepData => {
            println!(
                "  NILS is gone from this machine; your data is still in {}",
                removal.dir.display()
            );
            println!("  to install again and pick it up:");
            println!(
                "    curl -fsSL https://nils.kineuro.se/get | sh -s -- --dir {}",
                removal.dir.display()
            );
        }
        Leaving::Purge => println!("  NILS and everything it made are gone from this machine"),
    }
    Ok(())
}

/// With no record, what a setup that stopped before it wrote one leaves in
/// the places a setup uses: this program when it is `nils`, the `nils-desk`
/// beside it, the first-party packs beside it, and the base directory only
/// when it holds nothing but the empty directories setup makes. Anything
/// holding data is left, since without a record nothing says it is ours.
pub(crate) fn leftovers(me: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(me) = me.filter(|m| m.file_name().is_some_and(|n| n == "nils")) {
        if let Some(bin) = me.parent() {
            let desk = bin.join("nils-desk");
            if desk.is_file() {
                out.push(desk);
            }
            if let Some(prefix) = bin.parent() {
                let packs = prefix.join("share").join("nils").join("packs");
                for pack in FIRST_PARTY_PACKS {
                    if packs.join(pack).is_dir() {
                        out.push(packs.join(pack));
                    }
                }
            }
        }
        out.push(me.to_path_buf());
    }
    if let Some(home) = home {
        let dir = home.join("nils");
        if only_empty_setup_dirs(&dir) {
            out.insert(0, dir);
        }
    }
    out
}

fn remove_leftovers(args: &UninstallArgs, console: &mut Console) -> Result<(), Exit> {
    let me = std::env::current_exe()
        .ok()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
    let found = leftovers(me.as_deref(), home_dir().as_deref());
    println!("  no setup is recorded at {}", state_path().display());
    if found.is_empty() {
        println!("  and nothing a setup leaves is in the usual places, so nothing was changed");
        return Ok(());
    }
    println!();
    println!("{}", console.bold("What an unfinished setup left"));
    for path in &found {
        println!("  {}", path.display());
    }
    if args.print {
        println!();
        println!("nothing was changed");
        return Ok(());
    }
    let go = args.yes || (console.interactive() && console.yes_no("Remove these?", false));
    if !go {
        println!("  nothing was changed");
        if !console.interactive() {
            println!("  add --yes to go ahead");
        }
        return Ok(());
    }
    println!();
    for path in &found {
        match remove_path(path, "") {
            Ok(()) => println!("  removed {}", path.display()),
            Err(e) => println!("  {} was not removed: {e}", path.display()),
        }
        // the packs directory and its parent, when nothing else is in them
        if let Some(packs) = path.parent().filter(|p| p.ends_with("share/nils/packs")) {
            let _ = std::fs::remove_dir(packs);
            if let Some(share) = packs.parent() {
                let _ = std::fs::remove_dir(share);
            }
        }
    }
    println!();
    println!("{}", console.bold("Removed"));
    Ok(())
}

/// The setup on one line, the same words the wizard opens with.
fn describe_state(state: &State) -> String {
    let parts: Vec<String> = state
        .parts
        .iter()
        .map(|(name, part)| match part.kind.as_str() {
            "node" => format!("{name} from source"),
            _ => format!("{name} {}", part.version),
        })
        .collect();
    format!(
        "{}{} in {}, {} mode, {}",
        if state.unfinished {
            "an install that did not finish: "
        } else {
            ""
        },
        parts.join(", "),
        state.dir,
        state.mode,
        match state.runtime.as_str() {
            "podman" => "in podman containers",
            "docker" => "in docker containers",
            _ => "on the machine",
        }
    )
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Whether a directory may be removed whole. It must be absolute, must not
/// be the root, the home directory or anything above it, and must look like
/// what an install made. A state file edited by hand, or a --dir given as
/// the home directory, should cost a refusal and not a home directory.
fn safe_to_purge(dir: &Path, home: Option<&Path>) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err("it is not an absolute path".to_string());
    }
    if dir.parent().is_none() {
        return Err("it is the root of the file system".to_string());
    }
    if let Some(home) = home
        && home.starts_with(dir)
    {
        return Err("it is the home directory, or holds it".to_string());
    }
    let made_here = dir.join("registry").join("nils.toml").exists()
        || dir.join("desk").join("nils-desk.toml").exists()
        || only_empty_setup_dirs(dir);
    if !made_here {
        return Err("it holds neither a registry nor a desk that an install made".to_string());
    }
    Ok(())
}

/// The directories setup makes under a base directory, and nothing else,
/// every one of them empty: what an install that stopped early leaves, and
/// nothing a person could lose.
fn only_empty_setup_dirs(dir: &Path) -> bool {
    const MADE: [&str; 8] = [
        "registry",
        "desk",
        "backups",
        "working",
        "export",
        "assistant",
        "kvasir",
        "postgres",
    ];
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut any = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let empty = std::fs::read_dir(entry.path()).is_ok_and(|mut e| e.next().is_none());
        if !MADE.contains(&name.as_str()) || !entry.path().is_dir() || !empty {
            return false;
        }
        any = true;
    }
    any
}

/// Gather what an uninstall would remove, looking at what is really there.
fn gather_removal(state: &State, me: Option<PathBuf>, leaving: Leaving) -> Removal {
    let dir = PathBuf::from(&state.dir);
    let mut removal = Removal {
        dir: dir.clone(),
        runtime: state.runtime.clone(),
        system: state.system.is_some(),
        units: Vec::new(),
        unit_files: Vec::new(),
        containers: Vec::new(),
        images: Vec::new(),
        programs: Vec::new(),
        me: None,
        me_kept: None,
        packs: Vec::new(),
        packs_kept: None,
        built: Vec::new(),
        kvasir: None,
        llama: None,
        state: state_path(),
        postgres: None,
    };

    // a Postgres this setup runs, wherever the parts run
    if let Some(pg) = state
        .parts
        .get("postgres")
        .filter(|p| p.kind == "podman" || p.kind == "docker")
    {
        if pg.kind == "podman" {
            let path = quadlet_dir().join("nils-postgres.container");
            if path.exists() {
                removal.units.push("nils-postgres".to_string());
                removal.unit_files.push(path);
            }
        }
        removal.postgres = Some(pg.kind.clone());
    }

    // services
    match state.runtime.as_str() {
        "podman" => {
            for (unit, file) in [
                ("nils-assistant", "nils-assistant.container"),
                ("nils-kvasir", "nils-kvasir.container"),
                ("nils-desk", "nils-desk.container"),
                ("nils-engine", "nils-engine.container"),
                ("nils-pod", "nils.pod"),
            ] {
                let path = quadlet_dir().join(file);
                if path.exists() {
                    removal.units.push(unit.to_string());
                    removal.unit_files.push(path);
                }
            }
            removal.containers = vec!["nils".to_string()];
        }
        "docker" => {
            // Named containers only. A compose project named for the
            // directory can share its name with another project on the same
            // machine, and bringing that down would take the other with it.
            removal.containers = ["nils-assistant", "nils-kvasir", "nils-desk", "nils-engine"]
                .iter()
                .filter(|name| run_quiet("docker", &["inspect", "-f", "{{.Name}}", name]).is_some())
                .map(|name| (*name).to_string())
                .collect();
        }
        _ if cfg!(target_os = "macos") => {
            if let Some(home) = home_dir() {
                for file in ["se.kineuro.nils-engine.plist", "se.kineuro.nils-desk.plist"] {
                    let path = home.join("Library").join("LaunchAgents").join(file);
                    if path.exists() {
                        removal.unit_files.push(path);
                    }
                }
            }
        }
        _ => {
            for unit in ["nils-assistant", "kvasir", "nils-desk", "nils-engine"] {
                let path = units_dir(removal.system).join(format!("{unit}.service"));
                if path.exists() {
                    removal.units.push(unit.to_string());
                    removal.unit_files.push(path);
                }
            }
        }
    }
    // the supervisor and llama.cpp run on the host, whichever runtime the parts use
    if cfg!(target_os = "macos") {
        if let Some(home) = home_dir() {
            for file in [
                "se.kineuro.nils-supervise.plist",
                "se.kineuro.nils-llama.plist",
            ] {
                let path = home.join("Library").join("LaunchAgents").join(file);
                if path.exists() {
                    removal.unit_files.push(path);
                }
            }
        }
    } else {
        for unit in ["nils-supervise", "nils-llama"] {
            let path = units_dir(removal.system).join(format!("{unit}.service"));
            if path.exists() {
                removal.units.push(unit.to_string());
                removal.unit_files.push(path);
            }
        }
    }
    if matches!(state.runtime.as_str(), "podman" | "docker") {
        let engine = state.runtime.as_str();
        let listed = run_quiet(engine, &["images", "--format", "{{.Repository}}:{{.Tag}}"])
            .unwrap_or_default();
        removal.images = listed
            .lines()
            .map(str::trim)
            .filter(|image| {
                image.starts_with(&format!("{ENGINE_IMAGE}:"))
                    || image.starts_with(&format!("{DESK_IMAGE}:"))
            })
            .map(str::to_string)
            .collect();
    }

    // Programs, this one last. Only what the record names: a nils-desk beside
    // this program, or this program run from somewhere else, may be someone
    // else's build.
    let recorded: Vec<PathBuf> = state
        .parts
        .values()
        .filter(|part| part.kind == "binary")
        .map(|part| part.path.as_str())
        .chain(state.programs.iter().map(String::as_str))
        .map(|path| {
            let path = PathBuf::from(path);
            std::fs::canonicalize(&path).unwrap_or(path)
        })
        .collect();
    for path in &recorded {
        if path.exists() && Some(path) != me.as_ref() && !removal.programs.contains(path) {
            removal.programs.push(path.clone());
        }
    }
    match me {
        Some(me) if recorded.contains(&me) => removal.me = Some(me),
        Some(me) => removal.me_kept = Some(me),
        None => {}
    }

    // packs, beside the nils this install put there, never a person's own
    let installed_nils = recorded
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == "nils"));
    if let Some(prefix) = installed_nils
        .and_then(|p| p.parent())
        .and_then(Path::parent)
    {
        let packs = prefix.join("share").join("nils").join("packs");
        if let Ok(entries) = std::fs::read_dir(&packs) {
            let mut others = false;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if FIRST_PARTY_PACKS.contains(&name.as_str()) {
                    removal.packs.push(entry.path());
                } else {
                    others = true;
                }
            }
            if others {
                removal.packs_kept = Some(packs);
            }
        }
    }

    // what building Kvasir and the assistant made; the rest of the
    // assistant's directory holds its data and its configuration
    for (name, part) in state.parts.iter().filter(|(_, p)| p.kind == "node") {
        let path = PathBuf::from(&part.path);
        let outside = !path.starts_with(&dir);
        if leaving == Leaving::Purge && outside {
            removal.built.push(path);
            continue;
        }
        if leaving == Leaving::KeepData && name != "kvasir" {
            for sub in ["node_modules", "dist"] {
                if path.join(sub).exists() {
                    removal.built.push(path.join(sub));
                }
            }
        }
    }
    // Kvasir's state goes with NILS where the data stays, the whole of its
    // directory, which holds nothing else a person keeps: the models it holds
    // and their keys, its subscriptions, its seal key and pepper, and the
    // assistant's key.
    if leaving == Leaving::KeepData {
        let kvasir = state
            .parts
            .get("kvasir")
            .map_or_else(|| dir.join("kvasir"), |part| PathBuf::from(&part.path));
        if kvasir.exists() {
            removal.kvasir = Some(kvasir);
        }
        let llama = dir.join(LLAMA_PART);
        if llama.exists() {
            removal.llama = Some(llama);
        }
    }
    removal
}

/// What will go and what will stay, in the plan's own form.
fn removal_text(removal: &Removal, leaving: Leaving, console: &Console) -> String {
    let mut out = String::new();
    let row = |out: &mut String, key: &str, value: String| {
        let _ = writeln!(out, "  {} {value}", console.dim(&format!("{key:<12}")));
    };
    let _ = writeln!(out, "{}", console.bold("Removing"));
    if !removal.units.is_empty() {
        row(&mut out, "services", removal.units.join(", "));
    } else if !removal.unit_files.is_empty() {
        row(
            &mut out,
            "services",
            removal
                .unit_files
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if removal.postgres.is_some() {
        row(
            &mut out,
            "postgres",
            format!("the {POSTGRES_CONTAINER} container"),
        );
    }
    if !removal.containers.is_empty() {
        let what = if removal.runtime == "podman" {
            format!("the {} pod", removal.containers.join(", "))
        } else {
            removal.containers.join(", ")
        };
        row(&mut out, "containers", what);
    }
    if !removal.images.is_empty() {
        row(&mut out, "images", removal.images.join(", "));
    }
    let mut programs: Vec<String> = removal
        .programs
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    if let Some(me) = &removal.me {
        programs.push(me.display().to_string());
    }
    if !programs.is_empty() {
        row(&mut out, "programs", programs.join(", "));
    }
    if !removal.packs.is_empty() {
        row(
            &mut out,
            "packs",
            removal
                .packs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !removal.built.is_empty() {
        row(
            &mut out,
            "built parts",
            removal
                .built
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(kvasir) = &removal.kvasir {
        row(
            &mut out,
            "kvasir",
            format!(
                "{}, with the models it holds, their keys, its subscriptions and the assistant's \
                 key",
                kvasir.display()
            ),
        );
    }
    if let Some(llama) = &removal.llama {
        row(
            &mut out,
            LLAMA_PART,
            format!(
                "{}, the build that ran the models Kvasir started",
                llama.display()
            ),
        );
    }
    row(
        &mut out,
        "setup record",
        removal.state.display().to_string(),
    );
    if leaving == Leaving::Purge {
        row(
            &mut out,
            "data",
            format!("{} and everything in it", removal.dir.display()),
        );
        for line in data_summary(&removal.dir) {
            let _ = writeln!(out, "  {:<12} {}", "", console.dim(&line));
        }
    }
    let _ = writeln!(out, "\n{}", console.bold("Keeping"));
    match leaving {
        Leaving::KeepData => {
            row(&mut out, "data", removal.dir.display().to_string());
            for line in data_summary(&removal.dir) {
                let _ = writeln!(out, "  {:<12} {}", "", console.dim(&line));
            }
        }
        Leaving::Purge => row(&mut out, "nothing", "that this setup made".to_string()),
    }
    if let Some(kept) = &removal.packs_kept {
        row(
            &mut out,
            "packs",
            format!("{}, which also holds packs of your own", kept.display()),
        );
    }
    if let Some(me) = &removal.me_kept {
        row(
            &mut out,
            "program",
            format!(
                "{}, which the setup record does not name; remove it yourself if nothing \
                 else put it there",
                me.display()
            ),
        );
    }
    out
}

/// A few lines saying what a data directory holds, without walking a
/// working directory that may hold a whole archive.
fn data_summary(dir: &Path) -> Vec<String> {
    let size_of = |path: &Path| -> u64 {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    };
    let count = |path: &Path| std::fs::read_dir(path).map(|e| e.count()).unwrap_or(0);
    let mut out = Vec::new();
    let registry = dir.join("registry");
    if registry.join("nils.toml").exists() {
        out.push(format!(
            "the registry, {}, with its key",
            human_size(size_of(&registry))
        ));
    }
    let backups = count(&dir.join("backups"));
    if backups > 0 {
        out.push(format!("{backups} backup file(s)"));
    }
    if dir.join("desk").join("nils-desk.sqlite").exists() {
        out.push("the desk's database, with the people it keeps".to_string());
    }
    if dir.join("postgres.env").exists() {
        out.push("Postgres's data, with its password".to_string());
    }
    if dir.join("assistant").join("assistant.sqlite").exists() {
        out.push("the assistant's conversations".to_string());
    }
    for sub in ["working", "export"] {
        let n = count(&dir.join(sub));
        if n > 0 {
            out.push(format!("{n} item(s) in {sub}"));
        }
    }
    out
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Do what the removal says, in the order that leaves nothing holding a file
/// open: services, containers, built parts, packs, programs, the record, the
/// data, and this program last. Every step says what it did; a step that
/// fails says so and the rest still run.
fn carry_out(removal: &Removal, leaving: Leaving, console: &Console) {
    let say = |text: String| println!("  {text}");

    // the units on this machine, the supervisor and llama.cpp among them on a
    // docker install too, whose containers go below
    if !removal.units.is_empty() {
        let mut args = vec!["disable", "--now"];
        args.extend(removal.units.iter().map(String::as_str));
        systemctl(removal.system, &args);
    }
    if cfg!(target_os = "macos") {
        for file in &removal.unit_files {
            let _ = Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(file)
                .output();
        }
    }
    for file in &removal.unit_files {
        let _ = std::fs::remove_file(file);
    }
    if !removal.unit_files.is_empty() && !cfg!(target_os = "macos") {
        systemctl(removal.system, &["daemon-reload"]);
        let mut args = vec!["reset-failed"];
        args.extend(removal.units.iter().map(String::as_str));
        systemctl(removal.system, &args);
        say(format!(
            "stopped and removed {}",
            if removal.units.is_empty() {
                "the services".to_string()
            } else {
                removal.units.join(", ")
            }
        ));
    }

    if let Some(rt) = removal.postgres.as_deref()
        && quietly(rt, &["rm", "-f", POSTGRES_CONTAINER])
    {
        say(format!("removed the {POSTGRES_CONTAINER} container"));
    }
    match removal.runtime.as_str() {
        "podman" => {
            if quietly("podman", &["pod", "rm", "-f", "nils"]) {
                say("removed the nils pod".to_string());
            }
        }
        "docker" => {
            if !removal.containers.is_empty() {
                let mut args = vec!["rm", "-f"];
                args.extend(removal.containers.iter().map(String::as_str));
                quietly("docker", &args);
                say(format!("removed {}", removal.containers.join(", ")));
            }
            // the network docker run made, and the one compose made for the
            // directory; docker refuses either while anything still uses it
            quietly("docker", &["network", "rm", "nils"]);
            if let Some(project) = removal.dir.file_name() {
                let project: String = project
                    .to_string_lossy()
                    .to_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                quietly("docker", &["network", "rm", &format!("{project}_default")]);
            }
            if leaving == Leaving::KeepData {
                let _ = std::fs::remove_file(removal.dir.join("compose.yaml"));
            }
        }
        _ => {}
    }
    if !removal.images.is_empty() {
        let engine = removal.runtime.as_str();
        let mut args = vec!["rmi", "-f"];
        args.extend(removal.images.iter().map(String::as_str));
        if quietly(engine, &args) {
            say(format!("removed {}", removal.images.join(", ")));
        }
    }

    for path in &removal.built {
        match remove_path(path, &removal.runtime) {
            Ok(()) => say(format!("removed {}", path.display())),
            Err(e) => say(format!("{} was not removed: {e}", path.display())),
        }
    }
    if let Some(kvasir) = &removal.kvasir {
        match remove_path(kvasir, &removal.runtime) {
            Ok(()) => say(format!(
                "removed {}, with the models Kvasir held and their keys",
                kvasir.display()
            )),
            Err(e) => say(format!("{} was not removed: {e}", kvasir.display())),
        }
    }
    if let Some(llama) = &removal.llama {
        match remove_path(llama, &removal.runtime) {
            Ok(()) => say(format!("removed {}", llama.display())),
            Err(e) => say(format!("{} was not removed: {e}", llama.display())),
        }
    }
    for path in &removal.packs {
        if remove_path(path, &removal.runtime).is_ok() {
            say(format!(
                "removed the {} pack",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }
    if removal.packs_kept.is_none()
        && let Some(packs) = removal.packs.first().and_then(|p| p.parent())
    {
        let _ = std::fs::remove_dir(packs);
        if let Some(share) = packs.parent() {
            let _ = std::fs::remove_dir(share);
        }
    }
    for path in &removal.programs {
        match std::fs::remove_file(path) {
            Ok(()) => say(format!("removed {}", path.display())),
            Err(e) => say(format!("{} was not removed: {e}", path.display())),
        }
    }
    if std::fs::remove_file(&removal.state).is_ok() {
        say(format!("removed {}", removal.state.display()));
        // the directory the record lived in, when nothing else lives there
        if let Some(parent) = removal.state.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    if leaving == Leaving::Purge {
        // data a podman Postgres wrote belongs to its user namespace
        let owner = match removal.postgres.as_deref() {
            Some("podman") => "podman",
            _ => removal.runtime.as_str(),
        };
        match remove_path(&removal.dir, owner) {
            Ok(()) => say(format!(
                "removed {} and everything in it",
                removal.dir.display()
            )),
            Err(e) => say(format!("{} was not removed: {e}", removal.dir.display())),
        }
    }
    if let Some(me) = &removal.me {
        match std::fs::remove_file(me) {
            Ok(()) => say(format!("removed {}", me.display())),
            Err(e) => say(format!("{} was not removed: {e}", me.display())),
        }
    }
    let _ = console;
}

/// A file or a directory, gone. A rootless podman container owns what it
/// wrote to its mounts, so where this account may not remove a directory,
/// podman removes it from inside the same user namespace.
fn remove_path(path: &Path, runtime: &str) -> Result<(), String> {
    let first = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match first {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && runtime == "podman" => {
            let ok = Command::new("podman")
                .args(["unshare", "rm", "-rf"])
                .arg(path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if ok { Ok(()) } else { Err(e.to_string()) }
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_person_is_held_to_the_desks_rules_where_it_is_asked() {
        // exactly what nils-desk's users::add refuses, which the installer asks for again
        assert_eq!(desk_username_refusal("admin"), None);
        assert_eq!(desk_username_refusal("nima.ch-2_x"), None);
        for bad in ["", "two words", "someone@ki", "åsa", "a/b"] {
            assert_eq!(
                desk_username_refusal(bad),
                Some("a username is letters, digits, dots, dashes and underscores"),
                "{bad:?}"
            );
        }
        assert_eq!(desk_password_refusal("12345678"), None);
        assert_eq!(
            desk_password_refusal("  spaced "),
            None,
            "spaces are part of a password"
        );
        for short in ["", "1234567", "short"] {
            assert_eq!(
                desk_password_refusal(short),
                Some("a password is at least eight characters"),
                "{short:?}"
            );
        }
        // counted in bytes, as the desk counts them: four two-byte letters make eight
        assert_eq!(desk_password_refusal("åäöü"), None);
    }

    #[test]
    fn a_part_left_unusable_stops_an_install_and_is_only_said_on_an_update() {
        let console = Console::new(true);
        assert!(
            console.broken("the desk was not installed").is_ok(),
            "an update or a repair says it and goes on"
        );
        console.strict.set(true);
        let stopped = console.broken("the desk was not installed").unwrap_err();
        assert_eq!(stopped.message, "the desk was not installed");
        assert_eq!(stopped.code, fail("x").code);
    }

    fn plan(runtime: Runtime) -> Plan {
        Plan {
            dir: PathBuf::from("/home/x/nils"),
            parts: vec![Part::Engine, Part::Desk],
            mode: Mode::Off,
            runtime,
            backend: BackendChoice::Sqlite,
            ports: Ports::default(),
            reach: Reach::Loopback,
            source: Some(PathBuf::from("/data/source")),
            sources: Vec::new(),
            registry_exists: false,
            service: true,
            channel: None,
            version: "1.0.0-alpha.2".to_string(),
            host_loopback: false,
            postgres: None,
            oidc: None,
            llama: None,
            system: None,
        }
    }

    /// A directory of its own under the system's temporary directory.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nils-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn station(dir: &Path, name: &str, purpose: &str, content: &str) {
        let at = dir.join("assistant").join("stations").join(name);
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(
            at.join("station.yml"),
            format!("id: {name}\npurpose: {purpose}\ncontent: {content}\n"),
        )
        .unwrap();
    }

    /// What the screens came to, and every screen they drew.
    type Screens<T> = (Result<(T, Vec<String>), Exit>, Vec<Vec<String>>);

    /// The screens with no terminal: the keys from a list, every screen kept,
    /// drawn plain and at one size.
    fn on_screens<T>(
        questions: impl FnMut(&mut Console) -> Result<T, Stop>,
        keys: Vec<tui::Key>,
    ) -> Screens<T> {
        let mut console = Console::new(false);
        console.palette = tui::Palette::Plain;
        let mut keys = keys.into_iter();
        let mut drawn = Vec::new();
        let outcome = console.drive(
            questions,
            &mut || {
                keys.next()
                    .map_or_else(|| vec![tui::Key::Interrupt], |key| vec![key])
            },
            &mut |screen| drawn.push(screen),
            &|| (100, 40),
        );
        (outcome, drawn)
    }

    #[test]
    fn the_screens_take_an_answer_back_and_offer_it_again() {
        use tui::Key::{Backspace, Char, Down, Enter, Escape};
        let mut dialled = 0;
        let (outcome, drawn) = on_screens(
            |console| {
                console.step(1);
                let pick =
                    console.ask_choice("Which?", &[("one", ""), ("two", ""), ("three", "")], 0)?;
                console.said(["one", "two", "three"][pick]);
                console.step(2);
                let name = console.ask_line("A name", "admin")?;
                let long = console.probe("the name's length", || {
                    dialled += 1;
                    name.len()
                });
                console.note(&format!("{long} long"));
                let sure = console.ask_yes_no("Sure?", true)?;
                Ok((pick, name, sure))
            },
            vec![
                Down,
                Enter, // two
                Char('x'),
                Enter,  // adminx
                Escape, // back to the name, with adminx on offer
                Backspace,
                Enter, // admin
                Down,
                Enter, // no
            ],
        );
        let ((pick, name, sure), summaries) = match outcome {
            Ok(done) => done,
            Err(e) => panic!("{}", e.message),
        };
        assert_eq!((pick, name.as_str(), sure), (1, "admin", false));
        assert_eq!(summaries, vec!["two".to_string()]);
        assert_eq!(
            dialled, 2,
            "probed once for each name given, not once for each run"
        );
        let last = drawn.last().map(|s| s.join("\n")).unwrap_or_default();
        assert!(
            last.contains("✓ What to install") && last.contains("two"),
            "{last}"
        );
        assert!(last.contains("▸ Where it runs"), "{last}");
        assert!(
            last.contains("✓ A name  admin") && last.contains("5 long"),
            "{last}"
        );
        assert!(last.contains("Sure?") && last.contains("← back"), "{last}");
    }

    #[test]
    fn ctrl_c_on_a_screen_stops_the_setup() {
        let (outcome, _) = on_screens(
            |console| console.ask_line("A name", ""),
            vec![tui::Key::Char('a'), tui::Key::Interrupt],
        );
        assert_eq!(outcome.err().map(|e| e.code), Some(crate::STOPPED));
    }

    #[derive(clap::Parser)]
    struct Wizard {
        #[command(flatten)]
        setup: SetupArgs,
    }

    #[test]
    fn the_wizard_is_answered_on_screens_with_an_earlier_step_changed() {
        use clap::Parser as _;
        use tui::Key::{Char, Enter, Escape, Up};
        let dir = scratch("screens");
        let args =
            Wizard::parse_from(["nils", "--dir", &dir.display().to_string(), "--no-service"]).setup;
        let facts = Facts {
            podman: false,
            docker: Err(DockerAbsent::NotInstalled),
            cards: Vec::new(),
            card: None,
            llama: None,
        };
        let (outcome, drawn) = on_screens(
            |console| questions(console, &args, None, &facts, false),
            vec![
                Enter, // the engine and the desk
                Enter, // no directory of DICOM
                Escape,
                Escape, // back past it to the parts
                Up,
                Enter, // the engine only
                Enter, // still no directory of DICOM
                Enter, // SQLite
                Char('p'),
                Enter, // a passphrase
                Char('p'),
                Enter, // and again
                Enter, // nobody signs in
                Enter, // no assistant
                Enter, // do it
            ],
        );
        let (flow, summaries) = match outcome {
            Ok(done) => done,
            Err(e) => panic!("{}", e.message),
        };
        let Flow::Install(install) = flow else {
            panic!("the questions did not end in an install");
        };
        let (plan, answers) = *install;
        assert!(
            plan.parts == vec![Part::Engine],
            "the parts as changed on the way back"
        );
        assert_eq!(plan.dir, dir);
        assert!(matches!(plan.backend, BackendChoice::Sqlite));
        assert!(plan.mode == Mode::Off && !plan.service);
        assert_eq!(answers.passphrase.as_deref(), Some("p"));
        assert_eq!(summaries.first().map(String::as_str), Some("engine"));
        let last = drawn.last().map(|s| s.join("\n")).unwrap_or_default();
        assert!(
            last.contains("Do it?") && last.contains("directory"),
            "{last}"
        );
        assert!(
            drawn.iter().all(|screen| screen.len() <= 40),
            "every screen fits its terminal"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_station_s_purpose_is_declared_to_the_gateway() {
        // Kvasir refuses a purpose it was not told of. The example declared
        // three; the assistant's stations use more, so on a fresh install
        // three of them would have answered nothing but a refusal.
        let dir = scratch("purposes");
        station(&dir, "ask-help", "assistant.ask-help", "rows");
        station(&dir, "concierge", "assistant.concierge", "rows");
        station(&dir, "operator", "assistant.operator", "catalog");
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        let declared = serde_json::json!([
            {"id": "assistant.ask-help", "app": "nils-assistant", "content": "rows", "kind": "foreground"},
            {"id": "assistant.title", "app": "nils-assistant", "content": "catalog", "kind": "background"},
        ]);
        let all = assistant_purposes(&plan, &declared);
        let ids: Vec<&str> = all.iter().filter_map(|p| p["id"].as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "assistant.ask-help",
                "assistant.title",
                "assistant.concierge",
                "assistant.operator"
            ],
            "declared ones kept first, each station added once"
        );
        let operator = all
            .iter()
            .find(|p| p["id"] == "assistant.operator")
            .unwrap();
        assert_eq!(
            operator["content"], "catalog",
            "the station's own class is kept"
        );

        // A fresh kvasir.json declares them and names no model, even from an
        // example that still names backends and the OAuth of Kvasir's first
        // design, which Kvasir refuses to start with.
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        std::fs::write(
            kvasir.join("kvasir.example.json"),
            serde_json::json!({
                "bind": "127.0.0.1:7100",
                "origin": "http://127.0.0.1:7100",
                "store": "kvasir.sqlite",
                "purposes": declared.clone(),
                "backends": [{"id": "card", "baseUrl": "http://127.0.0.1:30000/v1", "locality": "local"}],
                "oauth": {"openai": {}},
            })
            .to_string(),
        )
        .unwrap();
        plan.ports.kvasir = 7101;
        let chosen = ModelChoice {
            url: "http://127.0.0.1:30000/v1".to_string(),
            local: true,
            key: Some("sk-local".to_string()),
            model: "qwen".to_string(),
            later: false,
            chatgpt: false,
        };
        let mut console = Console::new(true);
        assert!(configure_kvasir(&plan, &mut console, Some(&chosen)).is_ok());
        let written = kvasir_config(&plan).unwrap();
        assert!(
            written.get("backends").is_none() && written.get("oauth").is_none(),
            "{written}"
        );
        assert_eq!(written["bind"], "127.0.0.1:7101");
        assert_eq!(
            written["store"], "kvasir.sqlite",
            "the example's other settings stay"
        );
        let ids: Vec<&str> = written["purposes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        assert!(
            ids.contains(&"assistant.concierge") && ids.contains(&"assistant.operator"),
            "{ids:?}"
        );
        assert!(kvasir_admin_token(&plan).is_some());
        // the model chosen is kept, with its key, for Kvasir to add once it runs
        let kept = to_add(&plan);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0]["id"], MODEL_BACKEND);
        assert_eq!(kept[0]["baseUrl"], "http://127.0.0.1:30000/v1");
        assert_eq!(kept[0]["locality"], "local");
        assert_eq!(kept[0]["models"], serde_json::json!(["qwen"]));
        assert_eq!(kept[0]["key"], "sk-local");
        assert!(
            !kvasir.join("model.key").exists(),
            "no key file of the model's own"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for file in [kvasir.join("kvasir.json"), to_add_path(&plan)] {
                assert_eq!(
                    std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{}",
                    file.display()
                );
            }
        }
        // in a container, Kvasir dials this machine's loopback the way a
        // container reaches it
        plan.runtime = Runtime::Podman;
        assert_eq!(
            model_door_body(&plan, &chosen)["baseUrl"],
            "http://host.containers.internal:30000/v1"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_mends_what_the_first_wizard_wrote_and_nothing_else() {
        // What an install from before left: backends in kvasir.json, which
        // Kvasir no longer starts with, among them a key file that is not on
        // this machine and the model's key in a file of setup's own; the
        // OAuth of Kvasir's first design; purposes missing for stations.
        let dir = scratch("repair");
        station(&dir, "concierge", "assistant.concierge", "rows");
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        std::fs::write(kvasir.join("model.key"), "sk-model\n").unwrap();
        let written = serde_json::json!({
            "bind": "127.0.0.1:7100",
            "auth": {"mode": "token", "tokens": {"tok": "nils-setup:admin"}},
            "oauth": {"openai": {"clientId": "x"}},
            "backends": [
                {"id": "card", "baseUrl": "http://127.0.0.1:30000/v1", "locality": "local",
                 "keyFile": "/etc/kvasir/card.key", "compat": null, "inlineReasoning": null,
                 "models": [{"id": "m", "contextWindow": 65536}]},
                {"id": "model", "baseUrl": "https://api.example.org/v1", "locality": "remote",
                 "keyFile": kvasir.join("model.key").display().to_string(), "provider": "model",
                 "models": [{"id": "big"}]},
                {"id": "kept", "baseUrl": "https://api.example.org/v1", "locality": "remote",
                 "key": "a key someone put there", "models": [{"id": "y"}]}
            ],
            "purposes": [{"id": "assistant.ask-help", "app": "nils-assistant", "content": "rows", "kind": "foreground"}]
        });
        std::fs::write(kvasir.join("kvasir.json"), written.to_string()).unwrap();
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        let mut console = Console::new(true);
        assert!(repair_kvasir(&plan, &mut console).is_ok());

        let mended = kvasir_config(&plan).unwrap();
        assert!(
            mended.get("backends").is_none(),
            "Kvasir refuses a file that names them: {mended}"
        );
        assert!(mended.get("oauth").is_none(), "{mended}");
        let purposes: Vec<&str> = mended["purposes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        assert!(purposes.contains(&"assistant.concierge"), "{purposes:?}");
        assert_eq!(kvasir_admin_token(&plan).as_deref(), Some("tok"));
        assert_eq!(mended["auth"]["mode"], "off", "as the desk signs in");
        assert_eq!(mended["auth"]["tokens"]["tok"], INSTALLER);

        // every backend is kept for Kvasir to add again, with its key
        let kept = to_add(&plan);
        let ids: Vec<&str> = kept.iter().filter_map(|b| b["id"].as_str()).collect();
        assert_eq!(
            ids,
            vec!["card", "model", "kept"],
            "nothing a person set up is lost"
        );
        assert!(
            kept[0].get("key").is_none() && kept[0].get("keyFile").is_none(),
            "a key file that is not there is dropped: {}",
            kept[0]
        );
        assert!(
            kept[0].get("compat").is_none() && kept[0].get("inlineReasoning").is_none(),
            "an empty field Kvasir's door refuses is left out: {}",
            kept[0]
        );
        assert_eq!(
            kept[0]["models"][0]["contextWindow"], 65536,
            "what a model's entry held is kept"
        );
        assert_eq!(kept[1]["key"], "sk-model", "the key its key file held");
        assert!(kept[1].get("provider").is_none(), "{}", kept[1]);
        assert_eq!(kept[2]["key"], "a key someone put there");
        assert!(
            kept.iter().all(|b| b.get("replaces").is_none()),
            "a backend from before takes no other's place"
        );
        assert!(
            !kvasir.join("model.key").exists(),
            "the key file setup wrote goes once its key is kept"
        );

        // and a second repair has nothing left to do
        let before = std::fs::read_to_string(kvasir.join("kvasir.json")).unwrap();
        assert!(repair_kvasir(&plan, &mut console).is_ok());
        assert_eq!(
            std::fs::read_to_string(kvasir.join("kvasir.json")).unwrap(),
            before
        );
        assert_eq!(to_add(&plan).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_has_kvasir_trust_what_the_desk_signs() {
        // An install that signs people in through a provider, from before
        // the desk signed for them: Kvasir trusts the provider alone.
        let dir = scratch("mend-auth");
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        let mut plan = plan(Runtime::Podman);
        plan.dir = dir.clone();
        plan.mode = Mode::Oidc;
        plan.oidc = registered_at(
            "the desk's [oidc] table:\n  issuer = \"https://auth.example.org/application/o/nils/\"\n  client_id = \"abc123\"\n  client_secret_file = \"/x/client-secret\"\n",
        );
        let before = serde_json::json!({
            "bind": "0.0.0.0:7100",
            "auth": {
                "mode": "oidc",
                "tokens": {"tok": INSTALLER},
                "trust": [{"issuer": "https://auth.example.org/application/o/nils/", "audience": "abc123",
                           "jwks": "https://auth.example.org/application/o/nils/jwks/"}],
                "groupsClaim": "roles",
                "roles": {"reader": "reader", "reviewer": "reviewer", "operator": "operator", "admin": "admin"}
            }
        });
        std::fs::write(kvasir.join("kvasir.json"), before.to_string()).unwrap();
        mend_kvasir_auth(&plan);
        let after = kvasir_config(&plan).unwrap();
        let (issuer, jwks) = desk_trust(&plan);
        assert_eq!(
            after["auth"]["trust"][0], before["auth"]["trust"][0],
            "{after}"
        );
        assert_eq!(
            after["auth"]["trust"][1]["issuer"],
            issuer.as_str(),
            "{after}"
        );
        assert_eq!(after["auth"]["trust"][1]["jwks"], jwks.as_str(), "{after}");
        assert_eq!(
            after["auth"]["tokens"]["tok"], INSTALLER,
            "the installer's token is kept"
        );
        assert_eq!(after["bind"], "0.0.0.0:7100", "nothing else is touched");
        // and the next update has nothing left to change
        assert!(kvasir_auth_now(&plan, &after["auth"]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_gateway_knows_callers_the_way_the_desk_signs_them_in() {
        let mut plan = plan(Runtime::Podman);
        plan.mode = Mode::Local;
        let auth = kvasir_auth(&plan, "tok");
        assert_eq!(auth["mode"], "oidc", "{auth}");
        assert_eq!(auth["tokens"]["tok"], INSTALLER, "{auth}");
        let (issuer, jwks) = desk_trust(&plan);
        assert_eq!(jwks, "http://127.0.0.1:7200/.well-known/jwks.json");
        assert_eq!(auth["trust"][0]["issuer"], issuer.as_str(), "{auth}");
        assert_eq!(auth["trust"][0]["jwks"], jwks.as_str(), "{auth}");
        assert_eq!(auth["trust"][0]["audience"], "nils", "{auth}");
        assert_eq!(auth["groupsClaim"], "roles", "{auth}");
        assert_eq!(auth["roles"]["admin"], "admin", "{auth}");
        assert_eq!(auth["roles"]["assist"], "assist", "{auth}");
        assert_eq!(
            auth["trust"][0]["keepSubject"], true,
            "the desk's subjects are its own: {auth}"
        );
        let engine = engine_args(&plan, "/r", "/b").join(" ");
        assert!(
            engine.contains(&format!("issuer={issuer},audience=nils,jwks={jwks}")),
            "the engine trusts the same issuer: {engine}"
        );
        plan.runtime = Runtime::Docker;
        assert_eq!(
            desk_trust(&plan).1,
            "http://nils-desk:7200/.well-known/jwks.json"
        );
        plan.mode = Mode::Off;
        assert_eq!(kvasir_auth(&plan, "tok")["mode"], "off");
    }

    #[test]
    fn a_rerun_that_chooses_another_model_sends_the_assistant_there() {
        let dir = scratch("rechoose");
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        let written = serde_json::json!({
            "auth": {"mode": "off", "tokens": {"tok": "nils-setup:admin"}},
            "backends": [{"id": "model", "kind": "openai-completions",
                          "baseUrl": "http://127.0.0.1:30000/v1", "locality": "local",
                          "models": [{"id": "qwen", "name": "qwen", "contextWindow": 32768}]}],
            "purposes": []
        });
        std::fs::write(kvasir.join("kvasir.json"), written.to_string()).unwrap();
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        let chosen = ModelChoice {
            url: "https://api.example.org/v1".to_string(),
            local: false,
            key: Some("sk-test".to_string()),
            model: "big".to_string(),
            later: false,
            chatgpt: false,
        };
        let mut console = Console::new(true);
        assert!(configure_kvasir(&plan, &mut console, Some(&chosen)).is_ok());
        // the model chosen takes the place of the one kvasir.json named
        let kept = to_add(&plan);
        assert_eq!(kept.len(), 1, "{kept:?}");
        let backend = &kept[0];
        assert_eq!(backend["id"], "model", "{backend}");
        assert_eq!(
            backend["baseUrl"], "https://api.example.org/v1",
            "{backend}"
        );
        assert_eq!(backend["locality"], "remote", "{backend}");
        assert_eq!(backend["models"], serde_json::json!(["big"]), "{backend}");
        assert_eq!(backend["key"], "sk-test", "{backend}");
        assert_eq!(backend["replaces"], true, "{backend}");
        assert!(kvasir_config(&plan).unwrap().get("backends").is_none());

        // the assistant asks for the model chosen, and teaches on the backend
        // setup adds for it
        let env = dir.join("assistant");
        std::fs::create_dir_all(&env).unwrap();
        let env = env.join("assistant.env");
        std::fs::write(
            &env,
            "ASSISTANT_MODEL=qwen\nASSISTANT_TEACHING_BACKEND=card0\n",
        )
        .unwrap();
        assert!(write_assistant_env(&plan, Some(&chosen)).is_ok());
        let text = std::fs::read_to_string(&env).unwrap();
        assert!(text.contains("ASSISTANT_MODEL=big"), "{text}");
        assert!(text.contains("ASSISTANT_TEACHING_BACKEND=model"), "{text}");
        assert_eq!(model_on_record(&plan).as_deref(), Some("big"));
        // and a run that chose nothing leaves what the file names
        std::fs::write(
            &env,
            "ASSISTANT_MODEL=mine\nASSISTANT_TEACHING_BACKEND=card\n",
        )
        .unwrap();
        assert!(write_assistant_env(&plan, None).is_ok());
        let text = std::fs::read_to_string(&env).unwrap();
        assert!(
            text.contains("ASSISTANT_MODEL=mine")
                && text.contains("ASSISTANT_TEACHING_BACKEND=card"),
            "{text}"
        );

        // the install's ChatGPT subscription is named as Kvasir names it, and
        // said on the card as what it is
        assert!(write_assistant_env(&plan, Some(&ModelChoice::chatgpt())).is_ok());
        let text = std::fs::read_to_string(&env).unwrap();
        assert!(text.contains("ASSISTANT_MODEL=chatgpt"), "{text}");
        assert!(
            text.contains("ASSISTANT_TEACHING_BACKEND=card"),
            "ChatGPT teaches nothing: {text}"
        );
        assert_eq!(
            model_on_record(&plan).as_deref(),
            Some("ChatGPT subscription")
        );
        // and the model chosen before, which Kvasir does not hold yet, is let go
        assert!(configure_kvasir(&plan, &mut console, Some(&ModelChoice::chatgpt())).is_ok());
        assert!(to_add(&plan).is_empty(), "{:?}", to_add(&plan));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Kvasir that answers each door from a table, with a status and a
    /// body, and writes down every call it is sent with its body.
    fn fake_gateway(
        answer: impl Fn(&str, &str, &str) -> (u16, String) + Send + 'static,
    ) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{BufRead as _, Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let mut reader = std::io::BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let mut words = first.split_whitespace();
                let method = words.next().unwrap_or_default().to_string();
                let path = words.next().unwrap_or_default().to_string();
                let body = String::from_utf8_lossy(&body).to_string();
                seen.lock().unwrap().push(format!("{method} {path} {body}"));
                let (status, text) = answer(&method, &path, &body);
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    204 => "No Content",
                    404 => "Not Found",
                    409 => "Conflict",
                    422 => "Unprocessable Entity",
                    _ => "Answered",
                };
                let _ = reader.into_inner().write_all(
                    format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (port, calls)
    }

    /// What a fake Kvasir holds between calls: its backends, the models it
    /// lists, its policy table, its admission records, and the states its
    /// ChatGPT subscription goes through, one each time it is asked, the last
    /// one staying.
    #[derive(Default)]
    struct Held {
        backends: Vec<serde_json::Value>,
        listed: Vec<String>,
        purposes: serde_json::Value,
        admission: Vec<serde_json::Value>,
        signing_in: Vec<&'static str>,
    }

    /// Kvasir's doors over what it holds. A backend is added unless one of
    /// its models is named `silent`, which does not answer, and a local one is
    /// listed at once, as Kvasir admits it on its own.
    fn fake_kvasir(
        held: std::sync::Arc<std::sync::Mutex<Held>>,
    ) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use serde_json::json;
        fake_gateway(move |method, path, body| {
            let mut held = held.lock().unwrap();
            let sent: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let ok = |value: serde_json::Value| (200, value.to_string());
            match (method, path) {
                ("GET", "/healthz") => ok(json!({"ok": true})),
                ("GET", "/v1/backends") => ok(json!({ "backends": held.backends })),
                ("POST", "/v1/backends") => {
                    let id = sent["id"].as_str().unwrap_or("model").to_string();
                    let models = model_ids(&sent["models"]);
                    if models.iter().any(|m| m == "silent") {
                        return (
                            422,
                            json!({"error": {"code": "backend", "message": "silent did not answer",
                                "models": [{"id": "silent", "answered": false,
                                    "error": {"kind": "unreachable", "message": "fetch failed"}}]}})
                            .to_string(),
                        );
                    }
                    if held.backends.iter().any(|b| b["id"] == id.as_str()) {
                        return (
                            409,
                            json!({"error": {"code": "backend", "message": format!("a backend named {id} is held already")}})
                                .to_string(),
                        );
                    }
                    if sent["locality"] == "local" {
                        held.listed.extend(models.iter().cloned());
                    }
                    held.backends.push(json!({
                        "id": id, "locality": sent["locality"], "base_url": sent["baseUrl"],
                        "models": models, "credential": sent.get("key").is_some(), "builtin": false,
                        "health": {"warming": false, "lastError": null},
                    }));
                    (201, json!({"backend": {"id": id}}).to_string())
                }
                ("DELETE", p) if p.starts_with("/v1/backends/") => {
                    let id = &p["/v1/backends/".len()..];
                    held.backends.retain(|b| b["id"] != id);
                    (204, String::new())
                }
                ("GET", "/v1/config") => {
                    let models: Vec<serde_json::Value> =
                        held.listed.iter().map(|m| json!({ "id": m })).collect();
                    ok(json!({ "models": models }))
                }
                ("GET", p) if p.starts_with("/v1/admission") => {
                    ok(json!({ "records": held.admission }))
                }
                ("POST", "/v1/admission/run") => ok(json!({"records": [
                    {"model": sent["model"], "passed": true, "checks": []}
                ]})),
                ("GET", "/v1/keys") => ok(json!({"keys": [
                    {"id": "k_old", "principal": "nils-assistant", "purposes": ["assistant.ask-help"],
                     "expiresAt": null, "revokedAt": null},
                    {"id": "k_new", "principal": "nils-assistant",
                     "purposes": ["assistant.ask-help", "assistant.operator"],
                     "expiresAt": null, "revokedAt": null}
                ]})),
                ("POST", "/v1/keys") => ok(json!({"id": "k_new", "key": "kvs_k_new.fresh"})),
                ("DELETE", p) if p.starts_with("/v1/keys/") => (204, String::new()),
                ("GET", "/v1/purposes") => ok(json!({ "purposes": held.purposes })),
                ("PUT", p) if p.ends_with("/policy") => ok(json!({})),
                ("POST", "/v1/subscriptions/chatgpt/sign-in") => ok(json!({
                    "state": "waiting", "user_code": "WXYZ-1234",
                    "verification_uri": "https://auth.openai.com/codex/device",
                    "expires_at": now_ms() + 900_000,
                })),
                ("GET", "/v1/subscriptions") => {
                    let state = if held.signing_in.len() > 1 {
                        held.signing_in.remove(0)
                    } else {
                        held.signing_in.first().copied().unwrap_or("signed_out")
                    };
                    let error = (state == "failed").then_some("the code was refused");
                    ok(json!({"subscriptions": [
                        {"provider": "chatgpt", "state": state, "model": "gpt-5.4-mini", "error": error}
                    ]}))
                }
                _ => (404, json!({"error": {"code": "no_such_door"}}).to_string()),
            }
        })
    }

    /// A setup directory whose kvasir.json holds the installer's token and
    /// these purposes, and whose assistant already has a key that covers
    /// both purposes the fake's keys name.
    fn kvasir_dir(name: &str, purposes: serde_json::Value) -> PathBuf {
        let dir = scratch(name);
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        std::fs::write(
            kvasir.join("kvasir.json"),
            serde_json::json!({
                "auth": {"mode": "off", "tokens": {"tok": INSTALLER}},
                "purposes": purposes,
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(kvasir.join("assistant.key"), "kvs_k_new.fresh").unwrap();
        dir
    }

    /// The bodies a fake Kvasir was sent to add a backend with, in order.
    fn added_bodies(
        calls: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Vec<serde_json::Value> {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| c.strip_prefix("POST /v1/backends "))
            .map(|body| serde_json::from_str(body).unwrap())
            .collect()
    }

    #[test]
    fn the_gateway_is_made_ready_for_the_assistant() {
        let dir = kvasir_dir(
            "ready",
            serde_json::json!([
                {"id": "assistant.ask-help", "content": "rows"},
                {"id": "assistant.operator", "content": "catalog"}
            ]),
        );
        let kvasir = dir.join("kvasir");
        std::fs::write(kvasir.join("assistant.key"), "kvs_k_old.stale").unwrap();
        let held = std::sync::Arc::new(std::sync::Mutex::new(Held {
            purposes: serde_json::json!([
                {"purpose": "assistant.operator", "content": "catalog", "backend": null},
                {"purpose": "assistant.ask-help", "content": "rows", "backend": null}
            ]),
            ..Held::default()
        }));
        let (port, calls) = fake_kvasir(held.clone());
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.ports.kvasir = port;
        let chosen = ModelChoice {
            url: "http://127.0.0.1:30000/v1".to_string(),
            local: true,
            key: Some("sk-local".to_string()),
            model: "m".to_string(),
            later: false,
            chatgpt: false,
        };
        assert!(keep_to_add(&plan, vec![model_door_body(&plan, &chosen)]).is_ok());
        let console = Console::new(true);

        // the model chosen is added through Kvasir, which admits it on its
        // own; a key made before a station was added is made again, and the
        // old one revoked
        assert_eq!(
            ready_kvasir(&plan, &console, Some(&chosen)).ok(),
            Some(true)
        );
        let bodies = added_bodies(&calls);
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0]["id"], "model");
        assert_eq!(bodies[0]["baseUrl"], "http://127.0.0.1:30000/v1");
        assert_eq!(bodies[0]["locality"], "local");
        assert_eq!(bodies[0]["models"], serde_json::json!(["m"]));
        assert_eq!(bodies[0]["key"], "sk-local");
        assert!(bodies[0].get("replaces").is_none(), "{}", bodies[0]);
        assert!(
            !to_add_path(&plan).exists(),
            "nothing is kept once Kvasir holds it"
        );
        let seen = calls.lock().unwrap().clone();
        assert!(
            !seen.iter().any(|c| c.starts_with("POST /v1/admission/run")),
            "Kvasir admits what it was just given: {seen:?}"
        );
        let key = std::fs::read_to_string(kvasir.join("assistant.key")).unwrap();
        assert_eq!(key.trim(), "kvs_k_new.fresh");
        let minted = seen
            .iter()
            .find(|c| c.starts_with("POST /v1/keys"))
            .unwrap();
        assert!(
            minted.contains("assistant.ask-help") && minted.contains("assistant.operator"),
            "{minted}"
        );
        assert!(
            seen.iter().any(|c| c.starts_with("DELETE /v1/keys/k_old")),
            "{seen:?}"
        );
        assert!(
            !seen.iter().any(|c| c.starts_with("PUT")),
            "a local model needs no mapping: {seen:?}"
        );

        // a model held from before and not listed is run through admission
        {
            let mut held = held.lock().unwrap();
            held.listed.clear();
        }
        calls.lock().unwrap().clear();
        assert_eq!(ready_kvasir(&plan, &console, None).ok(), Some(true));
        let seen = calls.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|c| c.starts_with("POST /v1/admission/run") && c.contains(r#""model":"m""#)),
            "{seen:?}"
        );

        // a commercial provider as the only model: what reads no rows is
        // mapped to it, and a key that covers every purpose is kept
        calls.lock().unwrap().clear();
        {
            let mut held = held.lock().unwrap();
            held.backends = vec![
                serde_json::json!({"id": "model", "locality": "remote", "base_url": "https://api.example.org/v1",
                                   "models": ["big"], "builtin": false}),
                serde_json::json!({"id": "chatgpt", "locality": "remote", "models": ["gpt-5.4-mini"], "builtin": true}),
            ];
            held.listed = vec!["big".to_string()];
        }
        assert_eq!(ready_kvasir(&plan, &console, None).ok(), Some(true));
        let seen = calls.lock().unwrap().clone();
        assert!(
            seen.iter().any(
                |c| c.starts_with("PUT /v1/purposes/assistant.operator/policy")
                    && c.contains(r#""backend":"model""#)
            ),
            "{seen:?}"
        );
        assert!(
            !seen.iter().any(|c| c.contains("assistant.ask-help/policy")),
            "what reads rows stays closed until an admin opens it: {seen:?}"
        );
        assert!(
            !seen
                .iter()
                .any(|c| c.starts_with("POST /v1/admission/run")
                    || c.starts_with("POST /v1/backends")),
            "a provider is not admitted, and nothing is added: {seen:?}"
        );
        assert!(
            !seen.iter().any(|c| c.starts_with("POST /v1/keys")),
            "{seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_chosen_model_is_added_once_and_again_only_with_other_values() {
        let dir = kvasir_dir("added-once", serde_json::json!([]));
        let held = std::sync::Arc::new(std::sync::Mutex::new(Held::default()));
        let (port, calls) = fake_kvasir(held.clone());
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.ports.kvasir = port;
        let console = Console::new(true);
        let mut chosen = ModelChoice {
            url: "http://127.0.0.1:30000/v1".to_string(),
            local: true,
            key: None,
            model: "m".to_string(),
            later: false,
            chatgpt: false,
        };
        // chosen twice, as two runs of setup would
        for _ in 0..2 {
            assert!(keep_to_add(&plan, vec![model_door_body(&plan, &chosen)]).is_ok());
            assert_eq!(
                ready_kvasir(&plan, &console, Some(&chosen)).ok(),
                Some(true)
            );
        }
        assert_eq!(
            added_bodies(&calls).len(),
            1,
            "held with the same address and model, it is not added again"
        );
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("DELETE /v1/backends"))
        );
        assert!(!to_add_path(&plan).exists());

        // another address takes the place of the one held
        chosen.url = "http://127.0.0.1:8000/v1".to_string();
        assert!(keep_to_add(&plan, vec![model_door_body(&plan, &chosen)]).is_ok());
        assert_eq!(
            ready_kvasir(&plan, &console, Some(&chosen)).ok(),
            Some(true)
        );
        let seen = calls.lock().unwrap().clone();
        let removed = seen
            .iter()
            .position(|c| c.starts_with("DELETE /v1/backends/model"))
            .expect("the one held is removed");
        let added = seen
            .iter()
            .rposition(|c| c.starts_with("POST /v1/backends "))
            .unwrap();
        assert!(
            removed < added && seen[added].contains("127.0.0.1:8000"),
            "{seen:?}"
        );
        assert_eq!(held.lock().unwrap().backends.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_model_kvasir_cannot_reach_stops_an_install_and_only_warns_on_an_update() {
        let dir = kvasir_dir("unreached", serde_json::json!([]));
        let held = std::sync::Arc::new(std::sync::Mutex::new(Held::default()));
        let (port, _calls) = fake_kvasir(held);
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.ports.kvasir = port;
        let chosen = ModelChoice {
            url: "http://127.0.0.1:30000/v1".to_string(),
            local: true,
            key: None,
            model: "silent".to_string(),
            later: false,
            chatgpt: false,
        };
        assert!(keep_to_add(&plan, vec![model_door_body(&plan, &chosen)]).is_ok());
        let console = Console::new(true);

        // an install stops with Kvasir's words and says plainly where it failed
        console.strict.set(true);
        let stopped = ready_kvasir(&plan, &console, Some(&chosen)).unwrap_err();
        for said in [
            "could not reach silent",
            "from where it runs",
            "silent: fetch failed",
            "reached it from this machine",
            "the assistant has no model",
        ] {
            assert!(
                stopped.message.contains(said),
                "{said}: {}",
                stopped.message
            );
        }
        assert_eq!(to_add(&plan).len(), 1, "kept for the next run");

        // an update says it and goes on
        console.strict.set(false);
        assert_eq!(ready_kvasir(&plan, &console, None).ok(), Some(true));
        assert_eq!(to_add(&plan).len(), 1);

        // a backend from before that does not answer now is only said, in an
        // install too, and kept
        std::fs::write(
            to_add_path(&plan),
            serde_json::json!([{"id": "card", "baseUrl": "http://127.0.0.1:30000/v1",
                                "locality": "local", "models": ["silent"]}])
            .to_string(),
        )
        .unwrap();
        console.strict.set(true);
        assert_eq!(ready_kvasir(&plan, &console, None).ok(), Some(true));
        assert_eq!(to_add(&plan).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_moves_old_backends_out_of_the_file_and_back_in_through_kvasir() {
        let dir = kvasir_dir("moved", serde_json::json!([]));
        let kvasir = dir.join("kvasir");
        std::fs::write(kvasir.join("card.key"), "sk-card\n").unwrap();
        std::fs::write(
            kvasir.join("kvasir.json"),
            serde_json::json!({
                "auth": {"mode": "off", "tokens": {"tok": INSTALLER}},
                "purposes": [],
                "backends": [
                    {"id": "card", "baseUrl": "http://127.0.0.1:30000/v1", "locality": "local",
                     "keyFile": "card.key", "models": [{"id": "qwen", "contextWindow": 65536}]},
                    {"id": "provider", "baseUrl": "https://api.example.org/v1", "locality": "remote",
                     "key": "sk-provider", "models": ["big"]}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let held = std::sync::Arc::new(std::sync::Mutex::new(Held::default()));
        let (port, calls) = fake_kvasir(held.clone());
        // the setup moved into a pod since the file was written
        let mut plan = plan(Runtime::Podman);
        plan.dir = dir.clone();
        plan.ports.kvasir = port;
        let mut console = Console::new(true);
        assert!(repair_kvasir(&plan, &mut console).is_ok());
        assert!(kvasir_config(&plan).unwrap().get("backends").is_none());

        assert_eq!(ready_kvasir(&plan, &console, None).ok(), Some(true));
        let bodies = added_bodies(&calls);
        assert_eq!(bodies.len(), 2, "{bodies:?}");
        assert_eq!(bodies[0]["id"], "card", "under the same id");
        assert_eq!(
            bodies[0]["key"], "sk-card",
            "with the key its key file holds"
        );
        assert_eq!(
            bodies[0]["baseUrl"], "http://host.containers.internal:30000/v1",
            "dialled from where Kvasir runs"
        );
        assert_eq!(bodies[0]["models"][0]["contextWindow"], 65536);
        assert_eq!(bodies[1]["id"], "provider");
        assert_eq!(bodies[1]["key"], "sk-provider");
        assert!(
            !to_add_path(&plan).exists(),
            "nothing is left once Kvasir holds them"
        );
        let ids: Vec<String> = held
            .lock()
            .unwrap()
            .backends
            .iter()
            .filter_map(|b| b["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(ids, vec!["card", "provider"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_s_chatgpt_subscription_is_signed_in_and_opens_what_reads_no_rows() {
        let dir = kvasir_dir("chatgpt", serde_json::json!([]));
        let held = std::sync::Arc::new(std::sync::Mutex::new(Held {
            purposes: serde_json::json!([
                {"purpose": "assistant.title", "content": "catalog", "backend": null},
                {"purpose": "assistant.operator", "content": "catalog", "backend": null},
                {"purpose": "assistant.ask-help", "content": "rows", "backend": null},
                {"purpose": "other.lookup", "content": "catalog", "backend": null}
            ]),
            signing_in: vec!["signed_out", "waiting", "signed_in"],
            ..Held::default()
        }));
        let (port, calls) = fake_kvasir(held.clone());
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.ports.kvasir = port;
        // a person at the terminal, whose screen is kept to be read
        let mut console = Console::new(false);
        console.screens = Some(std::cell::RefCell::new(Replay::default()));
        console.strict.set(true);
        let said = |console: &Console| -> String {
            console
                .screens
                .as_ref()
                .map(|replay| {
                    replay
                        .borrow()
                        .said
                        .iter()
                        .filter_map(|s| match s {
                            tui::Said::Text(t) | tui::Said::Note(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };

        assert_eq!(
            ready_kvasir(&plan, &console, Some(&ModelChoice::chatgpt())).ok(),
            Some(true)
        );
        let shown = said(&console);
        assert!(
            shown.contains("open https://auth.openai.com/codex/device")
                && shown.contains("enter the code WXYZ-1234"),
            "the link and the code, in plain words: {shown}"
        );
        assert!(shown.contains("works for 15 minute(s)"), "{shown}");
        assert!(shown.contains("signed in to ChatGPT"), "{shown}");
        assert!(
            shown.contains(
                "2 purpose(s) that read no rows of the archive go to your ChatGPT subscription"
            ),
            "{shown}"
        );
        assert!(
            shown.contains(
                "assistant.ask-help read rows of the archive and stay closed to your ChatGPT subscription"
            ),
            "{shown}"
        );
        let seen = calls.lock().unwrap().clone();
        assert!(
            seen.iter()
                .filter(|c| c.starts_with("GET /v1/subscriptions"))
                .count()
                >= 3,
            "asked again until it is signed in: {seen:?}"
        );
        for purpose in ["assistant.title", "assistant.operator"] {
            assert!(
                seen.iter().any(
                    |c| c.starts_with(&format!("PUT /v1/purposes/{purpose}/policy"))
                        && c.contains(r#""backend":"chatgpt""#)
                ),
                "{purpose}: {seen:?}"
            );
        }
        assert!(
            !seen
                .iter()
                .any(|c| c.contains("assistant.ask-help/policy")
                    || c.contains("other.lookup/policy")),
            "what reads rows stays closed, and what is not the assistant's is not touched: {seen:?}"
        );
        assert!(
            !seen.iter().any(|c| c.starts_with("POST /v1/backends")),
            "ChatGPT is Kvasir's own: {seen:?}"
        );

        // a sign-in that does not finish leaves the assistant with no model,
        // which stops an install
        held.lock().unwrap().signing_in = vec!["signed_out", "failed"];
        let stopped = ready_kvasir(&plan, &console, Some(&ModelChoice::chatgpt())).unwrap_err();
        assert!(
            stopped
                .message
                .contains("the ChatGPT sign-in did not finish (the code was refused)")
                && stopped.message.contains("the assistant has no model"),
            "{}",
            stopped.message
        );
        assert!(said(&console).contains("run nils setup again"));

        // with nobody at the terminal, no sign-in is started
        calls.lock().unwrap().clear();
        let blind = Console::new(true);
        assert_eq!(
            ready_kvasir(&plan, &blind, Some(&ModelChoice::chatgpt())).ok(),
            Some(true)
        );
        assert!(
            !calls.lock().unwrap().iter().any(|c| c.contains("/sign-in")),
            "a sign-in is a person's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The question the model is chosen with, as the screens would ask it,
    /// and what was said above it.
    fn model_question(mode: Mode) -> (Vec<(String, String)>, Vec<tui::Said>) {
        let mut console = Console::new(false);
        console.screens = Some(std::cell::RefCell::new(Replay::default()));
        let Err(Stop::Ask(Question {
            ask: tui::Ask::Pick { options, .. },
            ..
        })) = choose_model(&mut console, true, mode)
        else {
            panic!("the model is asked with a list");
        };
        let said = console
            .screens
            .take()
            .map(|replay| replay.into_inner().said)
            .unwrap_or_default();
        (options, said)
    }

    #[test]
    fn chatgpt_is_offered_only_where_nobody_signs_in() {
        let (options, _) = model_question(Mode::Off);
        let titles: Vec<&str> = options.iter().map(|(title, _)| title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "A model server on this machine",
                "A model server on another machine of yours",
                "A commercial provider",
                "Your ChatGPT subscription",
                "Decide later"
            ]
        );
        assert!(
            options[3].1.contains("once setup has started Kvasir")
                && options[3].1.contains("the prompt leaves your systems"),
            "{:?}",
            options[3]
        );

        // where people sign in, each signs in to their own from the desk
        for mode in [Mode::Local, Mode::Oidc] {
            let (options, said) = model_question(mode);
            assert!(
                !options.iter().any(|(title, _)| title.contains("ChatGPT")),
                "{options:?}"
            );
            assert!(
                said.iter().any(|s| matches!(s, tui::Said::Note(note)
                    if note.contains("each signs in to theirs from the desk"))),
                "{said:?}"
            );
        }

        // chosen, it has no address and no key, and names Kvasir's ChatGPT
        use tui::Key::{Down, Enter};
        let (outcome, _) = on_screens(
            |console| choose_model(console, true, Mode::Off),
            vec![Down, Down, Down, Enter],
        );
        let Ok((chosen, _)) = outcome else {
            panic!("the subscription was not chosen");
        };
        assert!(chosen.chatgpt && !chosen.later);
        assert_eq!(chosen.model, CHATGPT);
        assert!(chosen.url.is_empty() && chosen.key.is_none());
    }

    #[test]
    fn the_clones_are_pinned_to_release_tags_and_a_lab_may_name_another_ref() {
        assert_eq!(KVASIR_REF, "v1.0.0-alpha.7");
        assert_eq!(ASSISTANT_REF, "v1.0.0-alpha.25");
        assert_eq!(source_ref(KVASIR_REF, None), "v1.0.0-alpha.7");
        assert_eq!(
            source_ref(KVASIR_REF, Some("models-held")),
            "models-held",
            "a branch, for a lab before the tag exists"
        );
        assert_eq!(
            source_ref(ASSISTANT_REF, Some("  ")),
            "v1.0.0-alpha.25",
            "an empty variable names nothing"
        );

        let into = Path::new("/home/x/nils/kvasir");
        assert_eq!(
            source_steps(KVASIR_REPO, KVASIR_REF, into, false),
            vec![vec![
                "clone",
                "--depth",
                "1",
                "--branch",
                "v1.0.0-alpha.7",
                "https://github.com/kineuro/kvasir",
                "/home/x/nils/kvasir"
            ]],
            "a clone of the tag alone"
        );
        // a checkout, one of main too, is brought to the ref and left detached
        assert_eq!(
            source_steps(KVASIR_REPO, "models-held", into, true),
            vec![
                vec!["fetch", "--depth", "1", "origin", "models-held"],
                vec!["checkout", "--detach", "FETCH_HEAD"]
            ]
        );
        let steps = source_steps(ASSISTANT_REPO, ASSISTANT_REF, into, true);
        assert_eq!(
            source_label(&steps[0], "the assistant", ASSISTANT_REF),
            "fetching the assistant at v1.0.0-alpha.25"
        );
        assert_eq!(
            source_label(&steps[1], "Kvasir", KVASIR_REF),
            "checking out Kvasir at v1.0.0-alpha.7"
        );
        assert_eq!(node_source("desk"), None, "only the two Node parts");
    }

    #[test]
    fn an_uninstall_removes_kvasirs_state_even_where_the_data_is_kept() {
        let root = scratch("uninstall-kvasir");
        let dir = root.join("nils");
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(kvasir.join("state")).unwrap();
        for file in [
            "kvasir.json",
            "kvasir.sqlite",
            "kvasir.seal",
            "kvasir.pepper",
            "assistant.key",
            "backends-to-add.json",
            "state/held",
        ] {
            std::fs::write(kvasir.join(file), "x").unwrap();
        }
        let assistant = dir.join("assistant");
        std::fs::create_dir_all(assistant.join("node_modules")).unwrap();
        std::fs::write(assistant.join("assistant.sqlite"), "x").unwrap();
        let mut state = State {
            dir: dir.display().to_string(),
            mode: "off".to_string(),
            runtime: "machine".to_string(),
            ..State::default()
        };
        for (name, path) in [("kvasir", &kvasir), ("assistant", &assistant)] {
            state.parts.insert(
                name.to_string(),
                PartState {
                    version: "from source".to_string(),
                    path: path.display().to_string(),
                    kind: "node".to_string(),
                },
            );
        }
        let removal = gather_removal(&state, None, Leaving::KeepData);
        assert_eq!(removal.kvasir.as_deref(), Some(kvasir.as_path()));
        assert_eq!(
            removal.built,
            vec![assistant.join("node_modules")],
            "the assistant's history stays, and Kvasir goes whole"
        );
        let text = removal_text(&removal, Leaving::KeepData, &Console::new(true));
        assert!(
            text.contains("the models it holds, their keys, its subscriptions"),
            "{text}"
        );

        // carried out with nothing of this machine's own in it
        let record = root.join("setup.toml");
        std::fs::write(&record, "").unwrap();
        let removal = Removal {
            units: Vec::new(),
            unit_files: Vec::new(),
            containers: Vec::new(),
            images: Vec::new(),
            programs: Vec::new(),
            me: None,
            packs: Vec::new(),
            packs_kept: None,
            state: record.clone(),
            postgres: None,
            ..removal
        };
        carry_out(&removal, Leaving::KeepData, &Console::new(true));
        assert!(!kvasir.exists(), "Kvasir's state stayed");
        assert!(
            assistant.join("assistant.sqlite").is_file(),
            "the assistant's history is data"
        );
        assert!(!assistant.join("node_modules").exists());
        assert!(!record.exists());

        // where everything goes, Kvasir goes with the base directory
        std::fs::create_dir_all(&kvasir).unwrap();
        assert_eq!(gather_removal(&state, None, Leaving::Purge).kvasir, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_server_is_asked_which_models_it_serves() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let Ok(mut s) = stream else { break };
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let asked = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if asked.contains("authorization: Bearer sk-test")
                    || !asked.contains("authorization")
                {
                    r#"{"object":"list","data":[{"id":"qwen-small"},{"id":"qwen-large"}]}"#
                } else {
                    "{}"
                };
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/v1");
        assert_eq!(
            list_models(&url, None),
            Some(vec!["qwen-small".to_string(), "qwen-large".to_string()])
        );
        assert_eq!(
            list_models(&url, Some("sk-test")).map(|ids| ids.len()),
            Some(2),
            "a key is sent as a bearer"
        );
        assert_eq!(
            list_models("http://127.0.0.1:1/v1", None),
            None,
            "nothing answering is None"
        );
    }

    #[test]
    fn a_model_is_taken_only_once_it_answers_a_short_question() {
        // the words for each way a model server or a provider answers
        assert_eq!(
            model_answer(200, r#"{"choices":[{"message":{"content":"ready"}}]}"#, "m"),
            Ok(())
        );
        assert!(
            model_answer(200, "<html></html>", "m")
                .unwrap_err()
                .contains("not the way an OpenAI compatible server does")
        );
        assert_eq!(
            model_answer(
                401,
                r#"{"error":{"message":"Incorrect API key provided"}}"#,
                "m"
            ),
            Err("the key was refused: Incorrect API key provided".to_string())
        );
        assert!(
            model_answer(404, "{}", "gpt-9")
                .unwrap_err()
                .contains("no model named gpt-9")
        );
        assert!(
            model_answer(429, r#"{"error":"insufficient quota"}"#, "m")
                .unwrap_err()
                .ends_with(": insufficient quota")
        );
        assert_eq!(
            model_answer(500, "", "m"),
            Err("it answered 500".to_string())
        );

        // over the wire: a server that answers with its key and refuses without one
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let Ok(mut s) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let asked = String::from_utf8_lossy(&buf[..n]).to_string();
                let (status, body) = if asked.contains("authorization: Bearer sk-test") {
                    (
                        "200 OK",
                        r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"ready"}}]}"#,
                    )
                } else {
                    ("401 Unauthorized", r#"{"error":{"message":"no key"}}"#)
                };
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/v1");
        assert_eq!(try_model(&url, Some("sk-test"), "qwen"), Ok(()));
        assert_eq!(
            try_model(&url, None, "qwen"),
            Err("the key was refused: no key".to_string())
        );
        assert!(
            try_model("http://127.0.0.1:1/v1", None, "qwen")
                .unwrap_err()
                .starts_with("nothing answered there")
        );
    }

    #[test]
    fn removing_everything_refuses_what_an_install_did_not_make() {
        let home = scratch("purge-home");
        let made = home.join("nils");
        std::fs::create_dir_all(made.join("registry")).unwrap();
        std::fs::write(made.join("registry").join("nils.toml"), "").unwrap();
        assert!(
            safe_to_purge(&made, Some(&home)).is_ok(),
            "an install's own directory"
        );

        let why = |dir: &Path| safe_to_purge(dir, Some(&home)).unwrap_err();
        assert!(why(&home).contains("home directory"), "{}", why(&home));
        assert!(
            why(Path::new("/")).contains("root"),
            "{}",
            why(Path::new("/"))
        );
        assert!(why(Path::new("nils")).contains("absolute"));
        let stranger = home.join("photos");
        std::fs::create_dir_all(&stranger).unwrap();
        assert!(
            why(&stranger).contains("neither a registry nor a desk"),
            "a directory named in a hand edited record, holding someone's photos"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_uninstall_removes_the_programs_the_record_names_and_no_other() {
        let root = scratch("programs");
        let bin = root.join("prefix").join("bin");
        let packs = root.join("prefix").join("share").join("nils").join("packs");
        let elsewhere = root.join("build").join("nils");
        for dir in [&bin, &packs.join("mri"), &packs.join("mine")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        for file in [bin.join("nils"), bin.join("nils-desk"), elsewhere.clone()] {
            std::fs::write(file, "").unwrap();
        }
        let bin = std::fs::canonicalize(&bin).unwrap();
        let packs = std::fs::canonicalize(&packs).unwrap();
        let elsewhere = std::fs::canonicalize(&elsewhere).unwrap();
        let part = |path: &Path, kind: &str| PartState {
            version: "1".to_string(),
            path: path.display().to_string(),
            kind: kind.to_string(),
        };

        // On the machine: the parts name both programs. This program, run
        // from a build somewhere else, is not the one the install put there.
        let mut state = State {
            dir: root.join("data").display().to_string(),
            runtime: "machine".to_string(),
            ..State::default()
        };
        state
            .parts
            .insert("engine".to_string(), part(&bin.join("nils"), "binary"));
        state
            .parts
            .insert("desk".to_string(), part(&bin.join("nils-desk"), "binary"));
        let removal = gather_removal(&state, Some(elsewhere.clone()), Leaving::KeepData);
        assert_eq!(
            removal.programs,
            vec![bin.join("nils-desk"), bin.join("nils")]
        );
        assert_eq!(removal.me, None, "a build the record does not name stays");
        assert_eq!(removal.me_kept, Some(elsewhere.clone()));
        assert_eq!(
            removal.packs,
            vec![packs.join("mri")],
            "beside the installed nils"
        );
        assert_eq!(
            removal.packs_kept,
            Some(packs.clone()),
            "mine is a person's own"
        );

        // In containers: the parts are images, and the record's programs
        // name the nils that set it up and the nils-desk the image came from.
        let mut state = State {
            dir: root.join("data").display().to_string(),
            runtime: "machine".to_string(),
            programs: vec![
                bin.join("nils").display().to_string(),
                bin.join("nils-desk").display().to_string(),
            ],
            ..State::default()
        };
        state.parts.insert(
            "engine".to_string(),
            part(Path::new("ghcr.io/kineuro/nils:v1"), "podman"),
        );
        let removal = gather_removal(&state, Some(bin.join("nils")), Leaving::KeepData);
        assert_eq!(removal.programs, vec![bin.join("nils-desk")]);
        assert_eq!(removal.me, Some(bin.join("nils")), "this one, last");
        assert_eq!(removal.me_kept, None);

        // A record from before programs were kept names no program at all.
        state.programs.clear();
        let removal = gather_removal(&state, Some(bin.join("nils")), Leaving::KeepData);
        assert!(removal.programs.is_empty());
        assert_eq!(removal.me, None);
        assert!(removal.packs.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_data_is_summarised_without_walking_an_archive() {
        let dir = scratch("summary");
        std::fs::create_dir_all(dir.join("registry")).unwrap();
        std::fs::write(dir.join("registry").join("nils.toml"), "x").unwrap();
        std::fs::write(dir.join("registry").join("registry.db"), vec![0u8; 2048]).unwrap();
        std::fs::create_dir_all(dir.join("backups")).unwrap();
        std::fs::write(dir.join("backups").join("a.tar.zst"), "b").unwrap();
        std::fs::create_dir_all(dir.join("desk")).unwrap();
        std::fs::write(dir.join("desk").join("nils-desk.sqlite"), "").unwrap();
        let lines = data_summary(&dir).join(" | ");
        assert!(
            lines.contains("the registry, 2.0 KB, with its key"),
            "{lines}"
        );
        assert!(lines.contains("1 backup file(s)"), "{lines}");
        assert!(lines.contains("the desk's database"), "{lines}");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_card_is_read_as_what_it_can_serve() {
        let none = card_advice(None).join(" ");
        assert!(none.starts_with("No local model worth serving."), "{none}");
        assert!(none.contains("no assistant at all"), "{none}");
        assert_eq!(card_advice(Some(0.0)).join(" "), none);
        assert_eq!(card_advice(Some(8.0)).join(" "), none, "8 GB serves none");
        let middling = card_advice(Some(16.0)).join(" ");
        assert!(
            middling.starts_with("A 7B to 14B model fits."),
            "{middling}"
        );
        assert!(middling.contains("more retries"), "{middling}");
        let plenty = card_advice(Some(48.0)).join(" ");
        assert!(plenty.starts_with("A 27B model at 4 bit fits"), "{plenty}");
        assert!(plenty.contains("stations were written against"), "{plenty}");
    }

    #[test]
    fn every_card_is_read_and_named_once_with_the_memory_of_them_all() {
        // several lines of nvidia-smi: two cards alike and one other
        let out = "NVIDIA RTX PRO 6000 Blackwell Workstation Edition, 97887\n\
                   NVIDIA RTX PRO 6000 Blackwell Workstation Edition, 97887\n\
                   NVIDIA GeForce RTX 4090, 24564\n";
        let cards = nvidia_cards(out);
        assert_eq!(cards.len(), 3, "{cards:?}");
        assert_eq!(cards[2].name, "NVIDIA GeForce RTX 4090");
        assert!(
            (cards[0].memory_gb - 97887.0 / 1024.0).abs() < 1e-9,
            "{cards:?}"
        );
        assert!((total_gb(&cards) - 220338.0 / 1024.0).abs() < 1e-9);
        assert_eq!(largest(&cards).unwrap().name, cards[0].name);
        assert_eq!(
            cards_words(&cards).as_deref(),
            Some(
                "2 × NVIDIA RTX PRO 6000 Blackwell Workstation Edition and NVIDIA GeForce RTX 4090, 215 GB"
            )
        );
        let pair = nvidia_cards("NVIDIA RTX PRO 6000, 97887\nNVIDIA RTX PRO 6000, 97887");
        assert_eq!(
            cards_words(&pair).as_deref(),
            Some("2 × NVIDIA RTX PRO 6000, 191 GB")
        );
        // one line reads as the one card did
        let one = nvidia_cards("NVIDIA GeForce RTX 4090, 24564\n");
        assert_eq!(one.len(), 1, "{one:?}");
        assert_eq!(
            cards_words(&one).as_deref(),
            Some("NVIDIA GeForce RTX 4090, 24 GB")
        );
        assert_eq!(largest(&one).unwrap().name, "NVIDIA GeForce RTX 4090");
        // none: nothing listed, or a line that names no card
        assert!(nvidia_cards("").is_empty());
        assert!(nvidia_cards("No devices were found\n").is_empty());
        assert_eq!(total_gb(&[]), 0.0);
        assert!(largest(&[]).is_none());
        assert_eq!(cards_words(&[]), None);
        // the largest is the one with the most memory, wherever it is listed
        let unlike = nvidia_cards("NVIDIA GeForce RTX 3060, 12288\nNVIDIA GeForce RTX 4090, 24564");
        assert_eq!(largest(&unlike).unwrap().name, "NVIDIA GeForce RTX 4090");
        assert_eq!(
            cards_words(&unlike).as_deref(),
            Some("NVIDIA GeForce RTX 3060 and NVIDIA GeForce RTX 4090, 36 GB")
        );
        // rocm-smi: a card on each line that names its memory
        let amd = amd_cards(
            "device,VRAM Total Memory (B),VRAM Total Used Memory (B)\n\
             card0,17163091968,1254912\n\
             card1,34342961152,11653120\n",
        );
        assert_eq!(amd.len(), 2, "{amd:?}");
        assert!((largest(&amd).unwrap().memory_gb - 34342961152.0 / 1073741824.0).abs() < 1e-9);
        assert!(amd_cards("device,VRAM Total Memory (B)\n").is_empty());
    }

    #[test]
    fn the_parts_of_a_list_always_hold_the_engine() {
        assert_eq!(parts_of("").unwrap(), vec![Part::Engine]);
        assert_eq!(parts_of("desk").unwrap(), vec![Part::Engine, Part::Desk]);
        assert_eq!(
            parts_of("assistant,desk,engine").unwrap(),
            vec![Part::Engine, Part::Desk, Part::Assistant]
        );
        assert!(parts_of("everything").is_err());
    }

    #[test]
    fn a_mode_and_a_runtime_are_one_of_three() {
        assert_eq!(Mode::parse("off").unwrap(), Mode::Off);
        assert_eq!(Mode::parse(" local ").unwrap(), Mode::Local);
        assert!(Mode::parse("open").is_err());
        assert_eq!(Runtime::parse("podman").unwrap(), Runtime::Podman);
        assert!(Runtime::parse("lxc").is_err());
        assert!(Runtime::Docker.container() && !Runtime::Machine.container());
    }

    #[test]
    fn a_taken_port_moves_to_the_next_free_one() {
        let taken = |p: u16| p < 7203;
        assert_eq!(next_free_port(7200, &taken), 7203);
        assert_eq!(next_free_port(7300, &taken), 7300);
    }

    #[test]
    fn every_runtime_this_machine_has_is_offered() {
        let names = |podman, docker| {
            runtime_choices(podman, docker)
                .iter()
                .map(|(r, _, _)| r.name())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(false, false), vec!["machine"]);
        assert_eq!(names(true, false), vec!["machine", "podman"]);
        assert_eq!(names(false, true), vec!["machine", "docker"]);
        assert_eq!(
            names(true, true),
            vec!["machine", "podman", "docker"],
            "both, when both are here, not podman in docker's place"
        );
        assert!(docker_absent_reason(Err(DockerAbsent::NoDaemon)).is_some());
        assert!(docker_absent_reason(Err(DockerAbsent::NotInstalled)).is_none());
        assert!(docker_absent_reason(Err(DockerAbsent::PodmanWrapper)).is_none());
        assert!(docker_absent_reason(Ok(())).is_none());
    }

    #[test]
    fn a_part_moves_off_a_taken_port_and_never_onto_another_part_s() {
        let taken = |p: u16| p == 7100 || p == 7101;
        assert_eq!(settle_port(7300, &[], &taken), None, "free, so it stays");
        assert_eq!(settle_port(7100, &[], &taken), Some(7102));
        // the gateway would take 7102, which the desk was just given
        assert_eq!(settle_port(7100, &[7102, 7200], &taken), Some(7103));
        assert_eq!(
            settle_port(7200, &[7200], &|_| false),
            Some(7201),
            "two parts are never given one port"
        );
    }

    #[test]
    fn the_desk_binds_where_it_may_be_reached() {
        let (bind, origin, also) = desk_binding(&Reach::Loopback, 7200, false);
        assert_eq!(bind, "127.0.0.1:7200");
        assert_eq!(origin, "http://127.0.0.1:7200");
        assert_eq!(also, vec!["http://localhost:7200".to_string()]);
        let (bind, _, _) = desk_binding(&Reach::Loopback, 7200, true);
        assert_eq!(bind, "0.0.0.0:7200", "a container binds inside itself");
        let (bind, origin, also) = desk_binding(&Reach::Network("10.0.0.5".into()), 7200, false);
        assert_eq!(bind, "0.0.0.0:7200");
        assert_eq!(origin, "http://10.0.0.5:7200");
        assert!(also.contains(&"http://127.0.0.1:7200".to_string()));
    }

    #[test]
    fn a_desk_behind_a_proxy_answers_at_its_origin_and_binds_where_it_is_told() {
        let behind = |network| Reach::Behind {
            origin: "https://nils.example.org".to_string(),
            network,
        };
        let (bind, origin, also) = desk_binding(&behind(false), 7200, false);
        assert_eq!(bind, "127.0.0.1:7200", "a proxy here reaches the loopback");
        assert_eq!(origin, "https://nils.example.org");
        assert!(
            also.contains(&"http://127.0.0.1:7200".to_string())
                && also.contains(&"http://localhost:7200".to_string()),
            "a browser on the machine still opens it: {also:?}"
        );
        let (bind, _, _) = desk_binding(&behind(true), 7200, false);
        assert_eq!(
            bind, "0.0.0.0:7200",
            "a proxy elsewhere reaches this machine's address"
        );
        let (bind, _, _) = desk_binding(&behind(false), 7200, true);
        assert_eq!(bind, "0.0.0.0:7200", "a container binds inside itself");

        // what the parts are told follows the same address: the desk signs as
        // the address a browser opens, and its keys are fetched where it runs
        let mut p = plan(Runtime::Machine);
        p.mode = Mode::Local;
        p.reach = behind(false);
        let (issuer, jwks) = desk_trust(&p);
        assert_eq!(issuer, "https://nils.example.org");
        assert_eq!(jwks, "http://127.0.0.1:7200/.well-known/jwks.json");
        assert!(
            engine_args(&p, "/r", "/b")
                .join(" ")
                .contains("--oidc-trust issuer=https://nils.example.org,audience=nils"),
            "the engine trusts what the desk signs"
        );

        // the port is published for a proxy here, and opened for one elsewhere
        p.runtime = Runtime::Podman;
        assert!(
            podman_commands(&p)[0].contains("-p 127.0.0.1:7200:7200"),
            "{}",
            podman_commands(&p)[0]
        );
        p.reach = behind(true);
        assert!(
            podman_commands(&p)[0].contains("-p 7200:7200"),
            "{}",
            podman_commands(&p)[0]
        );
    }

    #[test]
    fn an_origin_is_a_scheme_and_a_host_and_anything_else_is_said() {
        assert_eq!(
            origin_given(" https://nils.example.org/ ").unwrap(),
            "https://nils.example.org",
            "the spaces around it and a trailing slash are not part of it"
        );
        assert_eq!(
            origin_given("HTTPS://Nils.Example.org").unwrap(),
            "https://nils.example.org",
            "a browser sends the host lower case"
        );
        assert_eq!(
            origin_given("http://10.0.0.5:7200").unwrap(),
            "http://10.0.0.5:7200"
        );
        assert_eq!(
            origin_given("https://[::1]:7200").unwrap(),
            "https://[::1]:7200"
        );
        for (bad, says) in [
            ("nils.example.org", "has no scheme"),
            ("ftp://nils.example.org", "is not a scheme"),
            ("https://nils.example.org/desk", "leave off /desk"),
            ("https://nils.example.org?q=1", "leave off ?q=1"),
            ("https://someone@nils.example.org", "carries no sign in"),
            ("https://nils.example.org:door", "is not a port"),
            ("https://", "names no host"),
            ("", "an origin is the address"),
            ("https://one two", "has a space in it"),
        ] {
            let refused = origin_given(bad).unwrap_err();
            assert!(refused.contains(says), "{bad}: {refused}");
        }
    }

    #[test]
    fn an_origin_on_record_is_the_origin_an_update_writes() {
        let dir = scratch("origin-record");
        let state = State {
            dir: dir.display().to_string(),
            mode: "local".to_string(),
            runtime: "machine".to_string(),
            service: "systemd user units".to_string(),
            reach: "loopback".to_string(),
            origin: "https://nils.example.org".to_string(),
            parts: BTreeMap::from([(
                "desk".to_string(),
                PartState {
                    version: "1.0.0-alpha.2".to_string(),
                    path: "nils-desk".to_string(),
                    kind: "binary".to_string(),
                },
            )]),
            ..State::default()
        };
        // it is written down, where a person can read it
        let written = toml::to_string(&state).unwrap();
        assert!(
            written.contains("origin = \"https://nils.example.org\""),
            "{written}"
        );

        // an update and a repair are made from the record and nothing else
        let plan = plan_from_state(&state, None);
        assert_eq!(
            plan.reach,
            Reach::Behind {
                origin: "https://nils.example.org".to_string(),
                network: false
            }
        );
        let text = desk_config_text(&plan);
        assert!(
            text.contains("origin = \"https://nils.example.org\""),
            "{text}"
        );
        assert!(text.contains("bind = \"127.0.0.1:7200\""), "{text}");
        assert!(
            text.contains("also_origins = [\"http://127.0.0.1:7200\", \"http://localhost:7200\"]"),
            "{text}"
        );

        // written once, an update has nothing to change in it
        std::fs::create_dir_all(plan.desk_dir()).unwrap();
        assert!(write_desk_config(&plan).is_ok());
        assert_eq!(
            desk_config_fate(&plan),
            DeskConfigFate::Kept,
            "an update stamped an address of its own over it"
        );
        let on_disk = std::fs::read_to_string(plan.desk_config()).unwrap();
        assert!(
            on_disk.contains("origin = \"https://nils.example.org\""),
            "{on_disk}"
        );

        // and the install reports the address a person opens
        let said = addresses(&state);
        assert_eq!(said[0]["part"], "desk", "{said:?}");
        assert_eq!(said[0]["address"], "https://nils.example.org", "{said:?}");

        // a record from before this, with no origin in it, is what it was
        let plain = State {
            origin: String::new(),
            reach: "10.0.0.5".to_string(),
            ..state
        };
        assert_eq!(
            plan_from_state(&plain, None).reach,
            Reach::Network("10.0.0.5".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_account_is_lingered_before_systemd_is_given_anything() {
        let units = vec!["nils-engine".to_string(), "nils-desk".to_string()];
        let calls = hand_units_to_systemd(&units, true, false);
        let said: Vec<String> = calls.iter().map(|c| c.argv.join(" ")).collect();
        assert!(
            said[0].starts_with("loginctl enable-linger"),
            "the account is lingered first, since the calls after it need a user manager: {said:?}"
        );
        assert_eq!(said[1], "systemctl --user daemon-reload", "{said:?}");
        assert_eq!(said[2], "systemctl --user enable nils-engine", "{said:?}");
        assert_eq!(said[3], "systemctl --user enable nils-desk", "{said:?}");
        assert!(
            !calls[0].needed,
            "lingering is not allowed everywhere, and an install does not stop on it"
        );
        assert!(
            calls[1].needed,
            "a daemon that will not read the units means nothing runs"
        );
        // the account is the one running this, not the one $USER names
        if let Some(me) = whoami() {
            assert!(said[0].ends_with(&me), "{said:?}");
        }
        // a quadlet is generated and carries its own [Install] section
        assert_eq!(hand_units_to_systemd(&[], false, false).len(), 2);
    }

    #[test]
    fn units_of_this_account_want_a_session_and_dockers_containers_do_not() {
        if cfg!(target_os = "linux") {
            assert_eq!(
                service_manager_when(Runtime::Machine, true),
                Some("systemd user units")
            );
            assert_eq!(
                service_manager_when(Runtime::Podman, true),
                Some("podman quadlets")
            );
            assert_eq!(
                service_manager_when(Runtime::Machine, false),
                None,
                "a binary that answers --version is not a session that takes units"
            );
            assert_eq!(service_manager_when(Runtime::Podman, false), None);
            assert_eq!(
                service_manager_when(Runtime::Docker, false),
                Some("a compose file"),
                "docker's own daemon brings its containers back"
            );
            let said = no_manager_words(Runtime::Machine, false);
            assert!(said.contains("no systemd session"), "{said}");
            assert!(said.contains("loginctl enable-linger"), "{said}");
        }
        assert_eq!(
            no_manager_words(Runtime::Machine, true),
            "no service manager here, so the commands are printed instead"
        );
    }

    /// A plan with the services of this machine, each part as its own
    /// account, in a directory that is nobody's home.
    fn deployment() -> Plan {
        let mut plan = plan(Runtime::Machine);
        plan.dir = PathBuf::from("/srv/nils");
        plan.parts = vec![Part::Engine, Part::Desk, Part::Assistant];
        plan.system = Some(SystemUnits {
            capabilities: vec![
                "CAP_DAC_OVERRIDE".to_string(),
                "CAP_DAC_READ_SEARCH".to_string(),
            ],
            accounts: BTreeMap::from([
                ("desk".to_string(), "nils-desk".to_string()),
                ("assistant".to_string(), "nils-assistant".to_string()),
            ]),
        });
        plan
    }

    /// Every part of a deployment, as its record names them.
    fn deployed_parts() -> [(&'static str, &'static str); 4] {
        [
            ("engine", "binary"),
            ("desk", "binary"),
            ("kvasir", "node"),
            ("assistant", "node"),
        ]
    }

    #[test]
    fn the_services_of_a_machine_name_an_account_and_the_engines_capabilities() {
        let plan = deployment();
        let state = state_of(&plan, &deployed_parts());
        let units = systemd_units(&plan, &state);
        let unit = |name: &str| {
            units
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| panic!("no {name} among {units:?}"))
        };

        // the engine keeps what it needs to read and write across the
        // filesystems a site mounts, which no unit of an account's own can
        let engine = unit("nils-engine.service");
        assert!(engine.contains("User=nils\n"), "{engine}");
        assert!(
            engine.contains("AmbientCapabilities=CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH"),
            "{engine}"
        );
        assert!(
            engine.contains("CapabilityBoundingSet=CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH"),
            "{engine}"
        );
        assert!(engine.contains("WantedBy=multi-user.target"), "{engine}");
        assert!(
            !engine.contains("InaccessiblePaths") && !engine.contains("ProtectHome"),
            "the engine is the part that reads the data: {engine}"
        );

        // the parts a browser reaches run as another account, and what they
        // never read is out of their reach
        let desk = unit("nils-desk.service");
        assert!(desk.contains("User=nils-desk\n"), "{desk}");
        assert!(
            !desk.contains("AmbientCapabilities"),
            "only the engine keeps capabilities: {desk}"
        );
        assert!(desk.contains("ProtectHome=yes"), "{desk}");
        assert!(
            desk.contains("InaccessiblePaths=-/srv/nils/registry"),
            "{desk}"
        );
        assert!(
            desk.contains("InaccessiblePaths=-/srv/nils/backups"),
            "{desk}"
        );
        assert!(desk.contains("InaccessiblePaths=-/data/source"), "{desk}");
        assert!(unit("kvasir.service").contains("User=nils-assistant\n"));
        assert!(unit("nils-assistant.service").contains("User=nils-assistant\n"));
        assert!(
            llama_unit(&plan, Path::new("/srv/nils/llama.cpp/b1"), "127.0.0.1")
                .contains("User=nils-assistant\n"),
            "llama.cpp runs the assistant's models"
        );

        // the supervisor restarts them and replaces their binaries, so it
        // stays with the account that installed them
        let (name, supervisor) = supervisor_service(&plan, &state);
        assert_eq!(name, "nils-supervise.service");
        assert!(!supervisor.contains("User="), "{supervisor}");
        assert!(
            supervisor.contains("WantedBy=multi-user.target"),
            "{supervisor}"
        );

        // an install inside a home is not shut out of its own files
        let mut at_home = plan.clone();
        at_home.dir = PathBuf::from("/home/you/nils");
        let state = state_of(&at_home, &deployed_parts());
        let desk = systemd_units(&at_home, &state)
            .into_iter()
            .find(|(n, _)| n == "nils-desk.service")
            .map(|(_, text)| text)
            .unwrap();
        assert!(desk.contains("User=nils-desk\n"), "{desk}");
        assert!(
            !desk.contains("ProtectHome"),
            "its own files are under a home: {desk}"
        );
    }

    #[test]
    fn a_plain_install_writes_the_units_it_always_wrote() {
        let plan = plan(Runtime::Machine);
        assert!(plan.system.is_none());
        let state = state_of(&plan, &[("engine", "binary"), ("desk", "binary")]);
        for (name, text) in systemd_units(&plan, &state) {
            assert!(!text.contains("User="), "{name}: {text}");
            assert!(!text.contains("AmbientCapabilities"), "{name}: {text}");
            assert!(!text.contains("ProtectHome"), "{name}: {text}");
            assert!(text.contains("WantedBy=default.target"), "{name}: {text}");
        }
        assert!(
            files_of(&plan).is_empty(),
            "an install of this account gives nothing away"
        );
        assert_eq!(units_dir(true), PathBuf::from("/etc/systemd/system"));
        assert!(units_dir(false).ends_with("systemd/user"));
    }

    #[test]
    fn the_services_of_a_machine_are_refused_where_they_cannot_be_had() {
        let refused = |runtime, root, systemd, missing: &[&str]| {
            let missing: Vec<String> = missing.iter().map(|m| (*m).to_string()).collect();
            system_refusal_when(runtime, root, systemd, &missing)
        };
        let said = refused(Runtime::Podman, true, true, &[]).unwrap();
        assert!(said.contains("--runtime machine"), "{said}");
        let said = refused(Runtime::Machine, true, false, &[]).unwrap();
        assert!(said.contains("no systemd"), "{said}");
        let said = refused(Runtime::Machine, false, true, &[]).unwrap();
        assert!(said.contains("is root's to do"), "{said}");
        let said = refused(Runtime::Machine, true, true, &["nils", "nils-desk"]).unwrap();
        assert!(
            said.contains("there are no accounts on this machine named nils, nils-desk"),
            "{said}"
        );
        let said = refused(Runtime::Machine, true, true, &["nils-desk"]).unwrap();
        assert!(
            said.contains("there is no account on this machine named nils-desk"),
            "the account named is the one said: {said}"
        );
        assert!(
            !said.contains("useradd"),
            "a service account is the site's own to make, and setup makes none: {said}"
        );
        assert_eq!(refused(Runtime::Machine, true, true, &[]), None);

        // what a service of this account cannot carry is not taken for one
        let said = without_system(false, true).unwrap();
        assert!(said.contains("--capabilities goes with --system"), "{said}");
        let said = without_system(true, false).unwrap();
        assert!(said.contains("--account goes with --system"), "{said}");
        assert_eq!(without_system(false, false), None);
    }

    #[test]
    fn an_account_and_a_capability_are_read_as_what_they_are() {
        let given = accounts_given(&["desk=nils-desk".to_string(), "engine=nils".to_string()])
            .expect("two parts");
        assert_eq!(given.get("desk").map(String::as_str), Some("nils-desk"));
        let system = SystemUnits {
            accounts: given,
            ..SystemUnits::default()
        };
        assert_eq!(system.account("desk"), "nils-desk");
        assert_eq!(
            system.account("assistant"),
            "nils",
            "a part not named runs as the default account"
        );
        for (bad, says) in [
            ("nils", "names no account"),
            ("gateway=nils", "is not a part"),
            ("desk=", "names no account"),
            ("desk=a b", "is not the name of an account"),
        ] {
            let refused = accounts_given(&[bad.to_string()]).unwrap_err();
            assert!(refused.contains(says), "{bad}: {refused}");
        }

        assert_eq!(
            capabilities_given("cap_dac_override, dac_read_search").unwrap(),
            vec![
                "CAP_DAC_OVERRIDE".to_string(),
                "CAP_DAC_READ_SEARCH".to_string()
            ],
            "written however a person writes them"
        );
        assert!(capabilities_given("").unwrap().is_empty());
        let refused = capabilities_given("CAP_DAC_OVERIDE").unwrap_err();
        assert!(
            refused.contains("is not a capability a service can keep"),
            "one misspelled is said here, not by a service that will not start: {refused}"
        );
    }

    #[test]
    fn the_services_a_record_names_are_the_services_an_update_writes() {
        let dir = scratch("system-record");
        let state = State {
            dir: dir.display().to_string(),
            mode: "off".to_string(),
            runtime: "machine".to_string(),
            service: SYSTEM_MANAGER.to_string(),
            reach: "loopback".to_string(),
            parts: BTreeMap::from([
                (
                    "engine".to_string(),
                    PartState {
                        version: "1.0.0".to_string(),
                        path: "/usr/local/bin/nils".to_string(),
                        kind: "binary".to_string(),
                    },
                ),
                (
                    "desk".to_string(),
                    PartState {
                        version: "1.0.0".to_string(),
                        path: "/usr/local/bin/nils-desk".to_string(),
                        kind: "binary".to_string(),
                    },
                ),
            ]),
            system: Some(SystemUnits {
                capabilities: vec!["CAP_DAC_OVERRIDE".to_string()],
                accounts: BTreeMap::from([("desk".to_string(), "nils-desk".to_string())]),
            }),
            ..State::default()
        };

        // it is written down, where a person can read it
        let written = toml::to_string(&state).unwrap();
        assert!(
            written.contains("service = \"systemd system units\""),
            "{written}"
        );
        assert!(
            written.contains("capabilities = [\"CAP_DAC_OVERRIDE\"]"),
            "{written}"
        );
        assert!(written.contains("[system.accounts]"), "{written}");
        assert!(written.contains("desk = \"nils-desk\""), "{written}");

        // an update and a repair are made from the record and nothing else
        let read: State = toml::from_str(&written).unwrap();
        let plan = plan_from_state(&read, None);
        assert_eq!(plan.system, state.system);
        let units = systemd_units(&plan, &read);
        assert!(units[0].1.contains("User=nils\n"), "{}", units[0].1);
        assert!(
            units[0].1.contains("AmbientCapabilities=CAP_DAC_OVERRIDE"),
            "{}",
            units[0].1
        );
        assert!(
            units
                .iter()
                .any(|(n, t)| n == "nils-desk.service" && t.contains("User=nils-desk\n")),
            "{units:?}"
        );
        // they go to the machine's directory, and to its manager
        let calls = unit_calls(&plan, &["nils-engine".to_string()]);
        assert!(
            !calls.iter().any(|call| call.contains("loginctl")),
            "the machine's manager is there whoever is logged in: {calls:?}"
        );
        assert!(
            calls.contains(&"systemctl daemon-reload".to_string()),
            "{calls:?}"
        );
        assert!(
            calls.contains(&"systemctl enable nils-engine".to_string()),
            "{calls:?}"
        );
        assert!(
            calls.contains(&"systemctl restart nils-engine".to_string()),
            "{calls:?}"
        );
        assert!(
            service_units(&read).iter().all(|unit| unit.system),
            "the supervisor asks the machine's manager too"
        );
        assert_eq!(install_doc(&read)["service"], SYSTEM_MANAGER);

        // and a record from before this, or one of an account's own, is
        // what it always was
        let plain = State {
            service: "systemd user units".to_string(),
            system: None,
            ..read
        };
        let plan = plan_from_state(&plain, None);
        assert!(plan.system.is_none());
        assert!(!systemd_units(&plan, &plain)[0].1.contains("User="));
        let calls = unit_calls(&plan, &["nils-engine".to_string()]);
        assert!(calls[0].starts_with("loginctl enable-linger"), "{calls:?}");
        assert!(
            calls.contains(&"systemctl --user enable nils-engine".to_string()),
            "{calls:?}"
        );
        assert!(service_units(&plain).iter().all(|unit| !unit.system));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_files_of_a_part_are_the_account_that_part_runs_as() {
        let plan = deployment();
        let files = files_of(&plan);
        let owner = |path: &str| {
            files
                .iter()
                .find(|(at, _)| at == Path::new(path))
                .map(|(_, account)| account.as_str())
        };
        // the engine writes the registry, the archives and the work in flight
        for path in [
            "/srv/nils/registry",
            "/srv/nils/backups",
            "/srv/nils/working",
            "/srv/nils/export",
        ] {
            assert_eq!(owner(path), Some("nils"), "{path}");
        }
        // the desk reads its own configuration and writes its own store, and
        // Kvasir's folder holds the key the assistant reads
        assert_eq!(owner("/srv/nils/desk"), Some("nils-desk"));
        assert_eq!(owner("/srv/nils/kvasir"), Some("nils-assistant"));
        assert_eq!(owner("/srv/nils/assistant"), Some("nils-assistant"));
        // what no part reads stays with the account that ran setup
        assert_eq!(owner("/srv/nils"), None);
        assert_eq!(owner("/srv/nils/supervise"), None);
        assert_eq!(owner("/srv/nils/key.passphrase"), None);

        // and a tree is given whole, not only its top
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let dir = scratch("system-files");
            std::fs::create_dir_all(dir.join("desk")).unwrap();
            std::fs::write(dir.join("desk").join("nils-desk.toml"), "bind = \"\"").unwrap();
            let me = std::fs::metadata(&dir).unwrap();
            give_to(&dir, me.uid(), me.gid()).expect("the tree is given");
            let file = std::fs::metadata(dir.join("desk").join("nils-desk.toml")).unwrap();
            assert_eq!((file.uid(), file.gid()), (me.uid(), me.gid()));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn print_says_the_units_of_an_install_on_this_machine() {
        if cfg!(target_os = "macos") {
            return;
        }
        let console = Console::new(true);
        let mut plan = plan(Runtime::Machine);
        plan.dir = PathBuf::from("/srv/nils");
        let said = commands_text(&plan, &console);
        assert!(said.contains("nils-engine.service"), "{said}");
        assert!(said.contains("ExecStart="), "{said}");
        assert!(
            said.contains("nils-supervise.service"),
            "the supervisor is a unit this writes too: {said}"
        );
        assert!(said.contains("systemctl --user daemon-reload"), "{said}");
        assert!(
            said.contains("systemctl --user restart nils-engine"),
            "{said}"
        );

        // the services of this machine, which are the ones worth reading
        plan.system = Some(SystemUnits {
            capabilities: vec!["CAP_DAC_OVERRIDE".to_string()],
            accounts: BTreeMap::new(),
        });
        let said = commands_text(&plan, &console);
        assert!(said.contains("/etc/systemd/system"), "{said}");
        assert!(said.contains("User=nils"), "{said}");
        assert!(
            said.contains("AmbientCapabilities=CAP_DAC_OVERRIDE"),
            "{said}"
        );
        assert!(said.contains("systemctl daemon-reload"), "{said}");
        assert!(
            !said.contains("--user"),
            "these are the machine's, not an account's: {said}"
        );

        // an install that writes none says how to start it by hand
        plan.service = false;
        let said = commands_text(&plan, &console);
        assert!(said.contains("nils serve --bind"), "{said}");
        assert!(!said.contains("ExecStart="), "{said}");
    }

    #[test]
    fn every_source_place_is_mounted_and_handed_to_the_engine() {
        let mut p = plan(Runtime::Podman);
        p.sources = vec![
            ("source".to_string(), PathBuf::from("/data/before")),
            ("scanner2".to_string(), PathBuf::from("/data/two")),
        ];
        let engine = &podman_commands(&p)[1];
        assert!(
            engine.contains("-v /data/source:/data/source:ro"),
            "{engine}"
        );
        assert!(engine.contains("-v /data/two:/data/two:ro"), "{engine}");
        assert!(
            !engine.contains("/data/before"),
            "the directory named now is the source, not the place's old path: {engine}"
        );
        assert!(
            engine.contains("--ingest-root source=/data/source --ingest-root scanner2=/data/two"),
            "{engine}"
        );
        p.runtime = Runtime::Docker;
        let engine = &docker_commands(&p)[1];
        assert!(engine.contains("-v /data/two:/data/two:ro"), "{engine}");
        let compose = docker_compose(&p);
        assert!(
            compose.contains("      - /data/two:/data/two:ro"),
            "{compose}"
        );
        assert!(
            plan_rows(&p)
                .iter()
                .any(|(key, value)| *key == "reads" && value.contains("/data/two")),
            "the summary names every directory read"
        );
    }

    #[test]
    fn a_provider_is_trusted_by_the_desk_the_engine_and_the_gateway() {
        let dir = scratch("provider");
        let mut plan = plan(Runtime::Podman);
        plan.dir = dir.clone();
        plan.mode = Mode::Oidc;
        assert!(
            !engine_args(&plan, "/r", "/b")
                .join(" ")
                .contains("--oidc-trust"),
            "no provider is named yet"
        );
        plan.oidc = registered_at(
            "the desk's [oidc] table:\n  issuer = \"https://auth.example.org/application/o/nils/\"\n  client_id = \"abc123\"\n  client_secret_file = \"/x/client-secret\"\n",
        );
        let oidc = plan.oidc.clone().expect("the table names a provider");
        assert_eq!(
            oidc.jwks,
            "https://auth.example.org/application/o/nils/jwks/"
        );
        let engine = engine_args(&plan, "/r", "/b").join(" ");
        assert!(
            engine.contains(
                "--auth oidc --oidc-trust issuer=https://auth.example.org/application/o/nils/,audience=abc123,jwks=https://auth.example.org/application/o/nils/jwks/ --oidc-groups-claim roles --role reader=reader"
            ),
            "{engine}"
        );
        let auth = kvasir_auth(&plan, "tok");
        assert_eq!(auth["mode"], "oidc", "{auth}");
        assert_eq!(auth["trust"][0]["audience"], "abc123", "{auth}");
        let (issuer, jwks) = desk_trust(&plan);
        assert!(
            engine.contains(&format!(
                "--oidc-trust issuer={issuer},audience=nils,jwks={jwks}"
            )),
            "the engine trusts what the desk signs for a person the provider named: {engine}"
        );
        assert_eq!(auth["trust"][1]["issuer"], issuer.as_str(), "{auth}");
        assert_eq!(auth["trust"][1]["audience"], "nils", "{auth}");
        assert_eq!(auth["trust"][1]["keepSubject"], true, "{auth}");
        assert_eq!(
            auth["trust"][0].get("keepSubject"),
            None,
            "the provider's subjects are qualified by its host: {auth}"
        );
        assert!(
            engine.contains(&format!("jwks={jwks},keep_subject=true")),
            "{engine}"
        );
        assert_eq!(auth["tokens"]["tok"], INSTALLER, "{auth}");
        assert!(write_desk_config(&plan).is_ok());
        let desk = std::fs::read_to_string(plan.desk_config()).unwrap();
        assert!(
            desk.contains("[oidc]\nissuer = \"https://auth.example.org/application/o/nils/\"\nclient_id = \"abc123\"\nclient_secret_file = \"client-secret\""),
            "{desk}"
        );
        assert!(!desk.contains("# [oidc]"), "{desk}");
        assert_eq!(desk_config_fate(&plan), DeskConfigFate::Kept);
        plan.oidc = Some(OidcPlan {
            client_id: "another".into(),
            ..oidc
        });
        assert_eq!(
            desk_config_fate(&plan),
            DeskConfigFate::Updated,
            "another client is written again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_sets_what_setup_writes_in_the_desks_configuration_and_keeps_the_rest() {
        let dir = scratch("desk-config-merge");
        let mut p = plan(Runtime::Machine);
        p.dir = dir.clone();
        let off = desk_config_text(&p);
        assert_eq!(desk_config_merged(&off, &off), Ok(None), "nothing changed");

        // a person set a key of their own, and the desk moves to keeping its people
        let hand = format!("session_hours = 4\n{off}");
        p.mode = Mode::Local;
        let local = desk_config_merged(&hand, &desk_config_text(&p))
            .unwrap()
            .expect("the mode changed");
        let table: toml::Table = toml::from_str(&local).unwrap();
        assert_eq!(table["mode"].as_str(), Some("local"), "{local}");
        assert_eq!(table["local"]["audience"].as_str(), Some("nils"), "{local}");
        assert_eq!(table["session_hours"].as_integer(), Some(4), "{local}");
        assert_eq!(
            desk_config_merged(&local, &desk_config_text(&p)),
            Ok(None),
            "written again with nothing more changed"
        );

        // opened to the network: where it binds and where it answers move with it
        p.reach = Reach::Network("lab.example.org".to_string());
        let open = desk_config_merged(&local, &desk_config_text(&p))
            .unwrap()
            .expect("the reach changed");
        let table: toml::Table = toml::from_str(&open).unwrap();
        assert_eq!(
            table["origin"].as_str(),
            Some("http://lab.example.org:7200")
        );
        assert_eq!(table["bind"].as_str(), Some("0.0.0.0:7200"));

        // the assistant added and taken away: its tables come and go with it
        p.parts.push(Part::Assistant);
        let with = desk_config_merged(&open, &desk_config_text(&p))
            .unwrap()
            .expect("the assistant came");
        let table: toml::Table = toml::from_str(&with).unwrap();
        assert!(
            table.contains_key("kvasir") && table.contains_key("assistant"),
            "{with}"
        );
        p.parts.retain(|part| *part != Part::Assistant);
        let without = desk_config_merged(&with, &desk_config_text(&p))
            .unwrap()
            .expect("the assistant went");
        let table: toml::Table = toml::from_str(&without).unwrap();
        assert!(
            !table.contains_key("kvasir") && !table.contains_key("assistant"),
            "{without}"
        );
        assert_eq!(table["session_hours"].as_integer(), Some(4), "{without}");

        // a file that no longer reads is written again whole, and the plan says
        // so before anything is changed
        assert_eq!(desk_config_merged("mode = ", &off), Err(()));
        assert_eq!(desk_config_fate(&p), DeskConfigFate::New, "no file yet");
        std::fs::create_dir_all(p.desk_dir()).unwrap();
        std::fs::write(p.desk_config(), "mode = ").unwrap();
        assert_eq!(desk_config_fate(&p), DeskConfigFate::Replaced);
        assert!(write_desk_config(&p).is_ok());
        assert_eq!(desk_config_fate(&p), DeskConfigFate::Kept);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_desk_keeps_people_only_once_one_is_added() {
        let dir = scratch("desk-people");
        let mut p = plan(Runtime::Machine);
        p.dir = dir.clone();
        p.mode = Mode::Local;
        assert!(!desk_has_people(&dir), "no store, nobody");
        std::fs::create_dir_all(dir.join("desk")).unwrap();
        let conn = rusqlite::Connection::open(dir.join("desk").join("nils-desk.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE user (username TEXT PRIMARY KEY, password_hash TEXT NOT NULL, display TEXT NOT NULL, entitlements TEXT NOT NULL DEFAULT '[]', admin INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);",
        )
        .unwrap();
        // the store a desk that ran with nobody signing in has: there, and empty
        assert!(!desk_has_people(&dir));
        assert!(
            nobody_yet(&p).is_some_and(|line| line.contains("nils-desk user add <name> --admin")),
            "the end of setup says how to add the first"
        );
        conn.execute(
            "INSERT INTO user VALUES ('anna', 'x', 'Anna', '[]', 1, '2026-09-13T00:00:00Z')",
            [],
        )
        .unwrap();
        assert!(desk_has_people(&dir));
        assert!(nobody_yet(&p).is_none());
        p.mode = Mode::Off;
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_supervisor_names_each_part_by_its_unit_and_says_where_it_answers() {
        let mut state = State {
            dir: "/home/x/nils".to_string(),
            mode: "local".to_string(),
            runtime: "docker".to_string(),
            service: "a compose file".to_string(),
            reach: "10.0.0.5".to_string(),
            backend: "postgres:nils".to_string(),
            ports: Ports::default(),
            ..State::default()
        };
        for (name, kind) in [
            ("engine", "docker"),
            ("desk", "docker"),
            ("kvasir", "node"),
            ("assistant", "node"),
            ("postgres", "docker"),
        ] {
            state.parts.insert(
                name.to_string(),
                PartState {
                    version: "1.0.0-alpha.14".to_string(),
                    path: "a path".to_string(),
                    kind: kind.to_string(),
                },
            );
        }
        let units: Vec<(&str, String, &str)> = service_units(&state)
            .into_iter()
            .map(|u| (u.part, u.name, u.watcher))
            .collect();
        assert_eq!(
            units,
            vec![
                ("postgres", "nils-postgres".to_string(), "docker"),
                ("engine", "nils-engine".to_string(), "docker"),
                ("desk", "nils-desk".to_string(), "docker"),
                ("gateway", "nils-kvasir".to_string(), "docker"),
                ("assistant", "nils-assistant".to_string(), "docker"),
            ],
            "in the order the parts start"
        );
        let at = addresses(&state);
        let of = |at: &[serde_json::Value], part: &str| {
            at.iter().find(|a| a["part"] == part).cloned().unwrap()
        };
        assert_eq!(of(&at, "engine")["address"], "nils-engine:8437");
        assert_eq!(of(&at, "engine")["reach"], "inside docker only");
        assert_eq!(of(&at, "gateway")["address"], "127.0.0.1:7100");
        assert_eq!(of(&at, "desk")["reach"], "this network");
        assert!(
            of(&at, "desk")["address"]
                .as_str()
                .unwrap()
                .contains("10.0.0.5"),
            "{at:?}"
        );

        state.runtime = "podman".to_string();
        let units: Vec<String> = service_units(&state)
            .into_iter()
            .map(|u| format!("{} {}", u.name, u.watcher))
            .collect();
        assert!(
            units.contains(&"nils-kvasir systemd".to_string()),
            "{units:?}"
        );
        let at = addresses(&state);
        assert_eq!(of(&at, "engine")["address"], "127.0.0.1:8437");
        assert_eq!(of(&at, "assistant")["reach"], "inside the pod only");

        state.service = "none".to_string();
        assert!(service_units(&state).is_empty());
        assert!(
            restart_units(&state, None).is_err(),
            "no services, nothing to restart"
        );
    }

    #[test]
    fn the_supervisor_is_written_for_the_desk_to_reach_from_where_the_desk_runs() {
        let dir = scratch("supervisor");
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        let token = write_supervisor(&plan).unwrap_or_else(|e| panic!("{}", e.message));
        let path = dir.join("supervise").join("supervise.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("bind = \"127.0.0.1:8470\""), "{text}");
        assert!(
            text.contains(&format!("\"{token}\" = \"nils-desk\"")),
            "{text}"
        );
        assert!(dir.join("supervise").join("trust.pub").exists());
        assert_eq!(
            write_supervisor(&plan).unwrap_or_else(|e| panic!("{}", e.message)),
            token,
            "a token made before is kept"
        );
        assert_eq!(
            supervisor_url(&plan).as_deref(),
            Some("http://127.0.0.1:8470")
        );

        assert!(write_desk_config(&plan).is_ok());
        let desk = std::fs::read_to_string(plan.desk_config()).unwrap();
        assert!(
            desk.contains(&format!(
                "[supervisor]\nurl = \"http://127.0.0.1:8470\"\ntoken = \"{token}\""
            )),
            "{desk}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for file in [&path, &plan.desk_config()] {
                assert_eq!(
                    std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{}",
                    file.display()
                );
            }
        }
        assert_eq!(desk_config_fate(&plan), DeskConfigFate::Kept);

        plan.runtime = Runtime::Docker;
        assert_eq!(
            supervisor_url(&plan).as_deref(),
            Some("http://host.docker.internal:8470")
        );
        let compose = docker_compose(&plan);
        let desk_service = &compose[compose.find("  desk:").unwrap()..];
        assert!(
            desk_service.contains("host.docker.internal:host-gateway"),
            "{compose}"
        );
        plan.runtime = Runtime::Podman;
        plan.host_loopback = false;
        assert_eq!(
            supervisor_url(&plan),
            None,
            "a pod without this host's loopback does not reach it"
        );
        plan.host_loopback = true;
        assert_eq!(
            supervisor_url(&plan).as_deref(),
            Some("http://169.254.1.2:8470")
        );
        if !cfg!(target_os = "macos") {
            let (name, unit) = supervisor_service(&plan, &State::default());
            assert_eq!(name, "nils-supervise.service");
            assert!(
                unit.contains("supervise run --config") && unit.contains("KillMode=process"),
                "{unit}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn podman_runs_a_pod_and_owns_its_mounts() {
        let p = plan(Runtime::Podman);
        let commands = podman_commands(&p);
        assert!(commands[0].contains("pod create --name nils -p 127.0.0.1:7200:7200"));
        let engine = &commands[1];
        assert!(engine.contains("--pod nils"), "{engine}");
        // at the paths the registry's places record, so a backup finds its place
        let (registry, backups) = (p.registry(), p.dir.join("backups"));
        assert!(
            engine.contains(&format!("-v {0}:{0}:U", registry.display())),
            "{engine}"
        );
        assert!(
            engine.contains(&format!("-v {0}:{0}:U", backups.display())),
            "{engine}"
        );
        assert!(
            engine.contains(&format!(
                "--registry {} --backup-dir {}",
                registry.display(),
                backups.display()
            )),
            "{engine}"
        );
        assert!(
            engine.contains("-v /data/source:/data/source:ro"),
            "{engine}"
        );
        assert!(
            engine.contains("ghcr.io/kineuro/nils:v1.0.0-alpha.2"),
            "{engine}"
        );
        assert!(engine.contains("--bind 0.0.0.0:8437"), "{engine}");
        assert!(commands[2].contains("ghcr.io/kineuro/nils-desk:v1.0.0-alpha.2"));
    }

    #[test]
    #[cfg(unix)]
    fn docker_runs_a_network_and_does_not_remap_the_user() {
        let commands = docker_commands(&plan(Runtime::Docker));
        assert_eq!(commands[0], "docker network create nils");
        assert!(commands[1].contains("--network nils"), "{}", commands[1]);
        assert!(!commands[1].contains(":U"), "docker owns its own mounts");
        // Podman remaps the user and `:U` hands the mount over. Docker does
        // neither, so a container running as the image's own user cannot
        // write the directory it was given, and the first thing it writes
        // is the registry's key. Every docker run is told to be this
        // account instead.
        let me = as_this_account();
        assert!(!me.is_empty(), "a unix account is a uid and a gid");
        assert!(
            commands[1].contains(&format!("--user {me}")),
            "{}",
            commands[1]
        );
        assert!(
            commands[2].contains(&format!("--user {me}")),
            "{}",
            commands[2]
        );
        let compose = docker_compose(&plan(Runtime::Docker));
        assert_eq!(
            compose.matches(&format!("user: \"{me}\"")).count(),
            2,
            "both services run as this account:\n{compose}"
        );
        assert!(
            !podman_commands(&plan(Runtime::Podman))
                .iter()
                .any(|c| c.contains("--user")),
            "podman remaps on its own and needs no --user"
        );
        assert!(commands[2].contains("-p 127.0.0.1:7200:7200"));
        let compose = docker_compose(&plan(Runtime::Docker));
        assert!(compose.contains("image: ghcr.io/kineuro/nils:v1.0.0-alpha.2"));
        assert!(compose.contains("container_name: nils-desk"));
        assert!(compose.contains("/data/source:/data/source:ro"));
    }

    #[test]
    fn the_quadlets_name_the_pod_and_the_images() {
        let files = quadlets(&plan(Runtime::Podman));
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["nils.pod", "nils-engine.container", "nils-desk.container"]
        );
        assert!(files[0].1.contains("PublishPort=127.0.0.1:7200:7200"));
        assert!(files[1].1.contains("Pod=nils.pod"));
        assert!(
            files[1]
                .1
                .contains("Image=ghcr.io/kineuro/nils:v1.0.0-alpha.2")
        );
        assert!(files[1].1.contains(":U"), "a rootless mount is owned");
    }

    #[test]
    fn an_image_is_named_by_the_tag_the_release_pushed() {
        // The release pushes ghcr.io/kineuro/nils:<git tag>, and a git tag
        // begins with a v. Every version this wizard holds has had that v
        // taken off by the updater, so a name built from the version alone
        // asks for a tag that was never pushed: the pull fails, the wizard
        // falls back to a local build, and nobody is told why.
        assert_eq!(image_tag("1.0.0-alpha.2"), "v1.0.0-alpha.2");
        assert_eq!(image_tag("v1.0.0-alpha.2"), "v1.0.0-alpha.2");
    }

    #[test]
    fn the_places_are_declared_in_an_order_the_registry_can_name() {
        let plan = plan(Runtime::Machine);
        let specs = place_specs(&plan);
        let names: Vec<&str> = specs.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["backups", "source", "registry", "working", "export"],
            "the backup place exists before the registry names it"
        );
        let registry = specs.iter().find(|s| s.name == "registry").unwrap();
        assert_eq!(place_argv(registry).last().unwrap(), "backups");
        assert!(place_argv(registry).contains(&"--role".to_string()));
    }

    fn state_of(plan: &Plan, kinds: &[(&str, &str)]) -> State {
        let mut state = State {
            dir: plan.dir.display().to_string(),
            mode: plan.mode.name().to_string(),
            runtime: plan.runtime.name().to_string(),
            ports: plan.ports,
            ..State::default()
        };
        for (name, kind) in kinds {
            state.parts.insert(
                (*name).to_string(),
                PartState {
                    version: "1.0.0".to_string(),
                    path: format!("/opt/nils/{name}"),
                    kind: (*kind).to_string(),
                },
            );
        }
        state
    }

    #[test]
    fn the_units_of_a_machine_run_name_the_binaries_that_were_installed() {
        let plan = plan(Runtime::Machine);
        let state = state_of(&plan, &[("engine", "binary"), ("desk", "binary")]);
        let units = systemd_units(&plan, &state);
        let names: Vec<&str> = units.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["nils-engine.service", "nils-desk.service"]);
        assert!(
            units[0].1.contains("ExecStart=/opt/nils/engine serve"),
            "{}",
            units[0].1
        );
        assert!(
            units[0].1.contains("--bind 127.0.0.1:8437"),
            "{}",
            units[0].1
        );
        assert!(
            units[0].1.contains("--ingest-root source=/data/source"),
            "{}",
            units[0].1
        );
        assert!(
            units[1]
                .1
                .contains("ExecStart=/opt/nils/desk serve --config"),
            "{}",
            units[1].1
        );
        assert!(
            units
                .iter()
                .all(|(_, t)| t.contains("WantedBy=default.target"))
        );
    }

    #[test]
    fn the_assistant_starts_from_its_own_entry_when_the_checkout_has_one() {
        let dir = scratch("assistant-entry");
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        let state = state_of(&plan, &[("engine", "binary"), ("assistant", "node")]);
        let unit = |plan: &Plan| {
            systemd_units(plan, &state)
                .into_iter()
                .find(|(n, _)| n == "nils-assistant.service")
                .map(|(_, t)| t)
                .unwrap()
        };
        // a checkout from before the entry: Flue's, which is all there is
        std::fs::create_dir_all(dir.join("assistant")).unwrap();
        let before = unit(&plan);
        assert!(
            before.contains("ExecStart=/usr/bin/env node dist/app/server.mjs"),
            "{before}"
        );
        // one with it: the entry that listens on loopback and stops cleanly
        std::fs::create_dir_all(dir.join("assistant").join("bin")).unwrap();
        std::fs::write(dir.join("assistant").join("bin").join("serve.mjs"), "").unwrap();
        let after = unit(&plan);
        assert!(
            after.contains("ExecStart=/usr/bin/env node bin/serve.mjs"),
            "{after}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_model_on_this_machine_is_dialled_the_way_a_container_reaches_it() {
        let local = "http://127.0.0.1:30000/v1";
        assert_eq!(model_address_for(Runtime::Machine, local), local);
        assert_eq!(
            model_address_for(Runtime::Podman, local),
            "http://host.containers.internal:30000/v1"
        );
        assert_eq!(
            model_address_for(Runtime::Docker, "http://localhost:8000/v1"),
            "http://host.docker.internal:8000/v1"
        );
        // somewhere else is dialled as it was typed, wherever the gateway runs
        for url in [
            "http://192.168.1.20:30000/v1",
            "https://api.openai.com/v1",
            "http://127.0.0.10:1/v1",
            "http://localhost.example.org/v1",
        ] {
            assert_eq!(model_address_for(Runtime::Podman, url), url);
        }
        // and back, for a setup moved from containers to the machine
        assert_eq!(
            model_address_on_machine("http://host.containers.internal:30000/v1"),
            local
        );
        assert!(model_reach_note(Runtime::Machine, local, || true).is_none());
        assert!(model_reach_note(Runtime::Docker, "https://api.openai.com/v1", || true).is_none());
        let docker = model_reach_note(Runtime::Docker, local, || true).unwrap();
        assert!(docker.contains("0.0.0.0"), "{docker}");
        let no_pasta = model_reach_note(Runtime::Podman, local, || false).unwrap();
        assert!(no_pasta.contains("pasta"), "{no_pasta}");
    }

    #[test]
    fn in_a_pod_the_gateway_and_the_assistant_run_beside_the_others() {
        let dir = scratch("pod-assistant");
        let mut plan = plan(Runtime::Podman);
        plan.dir = dir.clone();
        plan.parts.push(Part::Assistant);
        plan.host_loopback = true;
        let files = quadlets(&plan);
        let named = |name: &str| {
            files
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let pod = named("nils.pod");
        assert!(pod.contains("PublishPort=127.0.0.1:7200:7200"), "{pod}");
        assert!(
            pod.contains("PublishPort=127.0.0.1:7100:7100"),
            "the gateway on this machine's loopback, for the key: {pod}"
        );
        assert!(
            pod.contains("Network=pasta:--map-host-loopback=169.254.1.2"),
            "{pod}"
        );
        assert!(
            !pod.contains("7300"),
            "the assistant is the desk's alone: {pod}"
        );
        let kvasir = named("nils-kvasir.container");
        let k = dir.join("kvasir");
        assert!(kvasir.contains(&format!("Image={NODE_IMAGE}")), "{kvasir}");
        assert!(kvasir.contains("Pod=nils.pod"), "{kvasir}");
        assert!(
            kvasir.contains(&format!("Volume={0}:{0}\n", k.display())),
            "mounted at the same path, so kvasir.json means the same: {kvasir}"
        );
        assert!(kvasir.contains("Exec=node dist/main.js --config kvasir.json"));
        let assistant = named("nils-assistant.container");
        let a = dir.join("assistant");
        assert!(
            assistant.contains(&format!("EnvironmentFile={}/assistant.env", a.display())),
            "{assistant}"
        );
        assert!(
            assistant.contains(&format!("Volume={0}:{0}:ro", k.display())),
            "the key, read only: {assistant}"
        );
        assert!(
            assistant.contains("After=nils-kvasir.service"),
            "{assistant}"
        );
        assert!(
            assistant.contains(&format!(
                "ConditionPathExists={}/assistant.key",
                k.display()
            )),
            "the pod starts its containers, so the assistant waits for its key: {assistant}"
        );
        // without the assistant, the pod is what it was
        plan.parts.retain(|p| *p != Part::Assistant);
        plan.host_loopback = false;
        let pod = quadlets(&plan).remove(0).1;
        assert!(!pod.contains("7100") && !pod.contains("Network="), "{pod}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn on_docker_each_part_is_reached_by_its_name() {
        let dir = scratch("docker-assistant");
        let mut plan = plan(Runtime::Docker);
        plan.dir = dir.clone();
        plan.parts.push(Part::Assistant);
        let compose = docker_compose(&plan);
        for want in [
            "container_name: nils-kvasir",
            "container_name: nils-assistant",
            &format!("image: {NODE_IMAGE}"),
            "\"127.0.0.1:7100:7100\"",
            "\"host.docker.internal:host-gateway\"",
            "depends_on: [engine, kvasir]",
        ] {
            assert!(compose.contains(want), "{want} is not in:\n{compose}");
        }
        assert!(
            !compose.contains(":7300"),
            "the assistant is published nowhere:\n{compose}"
        );

        assert!(write_desk_config(&plan).is_ok());
        let desk = std::fs::read_to_string(plan.desk_config()).unwrap();
        assert!(desk.contains("url = \"http://nils-kvasir:7100\""), "{desk}");
        assert!(
            desk.contains("url = \"http://nils-assistant:7300\""),
            "{desk}"
        );

        std::fs::create_dir_all(dir.join("assistant")).unwrap();
        assert!(write_assistant_env(&plan, None).is_ok());
        let env_path = dir.join("assistant").join("assistant.env");
        let env = std::fs::read_to_string(&env_path).unwrap();
        assert!(env.contains("NILS_URL=http://nils-engine:8437"), "{env}");
        assert!(env.contains("KVASIR_URL=http://nils-kvasir:7100"), "{env}");
        assert!(env.contains("HOST=0.0.0.0"), "{env}");

        // the same setup moved to a pod: the addresses follow, the rest stays
        std::fs::write(&env_path, format!("{env}ASSISTANT_MODEL=mine\n")).unwrap();
        plan.runtime = Runtime::Podman;
        assert!(write_assistant_env(&plan, None).is_ok());
        let moved = std::fs::read_to_string(&env_path).unwrap();
        assert!(moved.contains("NILS_URL=http://127.0.0.1:8437"), "{moved}");
        assert!(!moved.contains("HOST="), "{moved}");
        assert!(moved.contains("ASSISTANT_MODEL=mine"), "{moved}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_postgres_on_this_machine_is_named_the_way_a_container_reaches_it() {
        let url = "postgres://nils:secret@127.0.0.1:5432/nils?sslmode=disable";
        assert_eq!(dsn_for(Runtime::Machine, url), url);
        assert_eq!(
            dsn_for(Runtime::Docker, url),
            "postgres://nils:secret@host.docker.internal:5432/nils?sslmode=disable"
        );
        assert_eq!(
            dsn_for(Runtime::Podman, "postgresql://localhost/nils"),
            "postgresql://host.containers.internal/nils"
        );
        assert_eq!(
            dsn_for(Runtime::Podman, "postgres://nils@[::1]:5433/nils"),
            "postgres://nils@host.containers.internal:5433/nils"
        );
        assert_eq!(
            dsn_for(
                Runtime::Docker,
                "host=localhost port=5432 user=nils dbname=nils"
            ),
            "host=host.docker.internal port=5432 user=nils dbname=nils"
        );
        // somewhere else, a socket, or a look-alike is left as it was
        for other in [
            "postgres://nils@db.example.org/nils",
            "postgres://127.0.0.1@db.example.org/nils",
            "postgres://nils@127.0.0.10/nils",
            "host=/var/run/postgresql user=nils",
        ] {
            assert_eq!(dsn_for(Runtime::Docker, other), other);
        }
        assert!(postgres_reach_note(Runtime::Machine, url, || true).is_none());
        let docker = postgres_reach_note(Runtime::Docker, url, || true).unwrap();
        assert!(docker.contains("pg_hba.conf"), "{docker}");
        let no_pasta = postgres_reach_note(Runtime::Podman, url, || false).unwrap();
        assert!(no_pasta.contains("pasta"), "{no_pasta}");
    }

    #[test]
    fn a_pod_or_a_container_reaches_a_postgres_on_this_machine() {
        let mut plan = plan(Runtime::Podman);
        plan.backend = BackendChoice::Postgres {
            dsn: "postgres://nils@127.0.0.1/nils".to_string(),
            schema: "nils".to_string(),
        };
        plan.host_loopback = true;
        let pod = quadlets(&plan).remove(0).1;
        assert!(
            pod.contains("Network=pasta:--map-host-loopback=169.254.1.2"),
            "the pod gets this machine's loopback without the assistant too: {pod}"
        );
        assert!(podman_commands(&plan)[0].contains("--network pasta:--map-host-loopback"));

        plan.runtime = Runtime::Docker;
        let compose = docker_compose(&plan);
        let engine = compose.split("  desk:").next().unwrap_or_default();
        assert!(
            engine.contains("host.docker.internal:host-gateway"),
            "{compose}"
        );
        assert!(docker_commands(&plan)[1].contains("--add-host host.docker.internal:host-gateway"));
        plan.backend = BackendChoice::Sqlite;
        let compose = docker_compose(&plan);
        let engine = compose.split("  desk:").next().unwrap_or_default();
        assert!(
            !engine.contains("host-gateway"),
            "the engine reaches the host only for its Postgres; the desk does, for the supervisor: {compose}"
        );

        // a repair reads back what the install wrote into its registry
        let dir = scratch("registry-dsn");
        std::fs::create_dir_all(dir.join("registry")).unwrap();
        std::fs::write(
            dir.join("registry").join("nils.toml"),
            "backend = \"postgres\"\ndsn = \"postgres://nils@host.containers.internal/nils\"\n",
        )
        .unwrap();
        assert_eq!(
            registry_dsn(&dir).as_deref(),
            Some("postgres://nils@host.containers.internal/nils")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_install_that_did_not_finish_says_so_in_its_record() {
        let mut state = State {
            dir: "/home/x/nils".to_string(),
            mode: "off".to_string(),
            unfinished: true,
            ..State::default()
        };
        let text = toml::to_string(&state).unwrap();
        assert!(text.contains("unfinished = true"), "{text}");
        let read: State = toml::from_str(&text).unwrap();
        assert!(read.unfinished);
        assert!(describe_state(&read).starts_with("an install that did not finish"));
        state.unfinished = false;
        let text = toml::to_string(&state).unwrap();
        assert!(
            !text.contains("unfinished"),
            "a finished record says nothing: {text}"
        );
        let older: State = toml::from_str("dir = \"/x\"\nmode = \"off\"\n").unwrap();
        assert!(
            !older.unfinished,
            "a record from before the marker is finished"
        );
    }

    #[test]
    fn only_the_empty_directories_setup_makes_may_go_without_a_registry() {
        let home = scratch("empty-setup-dirs");
        let dir = home.join("nils");
        for sub in ["registry", "desk", "backups", "working", "export"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        assert!(only_empty_setup_dirs(&dir));
        assert!(
            safe_to_purge(&dir, Some(&home)).is_ok(),
            "nothing in it to lose"
        );

        std::fs::write(dir.join("working").join("notes.txt"), "mine").unwrap();
        assert!(
            !only_empty_setup_dirs(&dir),
            "a file in one of them is data"
        );
        std::fs::remove_file(dir.join("working").join("notes.txt")).unwrap();
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        assert!(
            !only_empty_setup_dirs(&dir),
            "a directory setup does not make"
        );
        assert!(!only_empty_setup_dirs(&home.join("absent")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn with_no_record_only_what_a_setup_leaves_in_its_places_is_offered() {
        let root = scratch("leftovers");
        let home = root.join("home");
        let bin = root.join("prefix").join("bin");
        let packs = root.join("prefix").join("share").join("nils").join("packs");
        for dir in [
            &bin,
            &packs.join("mri"),
            &packs.join("clinical"),
            &packs.join("mine"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for sub in ["registry", "desk"] {
            std::fs::create_dir_all(home.join("nils").join(sub)).unwrap();
        }
        std::fs::write(bin.join("nils"), "").unwrap();
        std::fs::write(bin.join("nils-desk"), "").unwrap();

        let found = leftovers(Some(&bin.join("nils")), Some(&home));
        assert_eq!(
            found,
            vec![
                home.join("nils"),
                bin.join("nils-desk"),
                packs.join("mri"),
                packs.join("clinical"),
                bin.join("nils"),
            ],
            "the base directory first, this program last, a person's own pack never"
        );

        // a base directory with data, or a program with another name, is not offered
        std::fs::write(home.join("nils").join("registry").join("registry.db"), "x").unwrap();
        std::fs::write(bin.join("other"), "").unwrap();
        let found = leftovers(Some(&bin.join("other")), Some(&home));
        assert!(found.is_empty(), "{found:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_postgres_is_offered_where_a_container_can_run_it() {
        assert_eq!(
            managed_runtime(Runtime::Machine, true, true),
            Some(Runtime::Podman)
        );
        assert_eq!(
            managed_runtime(Runtime::Machine, false, true),
            Some(Runtime::Docker)
        );
        assert_eq!(managed_runtime(Runtime::Machine, false, false), None);
        assert_eq!(
            managed_runtime(Runtime::Docker, true, true),
            Some(Runtime::Docker)
        );
        assert_eq!(
            managed_runtime(Runtime::Podman, false, false),
            Some(Runtime::Podman)
        );
        assert_eq!(
            dsn_password("postgres://nils:s3cret@127.0.0.1:5432/nils"),
            Some("s3cret")
        );
        assert_eq!(dsn_password("postgres://nils@127.0.0.1/nils"), None);
    }

    #[test]
    fn a_postgres_set_up_here_runs_standalone_with_its_data_in_the_base_directory() {
        let dir = scratch("managed-postgres");
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.ports.postgres = 5433;
        plan.postgres = Some(ManagedPostgres {
            runtime: Runtime::Podman,
        });
        let quadlet = postgres_quadlet(&plan);
        for want in [
            &format!("Image={POSTGRES_IMAGE}"),
            "ContainerName=nils-postgres",
            &format!("EnvironmentFile={}/postgres.env", dir.display()),
            &format!(
                "Volume={}/postgres:/var/lib/postgresql/data:U",
                dir.display()
            ),
            "PublishPort=127.0.0.1:5433:5432",
            "WantedBy=default.target",
        ] {
            assert!(quadlet.contains(want), "{want} is not in:\n{quadlet}");
        }
        assert!(
            !quadlet.contains("Pod="),
            "standalone, reached on loopback: {quadlet}"
        );

        let state = state_of(&plan, &[("engine", "binary")]);
        let engine = systemd_units(&plan, &state).remove(0).1;
        assert!(engine.contains("After=nils-postgres.service"), "{engine}");
        let mut in_pod = self::plan(Runtime::Podman);
        in_pod.postgres = plan.postgres;
        let engine_quadlet = quadlets(&in_pod)
            .into_iter()
            .find(|(n, _)| n == "nils-engine.container")
            .unwrap()
            .1;
        assert!(
            engine_quadlet.contains("Wants=nils-postgres.service"),
            "{engine_quadlet}"
        );

        plan.postgres = Some(ManagedPostgres {
            runtime: Runtime::Docker,
        });
        let machine = postgres_docker_run(&plan, None).join(" ");
        assert!(machine.contains("--restart unless-stopped"), "{machine}");
        assert!(machine.contains("-p 127.0.0.1:5433:5432"), "{machine}");
        assert!(!machine.contains("172.17.0.1"), "{machine}");
        let in_docker = postgres_docker_run(&plan, Some("172.17.0.1")).join(" ");
        assert!(in_docker.contains("-p 172.17.0.1:5433:5432"), "{in_docker}");

        std::fs::write(
            dir.join("postgres.env"),
            "POSTGRES_USER=nils\nPOSTGRES_PASSWORD=kept\n",
        )
        .unwrap();
        assert_eq!(
            postgres_password(&dir.join("postgres.env")).as_deref(),
            Some("kept")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recorded_postgres_is_rebuilt_kept_on_update_and_removed_with_the_rest() {
        let mut state = State {
            dir: "/home/x/nils".to_string(),
            mode: "off".to_string(),
            runtime: "machine".to_string(),
            backend: "postgres:nils".to_string(),
            ..State::default()
        };
        state.parts.insert(
            "postgres".to_string(),
            PartState {
                version: POSTGRES_MAJOR.to_string(),
                path: POSTGRES_IMAGE.to_string(),
                kind: "docker".to_string(),
            },
        );
        let plan = plan_from_state(&state, None);
        assert_eq!(
            plan.postgres,
            Some(ManagedPostgres {
                runtime: Runtime::Docker
            })
        );
        let removal = gather_removal(&state, None, Leaving::Purge);
        assert_eq!(removal.postgres.as_deref(), Some("docker"));
    }

    #[test]
    fn a_kept_registry_says_where_it_is_kept() {
        let dir = scratch("registry-backend");
        std::fs::create_dir_all(dir.join("registry")).unwrap();
        assert_eq!(registry_backend(&dir), None, "no registry yet");
        std::fs::write(
            dir.join("registry").join("nils.toml"),
            "backend = \"sqlite\"\n",
        )
        .unwrap();
        assert_eq!(registry_backend(&dir), None);
        std::fs::write(
            dir.join("registry").join("nils.toml"),
            "backend = \"postgres\"\ndsn = \"postgres://nils:k@127.0.0.1:5432/nils\"\nschema = \"nils\"\n",
        )
        .unwrap();
        assert_eq!(
            registry_backend(&dir),
            Some((
                "postgres://nils:k@127.0.0.1:5432/nils".to_string(),
                "nils".to_string()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_container_part_has_no_unit_of_its_own_on_the_machine() {
        let plan = plan(Runtime::Podman);
        let state = state_of(&plan, &[("engine", "podman"), ("desk", "podman")]);
        // The desk in a container is started by its quadlet, not by a unit
        // naming a binary that is not on this machine.
        let units = systemd_units(&plan, &state);
        assert!(
            !units.iter().any(|(n, _)| n == "nils-desk.service"),
            "a container desk was given a binary unit"
        );
    }

    #[test]
    fn launchd_gets_the_same_two_parts_as_a_plist_each() {
        let plan = plan(Runtime::Machine);
        let state = state_of(&plan, &[("engine", "binary"), ("desk", "binary")]);
        let plists = launchd_plists(&plan, &state);
        let names: Vec<&str> = plists.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["se.kineuro.nils-engine.plist", "se.kineuro.nils-desk.plist"]
        );
        assert!(
            plists[0].1.contains("<string>/opt/nils/engine</string>"),
            "{}",
            plists[0].1
        );
        assert!(
            plists[0].1.contains("<key>RunAtLoad</key><true/>"),
            "{}",
            plists[0].1
        );
        assert!(plists[0].1.starts_with("<?xml"), "{}", plists[0].1);
    }

    #[test]
    fn a_machine_takes_the_llama_cpp_build_it_can_run() {
        assert_eq!(
            llama_variant("linux", "x86_64", true),
            Some("ubuntu-vulkan-x64")
        );
        assert_eq!(llama_variant("linux", "x86_64", false), Some("ubuntu-x64"));
        assert_eq!(
            llama_variant("linux", "aarch64", true),
            Some("ubuntu-vulkan-arm64")
        );
        assert_eq!(
            llama_variant("linux", "aarch64", false),
            Some("ubuntu-arm64")
        );
        assert_eq!(
            llama_variant("macos", "aarch64", false),
            Some("macos-arm64")
        );
        assert_eq!(llama_variant("macos", "x86_64", true), Some("macos-x64"));
        assert_eq!(llama_variant("windows", "x86_64", true), None);
        assert_eq!(llama_variant("linux", "riscv64", false), None);
        // every build a machine may take has its archive's sha256 pinned
        for os in ["linux", "macos"] {
            for arch in ["x86_64", "aarch64"] {
                for graphics in [true, false] {
                    let variant = llama_variant(os, arch, graphics).unwrap();
                    let digest = llama_digest(variant).unwrap();
                    assert_eq!(digest.len(), 64, "{variant}");
                    assert!(digest.chars().all(|c| c.is_ascii_hexdigit()), "{variant}");
                }
            }
        }
        assert_eq!(
            llama_archive(
                "https://github.com/ggml-org/llama.cpp/releases/download/",
                "ubuntu-x64"
            ),
            "https://github.com/ggml-org/llama.cpp/releases/download/b10964/llama-b10964-bin-ubuntu-x64.tar.gz"
        );
        // a build's folder on record says which build it is
        assert_eq!(
            llama_recorded("/home/x/nils/llama.cpp/b10964-ubuntu-vulkan-x64"),
            Some("ubuntu-vulkan-x64")
        );
        assert_eq!(
            llama_recorded("/home/x/nils/llama.cpp/b9000-macos-arm64"),
            Some("macos-arm64")
        );
        assert_eq!(
            llama_recorded("/home/x/nils/llama.cpp/.b10964-ubuntu-x64.partial"),
            None
        );
        assert_eq!(llama_words("ubuntu-vulkan-x64"), "Vulkan");
        assert_eq!(llama_words("macos-arm64"), "Metal");
        assert_eq!(llama_words("ubuntu-arm64"), "CPU");
        assert_eq!(
            devices_listed(
                "ggml_vulkan: Found 2 Vulkan devices:\nAvailable devices:\n  Vulkan0: a card (8192 MiB, 8000 MiB free)\n  Vulkan1: another card (6144 MiB, 5741 MiB free)\n"
            ),
            vec![
                "Vulkan0: a card (8192 MiB, 8000 MiB free)",
                "Vulkan1: another card (6144 MiB, 5741 MiB free)"
            ]
        );
        assert!(devices_listed("Available devices:\n").is_empty());
        // a machine with no graphics device, as the CPU build says it
        assert!(devices_listed("Available devices:\n  (none)\n").is_empty());
    }

    /// A build's archive shaped as llama.cpp publishes one: a folder of the
    /// build's name holding the server, its libraries and links to them.
    fn llama_tar_gz(files: &[(&str, &[u8])], link: Option<(&str, &str)>) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            builder.mode(tar::HeaderMode::Deterministic);
            for (path, body) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, path, *body).unwrap();
            }
            if let Some((path, target)) = link {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                builder.append_link(&mut header, path, target).unwrap();
            }
            builder.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn a_llama_cpp_archive_is_unpacked_only_with_its_pinned_sha256() {
        let root = scratch("llama-archive");
        let into = llama_build_dir(&root, "ubuntu-x64");
        std::fs::create_dir_all(into.parent().unwrap()).unwrap();
        let good = llama_tar_gz(
            &[
                ("llama-b10964/llama-server", b"#!/bin/sh\necho server\n"),
                ("llama-b10964/libllama.so.0", b"a library"),
            ],
            Some(("llama-b10964/libllama.so", "libllama.so.0")),
        );
        let digest = crate::supervise::sha256_hex(&good);

        // an archive whose sha256 is not the pinned one is not unpacked at all
        let refused = unpack_llama(&good, &"0".repeat(64), &into).unwrap_err();
        assert!(refused.contains("sha256"), "{refused}");
        assert!(!into.exists());
        assert_eq!(
            std::fs::read_dir(into.parent().unwrap()).unwrap().count(),
            0,
            "nothing is left behind"
        );

        unpack_llama(&good, &digest, &into).unwrap();
        let server = into.join("llama-server");
        assert!(server.is_file(), "the build's own folder is left out");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_ne!(
                std::fs::metadata(&server).unwrap().permissions().mode() & 0o111,
                0,
                "the server runs"
            );
            assert_eq!(
                std::fs::read_link(into.join("libllama.so")).unwrap(),
                PathBuf::from("libllama.so.0"),
                "a link stays a link"
            );
        }

        // an archive of another folder, or with no server, is refused, and
        // the build already there stays
        let elsewhere = llama_tar_gz(&[("other/llama-server", b"x")], None);
        let refused =
            unpack_llama(&elsewhere, &crate::supervise::sha256_hex(&elsewhere), &into).unwrap_err();
        assert!(refused.contains("outside llama-b10964/"), "{refused}");
        let serverless = llama_tar_gz(&[("llama-b10964/README.md", b"x")], None);
        let refused = unpack_llama(
            &serverless,
            &crate::supervise::sha256_hex(&serverless),
            &into,
        )
        .unwrap_err();
        assert!(
            refused.contains("no llama-b10964/llama-server"),
            "{refused}"
        );
        assert!(server.is_file(), "the build already there stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn llama_cpp_runs_as_a_service_of_its_own_ahead_of_kvasir() {
        let mut plan = plan(Runtime::Machine);
        plan.parts.push(Part::Assistant);
        plan.llama = Some(Llama {
            variant: "ubuntu-vulkan-x64",
            loader: true,
        });
        let mut state = state_of(
            &plan,
            &[
                ("engine", "binary"),
                ("kvasir", "node"),
                ("assistant", "node"),
            ],
        );
        let build = llama_build_dir(&plan.dir, "ubuntu-vulkan-x64");
        state.parts.insert(
            LLAMA_PART.to_string(),
            PartState {
                version: LLAMA_BUILD.to_string(),
                path: build.display().to_string(),
                kind: LLAMA_PART.to_string(),
            },
        );
        let units = systemd_units(&plan, &state);
        let names: Vec<&str> = units.iter().map(|(n, _)| n.as_str()).collect();
        let llama = names
            .iter()
            .position(|n| *n == "nils-llama.service")
            .expect("a unit of llama.cpp's own");
        let kvasir = names.iter().position(|n| *n == "kvasir.service").unwrap();
        assert!(llama < kvasir, "llama.cpp starts before Kvasir: {names:?}");
        let unit = &units[llama].1;
        assert!(
            unit.contains(
                "ExecStart=/home/x/nils/llama.cpp/b10964-ubuntu-vulkan-x64/llama-server \
                 --models-preset /home/x/nils/kvasir/runtime/models.ini --no-models-autoload \
                 --models-max 1 --api-key-file /home/x/nils/kvasir/runtime/runtime.key \
                 --host 127.0.0.1 --port 7110 --no-webui \
                 --log-file /home/x/nils/kvasir/runtime/runtime.log\n"
            ),
            "{unit}"
        );
        assert!(
            unit.contains("WorkingDirectory=/home/x/nils/kvasir/runtime\n"),
            "{unit}"
        );
        assert!(unit.contains("Restart=on-failure"), "{unit}");
        // docker's containers reach it on the bridge
        assert!(
            llama_unit(&plan, &build, "172.17.0.1").contains("--host 172.17.0.1 --port 7110"),
            "listening where a container reaches this machine"
        );
        // launchd runs the same command
        let plists = launchd_plists(&plan, &state);
        let plist = &plists
            .iter()
            .find(|(n, _)| n == "se.kineuro.nils-llama.plist")
            .expect("a plist of llama.cpp's own")
            .1;
        assert!(
            plist.contains("<key>Label</key><string>se.kineuro.nils-llama</string>"),
            "{plist}"
        );
        assert!(
            plist.contains("<string>--no-models-autoload</string>"),
            "{plist}"
        );
        // the supervisor names it as a part, and says where it answers
        let mut recorded = state.clone();
        recorded.service = "systemd user units".to_string();
        let parts: Vec<(&str, String)> = service_units(&recorded)
            .into_iter()
            .map(|u| (u.part, u.name))
            .collect();
        assert!(
            parts.contains(&(LLAMA_PART, "nils-llama".to_string())),
            "{parts:?}"
        );
        let at = addresses(&recorded);
        assert!(
            at.iter()
                .any(|a| a["part"] == LLAMA_PART && a["address"] == "127.0.0.1:7110"),
            "{at:?}"
        );
        // without a build on record there is no unit to start
        state.parts.remove(LLAMA_PART);
        assert!(
            !systemd_units(&plan, &state)
                .iter()
                .any(|(n, _)| n == "nils-llama.service")
        );
    }

    #[test]
    fn kvasir_is_told_where_llama_cpp_is_from_where_kvasir_runs() {
        let dir = scratch("llama-kvasir");
        let mut plan = plan(Runtime::Machine);
        plan.dir = dir.clone();
        plan.parts.push(Part::Assistant);
        plan.llama = Some(Llama {
            variant: "ubuntu-x64",
            loader: true,
        });
        let mut value = serde_json::json!({
            "bind": "127.0.0.1:7100",
            "local": { "endpoint": "https://huggingface.co" },
        });
        // with no build here yet, nothing is named
        assert!(runtime_into_kvasir(&mut value, &plan).is_empty());
        assert!(value["local"].get("runtime").is_none(), "{value}");

        let build = llama_build_dir(&dir, "ubuntu-x64");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("llama-server"), "").unwrap();
        let said = runtime_into_kvasir(&mut value, &plan);
        assert_eq!(said.len(), 1, "{said:?}");
        let runtime = dir.join("kvasir").join("runtime");
        assert_eq!(
            value["local"]["runtime"],
            serde_json::json!({
                "url": "http://127.0.0.1:7110",
                "keyFile": runtime.join("runtime.key").display().to_string(),
                "presets": runtime.join("models.ini").display().to_string(),
                "log": runtime.join("runtime.log").display().to_string(),
                "build": "b10964",
                "variant": "ubuntu-x64",
            })
        );
        assert_eq!(
            value["local"]["endpoint"], "https://huggingface.co",
            "the rest of the file is kept"
        );
        assert_eq!(value["bind"], "127.0.0.1:7100");
        assert!(
            value.get("hostAlias").is_none(),
            "Kvasir on the machine needs no alias"
        );
        assert!(
            runtime_into_kvasir(&mut value, &plan).is_empty(),
            "a second run changes nothing"
        );

        // in a pod given this machine's loopback, and in one that is not
        plan.runtime = Runtime::Podman;
        plan.host_loopback = true;
        runtime_into_kvasir(&mut value, &plan);
        assert_eq!(value["local"]["runtime"]["url"], "http://169.254.1.2:7110");
        assert_eq!(value["hostAlias"], "169.254.1.2");
        plan.host_loopback = false;
        runtime_into_kvasir(&mut value, &plan);
        assert_eq!(value["hostAlias"], "host.containers.internal");
        // on docker
        plan.runtime = Runtime::Docker;
        runtime_into_kvasir(&mut value, &plan);
        assert_eq!(
            value["local"]["runtime"]["url"],
            "http://host.docker.internal:7110"
        );
        assert_eq!(value["hostAlias"], "host.docker.internal");
        // and moved back to the machine, where the alias goes
        plan.runtime = Runtime::Machine;
        let said = runtime_into_kvasir(&mut value, &plan);
        assert!(value.get("hostAlias").is_none(), "{value}");
        assert_eq!(said.len(), 2, "{said:?}");

        // the runtime's files are made once and kept
        assert!(write_runtime_files(&plan).is_ok());
        let key = std::fs::read_to_string(runtime.join("runtime.key")).unwrap();
        assert_eq!(key.trim().len(), 48, "{key}");
        assert_eq!(key.lines().count(), 1);
        assert_eq!(
            std::fs::read_to_string(runtime.join("models.ini")).unwrap(),
            "[*]\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(runtime.join("runtime.key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::write(
            runtime.join("models.ini"),
            "[*]\ncache-type-k = q8_0\n\n[a-model]\nmodel = /m.gguf\n",
        )
        .unwrap();
        assert!(write_runtime_files(&plan).is_ok());
        assert_eq!(
            std::fs::read_to_string(runtime.join("runtime.key")).unwrap(),
            key,
            "the key is kept"
        );
        assert!(
            std::fs::read_to_string(runtime.join("models.ini"))
                .unwrap()
                .contains("[a-model]"),
            "the presets Kvasir wrote are kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_names_llama_cpp_and_a_machine_without_a_vulkan_loader() {
        let mut plan = plan(Runtime::Machine);
        plan.parts.push(Part::Assistant);
        let row = |plan: &Plan| {
            plan_rows(plan)
                .into_iter()
                .find(|(key, _)| *key == LLAMA_PART)
                .map(|(_, value)| value)
                .unwrap()
        };
        assert!(
            row(&plan).starts_with("no build for this machine"),
            "{}",
            row(&plan)
        );
        plan.llama = Some(Llama {
            variant: "ubuntu-vulkan-x64",
            loader: true,
        });
        assert_eq!(
            row(&plan),
            "b10964, the Vulkan build, runs the models Kvasir starts, on 127.0.0.1:7110"
        );
        assert!(
            stages(&plan, false)
                .iter()
                .any(|(stage, label)| *stage == Stage::Runtime && label == LLAMA_PART)
        );
        plan.llama = Some(Llama {
            variant: "ubuntu-vulkan-x64",
            loader: false,
        });
        assert!(
            row(&plan)
                .contains("no Vulkan loader, so a model runs on the processor until libvulkan1"),
            "{}",
            row(&plan)
        );
        plan.parts.retain(|p| *p != Part::Assistant);
        assert!(!plan_rows(&plan).iter().any(|(key, _)| *key == LLAMA_PART));
        assert!(
            !stages(&plan, false)
                .iter()
                .any(|(stage, _)| *stage == Stage::Runtime)
        );
    }

    #[test]
    fn an_uninstall_removes_llama_cpps_build_with_nils() {
        let root = scratch("uninstall-llama");
        let dir = root.join("nils");
        let build = llama_build_dir(&dir, "ubuntu-x64");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("llama-server"), "x").unwrap();
        std::fs::create_dir_all(dir.join("registry")).unwrap();
        let mut state = State {
            dir: dir.display().to_string(),
            mode: "off".to_string(),
            runtime: "machine".to_string(),
            ..State::default()
        };
        state.parts.insert(
            LLAMA_PART.to_string(),
            PartState {
                version: LLAMA_BUILD.to_string(),
                path: build.display().to_string(),
                kind: LLAMA_PART.to_string(),
            },
        );
        let removal = gather_removal(&state, None, Leaving::KeepData);
        assert_eq!(
            removal.llama.as_deref(),
            Some(dir.join(LLAMA_PART).as_path())
        );
        assert!(
            removal.programs.is_empty(),
            "a build is not a program the record names"
        );
        let text = removal_text(&removal, Leaving::KeepData, &Console::new(true));
        assert!(
            text.contains("the build that ran the models Kvasir started"),
            "{text}"
        );

        // carried out with nothing of this machine's own in it
        let record = root.join("setup.toml");
        std::fs::write(&record, "").unwrap();
        let removal = Removal {
            units: Vec::new(),
            unit_files: Vec::new(),
            state: record.clone(),
            ..removal
        };
        carry_out(&removal, Leaving::KeepData, &Console::new(true));
        assert!(!dir.join(LLAMA_PART).exists());
        assert!(dir.join("registry").is_dir(), "the data stays");

        // where everything goes, it goes with the base directory
        std::fs::create_dir_all(&build).unwrap();
        assert_eq!(gather_removal(&state, None, Leaving::Purge).llama, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plan_rebuilt_from_a_state_is_the_plan_that_wrote_it() {
        let state = State {
            dir: "/home/x/nils".to_string(),
            mode: "local".to_string(),
            runtime: "docker".to_string(),
            service: "a compose file".to_string(),
            reach: "10.0.0.5".to_string(),
            backend: "postgres:nils".to_string(),
            ports: Ports {
                engine: 9000,
                ..Ports::default()
            },
            places: vec![
                PlaceState {
                    name: "scanner2".to_string(),
                    role: "source".to_string(),
                    path: "/data/two".to_string(),
                },
                PlaceState {
                    name: "source".to_string(),
                    role: "source".to_string(),
                    path: "/data/source".to_string(),
                },
            ],
            ..State::default()
        };
        let plan = plan_from_state(&state, None);
        assert_eq!(plan.mode, Mode::Local);
        assert_eq!(plan.runtime, Runtime::Docker);
        assert_eq!(plan.reach, Reach::Network("10.0.0.5".to_string()));
        assert_eq!(plan.ports.engine, 9000);
        assert_eq!(plan.source.as_deref(), Some(Path::new("/data/source")));
        assert_eq!(
            plan.read_from(),
            vec![
                ("source".to_string(), PathBuf::from("/data/source")),
                ("scanner2".to_string(), PathBuf::from("/data/two")),
            ],
            "the source setup asked for first, then every other one on record"
        );
        assert!(
            plan.service,
            "a state that names a service manager keeps it"
        );
        assert!(plan.registry_exists, "a recorded setup has a registry");
        assert!(matches!(plan.backend, BackendChoice::Postgres { .. }));
    }

    #[test]
    fn a_container_desk_reaches_the_engine_by_name_on_docker() {
        // The desk in a pod shares a loopback; on docker it is a service name.
        let mut p = plan(Runtime::Docker);
        p.parts = vec![Part::Engine, Part::Desk];
        assert!(docker_compose(&p).contains("depends_on: [engine]"));
    }
}
