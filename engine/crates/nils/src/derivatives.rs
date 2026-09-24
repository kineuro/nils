// SPDX-License-Identifier: AGPL-3.0-only

//! Derivatives at the doors and the keyboard (record 42 S4). The registry
//! keeps the rows (`nils_registry::derivative`); this module writes the
//! bytes into a working place, hashes them on the way in, and serves them
//! back.
//!
//! A file is written under `derivatives/<kind>/<first two of its digest>/`
//! in the working place, named by its sha256 and the extension it came
//! with, and never by a name a caller chose: a name may carry a person's
//! code, a digest cannot. The same content registered twice is one file and
//! two rows.
//!
//! Absence (D1): a deployment that binds no working place has no home for
//! a derivative, so the capability is off and the doors that write or read
//! bytes answer so, with the sentence that names the cure.
//!
//! The grants are the Pipelines page's (record 25 declares one grant per
//! page of the desk, and derivatives are what pipelines make): the
//! metadata and the listing need `pipelines:see`, the bytes `pipelines:see`
//! at detail quasi, since a mask or an embedding is drawn from the pixels,
//! and registering one `pipelines:work`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use nils_registry::Registry;
use nils_registry::derivative::{self, Belongs, Derivative, KINDS, TREE};
use nils_registry::place::{self, Place, Role};
use nils_registry::store::Store;
use serde_json::{Value, json};

use crate::serve::{Caller, Reply};

/// The largest body the door takes: 16 GiB.
pub(crate) const UPLOAD_MAX: u64 = 16 * 1024 * 1024 * 1024;

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 4] = [
    "GET /api/derivatives",
    "POST /api/derivatives",
    "GET /api/derivatives/{id}",
    "GET /api/derivatives/{id}/content",
];

/// The working place a derivative is written into: the first active one,
/// or the sentence saying the capability is off.
pub(crate) fn working(store: &mut Store) -> Result<Place, String> {
    let places = place::active(store).map_err(|e| e.to_string())?;
    places
        .into_iter()
        .find(|p| p.role == Role::Working)
        .ok_or_else(|| {
            "derivatives are off: no working place is bound, and a derivative lives in one; an operator adds one under Settings or with nils place add --role working (Wave 5 section 10.2)".to_string()
        })
}

/// What `GET /api/capabilities` says of derivatives.
pub(crate) fn capability(store: &mut Store) -> Value {
    let grants = json!({"see": "pipelines:see", "content": "pipelines:see", "content_detail": "quasi", "register": "pipelines:work"});
    match working(store) {
        Ok(p) => json!({
            "enabled": true, "place": p.name, "kinds": KINDS, "grants": grants,
            "upload_max_bytes": UPLOAD_MAX,
        }),
        Err(reason) => json!({
            "enabled": false, "reason": reason, "kinds": KINDS, "grants": grants,
        }),
    }
}

/// A file written into a place: its path under the place, its size and
/// its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub path: String,
    pub bytes: i64,
    pub sha256: String,
}

/// Why a file was not stored.
#[derive(Debug)]
pub(crate) enum StoreFail {
    /// The bytes are not the ones the caller named; nothing was kept.
    Mismatch {
        named: String,
        found: String,
    },
    TooLarge,
    Io(String),
}

impl std::fmt::Display for StoreFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreFail::Mismatch { named, found } => write!(
                f,
                "the bytes hash to sha256 {found}, not the {named} named; nothing was registered"
            ),
            StoreFail::TooLarge => write!(f, "a derivative is at most {UPLOAD_MAX} bytes"),
            StoreFail::Io(m) => write!(f, "{m}"),
        }
    }
}

/// The extension a file came with, when it is a plain one (`.nii.gz`,
/// `.npy`), and nothing else of its name.
fn extension(name: Option<&str>) -> String {
    let Some(name) = name else {
        return String::new();
    };
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let Some((_, ext)) = base.split_once('.') else {
        return String::new();
    };
    if ext.is_empty()
        || ext.len() > 16
        || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        || ext.starts_with('.')
        || ext.ends_with('.')
    {
        return String::new();
    }
    format!(".{}", ext.to_ascii_lowercase())
}

/// Whether a text is a sha256 as hex.
pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Write what `reader` gives into the place at `root`, hashing it on the
/// way, under its content address. With `expect`, bytes that hash to
/// anything else are removed and refused.
pub(crate) fn store_file(
    root: &Path,
    kind: &str,
    name: Option<&str>,
    reader: &mut dyn Read,
    expect: Option<&str>,
) -> Result<Stored, StoreFail> {
    let io = |what: &str, e: std::io::Error| StoreFail::Io(format!("{what}: {e}"));
    let incoming = root.join(TREE).join(".incoming");
    std::fs::create_dir_all(&incoming).map_err(|e| io("the working place", e))?;
    let mut nonce = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
        .map_err(|_| StoreFail::Io("no randomness for a temporary name".into()))?;
    let temp = incoming.join(hex::encode(nonce));
    let result = (|| -> Result<(u64, String), StoreFail> {
        let mut out = std::fs::File::create(&temp).map_err(|e| io("the working place", e))?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0u8; 1 << 20];
        let mut total: u64 = 0;
        loop {
            let n = reader.read(&mut buffer).map_err(|e| io("the upload", e))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > UPLOAD_MAX {
                return Err(StoreFail::TooLarge);
            }
            context.update(&buffer[..n]);
            out.write_all(&buffer[..n])
                .map_err(|e| io("the working place", e))?;
        }
        out.sync_all().map_err(|e| io("the working place", e))?;
        Ok((total, hex::encode(context.finish().as_ref())))
    })();
    let (bytes, sha256) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    };
    if let Some(named) = expect
        && !named.eq_ignore_ascii_case(&sha256)
    {
        let _ = std::fs::remove_file(&temp);
        return Err(StoreFail::Mismatch {
            named: named.to_ascii_lowercase(),
            found: sha256,
        });
    }
    let path = format!("{TREE}/{kind}/{}/{sha256}{}", &sha256[..2], extension(name));
    let target = root.join(&path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io("the working place", e))?;
    }
    // The same content is the same file: a second registration leaves the
    // first file where it is.
    let same = std::fs::metadata(&target).is_ok_and(|m| m.len() == bytes);
    if same {
        let _ = std::fs::remove_file(&temp);
    } else {
        std::fs::rename(&temp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            io("the working place", e)
        })?;
    }
    Ok(Stored {
        path,
        bytes: bytes as i64,
        sha256,
    })
}

/// What a registration asks for, however it arrived.
pub(crate) struct Ask<'a> {
    pub kind: &'a str,
    pub stack: Option<i64>,
    pub series: Option<i64>,
    pub subject: Option<i64>,
    pub day: Option<&'a str>,
    pub media_type: &'a str,
    pub supersedes: Option<i64>,
    /// The registered model that made it: its id, its digest or
    /// `name@version` (record 42 S2).
    pub model: Option<&'a str>,
    pub name: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub principal: &'a str,
    pub actor: Option<&'a Value>,
}

/// Why a registration was refused: a status and a sentence.
pub(crate) type Refused = (u16, String);

/// What an ask resolved to: what it belongs to, the place it goes into and
/// the registered model that made it.
pub(crate) struct Checked {
    pub belongs: Belongs,
    pub place: Place,
    pub model_id: Option<i64>,
}

/// Check an ask before any byte is read: the kind, what it belongs to, the
/// row it supersedes, the model that made it, and the place it goes into.
pub(crate) fn check(store: &mut Store, a: &Ask<'_>) -> Result<Checked, Refused> {
    let place = working(store).map_err(|m| (409, m))?;
    if !derivative::is_kind(a.kind) {
        return Err((
            400,
            format!(
                "{} is not a kind of derivative; the kinds are {}",
                a.kind,
                KINDS.join(", ")
            ),
        ));
    }
    if let Some(s) = a.sha256
        && !is_sha256(s)
    {
        return Err((400, "sha256 is 64 hexadecimal digits".into()));
    }
    let belongs =
        derivative::belongs(store, a.stack, a.series, a.subject, a.day).map_err(|e| match e {
            Ok(m) => (400, m),
            Err(e) => (500, e.to_string()),
        })?;
    if let Some(old) = a.supersedes {
        let prior = derivative::get(store, old)
            .map_err(|e| (500, e.to_string()))?
            .ok_or_else(|| (400, format!("no derivative {old} to supersede")))?;
        if prior.kind != a.kind {
            return Err((
                400,
                format!(
                    "derivative {old} is a {}; a {} does not supersede it",
                    prior.kind, a.kind
                ),
            ));
        }
    }
    let model_id = match a.model {
        None => None,
        Some(reference) => Some(
            nils_registry::model::resolve(store, reference)
                .map_err(|e| (500, e.to_string()))?
                .ok_or_else(|| (400, format!("no registered model answers to {reference}")))?
                .id,
        ),
    };
    Ok(Checked {
        belongs,
        place,
        model_id,
    })
}

/// Register a derivative: check, write the bytes, write the row, audit.
pub(crate) fn register(
    registry: &mut Registry,
    a: &Ask<'_>,
    reader: &mut dyn Read,
) -> Result<Derivative, Refused> {
    let Checked {
        belongs,
        place,
        model_id,
    } = check(registry.store(), a)?;
    let stored = store_file(Path::new(&place.path), a.kind, a.name, reader, a.sha256).map_err(
        |e| match e {
            StoreFail::Mismatch { .. } => (422, e.to_string()),
            StoreFail::TooLarge => (413, e.to_string()),
            StoreFail::Io(m) => (500, m),
        },
    )?;
    let now = nils_registry::time::now_iso();
    let store = registry.store();
    let id = derivative::insert(
        store,
        &derivative::New {
            kind: a.kind,
            belongs: &belongs,
            place_id: place.id,
            path: &stored.path,
            bytes: stored.bytes,
            sha256: &stored.sha256,
            media_type: a.media_type,
            registered_by: a.principal,
            actor: a.actor,
            model_id,
            supersedes_id: a.supersedes,
            created_at: &now,
        },
    )
    .map_err(|e| (500, e.to_string()))?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: a.principal,
            action: nils_registry::audit::Action::DerivativeRegister,
            scope: json!({
                "derivative": id, "kind": a.kind, "scope": belongs.scope,
                "stack": belongs.stack_id, "series": belongs.series_id,
                "subject": belongs.subject_id, "place": place.name, "model": model_id,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({
                "bytes": stored.bytes, "sha256": stored.sha256, "supersedes": a.supersedes,
            })),
        },
    )
    .map_err(|e| (500, e.to_string()))?;
    derivative::get(registry.store(), id)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (500, "the derivative was not written back".to_string()))
}

/// A derivative as the doors and `--json` answer it: the row, the place
/// by name and the door its bytes are read from.
pub(crate) fn doc(store: &mut Store, d: &Derivative) -> Value {
    let place = place::show(store, d.place_id).ok().flatten();
    doc_in(d, place.map(|p| p.name))
}

/// [`doc`] with its place's name read already.
fn doc_in(d: &Derivative, place: Option<String>) -> Value {
    let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
    v["place"] = json!(place);
    v["content"] = json!(format!("/api/derivatives/{}/content", d.id));
    v
}

/// The file of a derivative on disk, or why it cannot be read.
pub(crate) fn file_of(store: &mut Store, d: &Derivative) -> Result<PathBuf, Refused> {
    let p = place::show(store, d.place_id)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (409, format!("the place of derivative {} is gone", d.id)))?;
    if p.retired_at.is_some() {
        return Err((
            409,
            format!(
                "derivative {} lives in {}, which is retired; its file is read after the place is brought back",
                d.id, p.name
            ),
        ));
    }
    let path = Path::new(&p.path).join(&d.path);
    match std::fs::metadata(&path) {
        Ok(m) if m.len() == d.bytes as u64 => Ok(path),
        Ok(m) => Err((
            409,
            format!(
                "derivative {} was registered at {} bytes and its file in {} is {}; it is not served",
                d.id,
                d.bytes,
                p.name,
                m.len()
            ),
        )),
        Err(_) => Err((
            404,
            format!("the file of derivative {} is not in {}", d.id, p.name),
        )),
    }
}

fn int_of(
    query: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<Option<i64>, Reply> {
    query
        .get(key)
        .map(|v| {
            v.parse::<i64>()
                .map_err(|_| Reply::error(400, format!("{key} is a number")))
        })
        .transpose()
}

/// Undo the percent-encoding of one query value.
fn decoded(text: &str) -> String {
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
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `POST /api/derivatives`: the body is the file, and the query says what
/// it is: `kind`, one of `stack`, `series` or `subject` (with `day` for one
/// occasion), `sha256`, and optionally `supersedes`, `model` and `name`, whose
/// extension alone is kept. The media type is the request's content type.
/// The bytes are read only once the caller, the place and the ask pass.
pub(crate) fn upload(
    registry: &mut Registry,
    caller: &Caller,
    query: &std::collections::HashMap<String, String>,
    request: &mut tiny_http::Request,
) -> Result<Reply, Reply> {
    let (need, detail) = crate::serve::door("POST", &["api", "derivatives"]);
    caller.allowed("/api/derivatives", need, detail)?;
    let q: std::collections::HashMap<String, String> =
        query.iter().map(|(k, v)| (k.clone(), decoded(v))).collect();
    let header = |name: &str| -> Option<String> {
        request
            .headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let media_type = header("Content-Type")
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if let Some(n) = header("Content-Length").and_then(|n| n.trim().parse::<u64>().ok())
        && n > UPLOAD_MAX
    {
        return Err(Reply::error(
            413,
            format!("a derivative is at most {UPLOAD_MAX} bytes"),
        ));
    }
    let kind = q
        .get("kind")
        .cloned()
        .ok_or_else(|| Reply::error(400, "kind names what the file is"))?;
    let sha256 = q.get("sha256").cloned().ok_or_else(|| {
        Reply::error(
            400,
            "sha256 names the digest of the body, which the engine checks before it registers anything",
        )
    })?;
    let ask = Ask {
        kind: &kind,
        stack: int_of(&q, "stack")?,
        series: int_of(&q, "series")?,
        subject: int_of(&q, "subject")?,
        day: q.get("day").map(String::as_str),
        media_type: &media_type,
        supersedes: int_of(&q, "supersedes")?,
        model: q.get("model").map(String::as_str),
        name: q.get("name").map(String::as_str),
        sha256: Some(&sha256),
        principal: &caller.principal,
        actor: Some(&caller.actor),
    };
    let d = register(registry, &ask, request.as_reader())
        .map_err(|(status, m)| Reply::error(status, m))?;
    Ok(Reply::created(doc(registry.store(), &d)))
}

/// The reading doors: the listing, one derivative, and its bytes.
pub(crate) fn route(
    registry: &mut Registry,
    caller: &Caller,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get {
        return None;
    }
    let id_of = |s: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, "a derivative is named by its id"))
    };
    let one = |registry: &mut Registry, id: i64| -> Result<Derivative, Reply> {
        derivative::get(registry.store(), id)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .ok_or_else(|| Reply::error(404, format!("no derivative {id}")))
    };
    Some(match segs {
        ["api", "derivatives"] => (|| {
            let kind = query.get("kind").map(|k| decoded(k));
            let rows = derivative::list(
                registry.store(),
                &derivative::Filter {
                    kind: kind.as_deref(),
                    stack_id: int_of(query, "stack")?,
                    subject_id: int_of(query, "subject")?,
                    limit: query
                        .get("limit")
                        .and_then(|l| l.parse().ok())
                        .unwrap_or(50),
                },
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
            let store = registry.store();
            // the places the rows live in, each read once
            let mut places: std::collections::BTreeMap<i64, Option<String>> =
                std::collections::BTreeMap::new();
            for d in &rows {
                if let std::collections::btree_map::Entry::Vacant(e) = places.entry(d.place_id) {
                    e.insert(
                        place::show(store, d.place_id)
                            .ok()
                            .flatten()
                            .map(|p| p.name),
                    );
                }
            }
            let docs: Vec<Value> = rows
                .iter()
                .map(|d| doc_in(d, places.get(&d.place_id).cloned().flatten()))
                .collect();
            Ok(Reply::ok(json!({
                "derivatives": docs,
                "capability": capability(store),
            })))
        })(),
        ["api", "derivatives", id] => (|| {
            let d = one(registry, id_of(id)?)?;
            Ok(Reply::ok(doc(registry.store(), &d)))
        })(),
        ["api", "derivatives", id, "content"] => (|| {
            let d = one(registry, id_of(id)?)?;
            let path =
                file_of(registry.store(), &d).map_err(|(status, m)| Reply::error(status, m))?;
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal: &caller.principal,
                    action: nils_registry::audit::Action::DerivativeRead,
                    scope: json!({
                        "derivative": d.id, "kind": d.kind, "stack": d.stack_id,
                        "subject": d.subject_id, "bytes": d.bytes,
                    }),
                    policy: None,
                    job_id: None,
                    details: None,
                },
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
            Ok(Reply::file(
                &d.media_type,
                path,
                vec![
                    ("X-Nils-Sha256".to_string(), d.sha256.clone()),
                    ("X-Nils-Derivative".to_string(), d.id.to_string()),
                    ("Cache-Control".to_string(), "private, no-store".to_string()),
                ],
            ))
        })(),
        _ => return None,
    })
}

// ------------------------------------------------------------ command line

/// `nils derivative`: register, list and show derivatives.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum DerivativeCommand {
    /// Copy a file into the working place under its digest and register it
    Add {
        /// The file
        file: PathBuf,
        /// What it is: mask, embedding, pyramid or output
        #[arg(long, value_name = "KIND")]
        kind: String,
        /// The stack it belongs to
        #[arg(long, value_name = "ID")]
        stack: Option<i64>,
        /// The series it belongs to
        #[arg(long, value_name = "ID")]
        series: Option<i64>,
        /// The subject it belongs to, by id
        #[arg(long, value_name = "ID")]
        subject: Option<i64>,
        /// With --subject: the day of the occasion it belongs to
        #[arg(long, value_name = "YYYY-MM-DD", requires = "subject")]
        day: Option<String>,
        /// Its media type
        #[arg(
            long = "media-type",
            value_name = "TYPE",
            default_value = "application/octet-stream"
        )]
        media_type: String,
        /// The derivative this one replaces; both stay
        #[arg(long, value_name = "ID")]
        supersedes: Option<i64>,
        /// The registered model that made it: its id, digest or name@version
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// The digest the file must have, checked before anything is registered
        #[arg(long, value_name = "HEX")]
        sha256: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The derivatives, newest first
    List {
        /// Only this kind
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Only this stack's
        #[arg(long, value_name = "ID")]
        stack: Option<i64>,
        /// Only this subject's, by id
        #[arg(long, value_name = "ID")]
        subject: Option<i64>,
        /// At most this many
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One derivative: what it is, what it belongs to, where it lives and who made it
    Show {
        /// The derivative's id
        id: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn command(
    home: &nils_registry::home::Home,
    command: DerivativeCommand,
) -> Result<(), crate::Exit> {
    let mut registry = crate::open(home)?;
    let print = |v: &Value| -> Result<(), crate::Exit> {
        println!(
            "{}",
            serde_json::to_string_pretty(v)
                .map_err(|e| crate::fail(format!("will not serialize: {e}")))?
        );
        Ok(())
    };
    match command {
        DerivativeCommand::Add {
            file,
            kind,
            stack,
            series,
            subject,
            day,
            media_type,
            supersedes,
            model,
            sha256,
            json,
        } => {
            let who = crate::actor();
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
            let ask = Ask {
                kind: &kind,
                stack,
                series,
                subject,
                day: day.as_deref(),
                media_type: &media_type,
                supersedes,
                model: model.as_deref(),
                name: name.as_deref(),
                sha256: sha256.as_deref(),
                principal: &who,
                actor: None,
            };
            // everything but the bytes first, so a refusal reads nothing
            check(registry.store(), &ask).map_err(|(_, m)| crate::usage(m))?;
            let mut f = std::fs::File::open(&file)
                .map_err(|e| crate::usage(format!("{}: {e}", file.display())))?;
            let d = register(&mut registry, &ask, &mut f).map_err(|(status, m)| {
                if status >= 500 {
                    crate::fail(m)
                } else {
                    crate::usage(m)
                }
            })?;
            if json {
                return print(&doc(registry.store(), &d));
            }
            println!(
                "derivative {}: a {} of {} {}, {} bytes, sha256 {}",
                d.id,
                d.kind,
                d.scope,
                d.stack_id
                    .or(d.series_id)
                    .or(d.subject_id)
                    .unwrap_or_default(),
                d.bytes,
                d.sha256
            );
            Ok(())
        }
        DerivativeCommand::List {
            kind,
            stack,
            subject,
            limit,
            json,
        } => {
            let rows = derivative::list(
                registry.store(),
                &derivative::Filter {
                    kind: kind.as_deref(),
                    stack_id: stack,
                    subject_id: subject,
                    limit,
                },
            )
            .map_err(|e| crate::fail(e.to_string()))?;
            let store = registry.store();
            if json {
                let docs: Vec<Value> = rows.iter().map(|d| doc(store, d)).collect();
                return print(&json!(docs));
            }
            println!(
                "{:>6}  {:<9} {:<8} {:>8} {:>12}  sha256",
                "id", "kind", "scope", "of", "bytes"
            );
            for d in &rows {
                println!(
                    "{:>6}  {:<9} {:<8} {:>8} {:>12}  {}",
                    d.id,
                    d.kind,
                    d.scope,
                    d.stack_id
                        .or(d.series_id)
                        .or(d.subject_id)
                        .unwrap_or_default(),
                    d.bytes,
                    &d.sha256[..d.sha256.len().min(16)]
                );
            }
            match working(store) {
                Ok(p) => println!("{} derivatives; new ones go into {}", rows.len(), p.name),
                Err(m) => println!("{} derivatives; {m}", rows.len()),
            }
            Ok(())
        }
        DerivativeCommand::Show { id, json } => {
            let d = derivative::get(registry.store(), id)
                .map_err(|e| crate::fail(e.to_string()))?
                .ok_or_else(|| crate::usage(format!("no derivative {id}")))?;
            let v = doc(registry.store(), &d);
            if json {
                return print(&v);
            }
            println!("derivative {id}: a {}", d.kind);
            println!(
                "  belongs to       {} {}{}",
                d.scope,
                d.stack_id
                    .or(d.series_id)
                    .or(d.subject_id)
                    .unwrap_or_default(),
                d.session_day
                    .as_deref()
                    .map(|s| format!(" on {s}"))
                    .unwrap_or_default()
            );
            println!(
                "  lives in         {} at {}",
                v["place"].as_str().unwrap_or("a place that is gone"),
                d.path
            );
            println!("  bytes            {}", d.bytes);
            println!("  sha256           {}", d.sha256);
            println!("  media type       {}", d.media_type);
            println!(
                "  registered by    {} at {}",
                d.registered_by.as_deref().unwrap_or("-"),
                d.created_at
            );
            if let Some(old) = d.supersedes_id {
                println!("  supersedes       {old}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_extension_of_a_name_is_kept() {
        assert_eq!(extension(Some("mask.nii.gz")), ".nii.gz");
        assert_eq!(extension(Some("/a/b/SUBJ01_mask.NPY")), ".npy");
        assert_eq!(extension(Some("noext")), "");
        assert_eq!(extension(Some("a.b c")), "");
        assert_eq!(extension(Some("a..b")), "");
        assert_eq!(extension(None), "");
    }

    #[test]
    fn a_query_value_is_decoded() {
        assert_eq!(decoded("application%2Fx-nifti"), "application/x-nifti");
        assert_eq!(decoded("a+b"), "a b");
        assert_eq!(decoded("100%"), "100%");
        assert_eq!(decoded("%zz"), "%zz");
    }

    #[test]
    fn a_file_is_kept_under_its_digest_and_a_wrong_digest_keeps_nothing() {
        let root = nils_dicom::synth::TempDir::new("derivative-store");
        let body = b"a mask, as bytes".to_vec();
        let sha = hex::encode(ring::digest::digest(&ring::digest::SHA256, &body).as_ref());
        let stored = store_file(
            root.path(),
            "mask",
            Some("m.nii.gz"),
            &mut body.as_slice(),
            Some(&sha),
        )
        .unwrap();
        assert_eq!(stored.sha256, sha);
        assert_eq!(stored.bytes, body.len() as i64);
        assert_eq!(
            stored.path,
            format!("derivatives/mask/{}/{sha}.nii.gz", &sha[..2])
        );
        assert_eq!(std::fs::read(root.path().join(&stored.path)).unwrap(), body);
        // the same bytes again are the same file
        let again = store_file(
            root.path(),
            "mask",
            Some("m.nii.gz"),
            &mut body.as_slice(),
            None,
        )
        .unwrap();
        assert_eq!(again, stored);

        let wrong = "0".repeat(64);
        let e =
            store_file(root.path(), "mask", None, &mut &b"other"[..], Some(&wrong)).unwrap_err();
        assert!(matches!(e, StoreFail::Mismatch { .. }), "{e}");
        let incoming: Vec<_> = std::fs::read_dir(root.path().join("derivatives/.incoming"))
            .unwrap()
            .collect();
        assert!(incoming.is_empty(), "nothing is left half written");
        let masks: Vec<_> = std::fs::read_dir(root.path().join("derivatives/mask"))
            .unwrap()
            .collect();
        assert_eq!(masks.len(), 1, "only the first file is kept");
    }
}
