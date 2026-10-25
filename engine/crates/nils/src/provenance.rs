// SPDX-License-Identifier: AGPL-3.0-only

//! The provenance door (Wave 7b A7; record 57 P8, record 58 P5; spec
//! section 8.6): for a subject, a session or a stack, which steps ran on
//! it, with which keys and which results, and for every result its
//! curation state: the MRI pack version and digest that classified its
//! inputs, the label set digest, System 1's model version, and whether a
//! person overrode an axis on an input, and which. A change of the rules
//! version on an input marks the result `stale`, in the same words at every
//! door (E5).

use std::collections::{BTreeMap, BTreeSet};

use nils_registry::home::Home;
use nils_registry::pipeline as rows;
use nils_registry::schema::Type;
use nils_registry::{Param, Registry, Store};
use serde_json::{Value, json};

use crate::serve::{Caller, Reply};
use crate::{Exit, fail, usage};

/// The door, as the capabilities list it.
pub(crate) const DOOR: &str = "GET /api/provenance/{kind}/{id}";

/// The policy row of [`DOOR`].
pub(crate) const POLICY: (&str, bool, bool, &str, &str, &str, &str) = (
    DOOR,
    false,
    false,
    "bounded",
    "one subject, session or stack",
    "Reading provenance",
    "Read provenance",
);

/// The words every door says of a result whose input's rules version
/// changed (section 8.6, as the comparability rule of section 5.6 is
/// worded): `stale`, and why.
pub(crate) fn stale_words(changes: &[(String, String, String)]) -> String {
    let list: Vec<String> = changes
        .iter()
        .map(|(pack, was, now)| format!("{pack} {was} at the run, {now} now"))
        .collect();
    format!(
        "stale: the rules version on an input changed ({}); the result is read apart from current ones and never pooled with them, at every door",
        list.join("; ")
    )
}

/// The curation state of a unit's inputs now: per stack the pack and
/// version that classified it (and under which overlay), the deployment
/// pack's digest where its folder is readable, the label set the run read,
/// System 1's answers on the stacks (a model's committed decisions, with
/// its version), and the axes a person overrode. Recorded on the unit at
/// its run (record 58 P5); read again at the door to tell `stale`.
pub(crate) fn curation_now(
    registry: &mut Registry,
    stacks: &[i64],
    label_set: Option<&nils_registry::labels::LabelSet>,
    pack_dir: &Option<std::path::PathBuf>,
) -> Value {
    let store = registry.store();
    let rules = rules_of(store, stacks);
    let (overrides, system1) = decisions_of(store, stacks);
    let digest = pack_dir.as_deref().and_then(pack_digest);
    json!({
        "rules": rules,
        "pack_digest": digest,
        "label_set": label_set.map(|l| json!({"id": l.id, "digest": l.digest, "label": format!("{}@{}", l.name, l.version)})),
        "system1": system1,
        "overrides": overrides,
        "at": nils_registry::time::now_iso(),
    })
}

/// Per stack, `{stack, pack, version, overlay, epoch}` of its
/// classification.
fn rules_of(store: &mut Store, stacks: &[i64]) -> Vec<Value> {
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, pack, pack_version, overlay, epoch FROM {} WHERE stack_id IN ({list}) ORDER BY stack_id",
            store.qualified("classification")
        );
        for r in store.query(&sql, &[]).unwrap_or_default() {
            out.push(json!({
                "stack": r.int(0).unwrap_or_default(),
                "pack": r.text(1).unwrap_or_default(),
                "version": r.text(2).unwrap_or_default(),
                "overlay": r.opt_text(3).ok().flatten(),
                "epoch": r.opt_int(4).ok().flatten(),
            }));
        }
    }
    out
}

/// The committed decisions on the stacks: a person's as overrides `{stack,
/// axis, by, at}`, a model's as System 1's `{stack, axis, model, version}`.
fn decisions_of(store: &mut Store, stacks: &[i64]) -> (Vec<Value>, Vec<Value>) {
    let mut overrides = Vec::new();
    let mut system1 = Vec::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT ref, axis, actor, author_kind, author_version, model_id, decided_at FROM {} \
             WHERE scope = 'stack' AND ref IN ({list}) AND committed_at IS NOT NULL AND withdrawn_at IS NULL \
             ORDER BY ref, axis, id",
            store.qualified("decision")
        );
        for r in store.query(&sql, &[]).unwrap_or_default() {
            let stack: i64 = r
                .text(0)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default();
            let axis = r.text(1).unwrap_or_default().to_string();
            let kind = r.text(3).unwrap_or_default().to_string();
            match kind.as_str() {
                "model" => system1.push(json!({
                    "stack": stack, "axis": axis,
                    "model": r.opt_int(5).ok().flatten(),
                    "version": r.opt_text(4).ok().flatten(),
                })),
                _ => overrides.push(json!({
                    "stack": stack, "axis": axis, "by": r.text(2).unwrap_or_default(),
                    "kind": kind, "at": r.opt_text(6).ok().flatten(),
                })),
            }
        }
    }
    (overrides, system1)
}

/// The digest of a pack folder: sha256 over its files' names and bytes, in
/// name order; none where the folder cannot be read.
pub(crate) fn pack_digest(dir: &std::path::Path) -> Option<String> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut all: Vec<u8> = Vec::new();
    for f in files {
        let rel = f.strip_prefix(dir).ok()?.to_string_lossy().into_owned();
        all.extend(rel.as_bytes());
        all.push(0);
        all.extend(std::fs::read(&f).ok()?);
        all.push(0);
    }
    Some(nils_pipeline::sha256(&all))
}

/// The curation state of a unit as a door answers it: what was recorded,
/// what holds now, and `state`: `current`, `stale` (a rules version on an
/// input changed, with [`stale_words`]) or `unknown` (a unit from before
/// recorded none).
pub(crate) fn state_of(store: &mut Store, recorded: &Value, stacks: &[i64]) -> Value {
    if !recorded.is_object() {
        return json!({
            "state": "unknown",
            "words": "no curation state was recorded for this unit (it ran before the provenance door)",
            "recorded": Value::Null,
        });
    }
    let now = rules_of(store, stacks);
    let then: BTreeMap<i64, (String, String)> = recorded["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                r["stack"].as_i64()?,
                (
                    r["pack"].as_str()?.to_string(),
                    r["version"].as_str()?.to_string(),
                ),
            ))
        })
        .collect();
    let mut changes: Vec<(String, String, String)> = Vec::new();
    for r in &now {
        let Some(stack) = r["stack"].as_i64() else {
            continue;
        };
        if let Some((pack, was)) = then.get(&stack) {
            let now_v = r["version"].as_str().unwrap_or_default();
            if now_v != was {
                let c = (pack.clone(), was.clone(), now_v.to_string());
                if !changes.contains(&c) {
                    changes.push(c);
                }
            }
        }
    }
    let (overrides_now, _) = decisions_of(store, stacks);
    if changes.is_empty() {
        json!({
            "state": "current",
            "words": "current: the rules version on every input is the one that ran",
            "recorded": recorded,
            "overrides_now": overrides_now,
        })
    } else {
        json!({
            "state": "stale",
            "words": stale_words(&changes),
            "changes": changes.iter().map(|(p, w, n)| json!({"pack": p, "was": w, "now": n})).collect::<Vec<_>>(),
            "recorded": recorded,
            "overrides_now": overrides_now,
        })
    }
}

/// What the door is asked about.
pub(crate) enum Target {
    Subject(i64),
    /// A subject and the session label as the runs name it (`ses-<label>`).
    Session(i64, String),
    Stack(i64),
}

impl Target {
    pub(crate) fn parse(kind: &str, id: &str) -> Result<Target, String> {
        match kind {
            "subject" => id
                .parse()
                .map(Target::Subject)
                .map_err(|_| "a subject is named by its id".to_string()),
            "stack" => id
                .parse()
                .map(Target::Stack)
                .map_err(|_| "a stack is named by its id".to_string()),
            "session" => {
                let (s, label) = id
                    .split_once('@')
                    .ok_or("a session is named <subject id>@<session label>, as the runs name it (sub-<s>_ses-<label>)")?;
                Ok(Target::Session(
                    s.parse().map_err(|_| {
                        "a session is named <subject id>@<session label>".to_string()
                    })?,
                    label.to_string(),
                ))
            }
            other => Err(format!(
                "provenance is of a subject, a session or a stack, not {other}"
            )),
        }
    }

    fn doc(&self) -> Value {
        match self {
            Target::Subject(s) => json!({"kind": "subject", "id": s}),
            Target::Session(s, l) => {
                json!({"kind": "session", "subject": s, "session": l, "id": format!("{s}@{l}")})
            }
            Target::Stack(s) => json!({"kind": "stack", "id": s}),
        }
    }
}

/// The stacks the target holds.
fn stacks_of(store: &mut Store, t: &Target) -> Result<Vec<i64>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    match t {
        Target::Stack(s) => Ok(vec![*s]),
        Target::Subject(s) | Target::Session(s, _) => {
            let d = store.dialect();
            let sql = format!(
                "SELECT st.id FROM {} st JOIN {} se ON se.id = st.series_id WHERE se.subject_id = {} ORDER BY st.id",
                store.qualified("stack"),
                store.qualified("series"),
                d.param(1, Type::Int)
            );
            let all: Vec<i64> = store
                .query(&sql, &[Param::Int(*s)])
                .map_err(err)?
                .iter()
                .filter_map(|r| r.int(0).ok())
                .collect();
            match t {
                Target::Session(_, label) => {
                    let sessions = crate::pipelines::stack_sessions(store, &all)?;
                    Ok(all
                        .into_iter()
                        .filter(|st| sessions.get(st).is_some_and(|(_, l)| l == label))
                        .collect())
                }
                _ => Ok(all),
            }
        }
    }
}

/// The runs that touched any of the stacks: those whose frozen handle holds
/// one, and those with a unit of one.
fn runs_touching(store: &mut Store, stacks: &[i64]) -> Result<Vec<i64>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut out: BTreeSet<i64> = BTreeSet::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT r.id FROM {} r JOIN {} hm ON hm.handle_id = r.handle_id WHERE hm.key IN ({list})",
            store.qualified("pipeline_run"),
            store.qualified("handle_member"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            out.insert(r.int(0).map_err(err)?);
        }
        let names = chunk
            .iter()
            .map(|s| format!("'stack-{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT run_id FROM {} WHERE unit IN ({names})",
            store.qualified("pipeline_unit")
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            out.insert(r.int(0).map_err(err)?);
        }
    }
    Ok(out.into_iter().collect())
}

/// The results of one unit: its files as derivatives and how many measures
/// it wrote.
fn results_of(store: &mut Store, run: i64, unit: &rows::Unit, stacks: &[i64]) -> Value {
    let source = unit.reused_from_run.unwrap_or(run);
    let files: Vec<Value> = nils_registry::derivative::list(
        store,
        &nils_registry::derivative::Filter {
            run_id: Some(source),
            limit: 10_000,
            ..Default::default()
        },
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|d| {
        d.withdrawn_at.is_none()
            && match (d.scope.as_str(), d.stack_id) {
                ("stack", Some(s)) => stacks.contains(&s),
                _ => {
                    unit.outcome["derivatives"]
                        .as_array()
                        .is_none_or(|ids| ids.iter().any(|i| i.as_i64() == Some(d.id)))
                        && unit.unit.starts_with("sub-")
                }
            }
    })
    .map(|d| {
        json!({
            "id": d.id, "kind": d.kind, "path": d.path, "sha256": d.sha256,
            "check": d.check_status, "run": d.run_id, "created_at": d.created_at,
        })
    })
    .collect();
    let measures = nils_registry::measure::of_run(store, source)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.unit_id == unit.unit)
        .count();
    json!({"derivatives": files, "measures": measures, "from_run": source})
}

/// `GET /api/provenance/{subject|session|stack}/{id}` and `nils provenance`:
/// every step that ran on the target, with its keys, its results and its
/// curation state.
pub(crate) fn answer(store: &mut Store, t: &Target) -> Result<Value, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let stacks = stacks_of(store, t)?;
    let runs = runs_touching(store, &stacks)?;
    let mut steps: Vec<Value> = Vec::new();
    let mut stale = 0usize;
    let mut unknown = 0usize;
    for run_id in runs {
        let Some(run) = rows::run(store, run_id).map_err(err)? else {
            continue;
        };
        let Some(p) = rows::get(store, run.pipeline_id).map_err(err)? else {
            continue;
        };
        let of_units: BTreeMap<String, Vec<i64>> = crate::pipelines::unit_stacks(store, run_id)?
            .into_iter()
            .collect();
        for u in rows::units(store, run_id).map_err(err)? {
            let its: Vec<i64> = of_units.get(&u.unit).cloned().unwrap_or_default();
            if !its.iter().any(|s| stacks.contains(s)) {
                continue;
            }
            let curation = state_of(store, &u.curation, &its);
            match curation["state"].as_str() {
                Some("stale") => stale += 1,
                Some("unknown") => unknown += 1,
                _ => {}
            }
            steps.push(json!({
                "run": run_id, "pipeline": p.label(), "pipeline_id": p.id,
                "run_status": run.status, "started_at": run.started_at, "finished_at": run.finished_at,
                "workflow_run": run.workflow_run_id, "step": run.step,
                "unit": u.unit, "state": u.state, "status": u.outcome["status"],
                "reused_from": u.reused_from_run,
                "keys": {
                    "comparability": u.comparability_key,
                    "result": u.result_key,
                    "short": u.comparability_key.as_deref().map(nils_pipeline::pack::keys::short),
                    "members": u.pack["key_members"],
                },
                "knobs": u.pack["knobs"], "caveats": u.pack["caveats"], "flags": u.pack["flags"],
                "pack_status": u.pack["status"], "words": u.pack["words"],
                "results": results_of(store, run_id, &u, &its),
                "stacks": its,
                "curation": curation,
            }));
        }
    }
    steps.sort_by(|a, b| {
        a["run"]
            .as_i64()
            .cmp(&b["run"].as_i64())
            .then(a["unit"].as_str().cmp(&b["unit"].as_str()))
    });
    Ok(json!({
        "of": t.doc(),
        "stacks": stacks,
        "steps": steps,
        "counts": {"steps": steps.len(), "stale": stale, "unknown": unknown},
        "stale_words": if stale > 0 { Some("stale: the rules version on an input changed; the result is read apart from current ones and never pooled with them, at every door") } else { None },
    }))
}

/// The door: `pipelines:see` or `data:see`; units are named only at detail
/// quasi; a unit holding a stack of a sample sealed now is withheld unless
/// the caller reads sealed samples.
pub(crate) fn route(
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    get: bool,
    segs: &[&str],
) -> Option<Result<Reply, Reply>> {
    let ["api", "provenance", kind, id] = segs else {
        return None;
    };
    if !get {
        return None;
    }
    Some((|| {
        let t = Target::parse(kind, id).map_err(|e| Reply::error(404, e))?;
        let mut v = answer(registry.store(), &t).map_err(|e| Reply::error(500, e))?;
        let stacks: Vec<i64> = v["stacks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .collect();
        let sealed = crate::sealed::withheld(registry.store(), &caller.access, &stacks)
            .map_err(|e| Reply::error(500, e.to_string()))?;
        if !sealed.is_empty() {
            let before = v["steps"].as_array().map_or(0, Vec::len);
            let kept: Vec<Value> = v["steps"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|s| {
                    !s["stacks"].as_array().is_some_and(|ss| {
                        ss.iter()
                            .filter_map(Value::as_i64)
                            .any(|x| sealed.contains(&x))
                    })
                })
                .collect();
            v["withheld"] = json!({
                "steps": before - kept.len(),
                "why": crate::sealed::refusal("the steps of a stack of a sample sealed now"),
            });
            v["steps"] = Value::Array(kept);
        }
        if !quasi {
            crate::pipelines::plain(&mut v);
        }
        Ok(Reply::ok(v))
    })())
}

/// `nils provenance <subject|session|stack> <id> [--json]`.
#[derive(Debug, clap::Args)]
pub(crate) struct ProvenanceArgs {
    /// subject, session or stack
    pub(crate) kind: String,
    /// The id: a subject's or a stack's id, or <subject id>@<session label> for a session
    pub(crate) id: String,
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn command(home: &Home, args: ProvenanceArgs) -> Result<(), Exit> {
    let t = Target::parse(&args.kind, &args.id).map_err(usage)?;
    let mut registry = crate::open(home)?;
    let v = answer(registry.store(), &t).map_err(fail)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| fail(e.to_string()))?
        );
        return Ok(());
    }
    println!(
        "{} {}: {} stacks, {} steps ({} stale, {} without a recorded state)",
        args.kind,
        args.id,
        v["stacks"].as_array().map_or(0, Vec::len),
        v["counts"]["steps"],
        v["counts"]["stale"],
        v["counts"]["unknown"]
    );
    for s in v["steps"].as_array().into_iter().flatten() {
        println!(
            "  run {} {} {}: {}{}; key {}; {} files, {} measures; {}",
            s["run"],
            s["pipeline"].as_str().unwrap_or_default(),
            s["unit"].as_str().unwrap_or_default(),
            s["status"].as_str().unwrap_or("not over"),
            s["reused_from"]
                .as_i64()
                .map_or(String::new(), |r| format!(" (reused from run {r})")),
            s["keys"]["short"].as_str().unwrap_or("none"),
            s["results"]["derivatives"].as_array().map_or(0, Vec::len),
            s["results"]["measures"],
            s["curation"]["words"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}
