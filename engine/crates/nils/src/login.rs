// SPDX-License-Identifier: AGPL-3.0-only

//! `nils login` (Wave 4c §5.8): a token for the command line, kept in the
//! user's configuration directory. Two paths: the desk in `local` mode,
//! which answers username and password with a token of one day; and a
//! provider in `oidc` mode, where the person's app password is exchanged
//! over the client credentials grant for a token the engine verifies, and
//! exchanged again on expiry. Every verb that takes `--server` reads the
//! kept token when `--token` and `NILS_TOKEN` are absent.

use std::path::PathBuf;

use serde_json::{Value, json};

/// Where the token lives: `NILS_CONFIG_DIR`, else `XDG_CONFIG_HOME/nils`,
/// else `~/.config/nils`.
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("NILS_CONFIG_DIR").map(PathBuf::from)
        && !d.as_os_str().is_empty()
    {
        return d;
    }
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return PathBuf::from(x).join("nils");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("nils")
}

pub fn token_path() -> PathBuf {
    config_dir().join("token.json")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn read() -> Option<Value> {
    let text = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// The kept token when it has not expired; for the provider path an
/// expired one is exchanged again from the kept app password.
pub fn saved_token() -> Option<String> {
    let doc = read()?;
    let exp = doc["expires_at"].as_i64().unwrap_or(0);
    if exp > now() + 30 {
        return doc["token"].as_str().map(str::to_string);
    }
    if doc["kind"] == "issuer"
        && let (Some(issuer), Some(client), Some(user), Some(pass)) = (
            doc["issuer"].as_str(),
            doc["client"].as_str(),
            doc["username"].as_str(),
            doc["password"].as_str(),
        )
        && let Ok(fresh) = exchange(issuer, client, user, pass)
    {
        let _ = keep(&fresh);
        return fresh["token"].as_str().map(str::to_string);
    }
    None
}

fn keep(doc: &Value) -> Result<(), String> {
    write_private(
        &token_path(),
        &serde_json::to_string_pretty(doc).unwrap_or_default(),
    )
}

fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    let r = ureq::post(url)
        .header("content-type", "application/json")
        .send(body.to_string().as_bytes());
    answer(r, url)
}

fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
    let r = ureq::post(url).send_form(fields.to_vec());
    answer(r, url)
}

fn answer(
    r: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    url: &str,
) -> Result<Value, String> {
    match r {
        Ok(mut resp) => {
            let text = resp
                .body_mut()
                .read_to_string()
                .map_err(|e| format!("{url}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{url}: not JSON: {e}"))
        }
        Err(ureq::Error::StatusCode(code)) => Err(format!("{url} answered {code}")),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// The desk's path: username and password for a token of one day.
pub fn desk(desk: &str, username: &str, password: &str) -> Result<Value, String> {
    let url = format!("{}/desk/cli-login", desk.trim_end_matches('/'));
    let got = post_json(&url, &json!({"username": username, "password": password}))?;
    let token = got["token"]
        .as_str()
        .ok_or_else(|| format!("{url}: no token in the answer"))?;
    Ok(json!({
        "kind": "desk",
        "desk": desk,
        "username": username,
        "token": token,
        "expires_at": got["expires_at"].as_i64().unwrap_or(now() + 24 * 3600),
        "issuer": got["issuer"],
    }))
}

/// The provider's path: the app password over the client credentials grant.
pub fn exchange(
    issuer: &str,
    client: &str,
    username: &str,
    password: &str,
) -> Result<Value, String> {
    let disc = ureq::get(format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ))
    .call()
    .map_err(|e| format!("the issuer's discovery document: {e}"))?
    .body_mut()
    .read_to_string()
    .map_err(|e| e.to_string())?;
    let disc: Value = serde_json::from_str(&disc)
        .map_err(|e| format!("the discovery document is not JSON: {e}"))?;
    let endpoint = disc["token_endpoint"]
        .as_str()
        .ok_or("the discovery document names no token_endpoint")?;
    let got = post_form(
        endpoint,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", client),
            ("username", username),
            ("password", password),
            ("scope", "openid profile email entitlements"),
        ],
    )?;
    let token = got["access_token"]
        .as_str()
        .ok_or_else(|| format!("{endpoint}: no access_token in the answer"))?;
    Ok(json!({
        "kind": "issuer",
        "issuer": issuer,
        "client": client,
        "username": username,
        "password": password,
        "token": token,
        "expires_at": now() + got["expires_in"].as_i64().unwrap_or(900),
    }))
}

pub fn login(doc: &Value) -> Result<PathBuf, String> {
    keep(doc)?;
    Ok(token_path())
}

pub fn logout() -> bool {
    std::fs::remove_file(token_path()).is_ok()
}
