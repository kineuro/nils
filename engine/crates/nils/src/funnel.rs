// SPDX-License-Identifier: AGPL-3.0-only

//! The yield funnel and the error-budget line (record 58 P4, Wave 7b spec
//! section 15, slice A8): `GET /api/funnel?selection=<id>&pipeline=<id>`
//! and `nils funnel <selection> <pipeline>`, the same document at both.
//!
//! The rows, in order:
//!
//! 1. **selection, row 0**: the stacks the selection starts from, its ask
//!    with no `where` filter;
//! 2. **filter, one row each**: the ask with its `where` filters added one
//!    at a time, set by set in the order of the sets' names and, within a
//!    set, as written; the last of them is the selection itself, frozen as
//!    a run freezes it. Every other part of the ask (its sources, `has`,
//!    `pick`, algebra) stays in every row. A filter is named by its words
//!    and the axes it reads; a filter on an identifying field (a code, a
//!    UID, a path, a name) is named by its field alone, never its values;
//! 3. **units**: the stacks that reach a unit of the run, as the pre-flight
//!    plans a new run with the descriptor's defaults;
//! 4. **inputs**: the units that have every input the pipeline takes, with
//!    the units lacking one counted by what they lack;
//! 5. **need, one row each**: the units left after each hard need of the
//!    pack, in the order the pack judges them (role by role, as written; a
//!    unit is skipped by the first hard need it fails, so each row removes
//!    the units whose first failing need it is); a unit whose need cannot
//!    be told is skipped by it unless the person lets it run, and counted
//!    as `cannot_tell`. A family's needs are its document's.
//!
//! Each row counts subjects, sessions and stacks, broken down by year,
//! base and 2D or 3D (`nils_registry::funnel`), and what it removed from
//! the row before. The last line is the error-budget line of
//! `nils_registry::funnel::budget`, over the filter rows that read an axis.
//! Counts only: no subject code, path or name leaves this door.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nils_ask::ast::{Clause, Grain};
use nils_registry::Registry;
use nils_registry::funnel::{self as count, Bounds, Read};
use nils_registry::home::Home;
use serde_json::{Value, json};

use crate::pipeline_pack::PersonAsk;
use crate::serve::Reply;
use crate::{Exit, fail, usage};

/// The door, as the capabilities list it.
pub(crate) const DOOR: &str = "GET /api/funnel";

/// The policy row of [`DOOR`].
pub(crate) const POLICY: (&str, bool, bool, &str, &str, &str, &str) = (
    DOOR,
    false,
    false,
    "job",
    "one selection and one pipeline or family",
    "Counting a funnel",
    "Counted a funnel",
);

/// The set the rows freeze at stack grain, under a name no selection
/// uses.
const STACKS: &str = "funnel_stacks";

/// What the funnel is asked.
pub(crate) struct Asked {
    pub selection: String,
    pub pipeline: String,
    pub certificate: Option<i64>,
    pub person: PersonAsk,
}

/// One `where` filter of the selection's ask.
struct Filter {
    set: String,
    index: usize,
    clause: Clause,
}

/// The field names a filter is not named by: their values identify.
fn identifying(path: &str) -> bool {
    let last = path.rsplit('.').next().unwrap_or(path).to_ascii_lowercase();
    last.ends_with("code")
        || last.contains("uid")
        || last.contains("path")
        || last.contains("patient")
        || last.contains("accession")
        || last == "name"
}

/// The refs of a clause, by op.
fn refs<'a>(c: &'a Clause, op: &str) -> Vec<&'a str> {
    let mut all = Vec::new();
    c.walk(&mut all);
    let mut out: Vec<&str> = all
        .into_iter()
        .filter(|c| c.op == op)
        .filter_map(Clause::ref_name)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

impl Filter {
    fn axes(&self) -> Vec<String> {
        refs(&self.clause, "axis")
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn words(&self) -> String {
        let fields = refs(&self.clause, "field");
        if fields.iter().any(|f| identifying(f)) {
            return format!(
                "a filter on {} (its values are not shown)",
                fields.join(", ")
            );
        }
        nils_ask::describe::clause_text(&self.clause)
    }
}

/// A selection named `name`, `name@<v>` or `selection:name@<v>`.
fn selection_spec(spec: &str) -> (&str, Option<u64>) {
    let bare = spec.strip_prefix("selection:").unwrap_or(spec);
    match bare.rsplit_once('@') {
        Some((n, v)) => (n, v.parse::<u64>().ok()),
        None => (bare, None),
    }
}

/// The ask of a row: the selection's own, with only the filters `keep`
/// names, its answer's keys at stack grain.
fn row_document(
    ask: &nils_ask::ast::Ask,
    keep: &BTreeSet<(String, usize)>,
) -> Result<Value, String> {
    let mut doc = serde_json::to_value(ask).map_err(|e| e.to_string())?;
    let out = ask.out.set.clone();
    let grain = ask
        .sets
        .get(&out)
        .map(|s| s.grain)
        .ok_or_else(|| format!("the selection answers a set it does not hold: {out}"))?;
    if let Some(sets) = doc["sets"].as_object_mut() {
        for (name, set) in sets.iter_mut() {
            if let Some(w) = set.get_mut("where").and_then(Value::as_array_mut) {
                let kept: Vec<Value> = w
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| keep.contains(&(name.clone(), *i)))
                    .map(|(_, c)| c.clone())
                    .collect();
                *w = kept;
            }
            if set["where"].as_array().is_some_and(Vec::is_empty) {
                set.as_object_mut().map(|o| o.remove("where"));
            }
        }
        let at = if grain == Grain::Stack {
            out
        } else if matches!(grain, Grain::Subject | Grain::Session) {
            sets.insert(STACKS.into(), json!({"grain": "stack", "of": out}));
            STACKS.to_string()
        } else {
            return Err(format!(
                "the selection is a {} set, and a funnel counts the stacks of subjects, sessions or stacks",
                grain.name()
            ));
        };
        doc["out"] = json!({"set": at, "level": "record"});
    }
    if let Some(o) = doc.as_object_mut() {
        o.remove("keep");
        o.remove("name");
    }
    Ok(doc)
}

/// The stacks a frozen ask holds.
fn frozen(
    registry: &mut Registry,
    pack: &nils_pack::Pack,
    doc: &Value,
    spec: &str,
) -> Result<BTreeSet<i64>, Exit> {
    let f = crate::ask_cli::freeze_doc(registry, pack, doc, crate::ask_cli::Freeze::Whole, spec)?;
    Ok(crate::pipelines::handle_stacks(registry.store(), f.handle)?
        .into_iter()
        .collect())
}

/// A unit as the need rows count it.
struct Judged {
    stacks: Vec<i64>,
    lacks: Vec<String>,
    /// Where it has its inputs: skipped, the index of the need that
    /// skipped it (`needs.len()` for one the list does not name), and
    /// whether that need could not be told.
    skipped: Option<Option<(usize, bool)>>,
}

/// What the pipeline or family makes of the selection's stacks.
struct Planned {
    pipeline: Value,
    unit: &'static str,
    units: Vec<Judged>,
    left: Vec<(i64, String)>,
    needs: Vec<(String, String)>,
}

fn need_index(needs: &[(String, String)], need: Option<&str>) -> usize {
    need.and_then(|n| needs.iter().position(|(k, _)| k == n))
        .unwrap_or(needs.len())
}

fn plan(
    registry: &mut Registry,
    pipeline: &str,
    stacks: &[i64],
    pack: &nils_pack::Pack,
    person: &PersonAsk,
) -> Result<Planned, Exit> {
    use nils_pipeline::pack::rules::Status;
    let is_pipeline = nils_registry::pipeline::resolve(registry.store(), pipeline)
        .map_err(|e| fail(e.to_string()))?
        .is_some();
    if !is_pipeline
        && let Some(stored) =
            crate::pipeline_family::get(registry.store(), pipeline).map_err(fail)?
    {
        let f = stored.family().map_err(fail)?;
        let p = crate::pipeline_family::person_of(f, person, &crate::actor()).map_err(usage)?;
        let routed = crate::pipeline_family::route_stacks(registry, f, stacks, &p).map_err(fail)?;
        let needs: Vec<(String, String)> = f
            .needs
            .iter()
            .flat_map(|(role, rn)| {
                rn.hard
                    .iter()
                    .map(move |n| (format!("{role}: {}", n.text), n.words.clone()))
            })
            .collect();
        let reached: BTreeSet<i64> = routed.iter().flat_map(|u| u.stacks.clone()).collect();
        let left = stacks
            .iter()
            .filter(|s| !reached.contains(s))
            .map(|s| (*s, "the registry holds no session for it".to_string()))
            .collect();
        let units = routed
            .into_iter()
            .map(|u| Judged {
                stacks: u.stacks,
                lacks: Vec::new(),
                skipped: Some((u.routing.status == Status::Skipped).then(|| {
                    (
                        need_index(&needs, u.routing.need.as_deref()),
                        u.routing.cannot_tell.is_some(),
                    )
                })),
            })
            .collect();
        return Ok(Planned {
            pipeline: json!({"family": stored.name, "digest": stored.digest}),
            unit: "unit",
            units,
            left,
            needs,
        });
    }
    let fu = crate::preflight::funnel_units(registry, pipeline, stacks, Some(pack), person)
        .map_err(|e| {
            if e.starts_with("no pipeline") {
                usage(format!("{e}, and no family of that name"))
            } else {
                fail(e)
            }
        })?;
    let needs = fu.needs.clone().unwrap_or_default();
    let units = fu
        .units
        .into_iter()
        .map(|u| Judged {
            stacks: u.stacks,
            skipped: if u.lacks.is_empty() {
                Some(u.judged.as_ref().and_then(|j| {
                    (j.status == Status::Skipped).then(|| {
                        (
                            need_index(&needs, j.need.as_deref()),
                            j.cannot_tell.is_some(),
                        )
                    })
                }))
            } else {
                None
            },
            lacks: u.lacks,
        })
        .collect();
    Ok(Planned {
        pipeline: json!({"pipeline": fu.pipeline, "pack_digest": fu.pack_digest, "has_pack": fu.needs.is_some()}),
        unit: fu.unit,
        units,
        left: fu.left,
        needs,
    })
}

/// The funnel of a selection and a pipeline or family.
pub(crate) fn compute(
    registry: &mut Registry,
    pack: &nils_pack::Pack,
    asked: &Asked,
) -> Result<Value, Exit> {
    let (name, version) = selection_spec(&asked.selection);
    let saved = nils_ask::selection::get(registry.store(), name, version)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no selection {}", asked.selection)))?;
    let spec = format!("selection:{}@{}", saved.name, saved.version);
    // the bounds first: a certificate asked for that is not there is refused
    // before anything is frozen
    let bounds = Bounds::read(registry.store(), asked.certificate).map_err(|e| match e {
        nils_registry::labels::Error::NotFound(m) => usage(m),
        other => fail(other.to_string()),
    })?;
    let filters: Vec<Filter> = saved
        .ask
        .sets
        .iter()
        .flat_map(|(set, s)| {
            s.where_.iter().enumerate().map(|(index, clause)| Filter {
                set: set.clone(),
                index,
                clause: clause.clone(),
            })
        })
        .collect();
    // row 0 and every filter but the last from the ask itself; the last is
    // the selection frozen as a run freezes it
    let mut sets: Vec<BTreeSet<i64>> = Vec::new();
    let mut keep: BTreeSet<(String, usize)> = BTreeSet::new();
    for f in &filters {
        let doc = row_document(&saved.ask, &keep).map_err(usage)?;
        sets.push(frozen(registry, pack, &doc, &spec)?);
        keep.insert((f.set.clone(), f.index));
    }
    let grain = saved
        .ask
        .sets
        .get(&saved.ask.out.set)
        .map(|s| s.grain)
        .unwrap_or(Grain::Subject);
    let whole = crate::campaigns::freeze_document(&saved.name, saved.version, grain, Grain::Stack)
        .map_err(usage)?;
    sets.push(frozen(registry, pack, &whole, &spec)?);
    let selected: Vec<i64> = sets
        .last()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let planned = plan(registry, &asked.pipeline, &selected, pack, &asked.person)?;

    let all: Vec<i64> = sets
        .iter()
        .flatten()
        .copied()
        .chain(planned.units.iter().flat_map(|u| u.stacks.iter().copied()))
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let of = count::stacks_of(registry.store(), &all).map_err(|e| fail(e.to_string()))?;
    let mut rows: Vec<Value> = Vec::new();
    let mut reads: Vec<Read> = Vec::new();
    for (k, stacks) in sets.iter().enumerate() {
        let mut row = count::row_counts(&of, stacks);
        if k == 0 {
            row["kind"] = json!("selection");
            row["what"] = json!("the stacks the selection starts from, before its filters");
        } else {
            let f = &filters[k - 1];
            let axes = f.axes();
            row["kind"] = json!("filter");
            row["set"] = json!(f.set);
            row["what"] = json!(f.words());
            row["axes"] = json!(axes);
            if !axes.is_empty() {
                reads.push(Read {
                    row: k,
                    axes,
                    entered: sets[k - 1].len(),
                });
            }
        }
        rows.push(row);
    }
    // the units: those that reach a unit, have their inputs, pass each need
    let word = planned.unit;
    let unit_row = |units: &[&Judged], kind: &str, what: String| -> Value {
        let stacks: BTreeSet<i64> = units
            .iter()
            .flat_map(|u| u.stacks.iter().copied())
            .collect();
        let mut row = count::row_counts(&of, &stacks);
        row["kind"] = json!(kind);
        row["what"] = json!(what);
        row["units"] = json!(units.len());
        row
    };
    let reach: Vec<&Judged> = planned.units.iter().collect();
    let mut row = unit_row(&reach, "units", format!("in a {word} of the run"));
    let mut left_why: BTreeMap<String, usize> = BTreeMap::new();
    for (_, why) in &planned.left {
        *left_why.entry(why.clone()).or_default() += 1;
    }
    row["left_out"] = json!(
        left_why
            .into_iter()
            .map(|(why, stacks)| json!({"why": why, "stacks": stacks}))
            .collect::<Vec<_>>()
    );
    rows.push(row);
    let inputs: Vec<&Judged> = reach
        .iter()
        .copied()
        .filter(|u| u.lacks.is_empty())
        .collect();
    let mut lacking: BTreeMap<String, usize> = BTreeMap::new();
    for u in reach.iter().filter(|u| !u.lacks.is_empty()) {
        for b in u
            .lacks
            .iter()
            .map(|w| crate::preflight::brief(w))
            .collect::<BTreeSet<_>>()
        {
            *lacking.entry(b).or_default() += 1;
        }
    }
    let mut row = unit_row(
        &inputs,
        "inputs",
        "with every input the pipeline takes".to_string(),
    );
    row["lacking"] = json!(
        lacking
            .into_iter()
            .map(|(why, units)| json!({"why": why, "units": units}))
            .collect::<Vec<_>>()
    );
    rows.push(row);
    let n_needs = planned.needs.len();
    let other = inputs
        .iter()
        .any(|u| matches!(u.skipped, Some(Some((i, _))) if i >= n_needs));
    let mut steps: Vec<(String, String, Option<String>)> = planned
        .needs
        .iter()
        .map(|(need, words)| (need.clone(), words.clone(), Some(need.clone())))
        .collect();
    if other {
        steps.push((
            "skipped by the pack for another reason".into(),
            String::new(),
            None,
        ));
    }
    for (i, (need, words, _)) in steps.iter().enumerate() {
        let passing: Vec<&Judged> = inputs
            .iter()
            .copied()
            .filter(|u| match u.skipped {
                Some(Some((at, _))) => at > i,
                _ => true,
            })
            .collect();
        let told_not = inputs
            .iter()
            .filter(|u| matches!(u.skipped, Some(Some((at, true))) if at == i))
            .count();
        let mut row = unit_row(&passing, "need", words.clone());
        row["need"] = json!(need);
        row["cannot_tell"] = json!(told_not);
        rows.push(row);
    }
    // what each row removed from the one before
    let mut monotone = true;
    for k in 1..rows.len() {
        let prev = rows[k - 1].clone();
        let row = &mut rows[k];
        let mut removed = json!({});
        for c in ["subjects", "sessions", "stacks"] {
            let d = prev[c].as_i64().unwrap_or(0) - row[c].as_i64().unwrap_or(0);
            if d < 0 {
                monotone = false;
            }
            removed[c] = json!(d);
        }
        row["removed"] = removed;
    }
    for (k, row) in rows.iter_mut().enumerate() {
        row["row"] = json!(k);
    }
    let budget = count::budget(&bounds, &reads);
    let mut doc = json!({
        "selection": spec,
        "unit": word,
        "person": asked.person.doc(),
        "rows": rows,
        "monotone": monotone,
        "budget": budget,
    });
    if let Some(o) = planned.pipeline.as_object() {
        for (k, v) in o {
            doc[k] = v.clone();
        }
    }
    Ok(doc)
}

/// The pack the rows' asks and the run's release name stacks by.
fn mri_pack(home: &Home, pack_dir: Option<PathBuf>, name: &str) -> Result<nils_pack::Pack, Exit> {
    let dir = crate::pack_dir(home, pack_dir)
        .map_err(|_| usage("no packs here: pass --pack-dir DIR to count a funnel"))?;
    crate::ask_cli::load_pack(&dir, name)
}

/// `GET /api/funnel?selection=<id>&pipeline=<id>`, with `certificate`,
/// `goal`, `switches` (`name=value,...`) and `let_run` where asked.
pub(crate) fn route(
    home: &Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get || segs != ["api", "funnel"] {
        return None;
    }
    Some((|| {
        let need = |k: &str| {
            query
                .get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    Reply::error(
                        400,
                        format!("the funnel is of a selection and a pipeline: ?selection=<name@version>&pipeline=<name> ({k} is missing)"),
                    )
                })
        };
        let selection = need("selection")?;
        let pipeline = need("pipeline")?;
        let certificate = match query.get("certificate") {
            None => None,
            Some(c) => Some(
                c.trim()
                    .parse::<i64>()
                    .map_err(|_| Reply::error(400, "certificate is a certificate's id"))?,
            ),
        };
        let switches: Vec<String> = query
            .get("switches")
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let person = PersonAsk::of_args(
            query.get("goal").map(String::as_str),
            &switches,
            query.get("let_run").is_some_and(|v| v == "true"),
        )
        .map_err(|e| Reply::error(400, e))?;
        let pack = mri_pack(
            home,
            pack_dir.map(Path::to_path_buf),
            query.get("pack").map_or("mri", String::as_str),
        )
        .map_err(|e| Reply::error(400, e.message))?;
        let asked = Asked {
            selection,
            pipeline,
            certificate,
            person,
        };
        compute(registry, &pack, &asked)
            .map(Reply::ok)
            .map_err(|e| {
                if e.message.starts_with("no ") {
                    Reply::error(404, e.message)
                } else if e.code == crate::USAGE {
                    Reply::error(400, e.message)
                } else {
                    Reply::error(500, e.message)
                }
            })
    })())
}

/// `nils funnel <selection> <pipeline>`.
#[derive(Debug, clap::Args)]
pub(crate) struct FunnelArgs {
    /// The selection: name, name@version or selection:name@version
    pub(crate) selection: String,
    /// The pipeline (name, name@version or id) or a family's name
    pub(crate) pipeline: String,
    /// The certificate whose bounds the last line multiplies by; the newest recorded by default
    #[arg(long)]
    pub(crate) certificate: Option<i64>,
    /// The goal the pack's needs are judged under (fast, standard, research)
    #[arg(long)]
    pub(crate) goal: Option<String>,
    /// A switch of the pack, name=value; repeatable
    #[arg(long = "switch", value_name = "NAME=VALUE")]
    pub(crate) switches: Vec<String>,
    /// Count a unit whose hard need the registry cannot tell as passing it
    #[arg(long)]
    pub(crate) let_run: bool,
    /// Where the packs are; $NILS_PACK_DIR, else `packs/` in the registry home
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<PathBuf>,
    #[arg(long, default_value = "mri")]
    pub(crate) pack: String,
    #[arg(long)]
    pub(crate) json: bool,
}

/// The funnel in lines: a row a line, then the error-budget line.
pub(crate) fn words(doc: &Value) -> String {
    let mut out = format!(
        "{} through {}:\n",
        doc["selection"].as_str().unwrap_or_default(),
        doc["pipeline"]
            .as_str()
            .or(doc["family"].as_str())
            .unwrap_or_default()
    );
    for r in doc["rows"].as_array().into_iter().flatten() {
        let units = r["units"].as_u64().map_or(String::new(), |n| {
            format!(", {n} {}s", doc["unit"].as_str().unwrap_or("unit"))
        });
        out.push_str(&format!(
            "  {:>2} {:<9} {:>7} subjects {:>7} sessions {:>8} stacks{units}  {}\n",
            r["row"],
            r["kind"].as_str().unwrap_or_default(),
            r["subjects"],
            r["sessions"],
            r["stacks"],
            r["what"].as_str().unwrap_or_default(),
        ));
    }
    out.push_str(doc["budget"]["words"].as_str().unwrap_or_default());
    out
}

pub(crate) fn command(home: &Home, args: FunnelArgs) -> Result<(), Exit> {
    let pack = mri_pack(home, args.pack_dir.clone(), &args.pack)?;
    let person =
        PersonAsk::of_args(args.goal.as_deref(), &args.switches, args.let_run).map_err(usage)?;
    let mut registry = crate::open(home)?;
    let doc = compute(
        &mut registry,
        &pack,
        &Asked {
            selection: args.selection,
            pipeline: args.pipeline,
            certificate: args.certificate,
            person,
        },
    )?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| fail(e.to_string()))?
        );
    } else {
        println!("{}", words(&doc));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifying_fields_are_named_without_their_values() {
        for f in ["subject.code", "series_instance_uid", "source_path", "name"] {
            assert!(identifying(f), "{f}");
        }
        for f in [
            "dataset",
            "protocol_name",
            "manufacturer",
            "series_description",
        ] {
            assert!(!identifying(f), "{f}");
        }
    }

    #[test]
    fn a_row_document_keeps_only_the_filters_named() {
        let ask: nils_ask::ast::Ask = serde_json::from_value(json!({
            "ast_version": 1,
            "sets": {"t1": {"grain": "session", "where": [
                ["=", {}, ["field", {}, "dataset"], "study-big"],
                ["=", {}, ["axis", {}, "base"], "T1w"]
            ]}},
            "out": {"set": "t1", "level": "count"}
        }))
        .unwrap();
        let none = row_document(&ask, &BTreeSet::new()).unwrap();
        assert!(none["sets"]["t1"].get("where").is_none(), "{none}");
        assert_eq!(none["out"]["set"], STACKS, "{none}");
        assert_eq!(none["sets"][STACKS]["of"], "t1", "{none}");
        let one = row_document(&ask, &[("t1".to_string(), 1)].into_iter().collect()).unwrap();
        assert_eq!(one["sets"]["t1"]["where"][0][2][2], "base", "{one}");
        assert_eq!(one["sets"]["t1"]["where"].as_array().unwrap().len(), 1);
    }
}
