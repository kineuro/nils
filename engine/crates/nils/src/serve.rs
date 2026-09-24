// SPDX-License-Identifier: AGPL-3.0-only

//! `nils serve` (Wave 4a §11.1): the one door. One route per operation,
//! resource-shaped, versioned under `/api`; a POST that does anything heavy
//! answers 202 with a job id, and progress and results are read from the
//! job; server-sent events carry progress for display and are never the
//! execution context. What the door exposes in this wave is exactly what
//! the command line has: selection, release, handover, review, jobs,
//! custody, status, audit, and `GET /api/capabilities` with the contract
//! versions, the loaded pack versions and the registry epoch (C26).
//!
//! The store is blocking and stays so (§13.6): the server runs it behind a
//! pool, one registry connection per handler thread. A request is handled
//! by whichever thread receives it, with that thread's registry.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};
use std::time::{Duration, Instant};

use nils_registry::home::Home;
use nils_registry::{Registry, Store};
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::grants::{Access, Detail, Need, Step};
use crate::{Exit, ServeArgs, fail, usage};

/// The contract versions this binary speaks, read from the checked-in
/// contracts at build time so that the door and the document cannot drift.
const OPENAPI_VERSION: &str = include_str!("../../../../contracts/openapi/VERSION");
const REVIEW_ITEM_VERSION: &str = include_str!("../../../../contracts/review-item/VERSION");
// Wave 4c §6.7: the suite vocabulary and the MCP door, each versioned.
const SUITE_VERSION: &str = include_str!("../../../../contracts/suite/VERSION");
const MCP_VERSION: &str = include_str!("../../../../contracts/mcp/VERSION");
const PACK_CONTRACT_VERSION: &str = include_str!("../../../../contracts/pack/VERSION");
const MODEL_CONTRACT_VERSION: &str = include_str!("../../../../contracts/model/VERSION");

/// The claims an OIDC token carries that the engine reads; `grants` and
/// `detail` (the suite contract, version 2) are read from the rest.
#[derive(Debug, Clone, serde::Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// RFC 8693: who is acting for the subject, when a token was exchanged.
    #[serde(default)]
    act: Option<serde_json::Value>,
    #[serde(flatten)]
    rest: HashMap<String, serde_json::Value>,
}

/// The `oidc` mode (D8): the engine validates the token against the
/// issuer's keys and its audience, takes the grants and detail it carries,
/// adds what its groups are bound to, and keeps no user table beyond a
/// cache of claims for the token's lifetime.
struct Oidc {
    /// Wave 4c §5.3: the issuers the engine trusts, each with its own
    /// audience and keys; a token is verified against the one it names.
    trusts: Vec<Trust>,
    groups_claim: String,
    /// group -> what `--role` binds it to: a grant, or a ladder name or
    /// `assist` as its set; a group bound twice holds both
    bindings: HashMap<String, Access>,
    /// token -> what was verified, until the token expires
    cache: std::sync::Mutex<ClaimsCache>,
    /// The floor between two fetches of one issuer's keys, in seconds.
    refetch_floor: u64,
}

type Key = (
    Option<String>,
    jsonwebtoken::DecodingKey,
    jsonwebtoken::Algorithm,
);

/// Where an issuer's keys come from: a file the deployment keeps current,
/// or the issuer's own URL, refetched on a key id the engine does not hold.
enum Jwks {
    File(PathBuf),
    Url(String),
}

/// One trusted issuer (Wave 4c §5.3).
struct Trust {
    issuer: String,
    audience: String,
    /// The issuer's host, which is the node half of the principal.
    node: String,
    /// Whether a subject that already holds `@` is the principal as it
    /// stands: the desk's own entry, which qualifies its subjects itself.
    /// Any other entry qualifies every subject by its host, so a provider
    /// whose subjects are mail addresses keeps its people's principals, and
    /// no issuer names a principal under another issuer's host.
    keep_subject: bool,
    jwks: Jwks,
    keys: std::sync::Mutex<Vec<Key>>,
    fetched: std::sync::Mutex<Instant>,
}

/// How long the engine waits between asking an issuer for keys it holds
/// none of. Short, so the first token after the issuer comes up is
/// verified; not nothing, so an issuer that stays down does not cost every
/// request a fetch of its own. Longer than a fetch can take, so two are
/// never in flight at once.
const EMPTY_FLOOR: u64 = 5;

impl Trust {
    /// Fetch the keys again, when they come from a URL and the floor has
    /// passed; true when the key list was replaced.
    ///
    /// The floor drops to `EMPTY_FLOOR` while the engine holds no key at
    /// all. That is the state it starts in when its issuer was not up yet,
    /// and waiting out a minute there would refuse every token meanwhile.
    /// It drops rather than goes, because an issuer that stays down would
    /// otherwise cost every request a fetch of its own.
    fn refetch(&self, floor: u64) -> bool {
        let Jwks::Url(_) = &self.jwks else {
            return false;
        };
        let floor = if self.keys.lock().map(|k| k.is_empty()).unwrap_or(false) {
            floor.min(EMPTY_FLOOR)
        } else {
            floor
        };
        let mut fetched = match self.fetched.lock() {
            Ok(f) => f,
            Err(_) => return false,
        };
        if fetched.elapsed().as_secs() < floor {
            return false;
        }
        // Stamped before the fetch and unlocked before it too, so that a
        // request arriving while an issuer is being asked reads the stamp
        // and is refused at once, rather than queueing behind the network.
        *fetched = Instant::now();
        drop(fetched);
        match load_jwks(&self.jwks) {
            Ok(keys) => {
                if let Ok(mut held) = self.keys.lock() {
                    *held = keys;
                }
                true
            }
            Err(e) => {
                eprintln!(
                    "nils serve: the keys of {} could not be refetched: {e}",
                    self.issuer
                );
                false
            }
        }
    }
}

/// The keys of a JWKS document, from a file or a URL; a shared secret is
/// skipped, because the engine holds no secrets.
fn load_jwks(jwks: &Jwks) -> Result<Vec<Key>, String> {
    let (text, name) = match jwks {
        Jwks::File(path) => (
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
            path.display().to_string(),
        ),
        Jwks::Url(url) => {
            // Bounded on purpose. This runs on a request handler when the
            // engine holds no key of an issuer, and an issuer that is down
            // must cost that request a moment, not the connect timeout of
            // whatever the default happens to be.
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(std::time::Duration::from_secs(2)))
                .timeout_global(Some(std::time::Duration::from_secs(4)))
                .build()
                .into();
            let mut response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
            (
                response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| format!("{url}: {e}"))?,
                url.clone(),
            )
        }
    };
    let set: jsonwebtoken::jwk::JwkSet =
        serde_json::from_str(&text).map_err(|e| format!("{name}: not a JWKS document: {e}"))?;
    let mut keys = Vec::new();
    for jwk in &set.keys {
        let Ok(key) = jsonwebtoken::DecodingKey::from_jwk(jwk) else {
            continue;
        };
        let algorithm = match &jwk.algorithm {
            jsonwebtoken::jwk::AlgorithmParameters::RSA(_) => jsonwebtoken::Algorithm::RS256,
            jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_) => {
                jsonwebtoken::Algorithm::ES256
            }
            jsonwebtoken::jwk::AlgorithmParameters::OctetKeyPair(_) => {
                jsonwebtoken::Algorithm::EdDSA
            }
            // A shared secret is not OIDC: the engine holds no secrets.
            _ => continue,
        };
        keys.push((jwk.common.key_id.clone(), key, algorithm));
    }
    if keys.is_empty() {
        return Err(format!("{name}: no RSA, EC or EdDSA key to verify with"));
    }
    Ok(keys)
}

/// What the engine keeps of a token it verified, until the token expires:
/// the principal, the grants and detail, and (Wave 4c §5.9) the display
/// name and mail beside the subject, which are listed in custody and are
/// never a key.
#[derive(Clone)]
struct Known {
    principal: String,
    access: Access,
    exp: u64,
    display: Option<String>,
    email: Option<String>,
    /// The `act` claim's subject, when the token was exchanged.
    act: Option<String>,
    /// The registered model the actor runs, when the issuer bound one into
    /// the `act` claim (`act.model`: an id, a digest or `name@version`).
    act_model: Option<String>,
}
type ClaimsCache = HashMap<String, Known>;

/// Who a request is from.
enum Auth {
    /// The local user, as the command line would record it.
    Off,
    /// A bearer token names the caller and what it holds.
    Token(HashMap<String, (String, Access)>),
    /// An OIDC token names the caller, and its grants, detail and groups say
    /// what they may do.
    Oidc(Box<Oidc>),
}

/// A caller: who, what they hold, and how much of a record they see.
pub(crate) struct Caller {
    pub(crate) principal: String,
    pub(crate) access: Access,
    /// Wave 4c §5.9: the display name and mail the token carried, kept
    /// beside the subject and never a key.
    pub(crate) display: Option<String>,
    pub(crate) email: Option<String>,
    /// Wave 4c §5.5: who acts for the principal on this call; absent is
    /// its own value.
    pub(crate) actor: serde_json::Value,
    /// Wave 4c §5.5: the downgrade only ceiling the call named, if any.
    pub(crate) ceiling: Option<Step>,
    /// Wave 4c §6.3: the `Idempotency-Key` the call carried, if any.
    pub(crate) idempotency_key: Option<String>,
}

impl Caller {
    /// Whether the caller passes a door that needs `need` at `detail`, or
    /// the refusal, which names what was needed. A caller that holds no
    /// grant is refused at every door, whatever the door needs (Wave 4b
    /// §12.4: never defaulted to a reader).
    pub(crate) fn allowed(&self, what: &str, need: Need, detail: Detail) -> Result<(), Reply> {
        let principal = &self.principal;
        if self.access.is_empty() {
            return Err(Reply::error(
                403,
                format!(
                    "{what}: {principal} holds no grant; an installer binds grants before a caller reads"
                ),
            ));
        }
        if !need.met(&self.access) {
            return Err(Reply::error(
                403,
                format!(
                    "{what} needs {}; {principal} holds {}",
                    need.words(),
                    self.access.list().join(", ")
                ),
            ));
        }
        if self.access.detail < detail {
            return Err(Reply::error(
                403,
                format!(
                    "{what} needs detail {}; {principal} sees {}",
                    detail.name(),
                    self.access.detail.name()
                ),
            ));
        }
        Ok(())
    }
}

impl Auth {
    fn parse(args: &ServeArgs) -> Result<Auth, Exit> {
        match args.auth.as_str() {
            "off" => Ok(Auth::Off),
            "token" => {
                let mut tokens = HashMap::new();
                let mut given: Vec<String> = args.token.clone();
                if let Ok(env) = std::env::var("NILS_TOKENS") {
                    given.extend(token_entries(&env));
                }
                for t in given {
                    let Some((token, rest)) = t.split_once('=') else {
                        return Err(usage(format!("{t} is not TOKEN=user@node[:grants]")));
                    };
                    // `user@node:reader,kvasir:see`: ladder names, which stand
                    // for their sets, and grants, added up; no suffix is
                    // everything, and an empty suffix is nothing, so its
                    // caller is refused at every door (Wave 4b §12.4)
                    let (who, access) = match rest.split_once(':') {
                        Some((who, list)) => (
                            who,
                            Access::of_list(list).map_err(|item| usage(not_a_grant(&item)))?,
                        ),
                        None => {
                            // Wave 4c §6.1: the shortest form is the widest one.
                            eprintln!(
                                "nils serve: the token for {rest} names no grants and therefore holds every grant and detail sensitive; a machine token that should hold less is written {rest}:pipelines:work,release:work"
                            );
                            (rest, Access::everything())
                        }
                    };
                    let Some(p) = nils_registry::principal::Principal::parse(who) else {
                        return Err(usage(format!("{who} is not a principal, user@node")));
                    };
                    if token.len() < 16 {
                        return Err(usage("a token is at least 16 characters"));
                    }
                    tokens.insert(token.to_string(), (p.to_string(), access));
                }
                if tokens.is_empty() {
                    return Err(usage(
                        "--auth token needs at least one --token TOKEN=user@node, or NILS_TOKENS",
                    ));
                }
                Ok(Auth::Token(tokens))
            }
            "oidc" => {
                // Wave 4c §5.3: a trust list; the three single flags of
                // Wave 4b are sugar for one entry, which keeps no subject.
                let mut specs: Vec<(String, String, Jwks, bool)> = Vec::new();
                for t in &args.oidc_trust {
                    let (mut issuer, mut audience, mut jwks) = (None, None, None);
                    let mut keep_subject = false;
                    for part in t.split(',') {
                        let Some((k, v)) = part.split_once('=') else {
                            return Err(usage(format!(
                                "{t} is not issuer=URL,audience=ID,jwks=URL[,keep_subject=true]"
                            )));
                        };
                        let v = v.trim().to_string();
                        match k.trim() {
                            "issuer" => issuer = Some(v),
                            "audience" => audience = Some(v),
                            "jwks" => {
                                jwks =
                                    Some(if v.starts_with("https://") || v.starts_with("http://") {
                                        Jwks::Url(v)
                                    } else {
                                        Jwks::File(PathBuf::from(v))
                                    })
                            }
                            "keep_subject" => {
                                keep_subject = match v.as_str() {
                                    "true" => true,
                                    "false" => false,
                                    other => {
                                        return Err(usage(format!(
                                            "keep_subject is true or false, not {other}"
                                        )));
                                    }
                                }
                            }
                            other => {
                                return Err(usage(format!(
                                    "{other} is not a part of --oidc-trust: issuer, audience, jwks, keep_subject"
                                )));
                            }
                        }
                    }
                    match (issuer, audience, jwks) {
                        (Some(i), Some(a), Some(j)) => specs.push((i, a, j, keep_subject)),
                        _ => {
                            return Err(usage(format!(
                                "{t}: --oidc-trust names issuer, audience and jwks together"
                            )));
                        }
                    }
                }
                match (&args.oidc_issuer, &args.oidc_audience, &args.oidc_jwks) {
                    (Some(i), Some(a), Some(j)) => {
                        specs.push((i.clone(), a.clone(), Jwks::File(j.clone()), false));
                    }
                    (None, None, None) => {}
                    _ => {
                        return Err(usage(
                            "--oidc-issuer, --oidc-audience and --oidc-jwks go together; or name an issuer as --oidc-trust issuer=URL,audience=ID,jwks=URL",
                        ));
                    }
                }
                if specs.is_empty() {
                    return Err(usage(
                        "--auth oidc needs an issuer: --oidc-trust issuer=URL,audience=ID,jwks=URL (repeatable), or --oidc-issuer, --oidc-audience and --oidc-jwks together",
                    ));
                }
                let mut trusts = Vec::new();
                for (issuer, audience, jwks, keep_subject) in specs {
                    // A file that cannot be read is a fault in the
                    // configuration and stops the engine here. A URL that
                    // does not answer is a matter of order: the issuer may
                    // simply not be up yet, and in a container run it often
                    // is not, so the engine starts holding no key of that
                    // issuer and fetches when the first token arrives.
                    let keys = match load_jwks(&jwks) {
                        Ok(keys) => keys,
                        Err(e) => match &jwks {
                            Jwks::File(_) => return Err(usage(e)),
                            Jwks::Url(_) => {
                                eprintln!(
                                    "nils serve: {issuer} did not answer for its keys yet ({e}); the engine starts without them and asks again when a token arrives"
                                );
                                Vec::new()
                            }
                        },
                    };
                    let node = issuer
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .split('/')
                        .next()
                        .unwrap_or("issuer")
                        .to_string();
                    trusts.push(Trust {
                        issuer,
                        audience,
                        node,
                        keep_subject,
                        jwks,
                        keys: std::sync::Mutex::new(keys),
                        fetched: std::sync::Mutex::new(Instant::now()),
                    });
                }
                let mut bindings: HashMap<String, Access> = HashMap::new();
                for r in &args.role {
                    let Some((group, bound)) = r.split_once('=') else {
                        return Err(usage(format!("{r} is not GROUP=GRANT")));
                    };
                    let Some(access) = Access::named(bound.trim()) else {
                        return Err(usage(not_a_grant(bound.trim())));
                    };
                    bindings
                        .entry(group.trim().to_string())
                        .or_default()
                        .add(&access);
                }
                Ok(Auth::Oidc(Box::new(Oidc {
                    trusts,
                    groups_claim: args.oidc_groups_claim.clone(),
                    bindings,
                    cache: std::sync::Mutex::new(HashMap::new()),
                    refetch_floor: args.jwks_refetch_secs.unwrap_or(60),
                })))
            }
            other => Err(usage(format!("--auth is off, token or oidc, not {other}"))),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Auth::Off => "off",
            Auth::Token(_) => "token",
            Auth::Oidc(_) => "oidc",
        }
    }

    /// The caller of a request, or why not.
    fn caller(&self, request: &Request) -> Result<Caller, Reply> {
        let bearer = || -> Result<String, Reply> {
            let header = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_string());
            let Some(value) = header else {
                return Err(Reply::error(401, "a bearer token is required"));
            };
            Ok(value
                .strip_prefix("Bearer ")
                .unwrap_or("")
                .trim()
                .to_string())
        };
        let caller = match self {
            Auth::Off => Caller {
                principal: crate::actor(),
                access: Access::everything(),
                display: None,
                email: None,
                actor: nils_registry::actor::absent(),
                ceiling: None,
                idempotency_key: None,
            },
            Auth::Token(tokens) => {
                let token = bearer()?;
                match tokens.get(&token) {
                    Some((p, access)) => Caller {
                        principal: p.clone(),
                        access: access.clone(),
                        display: None,
                        email: None,
                        actor: nils_registry::actor::absent(),
                        ceiling: None,
                        idempotency_key: None,
                    },
                    None => return Err(Reply::error(401, "the token names nobody")),
                }
            }
            Auth::Oidc(oidc) => {
                let token = bearer()?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let cached = oidc
                    .cache
                    .lock()
                    .ok()
                    .and_then(|c| c.get(&token).filter(|k| k.exp > now).cloned());
                let known = match cached {
                    Some(k) => k,
                    None => oidc.verify(&token, now)?,
                };
                let actor = match &known.act {
                    Some(sub) => {
                        let mut a = serde_json::json!({ "kind": "agent", "name": sub });
                        if let Some(m) = &known.act_model {
                            a["model"] = serde_json::Value::String(m.clone());
                        }
                        a
                    }
                    None => nils_registry::actor::absent(),
                };
                Caller {
                    principal: known.principal,
                    access: known.access,
                    display: known.display,
                    email: known.email,
                    actor,
                    ceiling: None,
                    idempotency_key: None,
                }
            }
        };
        narrow(caller, request)
    }
}

/// Wave 4c §5.5: the two headers a call may carry. `X-Nils-Ceiling` names a
/// ladder step and can only narrow: the caller keeps the grants of the
/// step's set and the assistant, and detail is lowered to the step's.
/// `X-Nils-Actor` names who acts for the principal and is recorded with the
/// ceiling inside it.
fn narrow(mut caller: Caller, request: &Request) -> Result<Caller, Reply> {
    let header = |name: &'static str| -> Option<String> {
        request
            .headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().trim().to_string())
            .filter(|v| !v.is_empty())
    };
    if let Some(actor) = header("X-Nils-Actor") {
        let value: serde_json::Value = serde_json::from_str(&actor)
            .map_err(|e| Reply::error(400, format!("X-Nils-Actor is not a JSON object: {e}")))?;
        if !value.is_object() || !value["kind"].is_string() {
            return Err(Reply::error(
                400,
                "X-Nils-Actor is a JSON object with a kind: person, agent or model",
            ));
        }
        // Record 42 S1: what the token proves the header cannot raise. A
        // token that acts for an agent (its `act` claim) may name the agent
        // or a model it runs, and never a person, nor leave the actor absent,
        // which is read as a person at a keyboard. The header cannot rename
        // the actor the claim proves, and a model is the one the issuer bound
        // into the claim (`act.model`) or none: an agent's token names no
        // model of its own choosing.
        let mut value = value;
        if let Some(proven @ ("agent" | "model")) = caller.actor["kind"].as_str() {
            let asked = value["kind"].as_str().unwrap_or("").to_string();
            let asked = asked.as_str();
            let rank = nils_registry::review::rank;
            if !matches!(asked, "agent" | "model") || rank(asked) > rank(proven) {
                return Err(Reply::error(
                    403,
                    format!(
                        "the token acts for {}; X-Nils-Actor cannot make it {}",
                        with_article(proven),
                        with_article(asked)
                    ),
                ));
            }
            let proven_name = caller.actor.get("name").and_then(|n| n.as_str());
            match (value.get("name").filter(|n| !n.is_null()), proven_name) {
                (Some(said), Some(name)) if said.as_str() != Some(name) => {
                    return Err(Reply::error(
                        403,
                        format!("the token acts as {name}; X-Nils-Actor cannot name it {said}"),
                    ));
                }
                (None, Some(name)) => value["name"] = serde_json::Value::from(name),
                _ => {}
            }
            let carried = caller.actor.get("model").filter(|m| !m.is_null()).cloned();
            let said = value.get("model").filter(|m| !m.is_null()).cloned();
            match (said, carried) {
                (Some(said), Some(carried)) if said != carried => {
                    return Err(Reply::error(
                        403,
                        format!(
                            "the token carries the model {carried}; X-Nils-Actor cannot name {said}"
                        ),
                    ));
                }
                (Some(said), None) => {
                    return Err(Reply::error(
                        403,
                        format!(
                            "the token carries no model, so X-Nils-Actor cannot name {said}; the issuer binds the model an agent runs into the act claim (act.model)"
                        ),
                    ));
                }
                (None, Some(carried)) => value["model"] = carried,
                _ => {}
            }
            if asked == "model" && value.get("model").is_none_or(|m| m.is_null()) {
                return Err(Reply::error(
                    403,
                    "the token carries no model, so it cannot act as one; the issuer binds the model an agent runs into the act claim (act.model)",
                ));
            }
        }
        caller.actor = value;
    }
    if let Some(ceiling) = header("X-Nils-Ceiling") {
        let Some(step) = Step::parse(&ceiling) else {
            return Err(Reply::error(
                400,
                format!(
                    "X-Nils-Ceiling {ceiling} is not a ladder name: reader, reviewer, operator or admin"
                ),
            ));
        };
        caller.access.narrow(step);
        caller.ceiling = Some(step);
        caller.actor["ceiling"] = serde_json::Value::String(step.name().to_string());
    }
    if let Some(key) = header("Idempotency-Key") {
        if key.len() > 256 {
            return Err(Reply::error(
                400,
                "Idempotency-Key is at most 256 characters",
            ));
        }
        caller.idempotency_key = Some(key);
    }
    Ok(caller)
}

/// `NILS_TOKENS`: the entries `--token` takes, separated by commas. A piece
/// that holds no `=` continues the entry before it, since an entry always
/// holds one and a ladder name or a grant never does, so an entry's own list
/// survives, as in `T1=bo@lab:reader,kvasir:see`. A piece before any entry
/// stays on its own, to be refused as it was.
fn token_entries(env: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for piece in env.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match entries.last_mut() {
            Some(last) if !piece.contains('=') && last.contains('=') => {
                last.push(',');
                last.push_str(piece);
            }
            _ => entries.push(piece.to_string()),
        }
    }
    entries
}

/// The refusal of a name that is neither a ladder name nor a grant.
fn not_a_grant(name: &str) -> String {
    format!(
        "{name} is neither a ladder name nor a grant: reader, reviewer, operator, admin, assist, or a grant such as query:see or kvasir:work"
    )
}

impl Oidc {
    /// Verify a token against the issuer it names, refetching that
    /// issuer's keys once on a key id the engine does not hold.
    fn verify(&self, token: &str, now: u64) -> Result<Known, Reply> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| Reply::error(401, format!("not a token: {e}")))?;
        let mut last = String::from("no key of any trusted issuer verifies it");
        let mut found: Option<(Claims, &Trust)> = None;
        'trusts: for trust in &self.trusts {
            for attempt in 0..2 {
                let keys: Vec<Key> = match trust.keys.lock() {
                    Ok(k) => k.clone(),
                    Err(_) => break,
                };
                // An empty list holds nothing, not even a token that names
                // no key: that is the state the engine starts in when its
                // issuer was not up yet, and it is what the refetch is for.
                let holds_kid = !keys.is_empty()
                    && header
                        .kid
                        .as_ref()
                        .is_none_or(|h| keys.iter().any(|(k, _, _)| k.as_ref() == Some(h)));
                if holds_kid {
                    for (kid, key, algorithm) in &keys {
                        if let (Some(k), Some(h)) = (kid, &header.kid)
                            && k != h
                        {
                            continue;
                        }
                        let mut validation = jsonwebtoken::Validation::new(*algorithm);
                        validation.set_issuer(&[trust.issuer.as_str()]);
                        validation.set_audience(&[trust.audience.as_str()]);
                        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
                        match jsonwebtoken::decode::<Claims>(token, key, &validation) {
                            Ok(data) => {
                                found = Some((data.claims, trust));
                                break 'trusts;
                            }
                            Err(e) => last = e.to_string(),
                        }
                    }
                    break;
                }
                // A key id the engine does not hold: the issuer may have
                // rotated, so fetch once, then read the keys again.
                if attempt == 0 && trust.refetch(self.refetch_floor) {
                    continue;
                }
                last = format!(
                    "no key named {} of any trusted issuer",
                    header.kid.as_deref().unwrap_or("")
                );
                break;
            }
        }
        let Some((claims, trust)) = found else {
            return Err(Reply::error(401, format!("the token is refused: {last}")));
        };
        let oidc = self;
        {
            // The groups: the standard claim, or the one the deployment names.
            let groups: Vec<String> = if oidc.groups_claim == "groups" {
                claims.groups.clone()
            } else {
                claims
                    .rest
                    .get(&oidc.groups_claim)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|g| g.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            // The suite contract, version 2: the grants and the detail the
            // token carries, taken as they are, and what its groups are
            // bound to, added up with the highest detail holding. Wave 4b
            // §12.4: a token left with no grant is refused at every door,
            // never defaulted to a reader.
            let access = Access::of_token(
                claims.rest.get("grants"),
                claims.rest.get("detail"),
                &groups,
                &oidc.bindings,
            );
            // The audit principal is the subject (§11.2), at the issuer's
            // node; a subject that already names its node is the principal
            // as it stands, but only from an entry that keeps subjects, as
            // the desk's own entry does.
            let principal = if trust.keep_subject && claims.sub.contains('@') {
                claims.sub.clone()
            } else {
                format!("{}@{}", claims.sub, trust.node)
            };
            let known =
                Known {
                    principal,
                    access,
                    exp: claims.exp,
                    display: claims.preferred_username.clone().or(claims.name.clone()),
                    email: claims.email.clone(),
                    act: claims
                        .act
                        .as_ref()
                        .and_then(|a| a.get("sub"))
                        .and_then(|s| s.as_str())
                        .map(String::from),
                    act_model: claims.act.as_ref().and_then(|a| a.get("model")).and_then(
                        |m| match m {
                            serde_json::Value::String(s) if !s.trim().is_empty() => {
                                Some(s.trim().to_string())
                            }
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        },
                    ),
                };
            if let Ok(mut cache) = oidc.cache.lock() {
                cache.retain(|_, k| k.exp > now);
                cache.insert(token.to_string(), known.clone());
            }
            Ok(known)
        }
    }
}

/// What a route answers.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
    /// Headers beside the content type: the MCP door's `WWW-Authenticate`
    /// carries the metadata a client fetches next (RFC 9728).
    pub(crate) headers: Vec<(String, String)>,
    /// A body that is not JSON, sent as it stands (a notification's empty
    /// answer).
    pub(crate) empty: bool,
    /// Wave 5 §12.7: bytes with their own content type, the instance door's
    /// tiles and renders; `body` is ignored when set.
    pub(crate) raw: Option<Box<(String, Vec<u8>)>>,
    /// Record 42 S4: a file sent as it stands, streamed from its place
    /// rather than read whole; its content type is among `headers`.
    pub(crate) file: Option<Box<std::path::PathBuf>>,
}

impl Reply {
    /// Bytes with a content type, and any headers beside it.
    pub(crate) fn raw(content_type: &str, bytes: Vec<u8>, headers: Vec<(String, String)>) -> Reply {
        Reply {
            status: 200,
            body: serde_json::Value::Null,
            headers,
            empty: false,
            raw: Some(Box::new((content_type.to_string(), bytes))),
            file: None,
        }
    }
    /// A file with a content type, streamed from where it lies.
    pub(crate) fn file(
        content_type: &str,
        path: std::path::PathBuf,
        mut headers: Vec<(String, String)>,
    ) -> Reply {
        headers.insert(0, ("Content-Type".to_string(), content_type.to_string()));
        Reply {
            status: 200,
            body: serde_json::Value::Null,
            headers,
            empty: false,
            raw: None,
            file: Some(Box::new(path)),
        }
    }
    pub(crate) fn ok(body: serde_json::Value) -> Reply {
        Reply {
            status: 200,
            body,
            headers: Vec::new(),
            empty: false,
            raw: None,
            file: None,
        }
    }
    pub(crate) fn accepted(body: serde_json::Value) -> Reply {
        Reply {
            status: 202,
            body,
            headers: Vec::new(),
            empty: false,
            raw: None,
            file: None,
        }
    }
    pub(crate) fn created(body: serde_json::Value) -> Reply {
        Reply {
            status: 201,
            body,
            headers: Vec::new(),
            empty: false,
            raw: None,
            file: None,
        }
    }
    /// An error, with its disclosure (Wave 5 section 12.6): `internal` for
    /// a 5xx, whose text names the engine's own failure; `safe` otherwise,
    /// for a message built from document, set, field and door names and
    /// counts. A message that could carry a value of a person is built
    /// with `gated` instead.
    pub(crate) fn error(status: u16, message: impl Into<String>) -> Reply {
        let disclosure = if status >= 500 { "internal" } else { "safe" };
        Reply {
            status,
            body: serde_json::json!({ "error": message.into(), "disclosure": disclosure }),
            headers: Vec::new(),
            empty: false,
            raw: None,
            file: None,
        }
    }
    /// An error whose text may carry a value of a person: a subject code,
    /// a display code, a header value, a name, a path under a source root.
    /// A desk renders it through the projection door with its audit row.
    pub(crate) fn gated(status: u16, message: impl Into<String>) -> Reply {
        Reply {
            status,
            body: serde_json::json!({ "error": message.into(), "disclosure": "gated" }),
            headers: Vec::new(),
            empty: false,
            raw: None,
            file: None,
        }
    }
    /// The same reply with one more header.
    pub(crate) fn with(mut self, name: &str, value: impl Into<String>) -> Reply {
        self.headers.push((name.to_string(), value.into()));
        self
    }
    /// A reply with no body at all: what a notification is answered with.
    pub(crate) fn nothing(status: u16) -> Reply {
        Reply {
            status,
            body: serde_json::Value::Null,
            headers: Vec::new(),
            empty: true,
            raw: None,
            file: None,
        }
    }
}

impl From<Exit> for Reply {
    fn from(e: Exit) -> Reply {
        let status = if e.code == crate::USAGE { 400 } else { 500 };
        Reply::error(status, e.message)
    }
}

impl From<nils_registry::Error> for Reply {
    fn from(e: nils_registry::Error) -> Reply {
        Reply::error(500, e.to_string())
    }
}

impl From<nils_ask::handle::HandleError> for Reply {
    fn from(e: nils_ask::handle::HandleError) -> Reply {
        Reply::error(500, e.to_string())
    }
}

/// What every handler shares.
pub(crate) struct Doors {
    pub(crate) home: Home,
    auth: Auth,
    pub(crate) pack_dir: Option<std::path::PathBuf>,
    /// The bound address, for the capabilities.
    pub(crate) node: String,
    started: Instant,
    served: AtomicUsize,
    /// Wave 4b §12.4: the ask doors' reader DSN, the caps, the pack.
    pub(crate) ask_dsn: Option<String>,
    pub(crate) ask_caps: nils_catalog::Caps,
    pub(crate) ask_pack: String,
    /// Wave 4b §12.3: the authorization servers the MCP door's metadata
    /// names (RFC 9728), and the address it is reached at, which is the
    /// resource that metadata identifies.
    pub(crate) mcp_authorization_servers: Vec<String>,
    pub(crate) bound: String,
    /// Wave 4c §6.5: the assistant installed beside this engine, if one is.
    pub(crate) assist: Option<String>,
    /// Wave 5 §10.4: the supervisor on this host, if one is.
    pub(crate) supervisor: Option<String>,
    /// Wave 4c §6.1: the cap on open event streams, and how many are open;
    /// every stream pins a worker for its life.
    pub(crate) event_streams: usize,
    streams_open: AtomicUsize,
    /// Wave 4c §6.5: the ingest locations the deployment registered, by
    /// name; a job that walks a tree names one of these, never a path.
    pub(crate) ingest_roots: std::collections::BTreeMap<String, PathBuf>,
    /// Wave 4c §6.5: where the backup job writes.
    pub(crate) backup_dir: Option<PathBuf>,
}

impl Doors {
    /// Record 48 R2: whether a caller's principal is an identity the engine
    /// verified (a token it holds, or a trusted issuer's subject), and not
    /// the local user name `--auth off` takes.
    pub(crate) fn identity_verified(&self) -> bool {
        !matches!(self.auth, Auth::Off)
    }
}

pub fn serve(home: &Home, args: ServeArgs) -> Result<(), Exit> {
    if !home.exists() {
        return Err(usage(format!("no registry in {}", home.dir().display())));
    }
    // Wave 4c §6.1: where an assistant is installed, the ask doors run as
    // their own SELECT only role, which on Postgres is a DSN and not a
    // session setting.
    if args.assist.is_some() && args.ask_dsn.is_none() {
        let registry = crate::open(home)?;
        if format!("{:?}", registry.config().backend).to_lowercase() == "postgres" {
            return Err(usage(
                "--assist names an assistant, so the ask doors need their own SELECT only role on Postgres: pass --ask-dsn (Wave 4c section 6.1)",
            ));
        }
    }
    let auth = Auth::parse(&args)?;
    // §12: a fresh install has no --pack-dir, so the packs are looked for
    // where an installer leaves them; without any, the engine still serves
    // and the doors that need a pack say so.
    let pack_dir = crate::pack_dir(home, args.pack_dir.clone()).ok();
    let server = tiny_http::Server::http(&args.bind)
        .map_err(|e| fail(format!("cannot listen on {}: {e}", args.bind)))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| args.bind.clone());
    println!(
        "nils serve   {bound}   auth {}   workers {}   registry {}   packs {}",
        auth.name(),
        args.workers.max(1),
        home.dir().display(),
        pack_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    // record 49 A4: the starter catalog, seeded where the setting allows;
    // a failure is said and the engine serves on
    // (never a panic: a caller that read the listening line and closed
    // the pipe is not a reason to stop)
    if let Ok(mut registry) = home.open() {
        let line = crate::starter::at_start(&mut registry);
        let _ = std::io::Write::write_all(&mut std::io::stdout(), format!("{line}\n").as_bytes());
    }
    let ask_caps = match &args.ask_caps {
        Some(text) => {
            let over: serde_json::Value = serde_json::from_str(text)
                .map_err(|e| usage(format!("--ask-caps is not a JSON object: {e}")))?;
            let mut base = serde_json::to_value(nils_catalog::Caps::default()).unwrap_or_default();
            for (k, v) in over.as_object().into_iter().flatten() {
                base[k] = v.clone();
            }
            serde_json::from_value(base).map_err(|e| usage(format!("--ask-caps: {e}")))?
        }
        None => nils_catalog::Caps::default(),
    };
    let doors = Arc::new(Doors {
        home: home.clone(),
        auth,
        pack_dir: pack_dir.clone(),
        node: nils_registry::job::hostname(),
        started: Instant::now(),
        served: AtomicUsize::new(0),
        ask_dsn: args.ask_dsn.clone(),
        ask_caps,
        ask_pack: args.ask_pack.clone(),
        mcp_authorization_servers: args.mcp_authorization_server.clone(),
        bound: bound.clone(),
        assist: args.assist.clone(),
        supervisor: args.supervisor.clone(),
        event_streams: args
            .event_streams
            .unwrap_or_else(|| (args.workers / 2).max(1)),
        streams_open: AtomicUsize::new(0),
        ingest_roots: {
            let mut roots = std::collections::BTreeMap::new();
            for r in &args.ingest_root {
                let Some((name, path)) = r.split_once('=') else {
                    return Err(usage(format!("{r} is not NAME=PATH")));
                };
                let path = PathBuf::from(path.trim());
                if !path.is_absolute() || !path.is_dir() {
                    return Err(usage(format!(
                        "--ingest-root {name}: {} is not an absolute directory",
                        path.display()
                    )));
                }
                roots.insert(name.trim().to_string(), path);
            }
            roots
        },
        backup_dir: args.backup_dir.clone(),
    });
    let server = Arc::new(server);
    let limit = args.requests;
    // A job a door queues runs without anyone starting a worker by hand. A
    // registry's queue has one worker, so where another already holds it,
    // this one waits and looks again.
    let stop_queue = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Record 49 A1: pipeline runs have a lane of their own beside it, so a
    // long run never holds up a digest, a classify or a release.
    let queue: Vec<std::thread::JoinHandle<()>> = if args.worker {
        [
            nils_registry::job::Lane::Main,
            nils_registry::job::Lane::Pipelines,
        ]
        .into_iter()
        .map(|lane| {
            let home = home.clone();
            let roots = args.ingest_root.clone();
            let stop = Arc::clone(&stop_queue);
            // lab 26b, finding 4: a job this engine queues runs with the
            // workers the engine was started with, where the caller
            // named none
            let workers = args.workers.max(1);
            std::thread::spawn(move || queue_worker(&home, &roots, workers, lane, &stop))
        })
        .collect()
    } else {
        Vec::new()
    };
    // Wave 5 §10.3: the backup schedule beside the queue that runs what it
    // queues, where there is a directory to write to.
    let schedule = args.backup_dir.clone().filter(|_| args.worker).map(|dir| {
        let home = home.clone();
        let stop = Arc::clone(&stop_queue);
        std::thread::spawn(move || crate::schedule::run(&home, &dir, &stop))
    });
    let mut handles = Vec::new();
    for _ in 0..args.workers.max(1) {
        let server = Arc::clone(&server);
        let doors = Arc::clone(&doors);
        handles.push(std::thread::spawn(move || {
            // One registry per handler thread: the pool of §13.6, and the
            // ask doors' reader, pack and catalog beside it.
            let mut registry: Option<Registry> = None;
            let mut ask_state = crate::ask_doors::AskState::default();
            loop {
                let Ok(request) = server.recv_timeout(Duration::from_millis(250)) else {
                    return;
                };
                let Some(request) = request else {
                    if limit.is_some_and(|n| doors.served.load(Ordering::SeqCst) >= n) {
                        return;
                    }
                    continue;
                };
                let n = doors.served.fetch_add(1, Ordering::SeqCst) + 1;
                if registry.is_none() {
                    registry = doors.home.open().ok();
                }
                let Some(reg) = registry.as_mut() else {
                    let _ = respond(
                        request,
                        Reply::error(500, "the registry could not be opened"),
                    );
                    continue;
                };
                handle(&doors, reg, &mut ask_state, request);
                if limit.is_some_and(|max| n >= max) {
                    server.unblock();
                    return;
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    stop_queue.store(true, Ordering::SeqCst);
    for q in queue {
        let _ = q.join();
    }
    if let Some(schedule) = schedule {
        let _ = schedule.join();
    }
    Ok(())
}

/// The queue's worker beside the doors: it takes the queue when no other
/// worker holds it and runs what is queued; when another worker has it, or
/// the registry cannot be opened, it looks again a little later.
fn queue_worker(
    home: &Home,
    roots: &[String],
    workers: usize,
    lane: nils_registry::job::Lane,
    stop: &std::sync::atomic::AtomicBool,
) {
    let stopped = || stop.load(Ordering::SeqCst);
    while !stopped() {
        let outcome = match home.open() {
            Ok(mut registry) => {
                let store = registry.store();
                match crate::worker::claim(store, false, lane) {
                    Ok(worker) => crate::worker::run(
                        home,
                        store,
                        worker,
                        &crate::worker::Options {
                            once: false,
                            every: 5,
                            ingest_roots: roots,
                            workers: Some(workers),
                            quiet: true,
                            lane,
                        },
                        &stopped,
                    )
                    .map(|_| ())
                    .map_err(|e| e.message),
                    Err(nils_registry::job::Error::Busy { .. }) => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(e.to_string()),
        };
        if let Err(why) = outcome {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "nils serve: the {} lane's worker: {why}",
                lane.name()
            );
        }
        for _ in 0..30 {
            if stopped() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn respond(request: Request, reply: Reply) -> std::io::Result<()> {
    if let Some(path) = &reply.file {
        let file = match std::fs::File::open(path.as_path()) {
            Ok(f) => f,
            Err(e) => {
                return respond(
                    request,
                    Reply::error(500, format!("the file will not open: {e}")),
                );
            }
        };
        let mut response = Response::from_file(file)
            .with_status_code(StatusCode(reply.status))
            .with_chunked_threshold(usize::MAX);
        for (name, value) in &reply.headers {
            if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response = response.with_header(h);
            }
        }
        return request.respond(response);
    }
    if let Some(raw) = reply.raw {
        let (content_type, bytes) = *raw;
        let mut response = Response::from_data(bytes)
            .with_status_code(StatusCode(reply.status))
            .with_chunked_threshold(usize::MAX)
            .with_header(
                Header::from_bytes("Content-Type", content_type.as_bytes()).expect("header"),
            );
        for (name, value) in &reply.headers {
            if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response = response.with_header(h);
            }
        }
        return request.respond(response);
    }
    let text = if reply.empty {
        String::new()
    } else {
        serde_json::to_string_pretty(&reply.body).unwrap_or_default()
    };
    // Always a content length, never a chunked body: a client that reads
    // the bytes it was told about (the notebook, a script, the tests) gets
    // the whole document, and the ask doors answer above the 32 KB default.
    let mut response = Response::from_string(text)
        .with_status_code(StatusCode(reply.status))
        .with_chunked_threshold(usize::MAX)
        .with_header(Header::from_bytes("Content-Type", "application/json").expect("header"));
    for (name, value) in &reply.headers {
        if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response = response.with_header(h);
        }
    }
    request.respond(response)
}

/// Undo the percent-encoding of one query key or value (record 45: every
/// door reads its query decoded, once, here). Percent-decoding only: a `+`
/// stays a `+`, since it means one in a time's offset (`+02:00`) and may in
/// a principal; a space comes as `%20`.
pub(crate) fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    mut request: Request,
) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let query: HashMap<String, String> = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (decoded(k), decoded(v))))
        .collect();
    let mut body = String::new();
    // Record 42 S4: a derivative's body is a file, read by its door once
    // the caller passes and streamed into its place, never into memory.
    let upload = method == Method::Post && path == "/api/derivatives";
    // A body rides on a POST and on a PUT (the selection door); reading
    // it only on a POST was why a PUT selection could carry no document.
    if matches!(method, Method::Post | Method::Put) && !upload {
        let _ = request.as_reader().read_to_string(&mut body);
    }
    if path == "/api/events" && method == Method::Get {
        // Display plumbing: the open jobs, every second, until the client
        // goes away. Never the execution context.
        let all = query.get("all").is_some_and(|a| a == "1" || a == "true");
        events(doors, registry, request, all);
        return;
    }
    let caller = doors.auth.caller(&request);
    // Wave 4c §5.5: the actor of this call, read by every writer of
    // provenance on this thread.
    if let Ok(c) = &caller {
        nils_registry::actor::set(c.actor.clone());
    }
    if upload {
        let reply = match &caller {
            Ok(c) => match crate::derivatives::upload(registry, c, &query, &mut request) {
                Ok(r) | Err(r) => r,
            },
            Err(_) => caller.err().expect("an error"),
        };
        nils_registry::actor::clear();
        let _ = respond(request, reply);
        return;
    }
    // Wave 4b §12.3: the MCP door and its public metadata, before the
    // older doors and before a refusal, so a client learns where to
    // authenticate from the answer it gets.
    if let Some(reply) = crate::mcp::route(
        doors,
        registry,
        ask,
        caller.as_ref().ok(),
        method.as_str(),
        path,
        &body,
    ) {
        nils_registry::actor::clear();
        let _ = respond(request, reply);
        return;
    }
    let reply = match caller {
        Ok(caller) => route(doors, registry, ask, &caller, &method, path, &query, &body),
        Err(reply) => reply,
    };
    nils_registry::actor::clear();
    let _ = respond(request, reply);
}

/// One ask door called from inside the engine: what the MCP door's tools
/// run. The grants, the reader and the caps are the doors' own.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask_call(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    method: &str,
    path: &str,
    query: &HashMap<String, String>,
    body: &str,
) -> Reply {
    let segs = segments(path);
    match crate::ask_doors::route(doors, registry, ask, caller, method, &segs, query, body) {
        Some(Ok(reply)) | Some(Err(reply)) => reply,
        None => Reply::error(404, format!("{method} {path} is not an ask door")),
    }
}

pub(crate) fn json_body(body: &str) -> Result<serde_json::Value, Reply> {
    if body.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(body).map_err(|e| Reply::error(400, format!("the body is not JSON: {e}")))
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/').split('/').collect()
}

#[allow(clippy::too_many_arguments)]
fn route(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    body: &str,
) -> Reply {
    match routed(doors, registry, ask, caller, method, path, query, body) {
        Ok(r) => r,
        Err(r) => r,
    }
}

#[allow(clippy::too_many_arguments)]
fn routed(
    doors: &Doors,
    registry: &mut Registry,
    ask: &mut crate::ask_doors::AskState,
    caller: &Caller,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    body: &str,
) -> Result<Reply, Reply> {
    let principal = caller.principal.as_str();
    let segs = segments(path);
    // Wave 4b §12.2: the ask doors check their own grants, from the same
    // table as every other door.
    if let Some(r) = crate::ask_doors::route(
        doors,
        registry,
        ask,
        caller,
        method.as_str(),
        &segs,
        query,
        body,
    ) {
        return r;
    }
    let get = *method == Method::Get;
    let post = *method == Method::Post;
    let put = *method == Method::Put;
    // What the door needs (the suite contract, version 2). The instance
    // doors gate their detail themselves, with a gated refusal (Wave 5
    // §12.7), so only the grant is checked here.
    let (need, detail) = door(method.as_str(), &segs);
    let detail = if matches!(segs.as_slice(), ["api", "instances", ..]) {
        Detail::Plain
    } else {
        detail
    };
    caller.allowed(path, need, detail)?;
    // record 42 S4: the derivative doors that read
    if let Some(r) = crate::derivatives::route(registry, caller, get, &segs, query) {
        return r;
    }
    // record 43: the pipeline catalog and its runs
    // a run names a unit by its subject or session only at detail quasi
    let quasi = caller.allowed(path, need, Detail::Quasi).is_ok();
    // a pipeline is named by its id or `name@version`, which a client may
    // send percent-encoded (`volumes%401`): its segments are decoded once
    // here, as the query is
    let decoded_segs: Vec<String> = segs.iter().map(|s| decoded(s)).collect();
    let pipeline_segs: Vec<&str> = decoded_segs.iter().map(String::as_str).collect();
    if let Some(r) = crate::pipelines::route(registry, quasi, get, &pipeline_segs, query) {
        return r;
    }
    // record 49 A3: the pre-flight of a run
    if let Some(r) = crate::preflight::route(
        &doors.home,
        doors.pack_dir.as_deref(),
        registry,
        quasi,
        post,
        &pipeline_segs,
        body,
    ) {
        return r;
    }
    // record 42: the campaigns and the label sets
    if let Some(r) = crate::campaigns::route(
        doors,
        registry,
        ask,
        caller,
        method.as_str(),
        &segs,
        query,
        body,
    ) {
        return r;
    }
    // record 26: the linkage doors, under the table's grants like the rest
    if let Some(r) = crate::linkage_doors::route(
        &doors.home,
        registry,
        caller,
        method.as_str(),
        &segs,
        query,
        body,
    ) {
        return r;
    }
    let id_at = |i: usize| -> Result<i64, Reply> {
        segs.get(i)
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| Reply::error(404, format!("{path} names no id")))
    };
    let limit = query
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50);
    match segs.as_slice() {
        ["api", "capabilities"] if get => Ok(Reply::ok(capabilities(doors, registry, caller, ask))),
        ["api", "status"] if get => Ok(Reply::ok(crate::status_doc(&doors.home, registry)?)),
        ["api", "summary"] if get => {
            // Wave 5 §12.1: counts only, never a row of a person
            let since = query.get("since").map(|s| {
                // a bare date is the start of that day
                if s.len() == 10 {
                    format!("{s}T00:00:00Z")
                } else {
                    s.clone()
                }
            });
            if let Some(s) = &since
                && nils_registry::time::secs_of(s).is_none()
            {
                return Err(Reply::error(400, format!("since={s} is not an ISO date")));
            }
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::ok(
                crate::summary::document(registry, since.as_deref())
                    .map_err(|e| Reply::error(500, e.to_string()))?,
            ))
        }
        ["api", "packs"] if get => {
            let dir = doors
                .pack_dir
                .clone()
                .ok_or_else(|| Reply::error(404, "no pack directory"))?;
            Ok(Reply::ok(crate::packs_doc(&dir)?))
        }
        ["api", "packs", name] if get => {
            let dir = doors
                .pack_dir
                .clone()
                .ok_or_else(|| Reply::error(404, "no pack directory"))?;
            // Record 26: the site's adopted overlays give the terms per list.
            let overlays = nils_registry::overlay::list(registry.store())?;
            match crate::pack_doc(&dir, name, &overlays)? {
                Some(doc) => Ok(Reply::ok(doc)),
                None => Err(Reply::error(404, format!("no pack named {name}"))),
            }
        }
        ["api", "batches"] if get => {
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50);
            Ok(Reply::ok(crate::batches_doc(registry, limit)?))
        }
        ["api", "batches", _] if get => {
            let id = id_at(2)?;
            match crate::batch_doc(registry, id)? {
                Some(doc) => Ok(Reply::ok(doc)),
                None => Err(Reply::error(404, format!("no batch {id}"))),
            }
        }
        ["api", "quarantine"] if get => {
            let batch = query.get("batch").and_then(|b| b.parse::<i64>().ok());
            // record 26 §10: a quarantined file is about no subject, so a
            // cohort narrows to the batches that fed it
            let batches = match query.get("cohort") {
                Some(name) => Some(
                    nils_registry::cohort::batches_feeding(registry.store(), name)?
                        .ok_or_else(|| Reply::error(404, format!("no cohort named {name}")))?,
                ),
                None => None,
            };
            Ok(Reply::ok(crate::quarantine_doc(
                registry,
                batch,
                query.get("class").map(String::as_str),
                batches.as_deref(),
            )?))
        }
        // record 26 §9: the cohort doors, Data work
        ["api", "cohorts"] if get => Ok(Reply::ok(serde_json::Value::Array(
            nils_registry::cohort::list(registry.store())?,
        ))),
        ["api", "cohorts", name] if get => {
            match nils_registry::cohort::show(registry.store(), name)? {
                Some(doc) => Ok(Reply::ok(doc)),
                None => Err(Reply::error(404, format!("no cohort named {name}"))),
            }
        }
        ["api", "cohorts"] if post => {
            let doc = json_body(body)?;
            let name = doc["name"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "name is required"))?;
            let owner = doc["owner"].as_str().unwrap_or(principal);
            let made = nils_registry::cohort::create(
                registry,
                name,
                owner,
                doc["description"].as_str(),
                principal,
            )
            .map_err(cohort_err)?;
            let shown = nils_registry::cohort::show(registry.store(), &made.name)?
                .unwrap_or_else(|| serde_json::json!({"name": made.name}));
            Ok(Reply::created(shown))
        }
        ["api", "cohorts", name] if put => {
            let doc = json_body(body)?;
            for key in ["name", "owner", "description"] {
                if !(doc[key].is_null() || doc[key].is_string()) {
                    return Err(Reply::error(400, format!("{key} is a string")));
                }
            }
            if !(doc["retired"].is_null() || doc["retired"].is_boolean()) {
                return Err(Reply::error(400, "retired is true or false"));
            }
            let change = nils_registry::cohort::Change {
                name: doc["name"].as_str(),
                owner: doc["owner"].as_str(),
                description: doc.get("description").map(|d| d.as_str()),
                retired: doc["retired"].as_bool(),
            };
            let set = nils_registry::cohort::set(registry, name, &change, principal)
                .map_err(cohort_err)?;
            let shown = nils_registry::cohort::show(registry.store(), &set.name)?
                .unwrap_or_else(|| serde_json::json!({"name": set.name}));
            Ok(Reply::ok(shown))
        }
        ["api", "cohorts", name, "members"] if post => {
            let doc = json_body(body)?;
            let codes = |key: &str| -> Result<Vec<String>, Reply> {
                match doc.get(key) {
                    None | Some(serde_json::Value::Null) => Ok(Vec::new()),
                    Some(serde_json::Value::Array(a)) => a
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_string)
                                .ok_or_else(|| Reply::error(400, format!("{key}: a list of codes")))
                        })
                        .collect(),
                    Some(_) => Err(Reply::error(400, format!("{key}: a list of codes"))),
                }
            };
            let add = codes("add")?;
            let remove = codes("remove")?;
            if add.is_empty() && remove.is_empty() {
                return Err(Reply::error(
                    400,
                    "add or remove: the codes to add or remove",
                ));
            }
            let done = nils_registry::cohort::members(
                registry,
                name,
                &add,
                &remove,
                doc["why"].as_str(),
                principal,
            )
            .map_err(cohort_err)?;
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::ok(serde_json::json!({
                "cohort": name,
                "added": done.added,
                "already": done.already,
                "removed": done.removed,
                "not_members": done.not_members,
                "epoch": registry.meta().epoch,
            })))
        }
        // record 26 §11: why one stack was judged so, as `nils explain` says it
        ["api", "explain", _] if get => {
            let stack = id_at(2)?;
            // record 48 R2: a stack read blind says nothing a system said
            if crate::campaigns::blind_to(registry.store(), caller, stack)? {
                return Ok(Reply::ok(serde_json::json!({
                    "stack": stack, "blind": true, "axes": [], "review": [],
                })));
            }
            match crate::explain::document(registry.store(), stack, doors.pack_dir.as_deref())? {
                Some(mut doc) => {
                    // the words a rule matched are text a stack carried
                    if caller.access.detail < Detail::Quasi {
                        // record 49 R4b: a pipeline's items are read as totals
                        // on the review list, never one a unit
                        if let Some(list) = doc["review"].as_array_mut() {
                            list.retain(|r| r["kind"] != nils_registry::review::PIPELINE_QC_KIND);
                        }
                        for a in doc["axes"].as_array_mut().into_iter().flatten() {
                            for e in a["evidence"].as_array_mut().into_iter().flatten() {
                                if let Some(m) = e.as_object_mut() {
                                    m.remove("matched");
                                }
                            }
                        }
                    }
                    Ok(Reply::ok(doc))
                }
                None => Err(Reply::error(
                    404,
                    format!("stack {stack} has not been classified"),
                )),
            }
        }
        // Wave 4c §6.6: the knob engine.
        ["api", "classify", "signals"] if get => {
            let scope = query.get("scope").map(String::as_str).ok_or_else(|| {
                Reply::error(400, "scope: batch:<id>, origin:<name> or pack:<version>")
            })?;
            let scope =
                nils_classify::scope::Scope::parse(scope).map_err(|e| Reply::error(400, e))?;
            let mut doc = nils_classify::signals::signals(registry.store(), &scope)?;
            // kineuro/nils#94: the text each unresolved axis was matched
            // against, under the pack the engine serves, over a bounded sample
            let name = query
                .get("pack")
                .cloned()
                .unwrap_or_else(|| doors.ask_pack.clone());
            let found = doors.pack_dir.as_ref().and_then(|dir| {
                crate::packs_in(dir)
                    .ok()?
                    .into_iter()
                    .find(|p| p.file_name().is_some_and(|f| *f == *name))
            });
            doc["unresolved_texts"] = match found {
                Some(dir) => {
                    let pack = nils_pack::load(&dir, None)
                        .map_err(|e| Reply::error(500, format!("the pack {name}: {e}")))?;
                    let pack = with_adopted_overlay(registry, &dir, pack)?;
                    let sample = nils_classify::rehearse::sample_of(
                        query.get("sample").and_then(|s| s.parse().ok()),
                    );
                    nils_classify::signals::unresolved_texts(
                        registry.store(),
                        &pack,
                        &scope,
                        sample,
                    )?
                }
                None => serde_json::Value::Null,
            };
            Ok(Reply::ok(doc))
        }
        ["api", "classify", "try"] if post => {
            let doc = json_body(body)?;
            let scope = scope_of(&doc)?;
            let sample = nils_classify::rehearse::sample_of(doc["sample"].as_i64());
            let (_, tried) = rehearsed(doors, registry, &doc["overlay"], &scope, sample)?;
            Ok(Reply::ok(tried))
        }
        // Wave 5 §12.7: the gated instance door, shaped by the viewer study.
        ["api", "instances", stack, rest @ ..] if get => {
            crate::pyramid::door(registry, caller, stack, rest, query)
        }
        ["api", "backups"] if get => {
            // Wave 5 §10.3: the archives in the backup directory, each with
            // what it holds, how long it took and its last check, beside the
            // schedule, read in the registry's timezone.
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let schedule = crate::schedule::document(registry, jiff::Timestamp::now());
            let Some(dir) = doors.backup_dir.as_ref() else {
                return Ok(Reply::ok(serde_json::json!({
                    "dir": null,
                    "place": null,
                    "count": 0,
                    "archives": [],
                    "schedule": schedule,
                })));
            };
            let place = nils_registry::place::holding(
                registry.store(),
                nils_registry::place::Role::Backup,
                dir,
            )?
            .map(|p| serde_json::json!({"id": p.id, "name": p.name, "guarantees": p.guarantees}));
            let archives = crate::backup::archives(dir, &registry.meta().registry_id);
            Ok(Reply::ok(serde_json::json!({
                "dir": dir.display().to_string(),
                "place": place,
                "count": archives.len(),
                "archives": archives,
                "schedule": schedule,
            })))
        }
        ["api", "backups", "schedule"] if put => {
            let doc = json_body(body)?;
            let schedule = crate::schedule::Schedule::parse(
                doc["every"].as_str().unwrap_or(""),
                doc["at"].as_str(),
                doc["day"].as_str(),
                doc["keep"].as_i64(),
            )
            .map_err(|m| Reply::error(400, m))?;
            if schedule.every != crate::schedule::Every::Off && doors.backup_dir.is_none() {
                return Err(Reply::error(
                    409,
                    "no backup directory: start nils serve with --backup-dir",
                ));
            }
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let now = jiff::Timestamp::now();
            crate::schedule::set(registry, &schedule, principal, now)
                .map_err(|m| Reply::error(500, m))?;
            Ok(Reply::ok(crate::schedule::document(registry, now)))
        }
        ["api", "settings"] if get => {
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::ok(crate::schedule::calendar(registry)))
        }
        ["api", "settings"] if put => {
            // Wave 5 §10.3: the registry's calendar, which every dated
            // answer is read under; a change moves the epoch.
            let doc = json_body(body)?;
            registry
                .refresh_meta()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let meta = registry.meta();
            let locale = nils_ask::hash::Locale {
                timezone: doc["timezone"]
                    .as_str()
                    .map_or_else(|| meta.timezone.clone(), str::to_string),
                week_start: doc["week_start"]
                    .as_str()
                    .map_or_else(|| meta.week_start.clone(), str::to_lowercase),
            };
            crate::schedule::set_calendar(registry, &locale, principal)
                .map_err(|(code, m)| Reply::error(code, m))?;
            Ok(Reply::ok(crate::schedule::calendar(registry)))
        }
        // Record 28: the pseudonymiser's own tag policy, the constants of
        // this binary and nothing of the registry, so any grant opens it.
        ["api", "pseudonymize", "tags"] if get => {
            Ok(Reply::ok(nils_pseudonymize::policy::document()))
        }
        ["api", "sources"] if get => {
            // the Data page: each source place, its dataset, its digests and
            // totals; `?probe=1` counts the trees again first, since a page
            // reads the counts the last probe kept and never walks a tree
            let recent = query
                .get("recent")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(12)
                .clamp(1, 100);
            if query.get("probe").is_some_and(|p| p == "1" || p == "true") {
                use nils_registry::place;
                for p in place::active(registry.store())? {
                    if p.role == place::Role::Source {
                        let probed = crate::dataset::probe_place(&p);
                        place::set(registry.store(), p.id, None, None, Some(&probed))?;
                    }
                }
            }
            Ok(Reply::ok(
                crate::sources::document(registry, recent)
                    .map_err(|e| Reply::error(500, e.to_string()))?,
            ))
        }
        ["api", "places"] if get => {
            // Wave 5 §12.5: every place with its role, guarantees, probe and
            // the deployment's paths bound under it. `?probe=1` measures
            // every active place again first.
            use nils_registry::place;
            let refresh = query.get("probe").is_some_and(|p| p == "1" || p == "true");
            let mut rows = place::list(registry.store())?;
            if refresh {
                let mut fresh = Vec::with_capacity(rows.len());
                for p in rows {
                    if p.retired_at.is_some() {
                        fresh.push(p);
                        continue;
                    }
                    let probed = crate::dataset::probe_place(&p);
                    fresh.push(place::set(
                        registry.store(),
                        p.id,
                        None,
                        None,
                        Some(&probed),
                    )?);
                }
                rows = fresh;
            }
            let configured: Vec<(&str, &std::path::Path)> = doors
                .ingest_roots
                .values()
                .map(|p| ("serve --ingest-root", p.as_path()))
                .chain(
                    doors
                        .backup_dir
                        .iter()
                        .map(|p| ("serve --backup-dir", p.as_path())),
                )
                .chain(std::iter::once(("registry", doors.home.dir())))
                .collect();
            let places: Vec<serde_json::Value> = rows
                .iter()
                .map(|p| {
                    let mut doc = p.as_json();
                    doc["bound"] = serde_json::json!(crate::places::bound_paths(p, &configured));
                    doc["holds"] = serde_json::json!(p.role.holds());
                    doc["must"] = serde_json::json!(p.role.must());
                    doc
                })
                .collect();
            Ok(Reply::ok(serde_json::json!({
                "count": places.len(),
                "enforced": rows.iter().any(|p| p.retired_at.is_none()),
                "places": places,
                "bindings": crate::places::bindings_doc(),
            })))
        }
        ["api", "places", _, "originals"] if get => {
            // record 26 §1: what a vault or a purge would do, without doing
            // it. Every original is checked against the pseudonymised tree
            // exactly as a purge checks it, so this answer costs what a
            // purge costs and is never read off the rows alone.
            let id = id_at(2)?;
            let dataset = crate::originals::dataset_at(registry.store(), id)
                .map_err(|r| Reply::error(r.status, r.message))?;
            let surveyed = crate::originals::survey(registry, &dataset)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::ok(surveyed.as_json()))
        }
        ["api", "places", _, "originals"] if post => {
            // record 26 §1: the act itself, as an `originals` job. What can
            // be known without reading every file is refused here in words;
            // the job asks again when it runs and refuses in the same words.
            let id = id_at(2)?;
            let dataset = crate::originals::dataset_at(registry.store(), id)
                .map_err(|r| Reply::error(r.status, r.message))?;
            let doc = json_body(body)?;
            let asked = doc["do"].as_str().unwrap_or_default();
            let act = crate::originals::Act::parse(asked)
                .ok_or_else(|| Reply::error(400, format!("do is vault or purge, not {asked:?}")))?;
            let why = doc["why"]
                .as_str()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .ok_or_else(|| {
                    Reply::error(400, "why: a sentence saying why, which the audit row keeps")
                })?;
            let into = doc["into"]
                .as_str()
                .map(str::trim)
                .filter(|i| !i.is_empty());
            crate::originals::check(registry, &dataset, act, into)
                .map_err(|r| Reply::error(r.status, r.message))?;
            let mut command = vec![
                "place".to_string(),
                "originals".to_string(),
                dataset.name.clone(),
                format!("--{}", act.name()),
            ];
            if let Some(into) = into {
                command.extend(["--into".to_string(), into.to_string()]);
            }
            command.extend(["--why".to_string(), why.to_string()]);
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                Some(&dataset.name),
                Some(principal),
                queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(serde_json::json!({
                "job": job,
                "state": "queued",
                "do": act.name(),
                "place": dataset.name,
                "into": into,
                "command": command,
            })))
        }
        ["api", "places"] if post => {
            use nils_registry::place::{self, Role as PlaceRole};
            let doc = json_body(body)?;
            // lab 26c, finding 4: what became of the originals is an act's
            // to write, never a declaration's
            if let Some(refused) = crate::dataset::engine_written_refused(&doc) {
                return Err(Reply::error(refused.status, refused.message));
            }
            let name = doc["name"]
                .as_str()
                .filter(|n| !n.trim().is_empty())
                .ok_or_else(|| Reply::error(400, "name: one word the place is called"))?;
            let role_text = doc["role"].as_str().unwrap_or("");
            let role = PlaceRole::parse(role_text).ok_or_else(|| {
                Reply::error(
                    400,
                    format!(
                        "role is one of {}, not {role_text}",
                        PlaceRole::ALL
                            .iter()
                            .map(|r| r.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;
            let path_text = doc["path"]
                .as_str()
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| Reply::error(400, "path: the directory"))?;
            let path = std::path::PathBuf::from(path_text);
            if !path.is_absolute() {
                return Err(Reply::error(
                    400,
                    format!("{path_text} is not an absolute path"),
                ));
            }
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let guarantees = if doc["guarantees"].is_object() {
                doc["guarantees"].clone()
            } else {
                serde_json::json!({"backup": null, "snapshots": false, "protected": false, "fast": false})
            };
            // record 26: a source place is a dataset. Its fields need
            // data:work beside places:work, and the folder is looked at
            // whichever were given, so a v0 folder is recognised and the
            // trees are set before anything reads it.
            let asked = dataset_asked(&doc);
            if crate::dataset::fields_given(&asked) {
                if role != PlaceRole::Source {
                    return Err(Reply::error(
                        400,
                        format!(
                            "a dataset is a source place; {name} is declared as a {} place",
                            role.name()
                        ),
                    ));
                }
                caller.allowed(
                    "POST /api/places with dataset fields",
                    Need::One("data:work"),
                    Detail::Plain,
                )?;
            }
            let (probed, dataset, layout) = if role == PlaceRole::Source {
                if place::by_name(registry.store(), name)?.is_some() {
                    return Err(Reply::error(
                        409,
                        format!("a place is already named {name}"),
                    ));
                }
                let d = crate::dataset::declare(registry.store(), &path, &asked, None)
                    .map_err(|r| Reply::error(r.status, r.message))?;
                (d.probed, d.dataset, d.layout)
            } else {
                (
                    crate::places::probe(&path),
                    serde_json::Value::Null,
                    serde_json::Value::Null,
                )
            };
            let id = place::add(
                registry.store(),
                &place::New {
                    name,
                    role,
                    path: &path.display().to_string(),
                    guarantees,
                    probed,
                    handling: doc["handling"].clone(),
                    dataset,
                },
            )
            .map_err(|e| match e {
                nils_registry::store::Error::Message(m) => Reply::error(409, m),
                other => Reply::error(500, other.to_string()),
            })?;
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal,
                    action: nils_registry::audit::Action::PlaceAdd,
                    scope: serde_json::json!({"place": id, "name": name, "role": role.name()}),
                    policy: None,
                    job_id: None,
                    details: layout
                        .is_object()
                        .then(|| serde_json::json!({"layout": layout})),
                },
            )?;
            let p = place::show(registry.store(), id)?
                .ok_or_else(|| Reply::error(500, format!("place {id} was not written")))?;
            let mut answer = p.as_json();
            answer["layout"] = layout;
            Ok(Reply::created(answer))
        }
        ["api", "places", _] if put => {
            use nils_registry::place;
            let id = id_at(2)?;
            let doc = json_body(body)?;
            // lab 26c, finding 4: the same at the door that changes a
            // dataset, which is the one the desk's Change form sends to
            if let Some(refused) = crate::dataset::engine_written_refused(&doc) {
                return Err(Reply::error(refused.status, refused.message));
            }
            let current = place::show(registry.store(), id)?
                .ok_or_else(|| Reply::error(404, format!("no place {id}")))?;
            if doc["retired"].as_bool() == Some(true) {
                let p = place::retire(registry.store(), id)?;
                nils_registry::audit::record(
                    registry,
                    &nils_registry::audit::Entry {
                        principal,
                        action: nils_registry::audit::Action::PlaceRetire,
                        scope: serde_json::json!({"place": id, "name": current.name}),
                        policy: None,
                        job_id: None,
                        details: None,
                    },
                )?;
                return Ok(Reply::ok(p.as_json()));
            }
            let path = match doc["path"].as_str() {
                Some(text) => {
                    let path = std::path::PathBuf::from(text);
                    if !path.is_absolute() {
                        return Err(Reply::error(400, format!("{text} is not an absolute path")));
                    }
                    Some(std::fs::canonicalize(&path).unwrap_or(path))
                }
                None => None,
            };
            let guarantees = doc["guarantees"]
                .is_object()
                .then(|| doc["guarantees"].clone());
            // how what comes in through the place is handled: checked here,
            // so a value that is not a handling is refused before anything is written
            let handling = match doc.get("handling") {
                None | Some(serde_json::Value::Null) => None,
                Some(h) => Some(place::handling_of(h).map_err(|m| Reply::error(400, m))?),
            };
            // record 26: the dataset fields need data:work beside
            // places:work; a source place whose dataset, path or arrival
            // changes has its folder looked at again
            let asked = dataset_asked(&doc);
            let dataset_given = crate::dataset::fields_given(&asked);
            if dataset_given {
                if current.role != place::Role::Source {
                    return Err(Reply::error(
                        400,
                        format!(
                            "a dataset is a source place; {} is a {} place",
                            current.name,
                            current.role.name()
                        ),
                    ));
                }
                caller.allowed(
                    "PUT /api/places/{id} with dataset fields",
                    Need::One("data:work"),
                    Detail::Plain,
                )?;
            }
            let looked = current.role == place::Role::Source && (dataset_given || path.is_some());
            let (probed, declared) = if looked {
                let folder = path
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from(&current.path));
                let d = crate::dataset::declare(registry.store(), &folder, &asked, Some(&current))
                    .map_err(|r| Reply::error(r.status, r.message))?;
                (Some(d.probed.clone()), Some(d))
            } else {
                (path.as_deref().map(crate::places::probe), None)
            };
            let mut p = place::set(
                registry.store(),
                id,
                path.as_deref().map(|p| p.display().to_string()).as_deref(),
                guarantees.as_ref(),
                probed.as_ref(),
            )
            .map_err(|e| match e {
                nils_registry::store::Error::Message(m) => Reply::error(409, m),
                other => Reply::error(500, other.to_string()),
            })?;
            if let Some(h) = &handling {
                p = place::set_handling(registry.store(), id, h).map_err(|e| match e {
                    nils_registry::store::Error::Message(m) => Reply::error(409, m),
                    other => Reply::error(500, other.to_string()),
                })?;
            }
            if let Some(d) = &declared {
                p = place::set_dataset(registry.store(), id, &d.dataset).map_err(|e| match e {
                    nils_registry::store::Error::Message(m) => Reply::error(409, m),
                    other => Reply::error(500, other.to_string()),
                })?;
            }
            let before = current.as_json();
            let mut details = serde_json::Map::new();
            if let Some(h) = &handling {
                details.insert(
                    "handling".into(),
                    serde_json::json!({"before": before["handling"], "after": h}),
                );
            }
            if let Some(d) = &declared {
                details.insert(
                    "dataset".into(),
                    serde_json::json!({"before": before["dataset"], "after": d.dataset, "layout": d.layout}),
                );
            }
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal,
                    action: nils_registry::audit::Action::PlaceSet,
                    scope: serde_json::json!({"place": id, "name": current.name}),
                    policy: None,
                    job_id: None,
                    details: (!details.is_empty()).then(|| serde_json::Value::Object(details)),
                },
            )?;
            let mut answer = p.as_json();
            answer["layout"] = declared
                .map(|d| d.layout)
                .unwrap_or(serde_json::Value::Null);
            Ok(Reply::ok(answer))
        }
        ["api", "overlays"] if get => {
            let rows = nils_registry::overlay::list(registry.store())?;
            Ok(Reply::ok(serde_json::json!({
                "overlays": rows.iter().map(|o| o.as_json(false)).collect::<Vec<_>>(),
            })))
        }
        ["api", "overlays"] if post => {
            let doc = json_body(body)?;
            let name = doc["name"]
                .as_str()
                .filter(|n| !n.trim().is_empty())
                .ok_or_else(|| Reply::error(400, "name: what the proposal is called"))?;
            let scope = scope_of(&doc)?;
            let sample = nils_classify::rehearse::sample_of(doc["sample"].as_i64());
            let (overlay, tried) = rehearsed(doors, registry, &doc["overlay"], &scope, sample)?;
            let (kind, _) = author_of(caller);
            let proposal = nils_registry::overlay::Proposal {
                name,
                version: overlay.id.rsplit('@').next().unwrap_or("0"),
                pack: &overlay.pack,
                author: principal,
                author_kind: kind,
                actor: caller.actor.clone(),
                scope: serde_json::json!({
                    "over": scope.text(),
                    "keyed": overlay.scope,
                }),
                document: doc["overlay"].clone(),
                tried: tried.clone(),
                why: doc["why"].as_str(),
            };
            let (id, item) = nils_registry::overlay::propose(registry.store(), &proposal)?;
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal,
                    action: nils_registry::audit::Action::OverlayPropose,
                    scope: serde_json::json!({"overlay": id, "scope": scope.text()}),
                    policy: None,
                    job_id: None,
                    details: Some(
                        serde_json::json!({"name": name, "pack": overlay.pack, "review_item": item, "tried": tried["review_items"]}),
                    ),
                },
            )?;
            let row = nils_registry::overlay::show(registry.store(), id)?;
            Ok(Reply::created(serde_json::json!({
                "overlay": row.map(|o| o.as_json(true)),
                "review_item": item,
                "status": nils_registry::overlay::PROPOSED,
            })))
        }
        ["api", "overlays", _] if get => {
            let id = id_at(2)?;
            match nils_registry::overlay::show(registry.store(), id)? {
                Some(o) => Ok(Reply::ok(o.as_json(true))),
                None => Err(Reply::error(404, format!("no overlay {id}"))),
            }
        }
        ["api", "overlays", _, "adopt"] if post => {
            let id = id_at(2)?;
            let Some(o) = nils_registry::overlay::show(registry.store(), id)? else {
                return Err(Reply::error(404, format!("no overlay {id}")));
            };
            if o.status != nils_registry::overlay::PROPOSED {
                return Err(Reply::error(
                    409,
                    format!(
                        "overlay {id} is {}, and only a proposed one is adopted",
                        o.status
                    ),
                ));
            }
            // The existing ranking: a lower author than the one who
            // proposed does not adopt over them.
            let (kind, _) = author_of(caller);
            if nils_registry::review::rank(kind) < nils_registry::review::rank(&o.author_kind) {
                return Err(Reply::error(
                    409,
                    format!(
                        "overlay {id} was proposed by a {}, and a {kind} does not adopt over them",
                        o.author_kind
                    ),
                ));
            }
            // Wave 5 §12.4, §12.8: the closure is computed before anything
            // moves; the handles it names stop reproducing, and the audit
            // row keeps the counts.
            let closure = match crate::depends::of(doors, registry, "overlay", &id.to_string())? {
                crate::depends::Outcome::Closure(c) => c,
                _ => return Err(Reply::error(404, format!("no overlay {id}"))),
            };
            let mut command = vec![
                "classify".to_string(),
                "--pack".into(),
                o.pack.clone(),
                "--overlay-id".into(),
                id.to_string(),
            ];
            // The worker runs where this engine runs, but knows no pack
            // directory of its own: the one this engine serves is named.
            if let Some(dir) = &doors.pack_dir {
                command.push("--pack-dir".into());
                command.push(dir.display().to_string());
            }
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                Some(&format!("adopt overlay {id}")),
                Some(principal),
                serde_json::json!({
                    "detail": caller.access.detail.name(),
                    "actor": caller.actor,
                    "overlay": id,
                }),
            )
            .map_err(job_err)?;
            nils_registry::overlay::decide(
                registry.store(),
                id,
                nils_registry::overlay::ADOPTED,
                principal,
                Some(job),
                None,
            )?;
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal,
                    action: nils_registry::audit::Action::OverlayAdopt,
                    scope: serde_json::json!({"overlay": id, "scope": o.scope}),
                    policy: None,
                    job_id: Some(job),
                    details: Some(
                        serde_json::json!({"name": o.name, "version": o.version, "pack": o.pack, "closure": closure.counts()}),
                    ),
                },
            )?;
            let invalidated = crate::depends::invalidate(
                registry,
                &closure,
                &format!("overlay {} {} adopted as {id}", o.name, o.version),
                principal,
            )?;
            if !invalidated.is_empty() {
                nils_registry::audit::record(
                    registry,
                    &nils_registry::audit::Entry {
                        principal,
                        action: nils_registry::audit::Action::HandleInvalidate,
                        scope: serde_json::json!({"overlay": id, "handles": invalidated.len()}),
                        policy: None,
                        job_id: Some(job),
                        details: Some(serde_json::json!({"handles": invalidated})),
                    },
                )?;
            }
            Ok(Reply::accepted(serde_json::json!({
                "job": job,
                "overlay": id,
                "status": nils_registry::overlay::ADOPTED,
                "closure": closure.counts(),
                "invalidated": invalidated,
            })))
        }
        // The desk's picker: the folders inside a folder of an ingest root, a
        // page at a time, and what a few of them hold, each named as
        // @root/relative and never outside the roots.
        ["api", "ingest", "folders"] if post => {
            let doc = json_body(body)?;
            crate::browse::folders_door(&doors.ingest_roots, registry.store(), &doc)
        }
        ["api", "ingest", "look"] if post => {
            let doc = json_body(body)?;
            crate::browse::look_door(&doors.ingest_roots, registry.store(), &doc)
        }
        ["api", "ingest", "probe"] if post => {
            let doc = json_body(body)?;
            // A pre-registered location, never a path: the name and an
            // optional relative part, resolved by the worker.
            let location = doc["location"].as_str().ok_or_else(|| {
                Reply::error(
                    400,
                    "location: a registered ingest location, as name or name/relative",
                )
            })?;
            let location = location.trim().trim_start_matches('@');
            let (name, rel) = location.split_once('/').unwrap_or((location, ""));
            if !doors.ingest_roots.contains_key(name) {
                return Err(Reply::error(
                    400,
                    format!(
                        "location {name} is not registered; this deployment names {}",
                        if doors.ingest_roots.is_empty() {
                            "none".to_string()
                        } else {
                            doors
                                .ingest_roots
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                ));
            }
            if rel.split('/').any(|s| s == "..") || rel.starts_with('/') {
                return Err(Reply::error(
                    400,
                    "location: the relative part stays inside the location",
                ));
            }
            let rules = doc["rules"]
                .as_array()
                .filter(|r| !r.is_empty())
                .ok_or_else(|| {
                    Reply::error(
                        400,
                        "rules: one or more candidate identity rules, as objects",
                    )
                })?;
            let mut command = vec![
                "ingest".to_string(),
                "probe".into(),
                format!(
                    "@{name}{}",
                    if rel.is_empty() {
                        String::new()
                    } else {
                        format!("/{rel}")
                    }
                ),
                "--sample".into(),
                nils_digest::probe::sample_of(doc["sample"].as_i64()).to_string(),
            ];
            for (i, r) in rules.iter().enumerate() {
                if !r.is_object() {
                    return Err(Reply::error(400, format!("rules[{i}]: an object")));
                }
                let text = serde_json::json!({"identity": r}).to_string();
                nils_digest::Rule::parse(&text)
                    .map_err(|e| Reply::error(400, format!("rules[{i}]: {e}")))?;
                command.push("--rule-json".into());
                command.push(r.to_string());
            }
            let job = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                doc["name"].as_str().or(Some("identity probe")),
                Some(principal),
                queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                serde_json::json!({ "job": job, "state": "queued" }),
            ))
        }
        ["api", "custody"] if get => Ok(Reply::ok(crate::custody_doc(&doors.home, registry)?)),
        ["api", "audit"] if get => {
            let rows = nils_registry::audit::list(
                registry.store(),
                &nils_registry::audit::Filter {
                    principal: query.get("principal").cloned(),
                    action: query.get("action").cloned(),
                    since: query.get("since").cloned(),
                    limit,
                },
            )?;
            Ok(Reply::ok(serde_json::json!({
                "count": rows.len(),
                "rows": rows.iter().map(|r| r.as_json()).collect::<Vec<_>>(),
            })))
        }
        ["api", "jobs"] if get => {
            let all = query.get("all").is_some_and(|a| a == "1" || a == "true");
            // the queue's worker is a row of its own kind, and not a job a
            // person queued or would cancel: listed with ?all only
            let jobs: Vec<_> = nils_registry::job::list(registry.store(), all, limit)
                .map_err(job_err)?
                .into_iter()
                .filter(|j| all || !nils_registry::job::is_worker(&j.kind))
                .collect();
            let mut docs: Vec<_> = jobs.iter().map(nils_registry::job::Job::as_json).collect();
            // record 49 R4: a run's result below detail quasi is its totals
            if !quasi {
                docs.iter_mut().for_each(crate::pipelines::job_totals_only);
            }
            Ok(Reply::ok(serde_json::json!({
                "count": jobs.len(),
                "jobs": docs,
            })))
        }
        ["api", "jobs"] if post => {
            // A command line for a worker: the one way anything heavy runs
            // through the door, answered 202 with the job's id.
            let doc = json_body(body)?;
            let command: Vec<String> = doc["command"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if command.is_empty() {
                return Err(Reply::error(
                    400,
                    "command: a nils command line, as a list, without the leading nils",
                ));
            }
            if !QUEUEABLE.contains(&command[0].as_str()) {
                return Err(Reply::error(
                    400,
                    format!(
                        "{} is not a verb the door queues; those are {}",
                        command[0],
                        QUEUEABLE.join(", ")
                    ),
                ));
            }
            // Wave 4c §6.5 and record 26 §6: of the linkage verbs `import`
            // and `merge` are jobs the door queues; purge and the rest stay
            // on the command line.
            if command[0] == "linkage"
                && !matches!(command.get(1).map(String::as_str), Some("import" | "merge"))
            {
                return Err(Reply::error(
                    400,
                    "linkage import and linkage merge are the linkage verbs the door queues",
                ));
            }
            // Record 26 §1: of the place verbs only the acts on a dataset's
            // originals are jobs; the rest declare a place and are not.
            if command[0] == "place" && command.get(1).map(String::as_str) != Some("originals") {
                return Err(Reply::error(
                    400,
                    "place originals is the place verb the door queues",
                ));
            }
            // Record 26 §7: the chain, `then: [command, ...]`, each a
            // command line queued when the one before ends done; and
            // `bring-in @dataset`, which stands for the thread of a
            // dataset with its own steps in front of whatever follows.
            let mut then = crate::chain::parse_then(&doc).map_err(|e| Reply::error(400, e))?;
            let mut command = command;
            let mut digest_first: Option<u64> = None;
            if command[0] == "bring-in" {
                let asked =
                    crate::chain::BringIn::parse(&command).map_err(|e| Reply::error(400, e))?;
                let place = nils_registry::place::by_name(registry.store(), &asked.dataset)?
                    .filter(|p| {
                        p.role == nils_registry::place::Role::Source && p.retired_at.is_none()
                    })
                    .ok_or_else(|| {
                        Reply::error(
                            400,
                            format!(
                                "@{} is not a dataset; bring-in names a source place",
                                asked.dataset
                            ),
                        )
                    })?;
                // a tree holding files no digest has read is digested
                // first, so the pseudonymiser knows what the tree holds
                let unread = crate::chain::unread_in_tree(registry.store(), &place);
                let (first, mut rest) = crate::chain::bring_in(
                    &place,
                    asked.name.as_deref(),
                    asked.pack.as_deref(),
                    unread.is_some(),
                );
                command = first;
                rest.append(&mut then);
                then = rest;
                if let Some(n) = unread {
                    digest_first = Some(n);
                }
            }
            // Wave 4c §6.5: a tree is named by a registered location, as
            // @name/relative, never by a path a caller composes; backup and
            // verify go to the deployment's backup directory.
            // The suite contract, version 2: each verb needs its own grant,
            // and a linkage import the sensitive detail it reads; the job
            // records the caller's detail, which the verb runs under.
            let Some((grant, detail)) = verb_needs(&command) else {
                return Err(Reply::error(
                    400,
                    "ask run and ask promote are the ask verbs the door queues",
                ));
            };
            let words = if matches!(command[0].as_str(), "ask" | "linkage") {
                2
            } else {
                1
            };
            let verb = command[..words.min(command.len())].join(" ");
            caller.allowed(&format!("{path} {verb}"), Need::One(grant), detail)?;
            let command = located(doors, registry.store(), command)?;
            // each step of the chain a verb the door queues, its tree
            // located now; its grant is checked when its turn comes, and a
            // refusal then ends the chain (record 26 §7)
            let mut steps = Vec::with_capacity(then.len());
            for step in then {
                let verb = step[0].as_str();
                if verb == "bring-in" || !QUEUEABLE.contains(&verb) || verb_needs(&step).is_none() {
                    return Err(Reply::error(
                        400,
                        format!("then: {verb} is not a verb the chain queues"),
                    ));
                }
                steps.push(located(doors, registry.store(), step)?);
            }
            let mut extra = queued_by(caller);
            if !steps.is_empty() {
                extra["then"] = serde_json::json!(steps);
            }
            if let Some(n) = digest_first {
                extra["digest_first"] = serde_json::json!({
                    "files": n,
                    "why": "the pseudonymised tree holds files no digest has read",
                });
            }
            let id = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                doc["name"].as_str(),
                Some(principal),
                extra,
            )
            .map_err(job_err)?;
            // the command as located, so a caller sees which tree @name was,
            // and why a digest goes first when one does
            Ok(Reply::accepted(serde_json::json!({
                "job": id, "state": "queued", "command": command, "then": steps,
                "digest_first": digest_first.map(|n| serde_json::json!({
                    "files": n,
                    "why": "the pseudonymised tree holds files no digest has read",
                })),
            })))
        }
        ["api", "jobs", _] if get => {
            let id = id_at(2)?;
            match nils_registry::job::show(registry.store(), id).map_err(job_err)? {
                Some(j) => {
                    let mut doc = j.as_json();
                    if !quasi {
                        crate::pipelines::job_totals_only(&mut doc);
                    }
                    Ok(Reply::ok(doc))
                }
                None => Err(Reply::error(404, format!("no job {id}"))),
            }
        }
        ["api", "jobs", _, "cancel"] if post => {
            let id = id_at(2)?;
            // a cancel needs the grant of the job's verb
            let Some(job) = nils_registry::job::show(registry.store(), id).map_err(job_err)? else {
                return Err(Reply::error(404, format!("no job {id}")));
            };
            caller.allowed(path, Need::One(cancel_needs(&job)), Detail::Plain)?;
            match nils_registry::job::request_cancel(registry.store(), id).map_err(job_err)? {
                Some(state) => Ok(Reply::ok(
                    serde_json::json!({ "job": id, "state": state.name() }),
                )),
                None => Err(Reply::error(404, format!("no job {id}"))),
            }
        }
        ["api", "releases"] if get => {
            // record 43: a run's input releases only when asked for
            let runs = query.get("runs").is_some_and(|v| v == "true" || v == "1");
            Ok(Reply::ok(crate::releases_doc(registry, limit, None, runs)?))
        }
        ["api", "releases"] if post => {
            // Heavy: a queued `nils release`, 202.
            let doc = json_body(body)?;
            let mut command = vec!["release".to_string()];
            let name = doc["name"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "name is required"))?;
            let out = doc["out"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "out is required"))?;
            // Wave 5 §10.2: a release writes only to an export place, refused
            // at the door and not discovered on disk.
            crate::places::require(
                registry.store(),
                nils_registry::place::Role::Export,
                std::path::Path::new(out),
            )
            .map_err(|r| Reply::error(409, r.message))?;
            // Record 38 S3: the date is the date. `keep`, which a desk from
            // before may send, is passed on; anything else is refused here
            // rather than by the job after it has queued.
            match &doc["dates"] {
                serde_json::Value::Null => {}
                serde_json::Value::String(d) if d == "keep" => {}
                other => {
                    return Err(Reply::error(
                        400,
                        format!("dates {other}: {}", nils_registry::place::DATES_ARE_KEPT),
                    ));
                }
            }
            command.extend(["--name".into(), name.into(), "--out".into(), out.into()]);
            for (flag, key) in [
                ("--layout", "layout"),
                ("--naming", "naming"),
                ("--dates", "dates"),
                ("--uids", "uids"),
                ("--on-unknown", "on_unknown"),
                ("--pack", "pack"),
                ("--scheme-name", "scheme_name"),
            ] {
                if let Some(v) = doc[key].as_str() {
                    command.extend([flag.to_string(), v.to_string()]);
                }
            }
            if let Some(dir) = &doors.pack_dir {
                command.extend(["--pack-dir".into(), dir.display().to_string()]);
            }
            for (flag, key) in [
                ("--subject", "subjects"),
                ("--session", "sessions"),
                ("--cohort", "cohorts"),
                // record 26 §13: a dataset as a selection
                ("--dataset", "datasets"),
                ("--axis", "axes"),
                ("--observation", "observations"),
            ] {
                for v in doc[key].as_array().into_iter().flatten() {
                    if let Some(v) = v.as_str() {
                        command.extend([flag.to_string(), v.to_string()]);
                    }
                }
            }
            for v in doc["stacks"].as_array().into_iter().flatten() {
                if let Some(n) = v.as_i64() {
                    command.extend(["--stack".into(), n.to_string()]);
                }
            }
            // Wave 5 §6.6: a handle as the source of a release is refused at
            // the server when it is stale; a stack handle's keys become the
            // stacks released.
            if let Some(hid) = doc["handle"].as_i64() {
                let h = nils_ask::handle::get(registry.store(), hid)?
                    .ok_or_else(|| Reply::error(404, format!("no handle {hid}")))?;
                if let Some(why) = crate::ask_doors::not_reproducible(registry, &h, "released")? {
                    return Err(Reply::error(409, why));
                }
                if h.grain != nils_ask::ast::Grain::Stack {
                    return Err(Reply::error(
                        400,
                        format!(
                            "handle {hid} is at {} grain; a release from a handle takes a stack handle",
                            h.grain.name()
                        ),
                    ));
                }
                for (key, _) in nils_ask::handle::keys(registry.store(), hid)? {
                    command.extend(["--stack".into(), key.to_string()]);
                }
            }
            let id = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                Some(name),
                Some(principal),
                queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                serde_json::json!({ "job": id, "state": "queued", "command": command }),
            ))
        }
        ["api", "handovers"] if post => {
            let doc = json_body(body)?;
            let release = doc["release"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "release is required"))?;
            let out = doc["out"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "out is required"))?;
            // Wave 5 §10.2: a handover writes only to an exchange place.
            crate::places::require(
                registry.store(),
                nils_registry::place::Role::Exchange,
                std::path::Path::new(out),
            )
            .map_err(|r| Reply::error(409, r.message))?;
            let mut command = vec![
                "handover".to_string(),
                "run".to_string(),
                "--release".to_string(),
                release.to_string(),
                "--out".to_string(),
                out.to_string(),
            ];
            if let Some(k) = doc["key"].as_str() {
                command.extend(["--key".into(), k.into()]);
            }
            let id = nils_registry::job::enqueue_with(
                registry.store(),
                &command,
                Some(release),
                Some(principal),
                queued_by(caller),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                serde_json::json!({ "job": id, "state": "queued", "command": command }),
            ))
        }
        ["api", "select"] if post => {
            // The bounded synchronous path: what a selection reaches,
            // without releasing it.
            let doc = json_body(body)?;
            let items = nils_release::select::Item::parse_all(&doc.to_string())
                .map_err(|e| Reply::error(400, e))?;
            let pack = doors.pack_dir.as_ref().and_then(|dir| {
                let name = doc["pack"].as_str().unwrap_or("mri");
                crate::packs_in(dir)
                    .ok()
                    .and_then(|found| {
                        found
                            .into_iter()
                            .find(|p| p.file_name().is_some_and(|f| f == name))
                    })
                    .and_then(|found| nils_pack::load(&found, None).ok())
            });
            let resolved = nils_release::select::resolve(registry, &items, pack.as_ref())
                .map_err(|e| Reply::error(400, e.to_string()))?;
            let preview = nils_release::run::preview(registry.store(), &resolved.selection)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let mut out = resolved.as_json();
            out["reaches"] = serde_json::json!({
                "subjects": preview.subjects, "studies": preview.studies,
                "stacks": preview.stacks, "files": preview.files, "bytes": preview.bytes,
            });
            Ok(Reply::ok(out))
        }
        ["api", "review"] if get => {
            let status = query.get("status").map(String::as_str);
            // record 26 §10: a cohort keeps the items whose subject, through
            // the item's stack, series or subject, holds an open membership
            let keep = match query.get("cohort") {
                Some(name) => {
                    let members =
                        nils_registry::cohort::open_members_of(registry.store(), name)?
                            .ok_or_else(|| Reply::error(404, format!("no cohort named {name}")))?;
                    let about = nils_registry::cohort::items_about(registry.store(), status)?;
                    Some(
                        about
                            .iter()
                            .filter(|it| it.subjects.iter().any(|s| members.contains(s)))
                            .map(|it| it.id)
                            .collect::<std::collections::HashSet<i64>>(),
                    )
                }
                None => None,
            };
            let mut rows = review_list(
                registry.store(),
                status,
                query.get("kind").map(String::as_str),
                if keep.is_some() {
                    i64::MAX as usize
                } else {
                    limit
                },
            )?;
            if let Some(keep) = keep {
                rows.retain(|r| r["id"].as_i64().is_some_and(|id| keep.contains(&id)));
                rows.truncate(limit.max(1));
            }
            blind_review(registry.store(), caller, &mut rows)?;
            // record 49 R4 and R4b: below detail quasi a pipeline's items
            // are one entry a run and a check or reason, with a count held
            // to 5 scans, never one a unit
            if !quasi {
                rows = crate::pipelines::qc_items_grouped(rows);
            }
            Ok(Reply::ok(
                serde_json::json!({ "count": rows.len(), "items": rows }),
            ))
        }
        ["api", "review", "summary"] if get => {
            let cohort = query.get("cohort").map(String::as_str);
            if let Some(name) = cohort
                && nils_registry::cohort::by_name(registry.store(), name)?.is_none()
            {
                return Err(Reply::error(404, format!("no cohort named {name}")));
            }
            let mut doc = nils_registry::cohort::review_summary(registry.store(), cohort)?;
            // record 49 R4b: the open pipeline items are units; 1 to 4 held
            if !quasi {
                crate::pipelines::hold_count(
                    &mut doc["by_kind"],
                    nils_registry::review::PIPELINE_QC_KIND,
                );
            }
            Ok(Reply::ok(doc))
        }
        ["api", "review", _] if get => {
            let id = id_at(2)?;
            let Some(item) =
                nils_registry::review::item(registry.store(), id).map_err(review_err)?
            else {
                return Err(Reply::error(404, format!("no review item {id}")));
            };
            let members = if item.scope == "group" {
                nils_registry::review::members(registry.store(), id)
                    .map_err(review_err)?
                    .iter()
                    .map(|m| serde_json::json!({ "stack_id": m.stack_id, "evidence": m.evidence, "decided_at": m.decided_at }))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let mut doc = serde_json::json!({
                "id": item.id, "kind": item.kind, "scope": item.scope, "status": item.status,
                "ref": item.reference, "evidence": item.evidence, "members": item.members,
                "member_stacks": members,
            });
            blind_review(registry.store(), caller, std::slice::from_mut(&mut doc))?;
            // record 49 R4b: below detail quasi a pipeline's item is not read
            // one by one, and answers as an item that is not there
            if !quasi && item.kind == nils_registry::review::PIPELINE_QC_KIND {
                return Err(Reply::error(404, format!("no review item {id}")));
            }
            Ok(Reply::ok(doc))
        }
        ["api", "review", _, "apply"] if post && json_body(body)?["values"].is_object() => {
            // Record 45 R5: an item that asks several axes (System 1's
            // classify.asked, a campaign's axes item) answered whole, one
            // decision per axis in one transaction, held to the served
            // pack's legal combinations
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let item = nils_registry::review::item(registry.store(), id)
                .map_err(review_err)?
                .ok_or_else(|| Reply::error(404, format!("no review item {id}")))?;
            not_held(registry, id)?;
            let axes: Vec<String> = item.evidence["axes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
            if axes.is_empty() {
                return Err(Reply::error(
                    400,
                    format!("review item {id} asks about one axis; answer it with value"),
                ));
            }
            let pack = doors
                .pack_dir
                .as_ref()
                .and_then(|dir| nils_pack::load(&dir.join(&doors.ask_pack), None).ok())
                .ok_or_else(|| {
                    Reply::error(
                        409,
                        "an answer to several axes is held to the pack's legal combinations, and no pack is served here",
                    )
                })?;
            let constraints =
                nils_pack::legal::constraints(&pack, &axes, &std::collections::BTreeMap::new())
                    .map_err(|e| Reply::error(400, e))?;
            let joint =
                nils_registry::campaign::joint_of(&axes, &constraints, &doc["values"].to_string())
                    .map_err(|e| Reply::error(400, e.to_string()))?;
            nils_registry::campaign::legal(&constraints, &joint)
                .map_err(|e| Reply::error(400, e))?;
            let values: Vec<(String, Option<String>)> = joint
                .iter()
                .map(|(a, v)| (a.clone(), (!v.is_empty()).then(|| v.join(","))))
                .collect();
            let (kind, version, model) = author_at_apply(registry, caller, &doc)?;
            let applied = nils_registry::review::apply_values(
                registry,
                &nils_registry::review::Apply {
                    item: id,
                    member: None,
                    scope: "stack",
                    value: None,
                    author: nils_registry::review::Author {
                        who: principal,
                        kind,
                        version,
                        model,
                    },
                    stage: doc["stage"].as_bool().unwrap_or(false),
                    why: doc["why"].as_str(),
                    campaign: None,
                },
                &values,
            )
            .map_err(review_err)?;
            Ok(Reply::ok(serde_json::json!({
                "decisions": applied.iter().map(|a| serde_json::json!({
                    "decision": a.decision, "axis": a.axis, "scope": a.scope,
                    "ref": a.reference, "closed": a.closed, "staged": a.staged,
                })).collect::<Vec<_>>(),
                "staged": applied.iter().any(|a| a.staged),
            })))
        }
        ["api", "review", _, "apply"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let nothing = doc["nothing"].as_bool().unwrap_or(false);
            let value = doc["value"].as_str();
            if value.is_none() && !nothing {
                return Err(Reply::error(400, "value, or nothing: true"));
            }
            not_held(registry, id)?;
            let (kind, version, model) = author_at_apply(registry, caller, &doc)?;
            let applied = nils_registry::review::apply(
                registry,
                &nils_registry::review::Apply {
                    item: id,
                    member: doc["member"].as_i64(),
                    scope: doc["scope"].as_str().unwrap_or("stack"),
                    value,
                    author: nils_registry::review::Author {
                        who: principal,
                        kind,
                        version,
                        model,
                    },
                    stage: doc["stage"].as_bool().unwrap_or(false),
                    why: doc["why"].as_str(),
                    campaign: None,
                },
            )
            .map_err(review_err)?;
            Ok(Reply::ok(serde_json::json!({
                "decision": applied.decision, "axis": applied.axis, "scope": applied.scope,
                "ref": applied.reference, "closed": applied.closed, "members": applied.members,
                "staged": applied.staged,
                "model": applied.model.as_ref().map(nils_registry::model::Model::named),
            })))
        }
        ["api", "review", _, "accept"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            crate::review_accept_as(
                registry,
                id,
                doc["why"].as_str().map(String::from),
                principal,
            )?;
            Ok(Reply::ok(
                serde_json::json!({ "review_item": id, "accepted_by": principal }),
            ))
        }
        ["api", "decisions", _, "commit"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let done = nils_registry::review::commit_as(
                registry,
                Some(id),
                doc["anyway"].as_bool().unwrap_or(false),
                principal,
                author_of(caller).0,
            )
            .map_err(review_err)?;
            Ok(Reply::ok(
                serde_json::json!({ "committed": done.decisions, "items": done.items }),
            ))
        }
        ["api", "decisions", _, "withdraw"] if post => {
            let id = id_at(2)?;
            let reopened =
                nils_registry::review::withdraw_as(registry, id, principal, author_of(caller).0)
                    .map_err(review_err)?;
            Ok(Reply::ok(
                serde_json::json!({ "withdrawn": id, "reopened": reopened }),
            ))
        }
        // Record 42 S2: the model registry.
        ["api", "models"] if get => {
            let filter = nils_registry::model::Filter {
                task: query.get("task").map(String::as_str),
                slot: query.get("slot").map(String::as_str),
                state: query.get("state").map(String::as_str),
            };
            let models = nils_registry::model::list(registry.store(), &filter)?;
            Ok(Reply::ok(serde_json::json!({
                "count": models.len(),
                "models": models.iter().map(nils_registry::model::Model::to_json).collect::<Vec<_>>(),
            })))
        }
        ["api", "models"] if post => {
            let doc = json_body(body)?;
            let m = nils_registry::model::register(registry, &doc, principal).map_err(model_err)?;
            Ok(Reply::ok(m.to_json()))
        }
        ["api", "models", _] if get => {
            let id = id_at(2)?;
            let Some(m) = nils_registry::model::get(registry.store(), id)? else {
                return Err(Reply::error(404, format!("no model {id}")));
            };
            let mut doc = m.to_json();
            doc["events"] =
                serde_json::Value::from(nils_registry::model::events(registry.store(), id)?);
            Ok(Reply::ok(doc))
        }
        ["api", "models", _, "admit"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let m =
                nils_registry::model::admit(registry, id, &doc, principal).map_err(model_err)?;
            Ok(Reply::ok(m.to_json()))
        }
        ["api", "models", _, "promote"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let done = nils_registry::model::promote(
                registry,
                id,
                principal,
                doc["review_item"].as_i64(),
                doc["why"].as_str(),
            )
            .map_err(model_err)?;
            Ok(Reply::ok(serde_json::json!({
                "model": done.model.to_json(),
                "retired": done.retired.as_ref().map(nils_registry::model::Model::to_json),
            })))
        }
        ["api", "models", _, "retire"] if post => {
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let m = nils_registry::model::retire(registry, id, principal, doc["why"].as_str())
                .map_err(model_err)?;
            Ok(Reply::ok(m.to_json()))
        }
        // Record 42 S3: a person's pick, which a pick run leaves standing,
        // and its withdrawal. A pick a person writes is a person's: an
        // agent or a model acting for the principal is refused here, since
        // its answer is evidence for a person and not a pick.
        ["api", "picks"] if post => {
            let (kind, _) = author_of(caller);
            if kind != "person" {
                return Err(Reply::error(
                    403,
                    format!(
                        "a pick is written by a person; X-Nils-Actor names a {kind} acting for {principal}"
                    ),
                ));
            }
            let doc = json_body(body)?;
            let role = doc["role"]
                .as_str()
                .ok_or_else(|| Reply::error(400, "role names the role the pick stands for"))?;
            let stacks: Vec<i64> = doc["stacks"]
                .as_array()
                .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
                .unwrap_or_default();
            let why = doc["why"].as_str().unwrap_or_default();
            let name = doc["pack"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| doors.ask_pack.clone());
            let found = doors.pack_dir.as_ref().and_then(|dir| {
                crate::packs_in(dir)
                    .ok()?
                    .into_iter()
                    .find(|p| p.file_name().is_some_and(|f| *f == *name))
            });
            let Some(found) = found else {
                return Err(Reply::error(
                    409,
                    format!("no pack named {name} is served, and a pick is the pack's"),
                ));
            };
            let pack = nils_pack::load(&found, None)
                .map_err(|e| Reply::error(500, format!("the pack {name}: {e}")))?;
            let scheme = match doc["scheme"].as_str() {
                None | Some("default" | "day") => nils_registry::session::Scheme::default(),
                Some(n) => crate::stored_scheme(registry, n).map_err(Reply::from)?,
            };
            let picked = nils_classify::picking::set_person(
                registry,
                &pack,
                &scheme,
                &nils_classify::picking::PersonPick {
                    role,
                    stacks: &stacks,
                    model: doc["pick"].as_str(),
                    why,
                    actor: principal,
                    campaign: None,
                    occasion: None,
                },
            )
            .map_err(pick_err)?;
            Ok(Reply::created(
                serde_json::to_value(picked).unwrap_or(serde_json::Value::Null),
            ))
        }
        ["api", "picks", _, "withdraw"] if post => {
            // a person's pick is a person's to withdraw, as it is theirs to
            // write
            let (kind, _) = author_of(caller);
            if kind != "person" {
                return Err(Reply::error(
                    403,
                    format!(
                        "a pick is withdrawn by a person; X-Nils-Actor names a {kind} acting for {principal}"
                    ),
                ));
            }
            let id = id_at(2)?;
            let doc = json_body(body)?;
            let done = nils_classify::picking::withdraw_person(
                registry,
                id,
                principal,
                doc["why"].as_str(),
            )
            .map_err(pick_err)?;
            Ok(Reply::ok(
                serde_json::to_value(done).unwrap_or(serde_json::Value::Null),
            ))
        }
        // Wave 5 section 12.2: every event on one object, in order.
        ["api", "depends", kind, id] if get => {
            match crate::depends::of(doors, registry, kind, id)? {
                crate::depends::Outcome::Closure(c) => Ok(Reply::ok(
                    serde_json::to_value(c).unwrap_or(serde_json::Value::Null),
                )),
                crate::depends::Outcome::NoKind => Err(Reply::error(
                    404,
                    format!(
                        "{kind} is not a kind the dependency door serves; the kinds are {}",
                        crate::depends::KINDS.join(", ")
                    ),
                )),
                crate::depends::Outcome::NoObject => {
                    Err(Reply::error(404, format!("no {kind} {id}")))
                }
            }
        }
        ["api", "timeline", kind, _] if get => {
            let id = id_at(3)?;
            match crate::timeline::of(registry, kind, id).map_err(|e| Reply::error(500, e))? {
                crate::timeline::Outcome::Events(events) => Ok(Reply::ok(serde_json::json!({
                    "kind": kind, "id": id, "count": events.len(), "events": events,
                }))),
                crate::timeline::Outcome::NoKind => Err(Reply::error(
                    404,
                    format!(
                        "{kind} is not a kind the timeline serves; the kinds are {}",
                        crate::timeline::KINDS.join(", ")
                    ),
                )),
                crate::timeline::Outcome::NoObject => {
                    Err(Reply::error(404, format!("no {kind} {id}")))
                }
            }
        }
        _ => Err(Reply::error(
            404,
            format!("{method} {path} is not a door; GET /api/capabilities lists them"),
        )),
    }
}

/// The verbs a queued command line may start with: what the door runs
/// through a worker, and nothing that reads a file the caller names.
const QUEUEABLE: &[&str] = &[
    // Wave 5 §12.7: the viewing pyramid of a stack, into a working place.
    "pyramid",
    // Wave 4c §6.5: an archive as a job, and its check; never a restore.
    "backup",
    "verify",
    "digest",
    // Record 26 §3 and §7: the pseudonymise step of a dataset, and the
    // whole thread of one as a chain.
    "pseudonymize",
    "bring-in",
    // Record 26 §1: `place originals`, the acts on a dataset's originals;
    // no other place verb is a job.
    "place",
    "fingerprint",
    "classify",
    "pick",
    "release",
    "handover",
    // Wave 4c §6.5: `linkage import` over a pre-registered location; the
    // CSV is named as @root/relative and never as a path of the host.
    "linkage",
    // Wave 4b §7: `session rebuild`, the cache built under the worker's
    // principal, never by a read door.
    "session",
    // Wave 4b §12.2: `ask run` and `ask promote`, the unbounded path and
    // the promotion, both jobs.
    "ask",
    // Record 43 S2: a pipeline over a frozen selection, a `pipeline` job.
    "run",
];

/// The grants of the verbs the door queues: `POST /api/jobs` needs any of
/// them, and each verb its own (`verb_needs`).
const JOB_GRANTS: &[&str] = &[
    "data:work",
    "database:work",
    "pipelines:work",
    "query:work",
    "release:work",
];

/// What a door needs (the suite contract, version 2): a grant, any of two,
/// or two at once, and the lowest detail. A door not named here needs a
/// grant, any grant: the capabilities, the status, the summary, the
/// calendar and the event stream. Every door reads this table, the ask
/// doors and the MCP door's tools included, and the policy rows are read
/// from it.
pub(crate) fn door(method: &str, segs: &[&str]) -> (Need, Detail) {
    use Detail::{Plain, Quasi};
    match (method, segs) {
        // the Query page: reading and running questions
        (
            "GET",
            [
                "api",
                "ask",
                "schema" | "catalog" | "guide" | "documents" | "handles",
            ],
        )
        | ("GET", ["api", "ask", "catalog", _])
        | ("GET", ["api", "ask", "catalog", _, _, "values"])
        | ("GET", ["api", "ask", "documents" | "selections" | "handles", _])
        | ("GET", ["api", "ask", "handles", _, "rows"])
        | (
            "POST",
            [
                "api",
                "ask",
                "draft" | "diff" | "validate" | "run" | "explain" | "options" | "apply"
                | "diagnose" | "preview" | "profile" | "describe" | "start",
            ],
        ) => (Need::One("query:see"), Plain),
        // Wave 5 §12.7: the viewer's pixels are quasi-identifying
        ("GET", ["api", "instances", _, ..]) => (Need::One("query:see"), Quasi),
        ("GET", ["api", "depends" | "timeline", _, _]) => {
            (Need::AnyOf(&["data:see", "query:see"]), Plain)
        }
        // saving what a question is; Wave 5 §12.1: a saved selection and an
        // identifier list are quasi-identifying territory
        ("POST", ["api", "ask", "documents" | "jobs"]) => (Need::One("query:work"), Plain),
        ("PUT", ["api", "ask", "selections", _]) | ("POST", ["api", "ask", "values"]) => {
            (Need::One("query:work"), Quasi)
        }
        // the Data page
        ("GET", ["api", "sources" | "packs" | "batches"])
        | ("GET", ["api", "packs" | "batches", _]) => (Need::One("data:see"), Plain),
        ("GET", ["api", "places"]) => (Need::AnyOf(&["data:see", "places:see"]), Plain),
        // record 26 §1: what becomes of a dataset's originals. What an act
        // would do is Data reading; the acts themselves move and delete
        // identified files, so they are Data work at detail sensitive.
        ("GET", ["api", "places", _, "originals"]) => (Need::One("data:see"), Plain),
        ("POST", ["api", "places", _, "originals"]) => (Need::One("data:work"), Detail::Sensitive),
        ("POST", ["api", "ingest", "folders" | "look" | "probe"]) => {
            (Need::One("data:work"), Plain)
        }
        // record 26 §9: the cohorts are Data work, a promotion among them
        ("GET", ["api", "cohorts"]) | ("GET", ["api", "cohorts", _]) => {
            (Need::One("data:see"), Plain)
        }
        ("POST", ["api", "cohorts"])
        | ("PUT", ["api", "cohorts", _])
        | ("POST", ["api", "cohorts", _, "members"])
        | ("POST", ["api", "ask", "handles", _, "promote"]) => (Need::One("data:work"), Plain),
        // record 26 §15: the identifier types and the held list are counts
        // and shapes; the map applied, the held reveal and the merge read
        // identifiers. The imports door answers a dry run at plain and
        // checks sensitive itself for the apply.
        ("GET", ["api", "linkage", "types" | "held"]) => (Need::One("data:see"), Plain),
        ("POST", ["api", "linkage", "types" | "imports"])
        | ("POST", ["api", "linkage", "held", "code"]) => (Need::One("data:work"), Plain),
        ("POST", ["api", "linkage", "held", "reveal"]) | ("POST", ["api", "linkage", "merge"]) => {
            (Need::One("data:work"), Detail::Sensitive)
        }
        // the Review page, and the knob engine of Wave 4c §6.6; record 26
        // §11: why a stack was judged so is a review reading
        ("GET", ["api", "review" | "overlays" | "quarantine"])
        | ("GET", ["api", "review" | "overlays" | "explain", _])
        | ("GET", ["api", "classify", "signals"]) => (Need::One("review:see"), Plain),
        ("POST", ["api", "review", _, "apply" | "accept"])
        | ("POST", ["api", "decisions", _, "commit" | "withdraw"])
        | ("POST", ["api", "picks"])
        | ("POST", ["api", "picks", _, "withdraw"])
        | ("POST", ["api", "classify", "try"])
        | ("POST", ["api", "overlays"]) => (Need::One("review:work"), Plain),
        // adopting a rule changes how data is sorted: work on both pages
        ("POST", ["api", "overlays", _, "adopt"]) => {
            (Need::Both("review:work", "data:work"), Plain)
        }
        // the Release page
        ("GET", ["api", "releases"]) | ("POST", ["api", "select"]) => {
            (Need::One("release:see"), Plain)
        }
        ("POST", ["api", "releases" | "handovers"]) => (Need::One("release:work"), Plain),
        // the Pipelines page; a queued verb needs its own grant and a cancel
        // the grant of the job's verb, both checked at the door itself
        ("GET", ["api", "jobs"]) | ("GET", ["api", "jobs", _]) => {
            (Need::One("pipelines:see"), Plain)
        }
        ("POST", ["api", "sessions", "rebuild"]) => (Need::One("pipelines:work"), Plain),
        // record 42 S4: what pipelines make; the bytes are drawn from the
        // pixels, so they open at detail quasi like the viewer's
        ("GET", ["api", "derivatives"]) | ("GET", ["api", "derivatives", _]) => {
            (Need::One("pipelines:see"), Plain)
        }
        ("GET", ["api", "derivatives", _, "content"]) => (Need::One("pipelines:see"), Quasi),
        // record 43: the catalog and the runs are the Pipelines page's
        ("GET", ["api", "pipelines" | "pipeline-runs"])
        | ("GET", ["api", "pipelines" | "pipeline-runs", _]) => (Need::One("pipelines:see"), Plain),
        // record 49 A3: the pre-flight reads and starts nothing
        ("POST", ["api", "pipelines", _, "preflight"]) => (Need::One("pipelines:see"), Plain),
        ("POST", ["api", "derivatives"]) => (Need::One("pipelines:work"), Plain),
        ("POST", ["api", "jobs"]) | ("POST", ["api", "jobs", _, "cancel"]) => {
            (Need::AnyOf(JOB_GRANTS), Plain)
        }
        // the settings pages
        ("POST", ["api", "places"]) | ("PUT", ["api", "places", _]) => {
            (Need::One("places:work"), Plain)
        }
        ("GET", ["api", "backups"]) => (Need::One("database:see"), Plain),
        ("PUT", ["api", "backups", "schedule"]) | ("PUT", ["api", "settings"]) => {
            (Need::One("database:work"), Plain)
        }
        ("GET", ["api", "audit" | "custody"]) => (Need::One("audit:see"), Plain),
        // record 42 R7: the model registry has grants of its own
        ("GET", ["api", "models"]) | ("GET", ["api", "models", _]) => {
            (Need::One("models:see"), Plain)
        }
        ("POST", ["api", "models"]) | ("POST", ["api", "models", _, _]) => {
            (Need::One("models:work"), Plain)
        }
        // record 42: the campaigns, the label sets and the commit by filter
        (m, s) if crate::campaigns::door(m, s).is_some() => {
            crate::campaigns::door(m, s).unwrap_or((Need::Any, Plain))
        }
        _ => (Need::Any, Plain),
    }
}

/// What a queued command needs by its verb: the grant, and the lowest
/// detail; none for an ask verb the door does not queue.
pub(crate) fn verb_needs(command: &[String]) -> Option<(&'static str, Detail)> {
    let verb = command.first().map(String::as_str).unwrap_or_default();
    Some(match (verb, command.get(1).map(String::as_str)) {
        ("digest", _) => ("data:work", Detail::Plain),
        // record 26 §3: the pseudonymiser reads the identifiers it replaces;
        // bring-in is checked by its first step at the door, and this is
        // what a cancel of it needs
        ("pseudonymize", _) => ("data:work", Detail::Sensitive),
        ("bring-in", _) => ("data:work", Detail::Plain),
        // record 26 §1: vaulting or purging a dataset's originals moves and
        // deletes the identified files themselves
        ("place", Some("originals")) => ("data:work", Detail::Sensitive),
        // a linkage import reads the identifiers it links
        ("linkage", _) => ("data:work", Detail::Sensitive),
        ("release" | "handover", _) => ("release:work", Detail::Plain),
        // record 26 §9: a promotion is a cohort act, which is Data work
        ("ask", Some("promote")) => ("data:work", Detail::Plain),
        ("ask", Some("run")) => ("query:work", Detail::Plain),
        // record 41: the vote matrix is a read that writes a file where it
        // is told, which no door queues
        ("classify", Some("votes")) => return None,
        ("fingerprint" | "classify" | "pick" | "session" | "pyramid", _) => {
            ("pipelines:work", Detail::Plain)
        }
        // record 43 S2: a pipeline reads pixels, which open at quasi
        ("run", _) => ("pipelines:work", Detail::Quasi),
        ("backup" | "verify", _) => ("database:work", Detail::Plain),
        _ => return None,
    })
}

/// The grant a cancel needs: the grant of the job's verb. A job a door
/// queued names its command line; one the command line claimed names its
/// verb as its kind, and a kind no door queues is a pipeline's.
fn cancel_needs(job: &nils_registry::job::Job) -> &'static str {
    if let Some((grant, _)) = job.queued().as_deref().and_then(verb_needs) {
        return grant;
    }
    match job.kind.as_str() {
        "digest" | "ingest" | "pseudonymize" | "bring-in" | "linkage" | "linkage-purge"
        | "clinical-import" | "originals" | "place" => "data:work",
        "release" | "handover" => "release:work",
        "backup" | "verify" => "database:work",
        "ask" if job.args["cohort"].is_string() => "data:work",
        "ask" => "query:work",
        _ => "pipelines:work",
    }
}

/// What a queued job records of its caller beside the principal: the detail
/// the verb runs under, never the worker's own, and who acted.
pub(crate) fn queued_by(caller: &Caller) -> serde_json::Value {
    // record 26 §7: the grants too, which a chain's later steps are
    // checked against when their turn comes
    let grants: Vec<&str> = caller.access.grants.iter().copied().collect();
    serde_json::json!({
        "detail": caller.access.detail.name(),
        "actor": caller.actor,
        "grants": grants,
    })
}

/// Record 26: the dataset fields a places body names, as the declaration
/// takes them, a null among them (no cohort, no rule) as much as a value;
/// `handling.arrives` stands for `arrives` for a caller from before, when
/// the body names no `arrives` of its own.
fn dataset_asked(doc: &serde_json::Value) -> serde_json::Value {
    let mut asked = serde_json::Map::new();
    for key in crate::dataset::FIELDS {
        if let Some(v) = doc.get(key) {
            asked.insert(key.to_string(), v.clone());
        }
    }
    if !asked.contains_key("arrives") && doc["handling"]["arrives"].is_string() {
        asked.insert("arrives".into(), doc["handling"]["arrives"].clone());
    }
    serde_json::Value::Object(asked)
}

/// Wave 4c §6.6: the scope a body names.
fn scope_of(doc: &serde_json::Value) -> Result<nils_classify::scope::Scope, Reply> {
    let text = doc["scope"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "scope: batch:<id>, origin:<name> or pack:<version>"))?;
    nils_classify::scope::Scope::parse(text).map_err(|e| Reply::error(400, e))
}

/// The author kind the caller acts as: the actor's kind when one was
/// declared, else a person at a keyboard. With the model version beside it.
pub(crate) fn author_of(caller: &Caller) -> (&str, Option<&str>) {
    // Anything else, `absent` included, is a person at a keyboard.
    let kind = match caller.actor["kind"].as_str() {
        Some("agent") => "agent",
        Some("model") => "model",
        _ => "person",
    };
    (kind, caller.actor["version"].as_str())
}

/// Record 42 S1: the author of a decision is the verified actor, never the
/// body. The kind (and a model's version) come from `X-Nils-Actor` as the
/// token allows it (`narrow`), a person at a keyboard when there is none. A
/// body from an older client may still say `author_kind`, `model_version`
/// and `model_id`; where it agrees with the actor it is taken, where it
/// says something else the call is refused rather than recorded under a
/// name the caller did not prove. Record 42 S2: a model acting names the
/// registered model in the header's `model`, by id, digest or
/// `name@version`, and one that no registered model answers to is refused.
fn author_at_apply<'a>(
    registry: &mut Registry,
    caller: &'a Caller,
    doc: &'a serde_json::Value,
) -> Result<(&'a str, Option<&'a str>, Option<i64>), Reply> {
    let (kind, version) = author_of(caller);
    if let Some(said) = doc.get("author_kind").filter(|v| !v.is_null())
        && said.as_str() != Some(kind)
    {
        return Err(Reply::error(
            403,
            format!(
                "this call acts as {}, as X-Nils-Actor and the token say; a body that says author_kind {said} is refused, because the author is the verified actor and never the body",
                with_article(kind)
            ),
        ));
    }
    let model = acting_model(registry, caller)?;
    if let Some(said) = doc.get("model_id").filter(|v| !v.is_null())
        && (said.as_i64() != model.as_ref().map(|m| m.id))
    {
        return Err(Reply::error(
            403,
            format!(
                "model_id {said} is not the model X-Nils-Actor names; the model is the actor's, never the body's"
            ),
        ));
    }
    if let Some(said) = doc.get("model_version").filter(|v| !v.is_null()) {
        let held = model.as_ref().map(|m| m.version.as_str()).or(version);
        if kind != "model" || said.as_str() != held {
            return Err(Reply::error(
                403,
                format!(
                    "model_version {said} is not the acting model's ({}); a model's version is the actor's, never the body's",
                    held.unwrap_or("none")
                ),
            ));
        }
    }
    Ok((kind, version, model.map(|m| m.id)))
}

/// Record 42 S2: the registered model a model acting names in
/// `X-Nils-Actor` (by id, digest or `name@version`); none when a person or
/// an agent acts. A model that names none, or one nothing registered
/// answers to, is refused.
pub(crate) fn acting_model(
    registry: &mut Registry,
    caller: &Caller,
) -> Result<Option<nils_registry::model::Model>, Reply> {
    if author_of(caller).0 != "model" {
        return Ok(None);
    }
    let reference = match &caller.actor["model"] {
        serde_json::Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => {
            return Err(Reply::error(
                400,
                "a model acting names the registered model in X-Nils-Actor: {\"kind\": \"model\", \"model\": <id, sha256 digest or name@version>}",
            ));
        }
    };
    let Some(m) = nils_registry::model::resolve(registry.store(), &reference)? else {
        return Err(Reply::error(
            404,
            format!("no registered model answers to {reference}; nils model list"),
        ));
    };
    Ok(Some(m))
}

/// An author kind as a sentence names it.
fn with_article(kind: &str) -> String {
    match kind {
        "agent" => "an agent".to_string(),
        "absent" | "" => "nobody in particular".to_string(),
        other => format!("a {other}"),
    }
}

/// Wave 4c §6.6: an overlay from a body, rehearsed over a scope. The pack
/// it amends is loaded bare and amended; the overlay's own cases are judged
/// and their failure is part of the answer, not a refusal.
/// The pack as the registry was classified under it: with the overlay the
/// site adopted for it last, because adopting an overlay reclassifies under
/// that overlay (`classify --overlay-id`). Without one, or where the adopted
/// overlay no longer loads on the pack served, the pack as it is; the
/// answer names the overlay it read, so the two are told apart.
fn with_adopted_overlay(
    registry: &mut Registry,
    dir: &std::path::Path,
    pack: nils_pack::Pack,
) -> Result<nils_pack::Pack, Reply> {
    let last = nils_registry::overlay::list(registry.store())?
        .into_iter()
        .filter(|o| o.status == nils_registry::overlay::ADOPTED && o.pack == pack.name)
        .max_by(|a, b| {
            a.decided_at
                .cmp(&b.decided_at)
                .then_with(|| a.id.cmp(&b.id))
        });
    let Some(row) = last else {
        return Ok(pack);
    };
    let adopted =
        nils_pack::Overlay::parse(&format!("overlay {}", row.id), &row.document.to_string())
            .ok()
            .and_then(|o| nils_pack::load(dir, Some(&o)).ok());
    Ok(adopted.unwrap_or(pack))
}

fn rehearsed(
    doors: &Doors,
    registry: &mut Registry,
    overlay: &serde_json::Value,
    scope: &nils_classify::scope::Scope,
    sample: usize,
) -> Result<(nils_pack::Overlay, serde_json::Value), Reply> {
    if !overlay.is_object() {
        return Err(Reply::error(
            400,
            "overlay: the overlay document, as an object",
        ));
    }
    let o = nils_pack::Overlay::parse("overlay", &overlay.to_string())
        .map_err(|e| Reply::error(400, e.to_string()))?;
    let dir = doors
        .pack_dir
        .as_ref()
        .map(|d| d.join(&o.pack))
        .filter(|d| d.join("pack.yml").is_file())
        .ok_or_else(|| {
            Reply::error(
                400,
                format!(
                    "the overlay amends {}, which this engine does not serve",
                    o.pack
                ),
            )
        })?;
    let (before, _) =
        nils_pack::load_judged(&dir, None).map_err(|e| Reply::error(500, e.to_string()))?;
    let (after, failure) =
        nils_pack::load_judged(&dir, Some(&o)).map_err(|e| Reply::error(400, e.to_string()))?;
    let assertions: usize = o
        .cases
        .iter()
        .map(|(_, c)| c.flags.len() + c.axes.len())
        .sum();
    let tried = nils_classify::rehearse::run(
        registry.store(),
        &before,
        &after,
        scope,
        sample,
        None,
        (assertions, failure.map(|e| e.to_string())),
    )?;
    Ok((o, tried))
}

pub(crate) fn job_err(e: nils_registry::job::Error) -> Reply {
    match e {
        nils_registry::job::Error::Busy { .. } => Reply::error(409, e.to_string()),
        other => Reply::error(500, other.to_string()),
    }
}

/// A cohort refusal: a name that is taken is 409, a code the registry does
/// not hold is 400 with the codes beside the text, and no cohort is 404.
fn cohort_err(e: nils_registry::cohort::Error) -> Reply {
    use nils_registry::cohort::Error;
    match &e {
        Error::NotFound(n) => Reply::error(404, format!("no cohort named {n}")),
        Error::Taken(_) => Reply::error(409, e.to_string()),
        Error::Unknown(codes) => {
            let mut r = Reply::error(400, e.to_string());
            r.body["unknown"] = serde_json::json!(codes);
            r
        }
        Error::Message(m) => Reply::error(400, m.clone()),
        Error::Store(e) => Reply::error(500, e.to_string()),
    }
}

fn model_err(e: nils_registry::model::Error) -> Reply {
    use nils_registry::model::Error;
    match e {
        Error::Invalid(m) => Reply::error(400, m),
        Error::Unknown(m) => Reply::error(404, m),
        Error::Refused(m) => Reply::error(409, m),
        Error::Store(e) => Reply::error(500, e.to_string()),
    }
}

fn pick_err(e: nils_classify::picking::PersonError) -> Reply {
    match e {
        nils_classify::picking::PersonError::Refused(m) => Reply::error(409, m),
        nils_classify::picking::PersonError::Store(e) => Reply::error(500, e.to_string()),
    }
}

/// A review refusal quotes what it refuses: a reference, a header value, a
/// name, the person who decided; every one is gated.
fn review_err(e: nils_registry::review::Error) -> Reply {
    match e {
        nils_registry::review::Error::Refused(m) => Reply::gated(409, m),
        nils_registry::review::Error::Forbidden(m) => Reply::error(403, m),
        other => Reply::error(500, other.to_string()),
    }
}

fn capabilities(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    ask: &mut crate::ask_doors::AskState,
) -> serde_json::Value {
    let meta = registry.meta().clone();
    let packs: Vec<serde_json::Value> = doors
        .pack_dir
        .as_ref()
        .and_then(|dir| crate::packs_in(dir).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| nils_pack::load(&p, None).ok())
        .map(|p| serde_json::json!({ "name": p.name, "version": p.version.to_string(), "contract": p.contract }))
        .collect();
    let doors_list: Vec<String> = [
        "GET /api/capabilities",
        "GET /api/status",
        "GET /api/summary",
        "GET /api/custody",
        "GET /api/audit",
        "GET /api/jobs",
        "POST /api/jobs",
        "GET /api/jobs/{id}",
        "POST /api/jobs/{id}/cancel",
        "GET /api/releases",
        "POST /api/releases",
        "POST /api/handovers",
        "POST /api/select",
        "GET /api/review",
        "GET /api/review/{id}",
        "POST /api/review/{id}/apply",
        "POST /api/review/{id}/accept",
        "POST /api/decisions/{id}/commit",
        "POST /api/decisions/{id}/withdraw",
        "GET /api/models",
        "POST /api/models",
        "GET /api/models/{id}",
        "POST /api/models/{id}/admit",
        "POST /api/models/{id}/promote",
        "POST /api/models/{id}/retire",
        "POST /api/picks",
        "POST /api/picks/{id}/withdraw",
        "GET /api/timeline/{kind}/{id}",
        "GET /api/depends/{kind}/{id}",
        "GET /api/events",
        "GET /api/packs",
        "GET /api/packs/{name}",
        "GET /api/batches",
        "GET /api/batches/{id}",
        "GET /api/quarantine",
        "GET /api/cohorts",
        "GET /api/cohorts/{name}",
        "POST /api/cohorts",
        "PUT /api/cohorts/{name}",
        "POST /api/cohorts/{name}/members",
        "GET /api/review/summary",
        "GET /api/explain/{stack}",
        "GET /api/classify/signals",
        "POST /api/classify/try",
        "GET /api/overlays",
        "POST /api/overlays",
        "GET /api/overlays/{id}",
        "POST /api/overlays/{id}/adopt",
        "POST /api/ingest/probe",
        "POST /api/ingest/folders",
        "POST /api/ingest/look",
        "GET /api/sources",
        "GET /api/pseudonymize/tags",
        "GET /api/places",
        "POST /api/places",
        "PUT /api/places/{id}",
        "GET /api/places/{id}/originals",
        "POST /api/places/{id}/originals",
        "GET /api/backups",
        "PUT /api/backups/schedule",
        "GET /api/settings",
        "PUT /api/settings",
        "GET /api/instances/{stack}/manifest",
        "GET /api/instances/{stack}/tiles/{level}/{z}",
        "GET /api/instances/{stack}/slab/{level}/{z0}-{z1}",
        "GET /api/instances/{stack}/render/{level}/{z}",
    ]
    .iter()
    .chain(crate::derivatives::DOORS.iter())
    .chain(crate::pipelines::DOORS.iter())
    .chain(std::iter::once(&crate::preflight::DOOR))
    .chain(crate::linkage_doors::DOORS.iter())
    .chain(crate::campaigns::DOORS.iter())
    .chain(crate::ask_doors::DOORS.iter())
    .map(|d| (*d).to_string())
    .collect();
    serde_json::json!({
        "engine": { "name": "nils", "version": env!("CARGO_PKG_VERSION") },
        "contracts": {
            "openapi": OPENAPI_VERSION.trim(),
            "review_item": REVIEW_ITEM_VERSION.trim(),
            "suite": SUITE_VERSION.trim(),
            "mcp": MCP_VERSION.trim(),
            "pack": PACK_CONTRACT_VERSION.trim(),
            "model": MODEL_CONTRACT_VERSION.trim(),
        },
        "packs": packs,
        "registry": { "id": meta.registry_id, "epoch": meta.epoch, "schema_version": meta.schema_version, "synthetic": meta.synthetic },
        "auth": doors.auth.name(),
        "principal": caller.principal,
        "grants": caller.access.list(),
        "detail": caller.access.detail.name(),
        // for one release: the ladder steps up to the caller's detail
        "roles": caller.access.steps(),
        "display": caller.display,
        "email": caller.email,
        "ceiling": caller.ceiling.map(Step::name),
        "actor": caller.actor,
        "node": doors.node,
        "uptime_seconds": doors.started.elapsed().as_secs(),
        "ask": crate::ask_doors::capabilities(doors, registry, ask),
        "mcp": {
            "path": crate::mcp::PATH,
            "protocol": crate::mcp::PROTOCOL,
            "metadata": "/.well-known/oauth-protected-resource",
            "tools": ask.model(doors, registry).map(|m| m.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "content_version": ask.model(doors, registry).map(|m| m.version).unwrap_or_default(),
        },
        "doors": doors_list,
        "assist": doors.assist.as_ref().map(|u| serde_json::json!({ "url": u })),
        "supervisor": doors.supervisor.as_ref().map(|u| serde_json::json!({ "url": u })),
        "event_streams": doors.event_streams,
        "ingest_roots": doors.ingest_roots.keys().collect::<Vec<_>>(),
        "backup_dir": doors.backup_dir.is_some(),
        "places": crate::places::capabilities(registry.store()),
        "derivatives": crate::derivatives::capability(registry.store()),
        "pipelines": crate::pipelines::capability(registry),
        "policy": policy(),
        "idempotency": {
            "header": "Idempotency-Key",
            "doors": crate::ask_doors::IDEMPOTENT_DOORS,
            "hours": nils_registry::idempotency::KEEP_HOURS,
        },
    })
}

/// Record 48 R2: review items about a stack the caller reads blind say
/// nothing a system said of it: a stack's item keeps its axis and loses its
/// evidence, and a group's hidden members lose theirs, the group's own
/// evidence going with them.
fn blind_review(
    store: &mut nils_registry::Store,
    caller: &Caller,
    rows: &mut [serde_json::Value],
) -> Result<(), Reply> {
    let mut about: Vec<(usize, Vec<i64>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mut stacks: Vec<i64> = r["ref"]["stack_id"].as_i64().into_iter().collect();
        if r["scope"] == "group"
            && let Some(id) = r["id"].as_i64()
        {
            let sql = format!(
                "SELECT stack_id FROM {} WHERE item_id = {}",
                store.qualified("review_member"),
                store.dialect().param(1, nils_registry::schema::Type::Int)
            );
            for m in store.query(&sql, &[nils_registry::Param::Int(id)])? {
                stacks.push(m.int(0)?);
            }
        }
        if !stacks.is_empty() {
            about.push((i, stacks));
        }
    }
    let all: Vec<i64> = about.iter().flat_map(|(_, s)| s.iter().copied()).collect();
    let hidden = crate::campaigns::blind_among(store, caller, &all)?;
    if hidden.is_empty() {
        return Ok(());
    }
    for (i, stacks) in about {
        if !stacks.iter().any(|s| hidden.contains(s)) {
            continue;
        }
        let r = &mut rows[i];
        let axis = r["evidence"]["axis"].clone();
        r["evidence"] = serde_json::json!({"axis": axis, "blind": true});
        if r.get("decision").is_some() {
            r["decision"] = serde_json::Value::Null;
        }
        r["blind"] = serde_json::json!(true);
        for m in r["member_stacks"].as_array_mut().into_iter().flatten() {
            if m["stack_id"].as_i64().is_some_and(|s| hidden.contains(&s)) {
                m["evidence"] = serde_json::json!({"blind": true});
            }
        }
    }
    Ok(())
}

fn review_list(
    store: &mut Store,
    status: Option<&str>,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, Reply> {
    use nils_registry::schema::Type;
    let d = store.dialect();
    let t = nils_registry::schema::table("review_item");
    let text = |c: &str| d.text_of(t.column(c).expect("review_item column"));
    let mut sql = format!(
        "SELECT id, kind, scope, status, actor, {}, {}, {}, {}, {}, members, group_key, accepted_by FROM {} WHERE 1 = 1",
        text("created_at"),
        text("decided_at"),
        text("ref"),
        text("evidence"),
        text("decision"),
        store.qualified("review_item")
    );
    let mut params = Vec::new();
    if let Some(st) = status {
        params.push(nils_registry::Param::from(st));
        sql.push_str(&format!(
            " AND status = {}",
            d.param(params.len(), Type::Text)
        ));
    }
    if let Some(k) = kind {
        params.push(nils_registry::Param::from(k));
        sql.push_str(&format!(
            " AND kind = {}",
            d.param(params.len(), Type::Text)
        ));
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", limit.max(1)));
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            Ok(serde_json::json!({
                "id": r.int(0)?, "kind": r.text(1)?, "scope": r.text(2)?, "status": r.text(3)?,
                "actor": r.opt_text(4)?, "created_at": r.opt_text(5)?, "decided_at": r.opt_text(6)?,
                "ref": json(r.opt_text(7)?), "evidence": json(r.opt_text(8)?), "decision": json(r.opt_text(9)?),
                "members": r.opt_int(10)?, "group_key": r.opt_text(11)?, "accepted_by": r.opt_text(12)?,
            }))
        })
        .collect::<Result<Vec<_>, nils_registry::Error>>()
        .map_err(Reply::from)
}

/// `GET /api/events`: server-sent events with the open jobs, every second,
/// until the client goes away. Display plumbing only. The queue's worker is
/// left out unless `?all` asks for it, as `GET /api/jobs` leaves it out.
fn events(doors: &Doors, registry: &mut Registry, request: Request, all: bool) {
    let caller = match doors.auth.caller(&request) {
        Ok(c) => c,
        Err(reply) => {
            let _ = respond(request, reply);
            return;
        }
    };
    // Wave 4c §6.1: a door like any other, so it needs a grant; and capped
    // well below the worker count, because an open stream pins a worker for
    // its life and four tabs must not wedge every door.
    if let Err(refused) = caller.allowed("/api/events", Need::Any, Detail::Plain) {
        let _ = respond(request, refused);
        return;
    }
    if doors.streams_open.fetch_add(1, Ordering::SeqCst) >= doors.event_streams {
        doors.streams_open.fetch_sub(1, Ordering::SeqCst);
        let _ = respond(
            request,
            Reply::error(
                503,
                format!(
                    "event_streams_full: {} event streams are open, which is the cap; poll GET /api/jobs instead",
                    doors.event_streams
                ),
            ),
        );
        return;
    }
    struct Open<'a>(&'a AtomicUsize);
    impl Drop for Open<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _open = Open(&doors.streams_open);
    let mut writer = request.into_writer();
    let _ = writer.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n");
    let _ = writer.write_all(b"event: hello\ndata: {}\n\n");
    let once = std::env::var("NILS_EVENTS_ONCE").is_ok();
    loop {
        let jobs: Vec<_> = nils_registry::job::list(registry.store(), false, 50)
            .unwrap_or_default()
            .into_iter()
            .filter(|j| all || !nils_registry::job::is_worker(&j.kind))
            .collect();
        let mut docs: Vec<_> = jobs.iter().map(nils_registry::job::Job::as_json).collect();
        if caller.access.detail < Detail::Quasi {
            docs.iter_mut().for_each(crate::pipelines::job_totals_only);
        }
        let data = serde_json::json!({
            "epoch": registry.meta().epoch,
            "jobs": docs,
        });
        if writer
            .write_all(format!("event: jobs\ndata: {data}\n\n").as_bytes())
            .and_then(|_| writer.flush())
            .is_err()
            || once
        {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Wave 4c §6.5: the paths a queued command may name. `@name/relative`
/// resolves against a registered ingest root and may not escape it; an
/// absolute path, or one with a parent step, is refused. `backup` takes
/// the deployment's directory; `verify NAME` checks one archive in it.
fn located(doors: &Doors, store: &mut Store, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let verb = command[0].as_str();
    match verb {
        "backup" => {
            let dir = doors.backup_dir.as_ref().ok_or_else(|| {
                Reply::error(
                    409,
                    "no backup directory: start nils serve with --backup-dir",
                )
            })?;
            // Wave 5 §10.3: how many archives to keep, and a rehearsal; nothing
            // else a caller composes
            let mut out = vec![
                "backup".to_string(),
                "--dir".to_string(),
                dir.display().to_string(),
            ];
            let mut rest = command.iter().skip(1);
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--rehearse" => out.push(arg.clone()),
                    "--keep" => {
                        let keep = rest
                            .next()
                            .and_then(|n| n.parse::<usize>().ok())
                            .filter(|n| (1..=1000).contains(n))
                            .ok_or_else(|| {
                                Reply::error(
                                    400,
                                    "backup --keep N: how many archives to keep, 1 to 1000",
                                )
                            })?;
                        out.extend(["--keep".to_string(), keep.to_string()]);
                    }
                    other => {
                        return Err(Reply::error(
                            400,
                            format!("backup takes --keep N and --rehearse, not {other}"),
                        ));
                    }
                }
            }
            return Ok(out);
        }
        "verify" => {
            let dir = doors.backup_dir.as_ref().ok_or_else(|| {
                Reply::error(
                    409,
                    "no backup directory: start nils serve with --backup-dir",
                )
            })?;
            let name = command.get(1).ok_or_else(|| {
                Reply::error(400, "verify NAME: an archive in the backup directory")
            })?;
            if name.contains('/') || name.contains("..") || name.is_empty() {
                return Err(Reply::error(400, "verify NAME: a name, not a path"));
            }
            let mut out = vec!["verify".to_string(), dir.join(name).display().to_string()];
            for arg in command.iter().skip(2) {
                if arg != "--rehearse" {
                    return Err(Reply::error(
                        400,
                        format!("verify NAME takes --rehearse, not {arg}"),
                    ));
                }
                out.push(arg.clone());
            }
            return Ok(out);
        }
        // record 43 S2: a run names a pipeline and a frozen selection, and
        // the deployment's packs; never a path
        "run" => return crate::pipelines::located(doors.pack_dir.as_deref(), command),
        // record 45 E1: a pyramid names a stack, a selection or a handle, and
        // the deployment's packs where a selection is frozen; never a path
        "pyramid" => return crate::pyramid::located(doors.pack_dir.as_deref(), command),
        _ => {}
    }
    let takes_a_tree =
        verb == "digest" || (verb == "linkage" && command.get(1).is_some_and(|c| c == "import"));
    // record 26: `@name` is the dataset's pseudonymised tree, and its
    // originals, `@name/originals`, are the pseudonymiser's alone
    let roots = crate::dataset::roots(store, &doors.ingest_roots);
    let digests = verb == "digest";
    // record 26 §3 and §1: the pseudonymiser reads a dataset by its name,
    // both trees at once, and an act on a dataset's originals names it the
    // same way, so `@name` stays a name for them
    let by_name = verb == "pseudonymize"
        || (verb == "place" && command.get(1).is_some_and(|c| c == "originals"));
    let named = match command.get(1) {
        Some(second) if verb == "place" => format!("{verb} {second}"),
        _ => verb.to_string(),
    };
    let mut out = Vec::with_capacity(command.len());
    for arg in command {
        if let Some(rest) = arg.strip_prefix('@') {
            let (name, rel) = rest.split_once('/').unwrap_or((rest, ""));
            let root = roots.get(name).ok_or_else(|| {
                Reply::error(
                    400,
                    format!(
                        "@{name} is not a registered ingest location; those are {}",
                        roots.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
            })?;
            if by_name {
                if root.place.is_none() || !rel.is_empty() {
                    return Err(Reply::error(
                        400,
                        format!("{named} takes a dataset as @name; @{name} is not one"),
                    ));
                }
                out.push(arg);
                continue;
            }
            if rel.split('/').any(|seg| seg == "..") || rel.starts_with('/') {
                return Err(Reply::error(
                    400,
                    format!("@{name}/{rel} steps outside its location"),
                ));
            }
            let path = root.resolve(rel);
            if digests && let Some(p) = crate::dataset::originals_holding(store, &path) {
                return Err(Reply::error(
                    409,
                    format!(
                        "@{name}/{rel} is in the originals of the dataset {}, which the pseudonymiser alone reads; a digest reads @{name} (record 26)",
                        p.name
                    ),
                ));
            }
            out.push(path.display().to_string());
        } else if takes_a_tree
            && !arg.starts_with('-')
            && (arg.starts_with('/') || arg.contains(".."))
        {
            return Err(Reply::error(
                400,
                format!(
                    "{arg}: a path a caller composes is refused; name a registered ingest location as @name/relative"
                ),
            ));
        } else {
            out.push(arg);
        }
    }
    Ok(out)
}

/// Wave 4c §6.5: one row per door: the grant it needs, whether it writes,
/// whether it takes an idempotency key, its cost class, its result cap and
/// a human label in the present and the past tense. The desk's controls,
/// the MCP door's gating and the audit line derive from this table. The
/// grant and the detail are read from the door table itself, so the policy
/// and the doors cannot disagree on a door.
pub(crate) fn policy() -> Vec<serde_json::Value> {
    let row =
        |name: &str, writes: bool, idem: bool, cost: &str, cap: &str, now: &str, then: &str| {
            let (method, path) = name.split_once(' ').unwrap_or(("GET", name));
            let (need, detail) = door(method, &segments(path));
            let mut r = serde_json::json!({
                "door": name, "grant": need.grant(), "writes": writes, "idempotent": idem,
                "cost": cost, "result_cap": cap, "label": {"present": now, "past": then},
            });
            if let Some(also) = need.also() {
                r["also"] = serde_json::Value::from(also);
            }
            if detail > Detail::Plain {
                r["detail"] = serde_json::Value::from(detail.name());
            }
            // record 26: the dataset fields of a place need data:work
            // beside places:work, checked at the door itself
            if matches!(name, "POST /api/places" | "PUT /api/places/{id}") {
                r["dataset"] = serde_json::Value::from("data:work");
            }
            r
        };
    vec![
        row(
            "GET /api/capabilities",
            false,
            false,
            "free",
            "one document",
            "Reading what the engine speaks",
            "Read what the engine speaks",
        ),
        row(
            "GET /api/status",
            false,
            false,
            "bounded",
            "one document",
            "Reading the status",
            "Read the status",
        ),
        row(
            "GET /api/custody",
            false,
            false,
            "bounded",
            "one document",
            "Reading custody",
            "Read custody",
        ),
        row(
            "GET /api/audit",
            false,
            false,
            "bounded",
            "limit rows",
            "Reading the audit log",
            "Read the audit log",
        ),
        row(
            "GET /api/jobs",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing jobs",
            "Listed jobs",
        ),
        row(
            "POST /api/jobs",
            true,
            false,
            "job",
            "one id",
            "Queuing a job",
            "Queued a job",
        ),
        row(
            "GET /api/jobs/{id}",
            false,
            false,
            "free",
            "one document",
            "Reading a job",
            "Read a job",
        ),
        row(
            "POST /api/jobs/{id}/cancel",
            true,
            true,
            "free",
            "one state",
            "Cancelling a job",
            "Cancelled a job",
        ),
        row(
            "GET /api/releases",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing releases",
            "Listed releases",
        ),
        row(
            "POST /api/releases",
            true,
            false,
            "job",
            "one id",
            "Cutting a release",
            "Cut a release",
        ),
        row(
            "POST /api/handovers",
            true,
            false,
            "job",
            "one id",
            "Handing over",
            "Handed over",
        ),
        row(
            "POST /api/select",
            false,
            false,
            "bounded",
            "sync_max_rows",
            "Previewing a selection",
            "Previewed a selection",
        ),
        row(
            "GET /api/review",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing the review queue",
            "Listed the review queue",
        ),
        row(
            "GET /api/review/{id}",
            false,
            false,
            "free",
            "one item",
            "Reading a review item",
            "Read a review item",
        ),
        row(
            "POST /api/review/{id}/apply",
            true,
            false,
            "free",
            "one decision",
            "Deciding",
            "Decided",
        ),
        row(
            "POST /api/review/{id}/accept",
            true,
            true,
            "free",
            "one item",
            "Acknowledging",
            "Acknowledged",
        ),
        row(
            "POST /api/decisions/{id}/commit",
            true,
            true,
            "free",
            "one decision",
            "Committing a decision",
            "Committed a decision",
        ),
        row(
            "POST /api/decisions/{id}/withdraw",
            true,
            true,
            "free",
            "one decision",
            "Withdrawing a decision",
            "Withdrew a decision",
        ),
        row(
            "GET /api/models",
            false,
            false,
            "bounded",
            "every model",
            "Listing the models",
            "Listed the models",
        ),
        row(
            "POST /api/models",
            true,
            false,
            "free",
            "one model",
            "Registering a model",
            "Registered a model",
        ),
        row(
            "GET /api/models/{id}",
            false,
            false,
            "free",
            "one model",
            "Reading a model's card",
            "Read a model's card",
        ),
        row(
            "POST /api/models/{id}/admit",
            true,
            false,
            "free",
            "one model",
            "Recording a check on a model",
            "Recorded a check on a model",
        ),
        row(
            "POST /api/models/{id}/promote",
            true,
            false,
            "free",
            "one model",
            "Promoting a model",
            "Promoted a model",
        ),
        row(
            "POST /api/models/{id}/retire",
            true,
            false,
            "free",
            "one model",
            "Retiring a model",
            "Retired a model",
        ),
        row(
            "POST /api/picks",
            true,
            false,
            "free",
            "one pick",
            "Picking a stack",
            "Picked a stack",
        ),
        row(
            "POST /api/picks/{id}/withdraw",
            true,
            false,
            "free",
            "one pick",
            "Withdrawing a pick",
            "Withdrew a pick",
        ),
        row(
            "GET /api/derivatives",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing derivatives",
            "Listed derivatives",
        ),
        row(
            "POST /api/derivatives",
            true,
            false,
            "bounded",
            "one derivative",
            "Registering a derivative",
            "Registered a derivative",
        ),
        row(
            "GET /api/derivatives/{id}",
            false,
            false,
            "free",
            "one derivative",
            "Reading a derivative",
            "Read a derivative",
        ),
        row(
            "GET /api/derivatives/{id}/content",
            false,
            false,
            "bounded",
            "one file",
            "Downloading a derivative",
            "Downloaded a derivative",
        ),
        // record 43: the pipeline catalog and its runs
        row(
            "GET /api/pipelines",
            false,
            false,
            "bounded",
            "the catalog",
            "Listing pipelines",
            "Listed pipelines",
        ),
        row(
            "GET /api/pipelines/{id}",
            false,
            false,
            "free",
            "one pipeline",
            "Reading a pipeline",
            "Read a pipeline",
        ),
        row(
            "GET /api/pipeline-runs",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing pipeline runs",
            "Listed pipeline runs",
        ),
        row(
            "GET /api/pipeline-runs/{id}",
            false,
            false,
            "free",
            "one run",
            "Reading a pipeline run",
            "Read a pipeline run",
        ),
        row(
            "GET /api/events",
            false,
            false,
            "stream",
            "event_streams",
            "Watching jobs",
            "Watched jobs",
        ),
        row(
            "GET /api/packs",
            false,
            false,
            "free",
            "one list",
            "Listing packs",
            "Listed packs",
        ),
        row(
            "GET /api/packs/{name}",
            false,
            false,
            "free",
            "one document",
            "Reading a pack",
            "Read a pack",
        ),
        row(
            "GET /api/batches",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing batches",
            "Listed batches",
        ),
        row(
            "GET /api/batches/{id}",
            false,
            false,
            "free",
            "one report",
            "Reading a batch",
            "Read a batch",
        ),
        row(
            "GET /api/quarantine",
            false,
            false,
            "bounded",
            "every file",
            "Listing quarantine",
            "Listed quarantine",
        ),
        row(
            "GET /api/cohorts",
            false,
            false,
            "bounded",
            "every cohort",
            "Listing cohorts",
            "Listed cohorts",
        ),
        row(
            "GET /api/cohorts/{name}",
            false,
            false,
            "bounded",
            "one cohort",
            "Reading a cohort",
            "Read a cohort",
        ),
        row(
            "POST /api/cohorts",
            true,
            false,
            "free",
            "one cohort",
            "Making a cohort",
            "Made a cohort",
        ),
        row(
            "PUT /api/cohorts/{name}",
            true,
            false,
            "free",
            "one cohort",
            "Changing a cohort",
            "Changed a cohort",
        ),
        row(
            "POST /api/cohorts/{name}/members",
            true,
            false,
            "bounded",
            "the counts",
            "Changing a cohort's members",
            "Changed a cohort's members",
        ),
        row(
            "GET /api/review/summary",
            false,
            false,
            "bounded",
            "one document",
            "Reading the review summary",
            "Read the review summary",
        ),
        row(
            "GET /api/explain/{stack}",
            false,
            false,
            "free",
            "one document",
            "Reading why a stack was judged so",
            "Read why a stack was judged so",
        ),
        row(
            "GET /api/classify/signals",
            false,
            false,
            "bounded",
            "one document",
            "Reading the classifier's signals",
            "Read the classifier's signals",
        ),
        row(
            "POST /api/classify/try",
            false,
            false,
            "bounded",
            "one manifest",
            "Rehearsing an overlay",
            "Rehearsed an overlay",
        ),
        row(
            "GET /api/instances/{stack}/manifest",
            false,
            false,
            "free",
            "one document",
            "Reading a stack's pyramid manifest",
            "Read a stack's pyramid manifest",
        ),
        row(
            "GET /api/instances/{stack}/tiles/{level}/{z}",
            false,
            false,
            "bounded",
            "one plane of tiles",
            "Reading a plane of a stack",
            "Read a plane of a stack",
        ),
        row(
            "GET /api/instances/{stack}/slab/{level}/{z0}-{z1}",
            false,
            false,
            "bounded",
            "thirty-two planes of tiles",
            "Reading a slab of a stack",
            "Read a slab of a stack",
        ),
        row(
            "GET /api/instances/{stack}/render/{level}/{z}",
            false,
            false,
            "bounded",
            "one image",
            "Rendering a plane of a stack",
            "Rendered a plane of a stack",
        ),
        row(
            "GET /api/places",
            false,
            false,
            "bounded",
            "every place",
            "Reading the places",
            "Read the places",
        ),
        row(
            "GET /api/sources",
            false,
            false,
            "bounded",
            "one document",
            "Reading the sources",
            "Read the sources",
        ),
        row(
            "POST /api/places",
            true,
            false,
            "bounded",
            "one place",
            "Declaring a place",
            "Declared a place",
        ),
        row(
            "PUT /api/places/{id}",
            true,
            true,
            "bounded",
            "one place",
            "Changing a place",
            "Changed a place",
        ),
        row(
            "GET /api/places/{id}/originals",
            false,
            false,
            "bounded",
            "one document",
            "Reading what an act on the originals would do",
            "Read what an act on the originals would do",
        ),
        row(
            "POST /api/places/{id}/originals",
            true,
            false,
            "job",
            "one job",
            "Acting on a dataset's originals",
            "Acted on a dataset's originals",
        ),
        row(
            "GET /api/linkage/types",
            false,
            false,
            "bounded",
            "every type",
            "Reading the identifier types",
            "Read the identifier types",
        ),
        row(
            "POST /api/linkage/types",
            true,
            true,
            "bounded",
            "one type",
            "Adding an identifier type",
            "Added an identifier type",
        ),
        row(
            "POST /api/linkage/imports",
            true,
            false,
            "job",
            "one report",
            "Providing an identifier map",
            "Provided an identifier map",
        ),
        row(
            "GET /api/linkage/held",
            false,
            false,
            "bounded",
            "one dataset's shapes",
            "Reading the held files",
            "Read the held files",
        ),
        row(
            "POST /api/linkage/held/code",
            true,
            true,
            "bounded",
            "one count",
            "Coding the held files anyway",
            "Coded the held files anyway",
        ),
        row(
            "POST /api/linkage/held/reveal",
            true,
            false,
            "bounded",
            "one dataset's identifiers",
            "Revealing the held identifiers",
            "Revealed the held identifiers",
        ),
        row(
            "POST /api/linkage/merge",
            true,
            false,
            "job",
            "one id",
            "Merging two subjects",
            "Merged two subjects",
        ),
        row(
            "GET /api/backups",
            false,
            false,
            "bounded",
            "every archive in the backup directory",
            "Reading the backups",
            "Read the backups",
        ),
        row(
            "PUT /api/backups/schedule",
            true,
            true,
            "free",
            "one schedule",
            "Setting the backup schedule",
            "Set the backup schedule",
        ),
        row(
            "GET /api/settings",
            false,
            false,
            "free",
            "one document",
            "Reading the registry's calendar",
            "Read the registry's calendar",
        ),
        row(
            "PUT /api/settings",
            true,
            true,
            "free",
            "one document",
            "Changing the registry's calendar",
            "Changed the registry's calendar",
        ),
        row(
            "GET /api/overlays",
            false,
            false,
            "free",
            "every overlay",
            "Listing overlays",
            "Listed overlays",
        ),
        row(
            "POST /api/overlays",
            true,
            false,
            "bounded",
            "one overlay",
            "Proposing an overlay",
            "Proposed an overlay",
        ),
        row(
            "GET /api/overlays/{id}",
            false,
            false,
            "free",
            "one overlay",
            "Reading an overlay",
            "Read an overlay",
        ),
        row(
            "POST /api/overlays/{id}/adopt",
            true,
            false,
            "job",
            "one job",
            "Adopting an overlay",
            "Adopted an overlay",
        ),
        row(
            "POST /api/ingest/probe",
            false,
            false,
            "job",
            "one job",
            "Probing identity rules",
            "Probed identity rules",
        ),
        row(
            "POST /api/ingest/folders",
            false,
            false,
            "bounded",
            "a page of at most 1,000 folders, read for at most five seconds",
            "Listing the folders of an ingest location",
            "Listed the folders of an ingest location",
        ),
        row(
            "POST /api/ingest/look",
            false,
            false,
            "bounded",
            "sixteen files sniffed a folder, for at most 64 folders within the budget",
            "Looking inside folders",
            "Looked inside folders",
        ),
        row(
            "GET /api/ask/schema",
            false,
            false,
            "free",
            "one schema",
            "Reading the schema",
            "Read the schema",
        ),
        row(
            "GET /api/ask/catalog",
            false,
            false,
            "bounded",
            "catalog_page_bytes",
            "Reading the catalog",
            "Read the catalog",
        ),
        row(
            "GET /api/ask/catalog/{level}",
            false,
            false,
            "bounded",
            "catalog_page_bytes",
            "Reading a level",
            "Read a level",
        ),
        row(
            "GET /api/ask/catalog/{level}/{field}/values",
            false,
            false,
            "bounded",
            "options_values",
            "Sampling a field",
            "Sampled a field",
        ),
        row(
            "GET /api/ask/guide",
            false,
            false,
            "free",
            "one document",
            "Reading the guide",
            "Read the guide",
        ),
        row(
            "POST /api/ask/draft",
            true,
            false,
            "bounded",
            "one document",
            "Drafting",
            "Drafted",
        ),
        row(
            "POST /api/ask/diff",
            false,
            false,
            "free",
            "one diff",
            "Comparing",
            "Compared",
        ),
        row(
            "POST /api/ask/validate",
            false,
            false,
            "free",
            "issues",
            "Validating",
            "Validated",
        ),
        row(
            "POST /api/ask/run",
            true,
            true,
            "bounded",
            "sync_max_rows",
            "Running a question",
            "Ran a question",
        ),
        row(
            "POST /api/ask/jobs",
            true,
            true,
            "job",
            "one id",
            "Queuing a question",
            "Queued a question",
        ),
        row(
            "POST /api/ask/explain",
            false,
            false,
            "free",
            "two texts",
            "Explaining",
            "Explained",
        ),
        row(
            "POST /api/ask/options",
            false,
            false,
            "free",
            "move_kinds",
            "Offering moves",
            "Offered moves",
        ),
        row(
            "POST /api/ask/apply",
            true,
            true,
            "free",
            "one document",
            "Applying moves",
            "Applied moves",
        ),
        row(
            "POST /api/ask/diagnose",
            false,
            false,
            "bounded",
            "diagnose_variants",
            "Diagnosing",
            "Diagnosed",
        ),
        row(
            "POST /api/ask/preview",
            false,
            false,
            "bounded",
            "preview_rows",
            "Previewing",
            "Previewed",
        ),
        row(
            "POST /api/ask/profile",
            false,
            false,
            "bounded",
            "a preview per part",
            "Profiling",
            "Profiled",
        ),
        row(
            "POST /api/ask/describe",
            false,
            false,
            "free",
            "one description",
            "Describing",
            "Described",
        ),
        row(
            "GET /api/summary",
            false,
            false,
            "bounded",
            "one document",
            "Reading what the registry holds",
            "Read what the registry holds",
        ),
        row(
            "POST /api/ask/start",
            false,
            false,
            "bounded",
            "one document",
            "Resolving a starting point",
            "Resolved a starting point",
        ),
        row(
            "GET /api/ask/documents",
            false,
            false,
            "bounded",
            "page_rows_max lineages",
            "Listing the documents",
            "Listed the documents",
        ),
        row(
            "POST /api/ask/documents",
            true,
            false,
            "free",
            "one handle",
            "Storing a document",
            "Stored a document",
        ),
        row(
            "GET /api/ask/documents/{id}",
            false,
            false,
            "free",
            "one document",
            "Reading a document",
            "Read a document",
        ),
        row(
            "PUT /api/ask/selections/{name}",
            true,
            false,
            "free",
            "one version",
            "Saving a selection",
            "Saved a selection",
        ),
        row(
            "GET /api/ask/selections/{name}",
            false,
            false,
            "free",
            "one version",
            "Reading a selection",
            "Read a selection",
        ),
        row(
            "GET /api/ask/handles",
            false,
            false,
            "free",
            "page_rows_max handles",
            "Listing results",
            "Listed results",
        ),
        row(
            "GET /api/ask/handles/{id}",
            false,
            false,
            "free",
            "one handle",
            "Reading a handle",
            "Read a handle",
        ),
        row(
            "GET /api/timeline/{kind}/{id}",
            false,
            false,
            "bounded",
            "one object's events",
            "Reading a timeline",
            "Read a timeline",
        ),
        row(
            "GET /api/depends/{kind}/{id}",
            false,
            false,
            "bounded",
            "one closure: counts, fifty stacks, the handles and releases",
            "Reading what a change would move",
            "Read what a change would move",
        ),
        row(
            "GET /api/ask/handles/{id}/rows",
            false,
            false,
            "bounded",
            "page_rows",
            "Paging a result",
            "Paged a result",
        ),
        row(
            "POST /api/ask/handles/{id}/promote",
            true,
            true,
            "job",
            "one id",
            "Promoting to a cohort",
            "Promoted to a cohort",
        ),
        row(
            "POST /api/ask/values",
            true,
            false,
            "bounded",
            "values_inline_rows",
            "Uploading a list",
            "Uploaded a list",
        ),
        row(
            "POST /api/sessions/rebuild",
            true,
            false,
            "job",
            "one id",
            "Rebuilding sessions",
            "Rebuilt sessions",
        ),
        // record 28: the constants of the binary, which say nothing of the
        // registry and cost nothing to read
        row(
            "GET /api/pseudonymize/tags",
            false,
            false,
            "free",
            "one document",
            "Reading what the pseudonymiser removes",
            "Read what the pseudonymiser removes",
        ),
    ]
    .into_iter()
    .chain(
        crate::campaigns::POLICY
            .iter()
            .map(|(door, writes, idem, cost, cap, now, then)| {
                row(door, *writes, *idem, cost, cap, now, then)
            }),
    )
    .collect()
}

/// Record 45: a review item an open campaign holds is answered in the
/// campaign, held to the constraints it froze, and closed by its close;
/// Review's apply doors refuse it and name the campaign.
fn not_held(registry: &mut Registry, id: i64) -> Result<(), Reply> {
    match nils_registry::campaign::holder(registry.store(), id) {
        Ok(Some((campaign, name))) => Err(Reply::error(
            409,
            format!(
                "review item {id} is asked by campaign {name} ({campaign}), which answers it and closes it; answer it there"
            ),
        )),
        Ok(None) => Ok(()),
        Err(e) => Err(Reply::error(500, e.to_string())),
    }
}

#[cfg(test)]
mod disclosure_tests {
    use super::*;

    #[test]
    fn every_error_carries_its_disclosure() {
        assert_eq!(
            Reply::error(400, "the set x is not declared").body["disclosure"],
            "safe"
        );
        assert_eq!(Reply::error(404, "no handle 7").body["disclosure"], "safe");
        assert_eq!(
            Reply::error(500, "the store: locked").body["disclosure"],
            "internal"
        );
        // a review refusal quotes a reference and who decided: gated
        let r = review_err(nils_registry::review::Error::Refused(
            "manufacturer at subject S-0001 was decided by a person (anna); an agent does not override that".into(),
        ));
        assert_eq!(r.status, 409);
        assert_eq!(r.body["disclosure"], "gated");
    }
}

#[cfg(test)]
mod token_tests {
    use super::token_entries;

    #[test]
    fn nils_tokens_splits_into_the_entries_token_takes() {
        // what worked before reads the same
        assert_eq!(
            token_entries("t1=bo@lab,t2=cy@lab:reader"),
            ["t1=bo@lab", "t2=cy@lab:reader"]
        );
        assert_eq!(
            token_entries(" t1=bo@lab: , ,t2=cy@lab "),
            ["t1=bo@lab:", "t2=cy@lab"]
        );
        // a piece that holds no `=` continues the entry before it
        assert_eq!(
            token_entries("t1=bo@lab:reader,kvasir:see, t2=cy@lab:query:work"),
            ["t1=bo@lab:reader,kvasir:see", "t2=cy@lab:query:work"]
        );
        // a piece before any entry stays on its own, and is refused as before
        assert_eq!(token_entries("reader,t1=bo@lab"), ["reader", "t1=bo@lab"]);
    }
}

#[cfg(test)]
mod verb_tests {
    use super::verb_needs;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_classify_is_queued_and_its_vote_matrix_is_not() {
        // record 41: `classify votes --out FILE` writes where it is told,
        // and a door never names a path of the host
        assert!(verb_needs(&words("classify --pack mri")).is_some());
        assert!(verb_needs(&words("classify")).is_some());
        assert!(verb_needs(&words("classify votes --out x.tsv")).is_none());
    }
}

#[cfg(test)]
mod query_tests {
    use super::decoded;

    #[test]
    fn a_query_key_and_value_are_decoded_once() {
        assert_eq!(decoded("body_part%3Amodel"), "body_part:model");
        assert_eq!(decoded("application%2Fx-nifti"), "application/x-nifti");
        // percent-decoding only: a `+` keeps its meaning, as in a time's
        // offset or a principal, and a space comes as %20
        assert_eq!(decoded("a+b"), "a+b");
        assert_eq!(
            decoded("2026-09-24T10:00:00+02:00"),
            "2026-09-24T10:00:00+02:00"
        );
        assert_eq!(decoded("anna+lab%40node"), "anna+lab@node");
        assert_eq!(decoded("a%20b"), "a b");
        assert_eq!(decoded("a%2Bb"), "a+b");
        assert_eq!(decoded("100%"), "100%");
        assert_eq!(decoded("%zz"), "%zz");
        assert_eq!(decoded("%25zz"), "%zz");
    }
}
