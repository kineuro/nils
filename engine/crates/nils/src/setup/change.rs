// SPDX-License-Identifier: AGPL-3.0-only
//! Record 55 ruling 35 (c), T31: the steps of `nils setup` the desk asks for
//! through the supervisor, so that none of them is a command printed in a
//! desk flow. How people sign in, where the engine writes its archives, the
//! stations' model server, and an archive put back over the registry.
//!
//! The supervisor's door checks what it was asked, leaves it in a request
//! file of its own folder (`<dir>/supervise/requests/`, read and written by
//! its account alone) and starts a run of `nils supervise change` or `nils
//! supervise restore`. Where the services are this machine's own, that run
//! asks the helper for it with one fixed word and no argument, so the sudoers
//! rule names whole command lines as it always did, and root reads the
//! request, checks it again and takes it away. Everything else is what `nils
//! setup` does for the same answers: the record written, then the install
//! repaired from it.
//!
//! Every change is audited in the registry under the person who asked for it,
//! which the desk names in `X-Nils-Person`; a restore writes its row into the
//! registry as it was put back, so the row is there to read afterwards.

use super::*;
use serde_json::{Value, json};

/// The word, the helper's and the request file's, of a change of the setup.
pub(crate) const CHANGE: &str = "change";
/// The same for an archive put back over the registry.
pub(crate) const RESTORE: &str = "restore";

/// The folder the supervisor leaves a request in for the account that makes
/// it, inside its own folder.
pub(crate) fn requests_dir(dir: &Path) -> PathBuf {
    dir.join("supervise").join("requests")
}

/// The first person of a desk going to `local` mode where it keeps nobody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct First {
    pub(crate) name: String,
    pub(crate) password: String,
}

/// A provider the desk is registered at already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Registered {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) roles_claim: Option<String>,
}

/// What a person asked the install to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "lowercase")]
pub(crate) enum Change {
    /// How people sign in: `local` or `oidc`.
    Signin {
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first: Option<First>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oidc: Option<Registered>,
    },
    /// Where the engine writes its archives.
    Backups { dir: String },
    /// The model server the stations use, its key and the model.
    Model {
        server: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// An archive of this registry's backup folder put back over it.
    Restore { archive: String },
}

/// A change and who asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Request {
    /// Who asked, as `user@node`: the person the desk named, else the
    /// supervisor's token.
    pub(crate) by: String,
    #[serde(flatten)]
    pub(crate) change: Change,
}

impl Request {
    /// The run that makes it, and the request file it is left in.
    pub(crate) fn word(&self) -> &'static str {
        match self.change {
            Change::Restore { .. } => RESTORE,
            _ => CHANGE,
        }
    }

    /// What the audit row says of it: what changed, never a secret.
    fn scope(&self) -> Value {
        match &self.change {
            Change::Signin { mode, first, oidc } => json!({
                "what": "signin",
                "mode": mode,
                "issuer": oidc.as_ref().map(|o| o.issuer.trim()),
                "client_id": oidc.as_ref().map(|o| o.client_id.trim()),
                "first": first.as_ref().map(|f| f.name.trim()),
            }),
            Change::Backups { dir } => json!({ "what": "backups", "dir": dir.trim() }),
            Change::Model { server, model, .. } => json!({
                "what": "model",
                "server": server.trim().trim_end_matches('/'),
                "model": model,
            }),
            Change::Restore { archive } => json!({ "what": "restore", "archive": archive }),
        }
    }
}

/// Whether an address is of this machine's own loopback, where a provider
/// may answer on plain http.
fn loopback(url: &str) -> bool {
    let rest = url.trim_start_matches("http://");
    rest.starts_with("[::1]")
        || matches!(
            rest.split(['/', ':']).next(),
            Some("127.0.0.1" | "localhost")
        )
}

/// An archive's name as the backup folder lists it: one folder's own name,
/// never a path.
fn archive_name_refusal(name: &str) -> Option<&'static str> {
    let fine = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    (name.is_empty() || name.starts_with('.') || !name.chars().all(fine))
        .then_some("archive: the name of one archive in the engine's backup folder")
}

/// Why a request cannot be made on this install, with the status a door
/// answers it with: 400 for what the request says, 409 for what the install
/// is. Asked by the door before anything is written, and again by the
/// account that makes the change, which never takes the door's word for it.
pub(crate) fn refusal(state: &State, request: &Request) -> Option<(u16, String)> {
    let bad = |why: &str| Some((400, why.to_string()));
    let install = |why: String| Some((409, why));
    if nils_registry::principal::Principal::parse(&request.by).is_none() {
        return bad("by: who asked, as user@node");
    }
    if service_units(state).is_empty() {
        return install(
            "this install runs no services, so nothing keeps it running to change it: run nils \
             setup on the machine"
                .to_string(),
        );
    }
    match &request.change {
        Change::Signin { mode, first, oidc } => {
            match mode.as_str() {
                "local" | "oidc" => {}
                "off" => {
                    return bad(
                        "mode: local or oidc; with nobody signing in, whoever reaches the desk \
                         is its operator, so that is set at the machine with nils setup",
                    );
                }
                _ => return bad("mode: local or oidc"),
            }
            if !state.parts.contains_key("desk") {
                return install("this install has no desk to sign in at".to_string());
            }
            match mode.as_str() {
                "local" => {
                    if state.mode == "local" {
                        return install("people sign in with a password here already".to_string());
                    }
                    if let Some(first) = first {
                        if let Some(why) = desk_username_refusal(first.name.trim()) {
                            return bad(&format!("first.name: {why}"));
                        }
                        if let Some(why) = desk_password_refusal(&first.password) {
                            return bad(&format!("first.password: {why}"));
                        }
                    }
                }
                "oidc" => {
                    let Some(p) = oidc else {
                        return bad("oidc: the issuer, the desk's client id there and its secret");
                    };
                    let issuer = p.issuer.trim();
                    if !(issuer.starts_with("https://")
                        || (issuer.starts_with("http://") && loopback(issuer)))
                    {
                        return bad("oidc.issuer: an address starting with https://");
                    }
                    if p.client_id.trim().is_empty() || p.client_secret.trim().is_empty() {
                        return bad("oidc: the desk's client id and its secret");
                    }
                    if p.roles_claim
                        .as_deref()
                        .is_some_and(|c| c.trim().is_empty() || c.contains(char::is_whitespace))
                    {
                        return bad("oidc.roles_claim: one claim's name");
                    }
                    let same = state
                        .oidc
                        .as_ref()
                        .is_some_and(|o| o.issuer == issuer && o.client_id == p.client_id.trim());
                    if state.mode == "oidc" && same {
                        return install(format!("people sign in at {issuer} already"));
                    }
                }
                _ => {}
            }
        }
        Change::Backups { dir } => {
            let dir = dir.trim();
            let path = Path::new(dir);
            if !path.is_absolute() || path.components().any(|c| c.as_os_str() == "..") {
                return bad("dir: a folder's whole path on this machine, as /data/archives");
            }
            let registry = Path::new(&state.dir).join("registry");
            if path.starts_with(&registry) {
                return bad("dir: a folder beside the registry, not inside it");
            }
        }
        Change::Model { server, key, model } => {
            if !state.parts.contains_key("kvasir") {
                return install(
                    "this install has no assistant, so its stations use no model".to_string(),
                );
            }
            let url = server.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return bad("server: an address starting with http:// or https://");
            }
            if key.trim().is_empty() || key.trim().contains('\n') {
                return bad("key: the server's key, one line");
            }
            if model.as_deref().is_some_and(|m| m.trim().is_empty()) {
                return bad("model: a model the server offers, or none for its default");
            }
        }
        Change::Restore { archive } => {
            if let Some(why) = archive_name_refusal(archive) {
                return bad(why);
            }
            if !service_units(state).iter().any(|u| u.part == "engine") {
                return install("this install runs no service for the engine to stop".to_string());
            }
        }
    }
    None
}

/// The request written where the account that makes it reads it: readable
/// by this account alone, written whole, so a run never reads half of one.
pub(crate) fn leave_request(dir: &Path, request: &Request) -> std::io::Result<()> {
    let folder = requests_dir(dir);
    std::fs::create_dir_all(&folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = folder.join(format!("{}.json", request.word()));
    let fresh = folder.join(format!(".{}.{}", request.word(), std::process::id()));
    let _ = std::fs::remove_file(&fresh);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&fresh)
        .and_then(|mut file| file.write_all(&serde_json::to_vec(request).unwrap_or_default()))
        .and_then(|()| std::fs::rename(&fresh, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
    written
}

/// The request a run makes, read and taken away, so that it is made once.
/// A link in its place is never followed: the folder is another account's
/// where root reads it.
fn take_request(dir: &Path, word: &str) -> Result<Request, Exit> {
    let folder = requests_dir(dir);
    let path = folder.join(format!("{word}.json"));
    let plain = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink());
    if !plain(&folder) || !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
        return Err(usage(format!(
            "nothing was asked for: no request is waiting in {}",
            folder.display()
        )));
    }
    let bytes = std::fs::read(&path).map_err(|e| fail(format!("{}: {e}", path.display())));
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice::<Request>(&bytes?).map_err(|_| {
        usage(format!(
            "nothing was changed: the request in {} does not read",
            folder.display()
        ))
    })
}

/// Whether the helper on this machine, as its text reads, answers a word.
/// One written before the word was among them does not, and its rule does
/// not name it either; asked anyway, sudo refuses it and keeps the refusal
/// in the machine's log.
fn helper_knows(text: &str, word: &str) -> bool {
    let wanted = format!(" supervise {word}");
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("exec ") && line.ends_with(&wanted)
    })
}

/// Why this install's helper cannot be asked for a word, where it cannot:
/// what to do instead. `None` where there is no helper, or it knows it.
pub(crate) fn helper_lacks(state: &State, word: &str) -> Option<String> {
    let helper = state.helper.as_ref()?;
    let text = std::fs::read_to_string(&helper.path).ok()?;
    (!helper_knows(&text, word)).then(|| {
        format!(
            "the helper on this machine was written before it could {}: update this install, or \
             run nils setup again as root, either of which writes the helper and its rule again",
            if word == RESTORE {
                "put an archive back"
            } else {
                "change the setup"
            }
        )
    })
}

/// The changes the supervisor of this install makes, for the install door:
/// the desk offers a button where one is served, and keeps the command where
/// none is. `wait` says why none is, where the helper is older than them.
pub(crate) fn served_doc(state: &State) -> Value {
    let units = service_units(state);
    let mut served: Vec<&str> = Vec::new();
    if !units.is_empty() {
        if state.parts.contains_key("desk") {
            served.push("signin");
        }
        served.push("backups");
        if state.parts.contains_key("kvasir") {
            served.push("model");
        }
        if units.iter().any(|u| u.part == "engine") {
            served.push("restore");
        }
    }
    let wait = helper_lacks(state, CHANGE).or_else(|| helper_lacks(state, RESTORE));
    if wait.is_some() {
        served.clear();
    }
    json!({ "served": served, "wait": wait })
}

/// The install on record, for a run.
fn recorded() -> Result<State, Exit> {
    read_state().ok_or_else(|| {
        usage(format!(
            "no setup is recorded at {}",
            state_path().display()
        ))
    })
}

/// `nils supervise change`: the change waiting in the request file made.
pub(crate) fn change_command() -> Result<(), Exit> {
    let state = recorded()?;
    if let Some(argv) = helper_call(&state, &[CHANGE]) {
        if let Some(why) = helper_lacks(&state, CHANGE) {
            return Err(fail(format!("nothing was changed: {why}")));
        }
        return run_helper(&argv);
    }
    let request = take_request(Path::new(&state.dir), CHANGE)?;
    if let Some((_, why)) = refusal(&state, &request) {
        return Err(usage(format!("nothing was changed: {why}")));
    }
    let said = match &request.change {
        Change::Signin { mode, first, oidc } => {
            signin(&state, mode, first.as_ref(), oidc.as_ref())?
        }
        Change::Backups { dir } => backups(&state, dir.trim())?,
        Change::Model { server, key, model } => {
            model_server(&state, server, key, model.as_deref())?
        }
        Change::Restore { .. } => {
            return Err(usage(
                "nothing was changed: a restore is nils supervise restore",
            ));
        }
    };
    if let Err(why) = audit(&state, &request) {
        println!("the change is not in the audit log: {why}");
    }
    println!("{said}");
    Ok(())
}

/// How people sign in, changed: the record, the provider's secret where one
/// is given, the first person where a desk going to passwords keeps nobody,
/// then the install repaired from the record, which writes the desk's
/// configuration and every part's service again and starts them.
fn signin(
    state: &State,
    mode: &str,
    first: Option<&First>,
    oidc: Option<&Registered>,
) -> Result<String, Exit> {
    let before = plan_from_state(state, None);
    if let Some(refused) = recorded_system_refusal(&before) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    let mut state = state.clone();
    let mut console = Console::new(true);
    let mut first_added = None;
    let said = match mode {
        "local" => {
            let keeps = desk_has_people(&before.desk_dir(), before.runtime.container());
            match (keeps, first) {
                (false, None) => {
                    return Err(usage(
                        "nothing was changed: the desk keeps nobody who signs in with a \
                         password yet, so name the first person, who may do everything",
                    ));
                }
                (false, Some(first)) => first_added = Some(first),
                (true, Some(first)) => console.say(&format!(
                    "the desk keeps people who sign in with a password already, so {} was not \
                     added",
                    first.name.trim()
                )),
                (true, None) => {}
            }
            state.mode = "local".to_string();
            "people sign in with a password the desk keeps".to_string()
        }
        _ => {
            let Some(p) = oidc else {
                return Err(usage("nothing was changed: name the provider"));
            };
            let issuer = p.issuer.trim().to_string();
            let discovery = discover(&issuer);
            let Some(jwks) = discovery.as_ref().and_then(jwks_of) else {
                return Err(usage(format!(
                    "nothing was changed: {issuer} did not say where its keys are; is it the \
                     provider's issuer address?"
                )));
            };
            state.oidc = Some(OidcPlan {
                issuer: issuer.clone(),
                client_id: p.client_id.trim().to_string(),
                jwks,
                roles_claim: p
                    .roles_claim
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("roles")
                    .to_string(),
                scopes: Some(scopes_for(discovery.as_ref())),
            });
            state.mode = "oidc".to_string();
            format!("people sign in at {issuer}")
        }
    };
    let plan = plan_from_state(&state, None);
    let existing = std::fs::read_to_string(plan.desk_config()).ok();
    if let Some(refused) = desk_config_refusal(&plan, existing.as_deref()) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    if let Some(p) = oidc.filter(|_| mode == "oidc") {
        write_secret(&desk_secret_file(&plan), p.client_secret.trim())?;
    }
    write_state(&state)?;
    write_desk_config(&plan)?;
    if let Some(first) = first_added {
        let desk = state
            .parts
            .get("desk")
            .filter(|p| p.kind == "binary")
            .map(|p| PathBuf::from(&p.path));
        let named = (first.name.trim().to_string(), first.password.clone());
        add_first_admin(&plan, desk, Some(&named), &console)?;
    }
    mend(&plan, &state, &mut console)?;
    Ok(format!("{said} now"))
}

/// Where the engine writes its archives, changed: the folder made where it
/// is not there yet and given to the engine's account, the record, then the
/// engine's service written again from it and the engine started again.
fn backups(state: &State, dir: &str) -> Result<String, Exit> {
    let plan = plan_from_state(state, None);
    if let Some(refused) = recorded_system_refusal(&plan) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    let path = PathBuf::from(dir);
    if path == plan.backups() {
        return Err(usage(format!(
            "nothing was changed: the engine writes its archives to {dir} already"
        )));
    }
    if path.exists() && !path.is_dir() {
        return Err(usage(format!(
            "nothing was changed: {dir} is a file, not a folder"
        )));
    }
    if !path.exists() {
        std::fs::create_dir_all(&path)
            .map_err(|e| fail(format!("nothing was changed: {dir}: {e}")))?;
        // a folder this made is the engine's to write; one that was there is
        // the site's own, and its owner stays
        if let Some(system) = &plan.system {
            let account = system.account("engine");
            if let Some((uid, gid)) = account_ids(account) {
                let _ = give_to(&path, uid, gid);
            }
        }
    }
    let mut state = state.clone();
    state.site.get_or_insert_with(Site::default).backup_dir = Some(dir.to_string());
    write_state(&state)?;
    reapply_engine(&state)?;
    Ok(format!(
        "the engine writes its archives to {dir} now; name the folder a backup place for a \
         backup to go there"
    ))
}

/// The model server the stations use, changed: its key in a file of
/// Kvasir's folder that only the account making the change reads, the
/// model asked of the server where none was named, the record, then the
/// install repaired from it, which starts Kvasir again and gives it the
/// server and the key.
fn model_server(
    state: &State,
    server: &str,
    key: &str,
    model: Option<&str>,
) -> Result<String, Exit> {
    let plan = plan_from_state(state, None);
    if let Some(refused) = recorded_system_refusal(&plan) {
        return Err(fail(format!("nothing was changed: {refused}")));
    }
    let url = server.trim().trim_end_matches('/').to_string();
    let kvasir = PathBuf::from(&state.dir).join("kvasir");
    let key_file = kvasir.join("model-server.key");
    // the key is tried from a file of its own first, so a server that does
    // not answer leaves the key of the one on record as it was
    let trying = kvasir.join(".model-server.key.new");
    write_secret(&trying, key.trim())?;
    let thinking = state
        .model_server
        .as_ref()
        .filter(|was| was.url == url)
        .map(|was| was.thinking)
        .unwrap_or_default();
    let mut named = ModelServer {
        url: url.clone(),
        key_file: trying.clone(),
        model: model.map(|m| m.trim().to_string()),
        thinking,
    };
    if named.model.is_none() {
        match unnamed_server_model(&named) {
            Ok(found) => named.model = Some(found),
            Err(why) => {
                let _ = std::fs::remove_file(&trying);
                return Err(usage(
                    why.replace("with --model-server-model", "in the model field"),
                ));
            }
        }
    }
    std::fs::rename(&trying, &key_file).map_err(|e| {
        let _ = std::fs::remove_file(&trying);
        fail(format!("nothing was changed: {}: {e}", key_file.display()))
    })?;
    named.key_file = key_file;
    let mut state = state.clone();
    let model = named.model.clone().unwrap_or_default();
    state.model_server = Some(named);
    write_state(&state)?;
    let plan = plan_from_state(&state, None);
    let mut console = Console::new(true);
    // a server Kvasir does not take is this change failing, not a note
    console.strict.set(true);
    mend(&plan, &state, &mut console)?;
    Ok(format!("the stations use {model} on {url} now"))
}

/// `nils supervise restore`: the archive waiting in the request file put
/// back. The engine is stopped, the archive is put back as the engine's
/// account by `nils restore`, which checks it and writes the pre-restore
/// archive first, and the engine is started again, also where the restore
/// failed, on the registry as it then is.
pub(crate) fn restore_command() -> Result<(), Exit> {
    let state = recorded()?;
    if let Some(argv) = helper_call(&state, &[RESTORE]) {
        if let Some(why) = helper_lacks(&state, RESTORE) {
            return Err(fail(format!("nothing was restored: {why}")));
        }
        return run_helper(&argv);
    }
    let request = take_request(Path::new(&state.dir), RESTORE)?;
    if let Some((_, why)) = refusal(&state, &request) {
        return Err(usage(format!("nothing was restored: {why}")));
    }
    let Change::Restore { archive } = &request.change else {
        return Err(usage(
            "nothing was restored: a change of the setup is nils supervise change",
        ));
    };
    let plan = plan_from_state(&state, None);
    let path = plan.backups().join(archive);
    if !path.join(crate::backup::MANIFEST).is_file() {
        return Err(usage(format!(
            "nothing was restored: {} holds no archive named {archive}",
            plan.backups().display()
        )));
    }
    let units = service_units(&state);
    let Some(unit) = units.iter().find(|u| u.part == "engine") else {
        return Err(usage("nothing was restored: no service runs the engine"));
    };
    let uid = run_quiet("id", &["-u"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let stop = stop_argv(unit, &uid);
    let words: Vec<&str> = stop[1..].iter().map(String::as_str).collect();
    if !quietly(&stop[0], &words) {
        return Err(fail(format!(
            "nothing was restored: the engine was not stopped ({} was refused)",
            stop.join(" ")
        )));
    }
    println!("{} is stopped", unit.name);
    let restored = restore_as(&plan, &path, &request.by);
    let started = restart_units(&state, Some("engine"));
    match (restored, started) {
        (Ok(said), Ok(_)) => {
            print!("{said}");
            println!("{archive} is put back, and the engine is running again");
            Ok(())
        }
        (Ok(said), Err(e)) => {
            print!("{said}");
            Err(fail(format!(
                "{archive} is put back, and the engine did not start again: {}",
                e.message
            )))
        }
        (Err(why), started) => Err(fail(format!(
            "nothing was restored: {why}; {}",
            match started {
                Ok(_) => "the engine is running again on the registry as it was".to_string(),
                Err(e) => format!("and the engine did not start again: {}", e.message),
            }
        ))),
    }
}

/// `nils restore` run as the engine's account where the services are the
/// machine's own, and as this one otherwise: what it said, or why it failed.
fn restore_as(plan: &Plan, archive: &Path, by: &str) -> Result<String, String> {
    let words = owned_words(&[
        "restore",
        &archive.display().to_string(),
        "--yes",
        "--by",
        by,
    ]);
    let acting = plan
        .system
        .as_ref()
        .and_then(|system| AsAccount::of(Some(system), am_root(), plan.registry()));
    if let Some(acting) = acting {
        let ran = acting.run(&words, None);
        return if ran.ok { Ok(ran.stdout) } else { Err(ran.why) };
    }
    let out = Command::new(running_binary())
        .arg("--registry")
        .arg(plan.registry())
        .args(&words)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(said
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("nils: "))
        .or_else(|| said.lines().last())
        .unwrap_or("it failed")
        .trim()
        .to_string())
}

/// The change's row in the registry's audit log, under the person who asked
/// for it, written as the engine's account where the services are the
/// machine's own.
fn audit(state: &State, request: &Request) -> Result<(), String> {
    audit_act(state, &request.by, request.scope())
}

/// An `install.change` row under `by` with `scope`, written as [`audit`]
/// writes a change's (the uninstall's too, wave 7b §18).
pub(crate) fn audit_act(state: &State, by: &str, scope: Value) -> Result<(), String> {
    let plan = plan_from_state(state, None);
    let acting = plan
        .system
        .as_ref()
        .and_then(|system| AsAccount::of(Some(system), am_root(), plan.registry()));
    match acting {
        Some(acting) => {
            let ran = acting.run(
                &owned_words(&[REGISTRY_STEP, "audit", "--by", by]),
                Some(scope.to_string().as_bytes()),
            );
            if ran.ok { Ok(()) } else { Err(ran.why) }
        }
        None => audit_here(&home_on_machine(&Home::new(plan.registry())), by, scope),
    }
}

/// One `install.change` row written into the registry here.
pub(crate) fn audit_here(home: &Home, by: &str, scope: Value) -> Result<(), String> {
    let by = nils_registry::principal::Principal::parse(by)
        .ok_or_else(|| format!("{by} is not user@node"))?
        .to_string();
    let mut registry = crate::open(home).map_err(|e| e.message)?;
    nils_registry::audit::record(
        &mut registry,
        &nils_registry::audit::Entry {
            principal: &by,
            action: nils_registry::audit::Action::InstallChange,
            scope,
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(service: &str, parts: &[&str]) -> State {
        let mut state = State {
            dir: "/opt/nils".to_string(),
            mode: "off".to_string(),
            runtime: "machine".to_string(),
            service: service.to_string(),
            ..State::default()
        };
        for part in parts {
            state.parts.insert(
                (*part).to_string(),
                PartState {
                    version: "1.0.0".to_string(),
                    path: format!("/opt/nils/bin/{part}"),
                    kind: "binary".to_string(),
                },
            );
        }
        state
    }

    fn asked(change: Change) -> Request {
        Request {
            by: "anna@desk".to_string(),
            change,
        }
    }

    #[test]
    fn a_request_reads_back_as_it_was_written_and_says_which_run_makes_it() {
        let request = asked(Change::Signin {
            mode: "oidc".to_string(),
            first: None,
            oidc: Some(Registered {
                issuer: "https://id.example.org/application/o/nils/".to_string(),
                client_id: "desk".to_string(),
                client_secret: "s3cret".to_string(),
                roles_claim: None,
            }),
        });
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains(r#""what":"signin""#), "{text}");
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        assert_eq!(request.word(), CHANGE);
        let restore = asked(Change::Restore {
            archive: "0a1b2c3d-20261016120000".to_string(),
        });
        assert_eq!(restore.word(), RESTORE);
        // the audit row names what changed and never the secret
        let scope = request.scope().to_string();
        assert!(
            scope.contains("id.example.org") && !scope.contains("s3cret"),
            "{scope}"
        );
        let model = asked(Change::Model {
            server: "https://models.example.org/v1/".to_string(),
            key: "sk-not-in-the-row".to_string(),
            model: None,
        });
        assert!(
            !model.scope().to_string().contains("sk-not"),
            "{}",
            model.scope()
        );
    }

    #[test]
    fn an_install_with_no_services_is_changed_at_the_machine() {
        let none = state("none", &["engine", "desk"]);
        let request = asked(Change::Backups {
            dir: "/data/archives".to_string(),
        });
        let (status, why) = refusal(&none, &request).unwrap();
        assert_eq!(status, 409);
        assert!(why.contains("runs no services"), "{why}");
        let kept = state("systemd", &["engine", "desk"]);
        assert_eq!(refusal(&kept, &request), None);
    }

    #[test]
    fn each_change_is_held_to_what_it_may_name() {
        let kept = state("systemd", &["engine", "desk"]);
        let refused = |change: Change| refusal(&kept, &asked(change));
        // nobody signing in is set at the machine
        let off = refused(Change::Signin {
            mode: "off".to_string(),
            first: None,
            oidc: None,
        })
        .unwrap();
        assert_eq!(off.0, 400);
        assert!(off.1.contains("at the machine"), "{}", off.1);
        // a provider on plain http only on this machine's loopback
        let provider = |issuer: &str| Change::Signin {
            mode: "oidc".to_string(),
            first: None,
            oidc: Some(Registered {
                issuer: issuer.to_string(),
                client_id: "desk".to_string(),
                client_secret: "s".to_string(),
                roles_claim: None,
            }),
        };
        assert_eq!(refused(provider("http://id.example.org/")).unwrap().0, 400);
        assert_eq!(refused(provider("https://id.example.org/")), None);
        assert_eq!(refused(provider("http://127.0.0.1:9000/")), None);
        // a first person held to the desk's own rules
        let first = |name: &str, password: &str| Change::Signin {
            mode: "local".to_string(),
            first: Some(First {
                name: name.to_string(),
                password: password.to_string(),
            }),
            oidc: None,
        };
        assert_eq!(refused(first("anna", "short")).unwrap().0, 400);
        assert_eq!(refused(first("an na", "long enough")).unwrap().0, 400);
        assert_eq!(refused(first("anna", "long enough")), None);
        // a backup folder is a whole path, never inside the registry
        let backups = |dir: &str| Change::Backups {
            dir: dir.to_string(),
        };
        assert_eq!(refused(backups("archives")).unwrap().0, 400);
        assert_eq!(refused(backups("/data/../etc")).unwrap().0, 400);
        assert_eq!(refused(backups("/opt/nils/registry/b")).unwrap().0, 400);
        assert_eq!(refused(backups("/data/archives")), None);
        // a model server only where there is an assistant
        let model = Change::Model {
            server: "https://models.example.org/v1".to_string(),
            key: "k".to_string(),
            model: None,
        };
        assert_eq!(refused(model.clone()).unwrap().0, 409);
        let with = state("systemd", &["engine", "desk", "kvasir", "assistant"]);
        assert_eq!(refusal(&with, &asked(model)), None);
        // an archive is one folder's name in the backup folder, never a path
        for name in ["", "../registry", "a/b", ".hidden"] {
            let (status, _) = refused(Change::Restore {
                archive: name.to_string(),
            })
            .unwrap();
            assert_eq!(status, 400, "{name}");
        }
        assert_eq!(
            refused(Change::Restore {
                archive: "0a1b2c3d-20261016120000".to_string()
            }),
            None
        );
        // and who asked is user@node
        let mut nobody = asked(backups("/data/archives"));
        nobody.by = "@".to_string();
        assert_eq!(refusal(&kept, &nobody).unwrap().0, 400);
    }

    #[test]
    fn a_request_is_taken_once_and_a_link_in_its_place_is_never_followed() {
        let root = std::env::temp_dir().join(format!("nils-change-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let request = asked(Change::Backups {
            dir: "/data/archives".to_string(),
        });
        leave_request(&root, &request).unwrap();
        let path = requests_dir(&root).join("change.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
        assert_eq!(take_request(&root, CHANGE).ok().as_ref(), Some(&request));
        assert!(!path.exists(), "taken away once read");
        assert!(take_request(&root, CHANGE).is_err(), "and made once");
        #[cfg(unix)]
        {
            let elsewhere = root.join("elsewhere.json");
            std::fs::write(&elsewhere, serde_json::to_vec(&request).unwrap()).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
            assert!(take_request(&root, CHANGE).is_err());
            assert!(
                elsewhere.exists(),
                "a link is neither read nor its target removed"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_says_which_words_it_knows() {
        let old = "case \"${1:-}\" in\n  reapply)\n    exec /usr/local/bin/nils supervise reapply --part all\n";
        assert!(!helper_knows(old, CHANGE));
        let new = format!("{old}  change)\n    exec /usr/local/bin/nils supervise change\n");
        assert!(helper_knows(&new, CHANGE));
        assert!(!helper_knows(&new, RESTORE));
    }
}
