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

/// The curation state of a run's inputs, read once for every stack the
/// run's units hold (after review, finding 5: not once per unit): per
/// stack the pack and version that classified it (and under which
/// overlay), the deployment pack's digest where its folder is readable,
/// the label set the run read, System 1's answers on the stacks (a model's
/// committed decisions, with its version), and the axes a person
/// overrode. [`Curation::of`] slices it per unit, which the unit records
/// at its run (record 58 P5); the door reads the state again to tell
/// `stale`.
#[derive(Debug)]
pub(crate) struct Curation {
    rules: Vec<Value>,
    overrides: Vec<Value>,
    system1: Vec<Value>,
    pack_digest: Option<String>,
    label_set: Value,
}

impl Curation {
    /// The state as the registry answers it, or the registry's error: a
    /// query that fails never reads as a state with no rules and no
    /// overrides (after review, finding 4 of the second review); the run
    /// records [`Curation::unknown`] for its units then.
    pub(crate) fn read(
        registry: &mut Registry,
        stacks: &[i64],
        label_set: Option<&nils_registry::labels::LabelSet>,
        pack_dir: &Option<std::path::PathBuf>,
    ) -> Result<Curation, String> {
        let store = registry.store();
        let rules = rules_of(store, stacks)?;
        let (overrides, system1) = decisions_of(store, stacks)?;
        Ok(Curation {
            rules,
            overrides,
            system1,
            pack_digest: pack_dir.as_deref().and_then(pack_digest),
            label_set: label_set.map_or(Value::Null, |l| {
                json!({"id": l.id, "digest": l.digest, "label": format!("{}@{}", l.name, l.version)})
            }),
        })
    }

    /// What a unit records when the state could not be read when it ran:
    /// `unknown`, with the registry's words, which the doors answer as they
    /// stand (never `current`).
    pub(crate) fn unknown(why: &str) -> Value {
        json!({
            "state": "unknown",
            "words": format!("unknown: the curation state could not be read when the unit ran ({why})"),
            "at": nils_registry::time::now_iso(),
        })
    }

    /// The state of one unit's stacks, as the unit records it.
    pub(crate) fn of(&self, stacks: &[i64]) -> Value {
        let mine = |rows: &[Value]| -> Vec<Value> {
            rows.iter()
                .filter(|r| r["stack"].as_i64().is_some_and(|s| stacks.contains(&s)))
                .cloned()
                .collect()
        };
        json!({
            "rules": mine(&self.rules),
            "pack_digest": self.pack_digest,
            "label_set": self.label_set,
            "system1": mine(&self.system1),
            "overrides": mine(&self.overrides),
            "at": nils_registry::time::now_iso(),
        })
    }
}

/// Per stack, `{stack, pack, version, overlay, epoch}` of its
/// classification; the registry's error where the query fails.
fn rules_of(store: &mut Store, stacks: &[i64]) -> Result<Vec<Value>, String> {
    let err = |e: nils_registry::Error| format!("the classification rows: {e}");
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
        for r in store.query(&sql, &[]).map_err(err)? {
            out.push(json!({
                "stack": r.int(0).map_err(err)?,
                "pack": r.text(1).map_err(err)?,
                "version": r.text(2).map_err(err)?,
                "overlay": r.opt_text(3).map_err(err)?,
                "epoch": r.opt_int(4).map_err(err)?,
            }));
        }
    }
    Ok(out)
}

/// The committed decisions on the stacks: a person's as overrides `{stack,
/// axis, by, at}`, a model's as System 1's `{stack, axis, model, version}`;
/// the registry's error where the query fails.
#[allow(clippy::type_complexity)]
fn decisions_of(store: &mut Store, stacks: &[i64]) -> Result<(Vec<Value>, Vec<Value>), String> {
    let err = |e: nils_registry::Error| format!("the decision rows: {e}");
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
        for r in store.query(&sql, &[]).map_err(err)? {
            let stack: i64 = r.text(0).map_err(err)?.parse().unwrap_or_default();
            let axis = r.text(1).map_err(err)?.to_string();
            let kind = r.text(3).map_err(err)?.to_string();
            match kind.as_str() {
                "model" => system1.push(json!({
                    "stack": stack, "axis": axis,
                    "model": r.opt_int(5).map_err(err)?,
                    "version": r.opt_text(4).map_err(err)?,
                })),
                _ => overrides.push(json!({
                    "stack": stack, "axis": axis, "by": r.text(2).map_err(err)?,
                    "kind": kind, "at": r.opt_text(6).map_err(err)?,
                })),
            }
        }
    }
    Ok((overrides, system1))
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

/// What holds now of some stacks' curation, read once for every unit a
/// door answers (after review, finding 6: not twice per unit): the rules
/// per stack and the overrides.
#[derive(Debug)]
pub(crate) struct States {
    now: Vec<Value>,
    overrides: Vec<Value>,
}

impl States {
    /// What holds now, or the registry's error (never an empty state that
    /// would read every unit `current`).
    pub(crate) fn read(store: &mut Store, stacks: &[i64]) -> Result<States, String> {
        let now = rules_of(store, stacks)?;
        let (overrides, _) = decisions_of(store, stacks)?;
        Ok(States { now, overrides })
    }

    /// The curation state of a unit as a door answers it: what was
    /// recorded, what holds now, and `state`: `current`, `stale` (a rules
    /// version on an input changed, with [`stale_words`]) or `unknown` (a
    /// unit from before recorded none, or one whose state could not be read
    /// when it ran, in the words it recorded).
    pub(crate) fn of(&self, recorded: &Value, stacks: &[i64]) -> Value {
        if !recorded.is_object() {
            return json!({
                "state": "unknown",
                "words": "no curation state was recorded for this unit (it ran before the provenance door)",
                "recorded": Value::Null,
            });
        }
        if recorded["state"] == "unknown" {
            return json!({
                "state": "unknown",
                "words": recorded["words"],
                "recorded": recorded,
            });
        }
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
        for r in &self.now {
            let Some(stack) = r["stack"].as_i64() else {
                continue;
            };
            if !stacks.contains(&stack) {
                continue;
            }
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
        let overrides_now: Vec<Value> = self
            .overrides
            .iter()
            .filter(|o| o["stack"].as_i64().is_some_and(|s| stacks.contains(&s)))
            .cloned()
            .collect();
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

/// What the runs a door answers made, read once per source run (after
/// review, finding 7: not once per unit): the derivatives of each run and
/// its measures counted by unit.
#[derive(Default)]
struct Made {
    derivatives: BTreeMap<i64, Vec<nils_registry::derivative::Derivative>>,
    measures: BTreeMap<i64, BTreeMap<String, usize>>,
}

impl Made {
    fn read(&mut self, store: &mut Store, run: i64) -> Result<(), String> {
        if self.derivatives.contains_key(&run) {
            return Ok(());
        }
        let files = nils_registry::derivative::list(
            store,
            &nils_registry::derivative::Filter {
                run_id: Some(run),
                limit: 10_000,
                ..Default::default()
            },
        )
        .map_err(|e| format!("the derivatives of run {run}: {e}"))?
        .into_iter()
        .filter(|d| d.withdrawn_at.is_none())
        .collect();
        self.derivatives.insert(run, files);
        let mut by_unit: BTreeMap<String, usize> = BTreeMap::new();
        for m in nils_registry::measure::of_run(store, run)
            .map_err(|e| format!("the measures of run {run}: {e}"))?
        {
            *by_unit.entry(m.unit_id).or_default() += 1;
        }
        self.measures.insert(run, by_unit);
        Ok(())
    }

    /// The results of one unit: its files as derivatives and how many
    /// measures it wrote, from the run that made them (the unit's own, or
    /// the one it reused). A file is the unit's as the reuse decides it
    /// ([`crate::reuse::belongs`]): a stack's by its stack, a session's by
    /// the unit's subject and day, a subject's by a subject unit's subject
    /// (after review, finding 1 of the second review: never every `sub-`
    /// unit's).
    fn of(
        &mut self,
        store: &mut Store,
        run: i64,
        unit: &rows::Unit,
        stacks: &[i64],
        subject_id: Option<i64>,
        session_day: Option<&str>,
    ) -> Result<Value, String> {
        let source = unit.reused_from_run.unwrap_or(run);
        self.read(store, source)?;
        let files: Vec<Value> = self
            .derivatives
            .get(&source)
            .into_iter()
            .flatten()
            .filter(|d| crate::reuse::belongs(d, stacks, subject_id, session_day))
            .map(|d| {
                json!({
                    "id": d.id, "kind": d.kind, "path": d.path, "sha256": d.sha256,
                    "check": d.check_status, "run": d.run_id, "created_at": d.created_at,
                })
            })
            .collect();
        let measures = self
            .measures
            .get(&source)
            .and_then(|m| m.get(&unit.unit))
            .copied()
            .unwrap_or(0);
        Ok(json!({"derivatives": files, "measures": measures, "from_run": source}))
    }
}

/// A unit's subject and its session's day, as the runs name it: a
/// `sub-<s>_ses-<t>` unit is its stacks' session, a `sub-<s>` unit its
/// subject with no day, a `stack-<id>` unit neither.
fn unit_scope(
    unit: &str,
    stacks: &[i64],
    sessions: &BTreeMap<i64, (i64, String)>,
) -> (Option<i64>, Option<String>) {
    if !unit.starts_with("sub-") {
        return (None, None);
    }
    let Some((subject, day)) = stacks.iter().find_map(|s| sessions.get(s)) else {
        return (None, None);
    };
    if unit.contains("_ses-") {
        (Some(*subject), Some(day.clone()))
    } else {
        (Some(*subject), None)
    }
}

/// A run that touched the target: the run, its pipeline, and the units of
/// it that hold a stack of the target, each with its stacks.
type Touched = (rows::Run, rows::Pipeline, Vec<(rows::Unit, Vec<i64>)>);

/// `GET /api/provenance/{subject|session|stack}/{id}` and `nils provenance`:
/// every step that ran on the target, with its keys, its results and its
/// curation state.
pub(crate) fn answer(store: &mut Store, t: &Target) -> Result<Value, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let stacks = stacks_of(store, t)?;
    let runs = runs_touching(store, &stacks)?;
    // the units of every run that touched the target, with their stacks,
    // and the curation now of every stack they hold, read once
    let mut touched: Vec<Touched> = Vec::new();
    let mut every_stack: BTreeSet<i64> = BTreeSet::new();
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
        let mut units = Vec::new();
        for u in rows::units(store, run_id).map_err(err)? {
            let its: Vec<i64> = of_units.get(&u.unit).cloned().unwrap_or_default();
            if !its.iter().any(|s| stacks.contains(s)) {
                continue;
            }
            every_stack.extend(its.iter().copied());
            units.push((u, its));
        }
        touched.push((run, p, units));
    }
    let every_stack: Vec<i64> = every_stack.into_iter().collect();
    let states = States::read(store, &every_stack)?;
    let sessions = crate::pipelines::stack_sessions(store, &every_stack)?;
    let mut made = Made::default();
    let mut steps: Vec<Value> = Vec::new();
    for (run, p, units) in &touched {
        let run_id = run.id;
        for (u, its) in units {
            let curation = states.of(&u.curation, its);
            let (subject_id, session_day) = unit_scope(&u.unit, its, &sessions);
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
                "results": made.of(store, run_id, u, its, subject_id, session_day.as_deref())?,
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
    Ok(assembled(t.doc(), stacks, steps))
}

/// The door's answer over these steps: the counts and the stale words are
/// of the steps answered, so a withholding recomputes them (after review,
/// finding 5 of the second review).
fn assembled(of: Value, stacks: Vec<i64>, steps: Vec<Value>) -> Value {
    let count = |state: &str| {
        steps
            .iter()
            .filter(|s| s["curation"]["state"] == state)
            .count()
    };
    let stale = count("stale");
    json!({
        "of": of,
        "stacks": stacks,
        "counts": {"steps": steps.len(), "stale": stale, "unknown": count("unknown")},
        "stale_words": if stale > 0 { Some("stale: the rules version on an input changed; the result is read apart from current ones and never pooled with them, at every door") } else { None },
        "steps": steps,
    })
}

/// The answer with the steps of a stack of a sample sealed now withheld,
/// and the stack itself, the counts and the stale words with them: a
/// caller without `sealed:see` learns nothing of a sealed stack, not even
/// its id or that something on it is stale.
fn withheld(mut v: Value, sealed: &BTreeSet<i64>) -> Value {
    if sealed.is_empty() {
        return v;
    }
    let is_sealed = |x: &Value| x.as_i64().is_some_and(|x| sealed.contains(&x));
    let before = v["steps"].as_array().map_or(0, Vec::len);
    let kept: Vec<Value> = v["steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            !s["stacks"]
                .as_array()
                .is_some_and(|ss| ss.iter().any(is_sealed))
        })
        .collect();
    let stacks: Vec<i64> = v["stacks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|x| !is_sealed(x))
        .filter_map(Value::as_i64)
        .collect();
    let mut out = assembled(v["of"].take(), stacks, kept);
    out["withheld"] = json!({
        "steps": before - out["steps"].as_array().map_or(0, Vec::len),
        "why": crate::sealed::refusal("the steps of a stack of a sample sealed now"),
    });
    out
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
        let v = answer(registry.store(), &t).map_err(|e| Reply::error(500, e))?;
        // every stack a step answered holds, not only the target's: a
        // session's step may hold a stack of the target's subject that the
        // target itself does not
        let stacks: Vec<i64> = v["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["stacks"].as_array().cloned().unwrap_or_default())
            .chain(v["stacks"].as_array().cloned().unwrap_or_default())
            .filter_map(|x| x.as_i64())
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .collect();
        let sealed = crate::sealed::withheld(registry.store(), &caller.access, &stacks)
            .map_err(|e| Reply::error(500, e.to_string()))?;
        let mut v = withheld(v, &sealed);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(name: &str) -> (nils_dicom::synth::TempDir, Registry) {
        let dir = nils_dicom::synth::TempDir::new(name);
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let registry = home
            .init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        (dir, registry)
    }

    /// After review (finding 4 of the second review): a registry error is
    /// an error at every read, never a state with no rules and no
    /// overrides that would read `current`; and the unknown state a run
    /// records when the read failed reads `unknown` at the door.
    #[test]
    fn a_failing_query_is_an_error_not_a_current_state() {
        let (_dir, mut registry) = registry("provenance-errors");
        let store = registry.store();
        assert!(States::read(store, &[1]).is_ok());
        store
            .execute(
                "ALTER TABLE classification RENAME TO classification_gone",
                &[],
            )
            .unwrap();
        let e = States::read(store, &[1]).unwrap_err();
        assert!(e.starts_with("the classification rows: "), "{e}");
        let e = Curation::read(&mut registry, &[1], None, &None).unwrap_err();
        assert!(e.starts_with("the classification rows: "), "{e}");
        let store = registry.store();
        store
            .execute("ALTER TABLE derivative RENAME TO derivative_gone", &[])
            .unwrap();
        let mut made = Made::default();
        let e = made.read(store, 1).unwrap_err();
        assert!(e.starts_with("the derivatives of run 1: "), "{e}");
        let states = States {
            now: vec![json!({"stack": 1, "pack": "mri", "version": "1.0.0"})],
            overrides: Vec::new(),
        };
        let v = states.of(&Curation::unknown("the classification rows: gone"), &[1]);
        assert_eq!(v["state"], "unknown", "{v}");
        assert!(
            v["words"]
                .as_str()
                .unwrap()
                .starts_with("unknown: the curation state could not be read when the unit ran"),
            "{v}"
        );
    }

    /// After review (finding 5 of the second review): withholding the steps
    /// of a sealed stack withholds the stack from `stacks` too, and the
    /// counts and the stale words are those of the steps kept.
    #[test]
    fn withholding_a_sealed_stack_takes_its_stack_counts_and_words_too() {
        let v = assembled(
            json!({"kind": "subject", "id": 1}),
            vec![1, 2],
            vec![
                json!({"unit": "stack-1", "stacks": [1], "curation": {"state": "stale"}}),
                json!({"unit": "stack-2", "stacks": [2], "curation": {"state": "current"}}),
            ],
        );
        assert_eq!(v["counts"], json!({"steps": 2, "stale": 1, "unknown": 0}));
        assert!(v["stale_words"].is_string(), "{v}");
        let w = withheld(v.clone(), &BTreeSet::new());
        assert_eq!(w, v, "nothing sealed, nothing withheld");
        let w = withheld(v, &BTreeSet::from([1]));
        assert_eq!(w["stacks"], json!([2]), "{w}");
        assert_eq!(w["steps"].as_array().unwrap().len(), 1, "{w}");
        assert_eq!(w["counts"], json!({"steps": 1, "stale": 0, "unknown": 0}));
        assert!(w["stale_words"].is_null(), "{w}");
        assert_eq!(w["withheld"]["steps"], 1, "{w}");
    }

    /// After review (finding 1 of the second review): a session unit's
    /// files are its session's, a subject unit's its subject's, a stack
    /// unit's its stack's; never another session's of the same run.
    #[test]
    fn a_units_files_are_its_own_sessions() {
        let sessions: BTreeMap<i64, (i64, String)> = [
            (1, (7, "2022-01-15".to_string())),
            (2, (7, "2022-01-15".to_string())),
            (3, (7, "2023-03-10".to_string())),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            unit_scope("sub-x_ses-a", &[1, 2], &sessions),
            (Some(7), Some("2022-01-15".to_string()))
        );
        assert_eq!(unit_scope("sub-x", &[1, 2, 3], &sessions), (Some(7), None));
        assert_eq!(unit_scope("stack-1", &[1], &sessions), (None, None));
        let d = |scope: &str, stack: Option<i64>, day: Option<&str>| {
            nils_registry::derivative::Derivative {
                id: 1,
                kind: "image".into(),
                scope: scope.into(),
                stack_id: stack,
                series_id: None,
                subject_id: Some(7),
                session_day: day.map(str::to_string),
                place_id: 1,
                path: "p".into(),
                bytes: 0,
                sha256: String::new(),
                media_type: String::new(),
                registered_by: None,
                actor: Value::Null,
                model_id: None,
                run_id: Some(1),
                preprocess_version: None,
                supersedes_id: None,
                created_at: String::new(),
                withdrawn_at: None,
                grid: Value::Null,
                grid_stack_id: None,
                grid_link: None,
                part_sets: Value::Null,
                check_status: None,
                check_reasons: Value::Null,
                check_decision_id: None,
                checked_at: None,
            }
        };
        let belongs = crate::reuse::belongs;
        let a = d("session", None, Some("2022-01-15"));
        let b = d("session", None, Some("2023-03-10"));
        assert!(belongs(&a, &[1, 2], Some(7), Some("2022-01-15")));
        assert!(!belongs(&b, &[1, 2], Some(7), Some("2022-01-15")));
        assert!(
            !belongs(&a, &[1, 2, 3], Some(7), None),
            "a subject unit takes the subject's files"
        );
        assert!(belongs(
            &d("subject", None, None),
            &[1, 2, 3],
            Some(7),
            None
        ));
        assert!(belongs(&d("stack", Some(1), None), &[1], None, None));
        assert!(!belongs(&d("stack", Some(3), None), &[1], None, None));
    }
}
