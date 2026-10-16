// SPDX-License-Identifier: AGPL-3.0-only
//! Gate T31, step 22.8 (record 55, ruling 35 (c), which brings into Wave 7a
//! what ruling 20 had left for 7b): the clinical layer's vocabulary and its
//! importer at doors, so the desk loads a vocabulary and imports clinical
//! events from a file a person chooses, as `nils clinical vocabulary load`
//! and `nils clinical import` do at the command line.
//!
//! `GET /api/clinical/vocabulary` lists what the registry holds;
//! `POST /api/clinical/vocabulary` loads a vocabulary's text, or the pack's
//! own `clinical/vocabulary.yml` when no text is sent; `POST
//! /api/clinical/import` previews an import of a CSV under a mapping, and
//! with `apply` writes it under the caller as one job. The grants are the
//! door table's: data:see to read, data:work to change. A mapping that names
//! its subjects by an identifier looks each up in the linkage store, so it
//! asks detail sensitive, as the linkage doors that read identifiers do;
//! the subject codes of the preview's rows are quasi-identifying and are
//! shown from detail quasi up. Every apply and every load is audited under
//! the caller (clinical.import, vocabulary.load).

use std::path::Path;

use nils_registry::Registry;
use nils_registry::import::{By, Mapping, Verdict};
use serde_json::{Value, json};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Reply, json_body};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 3] = [
    "GET /api/clinical/vocabulary",
    "POST /api/clinical/vocabulary",
    "POST /api/clinical/import",
];

/// Their rows of the policy table.
pub(crate) const POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/clinical/vocabulary",
        false,
        false,
        "free",
        "the vocabulary",
        "Reading the clinical vocabulary",
        "Read the clinical vocabulary",
    ),
    (
        "POST /api/clinical/vocabulary",
        true,
        true,
        "free",
        "one vocabulary",
        "Loading a clinical vocabulary",
        "Loaded a clinical vocabulary",
    ),
    (
        "POST /api/clinical/import",
        true,
        true,
        "bounded",
        "one file",
        "Importing clinical rows",
        "Imported clinical rows",
    ),
];

/// The grant and detail each door needs, for the door table.
pub(crate) fn door(method: &str, segs: &[&str]) -> Option<(Need, Detail)> {
    match (method, segs) {
        ("GET", ["api", "clinical", "vocabulary"]) => Some((Need::One("data:see"), Detail::Plain)),
        ("POST", ["api", "clinical", "vocabulary" | "import"]) => {
            Some((Need::One("data:work"), Detail::Plain))
        }
        _ => None,
    }
}

/// The doors under `/api/clinical`, or nothing when the path is not one.
pub(crate) fn route(
    registry: &mut Registry,
    pack_dir: Option<&Path>,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    Some(match (method, segs) {
        ("GET", ["api", "clinical", "vocabulary"]) => vocabulary(registry),
        ("POST", ["api", "clinical", "vocabulary"]) => load(registry, pack_dir, caller, body),
        ("POST", ["api", "clinical", "import"]) => import(registry, caller, body),
        _ => return None,
    })
}

fn internal(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

/// What the registry holds: the diseases with their types, and the kinds
/// of observation, as `nils clinical vocabulary list --json` answers.
fn vocabulary(registry: &mut Registry) -> Result<Reply, Reply> {
    let diseases = nils_registry::clinical::diseases(registry.store()).map_err(internal)?;
    let kinds = nils_registry::clinical::observation_types(registry.store()).map_err(internal)?;
    Ok(Reply::ok(json!({
        "diseases": diseases.iter().map(|(id, d)| json!({
            "id": id, "name": d.name, "code": d.code, "description": d.description,
            "types": d.types.iter().map(|t| json!({
                "name": t.name, "description": t.description,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "observation_types": kinds.iter().map(|k| json!({
            "id": k.id, "name": k.name, "category": k.category,
            "value_type": k.value_type, "unit": k.unit, "primary": k.primary,
            "sensitive": k.sensitive,
        })).collect::<Vec<_>>(),
    })))
}

/// `POST /api/clinical/vocabulary {text?, name?}`: the text of a vocabulary
/// file, or with none the pack's own, loaded by name: what exists is
/// updated, what is new is added, nothing is removed.
fn load(
    registry: &mut Registry,
    pack_dir: Option<&Path>,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = if body.trim().is_empty() {
        json!({})
    } else {
        json_body(body)?
    };
    let (text, from) = match doc["text"].as_str().filter(|t| !t.trim().is_empty()) {
        Some(t) => (
            t.to_string(),
            doc["name"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or("an uploaded file")
                .to_string(),
        ),
        None => {
            let Some(dir) = pack_dir else {
                return Err(Reply::error(
                    409,
                    "this engine reads no pack directory, so it has no vocabulary of its own; send the text of one",
                ));
            };
            let path = dir.join("clinical").join("vocabulary.yml");
            let text = std::fs::read_to_string(&path).map_err(|e| {
                Reply::error(
                    409,
                    format!("the pack's vocabulary could not be read ({e}); send the text of one"),
                )
            })?;
            (text, "the pack's clinical/vocabulary.yml".to_string())
        }
    };
    let vocabulary = nils_registry::clinical::Vocabulary::parse(&text)
        .map_err(|e| Reply::error(400, format!("the vocabulary: {e}")))?;
    let loaded = nils_registry::clinical::load(registry.store(), &vocabulary).map_err(internal)?;
    let counts = json!({
        "diseases": {"added": loaded.diseases_added, "updated": loaded.diseases_updated},
        "disease_types": {"added": loaded.disease_types_added, "updated": loaded.disease_types_updated},
        "observation_types": {"added": loaded.observation_types_added, "updated": loaded.observation_types_updated},
    });
    let mut scope = counts.clone();
    scope["file"] = json!(from);
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::VocabularyLoad,
            scope,
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map_err(internal)?;
    let mut out = counts;
    out["from"] = json!(from);
    out["changed"] = json!(loaded.changed());
    Ok(Reply::ok(out))
}

/// `POST /api/clinical/import {mapping, csv, apply?}`: the mapping's text
/// and the CSV's, previewed, or with `apply` written under the caller in one
/// transaction, idempotent on the key the mapping names.
fn import(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let Some(mapping_text) = doc["mapping"].as_str().filter(|t| !t.trim().is_empty()) else {
        return Err(Reply::error(
            400,
            "mapping is the text of the mapping: the target, how a row names its subject, the columns",
        ));
    };
    let Some(csv) = doc["csv"].as_str().filter(|t| !t.trim().is_empty()) else {
        return Err(Reply::error(400, "csv is the text of the file to import"));
    };
    let apply = match &doc["apply"] {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        _ => return Err(Reply::error(400, "apply is true or false")),
    };
    let mapping =
        Mapping::parse(mapping_text).map_err(|e| Reply::error(400, format!("the {e}")))?;
    // a row named by an identifier is looked up in the linkage store
    if mapping
        .subject
        .as_ref()
        .is_some_and(|s| s.by == By::Identifier)
    {
        caller.allowed(
            "an import whose rows name their subjects by an identifier",
            Need::One("data:work"),
            Detail::Sensitive,
        )?;
    }
    let who = caller.principal.clone();
    let report = if apply {
        nils_registry::import::apply(registry, &mapping, csv, &who)
    } else {
        nils_registry::import::preview(registry, &mapping, csv, &who)
    }
    .map_err(|e| match e {
        nils_registry::import::Error::Mapping(_) | nils_registry::import::Error::Csv(_) => {
            Reply::error(400, e.to_string())
        }
        other => internal(other),
    })?;
    let codes = caller.access.detail >= Detail::Quasi;
    Ok(Reply::ok(json!({
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
        "samples": report.samples.iter().map(|o| json!({
            "row": o.row,
            "subject": if codes { json!(o.subject) } else { Value::Null },
            "verdict": o.verdict.name(),
            "detail": match &o.verdict {
                Verdict::Refused(why) => Some(why.clone()),
                Verdict::Reviewed { field } => Some(field.clone()),
                _ => None,
            },
        })).collect::<Vec<_>>(),
    })))
}
