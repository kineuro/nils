// SPDX-License-Identifier: AGPL-3.0-only

//! A pack change shows its effect first (record 57 P10; Wave 7b spec
//! section 10, G14): a pack replayed over the registry, read-only, before
//! it ships. Over every session the registry's main-scan picks make (the
//! picks a run over a dataset's or a cohort's main scans reads), the pack
//! to ship and the pack the catalog runs today judge each session under
//! the same facts, and the replay counts what changes: how many sessions
//! go from run to skipped and why (by need), which variant they are routed
//! to, which knob values change and from what to what. Over the units the
//! pipeline's kept runs recorded, it counts the results the new pack would
//! make stale: a knob its rules now choose differently, or a need the
//! recorded facts now fail. Nothing is written; on Postgres the session is
//! set read-only first.

use std::collections::{BTreeMap, BTreeSet};

use nils_pipeline::descriptor::{self, Descriptor};
use nils_pipeline::pack::PipelinePack;
use nils_pipeline::pack::rules::{self, Judgement, Status};
use nils_registry::Registry;
use nils_registry::pipeline::{self as rows, Pipeline};
use nils_registry::store::{Param, Store};
use serde_json::{Map, Value, json};

use crate::pipeline_pack::{self, PersonAsk, UnitOf};

/// The sessions the registry's main-scan picks make, each with its stacks
/// by role: the population a run over `main@dataset:<name>` would read.
pub(crate) fn sessions(store: &mut Store) -> Result<Vec<UnitOf>, String> {
    let of = nils_registry::pick_truth::Of::own(store).map_err(|e| e.to_string())?;
    let picks = nils_registry::pick_truth::Picks::Live(of);
    let day = store.dialect().text_of_qualified(
        Some("p"),
        nils_registry::schema::table("pick")
            .column("session_day")
            .expect("pick.session_day"),
    );
    let sql = format!(
        "SELECT p.subject_id, {day}, p.role, p.stack_id FROM {} p \
         ORDER BY p.subject_id, p.session_day, p.role, p.stack_id",
        picks.stacks(store)
    );
    let mut units: BTreeMap<(i64, String), UnitOf> = BTreeMap::new();
    for r in store.query(&sql, &[]).map_err(|e| e.to_string())? {
        let (Ok(subject), Ok(day), Ok(role), Ok(stack)) =
            (r.int(0), r.text(1), r.text(2), r.int(3))
        else {
            continue;
        };
        let u = units
            .entry((subject, day.to_string()))
            .or_insert_with(|| UnitOf {
                by_role: BTreeMap::new(),
                subject_id: Some(subject),
                session_day: Some(day.to_string()),
            });
        u.by_role.entry(role.to_string()).or_default().push(stack);
    }
    Ok(units.into_values().collect())
}

/// The runs of a pipeline name, every version.
fn runs_of(store: &mut Store, name: &str) -> Result<Vec<i64>, String> {
    let sql = format!(
        "SELECT pr.id FROM {} pr JOIN {} pl ON pl.id = pr.pipeline_id WHERE pl.name = {} ORDER BY pr.id",
        store.qualified("pipeline_run"),
        store.qualified("pipeline"),
        store.dialect().param(1, nils_registry::schema::Type::Text)
    );
    Ok(store
        .query(&sql, &[Param::from(name)])
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|r| r.int(0).ok())
        .collect())
}

/// Nothing a replay does writes: on Postgres it reads inside one read-only
/// transaction that ends with the replay on every path, so the connection
/// (the server keeps one per handler thread) is not left read-only.
fn begin_read_only(store: &mut Store) -> Result<(), String> {
    if store.backend() == nils_registry::Backend::Postgres {
        store.batch("BEGIN READ ONLY").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn end_read_only(store: &mut Store) {
    if store.backend() == nils_registry::Backend::Postgres {
        // a transaction the replay's reads aborted is rolled back all the same
        let _ = store.rollback();
    }
}

fn runs(j: &Judgement) -> bool {
    j.status != Status::Skipped
}

/// The counts of one pack's judgements: ready, skipped (by need and words),
/// routed (by variant), cannot tell, with caveats.
fn counts(judged: &[Judgement]) -> Value {
    let count = |s: Status| judged.iter().filter(|j| j.status == s).count();
    let mut by_need: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut routed: BTreeMap<String, usize> = BTreeMap::new();
    let mut caveats: BTreeMap<String, usize> = BTreeMap::new();
    for j in judged {
        if j.status == Status::Skipped {
            *by_need
                .entry((
                    j.need.clone().unwrap_or_default(),
                    j.words.clone().unwrap_or_default(),
                ))
                .or_default() += 1;
        }
        if let Some(v) = &j.variant {
            *routed.entry(v.variant.clone()).or_default() += 1;
        }
        for c in &j.caveats {
            *caveats.entry(c.words.clone()).or_default() += 1;
        }
    }
    let mut skipped_by_need: Vec<Value> = by_need
        .into_iter()
        .map(|((need, words), n)| json!({"need": need, "words": words, "sessions": n}))
        .collect();
    skipped_by_need.sort_by(|a, b| b["sessions"].as_u64().cmp(&a["sessions"].as_u64()));
    let mut caveats: Vec<Value> = caveats
        .into_iter()
        .map(|(words, n)| json!({"words": words, "sessions": n}))
        .collect();
    caveats.sort_by(|a, b| b["sessions"].as_u64().cmp(&a["sessions"].as_u64()));
    json!({
        "run": judged.iter().filter(|j| runs(j)).count(),
        "ready": count(Status::Ready),
        "skipped": count(Status::Skipped),
        // routed is a subset of run; a skipped session that names the variant
        // it goes to is counted apart
        "routed": judged.iter().filter(|j| runs(j) && j.variant.is_some()).count(),
        "skipped_routed": judged.iter().filter(|j| !runs(j) && j.variant.is_some()).count(),
        "cannot_tell": judged.iter().filter(|j| j.cannot_tell.is_some()).count(),
        "with_caveats": judged.iter().filter(|j| !j.caveats.is_empty()).count(),
        "skipped_by_need": skipped_by_need,
        "routed_to": routed,
        "caveats": caveats,
    })
}

/// Sessions per (from, to) change, each side a value kept as JSON text.
type ChangeCounts = BTreeMap<(Option<String>, Option<String>), usize>;

/// A value kept as JSON text, back as the value; none is null.
fn json_of(text: Option<String>) -> Value {
    text.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// A value as a person reads it: a string bare, none as "(none)".
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "(none)".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// What changes from `before` to `now` over the same sessions.
fn changes(before: &[Judgement], now: &[Judgement]) -> Value {
    let mut run_to_skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped_to_run = 0usize;
    // from and to are JSON values as the engine holds them, null for none;
    // keyed by their JSON text so equal values count together
    let mut variant: BTreeMap<(Option<String>, Option<String>), usize> = BTreeMap::new();
    let mut knobs: BTreeMap<String, ChangeCounts> = BTreeMap::new();
    for (b, n) in before.iter().zip(now) {
        match (runs(b), runs(n)) {
            (true, false) => {
                *run_to_skipped
                    .entry(n.words.clone().unwrap_or_default())
                    .or_default() += 1;
            }
            (false, true) => skipped_to_run += 1,
            _ => {}
        }
        let (bv, nv) = (
            b.variant.as_ref().map(|v| v.variant.clone()),
            n.variant.as_ref().map(|v| v.variant.clone()),
        );
        if bv != nv {
            *variant.entry((bv, nv)).or_default() += 1;
        }
        if !runs(n) {
            continue;
        }
        for (k, c) in &n.knobs {
            let was = b.knobs.get(k).map(|c| c.value.clone());
            if was.as_ref() != Some(&c.value) {
                *knobs
                    .entry(k.clone())
                    .or_default()
                    .entry((was.map(|v| v.to_string()), Some(c.value.to_string())))
                    .or_default() += 1;
            }
        }
    }
    let mut run_to_skipped_by: Vec<Value> = run_to_skipped
        .iter()
        .map(|(words, n)| json!({"words": words, "sessions": n}))
        .collect();
    run_to_skipped_by.sort_by(|a, b| b["sessions"].as_u64().cmp(&a["sessions"].as_u64()));
    json!({
        "run_to_skipped": run_to_skipped.values().sum::<usize>(),
        "run_to_skipped_by_need": run_to_skipped_by,
        "skipped_to_run": skipped_to_run,
        "variant": variant.into_iter().map(|((from, to), n)| json!({"from": from, "to": to, "sessions": n})).collect::<Vec<_>>(),
        "knobs": knobs.into_iter().map(|(k, m)| (k, json!(m.into_iter().map(|((from, to), n)| json!({"from": json_of(from), "to": json_of(to), "sessions": n})).collect::<Vec<_>>()))).collect::<Map<_, _>>(),
    })
}

/// The kept results of a pipeline name the new pack would make stale: each
/// unit record judged again from the facts it recorded.
fn stale(
    store: &mut Store,
    name: &str,
    d: &Descriptor,
    pack: &PipelinePack,
) -> Result<Value, String> {
    let run_ids = runs_of(store, name)?;
    // only the units that made a result, in runs that ended, with a pack record
    let records = rows::kept_unit_records(store, name).map_err(|e| e.to_string())?;
    let (mut kept, mut by_knobs, mut by_needs, mut unreplayable, mut unjudged, mut skipped) =
        (0, 0, 0, 0, 0, 0);
    let mut stale_runs: BTreeSet<i64> = BTreeSet::new();
    let mut knob_changes: BTreeMap<String, usize> = BTreeMap::new();
    let mut stale_units = 0usize;
    for (run, _unit, _key, record) in &records {
        if !record["knobs"].is_object() {
            unjudged += 1;
            continue;
        }
        // a unit its pack skipped made nothing: nothing of it goes stale
        if record["status"] == "skipped" {
            skipped += 1;
            continue;
        }
        kept += 1;
        let j = match rules::rejudge(pack, d, record) {
            Ok(j) => j,
            Err(_) => {
                unreplayable += 1;
                continue;
            }
        };
        let mut is_stale = false;
        if j.status == Status::Skipped {
            by_needs += 1;
            is_stale = true;
        }
        let mut differs = false;
        for (k, c) in record["knobs"].as_object().into_iter().flatten() {
            if j.knobs.get(k).map(|n| &n.value) != Some(&c["value"]) {
                differs = true;
                *knob_changes.entry(k.clone()).or_default() += 1;
            }
        }
        if differs {
            by_knobs += 1;
            is_stale = true;
        }
        if is_stale {
            stale_units += 1;
            stale_runs.insert(*run);
        }
    }
    Ok(json!({
        "runs": run_ids.len(),
        "units": kept,
        "unjudged": unjudged,
        "skipped": skipped,
        "stale": stale_units,
        "stale_by_knobs": by_knobs,
        "stale_by_needs": by_needs,
        "knobs": knob_changes,
        "unreplayable": unreplayable,
        "runs_touched": stale_runs,
    }))
}

/// Replay a folder's pack (`d`, `pack`) over the registry, against the
/// catalog version `against` where there is one, under the person's goal
/// and switches. The answer: `{pipeline, pack: {digest, version}, against,
/// sessions, without_inputs, now, before, changes, kept}`.
pub(crate) fn replay_folder(
    registry: &mut Registry,
    d: &Descriptor,
    pack: &PipelinePack,
    against: Option<&Pipeline>,
    ask: &PersonAsk,
) -> Result<Value, String> {
    let host = pipeline_pack::host_facts(registry);
    let store = registry.store();
    begin_read_only(store)?;
    let done = replay_in(store, d, pack, against, ask, &host);
    end_read_only(store);
    done
}

fn replay_in(
    store: &mut Store,
    d: &Descriptor,
    pack: &PipelinePack,
    against: Option<&Pipeline>,
    ask: &PersonAsk,
    host: &pipeline_pack::HostFacts,
) -> Result<Value, String> {
    let all = sessions(store)?;
    let (units, without): (Vec<UnitOf>, Vec<UnitOf>) = all
        .into_iter()
        .partition(|u| d.roles.iter().all(|r| u.by_role.contains_key(r)));
    let facts = pipeline_pack::facts_for(store, &units, host);
    let person = ask.person(pack, &crate::actor(), &[])?;
    let resolved = d.resolve(&[])?;
    let now: Vec<Judgement> = facts
        .iter()
        .map(|f| rules::judge(pack, d, &resolved, f, &person))
        .collect();
    let mut before_doc = Value::Null;
    let mut changes_doc = Value::Null;
    let mut against_doc = Value::Null;
    if let Some(p) = against {
        if let Some(e) = pipeline_pack::pack_error(p) {
            return Err(e);
        }
        against_doc = json!({
            "pipeline": p.label(), "pipeline_id": p.id, "pack_digest": p.pack_digest,
            "same_pack": p.pack_digest.as_deref() == Some(pack.digest().as_str()),
        });
        let old_d = descriptor::from_value(p.descriptor.clone())?;
        let old_resolved = old_d.resolve(&[])?;
        let before: Vec<Judgement> = match pipeline_pack::pack_of(p) {
            Some(old) => {
                // the same facts; the old pack's own switches where it has
                // them, else its defaults
                let old_ask = PersonAsk {
                    goal: ask.goal.clone(),
                    switches: ask
                        .switches
                        .iter()
                        .filter(|(k, _)| old.goals.switches.contains_key(*k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    let_run: false,
                };
                let old_person = old_ask.person(&old, &crate::actor(), &[])?;
                facts
                    .iter()
                    .map(|f| rules::judge(&old, &old_d, &old_resolved, f, &old_person))
                    .collect()
            }
            // no pack: every unit with its inputs runs with the defaults
            None => facts
                .iter()
                .map(|_| Judgement {
                    status: Status::Ready,
                    need: None,
                    words: None,
                    cannot_tell: None,
                    let_run_by: None,
                    variant: None,
                    caveats: Vec::new(),
                    knobs: old_resolved
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                rules::Chosen {
                                    value: v.clone(),
                                    by: "default".into(),
                                    rule: None,
                                    facts: BTreeMap::new(),
                                    proposed: None,
                                },
                            )
                        })
                        .collect(),
                    facts: Default::default(),
                })
                .collect(),
        };
        before_doc = counts(&before);
        changes_doc = changes(&before, &now);
    }
    let kept = stale(store, &d.name, d, pack)?;
    Ok(json!({
        "pipeline": d.name,
        "pack": {"digest": pack.digest(), "version": pack.version, "descriptor_digest": d.digest()},
        "against": against_doc,
        "sessions": units.len() + without.len(),
        "with_inputs": units.len(),
        "without_inputs": without.len(),
        "population": if units.len() + without.len() == 0 {
            json!({"sessions": 0, "note": "no main-scan picks are saved; nothing was judged"})
        } else {
            json!({"sessions": units.len() + without.len()})
        },
        "person": ask.doc(),
        "now": counts(&now),
        "before": before_doc,
        "changes": changes_doc,
        "kept": kept,
        "read_only": true,
    }))
}

/// The replay as a person reads it.
pub(crate) fn words(doc: &Value) -> String {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let mut out = Vec::new();
    out.push(format!(
        "{} pack {} replayed over {} sessions ({} with the pipeline's roles, {} without), read-only",
        doc["pipeline"].as_str().unwrap_or_default(),
        doc["pack"]["digest"].as_str().map(nils_pipeline::pack::keys::short).unwrap_or_default(),
        n(&doc["sessions"]),
        n(&doc["with_inputs"]),
        n(&doc["without_inputs"])
    ));
    if let Some(note) = doc["population"]["note"].as_str() {
        out.push(format!("note: {note}"));
    }
    let now = &doc["now"];
    out.push(format!(
        "now: {} run ({} routed to a variant), {} skipped ({} of them routed to a variant, {} cannot tell), {} with caveats",
        n(&now["run"]),
        n(&now["routed"]),
        n(&now["skipped"]),
        n(&now["skipped_routed"]),
        n(&now["cannot_tell"]),
        n(&now["with_caveats"])
    ));
    for s in now["skipped_by_need"].as_array().into_iter().flatten() {
        out.push(format!(
            "  skipped {}: {}",
            n(&s["sessions"]),
            s["words"].as_str().unwrap_or_default()
        ));
    }
    for (v, k) in now["routed_to"].as_object().into_iter().flatten() {
        out.push(format!("  routed {}: {v}", n(k)));
    }
    for c in now["caveats"].as_array().into_iter().flatten() {
        out.push(format!(
            "  caveat {}: {}",
            n(&c["sessions"]),
            c["words"].as_str().unwrap_or_default()
        ));
    }
    if doc["against"].is_object() {
        let ch = &doc["changes"];
        out.push(format!(
            "against {}: {} sessions change from run to skipped, {} from skipped to run",
            doc["against"]["pipeline"].as_str().unwrap_or_default(),
            n(&ch["run_to_skipped"]),
            n(&ch["skipped_to_run"])
        ));
        for s in ch["run_to_skipped_by_need"]
            .as_array()
            .into_iter()
            .flatten()
        {
            out.push(format!(
                "  run to skipped {}: {}",
                n(&s["sessions"]),
                s["words"].as_str().unwrap_or_default()
            ));
        }
        for v in ch["variant"].as_array().into_iter().flatten() {
            out.push(format!(
                "  variant {}: {} to {}",
                n(&v["sessions"]),
                shown(&v["from"]),
                shown(&v["to"])
            ));
        }
        for (k, list) in ch["knobs"].as_object().into_iter().flatten() {
            for c in list.as_array().into_iter().flatten() {
                out.push(format!(
                    "  knob {k} {}: {} to {}",
                    n(&c["sessions"]),
                    shown(&c["from"]),
                    shown(&c["to"])
                ));
            }
        }
    } else {
        out.push("against: nothing in the catalog (a first pack)".to_string());
    }
    let kept = &doc["kept"];
    out.push(format!(
        "kept results: {} units of {} runs, {} would be stale ({} by a knob, {} by a need), {} unjudged",
        n(&kept["units"]),
        n(&kept["runs"]),
        n(&kept["stale"]),
        n(&kept["stale_by_knobs"]),
        n(&kept["stale_by_needs"]),
        n(&kept["unjudged"])
    ));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::home::{Home, InitOptions};
    use nils_registry::{Backend, Scheme};

    /// Item 1 of the review: a replay on Postgres reads inside a read-only
    /// transaction that ends with it, so the connection (the server keeps one
    /// per handler thread) still writes afterwards, on every path.
    #[test]
    fn a_replay_leaves_the_postgres_connection_writable() {
        let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
            .ok()
            .filter(|d| !d.is_empty())
        else {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        };
        const SCHEMA: &str = "nils_replay_read_only";
        let mut admin = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        admin
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .unwrap();
        let dir = nils_dicom::synth::TempDir::new("replay-read-only");
        let home = Home::new(dir.path());
        home.keys(None).add("k", b"replay-read-only-key").unwrap();
        let mut registry = home
            .init(&InitOptions {
                backend: Backend::Postgres,
                dsn: Some(dsn.clone()),
                schema: Some(SCHEMA.to_string()),
                scheme: Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: 12,
                session_scheme: None,
            })
            .unwrap();
        let folder =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pipelines/synthstrip");
        let d = descriptor::parse(
            &std::fs::read_to_string(folder.join(nils_pipeline::pack::DESCRIPTOR_FILE)).unwrap(),
        )
        .unwrap();
        let pack = pipeline_pack::load(
            registry.store(),
            &d,
            &std::fs::read_to_string(folder.join(nils_pipeline::pack::FILE)).unwrap(),
        )
        .unwrap();
        let ask = PersonAsk::default();
        let write = |registry: &mut Registry, n: i64| {
            let store = registry.store();
            store
                .batch(&format!(
                    "CREATE TABLE IF NOT EXISTS {SCHEMA}.replay_probe (n bigint)"
                ))
                .unwrap();
            store
                .execute(
                    &format!("INSERT INTO {SCHEMA}.replay_probe (n) VALUES ({n})"),
                    &[],
                )
                .unwrap();
        };
        // the answer path, then the path of an error: a replay against a
        // version whose pack cannot be read ends before it judges
        let doc = replay_folder(&mut registry, &d, &pack, None, &ask).unwrap();
        assert_eq!(doc["read_only"], true);
        write(&mut registry, 1);
        // a replay that fails halfway still ends its transaction
        let store = registry.store();
        begin_read_only(store).unwrap();
        assert!(
            store
                .execute(
                    &format!("INSERT INTO {SCHEMA}.replay_probe (n) VALUES (9)"),
                    &[]
                )
                .is_err(),
            "the replay's transaction refuses writes"
        );
        end_read_only(store);
        write(&mut registry, 2);
        let rows = registry
            .store()
            .query(
                &format!("SELECT n FROM {SCHEMA}.replay_probe ORDER BY n"),
                &[],
            )
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter_map(|r| r.int(0).ok())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        admin
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .ok();
    }

    /// Item 7 of the review: no saved pick rules means no sessions, and the
    /// answer and the words say why.
    #[test]
    fn a_registry_with_no_saved_picks_says_why_nothing_was_judged() {
        let dir = nils_dicom::synth::TempDir::new("replay-no-picks");
        let home = Home::new(dir.path());
        home.keys(None).add("k", b"replay-no-picks-key").unwrap();
        let mut registry = home
            .init(&InitOptions {
                backend: Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: 12,
                session_scheme: None,
            })
            .unwrap();
        let folder =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pipelines/synthstrip");
        let d = descriptor::parse(
            &std::fs::read_to_string(folder.join(nils_pipeline::pack::DESCRIPTOR_FILE)).unwrap(),
        )
        .unwrap();
        let pack = pipeline_pack::load(
            registry.store(),
            &d,
            &std::fs::read_to_string(folder.join(nils_pipeline::pack::FILE)).unwrap(),
        )
        .unwrap();
        let doc = replay_folder(&mut registry, &d, &pack, None, &PersonAsk::default()).unwrap();
        assert_eq!(doc["population"]["sessions"], 0, "{doc}");
        assert_eq!(
            doc["population"]["note"],
            "no main-scan picks are saved; nothing was judged"
        );
        assert!(
            words(&doc).contains("note: no main-scan picks are saved; nothing was judged"),
            "{}",
            words(&doc)
        );
    }

    #[test]
    fn values_are_shown_bare_and_none_is_not_a_value() {
        assert_eq!(shown(&json!("clinical")), "clinical");
        assert_eq!(shown(&json!(true)), "true");
        assert_eq!(shown(&json!(1.5)), "1.5");
        assert_eq!(shown(&Value::Null), "(none)");
        assert_eq!(shown(&json!("(none)")), "(none)");
        assert_eq!(json_of(None), Value::Null);
        assert_eq!(json_of(Some("\"a\"".into())), json!("a"));
    }
}
