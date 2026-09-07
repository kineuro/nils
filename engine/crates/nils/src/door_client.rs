// SPDX-License-Identifier: AGPL-3.0-only

//! The client side of the doors, for `nils ask --server` (Wave 4b §12.3):
//! one HTTP/1.1 request per call over the standard library, because the
//! command line calls the crate in process for a standalone registry and
//! the doors for a server, and neither path may drag a runtime in. Every
//! reply of `nils serve` carries a content length, so a body is read
//! exactly. TLS belongs to whatever sits in front of the engine: an
//! `https` URL is refused by name, and a tunnel or a local port is what
//! the command line speaks to.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;

use crate::{Exit, fail, usage};

/// A door to talk to: where it listens and who we are.
pub(crate) struct Door {
    host: String,
    port: u16,
    prefix: String,
    token: Option<String>,
    timeout: Duration,
}

impl Door {
    /// `http://127.0.0.1:8437`, with an optional path prefix.
    pub(crate) fn new(url: &str, token: Option<String>, timeout_ms: u64) -> Result<Door, Exit> {
        let rest = match url.strip_prefix("http://") {
            Some(r) => r,
            None if url.starts_with("https://") => {
                return Err(usage(format!(
                    "{url}: the command line speaks http to the engine; TLS belongs to whatever sits in front of it, so name the local port or a tunnel"
                )));
            }
            None => {
                return Err(usage(format!("{url} is not a URL: http://host:port")));
            }
        };
        let (authority, prefix) = match rest.split_once('/') {
            Some((a, p)) => (a, format!("/{}", p.trim_end_matches('/'))),
            None => (rest, String::new()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| usage(format!("{p} is not a port")))?,
            ),
            None => (authority.to_string(), 80),
        };
        if host.is_empty() {
            return Err(usage(format!("{url} names no host")));
        }
        Ok(Door {
            host,
            port,
            prefix,
            token,
            timeout: Duration::from_millis(timeout_ms.max(1_000)),
        })
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value, Exit> {
        self.call("GET", path, None)
    }

    pub(crate) fn post(&self, path: &str, body: &Value) -> Result<Value, Exit> {
        self.call("POST", path, Some(body))
    }

    pub(crate) fn put(&self, path: &str, body: &Value) -> Result<Value, Exit> {
        self.call("PUT", path, Some(body))
    }

    /// One request; a reply outside 200 to 299 is the door's error.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Exit> {
        let (status, text) = self.raw(method, path, body)?;
        let doc: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| serde_json::json!({ "error": text.trim() }));
        if (200..300).contains(&status) {
            return Ok(doc);
        }
        let message = doc["error"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| doc.to_string());
        let issues = doc["issues"].as_array().map(|a| {
            a.iter()
                .filter_map(|i| {
                    Some(format!(
                        "\n  {} at {}: {}",
                        i["code"].as_str()?,
                        i["path"].as_str()?,
                        i["message"].as_str()?
                    ))
                })
                .collect::<String>()
        });
        Err(match status {
            400 | 404 | 409 => usage(format!(
                "{method} {path}: {message}{}",
                issues.unwrap_or_default()
            )),
            401 | 403 => fail(format!("{method} {path}: {message}")),
            _ => fail(format!("{method} {path}: {status} {message}")),
        })
    }

    fn raw(&self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, String), Exit> {
        let where_ = format!("{}:{}", self.host, self.port);
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))
            .map_err(|e| fail(format!("cannot reach {where_}: {e}")))?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!(
            "{method} {}{path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\nContent-Length: {}\r\n",
            self.prefix,
            self.host,
            payload.len()
        );
        if body.is_some() {
            head.push_str("Content-Type: application/json\r\n");
        }
        if let Some(t) = &self.token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        head.push_str("\r\n");
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(payload.as_bytes()))
            .and_then(|_| stream.flush())
            .map_err(|e| fail(format!("cannot write to {where_}: {e}")))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|e| fail(format!("cannot read from {where_}: {e}")))?;
        // Split on the bytes: a content length counts bytes, not characters.
        let split = response
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| fail(format!("{where_} answered no headers")))?;
        let headers = String::from_utf8_lossy(&response[..split]).into_owned();
        let rest = &response[split + 4..];
        let status: u16 = headers
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| fail(format!("{where_} answered no status")))?;
        // The engine always sends a content length; anything else is read
        // to the close.
        let length = headers
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(rest.len())
            .min(rest.len());
        Ok((
            status,
            String::from_utf8_lossy(&rest[..length]).into_owned(),
        ))
    }
}
