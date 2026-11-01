// SPDX-License-Identifier: AGPL-3.0-only

//! Site participation (2026-11-01): an opt-in, default-closed channel by
//! which a site shares counts about its own install, and nothing else.
//!
//! - A site **takes part**; a report is **a file the site reads before it
//!   leaves**; a tier is **what the site shares**: tier 1 the counts, tier 3
//!   a sealed read. Nothing is sent that a person did not preview: the
//!   first send of each tier needs a preview on record, and later sends go
//!   by themselves for tier 1 alone, where the administrator turned `auto`
//!   on.
//! - A document carries no subject code, path, header text, stack id,
//!   person's name or key. Counts hold to the minimum cell of record 14
//!   (k = 5, [`crate::audit_cli::held_cells`]).
//! - Every document is signed (Ed25519, `ring`) with a key pair made when
//!   the site enabled participation: the public half in the registry, the
//!   private half a file of mode 600 in the registry home. The receiver
//!   verifies the signature against the site's public key and the bearer
//!   key against its sha256; it keeps neither key.
//!
//! `nils participate status | enable | preview | send | disable`, the doors
//! under `/api/participation`, and `nils participate receive`, the receiver
//! in the same binary.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use nils_registry::Registry;
use nils_registry::participation::{self as rows, Participation};
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Map, Value, json};

use crate::audit_cli::{MIN_CELL, held_cells};
use crate::campaigns::rerr;
use crate::grants::{Detail, Need};
use crate::serve::{Caller, Doors, Reply, json_body};
use crate::{Exit, Home, fail, usage};

/// The two kinds of document, by tier.
const KIND_COUNTS: &str = "nils participation counts";
const KIND_SEALED: &str = "nils participation sealed read";

/// The private half of the signing key, under the registry home.
const KEY_DIR: &str = "participation";
const KEY_FILE: &str = "signing.key";

/// The receiver's cap on a body, and the send's wait.
const BODY_CAP: usize = 2 * 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn sha256_hex(bytes: &[u8]) -> String {
    nils_registry::verify::sha256_hex(bytes)
}

// ------------------------------------------------------------ canonical JSON

/// A document with its keys sorted at every level, as one line: what is
/// signed and what is digested, the same bytes from any writer.
pub(crate) fn canonical(v: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(l) => Value::Array(l.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// The document without what a signature covers not: its signature and
/// signer.
fn unsigned(doc: &Value) -> Value {
    let mut d = doc.clone();
    if let Some(m) = d.as_object_mut() {
        m.remove("signature");
        m.remove("signed_by");
    }
    d
}

/// The content digest of a document: the sha256 of its canonical form
/// without the signature, the signer and the time it was made, so that
/// the same counts made again give the same digest, and a preview is good
/// for a send while nothing it counted moved.
pub(crate) fn content_digest(doc: &Value) -> String {
    let mut d = unsigned(doc);
    if let Some(m) = d.as_object_mut() {
        m.remove("made_at");
    }
    sha256_hex(canonical(&d).as_bytes())
}

// ------------------------------------------------------------ the key pair

/// The public key's fingerprint: the sha256 of its raw bytes, hex, the
/// first 16 characters.
pub(crate) fn fingerprint(public_key: &[u8]) -> String {
    sha256_hex(public_key)[..16].to_string()
}

fn key_path(registry: &Registry) -> PathBuf {
    registry.home().dir().join(KEY_DIR).join(KEY_FILE)
}

/// Make the install's signing key pair: the private half written to the
/// registry home at mode 600, the public half answered as base64. A pair
/// already there is kept, its public half read back.
fn make_or_read_key(path: &Path) -> Result<String, String> {
    if let Ok(bytes) = std::fs::read(path) {
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(&bytes)
            .map_err(|_| format!("{} does not hold a signing key", path.display()))?;
        return Ok(b64().encode(ring::signature::KeyPair::public_key(&pair).as_ref()));
    }
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| "the signing key could not be made".to_string())?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| "the signing key could not be read back".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write_private(path, pkcs8.as_ref()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(b64().encode(ring::signature::KeyPair::public_key(&pair).as_ref()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn load_key(path: &Path) -> Result<ring::signature::Ed25519KeyPair, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "the signing key {} cannot be read ({e}); nils participate disable and enable again makes a new pair",
            path.display()
        )
    })?;
    ring::signature::Ed25519KeyPair::from_pkcs8(&bytes)
        .map_err(|_| format!("{} does not hold a signing key", path.display()))
}

/// Sign a document in place: `signature`, base64 of the Ed25519 signature
/// over its canonical form without `signature` and `signed_by`, and
/// `signed_by`, the public key's fingerprint.
pub(crate) fn sign(doc: &mut Value, pair: &ring::signature::Ed25519KeyPair) {
    let message = canonical(&unsigned(doc));
    let signature = pair.sign(message.as_bytes());
    let public = ring::signature::KeyPair::public_key(pair).as_ref().to_vec();
    doc["signature"] = json!(b64().encode(signature.as_ref()));
    doc["signed_by"] = json!(fingerprint(&public));
}

/// Check a document's signature against a public key (raw bytes).
pub(crate) fn verify(doc: &Value, public_key: &[u8]) -> Result<(), String> {
    let signature = doc["signature"]
        .as_str()
        .ok_or("the document carries no signature")?;
    let signature = b64()
        .decode(signature)
        .map_err(|_| "the signature is not base64".to_string())?;
    let message = canonical(&unsigned(doc));
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(message.as_bytes(), &signature)
        .map_err(|_| "the signature does not verify".to_string())
}

// ------------------------------------------------------------ the documents

/// The versions of the parts, where known: the engine's own, the pack
/// served, and the desk, the assistant, Kvasir and the stations' model from
/// the install's record where there is one. Never an address.
fn versions(pack_dir: Option<&Path>, ask_pack: &str) -> Value {
    let mut v = json!({"engine": env!("CARGO_PKG_VERSION")});
    if let Some(p) = crate::reader::served_pack(pack_dir, ask_pack) {
        v["pack"] = json!({"name": p.name, "version": p.version.to_string()});
    }
    if let Some(state) = crate::setup::read_state() {
        for part in ["desk", "assistant", "kvasir"] {
            if let Some(s) = state.parts.get(part) {
                v[part] = json!(s.version);
            }
        }
        if let Some(m) = &state.model_server {
            v["model"] = json!({"runtime": "kvasir", "name": m.model});
        }
    }
    v
}

/// The counts of a list, every cell under the minimum held back with its
/// complement; the held cells' names are listed in `held` under `name`.
fn celled(name: &str, counts: &BTreeMap<String, u64>, held: &mut Vec<String>) -> Value {
    let hidden = held_cells(counts, false);
    let mut out = Map::new();
    for (k, n) in counts {
        if hidden.contains(k) {
            held.push(format!("{name}:{k}"));
            out.insert(k.clone(), Value::Null);
        } else {
            out.insert(k.clone(), json!(n));
        }
    }
    Value::Object(out)
}

/// One count, held back under the minimum.
fn total(n: u64) -> Value {
    if n < MIN_CELL { Value::Null } else { json!(n) }
}

/// A grouped count read from the store: label, count.
fn grouped(store: &mut Store, sql: &str) -> Result<BTreeMap<String, u64>, StoreError> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for r in store.query(sql, &[])? {
        let label = r
            .opt_text(0)?
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let n = r.int(1)? as u64;
        *out.entry(label).or_default() += n;
    }
    Ok(out)
}

fn count_of(store: &mut Store, sql: &str) -> Result<u64, StoreError> {
    Ok(store
        .query_opt(sql, &[])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0) as u64)
}

/// A word a document may carry as a label: lower case, one space between
/// words, only letters, digits and `. _ -`, at most 40 characters, so that
/// nothing shaped like an identifier rides along in a label; anything else
/// folds to `other`.
fn label_word(s: &str) -> String {
    let folded: String = s
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if folded.is_empty() {
        return "unknown".into();
    }
    if folded.len() > 40
        || !folded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
    {
        return "other".into();
    }
    folded
}

fn fold_labels(counts: BTreeMap<String, u64>, f: impl Fn(&str) -> String) -> BTreeMap<String, u64> {
    let mut out: BTreeMap<String, u64> = BTreeMap::new();
    for (k, n) in counts {
        *out.entry(f(&k)).or_default() += n;
    }
    out
}

/// Tier 1: the counts of the install's archive, answers, review, releases
/// and audit, every cell under the minimum held back.
fn tier1(
    registry: &mut Registry,
    p: &Participation,
    pack_dir: Option<&Path>,
    ask_pack: &str,
) -> Result<Value, Reply> {
    let registry_id = registry.meta().registry_id.clone();
    let versions = versions(pack_dir, ask_pack);
    let store = registry.store();
    let d = store.dialect();
    let fp = nils_registry::schema::table("stack_fingerprint");
    let (stack, series, fpt, axis_t, study, subject, release, release_stack, decision) = (
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
        store.qualified("classification_axis"),
        store.qualified("study"),
        store.qualified("subject"),
        store.qualified("release"),
        store.qualified("release_stack"),
        store.qualified("decision"),
    );
    let mut held: Vec<String> = Vec::new();
    // ---- the archive
    let by_modality = grouped(
        store,
        &format!(
            "SELECT k, COUNT(*) FROM (SELECT UPPER(COALESCE(NULLIF(x.modality, ''), f.modality, 'unknown')) AS k \
             FROM {stack} x LEFT JOIN {fpt} f ON f.stack_id = x.id) t GROUP BY k"
        ),
    )
    .map_err(failed)?;
    let by_vendor = fold_labels(
        grouped(
            store,
            &format!(
                "SELECT k, COUNT(*) FROM (SELECT f.manufacturer AS k FROM {stack} x LEFT JOIN {fpt} f ON f.stack_id = x.id) t GROUP BY k"
            ),
        )
        .map_err(failed)?,
        |m| {
            if m == "unknown" {
                "unknown".into()
            } else {
                nils_pipeline::pack::facts::vendor_of(m).to_string()
            }
        },
    );
    let by_model = fold_labels(
        grouped(
            store,
            &format!(
                "SELECT k, COUNT(*) FROM (SELECT f.manufacturer_model_name AS k FROM {stack} x LEFT JOIN {fpt} f ON f.stack_id = x.id) t GROUP BY k"
            ),
        )
        .map_err(failed)?,
        label_word,
    );
    // the acquisition year: the fingerprint's earliest acquisition date,
    // else the study's date, as the audit's frame reads it
    let study_t = nils_registry::schema::table("study");
    let year_expr = format!(
        "SUBSTR(COALESCE({}, {}), 1, 4)",
        d.text_of_qualified(
            Some("f"),
            fp.column("earliest_acquisition_date")
                .expect("earliest_acquisition_date"),
        ),
        d.text_of_qualified(
            Some("st"),
            study_t.column("study_date").expect("study_date")
        ),
    );
    let by_year = fold_labels(
        grouped(
            store,
            &format!(
                "SELECT k, COUNT(*) FROM (SELECT {year_expr} AS k FROM {stack} x \
                 JOIN {series} se ON se.id = x.series_id JOIN {study} st ON st.id = se.study_id \
                 LEFT JOIN {fpt} f ON f.stack_id = x.id) t GROUP BY k"
            ),
        )
        .map_err(failed)?,
        |y| {
            if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) {
                y.to_string()
            } else {
                "unknown".into()
            }
        },
    );
    let by_body_part = fold_labels(
        grouped(
            store,
            &format!(
                "SELECT a.value, COUNT(*) FROM {axis_t} a WHERE a.axis = 'body_part' GROUP BY a.value"
            ),
        )
        .map_err(failed)?,
        label_word,
    );
    let stacks = count_of(store, &format!("SELECT COUNT(*) FROM {stack}")).map_err(failed)?;
    let sessions = count_of(store, &format!("SELECT COUNT(*) FROM {study}")).map_err(failed)?;
    let subjects = count_of(store, &format!("SELECT COUNT(*) FROM {subject}")).map_err(failed)?;
    let archive = json!({
        "stacks": total(stacks),
        "sessions": total(sessions),
        "subjects": total(subjects),
        "by_modality": celled("archive.modality", &by_modality, &mut held),
        "by_vendor": celled("archive.vendor", &by_vendor, &mut held),
        "by_scanner_model": celled("archive.scanner_model", &by_model, &mut held),
        "by_year": celled("archive.year", &by_year, &mut held),
        "by_body_part": celled("archive.body_part", &by_body_part, &mut held),
    });
    // ---- the answers: per axis, by value, and the abstentions
    let mut per_axis: BTreeMap<String, (BTreeMap<String, u64>, u64)> = BTreeMap::new();
    for r in store
        .query(
            &format!("SELECT axis, value, COUNT(*) FROM {axis_t} GROUP BY axis, value"),
            &[],
        )
        .map_err(failed)?
    {
        let axis = label_word(r.text(0).map_err(failed)?);
        let n = r.int(2).map_err(failed)? as u64;
        let e = per_axis.entry(axis).or_default();
        match r
            .opt_text(1)
            .map_err(failed)?
            .filter(|v| !v.trim().is_empty())
        {
            Some(v) => *e.0.entry(label_word(v)).or_default() += n,
            None => e.1 += n,
        }
    }
    let mut answers = Map::new();
    for (axis, (values, abstained)) in &per_axis {
        answers.insert(
            axis.clone(),
            json!({
                "by_value": celled(&format!("answers.{axis}"), values, &mut held),
                "abstained": total(*abstained),
            }),
        );
    }
    // ---- the review: a person's corrections as (rules' value, person's
    // value) per axis, where a decision in force differs from the rules
    let mut corrections: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let sql = format!(
        "SELECT dc.axis, a.value, dc.value, COUNT(*) FROM {decision} dc \
         LEFT JOIN {axis_t} a ON a.axis = dc.axis AND CAST(a.stack_id AS TEXT) = dc.ref \
         WHERE dc.scope = 'stack' AND dc.author_kind = 'person' AND dc.withdrawn_at IS NULL \
         AND (dc.staged_at IS NULL OR dc.committed_at IS NOT NULL) \
         GROUP BY dc.axis, a.value, dc.value"
    );
    for r in store.query(&sql, &[]).map_err(failed)? {
        let axis = label_word(r.text(0).map_err(failed)?);
        let rules = r
            .opt_text(1)
            .map_err(failed)?
            .filter(|v| !v.trim().is_empty())
            .map_or_else(|| "none".to_string(), label_word);
        let person = r
            .opt_text(2)
            .map_err(failed)?
            .filter(|v| !v.trim().is_empty())
            .map_or_else(|| "none".to_string(), label_word);
        if rules == person {
            continue;
        }
        let n = r.int(3).map_err(failed)? as u64;
        *corrections
            .entry(axis)
            .or_default()
            .entry(format!("{rules} -> {person}"))
            .or_default() += n;
    }
    let mut review = Map::new();
    for (axis, pairs) in &corrections {
        review.insert(
            axis.clone(),
            json!({"corrections": celled(&format!("review.{axis}"), pairs, &mut held)}),
        );
    }
    // ---- the releases
    let releases_n = count_of(store, &format!("SELECT COUNT(*) FROM {release}")).map_err(failed)?;
    let layouts = fold_labels(
        grouped(
            store,
            &format!("SELECT layout, COUNT(*) FROM {release} GROUP BY layout"),
        )
        .map_err(failed)?,
        label_word,
    );
    let released_stacks =
        count_of(store, &format!("SELECT COUNT(*) FROM {release_stack}")).map_err(failed)?;
    let files = count_of(
        store,
        &format!("SELECT CAST(COALESCE(SUM(files), 0) AS BIGINT) FROM {release}"),
    )
    .map_err(failed)?;
    let failures: BTreeMap<String, u64> = [
        (
            "failed",
            count_of(
                store,
                &format!("SELECT COUNT(*) FROM {release} WHERE error IS NOT NULL AND NOT COALESCE(stopped, FALSE)"),
            )
            .map_err(failed)?,
        ),
        (
            "stopped",
            count_of(
                store,
                &format!("SELECT COUNT(*) FROM {release} WHERE COALESCE(stopped, FALSE)"),
            )
            .map_err(failed)?,
        ),
        (
            "withdrawn",
            count_of(
                store,
                &format!("SELECT COUNT(*) FROM {release} WHERE withdrawn_at IS NOT NULL"),
            )
            .map_err(failed)?,
        ),
    ]
    .into_iter()
    .map(|(k, n)| (k.to_string(), n))
    .collect();
    let releases = json!({
        "count": total(releases_n),
        "by_layout": celled("releases.layout", &layouts, &mut held),
        "stacks": total(released_stacks),
        "files": total(files),
        "failures": celled("releases.failures", &failures, &mut held),
    });
    // ---- the audit: rounds graded, the last one's wrong count and bound
    let graded = nils_registry::audit_draw::list(store, None)
        .map_err(failed)?
        .into_iter()
        .filter(|d| d.graded_at.is_some())
        .collect::<Vec<_>>();
    let last = graded.iter().max_by_key(|d| d.id);
    let audit = json!({
        "rounds_graded": graded.len(),
        "last": last.map(|d| json!({
            "wrong": d.grade["wrong"], "bound": d.grade["bound"], "read": d.grade["read"],
        })),
    });
    Ok(json!({
        "kind": KIND_COUNTS,
        "site": p.site,
        "registry_id": registry_id,
        "made_at": nils_registry::time::now_iso(),
        "versions": versions,
        "archive": archive,
        "answers": answers,
        "review": review,
        "releases": releases,
        "audit": audit,
        "min_cell": {"k": MIN_CELL, "rule": "record 14, D27: a cell under k is held back, and the smallest other cell of its list with it where it is held alone", "held_back": held},
        "trains": false,
    }))
}

/// Tier 3: the sealed read of a draw: the audit export document whole, the
/// draw's composition and the protocol it ran under.
fn tier3(
    registry: &mut Registry,
    p: &Participation,
    pack_dir: Option<&Path>,
    ask_pack: &str,
    draw: Option<i64>,
) -> Result<Value, Reply> {
    let export = crate::audit_cli::export_doc(registry, draw)
        .map_err(|e| Reply::error(if e.code == crate::USAGE { 409 } else { 500 }, e.message))?;
    let id = export["draw"]["id"]
        .as_i64()
        .ok_or_else(|| Reply::error(500, "the export names no draw"))?;
    let registry_id = registry.meta().registry_id.clone();
    let versions = versions(pack_dir, ask_pack);
    let store = registry.store();
    let d = nils_registry::audit_draw::get(store, id)
        .map_err(failed)?
        .ok_or_else(|| Reply::error(404, format!("no audit draw {id}")))?;
    let drawn = nils_registry::audit_draw::stacks(store, id).map_err(failed)?;
    let mut held: Vec<String> = Vec::new();
    let whole_held = (drawn.len() as u64) < MIN_CELL;
    let mut by_vendor: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_year: BTreeMap<String, u64> = BTreeMap::new();
    for f in &drawn {
        *by_vendor
            .entry(f.vendor.clone().unwrap_or_else(|| "unknown".into()))
            .or_default() += 1;
        *by_year
            .entry(
                f.year
                    .map_or_else(|| "unknown".to_string(), |y| y.to_string()),
            )
            .or_default() += 1;
    }
    // the scanner model and the field strength are read now, from the
    // fingerprint of the stacks drawn
    let (mut by_model, mut by_field): (BTreeMap<String, u64>, BTreeMap<String, u64>) =
        (BTreeMap::new(), BTreeMap::new());
    let ids: Vec<i64> = drawn.iter().map(|f| f.stack_id).collect();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT manufacturer_model_name, magnetic_field_strength FROM {} WHERE stack_id IN ({})",
            store.qualified("stack_fingerprint"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[]).map_err(failed)? {
            let model = r
                .opt_text(0)
                .map_err(failed)?
                .map_or_else(|| "unknown".to_string(), label_word);
            *by_model.entry(model).or_default() += 1;
            let field = r
                .opt_double(1)
                .map_err(failed)?
                .map_or_else(|| "unknown".to_string(), |t| format!("{t:.1}T"));
            *by_field.entry(field).or_default() += 1;
        }
    }
    let celled_whole = |name: &str, counts: &BTreeMap<String, u64>, held: &mut Vec<String>| {
        if whole_held {
            held.push(name.to_string());
            Value::Null
        } else {
            celled(name, counts, held)
        }
    };
    let composition = json!({
        "drawn": total(drawn.len() as u64),
        "by_vendor": celled_whole("composition.vendor", &by_vendor, &mut held),
        "by_scanner_model": celled_whole("composition.scanner_model", &by_model, &mut held),
        "by_year": celled_whole("composition.year", &by_year, &mut held),
        "by_field_strength": celled_whole("composition.field_strength", &by_field, &mut held),
    });
    // the protocol: model first where the draw had a raters' campaign,
    // else people; the settle's share; the planted catch from the verify
    // campaign's counts; how many people answered
    let path = if d.campaign_id.is_some() {
        "model-first"
    } else {
        "people"
    };
    let planted = match d.settle_campaign_id {
        Some(c) => {
            match nils_registry::campaign::get(store, c).map_err(crate::campaigns::campaign_err)? {
                Some(c) => {
                    let catch = nils_registry::verify::catch(store, &c)
                        .map_err(crate::campaigns::campaign_err)?;
                    json!({"planted": catch["planted"], "caught": catch["caught"]})
                }
                None => Value::Null,
            }
        }
        None => Value::Null,
    };
    // the people who read: those who answered the campaign a person
    // settles in (on the people path, the verify campaign itself); the
    // raters' campaign is the model's
    let mut readers: BTreeSet<String> = BTreeSet::new();
    if let Some(c) = d.settle_campaign_id {
        for a in nils_registry::campaign::current_answers(store, c)
            .map_err(crate::campaigns::campaign_err)?
        {
            if a.role == "rater" && a.author_kind == "person" {
                readers.insert(a.principal);
            }
        }
    }
    let mut doc = export;
    let m = doc.as_object_mut().expect("the export is an object");
    m.insert("kind".into(), json!(KIND_SEALED));
    m.insert("export_kind".into(), json!("nils audit export"));
    m.insert("site".into(), json!(p.site));
    m.insert("registry_id".into(), json!(registry_id));
    m.insert("made_at".into(), json!(nils_registry::time::now_iso()));
    m.insert("versions".into(), versions);
    m.insert("composition".into(), composition);
    m.insert(
        "protocol".into(),
        json!({
            "path": path,
            "share": d.settle_share,
            "planted": planted,
            "readers": readers.len(),
        }),
    );
    if let Some(list) = m
        .get_mut("min_cell")
        .and_then(|c| c["held_back"].as_array_mut())
    {
        list.extend(held.into_iter().map(Value::from));
    }
    m.insert("trains".into(), json!(false));
    Ok(doc)
}

/// The document of a tier, unsigned.
fn document(
    registry: &mut Registry,
    p: &Participation,
    pack_dir: Option<&Path>,
    ask_pack: &str,
    tier: i64,
    draw: Option<i64>,
) -> Result<Value, Reply> {
    if !p.tiers.contains(&tier) {
        return Err(Reply::error(
            409,
            format!(
                "this install does not share tier {tier}; nils participate enable --tier {tier} turns it on"
            ),
        ));
    }
    match tier {
        1 => tier1(registry, p, pack_dir, ask_pack),
        3 => tier3(registry, p, pack_dir, ask_pack, draw),
        _ => Err(Reply::error(
            400,
            "tier: 1 (the counts) or 3 (a sealed read)",
        )),
    }
}

fn tier_of(v: &Value) -> Result<i64, Reply> {
    match v.as_i64() {
        Some(t) if rows::TIERS.contains(&t) => Ok(t),
        _ => Err(Reply::error(
            400,
            "tier: 1 (the counts) or 3 (a sealed read); tier 2 is not built yet",
        )),
    }
}

fn taking_part(registry: &mut Registry) -> Result<Participation, Reply> {
    rows::get(registry.store())
        .map_err(failed)?
        .ok_or_else(|| Reply::error(409, "this install takes no part: nothing leaves it; nils participate enable --tier 1 turns the counts on"))
}

// ------------------------------------------------------------ status

/// Whether an address answers at all within a short wait: any HTTP answer
/// counts, a connection that fails does not.
fn reachable(url: &str) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(PROBE_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();
    agent.get(url).call().is_ok()
}

/// The status document: how the install takes part, the last send per
/// tier, and whether the receiver and the assistant's rater door answer.
fn status(registry: &mut Registry, assist: Option<&str>, probe: bool) -> Result<Value, Reply> {
    let p = rows::get(registry.store()).map_err(failed)?;
    let Some(p) = p else {
        return Ok(json!({
            "enabled": false,
            "tiers": [],
            "note": "Nothing leaves this install.",
            "assistant_rater_door": assist.filter(|_| probe).map(|a| reachable(&format!("{}/raters/runs/0", a.trim_end_matches('/')))),
        }));
    };
    let public = b64().decode(&p.public_key).unwrap_or_default();
    let last: BTreeMap<String, Value> = rows::last_sent(registry.store())
        .map_err(failed)?
        .into_iter()
        .map(|s| (s.tier.to_string(), s.as_json()))
        .collect();
    let receiver_reachable = p
        .receiver
        .as_deref()
        .filter(|_| probe)
        .map(|r| reachable(&format!("{}/healthz", r.trim_end_matches('/'))));
    let assistant = assist
        .filter(|_| probe)
        .map(|a| reachable(&format!("{}/raters/runs/0", a.trim_end_matches('/'))));
    Ok(json!({
        "enabled": true,
        "site": p.site,
        "tiers": p.tiers,
        "receiver": p.receiver,
        "key_file": p.key_file,
        "key_fingerprint": fingerprint(&public),
        "public_key": p.public_key,
        "auto": p.auto,
        "enabled_at": p.enabled_at,
        "enabled_by": p.enabled_by,
        "previewed": p.previewed,
        "last_send": last,
        "receiver_reachable": receiver_reachable,
        "assistant_rater_door": assistant,
    }))
}

// ------------------------------------------------------------ enable

/// What an enable changes; absent fields keep what is there.
#[derive(Debug, Default, Clone)]
pub(crate) struct Change {
    pub(crate) site: Option<String>,
    /// Tiers to turn on (added to the set), or the whole set.
    pub(crate) tiers: Vec<i64>,
    pub(crate) replace_tiers: bool,
    pub(crate) receiver: Option<String>,
    pub(crate) key_file: Option<PathBuf>,
    pub(crate) auto: Option<bool>,
}

fn site_word(s: &str) -> Result<String, Reply> {
    let s = s.trim();
    if s.is_empty()
        || s.len() > 64
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(Reply::error(
            400,
            "site: a name of letters, digits, dots, dashes and underscores, at most 64 characters",
        ));
    }
    Ok(s.to_string())
}

fn receiver_word(s: &str) -> Result<String, Reply> {
    let s = s.trim().trim_end_matches('/').to_string();
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return Err(Reply::error(
            400,
            "receiver: an address starting with http:// or https://",
        ));
    }
    Ok(s)
}

/// Read the bearer key from its file: one line, never printed, never kept.
fn read_key_file(path: &Path) -> Result<String, Reply> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Reply::error(
            400,
            format!(
                "no key could be read from {} ({e}); give --key-file the file the receiver's key is in",
                path.display()
            ),
        )
    })?;
    let key = text.trim();
    if key.is_empty() {
        return Err(Reply::error(
            400,
            format!(
                "{} is empty; give --key-file the file the receiver's key is in",
                path.display()
            ),
        ));
    }
    Ok(key.to_string())
}

/// Turn participation on, or change it.
pub(crate) fn enable(
    registry: &mut Registry,
    who: &str,
    c: &Change,
) -> Result<Participation, Reply> {
    let before = rows::get(registry.store()).map_err(failed)?;
    let site = match (&c.site, &before) {
        (Some(s), _) => site_word(s)?,
        (None, Some(p)) => p.site.clone(),
        (None, None) => registry.meta().registry_id.chars().take(8).collect(),
    };
    let mut tiers: BTreeSet<i64> = if c.replace_tiers {
        BTreeSet::new()
    } else {
        before.as_ref().map(|p| p.tiers.clone()).unwrap_or_default()
    };
    for t in &c.tiers {
        tiers.insert(tier_of(&json!(t))?);
    }
    if tiers.is_empty() {
        return Err(Reply::error(
            400,
            "tier: name what the site shares, 1 (the counts) or 3 (a sealed read)",
        ));
    }
    let receiver = match (&c.receiver, &before) {
        (Some(r), _) => Some(receiver_word(r)?),
        (None, Some(p)) => p.receiver.clone(),
        (None, None) => None,
    };
    let key_file = match (&c.key_file, &before) {
        (Some(f), _) => {
            let path = std::path::absolute(f).unwrap_or_else(|_| f.clone());
            read_key_file(&path)?;
            Some(path.display().to_string())
        }
        (None, Some(p)) => p.key_file.clone(),
        (None, None) => None,
    };
    let auto = c
        .auto
        .unwrap_or_else(|| before.as_ref().is_some_and(|p| p.auto));
    let public_key = match &before {
        Some(p) => p.public_key.clone(),
        None => make_or_read_key(&key_path(registry)).map_err(|e| Reply::error(500, e))?,
    };
    rows::enable(
        registry.store(),
        &rows::Enable {
            site: &site,
            tiers: &tiers,
            receiver: receiver.as_deref(),
            key_file: key_file.as_deref(),
            public_key: &public_key,
            auto,
            who,
        },
    )
    .map_err(failed)
}

/// Turn participation off, whole or a tier of it. Off whole, the signing
/// key goes with the row, so an enable later makes a new pair, which the
/// receiver's sites file is told of.
pub(crate) fn disable(
    registry: &mut Registry,
    tiers: Option<&BTreeSet<i64>>,
) -> Result<bool, Reply> {
    let gone = rows::disable(registry.store(), tiers).map_err(failed)?;
    if gone {
        let path = key_path(registry);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| Reply::error(500, format!("{}: {e}", path.display())))?;
        }
    }
    Ok(gone)
}

// ------------------------------------------------------------ preview and send

/// The preview of a tier: the document, signed as it would leave, its
/// content digest on record with the time.
pub(crate) fn preview(
    registry: &mut Registry,
    pack_dir: Option<&Path>,
    ask_pack: &str,
    tier: i64,
    draw: Option<i64>,
) -> Result<(Value, String, String), Reply> {
    let p = taking_part(registry)?;
    let mut doc = document(registry, &p, pack_dir, ask_pack, tier, draw)?;
    let pair = load_key(&key_path(registry)).map_err(|e| Reply::error(500, e))?;
    sign(&mut doc, &pair);
    let digest = content_digest(&doc);
    let draw = doc["draw"]["id"].as_i64();
    let at = rows::record_preview(registry.store(), tier, &digest, draw).map_err(failed)?;
    Ok((doc, digest, at))
}

/// What a send answers.
pub(crate) struct Posted {
    pub(crate) send: rows::Send,
    pub(crate) stored_as: Option<String>,
}

/// Post a document to the receiver: `POST <receiver>/v1/sites/<site>/reports`
/// with the bearer key from its file. Answers the receipt and the stored
/// name, or the error's words.
fn post(receiver: &str, site: &str, key: &str, body: &[u8]) -> Result<(String, String), String> {
    let url = format!("{}/v1/sites/{site}/reports", receiver.trim_end_matches('/'));
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| format!("{url}: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match status {
        200 | 201 => {
            let receipt = answer["receipt"]
                .as_str()
                .ok_or_else(|| format!("{url}: http status {status} without a receipt"))?;
            Ok((
                receipt.to_string(),
                answer["stored_as"].as_str().unwrap_or_default().to_string(),
            ))
        }
        _ => Err(format!(
            "{url}: http status {status}{}",
            answer["error"]
                .as_str()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        )),
    }
}

/// Send a tier's document. A person's send needs the document previewed
/// as it stands (its content digest on record); a send by `auto` needs
/// tier 1 previewed once, `auto` on, and no send by itself today. The
/// send is recorded whatever the receiver answered, with an audit row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn send(
    registry: &mut Registry,
    pack_dir: Option<&Path>,
    ask_pack: &str,
    who: &str,
    tier: i64,
    draw: Option<i64>,
    key_file: Option<&Path>,
    auto: bool,
) -> Result<Posted, Reply> {
    let p = taking_part(registry)?;
    let receiver = p.receiver.clone().ok_or_else(|| {
        Reply::error(
            409,
            "no receiver is set; nils participate enable --receiver <url> --key-file <file> names it",
        )
    })?;
    let key_path_given = key_file
        .map(Path::to_path_buf)
        .or_else(|| p.key_file.as_deref().map(PathBuf::from))
        .ok_or_else(|| {
            Reply::error(
                409,
                "no key file is set; nils participate enable --key-file <file> names the file the receiver's key is in",
            )
        })?;
    let key = read_key_file(&key_path_given)?;
    let previewed = p.preview_of(tier);
    // what a send by itself needs is known before any document is made
    if auto {
        if tier != 1 {
            return Err(Reply::error(
                409,
                "only the counts (tier 1) go by themselves; a sealed read is sent by a person",
            ));
        }
        if !p.auto {
            return Err(Reply::error(
                409,
                "auto is off: the counts go only when a person sends them; nils participate enable --tier 1 --auto turns it on",
            ));
        }
        if previewed.is_none() {
            return Err(Reply::error(
                409,
                "the counts were never previewed: nils participate preview --tier 1 first, so that a person has read the file before any send",
            ));
        }
        if rows::auto_sent_on(registry.store(), tier, &nils_registry::time::today())
            .map_err(failed)?
        {
            return Err(Reply::error(
                409,
                "the counts went by themselves today already; at most one send a day goes by itself, a person may send again",
            ));
        }
    }
    let mut doc = document(registry, &p, pack_dir, ask_pack, tier, draw)?;
    let pair = load_key(&key_path(registry)).map_err(|e| Reply::error(500, e))?;
    sign(&mut doc, &pair);
    let digest = content_digest(&doc);
    let previewed_at = match previewed {
        Some((_, at, _)) if auto => at,
        Some((d, at, _)) if d == digest => at,
        Some(_) => {
            return Err(Reply::error(
                409,
                format!(
                    "the tier {tier} document changed since it was previewed (the registry moved); nils participate preview --tier {tier} first, then send"
                ),
            ));
        }
        None => {
            return Err(Reply::error(
                409,
                format!(
                    "the tier {tier} document was never previewed; nils participate preview --tier {tier} first: nothing leaves that a person did not read"
                ),
            ));
        }
    };
    let body = canonical(&doc);
    let body_digest = sha256_hex(body.as_bytes());
    let posted = post(&receiver, &p.site, &key, body.as_bytes());
    let (receipt, stored_as, error) = match &posted {
        Ok((receipt, stored)) if *receipt == body_digest => {
            (Some(receipt.as_str()), Some(stored.clone()), None)
        }
        Ok((receipt, _)) => (
            None,
            None,
            Some(format!(
                "the receiver's receipt {receipt} is not the body's digest {body_digest}"
            )),
        ),
        Err(e) => (None, None, Some(e.clone())),
    };
    let by = if auto { "auto" } else { who };
    let id = rows::record_send(
        registry.store(),
        &rows::Sent {
            tier,
            digest: &body_digest,
            bytes: body.len() as i64,
            by,
            receiver: &receiver,
            receipt,
            error: error.as_deref(),
            previewed_at: Some(&previewed_at),
            draw: doc["draw"]["id"].as_i64(),
        },
    )
    .map_err(failed)?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: who,
            action: nils_registry::audit::Action::ParticipationSend,
            scope: json!({"tier": tier, "send": id, "digest": body_digest, "bytes": body.len(),
                          "site": p.site, "draw": doc["draw"]["id"]}),
            policy: None,
            job_id: None,
            details: Some(json!({"ok": error.is_none(), "error": error, "auto": auto})),
        },
    )
    .map_err(failed)?;
    let send = rows::sends(registry.store(), 1000)
        .map_err(failed)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| Reply::error(500, "the send was not kept"))?;
    Ok(Posted { send, stored_as })
}

// ------------------------------------------------------------ the doors

/// What the participation doors need: an administrator's grants, the ones
/// the settings' change takes.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    Some(match (method, segs) {
        ("GET", ["api", "participation"]) | ("GET", ["api", "participation", "sends"]) => {
            (Need::One("database:see"), Detail::Plain)
        }
        ("GET", ["api", "participation", "preview"])
        | ("POST", ["api", "participation"])
        | ("POST", ["api", "participation", "send"]) => (Need::One("database:work"), Detail::Plain),
        _ => return None,
    })
}

pub(crate) fn route(
    doors: &Doors,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    let pack_dir = doors.pack_dir.as_deref();
    let ask_pack = doors.ask_pack.as_str();
    Some(match segs {
        ["api", "participation"] if get => {
            status(registry, doors.assist.as_deref(), true).map(Reply::ok)
        }
        ["api", "participation"] if post => (|| {
            let doc = json_body(body)?;
            match &doc["disable"] {
                Value::Null | Value::Bool(false) => {}
                Value::Bool(true) => {
                    disable(registry, None)?;
                    return status(registry, doors.assist.as_deref(), false).map(Reply::ok);
                }
                Value::Array(l) => {
                    let tiers: BTreeSet<i64> = l.iter().map(tier_of).collect::<Result<_, _>>()?;
                    disable(registry, Some(&tiers))?;
                    return status(registry, doors.assist.as_deref(), false).map(Reply::ok);
                }
                _ => return Err(Reply::error(400, "disable: true, or the tiers to turn off")),
            }
            let tiers: Vec<i64> = match &doc["tiers"] {
                Value::Null => Vec::new(),
                Value::Array(l) => l.iter().map(tier_of).collect::<Result<_, _>>()?,
                _ => return Err(Reply::error(400, "tiers: a list, of 1 and 3")),
            };
            let auto = match &doc["auto"] {
                Value::Null => None,
                Value::Bool(b) => Some(*b),
                _ => return Err(Reply::error(400, "auto: true or false")),
            };
            enable(
                registry,
                &caller.principal,
                &Change {
                    site: doc["site"].as_str().map(str::to_string),
                    tiers,
                    replace_tiers: !doc["tiers"].is_null(),
                    receiver: doc["receiver"].as_str().map(str::to_string),
                    key_file: doc["key_file"].as_str().map(PathBuf::from),
                    auto,
                },
            )?;
            status(registry, doors.assist.as_deref(), false).map(Reply::ok)
        })(),
        ["api", "participation", "preview"] if get => (|| {
            let tier = tier_of(&json!(
                query
                    .get("tier")
                    .and_then(|t| t.parse::<i64>().ok())
                    .ok_or_else(|| Reply::error(400, "tier: 1 or 3"))?
            ))?;
            let draw = query.get("draw").and_then(|d| d.parse::<i64>().ok());
            let (doc, digest, at) = preview(registry, pack_dir, ask_pack, tier, draw)?;
            Ok(Reply::ok(json!({
                "document": doc, "digest": digest, "previewed_at": at, "tier": tier,
                "words": "This is the file. Nothing else leaves.",
            })))
        })(),
        ["api", "participation", "send"] if post => (|| {
            let doc = json_body(body)?;
            let tier = tier_of(&doc["tier"])?;
            let draw = doc["draw"].as_i64();
            let posted = send(
                registry,
                pack_dir,
                ask_pack,
                &caller.principal,
                tier,
                draw,
                None,
                false,
            )?;
            let mut v = posted.send.as_json();
            v["stored_as"] = json!(posted.stored_as);
            Ok(Reply::ok(v))
        })(),
        ["api", "participation", "sends"] if get => (|| {
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50);
            let sends: Vec<Value> = rows::sends(registry.store(), limit)
                .map_err(failed)?
                .iter()
                .map(rows::Send::as_json)
                .collect();
            Ok(Reply::ok(json!({"sends": sends})))
        })(),
        _ => return None,
    })
}

// ------------------------------------------------------------ the command

#[derive(Debug, clap::Subcommand)]
pub(crate) enum ParticipateCommand {
    /// How this install takes part: off by default, nothing leaves it
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Take part: turn a tier on (1, the counts; 3, a sealed read), name
    /// the site, the receiver and the file its key is read from; a signing
    /// key pair is made the first time
    Enable {
        /// What the site shares: 1 (the counts) or 3 (a sealed read); may
        /// be given twice
        #[arg(long, value_name = "N", required = true)]
        tier: Vec<i64>,
        /// The site's name (default: the registry id's first 8 characters)
        #[arg(long, value_name = "NAME")]
        site: Option<String>,
        /// The receiver's address
        #[arg(long, value_name = "URL")]
        receiver: Option<String>,
        /// The file the receiver's bearer key is read from; the key is
        /// never stored
        #[arg(long, value_name = "PATH")]
        key_file: Option<PathBuf>,
        /// Let the counts (tier 1) go by themselves, once a person has
        /// previewed them, at most once a day
        #[arg(long)]
        auto: bool,
        #[arg(long)]
        json: bool,
    },
    /// Write a tier's document, the file as it would leave, and record
    /// that a person saw it; a send needs this first
    Preview {
        #[arg(long, value_name = "N")]
        tier: i64,
        /// For a sealed read: the draw (default: the newest draw by count)
        #[arg(long, value_name = "ID")]
        draw: Option<i64>,
        /// Where the document is written (default: standard output)
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// The packs' directory, for the pack's name and version
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
    },
    /// Send a tier's document to the receiver; refused until it was
    /// previewed as it stands
    Send {
        #[arg(long, value_name = "N")]
        tier: i64,
        #[arg(long, value_name = "ID")]
        draw: Option<i64>,
        /// The file the receiver's key is read from, in place of the one
        /// enable recorded
        #[arg(long, value_name = "PATH")]
        key_file: Option<PathBuf>,
        /// A send by itself (the supervisor's timer): tier 1 only, auto on,
        /// previewed once, at most once a day
        #[arg(long)]
        auto: bool,
        /// The packs' directory, for the pack's name and version
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Stop taking part, whole or a tier of it
    Disable {
        /// Only this tier (default: everything, and the signing key with it)
        #[arg(long, value_name = "N")]
        tier: Vec<i64>,
    },
    /// The receiver: takes the sites' reports, checks the bearer key, the
    /// signature, the site and the kind, and keeps each once
    Receive(ReceiveArgs),
}

#[derive(Debug, clap::Args)]
pub(crate) struct ReceiveArgs {
    /// Where the reports are kept: one folder per site, a file per report
    #[arg(long, value_name = "DIR")]
    dir: PathBuf,
    /// The sites file (TOML): `[[sites]]` with `name`, `key_sha256` (the
    /// bearer key's sha256, never the key) and `public_key` (base64 Ed25519)
    #[arg(long, value_name = "FILE")]
    sites: PathBuf,
    /// The address to listen on
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:8490")]
    listen: String,
}

fn exit(r: Reply) -> Exit {
    rerr(r)
}

pub(crate) fn command(home: &Home, command: ParticipateCommand) -> Result<(), Exit> {
    match command {
        ParticipateCommand::Status { json } => {
            let mut registry = crate::open(home)?;
            let doc = status(&mut registry, None, false).map_err(exit)?;
            if json {
                crate::campaigns::print(&doc);
                return Ok(());
            }
            if doc["enabled"] != true {
                println!("off: nothing leaves this install");
                return Ok(());
            }
            println!(
                "site {}, tiers {}, receiver {}, key {}, auto {}",
                doc["site"].as_str().unwrap_or_default(),
                doc["tiers"],
                doc["receiver"].as_str().unwrap_or("none"),
                doc["key_fingerprint"].as_str().unwrap_or_default(),
                if doc["auto"] == true { "on" } else { "off" }
            );
            for (tier, s) in doc["last_send"].as_object().into_iter().flatten() {
                println!(
                    "tier {tier}: last sent {} ({} bytes, receipt {})",
                    s["sent_at"].as_str().unwrap_or_default(),
                    s["bytes"],
                    s["receipt"].as_str().unwrap_or("none")
                );
            }
            Ok(())
        }
        ParticipateCommand::Enable {
            tier,
            site,
            receiver,
            key_file,
            auto,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let who = crate::ask_cli::principal();
            enable(
                &mut registry,
                &who,
                &Change {
                    site,
                    tiers: tier,
                    replace_tiers: false,
                    receiver,
                    key_file,
                    auto: auto.then_some(true),
                },
            )
            .map_err(exit)?;
            let doc = status(&mut registry, None, false).map_err(exit)?;
            if json {
                crate::campaigns::print(&doc);
                return Ok(());
            }
            println!(
                "taking part as {}: tiers {}, key {}; next: nils participate preview --tier N, then send",
                doc["site"].as_str().unwrap_or_default(),
                doc["tiers"],
                doc["key_fingerprint"].as_str().unwrap_or_default()
            );
            Ok(())
        }
        ParticipateCommand::Preview {
            tier,
            draw,
            out,
            pack_dir,
        } => {
            let mut registry = crate::open(home)?;
            let pack_dir = crate::pack_dir(home, pack_dir).ok();
            let (doc, digest, at) =
                preview(&mut registry, pack_dir.as_deref(), "mri", tier, draw).map_err(exit)?;
            let text = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
            match out {
                Some(path) => {
                    std::fs::write(&path, text)
                        .map_err(|e| fail(format!("{}: {e}", path.display())))?;
                    println!(
                        "tier {tier} previewed at {at}, digest {digest}, written to {}; this is the file, nothing else leaves; next: nils participate send --tier {tier}",
                        path.display()
                    );
                }
                None => {
                    print!("{text}");
                    eprintln!(
                        "tier {tier} previewed at {at}, digest {digest}; this is the file, nothing else leaves; next: nils participate send --tier {tier}"
                    );
                }
            }
            Ok(())
        }
        ParticipateCommand::Send {
            tier,
            draw,
            key_file,
            auto,
            pack_dir,
            json,
        } => {
            let mut registry = crate::open(home)?;
            let pack_dir = crate::pack_dir(home, pack_dir).ok();
            let who = crate::ask_cli::principal();
            let posted = send(
                &mut registry,
                pack_dir.as_deref(),
                "mri",
                &who,
                tier,
                draw,
                key_file.as_deref(),
                auto,
            )
            .map_err(exit)?;
            let mut v = posted.send.as_json();
            v["stored_as"] = json!(posted.stored_as);
            if json {
                crate::campaigns::print(&v);
            } else {
                match &posted.send.error {
                    None => println!(
                        "sent tier {tier}: {} bytes, receipt {}, kept as {}",
                        posted.send.bytes,
                        posted.send.receipt.as_deref().unwrap_or_default(),
                        posted.stored_as.as_deref().unwrap_or_default()
                    ),
                    Some(e) => println!("tier {tier} not taken: {e}"),
                }
            }
            if posted.send.error.is_some() {
                return Err(fail("the send was recorded with its error"));
            }
            Ok(())
        }
        ParticipateCommand::Disable { tier } => {
            let mut registry = crate::open(home)?;
            let tiers: BTreeSet<i64> = tier.into_iter().collect();
            let gone = disable(
                &mut registry,
                if tiers.is_empty() { None } else { Some(&tiers) },
            )
            .map_err(exit)?;
            if gone {
                println!("off: nothing leaves this install; the signing key was removed");
            } else {
                println!("tiers {tiers:?} off");
            }
            Ok(())
        }
        ParticipateCommand::Receive(args) => receive(args),
    }
}

// ------------------------------------------------------------ the receiver

#[derive(Debug, serde::Deserialize)]
struct SitesFile {
    #[serde(default)]
    sites: Vec<Site>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Site {
    name: String,
    key_sha256: String,
    public_key: String,
}

fn read_sites(path: &Path) -> Result<BTreeMap<String, Site>, Exit> {
    let text =
        std::fs::read_to_string(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let file: SitesFile =
        toml::from_str(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let mut out = BTreeMap::new();
    for s in file.sites {
        if site_word(&s.name).is_err() {
            return Err(usage(format!(
                "{}: site {:?} is not a name of letters, digits, dots, dashes and underscores",
                path.display(),
                s.name
            )));
        }
        if s.key_sha256.len() != 64 || !s.key_sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(usage(format!(
                "{}: site {}: key_sha256 is the bearer key's sha256, 64 hex characters",
                path.display(),
                s.name
            )));
        }
        if b64().decode(&s.public_key).map(|k| k.len()).ok() != Some(32) {
            return Err(usage(format!(
                "{}: site {}: public_key is a base64 Ed25519 public key",
                path.display(),
                s.name
            )));
        }
        out.insert(s.name.clone(), s);
    }
    Ok(out)
}

/// Whether two byte strings are the same, read whole whatever the first
/// difference, so the compare's time says nothing of where they differ.
fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What the receiver answers one request with.
struct Answer {
    status: u16,
    body: Value,
}

fn refuse(status: u16, why: impl Into<String>) -> Answer {
    Answer {
        status,
        body: json!({"error": why.into()}),
    }
}

/// One report taken: checked and kept.
fn take_report(
    dir: &Path,
    sites: &BTreeMap<String, Site>,
    site: &str,
    bearer: Option<&str>,
    body: &[u8],
) -> Answer {
    let Some(s) = sites.get(site) else {
        return refuse(404, "no such site");
    };
    let Some(bearer) = bearer else {
        return refuse(401, "no bearer key");
    };
    let given = sha256_hex(bearer.trim().as_bytes());
    if !same_bytes(given.as_bytes(), s.key_sha256.to_lowercase().as_bytes()) {
        return refuse(401, "the bearer key is not this site's");
    }
    let doc: Value = match serde_json::from_slice(body) {
        Ok(Value::Object(m)) => Value::Object(m),
        _ => return refuse(400, "the body is not a JSON document"),
    };
    let tier = match doc["kind"].as_str() {
        Some(KIND_COUNTS) => 1,
        Some(KIND_SEALED) => 3,
        _ => {
            return refuse(
                400,
                "kind: nils participation counts, or nils participation sealed read",
            );
        }
    };
    if doc["site"].as_str() != Some(site) {
        return refuse(403, "the document's site is not the one posted to");
    }
    let public = b64().decode(&s.public_key).unwrap_or_default();
    if let Err(e) = verify(&doc, &public) {
        return refuse(403, e);
    }
    let digest = sha256_hex(body);
    let short = &digest[..8];
    let site_dir = dir.join(site);
    if let Err(e) = std::fs::create_dir_all(&site_dir) {
        return refuse(500, format!("the site's folder could not be made: {e}"));
    }
    // the same body again is answered as the first time, nothing written
    if let Ok(entries) = std::fs::read_dir(&site_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(&format!("-{short}.json")) {
                return Answer {
                    status: 200,
                    body: json!({"receipt": digest, "stored_as": name}),
                };
            }
        }
    }
    let day = doc["made_at"]
        .as_str()
        .and_then(|t| t.get(0..10))
        .filter(|d| d.len() == 10 && d.chars().all(|c| c.is_ascii_digit() || c == '-'))
        .unwrap_or("undated");
    let name = format!("{day}-tier{tier}-{short}.json");
    let path = site_dir.join(&name);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, body));
    if let Err(e) = written {
        return refuse(500, format!("the report could not be kept: {e}"));
    }
    eprintln!(
        "received site={site} tier={tier} digest={digest} bytes={} stored_as={name}",
        body.len()
    );
    Answer {
        status: 201,
        body: json!({"receipt": digest, "stored_as": name}),
    }
}

fn receive(args: ReceiveArgs) -> Result<(), Exit> {
    let sites = read_sites(&args.sites)?;
    std::fs::create_dir_all(&args.dir).map_err(|e| fail(format!("{}: {e}", args.dir.display())))?;
    let server = tiny_http::Server::http(args.listen.as_str())
        .map_err(|e| fail(format!("{}: {e}", args.listen)))?;
    println!(
        "nils participate receive listening on {} for {} sites, keeping reports in {}",
        server.server_addr(),
        sites.len(),
        args.dir.display()
    );
    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or_default().to_string();
        let method = request.method().to_string();
        let answer = if method == "GET" && path == "/healthz" {
            Answer {
                status: 200,
                body: Value::String("ok".into()),
            }
        } else if method == "POST"
            && let Some(site) = path
                .strip_prefix("/v1/sites/")
                .and_then(|rest| rest.strip_suffix("/reports"))
                .filter(|s| !s.contains('/'))
        {
            if request.body_length().is_some_and(|n| n > BODY_CAP) {
                refuse(413, "the body is over 2 MB")
            } else {
                let bearer = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
                    .map(str::to_string);
                let mut body = Vec::new();
                let read = request
                    .as_reader()
                    .take(BODY_CAP as u64 + 1)
                    .read_to_end(&mut body);
                match read {
                    Err(e) => refuse(400, format!("the body could not be read: {e}")),
                    Ok(_) if body.len() > BODY_CAP => refuse(413, "the body is over 2 MB"),
                    Ok(_) => take_report(&args.dir, &sites, site, bearer.as_deref(), &body),
                }
            }
        } else {
            refuse(404, "no such door")
        };
        let text = match &answer.body {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let content_type = if answer.body.is_string() {
            "text/plain"
        } else {
            "application/json"
        };
        let response = tiny_http::Response::from_string(text)
            .with_status_code(answer.status)
            .with_header(
                tiny_http::Header::from_bytes("Content-Type", content_type)
                    .expect("a content type header"),
            );
        let _ = request.respond(response);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let v = json!({"b": {"z": 1, "a": [{"y": 2, "x": 3}]}, "a": 0});
        assert_eq!(canonical(&v), r#"{"a":0,"b":{"a":[{"x":3,"y":2}],"z":1}}"#);
    }

    #[test]
    fn a_signed_document_verifies_and_a_changed_byte_does_not() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = ring::signature::KeyPair::public_key(&pair)
            .as_ref()
            .to_vec();
        let mut doc = json!({"kind": KIND_COUNTS, "site": "lab", "archive": {"stacks": 172}});
        sign(&mut doc, &pair);
        assert_eq!(doc["signed_by"], json!(fingerprint(&public)));
        assert_eq!(doc["signed_by"].as_str().unwrap().len(), 16);
        assert!(verify(&doc, &public).is_ok());
        // the same document with its keys in another order verifies too
        let text = serde_json::to_string(&doc).unwrap();
        let again: Value = serde_json::from_str(&text).unwrap();
        assert!(verify(&again, &public).is_ok());
        // one count changed, and the signature no longer holds
        let mut changed = doc.clone();
        changed["archive"]["stacks"] = json!(173);
        assert!(verify(&changed, &public).is_err());
        // another key does not verify it
        let other = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let other = ring::signature::Ed25519KeyPair::from_pkcs8(other.as_ref()).unwrap();
        let other_public = ring::signature::KeyPair::public_key(&other)
            .as_ref()
            .to_vec();
        assert!(verify(&doc, &other_public).is_err());
    }

    #[test]
    fn the_content_digest_ignores_the_time_and_the_signature() {
        let a = json!({"kind": KIND_COUNTS, "made_at": "2026-11-01T10:00:00Z", "x": 1,
                       "signature": "s", "signed_by": "f"});
        let b = json!({"kind": KIND_COUNTS, "made_at": "2026-11-02T10:00:00Z", "x": 1});
        assert_eq!(content_digest(&a), content_digest(&b));
        let c = json!({"kind": KIND_COUNTS, "x": 2});
        assert_ne!(content_digest(&a), content_digest(&c));
    }

    #[test]
    fn labels_fold_to_safe_words() {
        assert_eq!(label_word("  Prisma   Fit "), "prisma fit");
        assert_eq!(label_word("Skyra"), "skyra");
        assert_eq!(label_word(""), "unknown");
        assert_eq!(label_word("Doe^John 19500101"), "other");
        assert_eq!(label_word(&"x".repeat(41)), "other");
    }

    #[test]
    fn the_receiver_takes_a_report_once_and_refuses_the_rest() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = b64().encode(ring::signature::KeyPair::public_key(&pair).as_ref());
        let key = "a-site-key-of-some-length";
        let sites: BTreeMap<String, Site> = [(
            "lab".to_string(),
            Site {
                name: "lab".into(),
                key_sha256: sha256_hex(key.as_bytes()),
                public_key: public,
            },
        )]
        .into();
        let dir = nils_dicom::synth::TempDir::new("participation-receiver");
        let mut doc = json!({"kind": KIND_COUNTS, "site": "lab", "made_at": "2026-11-01T10:00:00Z",
                             "archive": {"stacks": 172}, "trains": false});
        sign(&mut doc, &pair);
        let body = canonical(&doc);
        // accepted, kept once
        let a = take_report(dir.path(), &sites, "lab", Some(key), body.as_bytes());
        assert_eq!(a.status, 201, "{}", a.body);
        assert_eq!(a.body["receipt"], json!(sha256_hex(body.as_bytes())));
        let name = a.body["stored_as"].as_str().unwrap().to_string();
        assert!(name.starts_with("2026-11-01-tier1-"), "{name}");
        assert!(dir.path().join("lab").join(&name).exists());
        // the same body again: 200, the same receipt, nothing more written
        let again = take_report(dir.path(), &sites, "lab", Some(key), body.as_bytes());
        assert_eq!(again.status, 200, "{}", again.body);
        assert_eq!(again.body["receipt"], a.body["receipt"]);
        assert_eq!(
            std::fs::read_dir(dir.path().join("lab")).unwrap().count(),
            1
        );
        // a bad key
        let bad = take_report(
            dir.path(),
            &sites,
            "lab",
            Some("not-the-key"),
            body.as_bytes(),
        );
        assert_eq!(bad.status, 401, "{}", bad.body);
        let none = take_report(dir.path(), &sites, "lab", None, body.as_bytes());
        assert_eq!(none.status, 401, "{}", none.body);
        // a bad signature: one count changed after signing
        let mut changed = doc.clone();
        changed["archive"]["stacks"] = json!(173);
        let changed = canonical(&changed);
        let bad = take_report(dir.path(), &sites, "lab", Some(key), changed.as_bytes());
        assert_eq!(bad.status, 403, "{}", bad.body);
        // an unknown site
        let bad = take_report(dir.path(), &sites, "other", Some(key), body.as_bytes());
        assert_eq!(bad.status, 404, "{}", bad.body);
        // a document posted to another site's door than its own
        let mut elsewhere =
            json!({"kind": KIND_COUNTS, "site": "other", "made_at": "2026-11-01T10:00:00Z"});
        sign(&mut elsewhere, &pair);
        let bad = take_report(
            dir.path(),
            &sites,
            "lab",
            Some(key),
            canonical(&elsewhere).as_bytes(),
        );
        assert_eq!(bad.status, 403, "{}", bad.body);
        // a kind the receiver does not take
        let mut other = json!({"kind": "something else", "site": "lab"});
        sign(&mut other, &pair);
        let bad = take_report(
            dir.path(),
            &sites,
            "lab",
            Some(key),
            canonical(&other).as_bytes(),
        );
        assert_eq!(bad.status, 400, "{}", bad.body);
        // still one file
        assert_eq!(
            std::fs::read_dir(dir.path().join("lab")).unwrap().count(),
            1
        );
    }
}
