// SPDX-License-Identifier: AGPL-3.0-only

//! The reader (record 48 R1): what a person rating a stack sees in one line
//! per axis, the answer the engine suggests, and batches of stacks that look
//! the same and are suggested the same answer.
//!
//! - **The evidence line.** Per axis: the value in force and who set it; the
//!   rule and the clause that decided, read from the evidence and the votes;
//!   the other rules that voted, each with its value; the header values the
//!   deciding clause read, as the pack says which (`nils_pack::reads`); the
//!   words it matched, at detail quasi or above only, since they are text a
//!   stack carried; and System 1's candidates where it asked about the
//!   stack (`classify.asked`).
//! - **The suggestion.** The answer the rules and System 1 agree on; the
//!   rules' own where System 1 has not asked; none where they disagree, so
//!   both candidates show and nothing is filled in.
//! - **Batches.** A campaign's open items grouped by the signature of the
//!   deciding physics (the deciding rules and the header values their
//!   clauses read, with the sequence the stack was made with) and the
//!   suggested answer. An item of a sealed sample is never in a batch.

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::campaign::{self, Campaign, Question};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Cell, Error as StoreError, Param, Store};
use serde_json::{Value, json};

/// The served pack, loaded once per process and again when its `pack.yml`
/// changes: the reader's doors are asked for every stack a person reads,
/// and loading a pack judges its whole corpus.
pub(crate) fn served_pack(
    dir: Option<&std::path::Path>,
    name: &str,
) -> Option<std::sync::Arc<nils_pack::Pack>> {
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache =
        Mutex<BTreeMap<std::path::PathBuf, (Option<std::time::SystemTime>, Arc<nils_pack::Pack>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let path = dir?.join(name);
    let stamp = std::fs::metadata(path.join("pack.yml"))
        .and_then(|m| m.modified())
        .ok();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(held) = cache.lock()
        && let Some((when, pack)) = held.get(&path)
        && *when == stamp
    {
        return Some(pack.clone());
    }
    let pack = Arc::new(nils_pack::load(&path, None).ok()?);
    if let Ok(mut held) = cache.lock() {
        held.insert(path, (stamp, pack.clone()));
    }
    Some(pack)
}

/// The header values a reader is shown for every stack: the physics a
/// sequence is recognised by, what the scanner says it made, and its shape.
/// Columns of the fingerprint; none of them is text a person typed.
pub(crate) const CORE: &[(&str, &str)] = &[
    ("repetition_time", "TR"),
    ("echo_time", "TE"),
    ("inversion_time", "TI"),
    ("flip_angle", "flip"),
    ("text_sequence_name", "sequence"),
    ("image_type", "image type"),
    ("scanning_sequence", "scanning sequence"),
    ("sequence_variant", "sequence variant"),
    ("scan_options", "scan options"),
    ("orientation", "orientation"),
    ("n_slices", "slices"),
];

/// Other header values a deciding clause may read, shown when it does.
const MORE: &[(&str, &str)] = &[
    ("echo_train_length", "echo train"),
    ("mr_acquisition_type", "acquisition"),
    ("magnetic_field_strength", "field"),
    ("field_strength_normalized", "field"),
    ("diffusion_b_value", "b"),
    ("dwi_b_values", "b values"),
    ("dwi_directions", "directions"),
    ("slice_thickness", "thickness"),
    ("spacing_between_slices", "spacing"),
    ("n_instances", "images"),
    ("modality", "modality"),
    ("manufacturer", "manufacturer"),
    ("echo_numbers", "echoes"),
    ("number_of_averages", "averages"),
    ("pixel_bandwidth", "bandwidth"),
    ("image_role", "role"),
    ("acquisition_type_filled", "acquisition"),
];

/// The fields a batch's signature always holds, beside those the deciding
/// clauses read: the sequence the stack was made with.
const SAME_SEQUENCE: &[&str] = &[
    "text_sequence_name",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "mr_acquisition_type",
];

fn shown(field: &str) -> Option<&'static str> {
    CORE.iter()
        .chain(MORE)
        .find(|(f, _)| *f == field)
        .map(|(_, l)| *l)
}

pub(crate) fn cell_json(c: &Cell) -> Value {
    match c {
        Cell::Null => Value::Null,
        Cell::Text(s) if s.is_empty() => Value::Null,
        Cell::Text(s) => json!(s),
        Cell::Int(i) => json!(i),
        Cell::Double(d) => json!(d),
        Cell::Bool(b) => json!(b),
        Cell::Bytes(_) => Value::Null,
    }
}

/// The header values of stacks, the fields shown by name, read five hundred
/// stacks at a time.
pub(crate) fn header(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, BTreeMap<String, Value>>, StoreError> {
    let t = table("stack_fingerprint");
    let d = store.dialect();
    let fields: Vec<&str> = CORE
        .iter()
        .chain(MORE)
        .map(|(f, _)| *f)
        .filter(|f| t.column(f).is_some())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let list = fields
        .iter()
        .map(|f| d.text_of(t.column(f).expect("a fingerprint column")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let ids = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, {list} FROM {} WHERE stack_id IN ({ids})",
            store.qualified("stack_fingerprint")
        );
        for r in store.query(&sql, &[])? {
            let mut m = BTreeMap::new();
            for (i, f) in fields.iter().enumerate() {
                m.insert(f.to_string(), cell_json(r.get(i + 1)));
            }
            out.insert(r.int(0)?, m);
        }
    }
    Ok(out)
}

/// A voter of the vote table: where it sits in the pack.
#[derive(Debug, Clone)]
struct Voter {
    rule_set: String,
    rule: String,
    clause: i64,
    axis: String,
    tier: String,
    restates: bool,
}

fn voters(store: &mut Store) -> Result<BTreeMap<i64, Voter>, StoreError> {
    let sql = format!(
        "SELECT id, rule_set, rule, clause, axis, tier, restates FROM {}",
        store.qualified("classification_voter")
    );
    let mut out = BTreeMap::new();
    for r in store.query(&sql, &[])? {
        out.insert(
            r.int(0)?,
            Voter {
                rule_set: r.text(1)?.to_string(),
                rule: r.text(2)?.to_string(),
                clause: r.int(3)?,
                axis: r.text(4)?.to_string(),
                tier: r.text(5)?.to_string(),
                restates: r.int(6)? != 0,
            },
        );
    }
    Ok(out)
}

/// One vote of a stack: the voter and the value it said.
type Said = (Voter, String);

fn votes_of(
    store: &mut Store,
    voters: &BTreeMap<i64, Voter>,
    stack: i64,
) -> Result<Vec<Said>, StoreError> {
    let sql = format!(
        "SELECT votes FROM {} WHERE stack_id = {} ORDER BY phase",
        store.qualified("classification_vote"),
        store.dialect().param(1, Type::Int)
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(stack)])? {
        let pairs: Vec<(i64, String)> = serde_json::from_str(r.text(0)?).unwrap_or_default();
        for (id, value) in pairs {
            if let Some(v) = voters.get(&id) {
                out.push((v.clone(), value));
            }
        }
    }
    Ok(out)
}

/// The open `classify.asked` item of a stack: its id and evidence.
fn asked_of(store: &mut Store, stack: i64) -> Result<Option<(i64, Value)>, StoreError> {
    let t = table("review_item");
    let d = store.dialect();
    let sql = format!(
        "SELECT id, {} FROM {} WHERE kind = {} AND group_key = {} AND status = 'open' ORDER BY id DESC",
        d.text_of(t.column("evidence").expect("evidence")),
        store.qualified("review_item"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let row = store.query_opt(
        &sql,
        &[
            Param::from(nils_registry::asked::KIND),
            Param::from(nils_registry::asked::key(stack)),
        ],
    )?;
    Ok(match row {
        Some(r) => Some((
            r.int(0)?,
            r.opt_text(1)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null),
        )),
        None => None,
    })
}

/// A value as the pack names it (its identity), from any name it goes by;
/// the value itself where the pack does not know it.
fn named(names: &BTreeMap<String, String>, v: &str) -> String {
    names.get(v).cloned().unwrap_or_else(|| v.to_string())
}

/// A candidate's value of one axis as one text: a value, a sorted list
/// joined by commas, or the empty text for none.
fn one_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(list) => {
            let mut w: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
            w.sort_unstable();
            w.join(",")
        }
        _ => String::new(),
    }
}

/// System 1's candidates on one axis: each value with the sum of the
/// probabilities of the joint candidates that name it, most probable first.
fn s1_on(asked: &Value, axis: &str) -> Option<Vec<Value>> {
    let axes = asked["axes"].as_array()?;
    if !axes.iter().any(|a| a.as_str() == Some(axis)) {
        return None;
    }
    let mut p: BTreeMap<String, f64> = BTreeMap::new();
    for c in asked["candidates"].as_array().into_iter().flatten() {
        *p.entry(one_text(&c["values"][axis])).or_insert(0.0) += c["p"].as_f64().unwrap_or(0.0);
    }
    let mut list: Vec<(String, f64)> = p.into_iter().collect();
    list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Some(
        list.into_iter()
            .map(|(v, p)| {
                json!({"value": if v.is_empty() { Value::Null } else { json!(v) }, "p": (p * 1e6).round() / 1e6})
            })
            .collect(),
    )
}

/// An axis's values in force, as a reader is shown one: the value, a list
/// for several, null for none.
fn value_json(values: &[String]) -> Value {
    match values {
        [] => Value::Null,
        [one] if one.is_empty() => Value::Null,
        [one] => json!(one),
        many => json!(many),
    }
}

/// A number as a short text: no trailing zeros.
fn short(v: &Value) -> String {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
            Some(f) => format!("{}", (f * 100.0).round() / 100.0),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The evidence line of one stack, one entry per axis (record 48 R1); none
/// when the stack has not been classified. `quasi` says whether the caller
/// reads at detail quasi or above, where the words a rule matched and why a
/// person decided are shown; below it neither is.
pub(crate) fn why(
    store: &mut Store,
    stack: i64,
    pack: Option<&nils_pack::Pack>,
    quasi: bool,
    hidden: bool,
) -> Result<Option<Value>, StoreError> {
    if hidden {
        return blind_doc(store, stack, quasi).map(Some);
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT pack, pack_version FROM {} WHERE stack_id = {}",
        store.qualified("classification"),
        d.param(1, Type::Int)
    );
    let Some(m) = store.query_opt(&sql, &[Param::Int(stack)])? else {
        return Ok(None);
    };
    let (pack_name, pack_version) = (m.text(0)?.to_string(), m.text(1)?.to_string());
    // the pack the reader is served, when it is the one that judged
    let pack = pack.filter(|p| p.name == pack_name);
    let same_version = pack.is_some_and(|p| p.version.to_string() == pack_version);

    let sql = format!(
        "SELECT axis, value, confidence, tier FROM {} WHERE stack_id = {} ORDER BY axis, value",
        store.qualified("classification_axis"),
        d.param(1, Type::Int)
    );
    let mut axes: BTreeMap<String, (Vec<String>, f64, String)> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(stack)])? {
        let e = axes
            .entry(r.text(0)?.to_string())
            .or_insert_with(|| (Vec::new(), 1.0, String::new()));
        if let Some(v) = r.opt_text(1)? {
            e.0.push(v.to_string());
        }
        let c = r.double(2)?;
        if e.2.is_empty() || c < e.1 {
            e.1 = c;
            e.2 = r.text(3)?.to_string();
        }
    }
    let sql = format!(
        "SELECT axis, value, tier, rule_set, rule, source, matched, author, author_kind, model_id \
         FROM {} WHERE stack_id = {} ORDER BY axis, id",
        store.qualified("classification_evidence"),
        d.param(1, Type::Int)
    );
    let mut evidence: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(stack)])? {
        evidence
            .entry(r.text(0)?.to_string())
            .or_default()
            .push(json!({
                "value": r.text(1)?, "tier": r.text(2)?, "rule_set": r.text(3)?,
                "rule": r.text(4)?, "source": r.text(5)?, "matched": r.opt_text(6)?,
                "author": r.opt_text(7)?, "author_kind": r.opt_text(8)?,
                "model_id": r.opt_int(9)?,
            }));
    }
    let voters = voters(store)?;
    let votes = votes_of(store, &voters, stack)?;
    let decisions = crate::explain::decisions_of(store, stack)?;
    let blind = false;
    let asked = asked_of(store, stack)?;
    let mut head = header(store, &[stack])?.remove(&stack).unwrap_or_default();
    // the sequence name is quasi-identifying text (catalogue.md): below
    // detail quasi it is not shown, in the header, a clause's or the line
    if !quasi {
        head.retain(|f, _| !HASHED_ONLY.contains(&f.as_str()));
    }

    let mut out_axes = Vec::new();
    for (axis, (values, confidence, tier)) in &axes {
        let names = crate::campaigns::value_names(pack, Some(axis));
        let ids: Vec<String> = values.iter().map(|v| named(&names, v)).collect();
        let rows = evidence.get(axis).cloned().unwrap_or_default();
        // who set the value: a person, an agent or a model where one did,
        // else the rules
        let author = rows.iter().find(|e| e["author_kind"].is_string());
        let set_by = match author {
            Some(e) => {
                let held = decisions.iter().find(|(a, ..)| a == axis);
                let mut v = json!({
                    "kind": e["author_kind"], "actor": e["author"], "model_id": e["model_id"],
                    "committed_by": held.and_then(|(_, _, _, by, ..)| by.clone()),
                    "decision": held.map(|(.., id, _)| *id),
                });
                if quasi {
                    v["why"] = json!(held.and_then(|(_, _, why, ..)| why.clone()));
                }
                v
            }
            None => json!({"kind": "rule"}),
        };
        // the rule that decided: the rule's own evidence of the value in
        // force, else the rule's evidence the value came against
        let rules: Vec<&Value> = rows.iter().filter(|e| e["author_kind"].is_null()).collect();
        let decider = rules
            .iter()
            .find(|e| {
                e["value"]
                    .as_str()
                    .is_some_and(|v| values.iter().any(|x| x == v))
            })
            .or_else(|| rules.first())
            .copied();
        let decided = decider.map(|e| {
            let (set, rule) = (
                e["rule_set"].as_str().unwrap_or_default(),
                e["rule"].as_str().unwrap_or_default(),
            );
            // its clause: the first of the rule's votes on the axis, of the
            // tier the evidence names
            let clause = votes
                .iter()
                .filter(|(v, _)| v.axis == *axis && v.rule_set == set && v.rule == rule)
                .min_by_key(|(v, _)| (v.tier != e["tier"].as_str().unwrap_or_default(), v.clause))
                .map(|(v, _)| v.clause);
            let reads = match (pack, clause) {
                (Some(p), Some(c)) if same_version => {
                    nils_pack::reads::clause_reads(p, set, rule, c as usize)
                }
                _ => None,
            };
            let mut hv = serde_json::Map::new();
            if let Some(r) = &reads {
                for f in &r.fields {
                    if shown(f).is_some()
                        && let Some(v) = head.get(f).filter(|v| !v.is_null())
                    {
                        hv.insert(f.clone(), v.clone());
                    }
                }
            }
            let mut v = json!({
                "rule_set": set, "rule": rule, "clause": clause, "tier": e["tier"],
                "source": e["source"],
                "reads": reads.as_ref().map(|r| json!({
                    "fields": r.fields, "texts": r.texts, "axes": r.axes, "flags": r.flags,
                })),
                "header": hv,
            });
            if quasi {
                v["matched"] = e["matched"].clone();
            }
            v
        });
        // the other rules that voted, one entry per rule, with its value
        let decider_rule = decider.map(|e| {
            (
                e["rule_set"].as_str().unwrap_or_default().to_string(),
                e["rule"].as_str().unwrap_or_default().to_string(),
            )
        });
        let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut voted = Vec::new();
        let mut said: BTreeSet<String> = BTreeSet::new();
        for (v, value) in votes.iter().filter(|(v, _)| v.axis == *axis) {
            let value = named(&names, value);
            if !v.restates {
                said.insert(value.clone());
            }
            if decider_rule.as_ref() == Some(&(v.rule_set.clone(), v.rule.clone())) {
                continue;
            }
            if !seen.insert((v.rule_set.clone(), v.rule.clone(), value.clone())) {
                continue;
            }
            voted.push(json!({
                "rule_set": v.rule_set, "rule": v.rule, "clause": v.clause, "tier": v.tier,
                "value": if value.is_empty() { Value::Null } else { json!(value) },
                "restates": v.restates,
            }));
        }
        let s1 = asked.as_ref().and_then(|(_, ev)| s1_on(ev, axis));
        let model_p = asked
            .as_ref()
            .map(|(_, ev)| ev["systems"]["model"]["p"][axis].clone())
            .filter(|p| !p.is_null());
        let value = value_json(&ids);
        let mut line = format!(
            "{axis}: {}",
            if ids.is_empty() {
                "(nothing)".to_string()
            } else {
                ids.join(", ")
            }
        );
        match (&set_by["kind"], decided.as_ref()) {
            (Value::String(k), _) if k != "rule" => {
                line.push_str(&format!(
                    ", a {k}'s ({})",
                    set_by["actor"].as_str().unwrap_or("unnamed")
                ));
            }
            (_, Some(dv)) => {
                line.push_str(&format!(
                    " by {}/{}",
                    dv["rule_set"].as_str().unwrap_or_default(),
                    dv["rule"].as_str().unwrap_or_default()
                ));
                if quasi && let Some(m) = dv["matched"].as_str().filter(|m| !m.is_empty()) {
                    line.push_str(&format!(" [{m}]"));
                }
                let hv: Vec<String> = dv["header"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(f, v)| format!("{} {}", shown(f).unwrap_or(f), short(v)))
                    .collect();
                if !hv.is_empty() {
                    line.push_str(&format!(", {}", hv.join(" ")));
                }
            }
            _ => {}
        }
        let others: Vec<String> = voted
            .iter()
            .filter(|v| !v["restates"].as_bool().unwrap_or(false))
            .filter(|v| v["value"].as_str().map(str::to_string) != ids.first().cloned())
            .map(|v| {
                format!(
                    "{} said {}",
                    v["rule"].as_str().unwrap_or_default(),
                    v["value"].as_str().unwrap_or("nothing")
                )
            })
            .collect();
        if !others.is_empty() {
            line.push_str(&format!("; {}", others.join(", ")));
        }
        if let Some(top) = s1.as_ref().and_then(|l| l.first()) {
            line.push_str(&format!(
                "; System 1 {} {:.2}",
                top["value"].as_str().unwrap_or("nothing"),
                top["p"].as_f64().unwrap_or(0.0)
            ));
        }
        let mut entry = json!({
            "axis": axis,
            "value": value,
            "confidence": confidence,
            "tier": tier,
            "set_by": set_by,
            "decided": decided,
            "voted": voted,
            "disagree": said.len() > 1,
            "line": line,
        });
        // System 1's word, only where it asked
        if let Some(s1) = s1 {
            entry["s1"] = json!(s1);
        }
        if let Some(p) = model_p {
            entry["model_p"] = p;
        }
        out_axes.push(entry);
    }
    let core: serde_json::Map<String, Value> = CORE
        .iter()
        .filter_map(|(f, _)| head.get(*f).map(|v| (f.to_string(), v.clone())))
        .filter(|(_, v)| !v.is_null())
        .collect();
    Ok(Some(json!({
        "stack": stack,
        "pack": pack_name,
        "version": pack_version,
        "detail": if quasi { "quasi" } else { "plain" },
        "blind": blind,
        "header": core,
        "asked": asked.map(|(id, ev)| json!({
            "item": id, "confidence": ev["confidence"], "agree": ev["agree"],
            "candidates": ev["candidates"],
        })),
        "axes": out_axes,
    })))
}

/// The ids as a list for `IN (...)`.
fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The header values a blind reader is shown: the physics, the sequence
/// type tokens and the geometry, raw, with the sequence name only at detail
/// quasi.
const BLIND_HEADER: &[&str] = &[
    "repetition_time",
    "echo_time",
    "inversion_time",
    "flip_angle",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "mr_acquisition_type",
    "text_sequence_name",
    "orientation",
    "n_slices",
    "n_instances",
    "slice_thickness",
    "spacing_between_slices",
];

/// Record 48 R2: what a blind reader of a stack is shown: the pictures'
/// references and the raw header values, and nothing a system said of it:
/// no value in force, no rule, no vote, no line, no author, nothing of
/// System 1's.
pub(crate) fn blind_doc(store: &mut Store, stack: i64, quasi: bool) -> Result<Value, StoreError> {
    let head = header(store, &[stack])?.remove(&stack).unwrap_or_default();
    let shown: serde_json::Map<String, Value> = BLIND_HEADER
        .iter()
        .filter(|f| quasi || !HASHED_ONLY.contains(f))
        .filter_map(|f| {
            head.get(*f)
                .filter(|v| !v.is_null())
                .map(|v| (f.to_string(), v.clone()))
        })
        .collect();
    Ok(json!({
        "stack": stack,
        "blind": true,
        "detail": if quasi { "quasi" } else { "plain" },
        "header": shown,
        "pictures": {"instances": format!("/api/instances/{stack}")},
    }))
}

/// Record 48 R2: the stacks, of these, a caller reads blind. A stack is
/// read blind while it is of a sample sealed now and an open campaign asks
/// it, by the campaign's raters (an assignment as a rater, or named as one)
/// and by anyone who is neither an adjudicator of it nor a holder of
/// review:work; an adjudicator or a holder of review:work who rates in no
/// such campaign reads it as usual.
pub(crate) fn hidden_stacks(
    store: &mut Store,
    stacks: &[i64],
    principal: &str,
    review_work: bool,
) -> Result<BTreeSet<i64>, StoreError> {
    let (sealed, _) = nils_registry::labels::sealed_now(store, stacks, &[])
        .map_err(|e| StoreError::Message(e.to_string()))?;
    let mut out = BTreeSet::new();
    if sealed.is_empty() {
        return Ok(out);
    }
    let list: Vec<i64> = sealed.into_iter().collect();
    let mut asked: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for chunk in list.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT i.stack_id, i.campaign_id FROM {} i JOIN {} c ON c.id = i.campaign_id \
             WHERE c.status IN ('open', 'closing') AND i.stack_id IN ({})",
            store.qualified("campaign_item"),
            store.qualified("campaign"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            asked.entry(r.int(1)?).or_default().insert(r.int(0)?);
        }
    }
    let d = store.dialect();
    for (campaign, stacks) in asked {
        let Some(c) =
            campaign::get(store, campaign).map_err(|e| StoreError::Message(e.to_string()))?
        else {
            continue;
        };
        let sql = format!(
            "SELECT role FROM {} WHERE campaign_id = {} AND principal = {}",
            store.qualified("campaign_assignment"),
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        );
        let roles: BTreeSet<String> = store
            .query(&sql, &[Param::Int(campaign), Param::from(principal)])?
            .iter()
            .filter_map(|r| r.text(0).ok().map(str::to_string))
            .collect();
        let rater = roles.contains("rater") || c.raters().iter().any(|p| p == principal);
        let adjudicator =
            roles.contains("adjudicator") || c.adjudicators().iter().any(|p| p == principal);
        if rater || !(review_work || adjudicator) {
            out.extend(stacks);
        }
    }
    Ok(out)
}

/// Whether one stack is read blind by a caller ([`hidden_stacks`]).
pub(crate) fn hidden(
    store: &mut Store,
    stack: i64,
    principal: &str,
    review_work: bool,
) -> Result<bool, StoreError> {
    Ok(!hidden_stacks(store, &[stack], principal, review_work)?.is_empty())
}

/// The rules' values of stacks' axes, as the pack names them, five hundred
/// stacks a query.
fn rules_values(
    store: &mut Store,
    stacks: &[i64],
    pack: Option<&nils_pack::Pack>,
) -> Result<BTreeMap<i64, BTreeMap<String, Vec<String>>>, StoreError> {
    let mut names: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut out: BTreeMap<i64, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({}) ORDER BY stack_id, axis, value",
            store.qualified("classification_axis"),
            id_list(chunk)
        );
        for r in store.query(&sql, &[])? {
            let axis = r.text(1)?.to_string();
            let n = names
                .entry(axis.clone())
                .or_insert_with(|| crate::campaigns::value_names(pack, Some(&axis)));
            let e = out.entry(r.int(0)?).or_default().entry(axis).or_default();
            if let Some(v) = r.opt_text(2)?.filter(|v| !v.is_empty()) {
                e.push(named(n, v));
            }
        }
    }
    Ok(out)
}

/// System 1's open questions on stacks, by stack: the item and its
/// evidence.
fn asked_many(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, (i64, Value)>, StoreError> {
    let t = table("review_item");
    let d = store.dialect();
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let keys = chunk
            .iter()
            .map(|s| format!("'{}'", nils_registry::asked::key(*s)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, group_key, {} FROM {} WHERE kind = '{}' AND status = 'open' AND group_key IN ({keys}) ORDER BY id",
            d.text_of(t.column("evidence").expect("evidence")),
            store.qualified("review_item"),
            nils_registry::asked::KIND,
        );
        for r in store.query(&sql, &[])? {
            let Some(stack) = r
                .opt_text(1)?
                .and_then(|k| k.rsplit(':').next())
                .and_then(|n| n.parse::<i64>().ok())
            else {
                continue;
            };
            let ev = r
                .opt_text(2)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(Value::Null);
            out.insert(stack, (r.int(0)?, ev));
        }
    }
    Ok(out)
}

/// The answer the rules and System 1 suggest, from what was read of a
/// stack: the rules' value where System 1 has not asked, the value both
/// agree on where it has, none where they disagree or the question is not
/// about axes.
fn suggest_from(
    rules: &BTreeMap<String, Vec<String>>,
    asked: Option<&Value>,
    question: &Question,
) -> Option<String> {
    // System 1's first candidate, on the axes the question asks, where it
    // asked about all of them
    let asks = campaign::axes_of(question);
    let top: Option<serde_json::Map<String, Value>> = asked.and_then(|ev| {
        let covered: Vec<&str> = ev["axes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if asks.is_empty() || !asks.iter().all(|a| covered.contains(&a.as_str())) {
            return None;
        }
        let first = &ev["candidates"][0]["values"];
        Some(
            asks.iter()
                .map(|a| (a.clone(), first[a.as_str()].clone()))
                .collect(),
        )
    });
    match question {
        Question::Axis { axis, .. } => {
            let mut mine = rules.get(axis).cloned().unwrap_or_default();
            mine.sort();
            if mine.is_empty() {
                return None;
            }
            let text = mine.join(",");
            match top {
                Some(t) => (one_text(&t[axis]) == text).then_some(text),
                None => Some(text),
            }
        }
        Question::Axes {
            axes, constraints, ..
        } => {
            let obj: serde_json::Map<String, Value> = axes
                .iter()
                .map(|a| (a.clone(), json!(rules.get(a).cloned().unwrap_or_default())))
                .collect();
            let joint =
                campaign::joint_of(axes, constraints, &Value::Object(obj).to_string()).ok()?;
            campaign::legal(constraints, &joint).ok()?;
            let mine = campaign::canonical_joint(constraints, &joint);
            match top {
                Some(t) => {
                    let theirs =
                        campaign::joint_of(axes, constraints, &Value::Object(t).to_string())
                            .map(|j| campaign::canonical_joint(constraints, &j))
                            .ok();
                    (theirs.as_deref() == Some(mine.as_str())).then_some(mine)
                }
                None => Some(mine),
            }
        }
        _ => None,
    }
}

/// Whether a stack is of a sample sealed now: it is read blind (record 48
/// R1 and R2), with no suggestion and no candidate of System 1's, so the
/// reference labels a certificate grades a model by are not anchored to
/// the model graded.
pub(crate) fn blind(store: &mut Store, stack: i64) -> Result<bool, StoreError> {
    nils_registry::labels::sealed_now(store, &[stack], &[])
        .map(|(s, _)| !s.is_empty())
        .map_err(|e| StoreError::Message(e.to_string()))
}

/// The answer the engine suggests for a stack under a question (record 48
/// R1), as an answer to it says it ([`suggest_from`]); none for a stack of
/// a sample sealed now, which is read blind.
pub(crate) fn suggestion(
    store: &mut Store,
    stack: i64,
    question: &Question,
    pack: Option<&nils_pack::Pack>,
) -> Result<Option<String>, StoreError> {
    if blind(store, stack)? {
        return Ok(None);
    }
    let rules = rules_values(store, &[stack], pack)?
        .remove(&stack)
        .unwrap_or_default();
    let asked = asked_many(store, &[stack])?
        .remove(&stack)
        .map(|(_, ev)| ev);
    Ok(suggest_from(&rules, asked.as_ref(), question))
}

/// Round a header value for a signature, so a TR of 2300 and 2300.0001 are
/// one sequence.
fn rounded(v: &Value) -> Value {
    match v.as_f64() {
        Some(f) if v.is_f64() => json!((f * 10.0).round() / 10.0),
        _ => v.clone(),
    }
}

/// The fields a batch's signature is hashed with and never returned, since
/// they are quasi-identifying text (the sequence name, `catalogue.md`).
const HASHED_ONLY: &[&str] = &["text_sequence_name"];

/// One batch: like stacks suggested one answer.
#[derive(Debug, Clone)]
pub(crate) struct Batch {
    pub(crate) key: String,
    pub(crate) suggested: String,
    /// What is returned of the signature: the rules and the header values,
    /// without the fields hashed only.
    pub(crate) signature: Value,
    pub(crate) items: Vec<i64>,
}

/// What [`batches`] found.
#[derive(Debug, Clone, Default)]
pub(crate) struct Batches {
    pub(crate) groups: Vec<Batch>,
    /// Items of a sealed sample, read one by one and never in a batch.
    pub(crate) sealed: usize,
    /// Items with no suggestion: the systems disagree, or nothing decided.
    pub(crate) unsuggested: usize,
    /// Items open to this rater.
    pub(crate) open: usize,
}

/// The batches of a campaign for a rater (record 48 R1): the items the rater
/// could still be given, of stacks that share the signature of the deciding
/// physics and the suggested answer. An axis or an axes question only.
/// `only`, when given, narrows the items looked at to those: the accept
/// door checks a batch's own items again without working out every batch.
/// Everything is read five hundred stacks a query, never a stack at a time.
pub(crate) fn batches(
    store: &mut Store,
    c: &Campaign,
    principal: &str,
    pack: Option<&nils_pack::Pack>,
    only: Option<&BTreeSet<i64>>,
) -> Result<Batches, (u16, String)> {
    let e500 = |e: StoreError| (500, e.to_string());
    let question = c.question().map_err(|e| (400, e.to_string()))?;
    if !matches!(question, Question::Axis { .. } | Question::Axes { .. }) {
        return Err((
            400,
            format!(
                "campaign {} asks a {} question; batches are of axis and axes questions",
                c.name,
                question.kind()
            ),
        ));
    }
    let axes = campaign::axes_of(&question);
    let open: Vec<(i64, i64)> = campaign::open_for(store, c, principal)
        .map_err(|e| (500, e.to_string()))?
        .into_iter()
        .filter(|it| only.is_none_or(|o| o.contains(&it.id)))
        .filter_map(|it| it.stack_id.map(|s| (it.id, s)))
        .collect();
    let stacks: Vec<i64> = open.iter().map(|(_, s)| *s).collect();
    let (sealed, _) =
        nils_registry::labels::sealed_now(store, &stacks, &[]).map_err(|e| (500, e.to_string()))?;
    let heads = header(store, &stacks).map_err(e500)?;
    let voters = voters(store).map_err(e500)?;
    let rules = rules_values(store, &stacks, pack).map_err(e500)?;
    let asked = asked_many(store, &stacks).map_err(e500)?;
    let mut votes: BTreeMap<i64, Vec<Said>> = BTreeMap::new();
    let mut versions: BTreeMap<i64, String> = BTreeMap::new();
    let mut deciding: BTreeMap<i64, Vec<(String, String, String, String)>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let ids = id_list(chunk);
        let sql = format!(
            "SELECT stack_id, votes FROM {} WHERE stack_id IN ({ids}) ORDER BY stack_id, phase",
            store.qualified("classification_vote")
        );
        for r in store.query(&sql, &[]).map_err(e500)? {
            let pairs: Vec<(i64, String)> =
                serde_json::from_str(r.text(1).map_err(e500)?).unwrap_or_default();
            let list = votes.entry(r.int(0).map_err(e500)?).or_default();
            for (id, value) in pairs {
                if let Some(v) = voters.get(&id) {
                    list.push((v.clone(), value));
                }
            }
        }
        let sql = format!(
            "SELECT stack_id, pack_version FROM {} WHERE stack_id IN ({ids})",
            store.qualified("classification")
        );
        for r in store.query(&sql, &[]).map_err(e500)? {
            versions.insert(
                r.int(0).map_err(e500)?,
                r.text(1).map_err(e500)?.to_string(),
            );
        }
        let sql = format!(
            "SELECT stack_id, axis, rule_set, rule, tier FROM {} \
             WHERE stack_id IN ({ids}) AND author_kind IS NULL ORDER BY stack_id, axis, id",
            store.qualified("classification_evidence")
        );
        for r in store.query(&sql, &[]).map_err(e500)? {
            deciding.entry(r.int(0).map_err(e500)?).or_default().push((
                r.text(1).map_err(e500)?.to_string(),
                r.text(2).map_err(e500)?.to_string(),
                r.text(3).map_err(e500)?.to_string(),
                r.text(4).map_err(e500)?.to_string(),
            ));
        }
    }
    let seed = campaign::hold_back_seed(store, c.id).map_err(|e| (500, e.to_string()))?;
    let none = Vec::new();
    let mut out = Batches {
        open: open.len(),
        ..Batches::default()
    };
    let mut groups: BTreeMap<String, Batch> = BTreeMap::new();
    for (item, stack) in &open {
        if sealed.contains(stack) {
            out.sealed += 1;
            continue;
        }
        let Some(suggested) = suggest_from(
            rules.get(stack).unwrap_or(&BTreeMap::new()),
            asked.get(stack).map(|(_, ev)| ev),
            &question,
        ) else {
            out.unsuggested += 1;
            continue;
        };
        // the deciding rule of each axis asked, and what its clause read
        let said = votes.get(stack).unwrap_or(&none);
        let version = versions.get(stack).cloned().unwrap_or_default();
        let mut decided: BTreeMap<String, String> = BTreeMap::new();
        let mut fields: BTreeSet<String> = SAME_SEQUENCE.iter().map(|f| f.to_string()).collect();
        for (axis, set, rule, tier) in deciding.get(stack).into_iter().flatten() {
            if !axes.contains(axis) || decided.contains_key(axis) {
                continue;
            }
            let clause = said
                .iter()
                .filter(|(v, _)| v.axis == *axis && v.rule_set == *set && v.rule == *rule)
                .min_by_key(|(v, _)| (v.tier != *tier, v.clause))
                .map(|(v, _)| v.clause);
            if let (Some(p), Some(cl)) = (pack, clause)
                && p.version.to_string() == version
                && let Some(reads) = nils_pack::reads::clause_reads(p, set, rule, cl as usize)
            {
                fields.extend(reads.fields.into_iter().filter(|f| shown(f).is_some()));
            }
            decided.insert(axis.clone(), format!("{set}/{rule}"));
        }
        let head = heads.get(stack).cloned().unwrap_or_default();
        let hv: serde_json::Map<String, Value> = fields
            .iter()
            .map(|f| (f.clone(), head.get(f).map(rounded).unwrap_or(Value::Null)))
            .collect();
        let hashed = json!({"rules": decided, "header": hv});
        // salted with the campaign's secret seed, so a key names no
        // signature a caller could work out
        let key_text =
            json!({"seed": seed, "signature": hashed, "suggested": suggested}).to_string();
        let key = crate::campaigns::sha256(key_text.as_bytes())[..16].to_string();
        groups
            .entry(key.clone())
            .or_insert_with(|| {
                let shown: serde_json::Map<String, Value> = hv
                    .iter()
                    .filter(|(f, _)| !HASHED_ONLY.contains(&f.as_str()))
                    .map(|(f, v)| (f.clone(), v.clone()))
                    .collect();
                Batch {
                    key,
                    suggested: suggested.clone(),
                    signature: json!({"rules": decided, "header": shown}),
                    items: Vec::new(),
                }
            })
            .items
            .push(*item);
    }
    let mut list: Vec<Batch> = groups.into_values().collect();
    list.sort_by(|a, b| b.items.len().cmp(&a.items.len()).then(a.key.cmp(&b.key)));
    out.groups = list;
    Ok(out)
}

/// The batches a rater listed last, by campaign, rater and key: the accept
/// door checks the items of the one batch named again rather than work out
/// every batch of the campaign.
type Listed = BTreeMap<(i64, String, String), Vec<i64>>;

fn listed() -> &'static std::sync::Mutex<Listed> {
    static LISTED: std::sync::OnceLock<std::sync::Mutex<Listed>> = std::sync::OnceLock::new();
    LISTED.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

/// Keep what a rater was shown, for the accept door.
pub(crate) fn remember(campaign: i64, principal: &str, found: &Batches) {
    if let Ok(mut held) = listed().lock() {
        held.retain(|(c, p, _), _| !(*c == campaign && p == principal));
        for g in &found.groups {
            held.insert(
                (campaign, principal.to_string(), g.key.clone()),
                g.items.clone(),
            );
        }
    }
}

/// One batch as it stands now: the items a rater was shown under the key,
/// looked at again, or every batch worked out where the rater listed none.
pub(crate) fn batch_now(
    store: &mut Store,
    c: &Campaign,
    principal: &str,
    pack: Option<&nils_pack::Pack>,
    key: &str,
) -> Result<Option<Batch>, (u16, String)> {
    let shown = listed().lock().ok().and_then(|h| {
        h.get(&(c.id, principal.to_string(), key.to_string()))
            .cloned()
    });
    let only: Option<BTreeSet<i64>> = shown.map(|v| v.into_iter().collect());
    let found = batches(store, c, principal, pack, only.as_ref())?;
    Ok(found.groups.into_iter().find(|g| g.key == key))
}

impl Batches {
    pub(crate) fn as_json(&self, campaign: &Campaign, question_kind: &str, sample: usize) -> Value {
        json!({
            "campaign": campaign.id,
            "open": self.open,
            "sealed": self.sealed,
            "unsuggested": self.unsuggested,
            "count": self.groups.len(),
            "groups": self.groups.iter().map(|g| {
                let mut suggested = json!(g.suggested);
                if question_kind == "axes"
                    && let Ok(v) = serde_json::from_str::<Value>(&g.suggested)
                {
                    suggested = v;
                }
                json!({
                    "key": g.key,
                    "count": g.items.len(),
                    "suggested": suggested,
                    "signature": g.signature,
                    "sample": g.items.iter().take(sample).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// The items of a batch held back to be read alone: `share` of them,
/// rounded up, chosen by the order of a digest of the seed and each item,
/// so one seed holds back the same items. A batch of one holds none.
pub(crate) fn held_back(items: &[i64], share: f64, seed: &str) -> BTreeSet<i64> {
    if items.len() < 2 || share <= 0.0 {
        return BTreeSet::new();
    }
    let n = ((share.min(1.0) * items.len() as f64).ceil() as usize).min(items.len());
    let mut ranked: Vec<(String, i64)> = items
        .iter()
        .map(|i| {
            (
                crate::campaigns::sha256(format!("{seed}:{i}").as_bytes()),
                *i,
            )
        })
        .collect();
    ranked.sort();
    ranked.into_iter().take(n).map(|(_, i)| i).collect()
}

/// Record 48, the reader's whole-combination search: the combinations of an
/// axes question's answered axes that the registry's classifications hold,
/// each with how many stacks hold it, most common first, `limit` at most.
///
/// Generic, never a stack's: a count says how common a combination is
/// across the registry, the same for every item, and no stack of this
/// campaign, nor any stack of a sample sealed now, is counted, so the counts
/// say nothing of any stack a rater reads, blind or not. Values are named by
/// identity; a combination outside the question's vocabulary, or one the
/// pack's constraints forbid, is left out and counted apart.
pub(crate) fn combinations(
    store: &mut Store,
    c: &Campaign,
    pack: Option<&nils_pack::Pack>,
    limit: usize,
) -> Result<Value, (u16, String)> {
    let q = &c.question;
    if q["kind"] != "axes" {
        return Err((
            400,
            format!(
                "campaign {} asks a {} question; combinations are an axes question's",
                c.name,
                q["kind"].as_str().unwrap_or("?")
            ),
        ));
    }
    let err = |e: StoreError| (500, e.to_string());
    let words = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let constraints = &q["constraints"];
    let derived = words(&q["derive"]);
    let axes: Vec<String> = words(&q["axes"])
        .into_iter()
        .filter(|a| !derived.contains(a))
        .collect();
    let multi = words(&constraints["multi"]);
    let vocabulary: BTreeMap<String, BTreeSet<String>> = axes
        .iter()
        .map(|a| {
            let listed = constraints["values"][a.as_str()].clone();
            let listed = if listed.is_array() {
                listed
            } else {
                q["values"][a.as_str()].clone()
            };
            (a.clone(), words(&listed).into_iter().collect())
        })
        .collect();
    let names: BTreeMap<String, BTreeMap<String, String>> = axes
        .iter()
        .map(|a| (a.clone(), crate::campaigns::value_names(pack, Some(a))))
        .collect();

    // what is never counted: this campaign's stacks and every sealed one
    let mut left_out: BTreeSet<i64> = campaign::items(store, c.id)
        .map_err(|e| (500, e.to_string()))?
        .iter()
        .filter_map(|i| i.stack_id)
        .collect();
    let sql = format!(
        "SELECT DISTINCT stack_id FROM {} WHERE unsealed_at IS NULL",
        store.qualified("sealed_stack")
    );
    for r in store.query(&sql, &[]).map_err(err)? {
        left_out.insert(r.int(0).map_err(err)?);
    }

    let sql = format!(
        "SELECT MIN(stack_id), MAX(stack_id) FROM {}",
        store.qualified("classification")
    );
    let rows = store.query(&sql, &[]).map_err(err)?;
    let (lo, hi) = match rows.first() {
        Some(r) => (r.opt_int(0).map_err(err)?, r.opt_int(1).map_err(err)?),
        None => (None, None),
    };
    let d = store.dialect();
    let axis_params: Vec<String> = (0..axes.len())
        .map(|i| d.param(i + 3, Type::Text))
        .collect();
    let sql = format!(
        "SELECT stack_id, axis, value FROM {} WHERE stack_id >= {} AND stack_id <= {} AND axis IN ({}) ORDER BY stack_id, axis, value",
        store.qualified("classification_axis"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        axis_params.join(", ")
    );
    let mut counts: BTreeMap<String, (Value, u64)> = BTreeMap::new();
    let (mut counted, mut outside, mut illegal) = (0u64, 0u64, 0u64);
    const SPAN: i64 = 20_000;
    if let (Some(lo), Some(hi)) = (lo, hi) {
        let mut from = lo;
        while from <= hi {
            let to = from.saturating_add(SPAN - 1).min(hi);
            let mut params = vec![Param::Int(from), Param::Int(to)];
            params.extend(axes.iter().map(|a| Param::from(a.as_str())));
            let mut by_stack: BTreeMap<i64, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
            for r in store.query(&sql, &params).map_err(err)? {
                let stack = r.int(0).map_err(err)?;
                if left_out.contains(&stack) {
                    continue;
                }
                let axis = r.text(1).map_err(err)?.to_string();
                let e = by_stack
                    .entry(stack)
                    .or_default()
                    .entry(axis.clone())
                    .or_default();
                if let Some(v) = r.opt_text(2).map_err(err)?.filter(|v| !v.is_empty()) {
                    e.insert(named(&names[&axis], v));
                }
            }
            'stack: for (_, held) in by_stack {
                let mut joint: campaign::Joint = BTreeMap::new();
                let mut shown = serde_json::Map::new();
                for a in &axes {
                    let values: Vec<String> = held
                        .get(a)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    let is_multi = multi.contains(a);
                    if values.iter().any(|v| !vocabulary[a].contains(v))
                        || (!is_multi && values.len() > 1)
                    {
                        outside += 1;
                        continue 'stack;
                    }
                    shown.insert(
                        a.clone(),
                        if is_multi {
                            json!(values)
                        } else {
                            values.first().map_or(Value::Null, |v| json!(v))
                        },
                    );
                    joint.insert(a.clone(), values);
                }
                if campaign::legal(constraints, &joint).is_err() {
                    illegal += 1;
                    continue;
                }
                counted += 1;
                let key = serde_json::to_string(&joint).unwrap_or_default();
                counts
                    .entry(key)
                    .or_insert_with(|| (Value::Object(shown), 0))
                    .1 += 1;
            }
            from = to.saturating_add(1);
        }
    }
    let distinct = counts.len();
    let mut list: Vec<(Value, u64)> = counts.into_values().collect();
    list.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });
    list.truncate(limit);
    Ok(json!({
        "campaign": c.id,
        "axes": axes,
        "counted": counted,
        "distinct": distinct,
        "left_out": {"outside": outside, "illegal": illegal, "stacks": left_out.len()},
        "combinations": list.into_iter().map(|(values, count)| json!({"values": values, "count": count})).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_holds_back_rounded_up_and_one_seed_the_same_items() {
        let items: Vec<i64> = (1..=20).collect();
        let a = held_back(&items, 0.1, "s");
        assert_eq!(a.len(), 2);
        assert_eq!(a, held_back(&items, 0.1, "s"));
        assert_eq!(held_back(&items, 0.11, "s").len(), 3);
        assert!(held_back(&[7], 0.5, "s").is_empty());
        assert!(held_back(&items, 0.0, "s").is_empty());
        assert_eq!(held_back(&items, 1.0, "s").len(), 20);
    }

    #[test]
    fn system_one_on_an_axis_sums_the_joint_candidates() {
        let ev = json!({
            "axes": ["base", "modifier"],
            "candidates": [
                {"values": {"base": "T1w", "modifier": ["FatSat"]}, "p": 0.5},
                {"values": {"base": "T1w", "modifier": []}, "p": 0.3},
                {"values": {"base": "T2w", "modifier": null}, "p": 0.1}
            ]
        });
        let base = s1_on(&ev, "base").unwrap();
        assert_eq!(base[0], json!({"value": "T1w", "p": 0.8}));
        assert_eq!(base[1], json!({"value": "T2w", "p": 0.1}));
        assert!(s1_on(&ev, "technique").is_none());
    }
}
