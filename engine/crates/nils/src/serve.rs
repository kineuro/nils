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

use crate::{Exit, ServeArgs, fail, usage};

/// The contract versions this binary speaks, read from the checked-in
/// contracts at build time so that the door and the document cannot drift.
const OPENAPI_VERSION: &str = include_str!("../../../../contracts/openapi/VERSION");
const REVIEW_ITEM_VERSION: &str = include_str!("../../../../contracts/review-item/VERSION");
// Wave 4c §6.7: the suite vocabulary and the MCP door, each versioned.
const SUITE_VERSION: &str = include_str!("../../../../contracts/suite/VERSION");
const MCP_VERSION: &str = include_str!("../../../../contracts/mcp/VERSION");
const PACK_CONTRACT_VERSION: &str = include_str!("../../../../contracts/pack/VERSION");

/// What a caller may do (Wave 4a §11.2): groups map to roles, and a door
/// asks for one. `reader` reads and previews; `reviewer` decides;
/// `operator` queues work and cancels it; `admin` reads the audit log and
/// the custody. Under `off` and `token` every caller holds every role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Role {
    Reader,
    Reviewer,
    Operator,
    Admin,
}

impl Role {
    pub(crate) fn parse(text: &str) -> Option<Role> {
        Some(match text {
            "reader" => Role::Reader,
            "reviewer" => Role::Reviewer,
            "operator" => Role::Operator,
            "admin" => Role::Admin,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Role::Reader => "reader",
            Role::Reviewer => "reviewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

/// The claims an OIDC token carries that the engine reads.
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
/// issuer's keys and its audience, maps groups to roles, and keeps no user
/// table beyond a cache of claims for the token's lifetime.
struct Oidc {
    /// Wave 4c §5.3: the issuers the engine trusts, each with its own
    /// audience and keys; a token is verified against the one it names.
    trusts: Vec<Trust>,
    groups_claim: String,
    /// group -> role
    roles: HashMap<String, Role>,
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
    jwks: Jwks,
    keys: std::sync::Mutex<Vec<Key>>,
    fetched: std::sync::Mutex<Instant>,
}

impl Trust {
    /// Fetch the keys again, when they come from a URL and the floor has
    /// passed; true when the key list was replaced.
    fn refetch(&self, floor: u64) -> bool {
        let Jwks::Url(_) = &self.jwks else {
            return false;
        };
        let mut fetched = match self.fetched.lock() {
            Ok(f) => f,
            Err(_) => return false,
        };
        if fetched.elapsed().as_secs() < floor {
            return false;
        }
        *fetched = Instant::now();
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
            let mut response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
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
/// the principal, the roles, and (Wave 4c §5.9) the display name and mail
/// beside the subject, which are listed in custody and are never a key.
#[derive(Clone)]
struct Known {
    principal: String,
    roles: Vec<Role>,
    exp: u64,
    display: Option<String>,
    email: Option<String>,
    /// The `act` claim's subject, when the token was exchanged.
    act: Option<String>,
}
type ClaimsCache = HashMap<String, Known>;

/// Who a request is from.
enum Auth {
    /// The local user, as the command line would record it.
    Off,
    /// A bearer token names the caller.
    Token(HashMap<String, (String, Vec<Role>)>),
    /// An OIDC token names the caller, and its groups say what they may do.
    Oidc(Box<Oidc>),
}

/// A caller: who, and with which roles.
pub(crate) struct Caller {
    pub(crate) principal: String,
    pub(crate) roles: Vec<Role>,
    /// Wave 4c §5.9: the display name and mail the token carried, kept
    /// beside the subject and never a key.
    pub(crate) display: Option<String>,
    pub(crate) email: Option<String>,
    /// Wave 4c §5.5: who acts for the principal on this call; absent is
    /// its own value.
    pub(crate) actor: serde_json::Value,
    /// Wave 4c §5.5: the downgrade only ceiling the call named, if any.
    pub(crate) ceiling: Option<Role>,
    /// Wave 4c §6.3: the `Idempotency-Key` the call carried, if any.
    pub(crate) idempotency_key: Option<String>,
}

impl Caller {
    pub(crate) fn can(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }
}

const EVERY_ROLE: [Role; 4] = [Role::Reader, Role::Reviewer, Role::Operator, Role::Admin];

impl Auth {
    fn parse(args: &ServeArgs) -> Result<Auth, Exit> {
        match args.auth.as_str() {
            "off" => Ok(Auth::Off),
            "token" => {
                let mut tokens = HashMap::new();
                let mut given: Vec<String> = args.token.clone();
                if let Ok(env) = std::env::var("NILS_TOKENS") {
                    given.extend(
                        env.split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from),
                    );
                }
                for t in given {
                    let Some((token, rest)) = t.split_once('=') else {
                        return Err(usage(format!("{t} is not TOKEN=user@node[:roles]")));
                    };
                    // `user@node:reader,operator`; no suffix is every role,
                    // an empty suffix is no role (Wave 4b §12.4)
                    let (who, roles) = match rest.split_once(':') {
                        Some((who, list)) => {
                            let mut roles = Vec::new();
                            for r in list.split(',').map(str::trim).filter(|r| !r.is_empty()) {
                                let Some(role) = Role::parse(r) else {
                                    return Err(usage(format!(
                                        "{r} is not a role: reader, reviewer, operator or admin"
                                    )));
                                };
                                roles.push(role);
                            }
                            (who, roles)
                        }
                        None => {
                            // Wave 4c §6.1: the shortest form is the widest one.
                            eprintln!(
                                "nils serve: the token for {rest} names no roles and therefore holds every role, admin included; a machine token that should hold less is written {rest}:reader,operator"
                            );
                            (rest, EVERY_ROLE.to_vec())
                        }
                    };
                    let Some(p) = nils_registry::principal::Principal::parse(who) else {
                        return Err(usage(format!("{who} is not a principal, user@node")));
                    };
                    if token.len() < 16 {
                        return Err(usage("a token is at least 16 characters"));
                    }
                    let mut roles = roles;
                    roles.sort();
                    roles.dedup();
                    if let Some(top) = roles.iter().max().copied() {
                        roles = EVERY_ROLE.iter().copied().filter(|r| *r <= top).collect();
                    }
                    tokens.insert(token.to_string(), (p.to_string(), roles));
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
                // Wave 4b are sugar for one entry.
                let mut specs: Vec<(String, String, Jwks)> = Vec::new();
                for t in &args.oidc_trust {
                    let (mut issuer, mut audience, mut jwks) = (None, None, None);
                    for part in t.split(',') {
                        let Some((k, v)) = part.split_once('=') else {
                            return Err(usage(format!(
                                "{t} is not issuer=URL,audience=ID,jwks=URL"
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
                            other => {
                                return Err(usage(format!(
                                    "{other} is not a part of --oidc-trust: issuer, audience, jwks"
                                )));
                            }
                        }
                    }
                    match (issuer, audience, jwks) {
                        (Some(i), Some(a), Some(j)) => specs.push((i, a, j)),
                        _ => {
                            return Err(usage(format!(
                                "{t}: --oidc-trust names issuer, audience and jwks together"
                            )));
                        }
                    }
                }
                match (&args.oidc_issuer, &args.oidc_audience, &args.oidc_jwks) {
                    (Some(i), Some(a), Some(j)) => {
                        specs.push((i.clone(), a.clone(), Jwks::File(j.clone())));
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
                for (issuer, audience, jwks) in specs {
                    let keys = load_jwks(&jwks).map_err(usage)?;
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
                        jwks,
                        keys: std::sync::Mutex::new(keys),
                        fetched: std::sync::Mutex::new(Instant::now()),
                    });
                }
                let mut roles = HashMap::new();
                for r in &args.role {
                    let Some((group, role)) = r.split_once('=') else {
                        return Err(usage(format!("{r} is not GROUP=ROLE")));
                    };
                    let Some(role) = Role::parse(role.trim()) else {
                        return Err(usage(format!(
                            "{role} is not a role: reader, reviewer, operator or admin"
                        )));
                    };
                    roles.insert(group.trim().to_string(), role);
                }
                Ok(Auth::Oidc(Box::new(Oidc {
                    trusts,
                    groups_claim: args.oidc_groups_claim.clone(),
                    roles,
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
                roles: EVERY_ROLE.to_vec(),
                display: None,
                email: None,
                actor: nils_registry::actor::absent(),
                ceiling: None,
                idempotency_key: None,
            },
            Auth::Token(tokens) => {
                let token = bearer()?;
                match tokens.get(&token) {
                    Some((p, roles)) => Caller {
                        principal: p.clone(),
                        roles: roles.clone(),
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
                    Some(sub) => serde_json::json!({ "kind": "agent", "name": sub }),
                    None => nils_registry::actor::absent(),
                };
                Caller {
                    principal: known.principal,
                    roles: known.roles,
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

/// Wave 4c §5.5: the two headers a call may carry. `X-Nils-Ceiling` can
/// only remove roles; `X-Nils-Actor` names who acts for the principal and
/// is recorded with the ceiling inside it.
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
        caller.actor = value;
    }
    if let Some(ceiling) = header("X-Nils-Ceiling") {
        let Some(role) = Role::parse(&ceiling) else {
            return Err(Reply::error(
                400,
                format!(
                    "X-Nils-Ceiling {ceiling} is not a role: reader, reviewer, operator or admin"
                ),
            ));
        };
        caller.roles.retain(|r| *r <= role);
        caller.ceiling = Some(role);
        caller.actor["ceiling"] = serde_json::Value::String(role.name().to_string());
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
                let holds_kid = header
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
            let mut roles: Vec<Role> = groups
                .iter()
                .filter_map(|g| oidc.roles.get(g).copied())
                .collect();
            roles.sort();
            roles.dedup();
            // A role implies the ones below it: an operator reads.
            if let Some(top) = roles.iter().max().copied() {
                roles = EVERY_ROLE.iter().copied().filter(|r| *r <= top).collect();
            }
            // Wave 4b §12.4: a token with no role is refused at every
            // door, never defaulted to reader.
            // The audit principal is the subject (§11.2), at the issuer's node.
            let principal = format!("{}@{}", claims.sub, trust.node);
            let known = Known {
                principal,
                roles,
                exp: claims.exp,
                display: claims.preferred_username.clone().or(claims.name.clone()),
                email: claims.email.clone(),
                act: claims
                    .act
                    .as_ref()
                    .and_then(|a| a.get("sub"))
                    .and_then(|s| s.as_str())
                    .map(String::from),
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
}

impl Reply {
    pub(crate) fn ok(body: serde_json::Value) -> Reply {
        Reply {
            status: 200,
            body,
            headers: Vec::new(),
            empty: false,
        }
    }
    pub(crate) fn accepted(body: serde_json::Value) -> Reply {
        Reply {
            status: 202,
            body,
            headers: Vec::new(),
            empty: false,
        }
    }
    pub(crate) fn created(body: serde_json::Value) -> Reply {
        Reply {
            status: 201,
            body,
            headers: Vec::new(),
            empty: false,
        }
    }
    pub(crate) fn error(status: u16, message: impl Into<String>) -> Reply {
        Reply {
            status,
            body: serde_json::json!({ "error": message.into() }),
            headers: Vec::new(),
            empty: false,
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
    let server = tiny_http::Server::http(&args.bind)
        .map_err(|e| fail(format!("cannot listen on {}: {e}", args.bind)))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .unwrap_or_else(|| args.bind.clone());
    println!(
        "nils serve   {bound}   auth {}   workers {}   registry {}",
        auth.name(),
        args.workers.max(1),
        home.dir().display()
    );
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
        pack_dir: args.pack_dir.clone(),
        node: nils_registry::job::hostname(),
        started: Instant::now(),
        served: AtomicUsize::new(0),
        ask_dsn: args.ask_dsn.clone(),
        ask_caps,
        ask_pack: args.ask_pack.clone(),
        mcp_authorization_servers: args.mcp_authorization_server.clone(),
        bound: bound.clone(),
        assist: args.assist.clone(),
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
    Ok(())
}

fn respond(request: Request, reply: Reply) -> std::io::Result<()> {
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
        .filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    let mut body = String::new();
    // A body rides on a POST and on a PUT (the selection door); reading
    // it only on a POST was why a PUT selection could carry no document.
    if matches!(method, Method::Post | Method::Put) {
        let _ = request.as_reader().read_to_string(&mut body);
    }
    if path == "/api/events" && method == Method::Get {
        // Display plumbing: the open jobs, every second, until the client
        // goes away. Never the execution context.
        events(doors, registry, request);
        return;
    }
    let caller = doors.auth.caller(&request);
    // Wave 4c §5.5: the actor of this call, read by every writer of
    // provenance on this thread.
    if let Ok(c) = &caller {
        nils_registry::actor::set(c.actor.clone());
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
/// run. The roles, the reader and the caps are the doors' own.
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
    // Wave 4b §12.2: the ask doors check their own roles.
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
    // Which role a door asks for (§11.2). A door not named here asks for
    // reader, which every caller holds.
    let needs = match (method.as_str(), segs.as_slice()) {
        ("GET", ["api", "audit"]) | ("GET", ["api", "custody"]) => Role::Admin,
        ("GET", ["api", "quarantine"]) => Role::Reviewer,
        // Wave 4c §6.6: the knob engine.
        ("GET", ["api", "classify", "signals"])
        | ("POST", ["api", "classify", "try"])
        | ("POST", ["api", "overlays"]) => Role::Reviewer,
        ("POST", ["api", "overlays", _, "adopt"]) | ("POST", ["api", "ingest", "probe"]) => {
            Role::Operator
        }
        ("POST", ["api", "jobs"])
        | ("POST", ["api", "jobs", _, "cancel"])
        | ("POST", ["api", "releases"])
        | ("POST", ["api", "handovers"]) => Role::Operator,
        ("POST", ["api", "review", _, _]) | ("POST", ["api", "decisions", _, _]) => Role::Reviewer,
        _ => Role::Reader,
    };
    if !caller.can(needs) {
        return Err(Reply::error(
            403,
            format!(
                "{path} asks for the {} role; {principal} holds {}",
                needs.name(),
                if caller.roles.is_empty() {
                    "no role: an installer binds roles before a caller reads".to_string()
                } else {
                    caller
                        .roles
                        .iter()
                        .map(|r| r.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ));
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
            match crate::pack_doc(&dir, name)? {
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
            Ok(Reply::ok(crate::quarantine_doc(
                registry,
                batch,
                query.get("class").map(String::as_str),
            )?))
        }
        // Wave 4c §6.6: the knob engine.
        ["api", "classify", "signals"] if get => {
            let scope = query.get("scope").map(String::as_str).ok_or_else(|| {
                Reply::error(400, "scope: batch:<id>, origin:<name> or pack:<version>")
            })?;
            let scope =
                nils_classify::scope::Scope::parse(scope).map_err(|e| Reply::error(400, e))?;
            Ok(Reply::ok(nils_classify::signals::signals(
                registry.store(),
                &scope,
            )?))
        }
        ["api", "classify", "try"] if post => {
            let doc = json_body(body)?;
            let scope = scope_of(&doc)?;
            let sample = nils_classify::rehearse::sample_of(doc["sample"].as_i64());
            let (_, tried) = rehearsed(doors, registry, &doc["overlay"], &scope, sample)?;
            Ok(Reply::ok(tried))
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
                    "roles": caller.roles.iter().map(|r| r.name()).collect::<Vec<_>>(),
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
                        serde_json::json!({"name": o.name, "version": o.version, "pack": o.pack}),
                    ),
                },
            )?;
            Ok(Reply::accepted(serde_json::json!({
                "job": job,
                "overlay": id,
                "status": nils_registry::overlay::ADOPTED,
            })))
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
                serde_json::json!({
                    "roles": caller.roles.iter().map(|r| r.name()).collect::<Vec<_>>(),
                    "actor": caller.actor,
                }),
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
            let jobs = nils_registry::job::list(registry.store(), all, limit).map_err(job_err)?;
            Ok(Reply::ok(serde_json::json!({
                "count": jobs.len(),
                "jobs": jobs.iter().map(nils_registry::job::Job::as_json).collect::<Vec<_>>(),
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
            // Wave 4c §6.5: of the linkage verbs only `import` is a job the
            // door queues; purge and the rest stay on the command line.
            if command[0] == "linkage" && command.get(1).map(String::as_str) != Some("import") {
                return Err(Reply::error(
                    400,
                    "linkage import is the one linkage verb the door queues",
                ));
            }
            // Wave 4c §6.5: a tree is named by a registered location, as
            // @name/relative, never by a path a caller composes; backup and
            // verify go to the deployment's backup directory.
            let command = located(doors, command)?;
            let id = nils_registry::job::enqueue(
                registry.store(),
                &command,
                doc["name"].as_str(),
                Some(principal),
            )
            .map_err(job_err)?;
            Ok(Reply::accepted(
                serde_json::json!({ "job": id, "state": "queued" }),
            ))
        }
        ["api", "jobs", _] if get => {
            let id = id_at(2)?;
            match nils_registry::job::show(registry.store(), id).map_err(job_err)? {
                Some(j) => Ok(Reply::ok(j.as_json())),
                None => Err(Reply::error(404, format!("no job {id}"))),
            }
        }
        ["api", "jobs", _, "cancel"] if post => {
            let id = id_at(2)?;
            match nils_registry::job::request_cancel(registry.store(), id).map_err(job_err)? {
                Some(state) => Ok(Reply::ok(
                    serde_json::json!({ "job": id, "state": state.name() }),
                )),
                None => Err(Reply::error(404, format!("no job {id}"))),
            }
        }
        ["api", "releases"] if get => Ok(Reply::ok(crate::releases_doc(registry, limit)?)),
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
            command.extend(["--name".into(), name.into(), "--out".into(), out.into()]);
            for (flag, key) in [
                ("--layout", "layout"),
                ("--dates", "dates"),
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
            let id = nils_registry::job::enqueue(
                registry.store(),
                &command,
                Some(name),
                Some(principal),
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
            let id = nils_registry::job::enqueue(
                registry.store(),
                &command,
                Some(release),
                Some(principal),
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
            let rows = review_list(
                registry.store(),
                query.get("status").map(String::as_str),
                query.get("kind").map(String::as_str),
                limit,
            )?;
            Ok(Reply::ok(
                serde_json::json!({ "count": rows.len(), "items": rows }),
            ))
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
            Ok(Reply::ok(serde_json::json!({
                "id": item.id, "kind": item.kind, "scope": item.scope, "status": item.status,
                "ref": item.reference, "evidence": item.evidence, "members": item.members,
                "member_stacks": members,
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
            let kind = doc["author_kind"].as_str().unwrap_or("person");
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
                        version: doc["model_version"].as_str(),
                    },
                    stage: doc["stage"].as_bool().unwrap_or(false),
                    why: doc["why"].as_str(),
                },
            )
            .map_err(review_err)?;
            Ok(Reply::ok(serde_json::json!({
                "decision": applied.decision, "axis": applied.axis, "scope": applied.scope,
                "ref": applied.reference, "closed": applied.closed, "members": applied.members,
                "staged": applied.staged,
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
            let done = nils_registry::review::commit(
                registry,
                Some(id),
                doc["anyway"].as_bool().unwrap_or(false),
                principal,
            )
            .map_err(review_err)?;
            Ok(Reply::ok(
                serde_json::json!({ "committed": done.decisions, "items": done.items }),
            ))
        }
        ["api", "decisions", _, "withdraw"] if post => {
            let id = id_at(2)?;
            let reopened =
                nils_registry::review::withdraw(registry, id, principal).map_err(review_err)?;
            Ok(Reply::ok(
                serde_json::json!({ "withdrawn": id, "reopened": reopened }),
            ))
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
    // Wave 4c §6.5: an archive as a job, and its check; never a restore.
    "backup",
    "verify",
    "digest",
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
];

/// Wave 4c §6.6: the scope a body names.
fn scope_of(doc: &serde_json::Value) -> Result<nils_classify::scope::Scope, Reply> {
    let text = doc["scope"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "scope: batch:<id>, origin:<name> or pack:<version>"))?;
    nils_classify::scope::Scope::parse(text).map_err(|e| Reply::error(400, e))
}

/// The author kind the caller acts as: the actor's kind when one was
/// declared, else a person at a keyboard. With the model version beside it.
fn author_of(caller: &Caller) -> (&str, Option<&str>) {
    // Anything else, `absent` included, is a person at a keyboard.
    let kind = match caller.actor["kind"].as_str() {
        Some("agent") => "agent",
        Some("model") => "model",
        _ => "person",
    };
    (kind, caller.actor["version"].as_str())
}

/// Wave 4c §6.6: an overlay from a body, rehearsed over a scope. The pack
/// it amends is loaded bare and amended; the overlay's own cases are judged
/// and their failure is part of the answer, not a refusal.
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

fn review_err(e: nils_registry::review::Error) -> Reply {
    match e {
        nils_registry::review::Error::Refused(m) => Reply::error(409, m),
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
        "GET /api/events",
        "GET /api/packs",
        "GET /api/packs/{name}",
        "GET /api/batches",
        "GET /api/batches/{id}",
        "GET /api/quarantine",
        "GET /api/classify/signals",
        "POST /api/classify/try",
        "GET /api/overlays",
        "POST /api/overlays",
        "GET /api/overlays/{id}",
        "POST /api/overlays/{id}/adopt",
        "POST /api/ingest/probe",
    ]
    .iter()
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
        },
        "packs": packs,
        "registry": { "id": meta.registry_id, "epoch": meta.epoch, "schema_version": meta.schema_version, "synthetic": meta.synthetic },
        "auth": doors.auth.name(),
        "principal": caller.principal,
        "roles": caller.roles.iter().map(|r| r.name()).collect::<Vec<_>>(),
        "display": caller.display,
        "email": caller.email,
        "ceiling": caller.ceiling.map(Role::name),
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
        "event_streams": doors.event_streams,
        "ingest_roots": doors.ingest_roots.keys().collect::<Vec<_>>(),
        "backup_dir": doors.backup_dir.is_some(),
        "policy": policy(),
        "idempotency": {
            "header": "Idempotency-Key",
            "doors": crate::ask_doors::IDEMPOTENT_DOORS,
            "hours": nils_registry::idempotency::KEEP_HOURS,
        },
    })
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
/// until the client goes away. Display plumbing only.
fn events(doors: &Doors, registry: &mut Registry, request: Request) {
    let caller = match doors.auth.caller(&request) {
        Ok(c) => c,
        Err(reply) => {
            let _ = respond(request, reply);
            return;
        }
    };
    // Wave 4c §6.1: a door like any other, so it asks for the reader role;
    // and capped well below the worker count, because an open stream pins
    // a worker for its life and four tabs must not wedge every door.
    if !caller.can(Role::Reader) {
        let _ = respond(
            request,
            Reply::error(
                403,
                format!(
                    "/api/events asks for the reader role; {} holds no role",
                    caller.principal
                ),
            ),
        );
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
        let jobs = nils_registry::job::list(registry.store(), false, 50).unwrap_or_default();
        let data = serde_json::json!({
            "epoch": registry.meta().epoch,
            "jobs": jobs.iter().map(nils_registry::job::Job::as_json).collect::<Vec<_>>(),
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
fn located(doors: &Doors, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let verb = command[0].as_str();
    match verb {
        "backup" => {
            let dir = doors.backup_dir.as_ref().ok_or_else(|| {
                Reply::error(
                    409,
                    "no backup directory: start nils serve with --backup-dir",
                )
            })?;
            return Ok(vec![
                "backup".into(),
                "--dir".into(),
                dir.display().to_string(),
            ]);
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
            return Ok(vec!["verify".into(), dir.join(name).display().to_string()]);
        }
        _ => {}
    }
    let takes_a_tree =
        verb == "digest" || (verb == "linkage" && command.get(1).is_some_and(|c| c == "import"));
    let mut out = Vec::with_capacity(command.len());
    for arg in command {
        if let Some(rest) = arg.strip_prefix('@') {
            let (name, rel) = rest.split_once('/').unwrap_or((rest, ""));
            let root = doors.ingest_roots.get(name).ok_or_else(|| {
                Reply::error(
                    400,
                    format!(
                        "@{name} is not a registered ingest location; those are {}",
                        doors
                            .ingest_roots
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;
            if rel.split('/').any(|seg| seg == "..") || rel.starts_with('/') {
                return Err(Reply::error(
                    400,
                    format!("@{name}/{rel} steps outside its location"),
                ));
            }
            let path = if rel.is_empty() {
                root.clone()
            } else {
                root.join(rel)
            };
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

/// Wave 4c §6.5: one row per door: the role it needs, whether it writes,
/// whether it takes an idempotency key, its cost class, its result cap and
/// a human label in the present and the past tense. The desk's controls,
/// the MCP door's gating and the audit line derive from this table.
pub(crate) fn policy() -> Vec<serde_json::Value> {
    let row = |door: &str,
               role: &str,
               writes: bool,
               idem: bool,
               cost: &str,
               cap: &str,
               now: &str,
               then: &str| {
        serde_json::json!({
            "door": door, "role": role, "writes": writes, "idempotent": idem,
            "cost": cost, "result_cap": cap, "label": {"present": now, "past": then},
        })
    };
    vec![
        row(
            "GET /api/capabilities",
            "reader",
            false,
            false,
            "free",
            "one document",
            "Reading what the engine speaks",
            "Read what the engine speaks",
        ),
        row(
            "GET /api/status",
            "reader",
            false,
            false,
            "bounded",
            "one document",
            "Reading the status",
            "Read the status",
        ),
        row(
            "GET /api/custody",
            "admin",
            false,
            false,
            "bounded",
            "one document",
            "Reading custody",
            "Read custody",
        ),
        row(
            "GET /api/audit",
            "admin",
            false,
            false,
            "bounded",
            "limit rows",
            "Reading the audit log",
            "Read the audit log",
        ),
        row(
            "GET /api/jobs",
            "reader",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing jobs",
            "Listed jobs",
        ),
        row(
            "POST /api/jobs",
            "operator",
            true,
            false,
            "job",
            "one id",
            "Queuing a job",
            "Queued a job",
        ),
        row(
            "GET /api/jobs/{id}",
            "reader",
            false,
            false,
            "free",
            "one document",
            "Reading a job",
            "Read a job",
        ),
        row(
            "POST /api/jobs/{id}/cancel",
            "operator",
            true,
            true,
            "free",
            "one state",
            "Cancelling a job",
            "Cancelled a job",
        ),
        row(
            "GET /api/releases",
            "reader",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing releases",
            "Listed releases",
        ),
        row(
            "POST /api/releases",
            "operator",
            true,
            false,
            "job",
            "one id",
            "Cutting a release",
            "Cut a release",
        ),
        row(
            "POST /api/handovers",
            "operator",
            true,
            false,
            "job",
            "one id",
            "Handing over",
            "Handed over",
        ),
        row(
            "POST /api/select",
            "reader",
            false,
            false,
            "bounded",
            "sync_max_rows",
            "Previewing a selection",
            "Previewed a selection",
        ),
        row(
            "GET /api/review",
            "reviewer",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing the review queue",
            "Listed the review queue",
        ),
        row(
            "GET /api/review/{id}",
            "reviewer",
            false,
            false,
            "free",
            "one item",
            "Reading a review item",
            "Read a review item",
        ),
        row(
            "POST /api/review/{id}/apply",
            "reviewer",
            true,
            false,
            "free",
            "one decision",
            "Deciding",
            "Decided",
        ),
        row(
            "POST /api/review/{id}/accept",
            "reviewer",
            true,
            true,
            "free",
            "one item",
            "Acknowledging",
            "Acknowledged",
        ),
        row(
            "POST /api/decisions/{id}/commit",
            "reviewer",
            true,
            true,
            "free",
            "one decision",
            "Committing a decision",
            "Committed a decision",
        ),
        row(
            "POST /api/decisions/{id}/withdraw",
            "reviewer",
            true,
            true,
            "free",
            "one decision",
            "Withdrawing a decision",
            "Withdrew a decision",
        ),
        row(
            "GET /api/events",
            "reader",
            false,
            false,
            "stream",
            "event_streams",
            "Watching jobs",
            "Watched jobs",
        ),
        row(
            "GET /api/packs",
            "reader",
            false,
            false,
            "free",
            "one list",
            "Listing packs",
            "Listed packs",
        ),
        row(
            "GET /api/packs/{name}",
            "reader",
            false,
            false,
            "free",
            "one document",
            "Reading a pack",
            "Read a pack",
        ),
        row(
            "GET /api/batches",
            "reader",
            false,
            false,
            "bounded",
            "limit rows",
            "Listing batches",
            "Listed batches",
        ),
        row(
            "GET /api/batches/{id}",
            "reader",
            false,
            false,
            "free",
            "one report",
            "Reading a batch",
            "Read a batch",
        ),
        row(
            "GET /api/quarantine",
            "reviewer",
            false,
            false,
            "bounded",
            "every file",
            "Listing quarantine",
            "Listed quarantine",
        ),
        row(
            "GET /api/classify/signals",
            "reviewer",
            false,
            false,
            "bounded",
            "one document",
            "Reading the classifier's signals",
            "Read the classifier's signals",
        ),
        row(
            "POST /api/classify/try",
            "reviewer",
            false,
            false,
            "bounded",
            "one manifest",
            "Rehearsing an overlay",
            "Rehearsed an overlay",
        ),
        row(
            "GET /api/overlays",
            "reader",
            false,
            false,
            "free",
            "every overlay",
            "Listing overlays",
            "Listed overlays",
        ),
        row(
            "POST /api/overlays",
            "reviewer",
            true,
            false,
            "bounded",
            "one overlay",
            "Proposing an overlay",
            "Proposed an overlay",
        ),
        row(
            "GET /api/overlays/{id}",
            "reader",
            false,
            false,
            "free",
            "one overlay",
            "Reading an overlay",
            "Read an overlay",
        ),
        row(
            "POST /api/overlays/{id}/adopt",
            "operator",
            true,
            false,
            "job",
            "one job",
            "Adopting an overlay",
            "Adopted an overlay",
        ),
        row(
            "POST /api/ingest/probe",
            "operator",
            false,
            false,
            "job",
            "one job",
            "Probing identity rules",
            "Probed identity rules",
        ),
        row(
            "GET /api/ask/schema",
            "reader",
            false,
            false,
            "free",
            "one schema",
            "Reading the schema",
            "Read the schema",
        ),
        row(
            "GET /api/ask/catalog",
            "reader",
            false,
            false,
            "bounded",
            "catalog_page_bytes",
            "Reading the catalog",
            "Read the catalog",
        ),
        row(
            "GET /api/ask/catalog/{level}",
            "reader",
            false,
            false,
            "bounded",
            "catalog_page_bytes",
            "Reading a level",
            "Read a level",
        ),
        row(
            "GET /api/ask/catalog/{level}/{field}/values",
            "reader",
            false,
            false,
            "bounded",
            "options_values",
            "Sampling a field",
            "Sampled a field",
        ),
        row(
            "GET /api/ask/guide",
            "reader",
            false,
            false,
            "free",
            "one document",
            "Reading the guide",
            "Read the guide",
        ),
        row(
            "POST /api/ask/draft",
            "reader",
            true,
            false,
            "bounded",
            "one document",
            "Drafting",
            "Drafted",
        ),
        row(
            "POST /api/ask/diff",
            "reader",
            false,
            false,
            "free",
            "one diff",
            "Comparing",
            "Compared",
        ),
        row(
            "POST /api/ask/validate",
            "reader",
            false,
            false,
            "free",
            "issues",
            "Validating",
            "Validated",
        ),
        row(
            "POST /api/ask/run",
            "reader",
            true,
            true,
            "bounded",
            "sync_max_rows",
            "Running a question",
            "Ran a question",
        ),
        row(
            "POST /api/ask/jobs",
            "reader",
            true,
            true,
            "job",
            "one id",
            "Queuing a question",
            "Queued a question",
        ),
        row(
            "POST /api/ask/explain",
            "reader",
            false,
            false,
            "free",
            "two texts",
            "Explaining",
            "Explained",
        ),
        row(
            "POST /api/ask/options",
            "reader",
            false,
            false,
            "free",
            "move_kinds",
            "Offering moves",
            "Offered moves",
        ),
        row(
            "POST /api/ask/apply",
            "reader",
            true,
            true,
            "free",
            "one document",
            "Applying moves",
            "Applied moves",
        ),
        row(
            "POST /api/ask/diagnose",
            "reader",
            false,
            false,
            "bounded",
            "diagnose_variants",
            "Diagnosing",
            "Diagnosed",
        ),
        row(
            "POST /api/ask/preview",
            "reader",
            false,
            false,
            "bounded",
            "preview_rows",
            "Previewing",
            "Previewed",
        ),
        row(
            "POST /api/ask/describe",
            "reader",
            false,
            false,
            "free",
            "one description",
            "Describing",
            "Described",
        ),
        row(
            "GET /api/summary",
            "reader",
            false,
            false,
            "bounded",
            "one document",
            "Reading what the registry holds",
            "Read what the registry holds",
        ),
        row(
            "POST /api/ask/start",
            "reader",
            false,
            false,
            "bounded",
            "one document",
            "Resolving a starting point",
            "Resolved a starting point",
        ),
        row(
            "GET /api/ask/documents",
            "reader",
            false,
            false,
            "bounded",
            "page_rows_max lineages",
            "Listing the documents",
            "Listed the documents",
        ),
        row(
            "POST /api/ask/documents",
            "reader",
            true,
            false,
            "free",
            "one handle",
            "Storing a document",
            "Stored a document",
        ),
        row(
            "GET /api/ask/documents/{id}",
            "reader",
            false,
            false,
            "free",
            "one document",
            "Reading a document",
            "Read a document",
        ),
        row(
            "PUT /api/ask/selections/{name}",
            "reviewer",
            true,
            false,
            "free",
            "one version",
            "Saving a selection",
            "Saved a selection",
        ),
        row(
            "GET /api/ask/selections/{name}",
            "reader",
            false,
            false,
            "free",
            "one version",
            "Reading a selection",
            "Read a selection",
        ),
        row(
            "GET /api/ask/handles",
            "reader",
            false,
            false,
            "free",
            "page_rows_max handles",
            "Listing results",
            "Listed results",
        ),
        row(
            "GET /api/ask/handles/{id}",
            "reader",
            false,
            false,
            "free",
            "one handle",
            "Reading a handle",
            "Read a handle",
        ),
        row(
            "GET /api/ask/handles/{id}/rows",
            "reader",
            false,
            false,
            "bounded",
            "page_rows",
            "Paging a result",
            "Paged a result",
        ),
        row(
            "POST /api/ask/handles/{id}/promote",
            "operator",
            true,
            true,
            "job",
            "one id",
            "Promoting to a cohort",
            "Promoted to a cohort",
        ),
        row(
            "POST /api/ask/values",
            "reader",
            true,
            false,
            "bounded",
            "values_inline_rows",
            "Uploading a list",
            "Uploaded a list",
        ),
        row(
            "POST /api/sessions/rebuild",
            "operator",
            true,
            false,
            "job",
            "one id",
            "Rebuilding sessions",
            "Rebuilt sessions",
        ),
    ]
}
