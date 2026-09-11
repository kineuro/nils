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

use crate::update;
use crate::{Exit, fail, usage};
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

/// Where the two Node parts come from.
const KVASIR_REPO: &str = "https://github.com/kineuro/kvasir";
const ASSISTANT_REPO: &str = "https://github.com/kineuro/nils-assistant";

/// Inside a container, everything lives under one prefix.
const IN_REGISTRY: &str = "/srv/nils/registry";
const IN_DESK: &str = "/srv/nils/desk";

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
    /// Where it runs: machine, podman or docker
    #[arg(long, value_name = "machine|podman|docker")]
    runtime: Option<String>,
    /// The registry's backend: sqlite or postgres
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
    /// The registry key's passphrase, from a file instead of a prompt
    #[arg(long, value_name = "FILE")]
    key_file: Option<PathBuf>,
    /// Write and start services
    #[arg(long)]
    service: bool,
    /// Write no services
    #[arg(long, conflicts_with = "service")]
    no_service: bool,
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
}

impl Default for Ports {
    fn default() -> Ports {
        Ports {
            engine: 8437,
            desk: 7200,
            kvasir: 7100,
            assistant: 7300,
        }
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
             through the gateway (the prompt then leaves the machine, and the gateway marks \
             that backend remote), or no assistant at all, which costs nothing else: the \
             engine and the desk are complete without it."
                .to_string(),
        ],
    }
}

/// Ask the machine what card it has: NVIDIA first, then AMD, then the
/// unified memory of an Apple machine. Nothing here installs anything.
pub(crate) fn probe_card() -> Option<Card> {
    if let Some(out) = run_quiet(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ],
    ) && let Some(line) = out.lines().next()
        && let Some((name, mib)) = line.split_once(',')
    {
        let mib: f64 = mib.trim().parse().unwrap_or(0.0);
        return Some(Card {
            name: name.trim().to_string(),
            memory_gb: mib / 1024.0,
        });
    }
    if let Some(out) = run_quiet("rocm-smi", &["--showmeminfo", "vram", "--csv"]) {
        let bytes = out
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse::<f64>().ok())
            .find(|n| *n > 1e9);
        if let Some(bytes) = bytes {
            return Some(Card {
                name: "an AMD card".to_string(),
                memory_gb: bytes / 1024.0 / 1024.0 / 1024.0,
            });
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
            return Some(Card {
                name: format!("{name}, unified memory"),
                memory_gb: bytes / 1024.0 / 1024.0 / 1024.0,
            });
        }
    }
    None
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

// ------------------------------------------------------------- the console

/// The terminal, when there is one. A piped run has no prompt and takes
/// every default; that is what `curl ... | sh` does.
struct Console {
    tty: Option<BufReader<std::fs::File>>,
    stdin: bool,
    colour: bool,
    yes: bool,
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
        Console {
            tty,
            stdin,
            colour,
            yes,
        }
    }

    /// Whether a person can be asked anything at all.
    fn interactive(&self) -> bool {
        !self.yes && (self.stdin || self.tty.is_some())
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

    fn step(&self, n: usize, of: usize, title: &str) {
        println!();
        println!(
            "{} {}",
            self.accent(&format!("[{n}/{of}]")),
            self.bold(title)
        );
    }

    fn note(&self, text: &str) {
        println!("  {}", self.dim(text));
    }

    fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = match self.tty.as_mut() {
            Some(tty) => tty.read_line(&mut line).ok()?,
            None => std::io::stdin().read_line(&mut line).ok()?,
        };
        (read > 0).then(|| line.trim().to_string())
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
        self.hidden_twice(question, "a passphrase")
    }

    /// A password, twice, without echoing it.
    fn password(&mut self, question: &str) -> Option<String> {
        self.hidden_twice(question, "a password")
    }

    fn hidden_twice(&mut self, question: &str, noun: &str) -> Option<String> {
        if !self.interactive() {
            return None;
        }
        loop {
            let first = self.secret_once(question)?;
            if first.is_empty() {
                println!("  {}", self.dim(&format!("{noun} is needed")));
                continue;
            }
            let again = self.secret_once("and again")?;
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
        self.secret_once(question).filter(|s| !s.is_empty())
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
        let live = std::io::stdout().is_terminal();
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
        if !live {
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
            if live {
                print!("\r\x1b[2K");
            }
            println!("  {label} {}", self.dim(&format!("done in {took}s")));
            return Ok(());
        }
        if live {
            print!("\r\x1b[2K");
        }
        println!("  {label} {}", self.dim(&format!("failed after {took}s")));
        let text = heard
            .lock()
            .map(|all| String::from_utf8_lossy(&all).to_string())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for line in &lines[lines.len().saturating_sub(25)..] {
            println!("    {line}");
        }
        Err(fail(format!("{program} {} failed", args.join(" "))))
    }

    fn secret_once(&mut self, question: &str) -> Option<String> {
        print!("  {question}: ");
        let _ = std::io::stdout().flush();
        let guard = EchoOff::on(self.tty.as_ref().map(BufReader::get_ref));
        let line = self.read_line();
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
}

/// What the wizard decided, before it does any of it.
pub(crate) struct Plan {
    pub(crate) dir: PathBuf,
    pub(crate) parts: Vec<Part>,
    pub(crate) mode: Mode,
    pub(crate) runtime: Runtime,
    pub(crate) backend: BackendChoice,
    pub(crate) ports: Ports,
    pub(crate) reach: Reach,
    pub(crate) source: Option<PathBuf>,
    pub(crate) registry_exists: bool,
    pub(crate) service: bool,
    pub(crate) channel: Option<String>,
    pub(crate) version: String,
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

    /// The tag the published images carry, for the version this plan holds.
    fn tag(&self) -> String {
        image_tag(&self.version)
    }

    fn has(&self, part: Part) -> bool {
        self.parts.contains(&part)
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
                dsn: String::new(),
                schema: schema.to_string(),
            },
            None => BackendChoice::Sqlite,
        },
        ports: state.ports,
        reach: match state.reach.as_str() {
            "" | "loopback" => Reach::Loopback,
            address => Reach::Network(address.to_string()),
        },
        source: state
            .places
            .iter()
            .find(|p| p.role == "source")
            .map(|p| PathBuf::from(&p.path)),
        registry_exists: true,
        service: !state.service.is_empty() && state.service != "none",
        channel: channel.map(str::to_string),
        version: update::VERSION.to_string(),
    }
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

/// What the desk's three address keys are, for a reach and a port.
pub(crate) fn desk_binding(
    reach: &Reach,
    port: u16,
    container: bool,
) -> (String, String, Vec<String>) {
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
            vec![
                format!("http://127.0.0.1:{port}"),
                format!("http://localhost:{port}"),
            ],
        ),
    }
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
    ];
    match plan.mode {
        Mode::Off => {
            argv.push("--auth".to_string());
            argv.push("off".to_string());
        }
        Mode::Local => {
            // The issuer is what the desk writes into its tokens, which is
            // its own origin; the keys are fetched by the engine, so that
            // address is one the engine can reach from where it runs.
            let (_, issuer, _) =
                desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
            let keys = match plan.runtime {
                Runtime::Docker => format!("http://nils-desk:{}", plan.ports.desk),
                _ => format!("http://127.0.0.1:{}", plan.ports.desk),
            };
            argv.push("--auth".to_string());
            argv.push("oidc".to_string());
            argv.push("--oidc-trust".to_string());
            argv.push(format!(
                "issuer={issuer},audience=nils,jwks={keys}/.well-known/jwks.json"
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
        }
    }
    if let Some(source) = &plan.source {
        argv.push("--ingest-root".to_string());
        argv.push(format!("source={}", source.display()));
    }
    argv
}

/// The commands a podman run is, in order, exactly as a person would type
/// them. `--print` shows these and the wizard runs them.
pub(crate) fn podman_commands(plan: &Plan) -> Vec<String> {
    let publish = match &plan.reach {
        Reach::Loopback => format!("127.0.0.1:{p}:{p}", p = plan.ports.desk),
        Reach::Network(_) => format!("{p}:{p}", p = plan.ports.desk),
    };
    let mut out = vec![format!("podman pod create --name nils -p {publish}")];
    let mut engine = format!(
        "podman run -d --pod nils --name nils-engine -v {}:{IN_REGISTRY}:U",
        plan.registry().display()
    );
    if let Some(source) = &plan.source {
        let _ = write!(engine, " -v {0}:{0}:ro", source.display());
    }
    let _ = write!(
        engine,
        " -v {}:/srv/nils/backups:U {ENGINE_IMAGE}:{} {}",
        plan.dir.join("backups").display(),
        plan.tag(),
        engine_args(plan, IN_REGISTRY, "/srv/nils/backups").join(" ")
    );
    out.push(engine);
    if plan.has(Part::Desk) {
        out.push(format!(
            "podman run -d --pod nils --name nils-desk -v {}:{IN_DESK}:U {DESK_IMAGE}:{} serve --config {IN_DESK}/nils-desk.toml",
            plan.desk_dir().display(),
            plan.tag()
        ));
    }
    out
}

/// The same for docker, which has no pod: a network of its own, the desk
/// publishing the port, and no `:U` because it does not remap the user.
pub(crate) fn docker_commands(plan: &Plan) -> Vec<String> {
    let publish = match &plan.reach {
        Reach::Loopback => format!("127.0.0.1:{p}:{p}", p = plan.ports.desk),
        Reach::Network(_) => format!("{p}:{p}", p = plan.ports.desk),
    };
    let mut out = vec!["docker network create nils".to_string()];
    let mut engine = format!(
        "docker run -d --network nils --name nils-engine {}-v {}:{IN_REGISTRY}",
        docker_user(),
        plan.registry().display()
    );
    if let Some(source) = &plan.source {
        let _ = write!(engine, " -v {0}:{0}:ro", source.display());
    }
    let _ = write!(
        engine,
        " -v {}:/srv/nils/backups {ENGINE_IMAGE}:{} {}",
        plan.dir.join("backups").display(),
        plan.tag(),
        engine_args(plan, IN_REGISTRY, "/srv/nils/backups").join(" ")
    );
    out.push(engine);
    if plan.has(Part::Desk) {
        out.push(format!(
            "docker run -d --network nils --name nils-desk {}-p {publish} -v {}:{IN_DESK} {DESK_IMAGE}:{} serve --config {IN_DESK}/nils-desk.toml",
            docker_user(),
            plan.desk_dir().display(),
            plan.tag()
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
    let _ = writeln!(
        out,
        "    command: {}",
        engine_args(plan, IN_REGISTRY, "/srv/nils/backups").join(" ")
    );
    let _ = writeln!(out, "    volumes:");
    let _ = writeln!(out, "      - {}:{IN_REGISTRY}", plan.registry().display());
    let _ = writeln!(
        out,
        "      - {}:/srv/nils/backups",
        plan.dir.join("backups").display()
    );
    if let Some(source) = &plan.source {
        let _ = writeln!(out, "      - {0}:{0}:ro", source.display());
    }
    if plan.has(Part::Desk) {
        let publish = match &plan.reach {
            Reach::Loopback => format!("127.0.0.1:{p}:{p}", p = plan.ports.desk),
            Reach::Network(_) => format!("{p}:{p}", p = plan.ports.desk),
        };
        let _ = writeln!(out, "  desk:");
        let _ = writeln!(out, "    image: {DESK_IMAGE}:{}", plan.tag());
        let _ = writeln!(out, "    container_name: nils-desk");
        if !as_this_account().is_empty() {
            let _ = writeln!(out, "    user: \"{}\"", as_this_account());
        }
        let _ = writeln!(out, "    restart: unless-stopped");
        let _ = writeln!(out, "    depends_on: [engine]");
        let _ = writeln!(out, "    command: serve --config {IN_DESK}/nils-desk.toml");
        let _ = writeln!(out, "    ports:");
        let _ = writeln!(out, "      - \"{publish}\"");
        let _ = writeln!(out, "    volumes:");
        let _ = writeln!(out, "      - {}:{IN_DESK}", plan.desk_dir().display());
    }
    out
}

/// Podman's quadlets: systemd builds the units from these.
pub(crate) fn quadlets(plan: &Plan) -> Vec<(String, String)> {
    let publish = match &plan.reach {
        Reach::Loopback => format!("127.0.0.1:{p}:{p}", p = plan.ports.desk),
        Reach::Network(_) => format!("{p}:{p}", p = plan.ports.desk),
    };
    let mut out = vec![(
        "nils.pod".to_string(),
        format!(
            "[Pod]\nPodName=nils\nPublishPort={publish}\n\n[Install]\nWantedBy=default.target\n"
        ),
    )];
    let mut engine = String::from("[Unit]\nDescription=NILS engine\n\n[Container]\n");
    let _ = writeln!(engine, "Image={ENGINE_IMAGE}:{}", plan.tag());
    let _ = writeln!(engine, "Pod=nils.pod");
    let _ = writeln!(
        engine,
        "Volume={}:{IN_REGISTRY}:U",
        plan.registry().display()
    );
    let _ = writeln!(
        engine,
        "Volume={}:/srv/nils/backups:U",
        plan.dir.join("backups").display()
    );
    if let Some(source) = &plan.source {
        let _ = writeln!(engine, "Volume={0}:{0}:ro", source.display());
    }
    let _ = writeln!(
        engine,
        "Exec={}",
        engine_args(plan, IN_REGISTRY, "/srv/nils/backups").join(" ")
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
    out
}

/// A Containerfile from a binary already downloaded, for a machine that
/// cannot pull the published image.
pub(crate) fn containerfile(part: &str) -> String {
    format!(
        "# Written by nils setup, from the release binary beside it.\n\
         FROM docker.io/library/debian:trixie-slim\n\
         RUN groupadd -g 1500 nils && useradd -u 1500 -g 1500 -M -d /srv/nils nils \\\n\
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
    let existing = read_state();

    println!(
        "{}",
        console.bold("NILS setup: the engine, the desk and the assistant")
    );

    // An install that is already there: say what it is, and offer the four
    // things a person comes back for.
    if let Some(state) = &existing {
        println!("  {}", what_is_there(state));
        let choice = if args.update {
            0
        } else if console.interactive() {
            console.choice(
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
            )
        } else {
            1
        };
        match choice {
            0 => return update_parts(state, &args, &mut console),
            3 => return repair(state, &args, &mut console),
            4 => {
                return uninstall(UninstallArgs {
                    keep_data: false,
                    purge: false,
                    yes: false,
                    print: args.print,
                });
            }
            _ => {}
        }
    } else if args.update {
        return Err(usage(format!(
            "--update needs a setup to update; none is recorded at {}",
            state_path().display()
        )));
    }

    if !console.interactive() && !args.print {
        console.note("no terminal here, so every default is taken");
    }

    let steps = 8;

    // 1. what to install
    console.step(1, steps, "What to install");
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
                .as_ref()
                .map(
                    |s| match (s.parts.contains_key("assistant"), s.parts.len()) {
                        (true, _) => 2,
                        (false, n) if n > 1 => 1,
                        _ => 0,
                    },
                )
                .unwrap_or(1);
            match console.choice("Which parts?", &choices, default) {
                0 => vec![Part::Engine],
                1 => vec![Part::Engine, Part::Desk],
                _ => vec![Part::Engine, Part::Desk, Part::Assistant],
            }
        }
    };
    console.note(
        &parts
            .iter()
            .map(|p| p.name())
            .collect::<Vec<_>>()
            .join(", "),
    );

    // 2. where it runs
    console.step(2, steps, "Where it runs");
    let podman = have("podman");
    let docker = have("docker");
    let runtime = match &args.runtime {
        Some(r) => Runtime::parse(r).map_err(usage)?,
        None => {
            if !podman && !docker {
                console.note(
                    "no podman and no docker here, so the parts run on the machine; \
                     install podman if you would rather have containers",
                );
                Runtime::Machine
            } else {
                let container = if podman { "podman" } else { "docker" };
                let choices = [
                    ("On this machine", "two small binaries and a directory"),
                    (
                        if podman {
                            "In containers (podman)"
                        } else {
                            "In containers (docker)"
                        },
                        "one image per part, the registry on a volume",
                    ),
                ];
                let default = existing
                    .as_ref()
                    .map(|s| usize::from(s.runtime == "podman" || s.runtime == "docker"))
                    .unwrap_or(0);
                if console.choice("How should the parts run?", &choices, default) == 0 {
                    Runtime::Machine
                } else if container == "podman" {
                    Runtime::Podman
                } else {
                    Runtime::Docker
                }
            }
        }
    };
    let engine_here = match runtime {
        Runtime::Machine => true,
        Runtime::Podman => podman,
        Runtime::Docker => docker,
    };
    if !engine_here {
        if !args.print {
            return Err(usage(format!(
                "{} is not on this machine; install it, or run with --runtime machine",
                runtime.name()
            )));
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

    // 3. where it lives
    console.step(3, steps, "Where it lives");
    let dir = match &args.dir {
        Some(d) => d.clone(),
        None => {
            let default = existing
                .as_ref()
                .map(|s| s.dir.clone())
                .unwrap_or_else(|| default_dir().display().to_string());
            expand(&console.line("One directory for everything", &default))
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
    let source = match &args.source {
        Some(s) => Some(s.clone()),
        None => {
            let answer = console.line("A directory of DICOM to read (empty for none)", "");
            let answer = answer.trim().to_string();
            (!answer.is_empty()).then(|| expand(&answer))
        }
    };

    // 4. the registry and its backend
    console.step(4, steps, "The registry");
    let home = Home::new(dir.join("registry"));
    let registry_exists = home.exists();
    let backend = if registry_exists {
        console.note(&format!(
            "a registry is already at {}; it is left alone",
            home.dir().display()
        ));
        BackendChoice::Sqlite
    } else {
        console.note(
            "pseudonyms are derived from a key: the same subject under the same key gets \
             the same code, so keep its passphrase where you keep passwords",
        );
        let chosen = match &args.backend {
            Some(b) if b.trim() == "postgres" => 1,
            Some(b) if b.trim() == "sqlite" => 0,
            Some(other) => {
                return Err(usage(format!(
                    "{other} is not a backend: sqlite or postgres"
                )));
            }
            None => console.choice(
                "Where should the registry itself be kept?",
                &[
                    (
                        "SQLite",
                        "one file in the directory above; a laptop or one server",
                    ),
                    ("Postgres", "a database server, for more than one machine"),
                ],
                0,
            ),
        };
        if chosen == 0 {
            BackendChoice::Sqlite
        } else {
            let dsn = match &args.dsn {
                Some(d) => d.clone(),
                None => console.line("The connection string", "postgres://nils@127.0.0.1/nils"),
            };
            let schema = match &args.schema {
                Some(s) => s.clone(),
                None => console.line("The schema", "nils"),
            };
            BackendChoice::Postgres { dsn, schema }
        }
    };
    let mut answers = Answers::default();
    if !registry_exists && args.key_file.is_none() && console.interactive() {
        answers.passphrase = console.secret("A passphrase for the registry's key");
    }

    // 5. who may sign in, and who may reach the desk
    console.step(5, steps, "Who may sign in");
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
                .as_ref()
                .and_then(|s| Mode::parse(&s.mode).ok())
                .map_or(0, |m| match m {
                    Mode::Off => 0,
                    Mode::Local => 1,
                    Mode::Oidc => 2,
                });
            match console.choice("How do people reach it?", &choices, default) {
                0 => Mode::Off,
                1 => Mode::Local,
                _ => Mode::Oidc,
            }
        }
    };

    console.note(mode.words());

    let mut reach = match &args.reach {
        Some(r) if r.trim() == "network" => {
            Reach::Network(host_address().unwrap_or_else(|| "127.0.0.1".to_string()))
        }
        Some(r) if r.trim() == "loopback" => Reach::Loopback,
        Some(other) => {
            return Err(usage(format!(
                "{other} is not a reach: loopback or network"
            )));
        }
        None if !parts.contains(&Part::Desk) => Reach::Loopback,
        None => {
            let address = host_address().unwrap_or_else(|| "this host".to_string());
            let choices = [
                ("Only this machine", "the desk answers on 127.0.0.1"),
                ("This network", "other machines here can open it"),
            ];
            if console.choice("Who may open the desk?", &choices, 0) == 0 {
                Reach::Loopback
            } else {
                Reach::Network(address)
            }
        }
    };
    if matches!(reach, Reach::Network(_)) && mode == Mode::Off {
        console.note(
            "off mode has no login, so anyone on that network who finds the port gets the \
             whole registry",
        );
        if console.yes_no("Keep the people in the desk instead (local mode)?", true) {
            mode = Mode::Local;
        } else if !console.interactive() {
            reach = Reach::Loopback;
        }
    }

    // the first person, for a desk that keeps its own and has none yet
    let store_exists = dir.join("desk").join("nils-desk.sqlite").exists();
    if mode == Mode::Local
        && parts.contains(&Part::Desk)
        && !store_exists
        && console.interactive()
        && console.yes_no("Add the first person now, who may do everything?", true)
    {
        let name = console.line("A username for them", "admin");
        if let Some(password) = console.password(&format!("A password for {name}")) {
            answers.first = Some((name, password));
        }
    }

    // ports
    let mut ports = existing.as_ref().map(|s| s.ports).unwrap_or_default();
    for (name, port) in [
        ("the engine", &mut ports.engine),
        ("the desk", &mut ports.desk),
    ] {
        if port_taken(*port) {
            let free = next_free_port(*port + 1, &port_taken);
            console.note(&format!("port {} is taken, so {name} takes {free}", *port));
            *port = free;
        }
    }

    // 6. the assistant, and what this machine can do
    console.step(6, steps, "What this machine can do");
    let card = probe_card();
    match &card {
        Some(card) => console.note(&format!("{}, {:.0} GB", card.name, card.memory_gb.round())),
        None => console.note("no graphics card the probe could find"),
    }
    for line in card_advice(card.as_ref().map(|c| c.memory_gb)) {
        println!("  {line}");
    }
    let served = card.as_ref().map(|c| c.memory_gb).unwrap_or(0.0) >= 12.0;
    if args.parts.is_none() {
        if parts.contains(&Part::Assistant) {
            if !console.yes_no("Install the assistant anyway?", served) {
                parts.retain(|p| *p != Part::Assistant);
            }
        } else if console.yes_no("Add the assistant as well?", false) {
            parts.push(Part::Assistant);
        }
    }
    if parts.contains(&Part::Assistant) && runtime.container() {
        console.note("the assistant and the gateway are built from source, on the machine");
    }
    if parts.contains(&Part::Assistant) && !dir.join("kvasir").join("kvasir.json").exists() {
        answers.model = Some(choose_model(&mut console, served));
    }

    // 7. services
    console.step(7, steps, "Keeping it running");
    let manager = service_manager(runtime);
    let service = match (args.no_service, args.service, manager) {
        (true, _, _) => false,
        (_, true, _) => true,
        (_, _, None) => {
            console.note("no service manager here, so the commands are printed instead");
            false
        }
        (_, _, Some(manager)) => {
            let before = existing
                .as_ref()
                .map(|s| !s.service.is_empty() && s.service != "none");
            console.yes_no(
                &format!("Write and start {manager} so it comes back after a restart?"),
                before.unwrap_or(true),
            )
        }
    };

    let plan = Plan {
        dir,
        parts,
        mode,
        runtime,
        backend,
        ports,
        reach,
        source,
        registry_exists,
        service,
        channel: args.channel.clone(),
        version: update::VERSION.to_string(),
    };

    // 8. the summary, then the work
    console.step(8, steps, "The plan");
    print!("{}", plan_text(&plan, &console));
    if let Some((name, _)) = &answers.first {
        println!(
            "  {} {name}, who may do everything",
            console.dim(&format!("{:<12}", "first person"))
        );
    }
    if let Some(model) = &answers.model {
        let said = if model.later {
            "none named yet".to_string()
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
        println!("  {} {said}", console.dim(&format!("{:<12}", "model")));
    }
    if args.print {
        print!("{}", commands_text(&plan, &console));
        println!();
        println!("nothing was changed");
        return Ok(());
    }
    if console.interactive() && !console.yes_no("Do it?", true) {
        println!("nothing was changed");
        return Ok(());
    }

    do_it(&plan, &mut console, &args, existing, false, &answers)
}

/// What is installed, where, in what mode and how it runs, on one line.
fn what_is_there(state: &State) -> String {
    let parts = state
        .parts
        .iter()
        .map(|(name, p)| format!("{name} {}", p.version))
        .collect::<Vec<_>>()
        .join(", ");
    let runtime = if state.runtime.is_empty() {
        "machine"
    } else {
        &state.runtime
    };
    let service = if state.service.is_empty() || state.service == "none" {
        "started by hand".to_string()
    } else {
        format!("kept running by {}", state.service)
    };
    format!(
        "{parts} in {}, {} mode, on the {runtime}, {service}",
        state.dir, state.mode
    )
}

/// `--update`, and the first thing the menu offers: the parts the state
/// names, brought to the newest release, without a question.
fn update_parts(state: &State, args: &SetupArgs, console: &mut Console) -> Result<(), Exit> {
    let mut plan = plan_from_state(state, args.channel.as_deref());
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
    do_it(
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
    if plan.has(Part::Desk) {
        write_desk_config(&plan, true)?;
        println!("  {}", plan.desk_config().display());
    }
    // The gateway's file is mended, not rewritten, and the assistant's
    // environment is written where it is missing; the key is made during the
    // start-up below, once the gateway answers.
    if plan.has(Part::Assistant) {
        if let Err(e) = configure_kvasir(&plan, console, None) {
            println!(
                "  the gateway's configuration was not mended: {}",
                e.message
            );
        }
        if let Err(e) = write_assistant_env(&plan) {
            println!(
                "  the assistant's environment was not written: {}",
                e.message
            );
        }
    }
    if plan.service {
        match start_everything(&plan, state, console) {
            Ok(what) => print!("{what}"),
            Err(e) => println!("  no services were written: {}", e.message),
        }
    } else {
        for line in container_commands(&plan) {
            println!("  run: {line}");
        }
    }
    let systemd = plan.runtime == Runtime::Machine && !cfg!(target_os = "macos");
    if plan.has(Part::Assistant) && !(plan.service && systemd) {
        mint_assistant_key(&plan, console);
    }
    summary(&plan, console);
    Ok(())
}

/// Which service manager this machine and runtime use, if any.
fn service_manager(runtime: Runtime) -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        return (runtime == Runtime::Machine).then_some("launchd agents");
    }
    if !cfg!(target_os = "linux") || run_quiet("systemctl", &["--user", "--version"]).is_none() {
        return None;
    }
    Some(match runtime {
        Runtime::Machine => "systemd user units",
        Runtime::Podman => "podman quadlets",
        Runtime::Docker => "a compose file",
    })
}

/// The plan as a person reads it before anything happens.
fn plan_text(plan: &Plan, console: &Console) -> String {
    let mut out = String::new();
    let mut row = |key: &str, value: String| {
        let _ = writeln!(out, "  {} {value}", console.dim(&format!("{key:<12}")));
    };
    row("directory", plan.dir.display().to_string());
    row(
        "parts",
        plan.parts
            .iter()
            .map(|p| p.name())
            .collect::<Vec<_>>()
            .join(", "),
    );
    row(
        "runs",
        match plan.runtime {
            Runtime::Machine => "on this machine".to_string(),
            Runtime::Podman => format!("in podman containers, images {ENGINE_IMAGE}"),
            Runtime::Docker => format!("in docker containers, images {ENGINE_IMAGE}"),
        },
    );
    row(
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
    );
    row(
        "backend",
        match &plan.backend {
            BackendChoice::Sqlite => "sqlite".to_string(),
            BackendChoice::Postgres { schema, .. } => format!("postgres, schema {schema}"),
        },
    );
    row(
        "sign in",
        format!("{} ({})", plan.mode.words(), plan.mode.name()),
    );
    if plan.has(Part::Desk) {
        let (bind, origin, _) =
            desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
        row("desk", format!("{origin} (binds {bind})"));
        row(
            "desk config",
            format!("{} (will be written)", plan.desk_config().display()),
        );
    }
    row("engine port", plan.ports.engine.to_string());
    if let Some(source) = &plan.source {
        row("reads", format!("{} (read only)", source.display()));
    }
    row(
        "places",
        place_specs(plan)
            .iter()
            .map(|s| format!("{} as {}", s.name, s.role))
            .collect::<Vec<_>>()
            .join(", "),
    );
    row(
        "services",
        if plan.service {
            service_manager(plan.runtime).unwrap_or("none").to_string()
        } else {
            "none; the commands are printed".to_string()
        },
    );
    row("state", state_path().display().to_string());
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
        Runtime::Machine => {}
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
    let _ = writeln!(out, "\n{}", console.bold("  places"));
    for spec in place_specs(plan) {
        let _ = writeln!(out, "    {}", place_argv(&spec).join(" "));
    }
    out
}

/// Everything the plan said, in order.
fn do_it(
    plan: &Plan,
    console: &mut Console,
    args: &SetupArgs,
    existing: Option<State>,
    only_update: bool,
    answers: &Answers,
) -> Result<(), Exit> {
    for sub in ["registry", "desk", "backups", "working", "export"] {
        let path = plan.dir.join(sub);
        std::fs::create_dir_all(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    }

    let mut state = State {
        dir: plan.dir.display().to_string(),
        mode: plan.mode.name().to_string(),
        runtime: plan.runtime.name().to_string(),
        service: if plan.service {
            service_manager(plan.runtime).unwrap_or("none").to_string()
        } else {
            "none".to_string()
        },
        reach: match &plan.reach {
            Reach::Loopback => "loopback".to_string(),
            Reach::Network(addr) => addr.clone(),
        },
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
    };
    let previous_parts = state.parts.clone();
    let existing_places = existing.map(|s| s.places).unwrap_or_default();

    // The engine: the running binary, or an image.
    let me = std::env::current_exe()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .map_err(|e| fail(format!("this binary cannot say where it is: {e}")))?;
    let into = binary_dir(&me, plan);

    if plan.runtime.container() {
        state.keep_program(&me);
        pull_or_build(plan, console, "nils", ENGINE_IMAGE, &me)?;
        if plan.has(Part::Desk) {
            let desk_binary = match install_desk(&into, plan.channel.as_deref()) {
                Ok((_, path)) => {
                    state.keep_program(&path);
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
    println!("  engine {} ready", update::VERSION);

    // The registry, made by the engine wherever it runs.
    let home = Home::new(plan.registry());
    if !plan.registry_exists && !only_update {
        make_registry(plan, &home, console, args, answers.passphrase.as_deref())?;
    }

    // The desk.
    if plan.has(Part::Desk) {
        if !plan.runtime.container() {
            match install_desk(&into, plan.channel.as_deref()) {
                Ok((version, path)) => {
                    println!("  desk {version} at {}", path.display());
                    state.parts.insert(
                        "desk".to_string(),
                        PartState {
                            version,
                            path: path.display().to_string(),
                            kind: "binary".to_string(),
                        },
                    );
                }
                Err(e) => println!("  the desk was not installed: {}", e.message),
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
        write_desk_config(plan, false)?;
        println!("  {}", plan.desk_config().display());
        if plan.mode == Mode::Local {
            let desk = state
                .parts
                .get("desk")
                .filter(|p| p.kind == "binary")
                .map(|p| PathBuf::from(&p.path));
            add_first_admin(plan, desk, answers.first.as_ref());
        }
        if plan.mode == Mode::Oidc {
            println!("  register the desk at your provider with:");
            let (_, origin, _) =
                desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
            println!(
                "    nils-desk register --authentik https://auth.example.org --token ./api-token \\\n      --origin {origin} --allow <group> --bind reader=<group> --secret-file {}",
                plan.desk_dir().join("client-secret").display()
            );
        }
    }

    // The assistant and its gateway, which are built rather than downloaded.
    if plan.has(Part::Assistant) {
        match install_node_parts(plan, console, answers.model.as_ref()) {
            Ok(paths) => {
                for (name, path) in paths {
                    println!("  {name} at {}", path.display());
                    state.parts.insert(
                        name.to_string(),
                        PartState {
                            version: "from source".to_string(),
                            path: path.display().to_string(),
                            kind: "node".to_string(),
                        },
                    );
                }
            }
            Err(e) => println!("  the assistant was not installed: {}", e.message),
        }
    }

    // The places, on an engine that keeps them.
    if only_update {
        state.places = existing_places;
    } else {
        state.places = declare_places(plan, &home);
    }

    // Start it.
    if plan.service {
        match start_everything(plan, &state, console) {
            Ok(what) => print!("{what}"),
            Err(e) => println!("  no services were written: {}", e.message),
        }
    } else if plan.runtime.container() {
        // No unit files were asked for, but a container still has to be
        // started, or the person is left with images and nothing running.
        for line in container_commands(plan) {
            match run_line(&line) {
                Ok(()) => println!("  {}", short(&line)),
                Err(e) => {
                    println!("  that failed: {e}");
                    println!("  run: {line}");
                }
            }
        }
    }

    // The assistant's key comes from the gateway. On systemd the start-up
    // itself makes it, between the gateway and the assistant; everywhere
    // else it is made here, once the gateway answers.
    let systemd = plan.runtime == Runtime::Machine && !cfg!(target_os = "macos");
    if plan.has(Part::Assistant) && plan.service && !systemd {
        mint_assistant_key(plan, console);
    }
    if plan.has(Part::Assistant) && !plan.service {
        console.note(
            "with no services, start the gateway yourself; then nils setup and repair makes \
             the assistant's key",
        );
    }

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

    let path = write_state(&state)?;
    println!("  {}", path.display());
    summary(plan, console);
    Ok(())
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
    let pulled = Command::new(engine)
        .args(["pull", &tag])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if pulled {
        console.note(&format!("pulled {tag}"));
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
    let status = Command::new(engine)
        .args(["build", "-t", &tag, "."])
        .current_dir(&context)
        .status()
        .map_err(|e| fail(format!("{engine}: {e}")))?;
    if !status.success() {
        return Err(fail(format!("{engine} build of {tag} failed")));
    }
    console.note(&format!("built {tag} from {}", context.display()));
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
                println!(
                    "  no terminal to ask on, so a passphrase was made and written to {}",
                    path.display()
                );
                println!("  move it into your password manager and delete the file");
                generated
            }
        },
    };

    if let BackendChoice::Postgres { dsn, schema } = &plan.backend {
        // Try the connection before anything is written, so a wrong dsn
        // costs a sentence rather than half a registry.
        if let Err(e) = nils_registry::store::Store::connect_postgres(dsn, schema) {
            return Err(fail(format!(
                "the database refused the connection: {e}\n  check the connection string, that \
                 the database exists, and that the role may create a schema"
            )));
        }
        console.note("the database answered");
    }

    if plan.runtime.container() {
        let engine = plan.runtime.name();
        let mount = format!(
            "{}:{IN_REGISTRY}{}",
            plan.registry().display(),
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
        let mut child = Command::new(engine)
            .args(["run", "--rm", "-i"])
            .args(&user)
            .args(["-v", &mount, &tag])
            .args(["--registry", IN_REGISTRY, "key", "add", "nils"])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| fail(format!("{engine}: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{passphrase}");
        }
        let status = child.wait().map_err(|e| fail(e.to_string()))?;
        if !status.success() {
            return Err(fail("the key could not be added inside the container"));
        }
        let status = Command::new(engine)
            .args(["run", "--rm"])
            .args(&user)
            .args(["-v", &mount, &tag])
            .args(["--registry", IN_REGISTRY, "init", "--key", "nils"])
            .status()
            .map_err(|e| fail(format!("{engine}: {e}")))?;
        if !status.success() {
            return Err(fail("the registry could not be made inside the container"));
        }
        println!("  registry at {}", plan.registry().display());
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
    println!("  registry at {}", home.dir().display());
    Ok(())
}

/// The places the engine now keeps, added in an order that lets the
/// registry name its backup. A place already there is left as it is, so a
/// second run of the wizard says the same thing as the first.
fn declare_places(plan: &Plan, home: &Home) -> Vec<PlaceState> {
    use nils_registry::place::{self, Role};
    let mut registry = match crate::open(home) {
        Ok(registry) => registry,
        Err(_) => return Vec::new(),
    };
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
                declared.push(PlaceState {
                    path: there.path,
                    ..row
                });
                continue;
            }
            Ok(None) => {}
            Err(_) => continue,
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
            Err(e) => println!("  the {} place was not declared: {e}", spec.name),
        }
    }
    if !declared.is_empty() {
        println!("  places: {}", say_places(&declared));
    }
    declared
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
        // Not fatal: the engine runs, digests and answers questions without
        // packs; what it cannot do is say what a scan is. That is worth a
        // plain sentence rather than a note, and worth being accurate
        // about, since an install that can still read a study is not a
        // broken one.
        Err(e) => {
            println!("  the rule packs were not installed: {e}");
            println!("  the engine still runs and digests; what it cannot do is classify.");
            println!(
                "  put a packs directory at {} or pass --pack-dir",
                dir.display()
            );
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

/// The desk's configuration for the mode chosen, and the tables for the
/// parts that were installed beside it.
fn write_desk_config(plan: &Plan, force: bool) -> Result<(), Exit> {
    let path = plan.desk_config();
    if path.exists() && !force {
        return Ok(());
    }
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
    let _ = writeln!(text, "\n[engine]");
    let _ = writeln!(text, "url = \"{engine_url}\"");
    if plan.has(Part::Assistant) {
        let _ = writeln!(text, "\n[kvasir]");
        let _ = writeln!(text, "url = \"http://127.0.0.1:{}\"", plan.ports.kvasir);
        let _ = writeln!(text, "\n[assistant]");
        let _ = writeln!(text, "url = \"http://127.0.0.1:{}\"", plan.ports.assistant);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// In local mode the desk keeps the people, and an empty desk has nobody to
/// let in. The offer is made once, and the desk's own command asks for the
/// password, so nothing here ever holds one.
fn add_first_admin(plan: &Plan, desk: Option<PathBuf>, first: Option<&(String, String)>) {
    let by_hand = format!(
        "nils-desk user add <name> --admin --config {}",
        plan.desk_config().display()
    );
    let Some((name, password)) = first else {
        println!("  add the first person with: {by_hand}");
        return;
    };
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
        Ok(out) if out.status.success() => println!("  {name} may sign in and do everything"),
        Ok(out) => {
            let why = String::from_utf8_lossy(&out.stderr);
            println!(
                "  {name} was not added: {}",
                why.lines().last().unwrap_or("the desk refused")
            );
            println!("  the command is: {by_hand}");
        }
        Err(e) => println!("  {name} was not added ({e}); the command is: {by_hand}"),
    }
}

/// The gateway and the assistant: cloned and built, since neither ships a
/// binary. Anything missing is said rather than guessed at.
fn install_node_parts(
    plan: &Plan,
    console: &mut Console,
    model: Option<&ModelChoice>,
) -> Result<Vec<(&'static str, PathBuf)>, Exit> {
    let node = run_quiet("node", &["--version"]).unwrap_or_default();
    let major: u32 = node
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    if major < 22 {
        return Err(fail(
            "the assistant and the gateway are built with Node 22; install it and run nils setup again",
        ));
    }
    if !have("git") {
        return Err(fail("git is needed to take the assistant's source"));
    }

    let mut out = Vec::new();
    for (name, said, repo) in [
        ("kvasir", "the gateway", KVASIR_REPO),
        ("assistant", "the assistant", ASSISTANT_REPO),
    ] {
        let into = plan.dir.join(name);
        if into.exists() {
            console.task(
                &format!("updating {said}'s source"),
                &into,
                "git",
                &["pull", "--ff-only"],
            )?;
        } else {
            console.task(
                &format!("fetching {said}"),
                &plan.dir,
                "git",
                &["clone", "--depth", "1", repo, &into.display().to_string()],
            )?;
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
    write_assistant_env(plan)?;
    Ok(out)
}

/// What the assistant will talk to, chosen by a person who may not know
/// what an OpenAI compatible address is.
struct ModelChoice {
    url: String,
    local: bool,
    key: Option<String>,
    model: String,
    later: bool,
}

/// The model the gateway is given when nobody has named one: the one the
/// assistant's stations were written against, on SGLang's own port.
const DEFAULT_MODEL_URL: &str = "http://127.0.0.1:30000/v1";

/// That model's name, as the gateway's own example gives it.
const EXAMPLE_MODEL_ID: &str = "qwen38-27b";

/// Ask what the assistant should talk to, and what to type for it. Where the
/// address answers, the server is asked which models it serves, so the name
/// is picked from a list rather than remembered.
fn choose_model(console: &mut Console, served: bool) -> ModelChoice {
    let example_model = EXAMPLE_MODEL_ID;
    println!();
    println!("  {}", console.bold("The model"));
    console.note(
        "the assistant talks to a model through the gateway, over the OpenAI chat API, which \
         almost every model server and provider speaks",
    );
    let pick = console.choice(
        "What should it talk to?",
        &[
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
            (
                "Decide later",
                "install it without a model now; run nils setup again to name one",
            ),
        ],
        if served { 0 } else { 3 },
    );

    if pick == 3 {
        console.note(&format!(
            "the gateway is pointed at {DEFAULT_MODEL_URL} for now; the assistant answers once a \
             model runs there, or once you run nils setup and name another"
        ));
        return ModelChoice {
            url: DEFAULT_MODEL_URL.to_string(),
            local: true,
            key: None,
            model: example_model.to_string(),
            later: true,
        };
    }

    let (url, local) = match pick {
        0 => {
            console.note(
                "SGLang listens on http://127.0.0.1:30000/v1, vLLM on :8000/v1, llama.cpp on \
                 :8080/v1 and Ollama on :11434/v1",
            );
            (ask_address(console, DEFAULT_MODEL_URL), true)
        }
        1 => {
            console.note(
                "the address of that server as this machine reaches it, for example \
                 http://192.168.1.20:30000/v1",
            );
            (ask_address(console, ""), true)
        }
        _ => {
            console.note(
                "its OpenAI compatible address: https://api.openai.com/v1, \
                 https://openrouter.ai/api/v1 or https://api.minimax.io/v1, among others",
            );
            (ask_address(console, "https://api.openai.com/v1"), false)
        }
    };

    let key = if local {
        if console.yes_no("Does that server need a key?", false) {
            console.hidden_once("Its key")
        } else {
            None
        }
    } else {
        let key = console.hidden_once("Your key from that provider");
        if key.is_none() {
            console.note("no key was given; the provider will refuse the gateway until one is");
        }
        key
    };

    let model = match list_models(&url, key.as_deref()) {
        Some(ids) if ids.len() == 1 => {
            console.note(&format!("{url} answered, serving {}", ids[0]));
            ids[0].clone()
        }
        Some(ids) if !ids.is_empty() => {
            console.note(&format!("{url} answered"));
            let shown: Vec<(&str, &str)> =
                ids.iter().take(12).map(|id| (id.as_str(), "")).collect();
            let at = console.choice("Which model?", &shown, 0);
            ids[at].clone()
        }
        _ => {
            console.note(&format!(
                "nothing answered at {url} yet; the gateway will reach it once it is running"
            ));
            let default = if local { example_model } else { "" };
            loop {
                let named = console.line("The model's name, as the server lists it", default);
                if !named.trim().is_empty() || !console.interactive() {
                    break named.trim().to_string();
                }
                console.note("a model's name is needed, for example gpt-4.1-mini");
            }
        }
    };

    if !local {
        console.note(
            "the gateway keeps your registry's rows on your own systems unless you decide \
             otherwise, so questions that read the registry are refused by this provider until \
             you open them: https://kineuro.se/nils/docs/assistant/kvasir/",
        );
    }
    ModelChoice {
        url,
        local,
        key,
        model,
        later: false,
    }
}

/// An http or https address, asked until one is given.
fn ask_address(console: &mut Console, default: &str) -> String {
    loop {
        let url = console
            .line("Its address", default)
            .trim()
            .trim_end_matches('/')
            .to_string();
        if url.starts_with("http://") || url.starts_with("https://") {
            return url;
        }
        if !console.interactive() {
            return DEFAULT_MODEL_URL.to_string();
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

/// The purposes the assistant uses: the host's own, from the gateway's
/// example, and one for each station the assistant ships, read from that
/// station's own file. The gateway refuses a purpose it was not told of, so
/// a station missing here is a station that never answers.
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

/// The gateway's configuration: the port this setup chose, an admin token
/// made here, one backend for the model the person named, and every purpose
/// the assistant uses. The file holds that token and a provider's key path,
/// so it is written readable by nobody else.
///
/// An existing file is repaired rather than rewritten, because a person may
/// have named their model in it by hand. What is repaired is what an earlier
/// version of this wizard got wrong: a key file copied from the example that
/// is not on this machine, which stops the gateway at start; the example's
/// commercial backends, which nobody chose and which have no key; and
/// purposes the assistant's stations use and the file never declared.
fn configure_kvasir(
    plan: &Plan,
    console: &mut Console,
    chosen: Option<&ModelChoice>,
) -> Result<(), Exit> {
    let dir = plan.dir.join("kvasir");
    let config = dir.join("kvasir.json");
    if config.exists() {
        return repair_kvasir(plan, console);
    }
    let example = dir.join("kvasir.example.json");
    let text = std::fs::read_to_string(&example)
        .map_err(|e| fail(format!("{}: {e}", example.display())))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| fail(format!("{}: {e}", example.display())))?;

    value["bind"] = serde_json::json!(format!("127.0.0.1:{}", plan.ports.kvasir));
    value["origin"] = serde_json::json!(format!("http://127.0.0.1:{}", plan.ports.kvasir));
    let admin = generated_passphrase();
    value["auth"] = serde_json::json!({
        "mode": "token",
        "tokens": { admin: "nils-setup:admin" },
    });

    let template = value["backends"][0].clone();
    let example_model = template["models"][0]["id"]
        .as_str()
        .unwrap_or(EXAMPLE_MODEL_ID)
        .to_string();
    // Nobody was asked (a run with no terminal, or --yes): the gateway gets
    // the example's model on SGLang's port, and the plan already said so.
    let later = ModelChoice {
        url: DEFAULT_MODEL_URL.to_string(),
        local: true,
        key: None,
        model: example_model.clone(),
        later: true,
    };
    let chosen = chosen.unwrap_or(&later);
    let model_id = if chosen.model.is_empty() {
        example_model.clone()
    } else {
        chosen.model.clone()
    };

    let mut backend = template;
    if let Some(fields) = backend.as_object_mut() {
        fields.remove("keyFile");
        fields.remove("key");
        fields.remove("provider");
    }
    backend["id"] = serde_json::json!("model");
    backend["baseUrl"] = serde_json::json!(chosen.url);
    backend["locality"] = serde_json::json!(if chosen.local { "local" } else { "remote" });
    backend["models"][0]["id"] = serde_json::json!(model_id);
    backend["models"][0]["name"] = serde_json::json!(model_id);
    if let Some(key) = &chosen.key {
        let path = dir.join("model.key");
        write_secret(&path, key)?;
        backend["keyFile"] = serde_json::json!(path.display().to_string());
    }
    value["backends"] = serde_json::json!([backend]);
    value["purposes"] = serde_json::json!(assistant_purposes(plan, &value["purposes"]));

    write_secret_bytes(
        &config,
        serde_json::to_string_pretty(&value)
            .unwrap_or(text)
            .as_bytes(),
    )?;
    if chosen.later {
        console.note(&format!(
            "{} is written, with no model named yet",
            config.display()
        ));
    } else {
        console.note(&format!(
            "{} sends the assistant to {} at {}",
            config.display(),
            model_id,
            chosen.url
        ));
    }
    Ok(())
}

/// Mend what an earlier version of this wizard wrote, and say what changed.
fn repair_kvasir(plan: &Plan, console: &mut Console) -> Result<(), Exit> {
    let config = plan.dir.join("kvasir").join("kvasir.json");
    let text =
        std::fs::read_to_string(&config).map_err(|e| fail(format!("{}: {e}", config.display())))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| fail(format!("{}: {e}", config.display())))?;
    let mut mended: Vec<String> = Vec::new();

    if let Some(backends) = value["backends"].as_array_mut() {
        for backend in backends.iter_mut() {
            let missing = backend["keyFile"]
                .as_str()
                .is_some_and(|path| !Path::new(path).exists());
            if missing {
                let path = backend["keyFile"].as_str().unwrap_or_default().to_string();
                if let Some(fields) = backend.as_object_mut() {
                    fields.remove("keyFile");
                }
                mended.push(format!(
                    "dropped a key file that is not on this machine ({path})"
                ));
            }
        }
        let before = backends.len();
        backends.retain(|b| {
            let keyless = b.get("keyFile").is_none() && b.get("key").is_none();
            !(b["locality"] == "remote" && keyless)
        });
        if backends.len() < before {
            mended.push(format!(
                "dropped {} remote backend(s) with no key, which nobody chose",
                before - backends.len()
            ));
        }
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

/// After `nils update` moves this binary, the setup record says so. The
/// wizard opens by naming what is installed, and it named the version the
/// install began with until something rewrote the record.
pub(crate) fn record_engine_version(path: &Path, version: &str) {
    let Some(mut state) = read_state() else {
        return;
    };
    let Some(engine) = state.parts.get_mut("engine") else {
        return;
    };
    let recorded =
        std::fs::canonicalize(&engine.path).unwrap_or_else(|_| PathBuf::from(&engine.path));
    let moved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if engine.kind != "binary" || recorded != moved {
        return;
    }
    engine.version = version.to_string();
    let _ = write_state(&state);
}

/// Everything the assistant reads from its environment, in one file its
/// service reads. Nothing here is a secret: the gateway's key is a path.
fn write_assistant_env(plan: &Plan) -> Result<(), Exit> {
    let dir = plan.dir.join("assistant");
    let path = dir.join("assistant.env");
    if path.exists() {
        return Ok(());
    }
    let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    // The model the assistant asks for is the first one the gateway serves.
    let model = std::fs::read_to_string(plan.dir.join("kvasir").join("kvasir.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|c| {
            c["backends"][0]["models"][0]["id"]
                .as_str()
                .map(str::to_string)
        });
    let mut text = String::from("# Written by nils setup.\n");
    if let Some(model) = model {
        let _ = writeln!(text, "ASSISTANT_MODEL={model}");
    }
    let _ = writeln!(text, "NILS_URL=http://127.0.0.1:{}", plan.ports.engine);
    let _ = writeln!(text, "KVASIR_URL=http://127.0.0.1:{}", plan.ports.kvasir);
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
    std::fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

/// The token this setup made for the gateway, read back from its own file.
fn gateway_admin_token(plan: &Plan) -> Option<String> {
    let text = std::fs::read_to_string(plan.dir.join("kvasir").join("kvasir.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["auth"]["tokens"].as_object()?.keys().next().cloned()
}

/// Whether the gateway answers its health door, waiting for it a while. A
/// service that was started a moment ago is not up yet, and asking it once
/// and giving up is how an install ended by printing a command to run by
/// hand instead of doing the thing.
fn gateway_up(plan: &Plan, console: &Console, seconds: u64) -> bool {
    let url = format!("http://127.0.0.1:{}/healthz", plan.ports.kvasir);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(1)))
        .build()
        .into();
    let started = std::time::Instant::now();
    let live = std::io::stdout().is_terminal();
    let up = loop {
        if agent.get(&url).call().is_ok() {
            break true;
        }
        if started.elapsed().as_secs() >= seconds {
            break false;
        }
        if live {
            print!(
                "\r  waiting for the gateway {}",
                console.dim(&format!("{}s", started.elapsed().as_secs()))
            );
            let _ = std::io::stdout().flush();
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    if live {
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }
    up
}

/// The assistant's key at the gateway, for every purpose the gateway
/// declares for it. Waits for the gateway first; where it never comes up,
/// the reason is the gateway's own and is shown with the services, so this
/// says only what did not happen and how it will.
fn mint_assistant_key(plan: &Plan, console: &Console) -> bool {
    let path = plan.dir.join("kvasir").join("assistant.key");
    if path.exists() {
        return true;
    }
    let Some(admin) = gateway_admin_token(plan) else {
        return false;
    };
    if !gateway_up(plan, console, 30) {
        println!("  the gateway did not come up, so the assistant has no key yet");
        println!("  once it runs, nils setup and then repair makes the key");
        return false;
    }
    let purposes: Vec<String> =
        std::fs::read_to_string(plan.dir.join("kvasir").join("kvasir.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
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
    let body = serde_json::json!({
        "principal": "nils-assistant",
        "purposes": purposes,
        "max_class": "rows",
    });
    let minted = ureq::post(&format!("http://127.0.0.1:{}/v1/keys", plan.ports.kvasir))
        .header("authorization", &format!("Bearer {admin}"))
        .header("content-type", "application/json")
        .send(body.to_string())
        .ok()
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|text| {
            let answer: serde_json::Value = serde_json::from_str(&text).ok()?;
            Some(answer.get("key")?.as_str()?.to_string())
        });
    match minted {
        Some(key) if write_secret(&path, &key).is_ok() => {
            console.note(&format!("the assistant's key is at {}", path.display()));
            true
        }
        _ => {
            println!("  the gateway would not make the assistant's key");
            println!("  nils setup and then repair asks it again");
            false
        }
    }
}

fn run_in(dir: &Path, program: &str, args: &[&str]) -> Result<(), Exit> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| fail(format!("{program}: {e}")))?;
    if !status.success() {
        return Err(fail(format!("{program} {} failed", args.join(" "))));
    }
    Ok(())
}

// ------------------------------------------------------------- the services

fn units_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("systemd")
        .join("user")
}

fn quadlet_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("containers")
        .join("systemd")
}

/// Whatever this machine and runtime use to keep the parts running.
fn start_everything(plan: &Plan, state: &State, console: &Console) -> Result<String, Exit> {
    match (plan.runtime, cfg!(target_os = "macos")) {
        (Runtime::Podman, _) => {
            let dir = quadlet_dir();
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let mut names = Vec::new();
            for (name, text) in quadlets(plan) {
                std::fs::write(dir.join(&name), text)
                    .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
                names.push(name);
            }
            let _ = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            // A quadlet's unit is generated, so it is never enabled: the
            // [Install] section of the file is what systemd reads. Restart
            // starts a stopped one and takes up a new image tag.
            for unit in ["nils-engine", "nils-desk"] {
                if unit == "nils-desk" && !plan.has(Part::Desk) {
                    continue;
                }
                let _ = Command::new("systemctl")
                    .args(["--user", "restart", unit])
                    .status();
            }
            linger();
            Ok(format!(
                "  services: {} in {}\n",
                names.join(", "),
                dir.display()
            ))
        }
        (Runtime::Docker, _) => {
            let path = plan.dir.join("compose.yaml");
            std::fs::write(&path, docker_compose(plan))
                .map_err(|e| fail(format!("{}: {e}", path.display())))?;
            let _ = Command::new("docker")
                .args(["compose", "up", "-d"])
                .current_dir(&plan.dir)
                .status();
            Ok(format!(
                "  services: {} (docker compose up -d; add it to your machine's own start up)\n",
                path.display()
            ))
        }
        (Runtime::Machine, true) => {
            let dir = std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join("Library").join("LaunchAgents"))
                .ok_or_else(|| fail("no home directory"))?;
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let mut names = Vec::new();
            for (name, text) in launchd_plists(plan, state) {
                let path = dir.join(&name);
                std::fs::write(&path, text)
                    .map_err(|e| fail(format!("{}: {e}", path.display())))?;
                let _ = Command::new("launchctl")
                    .args(["load", "-w"])
                    .arg(&path)
                    .status();
                names.push(name);
            }
            Ok(format!(
                "  services: {} in {}\n",
                names.join(", "),
                dir.display()
            ))
        }
        (Runtime::Machine, false) => {
            let dir = units_dir();
            std::fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
            let mut names = Vec::new();
            for (name, text) in systemd_units(plan, state) {
                std::fs::write(dir.join(&name), text)
                    .map_err(|e| fail(format!("{}: {e}", dir.display())))?;
                names.push(name.trim_end_matches(".service").to_string());
            }
            quietly("systemctl", &["--user", "daemon-reload"]);
            // Enabling prints a line for every link it makes, which is
            // systemd's business and not the person's.
            for unit in &names {
                quietly("systemctl", &["--user", "enable", unit]);
            }
            // The assistant reads a key the gateway makes, so everything
            // else starts first, the key is made once the gateway answers,
            // and the assistant starts last. Started together, the
            // assistant died looking for a key that did not exist yet.
            let (assistant, rest): (Vec<&String>, Vec<&String>) =
                names.iter().partition(|u| u.as_str() == "nils-assistant");
            for unit in &rest {
                quietly("systemctl", &["--user", "restart", unit]);
            }
            if !assistant.is_empty() {
                mint_assistant_key(plan, console);
                for unit in &assistant {
                    quietly("systemctl", &["--user", "restart", unit]);
                }
            }
            linger();
            Ok(unit_report(&names, console))
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

/// Which units are running, said plainly, with the reason for any that is
/// not. Each is looked at twice, two seconds apart: a service that fails at
/// start is active for an instant and then restarting, and a single look
/// at that instant reported it running while it crashed in a loop.
fn unit_report(names: &[String], console: &Console) -> String {
    let look = |unit: &str| {
        run_quiet("systemctl", &["--user", "is-active", unit]).is_some_and(|s| s.trim() == "active")
    };
    console.note("checking that the services came up");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let first: Vec<bool> = names.iter().map(|u| look(u)).collect();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let mut running = Vec::new();
    let mut stopped = Vec::new();
    for (unit, was) in names.iter().zip(first) {
        if was && look(unit) {
            running.push(unit.as_str());
        } else {
            stopped.push(unit.as_str());
        }
    }
    let mut out = String::new();
    if !running.is_empty() {
        let _ = writeln!(out, "  running: {}", running.join(", "));
    }
    for unit in stopped {
        let _ = writeln!(out, "  not running: {unit}");
        if let Some(why) = last_error(unit) {
            let _ = writeln!(out, "    it said: {why}");
        }
        let _ = writeln!(out, "    its log: journalctl --user -u {unit}");
    }
    out
}

/// The line of a unit's log most likely to say why it stopped.
fn last_error(unit: &str) -> Option<String> {
    let log = run_quiet(
        "journalctl",
        &["--user", "-u", unit, "-n", "80", "--no-pager", "-o", "cat"],
    )?;
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

fn linger() {
    if let Some(user) = std::env::var_os("USER") {
        let _ = Command::new("loginctl")
            .arg("enable-linger")
            .arg(user)
            .status();
    }
}

/// One unit per part, for the parts that run on the machine.
pub(crate) fn systemd_units(plan: &Plan, state: &State) -> Vec<(String, String)> {
    let engine = state
        .parts
        .get("engine")
        .map(|p| p.path.clone())
        .unwrap_or_else(|| "nils".to_string());
    let mut out = vec![(
        "nils-engine.service".to_string(),
        format!(
            "[Unit]\nDescription=NILS engine\nAfter=network-online.target\n\n[Service]\n\
             ExecStart={engine} {}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
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
                "[Unit]\nDescription=NILS desk\nAfter=nils-engine.service\n\n[Service]\n\
                 ExecStart={} serve --config {}\nWorkingDirectory={}\nRestart=on-failure\n\n\
                 [Install]\nWantedBy=default.target\n",
                desk.path,
                plan.desk_config().display(),
                plan.desk_dir().display(),
            ),
        ));
    }
    if state.parts.contains_key("kvasir") {
        out.push((
            "kvasir.service".to_string(),
            format!(
                "[Unit]\nDescription=Kvasir, the model gateway\n\n[Service]\n\
                 ExecStart=/usr/bin/env node dist/main.js --config kvasir.json\n\
                 WorkingDirectory={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
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
                 [Service]\nEnvironmentFile={}\n\
                 ExecStart=/usr/bin/env node dist/app/server.mjs\n\
                 WorkingDirectory={}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
                dir.join("assistant.env").display(),
                dir.display()
            ),
        ));
    }
    out
}

/// The same, as launchd agents, for a machine with no systemd.
pub(crate) fn launchd_plists(plan: &Plan, state: &State) -> Vec<(String, String)> {
    let plist = |label: &str, argv: Vec<String>, cwd: &str| {
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
    };
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
    out
}

/// The last lines: what is there, where to open it, what to run.
fn summary(plan: &Plan, console: &Console) {
    let (_, origin, _) = desk_binding(&plan.reach, plan.ports.desk, plan.runtime.container());
    println!();
    println!("{}", console.bold("Installed"));
    println!("  everything under {}", plan.dir.display());
    if plan.has(Part::Desk) {
        println!("  the desk at {origin}");
    }
    println!("  the engine on port {}", plan.ports.engine);
    println!();
    if !plan.service && plan.runtime == Runtime::Machine {
        // The same arguments the services would have been given, so a port
        // this setup moved and the trust a desk that keeps its own people
        // needs are in the command a person copies, not only in a unit.
        println!("{}", console.bold("Start it"));
        let registry = plan.registry().display().to_string();
        let backups = plan.dir.join("backups").display().to_string();
        println!(
            "  nils {}",
            engine_args(plan, &registry, &backups).join(" ")
        );
        if plan.has(Part::Desk) {
            println!(
                "  nils-desk serve --config {}",
                plan.desk_config().display()
            );
        }
        if plan.has(Part::Assistant) {
            println!(
                "  (cd {} && node dist/main.js --config kvasir.json)",
                plan.dir.join("kvasir").display()
            );
            println!(
                "  (cd {} && set -a && . ./assistant.env && node dist/app/server.mjs)",
                plan.dir.join("assistant").display()
            );
            println!(
                "  {}",
                console
                    .dim("once the gateway runs, nils setup and repair makes the assistant's key")
            );
        }
        println!();
    }
    println!("{}", console.bold("Then"));
    println!("  bring data in with: nils digest <a directory of DICOM files>");
    println!("  the documentation is at https://kineuro.se/nils/docs/");
    println!("  the next version, when there is one: nils update --all");
}

// -------------------------------------------------------------- the update

/// Every part the state file names, brought up to date in whatever way that
/// part runs: a binary is replaced, a container is a pull of the new tag and
/// a restart of its unit, a Node part is a fetch and a rebuild. One line
/// each. The engine is not among them: `nils update` replaces the binary
/// doing the replacing, and it goes last.
pub(crate) fn update_all(channel: Option<&str>) -> Result<(), Exit> {
    let mut state = read_state().ok_or_else(|| {
        fail(format!(
            "no setup is recorded at {}; nils update takes the engine alone",
            state_path().display()
        ))
    })?;
    let plan = plan_from_state(&state, channel);
    let newest = update::newest_version(&update::engine_base(channel)).ok();
    let mut units = false;

    let names: Vec<String> = state
        .parts
        .keys()
        .filter(|n| n.as_str() != "engine")
        .cloned()
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
                let pulled = Command::new(&part.kind)
                    .args(["pull", &tag])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !pulled {
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
                units = true;
                println!("{name}: {tag}");
            }
            "node" => {
                let dir = PathBuf::from(&part.path);
                let built = run_in(&dir, "git", &["pull", "--ff-only"])
                    .and_then(|()| run_in(&dir, "npm", &["ci", "--no-audit", "--no-fund"]))
                    .and_then(|()| run_in(&dir, "npm", &["run", "build"]));
                match built {
                    Ok(()) => println!("{name}: fetched and rebuilt in {}", dir.display()),
                    Err(e) => println!("{name}: {}", e.message),
                }
            }
            _ => match update_binary_part(&name, &part.path, &update::desk_base(channel)) {
                Ok((version, said)) => {
                    println!("{name}: {said}");
                    state
                        .parts
                        .insert(name.clone(), PartState { version, ..part });
                }
                Err(e) => println!("{name}: {}", e.message),
            },
        }
    }

    // A container's unit names the tag, so the units are written again with
    // the new one and the parts restarted from them.
    if units {
        let mut fresh = plan;
        fresh.version = newest
            .clone()
            .unwrap_or_else(|| update::VERSION.to_string());
        let console = Console::new(true);
        match start_everything(&fresh, &state, &console) {
            Ok(what) => print!("{what}"),
            Err(e) => println!("the services were left alone: {}", e.message),
        }
    }
    state.at = nils_registry::time::now_iso();
    write_state(&state)?;
    Ok(())
}

/// One binary part from its own releases, when a newer one is published.
fn update_binary_part(name: &str, path: &str, base: &str) -> Result<(String, String), Exit> {
    if name != "desk" {
        return Err(fail(format!("{name} is not a part this can update")));
    }
    let version = update::newest_version(base)?;
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
    /// What building the gateway and the assistant made.
    built: Vec<PathBuf>,
    state: PathBuf,
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
        return Err(fail(format!(
            "no setup is recorded at {}, so there is nothing this knows it installed; the \
             documentation says how to remove a hand made install: \
             https://kineuro.se/nils/docs/intro/install/",
            state_path().display()
        )));
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
                        "the services, the programs and the packs; the registry and its key, the \
                         backups, the desk's people and the assistant's history stay in {dir}"
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
        "{} in {}, {} mode, {}",
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
        || dir.join("desk").join("nils-desk.toml").exists();
    if !made_here {
        return Err("it holds neither a registry nor a desk that an install made".to_string());
    }
    Ok(())
}

/// Gather what an uninstall would remove, looking at what is really there.
fn gather_removal(state: &State, me: Option<PathBuf>, leaving: Leaving) -> Removal {
    let dir = PathBuf::from(&state.dir);
    let mut removal = Removal {
        dir: dir.clone(),
        runtime: state.runtime.clone(),
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
        state: state_path(),
    };

    // services
    match state.runtime.as_str() {
        "podman" => {
            for (unit, file) in [
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
            removal.containers = vec!["nils-desk".to_string(), "nils-engine".to_string()];
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
                let path = units_dir().join(format!("{unit}.service"));
                if path.exists() {
                    removal.units.push(unit.to_string());
                    removal.unit_files.push(path);
                }
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

    // what building the gateway and the assistant made; the rest of those
    // directories holds their data and their configuration
    for part in state.parts.values().filter(|p| p.kind == "node") {
        let path = PathBuf::from(&part.path);
        let outside = !path.starts_with(&dir);
        if leaving == Leaving::Purge && outside {
            removal.built.push(path);
            continue;
        }
        if leaving == Leaving::KeepData {
            for sub in ["node_modules", "dist"] {
                if path.join(sub).exists() {
                    removal.built.push(path.join(sub));
                }
            }
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

    if !removal.units.is_empty() && removal.runtime != "docker" {
        let mut args = vec!["--user", "disable", "--now"];
        args.extend(removal.units.iter().map(String::as_str));
        quietly("systemctl", &args);
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
        quietly("systemctl", &["--user", "daemon-reload"]);
        let mut args = vec!["--user", "reset-failed"];
        args.extend(removal.units.iter().map(String::as_str));
        quietly("systemctl", &args);
        say(format!(
            "stopped and removed {}",
            if removal.units.is_empty() {
                "the services".to_string()
            } else {
                removal.units.join(", ")
            }
        ));
    }

    match removal.runtime.as_str() {
        "podman" => {
            if quietly("podman", &["pod", "rm", "-f", "nils"]) {
                say("removed the nils pod".to_string());
            }
        }
        "docker" => {
            let mut args = vec!["rm", "-f"];
            args.extend(removal.containers.iter().map(String::as_str));
            quietly("docker", &args);
            quietly("docker", &["network", "rm", "nils"]);
            say(format!("removed {}", removal.containers.join(", ")));
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
        match remove_path(&removal.dir, &removal.runtime) {
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
            registry_exists: false,
            service: true,
            channel: None,
            version: "1.0.0-alpha.2".to_string(),
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

    #[test]
    fn every_station_s_purpose_is_declared_to_the_gateway() {
        // The gateway refuses a purpose it was not told of. The example
        // declared three; the assistant's stations use more, so on a fresh
        // install three of them would have answered nothing but a refusal.
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_mends_what_the_first_wizard_wrote_and_nothing_else() {
        // What alpha.4 left: the example's key file path, which is not on a
        // laptop and stopped the gateway at start; two commercial backends
        // with no key, which nobody chose; purposes missing for stations.
        let dir = scratch("repair");
        station(&dir, "concierge", "assistant.concierge", "rows");
        let kvasir = dir.join("kvasir");
        std::fs::create_dir_all(&kvasir).unwrap();
        let written = serde_json::json!({
            "auth": {"mode": "token", "tokens": {"tok": "nils-setup:admin"}},
            "backends": [
                {"id": "card", "baseUrl": "http://127.0.0.1:30000/v1", "locality": "local",
                 "keyFile": "/etc/kvasir/card.key", "models": [{"id": "m"}]},
                {"id": "minimax-openai", "baseUrl": "https://api.minimax.io/v1", "locality": "remote",
                 "provider": "minimax", "models": [{"id": "x"}]},
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

        let mended: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(kvasir.join("kvasir.json")).unwrap())
                .unwrap();
        let backends = mended["backends"].as_array().unwrap();
        let ids: Vec<&str> = backends.iter().filter_map(|b| b["id"].as_str()).collect();
        assert_eq!(
            ids,
            vec!["card", "kept"],
            "the keyless remote one goes, a keyed one stays"
        );
        assert!(
            backends[0].get("keyFile").is_none(),
            "a key file that is not there is dropped"
        );
        assert_eq!(
            backends[0]["baseUrl"], "http://127.0.0.1:30000/v1",
            "the model named is untouched"
        );
        let purposes: Vec<&str> = mended["purposes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["id"].as_str())
            .collect();
        assert!(purposes.contains(&"assistant.concierge"), "{purposes:?}");
        assert_eq!(gateway_admin_token(&plan).as_deref(), Some("tok"));

        // and a second repair has nothing left to do
        let before = std::fs::read_to_string(kvasir.join("kvasir.json")).unwrap();
        assert!(repair_kvasir(&plan, &mut console).is_ok());
        assert_eq!(
            std::fs::read_to_string(kvasir.join("kvasir.json")).unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(&dir);
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
    fn podman_runs_a_pod_and_owns_its_mounts() {
        let commands = podman_commands(&plan(Runtime::Podman));
        assert!(commands[0].contains("pod create --name nils -p 127.0.0.1:7200:7200"));
        let engine = &commands[1];
        assert!(engine.contains("--pod nils"), "{engine}");
        assert!(engine.contains(":/srv/nils/registry:U"), "{engine}");
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
            places: vec![PlaceState {
                name: "source".to_string(),
                role: "source".to_string(),
                path: "/data/source".to_string(),
            }],
            ..State::default()
        };
        let plan = plan_from_state(&state, None);
        assert_eq!(plan.mode, Mode::Local);
        assert_eq!(plan.runtime, Runtime::Docker);
        assert_eq!(plan.reach, Reach::Network("10.0.0.5".to_string()));
        assert_eq!(plan.ports.engine, 9000);
        assert_eq!(plan.source.as_deref(), Some(Path::new("/data/source")));
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
