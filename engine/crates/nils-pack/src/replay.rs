// SPDX-License-Identifier: AGPL-3.0-only

//! A pack replayed over header packets (record 53, S3): what the pack decides
//! of a stack from the packet a rater reads, offline, with the private
//! elements the pack shows and the session list, so a replay exercises the
//! rules that read a vendor's private element and the session pass as
//! production does. `nils pack replay` is this, one packet a line.
//!
//! A packet is the reader's: `{stack, header: {texts, sequence, physics,
//! geometry, private}, session: [...]}`, or a flat `fields` object of the
//! pack's field names. The header's keys are the reader's names, which are
//! the fingerprint's except where the reader spells them for a person
//! ([`field_of`]). `header.private` holds the elements the pack shows, by
//! name; an element the pack does not list under `shown` is not read, and a
//! value shaped like an identifier is withheld, as the reader withholds it.
//! Each session entry is another stack of the study: `this_stack` marks the
//! packet's own and is passed over; `same_series` and
//! `same_frame_of_reference` are its relations to the packet's stack, which a
//! packet builder works out and never copies (a UID is not in a packet); its
//! fields are read as the header's are, and only those the pack's session
//! passes name are shown to them.
//!
//! What cannot be replayed from a packet, and is not: a vote among the
//! registry's stacks (the physics vote), which reads the whole archive.

use serde_json::{Map, Value, json};

use crate::session::{InForce, Sib, TIER};
use crate::stack::{FIELDS, Stack, Value as FieldValue};
use crate::{Evaluated, Pack};

/// The fingerprint field a packet's key names, where the reader spells it
/// otherwise; None where the key is no field.
pub fn field_of(key: &str) -> Option<&'static str> {
    let name = match key {
        "series_description" | "description" => "text_series_description",
        "protocol_name" | "protocol" => "text_protocol_name",
        "sequence_name" => "text_sequence_name",
        "body_part_examined" => "text_body_part",
        "image_comments" => "text_image_comments",
        "series_comments" => "text_series_comments",
        "number_of_temporal_positions" => "temporal_positions",
        "fov_x_mm" => "fov_x",
        "fov_y_mm" => "fov_y",
        other => other,
    };
    FIELDS.iter().copied().find(|f| *f == name)
}

/// A stack as a packet gives it: its fields, its shown private elements
/// aligned with `pack.ingest`, the names read and the names withheld.
type Given = (Stack, Vec<String>, Vec<String>, Vec<String>);

/// One stack as a packet gives it: the fingerprint's fields, and the shown
/// private elements aligned with `pack.ingest`. Also the names of the
/// private elements read, and of those withheld.
fn stack_of(
    pack: &Pack,
    groups: &[&Map<String, Value>],
    private: Option<&Map<String, Value>>,
) -> Result<Given, String> {
    let mut s = Stack::new();
    s.set("modality", FieldValue::Text(Some(&pack.modality)))?;
    let mut contrast: Vec<String> = Vec::new();
    for g in groups {
        for (k, v) in g.iter() {
            if k.starts_with("contrast_") {
                if let Some(t) = v.as_str().filter(|t| !t.is_empty()) {
                    contrast.push(t.to_string());
                }
                continue;
            }
            let Some(f) = field_of(k) else { continue };
            let text = match v {
                Value::Null => continue,
                Value::String(t) => t.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => continue,
            };
            s.set(f, FieldValue::Text(Some(&text)))?;
        }
    }
    if !contrast.is_empty() {
        s.set("text_contrast", FieldValue::Text(Some(&contrast.join(" "))))?;
    }
    let mut values = vec![String::new(); pack.ingest.len()];
    let (mut read, mut withheld) = (Vec::new(), Vec::new());
    if let Some(p) = private {
        for (name, v) in p {
            if !pack.shown.iter().any(|x| x.name == *name) {
                continue;
            }
            let Some(i) = pack.ingest.iter().position(|x| x.name == *name) else {
                continue;
            };
            let text = match v {
                Value::String(t) => t.clone(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            match crate::private::shown_value(&text) {
                Some(t) => {
                    values[i] = t.to_string();
                    read.push(name.clone());
                }
                None => withheld.push(name.clone()),
            }
        }
    }
    read.sort();
    withheld.sort();
    Ok((s, values, read, withheld))
}

/// What the pack decides of the stack of one packet.
pub fn replay(pack: &Pack, packet: &Value) -> Result<Value, String> {
    let empty = Map::new();
    let header = packet.get("header").and_then(Value::as_object);
    let mut groups: Vec<&Map<String, Value>> = Vec::new();
    for g in ["texts", "sequence", "physics", "geometry"] {
        if let Some(m) = header.and_then(|h| h.get(g)).and_then(Value::as_object) {
            groups.push(m);
        }
    }
    if let Some(f) = packet.get("fields").and_then(Value::as_object) {
        groups.push(f);
    }
    let private = header
        .and_then(|h| h.get("private"))
        .or_else(|| packet.get("private"))
        .and_then(Value::as_object);
    let (stack, private, read, withheld) = stack_of(pack, &groups, private)?;

    // The rules, as a classification runs them.
    let evaluated = Evaluated::with_private(pack, &stack, private.clone());
    // With its votes, which say where a rule decided an axis as nothing.
    let class = evaluated.classify_with_votes();
    let mut in_force: Vec<InForce> = pack
        .axes
        .iter()
        .map(|a| match class.axis(&a.name) {
            Some(v) => InForce {
                values: v.values.iter().filter(|x| !x.is_empty()).cloned().collect(),
                tier: v.tier.clone(),
                confidence: v.confidence,
            },
            None => InForce::default(),
        })
        .collect();

    // The session passes, in the pack's order, over the packet's session.
    let mut said = Vec::new();
    let entries: Vec<&Value> = packet
        .get("session")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    for pass in &pack.passes {
        let Some(session) = pass.session() else {
            continue;
        };
        let reads = crate::session::sibling_reads(session);
        let mut siblings = Vec::new();
        for (n, e) in entries.iter().enumerate() {
            let Some(m) = e.as_object() else { continue };
            if m.get("this_stack").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let mut g: Vec<&Map<String, Value>> = vec![m];
            if let Some(f) = m.get("fields").and_then(Value::as_object) {
                g.push(f);
            }
            let (full, full_private, _, _) = stack_of(
                pack,
                &g,
                m.get("private").and_then(Value::as_object).or(Some(&empty)),
            )?;
            let mut seen = Stack::new();
            let mut seen_private = vec![String::new(); pack.ingest.len()];
            let first_private = FIELDS.len() + pack.derived.len();
            for f in &reads {
                if *f < FIELDS.len() {
                    let t = full.as_text(*f).into_owned();
                    seen.set(FIELDS[*f], FieldValue::Text(Some(&t)))?;
                } else if *f >= first_private {
                    seen_private[f - first_private] = full_private[f - first_private].clone();
                }
            }
            siblings.push(Sib {
                id: m
                    .get("stack")
                    .and_then(Value::as_i64)
                    .unwrap_or(-(n as i64) - 1),
                stack: seen,
                private: seen_private,
                same_series: m.get("same_series").and_then(Value::as_bool) == Some(true),
                same_frame_of_reference: m.get("same_frame_of_reference").and_then(Value::as_bool),
            });
        }
        let Some(a) = crate::session::decide(
            pack,
            pass.target.as_ref(),
            session,
            &stack,
            &private,
            &in_force,
            &siblings,
        ) else {
            continue;
        };
        for (axis, values) in &a.writes {
            in_force[*axis] = InForce {
                values: values.clone(),
                tier: TIER.to_string(),
                confidence: a.confidence,
            };
        }
        said.push(json!({
            "pass": pass.name, "rule": a.rule, "cited": a.cited, "held": a.held,
            "writes": a.writes.iter().map(|(x, _)| pack.axes[*x].name.clone()).collect::<Vec<_>>(),
        }));
    }

    // The pack's constraints the answer breaks, as a classification reads
    // them (record 48): the class-phase axes in identities, and which rule
    // set decided each, a session pass's write counted as the pass's.
    let mut decided = std::collections::BTreeMap::new();
    for (i, a) in pack.axes.iter().enumerate() {
        if a.phase != crate::rules::AxisPhase::Class {
            continue;
        }
        let v: Vec<String> = in_force[i]
            .values
            .iter()
            .filter(|v| !v.is_empty() && a.default.as_deref() != Some(v.as_str()))
            .map(|v| {
                a.id_of_stored(v)
                    .map(str::to_string)
                    .unwrap_or_else(|| v.clone())
            })
            .collect();
        decided.insert(a.name.clone(), v);
    }
    let mut decided_by: std::collections::BTreeMap<String, String> = class
        .evidence
        .iter()
        .map(|e| (e.axis.clone(), e.rule_set.clone()))
        .collect();
    for s in &said {
        for w in s["writes"].as_array().into_iter().flatten() {
            if let (Some(axis), Some(pass)) = (w.as_str(), s["pass"].as_str()) {
                decided_by.insert(axis.to_string(), pass.to_string());
            }
        }
    }
    let constraints = class_constraints(pack);
    let broken: Vec<Value> = crate::legal::broken(pack, &constraints, &decided, &decided_by)
        .into_iter()
        .map(|b| json!({"kind": b.kind, "id": b.id}))
        .collect();

    // And what to do with the stack, from what was decided.
    let seed: Vec<Vec<String>> = in_force.iter().map(|a| a.values.clone()).collect();
    let dispose = evaluated.dispose(&seed);
    let mut values = Map::new();
    let mut tiers = Map::new();
    // The axes a rule decided as nothing, which an empty value alone cannot
    // tell from an axis no rule reached: a base of none is an answer (a
    // phase image has no contrast weighting), an empty base is a gap.
    let mut none = Vec::new();
    for (i, a) in pack.axes.iter().enumerate() {
        let (v, t) = match dispose.axis(&a.name) {
            Some(d) if a.phase == crate::rules::AxisPhase::Disposition => {
                (d.values.join(","), d.tier.clone())
            }
            _ => (in_force[i].values.join(","), in_force[i].tier.clone()),
        };
        if v.is_empty()
            && class
                .votes
                .iter()
                .any(|x| x.axis == a.name && x.value.is_empty())
        {
            none.push(a.name.clone());
        }
        values.insert(a.name.clone(), json!(v));
        tiers.insert(a.name.clone(), json!(t));
    }
    Ok(json!({
        "stack": packet.get("stack").cloned().unwrap_or(Value::Null),
        "values": values,
        "tiers": tiers,
        "none": none,
        "session": said,
        "private": read,
        "withheld": withheld,
        "broken": broken,
    }))
}

/// The pack's class constraints, built once per pack and kept: a replay of
/// the archive asks for them on every packet.
fn class_constraints(pack: &Pack) -> std::sync::Arc<Value> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Value>>>> = OnceLock::new();
    let key = format!("{}@{:p}", pack.id(), pack as *const Pack);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| Arc::new(crate::legal::class_constraints(pack)))
        .clone()
}
