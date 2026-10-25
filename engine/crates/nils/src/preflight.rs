// SPDX-License-Identifier: AGPL-3.0-only

//! The pre-flight of a run (record 49 A3): before anything runs, what a run
//! of a pipeline over a selection would do, from the registry alone. The
//! units it would have, the units that lack an input and why, the stacks of
//! the selection that would reach no unit, the time it would take (from the
//! pipeline's own past runs here, else from its descriptor), whether it
//! wants a GPU and whether this engine has one, and what a unit needs of
//! the cores and memory against the lane's budget.
//!
//! It is `POST /api/pipelines/{name}/preflight` and `nils run --preflight`.
//! It starts nothing and writes nothing but the handle a selection is frozen
//! into, which a run would freeze the same way (and which is reused while
//! the registry has not moved), and the session cache the release reads,
//! which the release would build the same way.
//!
//! Record 55 (rulings of 2026-10-12, item 6): a run over a dataset's or a
//! cohort's main scans of a role (`main:<role>@dataset:<name>`) is
//! pre-flown over the list a run would freeze now, and says which picks it
//! read and the scope's visits that have no main scan of the role, counted
//! by why and named (`crate::main_input`). Its bids units are planned under
//! the picks the list was resolved from, as the run releases its input.
//!
//! The units are counted the way the runner makes them: in the stacks
//! layout one a stack; in the bids layout one a session (or a participant)
//! of the release's own tree, since the runner's input is a release with
//! the picks applied (record 43 R4) and its units are that tree's folders.
//! The release's plan says where each picked stack goes, so a stack it
//! keeps in `sourcedata/` or holds is left out as the run leaves it out. A
//! session is missing an input when no stack picked for a role the
//! descriptor names under `x-nils.input.roles` is released under that
//! role's own BIDS suffix, or, for a suffix the release spells as a
//! modifier, with it in its `acq-` (a `flair` pick as
//! `acq-Ax+2D+FLAIR+IRTSE_T2w`, record 55 C4; a `t1w` pick the release
//! names `T2w` is no T1w), and the run skips it; a stack when the registry
//! holds no file of it. A unit of either layout is missing an input when
//! it has no derivative a typed input needs that the input's check takes
//! (a mask a check rejected is none, unless the input takes any), and the
//! run skips that one too.
//!
//! 2026-10-12 (Nima: "if the selection does not fulfill or maybe some part
//! of it the pipeline should say that these sessions cannot be [run] unless
//! they have whatever"): the units that cannot run are said by what they
//! lack in a few words ([`brief`], "no FLAIR picked"), one line a reason
//! with the units under it (`cannot`), and a run none of whose units could
//! run is not ready, since it would make nothing. A run taken up again
//! (`nils run --resume`, `{resume: <run>}` at the door) is checked as a new
//! run is, for the units it still has to run: of the bids input it released
//! before where it did, else as a new run over its handle, the units it is
//! over with kept. The jobs door and `nils run` refuse a run that could run
//! none of its units and has made nothing ([`units_refusal`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nils_pipeline::descriptor::{self, Descriptor, Gpu, Layout, Level};
use nils_registry::home::Home;
use nils_registry::pipeline as rows;
use nils_registry::store::Store;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::Reply;
use crate::{Exit, usage};

/// The door, as the capabilities list it.
pub(crate) const DOOR: &str = "POST /api/pipelines/{name}/preflight";

/// How many of a pipeline's past runs the estimate reads.
const PAST_RUNS: usize = 20;

/// What the pre-flight is asked.
pub(crate) struct Asked<'a> {
    /// The registry's folder, where the engine keeps the tools it installs.
    pub home: &'a Path,
    /// The converter as the run would name it (2026-10-12).
    pub dcm2niix: &'a Path,
    pub pipeline: &'a str,
    /// The pack the run's release names its stacks by; without it the bids
    /// units are counted from the picks alone.
    pub pack: Option<&'a nils_pack::Pack>,
    pub handle: i64,
    pub selection: Option<&'a str>,
    pub params: &'a [(String, String)],
    /// Record 55: where the handle is a dataset's or a cohort's main scans,
    /// what was resolved, the visits with none named; else what the handle
    /// records of them.
    pub main: Option<Value>,
    /// 2026-10-12: the run taken up again, whose pipeline, parameters and
    /// handle these are; none for a new run.
    pub resume: Option<i64>,
    /// 2026-10-12: a version a later one replaces is asked for at the
    /// keyboard with `--replaced`; a door never asks so.
    pub replaced: bool,
    /// Record 57 P2 (Wave 7b A1): the goal, the switches and whether a
    /// unit whose hard need cannot be told runs.
    pub person: crate::pipeline_pack::PersonAsk,
}

/// Record 57 P2: the roles each stack of a run is picked for, under the
/// live picks, so the runner reads a unit's facts by role as the pre-flight
/// does.
pub(crate) fn roles_of_stacks(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, BTreeSet<String>>, String> {
    let of = nils_registry::pick_truth::Of::own(store).map_err(|e| e.to_string())?;
    let picks = pick_roles(store, stacks, &nils_registry::pick_truth::Picks::Live(of))?;
    Ok(picks.into_iter().map(|(s, p)| (s, p.roles)).collect())
}

/// The lane's budget (record 49 A1, R6), as the lane itself reads it: the
/// setting of `nils pipeline lane`, else the ruling's 48 cores and 512 GB,
/// each no more than this machine or the container the engine runs in
/// offers; and the card a GPU unit would lease.
pub(crate) fn budget(registry: &mut Registry) -> (crate::pipelines::Lane, Value) {
    let lane = crate::pipelines::lane_of(registry);
    let mut doc = lane.doc();
    doc["memory_gb"] = json!(lane.ledger.memory_mib as f64 / 1024.0);
    (lane, doc)
}

/// One unit the run would have.
#[derive(Debug, Clone, Default)]
struct Unit {
    /// The run's own name for it, `sub-<s>_ses-<t>` or `sub-<s>`, where the
    /// release's plan gave it one.
    id: Option<String>,
    subject_id: Option<i64>,
    session_day: Option<String>,
    stack_id: Option<i64>,
    stacks: Vec<i64>,
    roles: BTreeSet<String>,
    /// Record 57 P2: the unit's stacks by the role each was taken for (a
    /// stacks unit's one stack under `stack`), which the pack's facts are
    /// read by.
    by_role: BTreeMap<String, Vec<i64>>,
    missing: Vec<String>,
}

impl Unit {
    fn name(&self) -> String {
        if let Some(id) = &self.id {
            return id.clone();
        }
        match (self.stack_id, self.subject_id, &self.session_day) {
            (Some(s), _, _) => format!("stack-{s}"),
            (None, Some(sub), Some(day)) => format!("subject {sub}, session of {day}"),
            (None, Some(sub), None) => format!("subject {sub}"),
            _ => "a unit".into(),
        }
    }
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many units one reason of `cannot` names; the rest are counted.
const CANNOT_NAMED: usize = 200;

/// A role as a person reads it: its BIDS suffix where the standard has one
/// (`flair` as FLAIR, `t1w` as T1w), else as the pack names it.
fn role_word(role: &str) -> String {
    nils_release::run::role_suffix(role).map_or_else(|| role.to_string(), str::to_string)
}

/// Why a unit cannot run, in a few words (2026-10-12): what it lacks, as a
/// run's line and the pre-flight's `cannot` say it ("no FLAIR picked" for
/// "no flair is picked for it among the selection's stacks"). Read from the
/// engine's own words, so a run from before reads the same; a reason it has
/// no short words for is said whole.
pub(crate) fn brief(why: &str) -> String {
    let why = why.trim();
    if let Some(role) = why
        .strip_prefix("no ")
        .and_then(|r| r.strip_suffix(" is picked for it among the selection's stacks"))
    {
        return format!("no {} picked", role_word(role));
    }
    if let Some((role, _)) = why
        .strip_prefix("no ")
        .and_then(|r| r.split_once(" stands for it: a person's pick "))
    {
        return format!("a person said no {} stands", role_word(role));
    }
    if let Some((role, _)) = why
        .strip_prefix("its ")
        .and_then(|r| r.split_once(" pick is released outside the session's BIDS folders"))
    {
        return format!("its {} is outside the input", role_word(role));
    }
    if let Some((role, rest)) = why
        .strip_prefix("its ")
        .and_then(|r| r.split_once(" pick is released as "))
    {
        let as_ = rest.split(", not ").next().unwrap_or(rest);
        return format!("its {} is released as {as_}", role_word(role));
    }
    // a derivative an input needs (crate::pipelines::derivative_lack)
    if let Some((kind, input)) = why
        .strip_prefix("no ")
        .and_then(|r| r.split_once(" derivative of it for the input "))
    {
        return if input.contains(" is accepted by a check") {
            format!("no accepted {kind}")
        } else {
            format!("no {kind} yet")
        };
    }
    if let Some((kind, _)) = why
        .strip_prefix("its ")
        .and_then(|r| r.split_once(" derivative for the input "))
        .filter(|(_, rest)| rest.contains("was rejected by a check"))
    {
        return format!("its {kind} was rejected");
    }
    if let Some(rest) = why
        .strip_prefix("its ")
        .filter(|r| r.contains(" were all rejected by a check"))
    {
        let kind = rest
            .split_once(" derivatives for the input ")
            .map(|(n_kind, _)| n_kind.split_once(' ').map_or(n_kind, |(_, k)| k))
            .unwrap_or("derivative");
        return format!("every {kind} rejected");
    }
    match why {
        "the registry does not hold this stack" => "not in the registry".into(),
        "the registry holds no file of it to read" => "no file to read".into(),
        _ => why.to_string(),
    }
}

/// A unit's reasons in a few words, each once, in the order it has them.
fn briefs(missing: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in missing {
        let b = brief(w);
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// What one unit of a pipeline is called (2026-10-12): a session or a
/// subject of the bids layout's release, a stack of the stacks layout.
pub(crate) fn unit_word(layout: Layout, level: Level) -> &'static str {
    match (layout, level) {
        (Layout::Stacks, _) => "stack",
        (Layout::Bids, Level::Session) => "session",
        (Layout::Bids, _) => "subject",
    }
}

/// A count of units in words: "1 session", "4 sessions".
fn units_words(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The units that cannot run, one entry a reason in a few words, the most
/// first: `{why, units, named}`, each unit counted once under each reason
/// it has, and at most [`CANNOT_NAMED`] of them named, as the run names
/// them.
fn cannot_of(missing: &[&Unit]) -> Vec<Value> {
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for u in missing {
        for why in briefs(&u.missing) {
            by.entry(why).or_default().push(u.name());
        }
    }
    let mut out: Vec<(String, Vec<String>)> = by.into_iter().collect();
    out.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    out.into_iter()
        .map(|(why, units)| {
            json!({
                "why": why,
                "units": units.len(),
                "named": units.into_iter().take(CANNOT_NAMED).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// The reasons of `cannot` on one line: "no FLAIR picked", or with their
/// counts where there are several, "no FLAIR picked: 3; no T1w picked: 1".
pub(crate) fn reasons_line(cannot: &[Value]) -> String {
    let why = |c: &Value| c["why"].as_str().unwrap_or_default().to_string();
    match cannot {
        [one] => why(one),
        many => many
            .iter()
            .map(|c| format!("{}: {}", why(c), c["units"]))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// Why a run none of whose units could run is not ready, and is refused:
/// "no session can run: 4 of 4 lack an input (no FLAIR picked)".
fn none_can_run(word: &str, n: usize, cannot: &[Value]) -> String {
    format!(
        "no {word} can run: {n} of {n} {} an input ({})",
        if n == 1 { "lacks" } else { "lack" },
        reasons_line(cannot)
    )
}

/// The stacks layout: one unit a stack, missing where the registry holds no
/// file of it or no derivative a typed input needs.
fn stack_units(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
    place_ids: &[i64],
) -> Result<Vec<Unit>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut with_files: BTreeSet<i64> = BTreeSet::new();
    let mut subject: BTreeMap<i64, i64> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let ids = list(chunk);
        for sql in [
            format!(
                "SELECT DISTINCT i.stack_id FROM {} i JOIN {} f ON f.id = i.source_file_id \
                 WHERE i.stack_id IN ({ids})",
                store.qualified("instance"),
                store.qualified("source_file")
            ),
            format!(
                "SELECT DISTINCT fr.stack_id FROM {} fr WHERE fr.stack_id IN ({ids})",
                store.qualified("instance_frame")
            ),
        ] {
            for r in store.query(&sql, &[]).map_err(err)? {
                with_files.insert(r.int(0).map_err(err)?);
            }
        }
        let sql = format!(
            "SELECT st.id, se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({ids})",
            store.qualified("stack"),
            store.qualified("series")
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            subject.insert(r.int(0).map_err(err)?, r.int(1).map_err(err)?);
        }
    }
    // the derivatives each typed input takes, stack by stack, as the run's
    // inputs take them: of the stack, its session or its subject, and left
    // out where the input's check leaves them out (the T33 pipe test)
    let one: Vec<[i64; 1]> = stacks.iter().map(|&s| [s]).collect();
    let units: Vec<&[i64]> = one.iter().map(|s| s.as_slice()).collect();
    let lacking = crate::pipelines::derivatives_missing(store, d, &units, place_ids)?;
    Ok(stacks
        .iter()
        .zip(lacking)
        .map(|(&s, lacks)| {
            let mut u = Unit {
                stack_id: Some(s),
                subject_id: subject.get(&s).copied(),
                stacks: vec![s],
                by_role: [("stack".to_string(), vec![s])].into_iter().collect(),
                ..Unit::default()
            };
            if !subject.contains_key(&s) {
                u.missing
                    .push("the registry does not hold this stack".into());
            } else if !with_files.contains(&s) {
                u.missing
                    .push("the registry holds no file of it to read".into());
            }
            u.missing.extend(lacks);
            u
        })
        .collect())
}

/// What the live picks say of one stack: the roles it was picked for, and
/// the subject and the session's day the pick names.
pub(crate) struct Picked {
    pub(crate) roles: BTreeSet<String>,
    subject: i64,
    day: String,
}

/// The stacks the selection leaves out of a run's input, each with why.
type LeftOut = Vec<(i64, String)>;

/// Record 55 (rulings of 2026-10-12, item 6): a frozen list of a dataset's
/// or a cohort's main scans, each stack the main scan of the list's role on
/// its visit, as it was resolved; what a run over it takes whatever the
/// picks say now (a scope's newer rules replace the picks of the version
/// before). Each with its subject and its visit's day under the default
/// scheme, as a pick names them. Of a list of several roles (2026-10-12),
/// each stack the main scan of the role it was taken for, as the list's
/// `entry` records it.
pub(crate) fn frozen_picks(
    registry: &mut Registry,
    stacks: &[i64],
    entry: &Value,
) -> Result<BTreeMap<i64, Picked>, String> {
    let roles_of = crate::main_input::stack_roles(entry, stacks);
    let err = |e: nils_registry::Error| e.to_string();
    let scheme = nils_registry::session::Scheme::default();
    let anchors = nils_session::Anchors::resolve(registry, &scheme, BTreeMap::new())
        .map_err(|e| e.to_string())?;
    nils_session::ensure(registry, &scheme, &anchors, None, false).map_err(|e| e.to_string())?;
    let labels =
        nils_session::labels_by_study(registry.store(), &scheme).map_err(|e| e.to_string())?;
    let store = registry.store();
    let mut out: BTreeMap<i64, Picked> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT st.id, se.subject_id, se.study_id FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let label = labels.get(&r.int(2).map_err(err)?);
            let stack = r.int(0).map_err(err)?;
            out.insert(
                stack,
                Picked {
                    roles: roles_of.get(&stack).cloned().unwrap_or_default(),
                    subject: label.map_or(r.int(1).map_err(err)?, |l| l.subject_id),
                    day: label.map(|l| l.first.to_string()).unwrap_or_default(),
                },
            );
        }
    }
    Ok(out)
}

/// The picks among `stacks`, by stack: the live ones, or those a release
/// froze when it was made.
pub(crate) fn pick_roles(
    store: &mut Store,
    stacks: &[i64],
    picks: &nils_registry::pick_truth::Picks,
) -> Result<BTreeMap<i64, Picked>, String> {
    // record 55, decision 6, and one place of truth (2026-10-11): the picks
    // a release read, as it froze them (the rulings of 2026-10-12, item 6,
    // carried to releases), or the main scans as every reader reads them
    let picks = picks.stacks(store);
    let err = |e: nils_registry::Error| e.to_string();
    let mut out: BTreeMap<i64, Picked> = BTreeMap::new();
    let d_ = store.dialect();
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT p.stack_id, p.subject_id, {}, p.role FROM {picks} p \
             WHERE p.stack_id IN ({}) ORDER BY p.stack_id",
            d_.text_of_qualified(
                Some("p"),
                nils_registry::schema::table("pick")
                    .column("session_day")
                    .expect("pick.session_day")
            ),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let stack = r.int(0).map_err(err)?;
            let subject = r.int(1).map_err(err)?;
            let day = r.text(2).map_err(err)?.to_string();
            let role = r.text(3).map_err(err)?.to_string();
            out.entry(stack)
                .or_insert_with(|| Picked {
                    roles: BTreeSet::new(),
                    subject,
                    day,
                })
                .roles
                .insert(role);
        }
    }
    Ok(out)
}

/// A unit's id as the runner names it from where the release put a stack:
/// `sub-<s>_ses-<t>` at the session level, `sub-<s>` at the subject level;
/// none for a stack outside a subject's folder.
fn unit_of_dir(dir: &str, level: Level) -> Option<String> {
    let mut parts = dir.split('/');
    let subject = parts.next()?.strip_prefix("sub-")?;
    let session = parts.next().and_then(|p| p.strip_prefix("ses-"));
    Some(match (level, session) {
        (Level::Session, Some(s)) => format!("sub-{subject}_ses-{s}"),
        _ => format!("sub-{subject}"),
    })
}

/// What a unit lacks of the roles a descriptor names (record 49 A3), from
/// what the release makes of the stacks picked for each role in it: the
/// BIDS stem of each in the raw tree, none for one the release puts
/// elsewhere. A role is there when a stack picked for it is released under
/// the role's own suffix (`t1w` as `T1w`), or, for a suffix the release
/// spells as a modifier, with it in its `acq-` (`flair` as
/// `acq-Ax+2D+FLAIR+IRTSE_T2w`, record 55 C4); a role the standard spells
/// no suffix for is there when a stack picked for it is in the raw tree.
/// The pre-flight and the runner both judge a unit by this, so a session
/// whose T1w pick the release writes as a T2w is missing its T1w before
/// the run, and is not run.
pub(crate) fn roles_missing(
    roles: &[String],
    held: &BTreeMap<String, Vec<Option<String>>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for role in roles {
        let got = held.get(role).map(Vec::as_slice).unwrap_or(&[]);
        if got.is_empty() {
            out.push(format!(
                "no {role} is picked for it among the selection's stacks"
            ));
            continue;
        }
        let raw: Vec<&str> = got.iter().filter_map(|s| s.as_deref()).collect();
        let want = nils_release::run::role_suffix(role);
        let ok = match want {
            Some(w) => raw.iter().any(|s| nils_release::run::stem_holds(s, w)),
            None => !raw.is_empty(),
        };
        if ok {
            continue;
        }
        let mut as_: Vec<&str> = raw
            .iter()
            .map(|s| nils_release::run::stem_suffix(s))
            .collect();
        as_.sort_unstable();
        as_.dedup();
        out.push(match (want, as_.is_empty()) {
            (_, true) => format!(
                "its {role} pick is released outside the session's BIDS folders, so its input holds none"
            ),
            (Some(w), false) => format!(
                "its {role} pick is released as {}, not {w}, so its input holds no {w}",
                as_.join(" and ")
            ),
            (None, false) => unreachable!("a role with no suffix is there when anything is raw"),
        });
    }
    out
}

/// The bids layout: the units the release of the picks makes, one a session
/// (or a participant) that holds a live pick of a stack of the selection,
/// missing where it has no pick of a role the descriptor names. Answers the
/// units and the stacks no pick takes, which the input leaves out, with why.
///
/// With the pack, the units are the release's own (record 49 A3, found on
/// the group's install): the plan of `nils release --layout bids --picked`
/// says where each picked stack goes, so a session the release merges, a
/// stack it routes to `sourcedata/` or holds, and a pick it names by
/// another suffix are counted as the run will meet them.
/// Why a picked stack the release would leave out: the registry holds no
/// file of it to read.
const NO_FILE: &str = "the registry holds no file of it to read, so the release leaves it out";

/// Of picked stacks the release's plan leaves out, those of which the
/// registry holds a file to read, and those whose disposition a release
/// takes only when asked (excluded, display_composite), with that
/// disposition: what tells why each is left out.
fn unplanned_why(
    store: &mut Store,
    stacks: &[i64],
) -> Result<(BTreeSet<i64>, BTreeMap<i64, String>), String> {
    let err = |e: nils_registry::Error| e.to_string();
    let (mut filed, mut excluded) = (BTreeSet::new(), BTreeMap::new());
    for chunk in stacks.chunks(500) {
        let sql = format!(
            "SELECT DISTINCT i.stack_id FROM {} i JOIN {} sf ON sf.instance_id = i.id \
             WHERE i.stack_id IN ({})",
            store.qualified("instance"),
            store.qualified("source_file"),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            filed.insert(r.int(0).map_err(err)?);
        }
        let sql = format!(
            "SELECT DISTINCT a.stack_id, a.value FROM {} a WHERE a.axis = 'disposition' \
             AND a.value IN ('excluded', 'display_composite') AND a.stack_id IN ({})",
            store.qualified("classification_axis"),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            excluded.insert(r.int(0).map_err(err)?, r.text(1).map_err(err)?.to_string());
        }
    }
    Ok((filed, excluded))
}

/// Why a bids input would hold no unit, from why each of its stacks is
/// left out: none picked, as it was; else the reasons, each with its
/// stacks counted, the most first ("4 of the selection's 4 stacks: the
/// registry holds no file of it to read").
fn no_units_why(stacks: usize, left: &LeftOut) -> String {
    let unpicked = left
        .iter()
        .all(|(_, why)| why.starts_with("no live pick takes it"));
    if left.is_empty() || unpicked {
        return "no stack of the selection is picked, so the release of the input would hold no subject's images; nils pick run picks them".into();
    }
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, why) in left {
        *by.entry(why.as_str()).or_default() += 1;
    }
    let mut by: Vec<(&str, usize)> = by.into_iter().collect();
    by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    format!(
        "the release of the input would hold no subject's images: {}",
        by.iter()
            .map(|(why, n)| format!("{n} of the selection's {stacks} stacks: {why}"))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

fn bids_units(
    registry: &mut Registry,
    d: &Descriptor,
    stacks: &[i64],
    pack: Option<&nils_pack::Pack>,
    place_ids: &[i64],
    main: Option<&Value>,
) -> Result<(Vec<Unit>, LeftOut), String> {
    // a run's input is a selection, released under the main scans as the
    // release of one reads them: each subject's own dataset's once read,
    // else the pack's (one place of truth, 2026-10-11); a frozen list of a
    // dataset's or a cohort's main scans as it was resolved (record 55,
    // rulings of 2026-10-12, item 6)
    let (of, picks) = match main {
        Some(m) => (
            nils_registry::pick_truth::Of::of_record(&m["picks"]),
            frozen_picks(registry, stacks, m)?,
        ),
        None => {
            let of =
                nils_registry::pick_truth::Of::own(registry.store()).map_err(|e| e.to_string())?;
            let picks = pick_roles(
                registry.store(),
                stacks,
                &nils_registry::pick_truth::Picks::Live(of.clone()),
            )?;
            (of, picks)
        }
    };
    let unpicked = "no live pick takes it, so the release of the input leaves it out".to_string();
    let mut left: LeftOut = stacks
        .iter()
        .filter(|s| !picks.contains_key(s))
        .map(|s| (*s, unpicked.clone()))
        .collect();
    let mut held: BTreeMap<String, BTreeMap<String, Vec<Option<String>>>> = BTreeMap::new();
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    match pack {
        Some(pack) => {
            let picked: Vec<i64> = picks.keys().copied().collect();
            let plan = nils_release::run::plan_picked(
                registry,
                pack,
                &nils_registry::session::Scheme::default(),
                &picked,
                main.map(|m| &m["picks"]),
            )
            .map_err(|e| format!("the plan of the input's release: {e}"))?;
            let planned: BTreeSet<i64> = plan.iter().map(|p| p.stack).collect();
            let unplanned: Vec<i64> = picked
                .iter()
                .copied()
                .filter(|s| !planned.contains(s))
                .collect();
            let (with_a_file, excluded) = unplanned_why(registry.store(), &unplanned)?;
            for s in unplanned {
                left.push((
                    s,
                    if !with_a_file.contains(&s) {
                        NO_FILE.into()
                    } else if let Some(value) = excluded.get(&s) {
                        format!("its disposition is {value}, which a release takes only when asked, so the release leaves it out")
                    } else {
                        "the release leaves it out: the registry holds no file of it to write, or its disposition is one a release takes only when asked (excluded, display_composite)".into()
                    },
                ));
            }
            for p in &plan {
                let Some(id) = p.dir.as_deref().and_then(|dir| unit_of_dir(dir, d.level)) else {
                    left.push((
                        p.stack,
                        p.why
                            .clone()
                            .unwrap_or_else(|| format!("the release puts it in {}", p.route)),
                    ));
                    continue;
                };
                // the plan is of the picked stacks alone; one it holds
                // besides is no part of the input, and is said so, never a
                // panic (2026-10-12)
                let Some(Picked { roles, day, .. }) = picks.get(&p.stack) else {
                    left.push((
                        p.stack,
                        "it is not among the stacks the run takes, so the input leaves it out"
                            .into(),
                    ));
                    continue;
                };
                let u = units.entry(id.clone()).or_insert(Unit {
                    id: Some(id.clone()),
                    subject_id: Some(p.subject_id),
                    session_day: (d.level == Level::Session).then(|| day.clone()),
                    ..Unit::default()
                });
                u.stacks.push(p.stack);
                let stem = (p.route == "raw").then(|| p.stem.clone()).flatten();
                for role in roles {
                    u.roles.insert(role.clone());
                    u.by_role.entry(role.clone()).or_default().push(p.stack);
                    held.entry(id.clone())
                        .or_default()
                        .entry(role.clone())
                        .or_default()
                        .push(stem.clone());
                }
            }
        }
        // no pack to plan with: the picks alone, as the pick names them
        None => {
            for (
                stack,
                Picked {
                    roles,
                    subject,
                    day,
                },
            ) in &picks
            {
                let day = (d.level == Level::Session).then(|| day.clone());
                let key = format!("{subject}/{}", day.clone().unwrap_or_default());
                let u = units.entry(key.clone()).or_insert(Unit {
                    subject_id: Some(*subject),
                    session_day: day,
                    ..Unit::default()
                });
                u.stacks.push(*stack);
                for role in roles {
                    u.roles.insert(role.clone());
                    u.by_role.entry(role.clone()).or_default().push(*stack);
                    held.entry(key.clone())
                        .or_default()
                        .entry(role.clone())
                        .or_default()
                        .push(Some(
                            nils_release::run::role_suffix(role)
                                .map_or_else(|| role.clone(), str::to_string),
                        ));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (key, mut u) in units {
        u.missing = roles_missing(&d.roles, held.get(&key).unwrap_or(&BTreeMap::new()));
        if let (Some(subject), Some(day)) = (u.subject_id, u.session_day.as_deref())
            && !u.missing.is_empty()
        {
            let said = picked_nothing(registry.store(), &of, subject, day)?;
            for (role, pick) in said {
                let not_picked = format!("no {role} is picked");
                for why in u.missing.iter_mut().filter(|w| w.starts_with(&not_picked)) {
                    *why = format!("no {role} stands for it: a person's pick {pick} says so");
                }
            }
        }
        out.push(u);
    }
    // the derivatives each unit's inputs need, as the run judges them: a
    // unit whose only one was rejected by its check is not ready either
    let units: Vec<&[i64]> = out.iter().map(|u| u.stacks.as_slice()).collect();
    let lacking = crate::pipelines::derivatives_missing(registry.store(), d, &units, place_ids)?;
    for (u, lacks) in out.iter_mut().zip(lacking) {
        u.missing.extend(lacks);
    }
    left.sort();
    Ok((out, left))
}

/// Record 51 R2: the roles a person said no stack stands for on one
/// occasion (a person's pick of no stack, which Keep writes on a border
/// where the run picked nothing), each with that pick's id.
fn picked_nothing(
    store: &mut Store,
    of: &nils_registry::pick_truth::Of,
    subject: i64,
    day: &str,
) -> Result<BTreeMap<String, i64>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let d = store.dialect();
    let sql = format!(
        "SELECT p.id, p.role FROM {} p WHERE p.author_kind = 'person' \
         AND p.subject_id = {} AND p.session_day = {} \
         AND NOT EXISTS (SELECT 1 FROM {} ps WHERE ps.pick_id = p.id) ORDER BY p.id",
        of.picks(store),
        d.param(1, nils_registry::schema::Type::Int),
        d.param(2, nils_registry::schema::Type::Date),
        store.qualified("pick_stack"),
    );
    let day = &day[..day.len().min(10)];
    let mut out = BTreeMap::new();
    for r in store
        .query(
            &sql,
            &[
                nils_registry::store::Param::Int(subject),
                nils_registry::store::Param::from(day),
            ],
        )
        .map_err(err)?
    {
        out.insert(r.text(1).map_err(err)?.to_string(), r.int(0).map_err(err)?);
    }
    Ok(out)
}

/// Seconds a unit took in the pipeline's past runs here (by name, any
/// version), the median of the newest that completed; and how many.
fn past_seconds(store: &mut Store, name: &str) -> Result<Option<(f64, usize)>, String> {
    let d = store.dialect();
    let t = nils_registry::schema::table("pipeline_run");
    let sql = format!(
        "SELECT {}, {}, {} FROM {} r JOIN {} p ON p.id = r.pipeline_id \
         WHERE p.name = {} AND r.status IN ('done', 'partial') AND r.finished_at IS NOT NULL \
         ORDER BY r.id DESC LIMIT {PAST_RUNS}",
        d.text_of_qualified(Some("r"), t.column("started_at").expect("started_at")),
        d.text_of_qualified(Some("r"), t.column("finished_at").expect("finished_at")),
        d.text_of_qualified(Some("r"), t.column("summary").expect("summary")),
        store.qualified("pipeline_run"),
        store.qualified("pipeline"),
        d.param(1, nils_registry::schema::Type::Text),
    );
    let mut per_unit: Vec<f64> = Vec::new();
    for r in store
        .query(&sql, &[Param::from(name)])
        .map_err(|e| e.to_string())?
    {
        let (Ok(a), Ok(b)) = (r.text(0), r.text(1)) else {
            continue;
        };
        let (Some(a), Some(b)) = (
            nils_registry::time::secs_of(a),
            nils_registry::time::secs_of(b),
        ) else {
            continue;
        };
        let summary: Value = r
            .opt_text(2)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let units = summary["units"]["total"].as_f64().unwrap_or(0.0);
        if units >= 1.0 && b >= a {
            per_unit.push((b - a) as f64 / units);
        }
    }
    if per_unit.is_empty() {
        return Ok(None);
    }
    per_unit.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Some((per_unit[per_unit.len() / 2], per_unit.len())))
}

/// A run taken up again (2026-10-12): the run, the units it is over with,
/// and how many of those made what they were for.
pub(crate) struct Taken {
    pub(crate) run: rows::Run,
    over: BTreeSet<String>,
    made: usize,
}

/// A run to take up again, as it stands.
pub(crate) fn taken_up(store: &mut Store, run_id: i64) -> Result<Taken, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let run = rows::run(store, run_id)
        .map_err(err)?
        .ok_or_else(|| format!("no pipeline run {run_id}"))?;
    let mut over = BTreeSet::new();
    let mut made = 0;
    for u in rows::units(store, run_id).map_err(err)? {
        if u.state == "over" {
            if u.outcome["status"] == "succeeded" {
                made += 1;
            }
            over.insert(u.unit);
        }
    }
    Ok(Taken { run, over, made })
}

/// The units a run would have, as the runner makes them, and the stacks it
/// leaves out; for a run taken up again (2026-10-12) the units it still has
/// to run, with how many are over already and how many of those made what
/// they were for.
struct Planned {
    units: Vec<Unit>,
    left: LeftOut,
    kept: usize,
    made: usize,
}

/// The units of a run over `stacks`: a new run's as its release would plan
/// them; a run taken up again goes on with the bids input it released
/// before, where it did, and its units over already are kept.
fn plan_units(
    registry: &mut Registry,
    d: &Descriptor,
    stacks: &[i64],
    pack: Option<&nils_pack::Pack>,
    place_ids: &[i64],
    main: Option<&Value>,
    taken: Option<&Taken>,
) -> Result<Planned, String> {
    let released = taken
        .and_then(|t| t.run.input_release_id)
        .filter(|_| d.layout == Layout::Bids);
    let (mut units, left) = match (d.layout, released) {
        (Layout::Stacks, _) => (
            stack_units(registry.store(), d, stacks, place_ids)?,
            LeftOut::new(),
        ),
        (Layout::Bids, Some(release)) => (
            released_units(registry, d, release, place_ids, main)?,
            LeftOut::new(),
        ),
        (Layout::Bids, None) => bids_units(registry, d, stacks, pack, place_ids, main)?,
    };
    let (kept, made) = match taken {
        Some(t) => {
            units.retain(|u| !t.over.contains(&u.name()));
            (t.over.len(), t.made)
        }
        None => (0, 0),
    };
    Ok(Planned {
        units,
        left,
        kept,
        made,
    })
}

/// The units of the bids input a run released before, as a run taken up
/// again goes on with them (record 49 A1): each with the roles it lacks as
/// the runner judges them, and the derivatives its inputs need.
fn released_units(
    registry: &mut Registry,
    d: &Descriptor,
    release: i64,
    place_ids: &[i64],
    main: Option<&Value>,
) -> Result<Vec<Unit>, String> {
    let mut out: Vec<Unit> =
        crate::pipelines::released_units(registry, release, d.level, &d.roles, main)?
            .into_iter()
            .map(|r| Unit {
                id: Some(r.id),
                subject_id: r.subject_id,
                session_day: r.session_day,
                stacks: r.stacks,
                missing: r.missing,
                ..Unit::default()
            })
            .collect();
    let units: Vec<&[i64]> = out.iter().map(|u| u.stacks.as_slice()).collect();
    let lacking = crate::pipelines::derivatives_missing(registry.store(), d, &units, place_ids)?;
    for (u, lacks) in out.iter_mut().zip(lacking) {
        u.missing.extend(lacks);
    }
    Ok(out)
}

/// Whether `named` names this pipeline: by its id, its name@version or its
/// name; nothing named is any.
fn names(p: &rows::Pipeline, named: &str) -> bool {
    named.is_empty() || named == p.name || named == p.label() || named == p.id.to_string()
}

/// The pre-flight of one run, from the registry alone.
pub(crate) fn check(registry: &mut Registry, asked: &Asked<'_>) -> Result<Value, String> {
    // 2026-10-12: a run taken up again is checked as a new run is, under
    // what it recorded, for the units it still has to run
    let taken = match asked.resume {
        Some(id) => Some(taken_up(registry.store(), id)?),
        None => None,
    };
    let p = match &taken {
        Some(t) => rows::get(registry.store(), t.run.pipeline_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("run {} names a pipeline the catalog lost", t.run.id))?,
        None => rows::resolve(registry.store(), asked.pipeline)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no pipeline {} in the catalog", asked.pipeline))?,
    };
    if let Some(t) = &taken
        && !names(&p, asked.pipeline)
    {
        return Err(format!(
            "run {} is a run of {}, not of {}",
            t.run.id,
            p.label(),
            asked.pipeline
        ));
    }
    let d = descriptor::from_value(p.descriptor.clone())
        .map_err(|e| {
            format!(
                "{}: the catalog's descriptor no longer checks: {e}",
                p.label()
            )
        })?
        // record 55 T24: as the run sizes it, to this lane's cores
        .sized_to(crate::pipelines::lane_of(registry).ledger.cores);
    let mut blockers: Vec<String> = Vec::new();
    // a run taken up again goes on with its pipeline whatever the catalog
    // says of it since, and with the parameters it recorded
    let params = match &taken {
        Some(t) => {
            if let Some(refused) = crate::pipelines::resume_refused(registry.store(), &t.run) {
                blockers.push(refused.message);
            }
            t.run.params.clone()
        }
        None => {
            if p.state != "active" {
                blockers.push(format!("{} is {}", p.label(), p.state));
            } else if !asked.replaced
                && let Some(refused) = crate::versions::refused(registry.store(), &p)
            {
                blockers.push(refused);
            }
            match d.resolve(asked.params) {
                Ok(v) => Value::Object(v),
                Err(e) => {
                    blockers.push(e);
                    Value::Null
                }
            }
        }
    };
    let stacks =
        crate::pipelines::handle_stacks(registry.store(), asked.handle).map_err(|e| e.message)?;
    let place_ids = places_of(registry.store(), taken.as_ref());
    // record 55: a dataset's or a cohort's main scans say what was read,
    // and their bids input is released under those picks; the handle's own
    // entry judges its units, each stack by the role it was taken for
    // (2026-10-12), and the answer says what was resolved, each role's own
    // stacks left out
    let stored = crate::main_input::entry_of(registry.store(), asked.handle);
    let main = asked
        .main
        .clone()
        .or_else(|| stored.as_ref().map(crate::main_input::shown));
    let Planned {
        units,
        left,
        kept,
        made,
    } = plan_units(
        registry,
        &d,
        &stacks,
        asked.pack,
        &place_ids,
        stored.as_ref(),
        taken.as_ref(),
    )?;
    // a run taken up again with every unit over has none left to run, and
    // taking it up closes it with what they made
    if units.is_empty() && kept == 0 {
        blockers.push(match (&main, d.layout) {
            (Some(m), _) if stacks.is_empty() => crate::main_input::nothing(m),
            (_, Layout::Bids) => no_units_why(stacks.len(), &left),
            (_, Layout::Stacks) => "the selection holds no stacks".into(),
        });
    }
    let missing: Vec<&Unit> = units.iter().filter(|u| !u.missing.is_empty()).collect();
    let ready_units = units.len() - missing.len();
    let word = unit_word(d.layout, d.level);
    let cannot = cannot_of(&missing);
    // 2026-10-12: a run none of whose units could run would make nothing,
    // and is not ready; one taken up again that made some is, since going
    // on closes it with what they made
    if !units.is_empty() && ready_units == 0 && made == 0 {
        blockers.push(none_can_run(word, units.len(), &cannot));
    }
    // record 57 P2 to P4 (Wave 7b A1): the pack judges every unit that has
    // its inputs: a hard need skips it in the pack's words (or "cannot
    // tell"), a soft need is a caveat, the rules choose each knob and say
    // who chose; a run whose every unit the pack skips would make nothing
    let mut judged_doc = Value::Null;
    let mut pack_skipped = 0usize;
    if let Some(e) = crate::pipeline_pack::pack_error(&p) {
        blockers.push(e);
    }
    if let Some(pack) = crate::pipeline_pack::pack_of(&p) {
        match asked.person.person(&pack, &crate::actor(), asked.params) {
            Err(e) => blockers.push(e),
            Ok(person) => {
                let resolved: serde_json::Map<String, Value> =
                    params.as_object().cloned().unwrap_or_default();
                let with_inputs: Vec<&Unit> =
                    units.iter().filter(|u| u.missing.is_empty()).collect();
                let of: Vec<crate::pipeline_pack::UnitOf> = with_inputs
                    .iter()
                    .map(|u| crate::pipeline_pack::UnitOf {
                        by_role: u.by_role.clone(),
                        subject_id: u.subject_id,
                        session_day: u.session_day.clone(),
                    })
                    .collect();
                let host = crate::pipeline_pack::host_facts(registry);
                let judged = crate::pipeline_pack::judge_units(
                    registry.store(),
                    &pack,
                    &d,
                    &resolved,
                    &of,
                    &host,
                    &person,
                );
                let names: Vec<String> = with_inputs.iter().map(|u| u.name()).collect();
                pack_skipped = judged
                    .iter()
                    .filter(|j| j.status == nils_pipeline::pack::rules::Status::Skipped)
                    .count();
                if d.units == descriptor::Units::Together
                    && let Some(words) = crate::pipeline_pack::together_disagree(&judged)
                {
                    blockers.push(words);
                }
                judged_doc = crate::pipeline_pack::judged_doc(&names, &judged);
                judged_doc["pack_digest"] = json!(p.pack_digest);
            }
        }
    }
    let ready_units = ready_units.saturating_sub(pack_skipped);
    if !units.is_empty() && ready_units == 0 && made == 0 && pack_skipped > 0 {
        let why: Vec<String> = judged_doc["skipped_by_why"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| w["why"].as_str().map(str::to_string))
                    .take(3)
                    .collect()
            })
            .unwrap_or_default();
        let mut why = why;
        let told_not = judged_doc["cannot_tell"].as_u64().unwrap_or(0);
        if told_not > 0 {
            why.push(format!("the registry cannot tell for {told_not}"));
        }
        blockers.push(format!(
            "no {word} can run: {pack_skipped} of {} are skipped by the pack's needs ({})",
            units.len(),
            why.join("; ")
        ));
    }

    // the runtime, the working place and the GPU
    let detected = crate::pipelines::detect_cached(registry);
    let capability = crate::pipelines::capability(registry);
    if capability["enabled"] != true {
        blockers.push(
            capability["reason"]
                .as_str()
                .unwrap_or("pipelines are off here")
                .to_string(),
        );
    }
    // a run taken up again goes on under the runtime it ran under
    if let (Some(t), Some(now)) = (&taken, detected.runtime.as_ref())
        && now.kind.name() != t.run.runtime
    {
        blockers.push(format!(
            "run {} ran under {}, and pipelines here run under {} now; its units would not be alike, so start a new run",
            t.run.id,
            t.run.runtime,
            now.kind.name()
        ));
    }
    // the GPU as a run would take it: the runtime's, and only where the
    // lane names a card to lease
    let (lane, mut budget) = budget(registry);
    let card = detected.runtime.as_ref().and_then(|r| r.gpu.clone());
    let offered = card.clone().filter(|_| lane.card.is_some());
    // record 55 A5: a card smaller than a unit's need is never leased, so
    // an optional need runs on the CPU and a required one cannot run
    let small = match (d.gpu, &offered) {
        (Gpu::Optional | Gpu::Required, Some(_)) => crate::pipelines::card_too_small(&d, &lane),
        _ => None,
    };
    let small_why = small
        .map(|(n, total)| crate::pipelines::too_small_words(&d, n, total))
        .or_else(|| {
            // a card large enough but too full now: the CPU after the grace
            (d.gpu == Gpu::Optional && offered.is_some())
                .then(|| crate::pipelines::card_full_words(&d, &lane))
                .flatten()
        });
    let device = match (d.gpu, &offered) {
        (Gpu::None, _) | (Gpu::Optional, None) => Some("cpu".to_string()),
        (Gpu::Optional, Some(_)) if small.is_some() => Some("cpu".to_string()),
        (Gpu::Required, Some(_)) if small.is_some() => {
            blockers.push(format!(
                "{} needs a GPU, and {}",
                p.label(),
                small_why.clone().unwrap_or_default()
            ));
            None
        }
        (_, Some(g)) => Some(g.clone()),
        (Gpu::Required, None) => {
            blockers.push(if card.is_some() {
                format!(
                    "{} needs a GPU, and the pipeline lane uses no card here; nils pipeline lane --gpu-card <n> names the one it leases",
                    p.label()
                )
            } else {
                format!(
                    "{} needs a GPU, and this engine's runtime offers none",
                    p.label()
                )
            });
            None
        }
    };
    if let (Some(t), Some(now)) = (&taken, device.as_deref())
        && (t.run.device == "cpu") != (now == "cpu")
    {
        blockers.push(format!(
            "run {} ran on {}, and would go on on {now}; start a new run",
            t.run.id, t.run.device
        ));
    }

    // what a unit needs, against the lane
    let ask = nils_pipeline::lane::Ask {
        cores: d.needs.cores,
        memory_mib: nils_pipeline::lane::mib_of_gb(d.needs.memory_gb),
    };
    let (cores, memory) = (f64::from(d.needs.cores), d.needs.memory_gb);
    let fits = lane.ledger.could(ask);
    budget["fits"] = json!(fits);
    if !fits {
        blockers.push(format!(
            "a unit needs {} cores and {} GB, and the lane has {} cores and {} GB{}",
            d.needs.cores,
            descriptor::number_text(memory),
            lane.ledger.cores,
            descriptor::number_text(lane.ledger.memory_mib as f64 / 1024.0),
            lane.why
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        ));
    }
    let apart = d.units == descriptor::Units::Apart;
    let slots = if apart && fits {
        f64::from(lane.ledger.cores / ask.cores.max(1))
            .min((lane.ledger.memory_mib / ask.memory_mib.max(1)) as f64)
            .max(1.0)
    } else {
        1.0
    };

    // the time: the pipeline's own runs here first, its descriptor next
    let past = past_seconds(registry.store(), &p.name)?;
    let (per_unit, source, runs) = match (past, d.unit_minutes) {
        (Some((s, n)), _) => (Some(s), Some("runs"), n),
        (None, Some(m)) => (Some(m * 60.0), Some("descriptor"), 0),
        (None, None) => (None, None, 0),
    };
    let n = units.len() as f64;
    let seconds = per_unit.map(|s| ((n / slots).ceil() * s).round());

    // 2026-10-12: what the run needs of this machine, each check answered,
    // and a failed one a blocker by its kind (the runtime, the lane and the
    // GPU are said above already); a run taken up again needs the converter
    // only where its bids input is still to be released
    let requirements = crate::requirements::checks(
        registry,
        asked.home,
        &crate::requirements::Asked {
            pipeline: &p,
            descriptor: &d,
            dcm2niix: asked.dcm2niix,
            units: units.len().max(1),
            detected: Some(&detected),
            releases: taken
                .as_ref()
                .is_none_or(|t| t.run.input_release_id.is_none()),
        },
    );
    for c in requirements.iter().filter(|c| !c.ok) {
        // record 57 P5: a tool's terms not accepted, or its licence file not
        // placed, refuse the run before it starts (G9)
        if matches!(c.check, "converter" | "image" | "disk" | "licence")
            || c.cause == Some(nils_pipeline::failure::Cause::Memory)
        {
            blockers.push(c.said.clone());
        }
    }
    let failure = crate::requirements::refusing(&requirements)
        .and_then(|c| c.cause)
        .map_or(Value::Null, nils_pipeline::failure::Cause::doc);
    let ready = blockers.is_empty();
    // `skipped_by_pack` only where the pipeline has a pack
    let mut units_doc = json!({
        "total": units.len(),
        "ready": ready_units,
        "missing": missing.len(),
    });
    if !judged_doc.is_null() {
        units_doc["skipped_by_pack"] = json!(pack_skipped);
    }
    Ok(json!({
        "pipeline": p.label(),
        "pipeline_id": p.id,
        "handle": asked.handle,
        "selection": asked.selection.or(taken.as_ref().and_then(|t| t.run.selection.as_deref())),
        "main": main,
        "params": params,
        "layout": d.layout.name(),
        "level": d.level.name(),
        "unit": word,
        "stacks": stacks.len(),
        "units": units_doc,
        // record 57 P2 to P4: per unit {status, need, words, variant, caveats,
        // knobs {value, by, rule}}, and the counts
        "units_judged": judged_doc,
        "person": asked.person.doc(),
        "missing": missing.iter().map(|u| json!({
            "unit": u.name(), "subject_id": u.subject_id, "session_day": u.session_day,
            "stack_id": u.stack_id, "why": u.missing, "lacks": briefs(&u.missing),
        })).collect::<Vec<_>>(),
        "cannot": cannot,
        "resume": taken.as_ref().map(|t| json!({
            "run": t.run.id, "status": t.run.status, "job": t.run.job_id,
            "kept": kept, "made": made,
        })),
        "roles": d.roles,
        "left_out": {
            "stacks": left.len(),
            "why": left_why(&left),
        },
        "estimate": {
            "seconds_per_unit": per_unit.map(f64::round),
            "source": source,
            "runs": runs,
            "slots": slots,
            "units_apart": apart,
            "seconds": seconds,
        },
        "gpu": {
            "need": d.gpu.name(), "available": card, "device": device, "card": lane.card,
            "gpu_memory_gb": (device.as_deref().is_some_and(|d| d != "cpu")).then_some(d.needs.gpu_memory_gb),
            // why the run takes no card that is there: it is smaller than a unit's need
            "why": small_why,
        },
        "needs": {"cores": cores, "memory_gb": memory},
        "budget": budget,
        "checks": d.checks.iter().map(|c| c.text()).collect::<Vec<_>>(),
        "runtime": capability["runtime"],
        "place": capability["place"],
        "scratch": capability["scratch"],
        "ready": ready,
        "blockers": blockers,
        "requirements": requirements.iter().map(crate::requirements::Check::doc).collect::<Vec<_>>(),
        "failure": failure,
    }))
}

/// A run refused before it starts for its units (2026-10-12): the words,
/// which end as a refusal does with its cause (input), the units that
/// cannot run by why, one entry a reason, how many units it has, and what
/// one is called.
pub(crate) struct UnitsRefused {
    pub(crate) words: String,
    pub(crate) cannot: Vec<Value>,
    pub(crate) units: usize,
    pub(crate) unit: &'static str,
}

impl UnitsRefused {
    /// What a door answers beside the words: the failure, the units and why
    /// they cannot run.
    pub(crate) fn doc(&self) -> Value {
        json!({
            "failure": nils_pipeline::failure::Cause::Input.refusal_doc(),
            "unit": self.unit,
            "units": {"total": self.units, "ready": 0, "missing": self.units},
            "cannot": self.cannot,
        })
    }
}

/// The places a run's derivative inputs are found in: a run taken up again
/// keeps its own, a new run the lane's.
fn places_of(store: &mut Store, taken: Option<&Taken>) -> Vec<i64> {
    match taken {
        Some(t) => [t.run.place_id, t.run.scratch_place_id]
            .into_iter()
            .flatten()
            .collect(),
        None => {
            let places = crate::pipelines::run_places(store).ok();
            let mut ids: Vec<i64> = places.iter().map(|p| p.output.id).collect();
            if let Some(p) = &places
                && p.scratch.id != p.output.id
            {
                ids.push(p.scratch.id);
            }
            ids
        }
    }
}

/// 2026-10-12: a run over `handle` (or `resume`, taken up again) that could
/// run none of the units it has, or still has, and has made nothing, would
/// make nothing: it is refused before it starts, by the pre-flight's units
/// and in its words. None where a unit could run, where it has no unit at
/// all (which the run says itself), or where a run taken up again made
/// something, since going on closes it with that.
pub(crate) fn units_refusal(
    registry: &mut Registry,
    d: &Descriptor,
    handle: i64,
    pack: Option<&nils_pack::Pack>,
    main: Option<&Value>,
    resume: Option<i64>,
) -> Result<Option<UnitsRefused>, String> {
    let taken = match resume {
        Some(id) => Some(taken_up(registry.store(), id)?),
        None => None,
    };
    let stacks =
        crate::pipelines::handle_stacks(registry.store(), handle).map_err(|e| e.message)?;
    let place_ids = places_of(registry.store(), taken.as_ref());
    // the handle's own entry judges the units, each stack by the role it
    // was taken for (2026-10-12)
    let main = crate::main_input::entry_of(registry.store(), handle).or_else(|| main.cloned());
    let planned = plan_units(
        registry,
        d,
        &stacks,
        pack,
        &place_ids,
        main.as_ref(),
        taken.as_ref(),
    )?;
    let missing: Vec<&Unit> = planned
        .units
        .iter()
        .filter(|u| !u.missing.is_empty())
        .collect();
    if planned.units.is_empty() || missing.len() < planned.units.len() || planned.made > 0 {
        return Ok(None);
    }
    let cannot = cannot_of(&missing);
    let unit = unit_word(d.layout, d.level);
    let said = none_can_run(unit, planned.units.len(), &cannot);
    let cause = nils_pipeline::failure::Cause::Input;
    Ok(Some(UnitsRefused {
        words: format!(
            "refused before it started: {said}; {} {}",
            cause.advice_before(),
            nils_pipeline::failure::mark(cause)
        ),
        cannot,
        units: planned.units.len(),
        unit,
    }))
}

/// The jobs door's look at a run's units (2026-10-12): over the handle its
/// command names, its selection frozen as the run would freeze it, or the
/// run it takes up again; refused where it could run none of them. What
/// cannot be read here (a selection that is not there, a pack that does not
/// load) is left to the run, which says it when it starts.
pub(crate) fn door_units(
    home: &Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    command: &[String],
    d: &Descriptor,
) -> Option<UnitsRefused> {
    let flag = |name: &str| {
        command
            .iter()
            .position(|w| w == name)
            .and_then(|i| command.get(i + 1))
            .cloned()
    };
    let pack_name = flag("--pack").unwrap_or_else(|| "mri".to_string());
    let dir = flag("--pack-dir")
        .map(PathBuf::from)
        .or_else(|| pack_dir.map(Path::to_path_buf))
        .or_else(|| crate::pack_dir(home, None).ok());
    let resume = flag("--resume").and_then(|r| r.parse::<i64>().ok());
    let (handle, main) = match resume {
        Some(run) => (
            rows::run(registry.store(), run).ok().flatten()?.handle_id?,
            None,
        ),
        None => frozen_over(
            home,
            flag("--select").as_deref(),
            flag("--handle").and_then(|h| h.parse::<i64>().ok()),
            dir.clone(),
            &pack_name,
            &d.roles,
        )
        .ok()
        .flatten()?,
    };
    let pack = dir.and_then(|dir| crate::ask_cli::load_pack(&dir, &pack_name).ok());
    units_refusal(registry, d, handle, pack.as_ref(), main.as_ref(), resume)
        .ok()
        .flatten()
}

/// Why the stacks left out are, each reason once with its count.
fn left_why(left: &[(i64, String)]) -> Value {
    if left.is_empty() {
        return Value::Null;
    }
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, why) in left {
        *by.entry(why.as_str()).or_insert(0) += 1;
    }
    json!(
        by.iter()
            .map(|(w, n)| format!("{n}: {w}"))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

/// A pre-flight read at detail plain: the counts and the reasons, no unit
/// and no visit without a main scan named by its subject or its day.
pub(crate) fn plain(v: &mut Value) {
    if let Some(list) = v["missing"].as_array_mut() {
        for m in list.iter_mut() {
            *m = json!({"why": m["why"].clone(), "lacks": m["lacks"].clone()});
        }
    }
    if let Some(list) = v["cannot"].as_array_mut() {
        for c in list.iter_mut() {
            c["named"] = json!([]);
        }
    }
    if let Some(list) = v["main"]["without"]["named"].as_array_mut() {
        for m in list.iter_mut() {
            *m = json!({"why": m["why"].clone()});
        }
    }
}

/// The pick roles a pipeline's units need (`x-nils.input.roles`), which a
/// scope's main scans written without a role take (2026-10-12); none where
/// the catalog holds no such pipeline, which the pre-flight says itself.
pub(crate) fn needs_of(store: &mut Store, pipeline: &str) -> Vec<String> {
    rows::resolve(store, pipeline)
        .ok()
        .flatten()
        .and_then(|p| descriptor::from_value(p.descriptor).ok())
        .map(|d| d.roles)
        .unwrap_or_default()
}

/// What a pre-flight is over: a dataset's or a cohort's main scans,
/// resolved and frozen now with the visits with none named (record 55); a
/// saved selection, frozen as a run freezes it; or a handle.
pub(crate) fn frozen_over(
    home: &Home,
    spec: Option<&str>,
    handle: Option<i64>,
    pack_dir: Option<PathBuf>,
    pack: &str,
    needs: &[String],
) -> Result<Option<(i64, Option<Value>)>, crate::Exit> {
    let grain = nils_ask::ast::Grain::Stack;
    let whole = crate::ask_cli::Freeze::Whole;
    Ok(match (spec, handle) {
        (Some(spec), _) => match crate::main_input::Spec::parse(spec) {
            Some(main) => {
                let main = main.map_err(usage)?;
                let (f, r) =
                    crate::main_input::freeze(home, &main, grain, pack_dir, pack, whole, needs)?;
                Some((f.handle, Some(r.preflight())))
            }
            None => Some((
                crate::ask_cli::freeze(home, spec, grain, pack_dir, pack, whole)?.handle,
                None,
            )),
        },
        (None, Some(h)) => Some((h, None)),
        (None, None) => None,
    })
}

/// `POST /api/pipelines/{name}/preflight`: `{select: "selection:<name>@<v>"}`
/// or `{handle: <id>}`, and `params` as an object or a list of `id=value`.
pub(crate) fn route(
    home: &Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    quasi: bool,
    post: bool,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let ["api", "pipelines", name, "preflight"] = segs else {
        return None;
    };
    if !post {
        return None;
    }
    Some((|| {
        let doc = crate::serve::json_body(body)?;
        let params: Vec<(String, String)> = match &doc["params"] {
            Value::Null => Vec::new(),
            Value::Object(o) => o
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    )
                })
                .collect(),
            Value::Array(a) => a
                .iter()
                .map(|w| {
                    w.as_str()
                        .and_then(|w| w.split_once('='))
                        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                        .ok_or_else(|| Reply::error(400, "params are id=value words"))
                })
                .collect::<Result<_, _>>()?,
            _ => {
                return Err(Reply::error(
                    400,
                    "params is an object or a list of id=value",
                ));
            }
        };
        let selection = doc["select"].as_str().map(str::to_string);
        // 2026-10-12: a run taken up again, over the handle it pinned
        let resume = match &doc["resume"] {
            Value::Null => None,
            r => Some(
                r.as_i64()
                    .ok_or_else(|| Reply::error(400, "resume is a run's id"))?,
            ),
        };
        let (handle, main) = match resume {
            Some(run) => {
                let r = rows::run(registry.store(), run)
                    .map_err(|e| Reply::error(500, e.to_string()))?
                    .ok_or_else(|| Reply::error(404, format!("no pipeline run {run}")))?;
                let handle = r.handle_id.ok_or_else(|| {
                    Reply::error(409, format!("run {run} pinned no handle"))
                })?;
                (handle, None)
            }
            // a scope's main scans without a role take each role the
            // pipeline needs (2026-10-12)
            None => frozen_over(
                home,
                selection.as_deref(),
                doc["handle"].as_i64(),
                pack_dir.map(PathBuf::from),
                doc["pack"].as_str().unwrap_or("mri"),
                &needs_of(registry.store(), name),
            )
            .map_err(|e| Reply::error(400, e.message))?
            .ok_or_else(|| {
                Reply::error(
                    400,
                    "a pre-flight is over a selection: {select: \"selection:<name>@<v>\"}, a dataset's or a cohort's main scans {select: \"main:<role>@dataset:<name>\"} (or {select: \"main@dataset:<name>\"} for each role the pipeline needs), {handle: <id>}, or a run taken up again {resume: <run>}",
                )
            })?,
        };
        let pack = pack_dir
            .map(Path::to_path_buf)
            .or_else(|| crate::pack_dir(home, None).ok())
            .and_then(|dir| {
                nils_pack::load(&dir.join(doc["pack"].as_str().unwrap_or("mri")), None).ok()
            });
        let mut v = check(
            registry,
            &Asked {
                home: home.dir(),
                dcm2niix: Path::new(crate::tools::DCM2NIIX),
                pipeline: name,
                pack: pack.as_ref(),
                handle,
                selection: selection.as_deref(),
                params: &params,
                main,
                resume,
                replaced: false,
                person: crate::pipeline_pack::PersonAsk::of_body(&doc)
                    .map_err(|e| Reply::error(400, e))?,
            },
        )
        .map_err(|e| {
            if e.starts_with("no pipeline") {
                Reply::error(404, e)
            } else {
                Reply::error(400, e)
            }
        })?;
        if !quasi {
            plain(&mut v);
        }
        Ok(Reply::ok(v))
    })())
}

/// `nils run --preflight`: the pre-flight printed, nothing run.
pub(crate) fn command(home: &Home, args: &crate::pipelines::RunArgs) -> Result<(), Exit> {
    // 2026-10-12: a run taken up again is pre-flown over the handle it
    // pinned, for the units it still has to run
    let (pipeline, handle, main) = match args.resume {
        Some(run) => {
            let mut registry = crate::open(home)?;
            let r = rows::run(registry.store(), run)?
                .ok_or_else(|| usage(format!("no pipeline run {run}")))?;
            let handle = r
                .handle_id
                .ok_or_else(|| usage(format!("run {run} pinned no handle")))?;
            (args.pipeline.as_deref().unwrap_or_default(), handle, None)
        }
        None => {
            let pipeline = args
                .pipeline
                .as_deref()
                .ok_or_else(|| usage("run <pipeline> --preflight: the pipeline to check"))?;
            let needs = {
                let mut registry = crate::open(home)?;
                needs_of(registry.store(), pipeline)
            };
            let (handle, main) = frozen_over(
                home,
                args.select.as_deref(),
                args.handle,
                args.pack_dir.clone(),
                &args.pack,
                &needs,
            )?
            .ok_or_else(|| {
                usage(
                    "a pre-flight is over a frozen selection: --select selection:<name>@<v>, a dataset's or a cohort's main scans (--select main:<role>@dataset:<name>, or main@dataset:<name> for each role the pipeline needs), --handle <id>, or a run taken up again (--resume <run>)",
                )
            })?;
            (pipeline, handle, main)
        }
    };
    let params: Vec<(String, String)> = args
        .params
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .ok_or_else(|| usage(format!("--param {w}: write it as id=value")))
        })
        .collect::<Result<_, _>>()?;
    let pack = crate::pack_dir(home, args.pack_dir.clone())
        .ok()
        .and_then(|dir| crate::ask_cli::load_pack(&dir, &args.pack).ok());
    let mut registry = crate::open(home)?;
    let v = check(
        &mut registry,
        &Asked {
            home: home.dir(),
            dcm2niix: &args.dcm2niix,
            pipeline,
            pack: pack.as_ref(),
            handle,
            selection: args.select.as_deref(),
            params: &params,
            main,
            resume: args.resume,
            replaced: args.replaced,
            person: crate::pipeline_pack::PersonAsk::of_args(
                args.goal.as_deref(),
                &args.switches,
                args.let_run,
            )
            .map_err(usage)?,
        },
    )
    .map_err(usage)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
        );
        return Ok(());
    }
    println!(
        "pre-flight of {} over {} ({} stacks, handle {})",
        v["pipeline"].as_str().unwrap_or_default(),
        v["selection"].as_str().unwrap_or("a handle"),
        v["stacks"],
        v["handle"]
    );
    if v["main"].is_object() {
        println!("  main scans       {}", crate::main_input::line(&v["main"]));
        for m in v["main"]["without"]["named"]
            .as_array()
            .into_iter()
            .flatten()
            .take(20)
        {
            println!(
                "    {} {}: {}",
                m["subject"].as_str().unwrap_or_default(),
                m["session_day"].as_str().unwrap_or_default(),
                m["why"].as_str().unwrap_or_default()
            );
        }
    }
    if v["resume"].is_object() {
        let r = &v["resume"];
        println!(
            "  taken up again   run {} ({}): {} over and kept, {} of them made",
            r["run"],
            r["status"].as_str().unwrap_or_default(),
            r["kept"],
            r["made"]
        );
    }
    println!(
        "  units            {} ({} ready, {} missing an input)",
        v["units"]["total"], v["units"]["ready"], v["units"]["missing"]
    );
    // 2026-10-12: the units that cannot run, one line a reason with the
    // units under it, or that every one can
    let word = v["unit"].as_str().unwrap_or("unit");
    let total = v["units"]["total"].as_u64().unwrap_or(0) as usize;
    if total > 0 && v["units"]["missing"].as_u64() == Some(0) {
        println!(
            "  can run          every {word}: {}",
            units_words(total, word)
        );
    }
    for c in v["cannot"].as_array().into_iter().flatten() {
        let n = c["units"].as_u64().unwrap_or(0) as usize;
        println!(
            "  cannot run       {}: {}",
            units_words(n, word),
            c["why"].as_str().unwrap_or_default()
        );
        let named: Vec<&str> = c["named"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !named.is_empty() {
            let shown = named.len().min(10);
            println!(
                "    {}{}",
                named[..shown].join(", "),
                if n > shown {
                    format!(" and {} more", n - shown)
                } else {
                    String::new()
                }
            );
        }
    }
    if v["left_out"]["stacks"].as_u64().unwrap_or(0) > 0 {
        println!(
            "  left out         {} stacks: {}",
            v["left_out"]["stacks"],
            v["left_out"]["why"].as_str().unwrap_or_default()
        );
    }
    let e = &v["estimate"];
    match e["seconds"].as_f64() {
        Some(s) => println!(
            "  time             about {} min ({} s a unit from {}, {} at once)",
            (s / 60.0).ceil(),
            e["seconds_per_unit"],
            e["source"].as_str().unwrap_or_default(),
            e["slots"]
        ),
        None => println!(
            "  time             unknown: it has not run here and its descriptor names no unit-minutes"
        ),
    }
    println!(
        "  GPU              needs {}, this engine has {}, it would run on {}{}",
        v["gpu"]["need"].as_str().unwrap_or_default(),
        v["gpu"]["available"].as_str().unwrap_or("none"),
        v["gpu"]["device"].as_str().unwrap_or("nothing"),
        v["gpu"]["why"]
            .as_str()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
    );
    println!(
        "  a unit needs     {} cores, {} GB; the lane has {} cores, {} GB{}",
        v["needs"]["cores"],
        v["needs"]["memory_gb"],
        v["budget"]["cores"],
        v["budget"]["memory_gb"],
        v["budget"]["why"]
            .as_str()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
    );
    for c in v["requirements"].as_array().into_iter().flatten() {
        println!(
            "  {:<16} {}{}",
            c["check"].as_str().unwrap_or_default(),
            if c["ok"] == true { "" } else { "not met: " },
            c["said"].as_str().unwrap_or_default()
        );
    }
    if v["ready"] == true {
        match args.resume {
            Some(run) => println!("ready: nils run --resume {run} takes it up again"),
            None => println!(
                "ready: nils run {} starts it",
                v["pipeline"].as_str().unwrap_or_default()
            ),
        }
    } else {
        for b in v["blockers"].as_array().into_iter().flatten() {
            println!("not ready: {}", b.as_str().unwrap_or_default());
        }
        if let Some(f) = crate::failures::line(&v["failure"]) {
            println!("what to do: {f}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-12: a unit's reason in a few words, from the words the
    /// pre-flight and the run say, a reason it has none for said whole; the
    /// units that cannot run one entry a reason, the most first; and a run
    /// none of whose units could run says so.
    #[test]
    fn a_reason_is_said_in_a_few_words_and_the_units_folded_by_it() {
        let roles = vec!["t1w".to_string(), "flair".to_string()];
        let no_t1w = roles_missing(&roles, &held(&[("flair", &[Some("FLAIR")])]));
        assert_eq!(brief(&no_t1w[0]), "no T1w picked");
        let no_flair = roles_missing(&roles, &held(&[("t1w", &[Some("T1w")])]));
        assert_eq!(brief(&no_flair[0]), "no FLAIR picked");
        let as_t2 = held(&[
            ("t1w", &[Some("sub-a_ses-b_acq-x_T2w")]),
            ("flair", &[Some("FLAIR")]),
        ]);
        assert_eq!(
            brief(&roles_missing(&roles, &as_t2)[0]),
            "its T1w is released as T2w"
        );
        let outside = held(&[("t1w", &[None]), ("flair", &[Some("FLAIR")])]);
        assert_eq!(
            brief(&roles_missing(&roles, &outside)[0]),
            "its T1w is outside the input"
        );
        assert_eq!(
            brief("no flair stands for it: a person's pick 12 says so"),
            "a person said no FLAIR stands"
        );
        assert_eq!(
            brief("no dixon is picked for it among the selection's stacks"),
            "no dixon picked"
        );
        use crate::pipelines::derivative_lack;
        assert_eq!(
            brief(&derivative_lack(0, "mask", "mask", "not_rejected")),
            "no mask yet"
        );
        assert_eq!(
            brief(&derivative_lack(2, "mask", "mask", "accepted")),
            "no accepted mask"
        );
        assert_eq!(
            brief(&derivative_lack(1, "mask", "mask", "not_rejected")),
            "its mask was rejected"
        );
        assert_eq!(
            brief(&derivative_lack(3, "mask", "lesions", "not_rejected")),
            "every mask rejected"
        );
        assert_eq!(
            brief("the registry holds no file of it to read"),
            "no file to read"
        );
        assert_eq!(brief("a reason of its own"), "a reason of its own");

        let a = Unit {
            id: Some("sub-a_ses-1".into()),
            missing: no_t1w.clone(),
            ..Unit::default()
        };
        let b = Unit {
            id: Some("sub-b_ses-1".into()),
            missing: [no_t1w.clone(), no_flair.clone()].concat(),
            ..Unit::default()
        };
        let cannot = cannot_of(&[&a, &b]);
        assert_eq!(
            cannot,
            vec![
                json!({"why": "no T1w picked", "units": 2, "named": ["sub-a_ses-1", "sub-b_ses-1"]}),
                json!({"why": "no FLAIR picked", "units": 1, "named": ["sub-b_ses-1"]}),
            ]
        );
        assert_eq!(
            none_can_run("session", 2, &cannot),
            "no session can run: 2 of 2 lack an input (no T1w picked: 2; no FLAIR picked: 1)"
        );
        assert_eq!(
            none_can_run("session", 1, &cannot_of(&[&a])),
            "no session can run: 1 of 1 lacks an input (no T1w picked)"
        );
        assert_eq!(unit_word(Layout::Bids, Level::Session), "session");
        assert_eq!(unit_word(Layout::Bids, Level::Participant), "subject");
        assert_eq!(unit_word(Layout::Stacks, Level::Stack), "stack");
    }

    fn held(pairs: &[(&str, &[Option<&str>])]) -> BTreeMap<String, Vec<Option<String>>> {
        pairs
            .iter()
            .map(|(r, s)| {
                (
                    r.to_string(),
                    s.iter().map(|x| x.map(str::to_string)).collect(),
                )
            })
            .collect()
    }

    /// A role is there when a stack picked for it is released under the
    /// role's own suffix, or with it in its `acq-` where the release spells
    /// it as a modifier (a FLAIR, record 55 C4); a role the standard spells
    /// no suffix for, when a stack picked for it is in the raw tree at all.
    #[test]
    fn a_role_is_there_only_under_its_own_suffix() {
        let roles = vec!["t1w".to_string(), "flair".to_string()];
        let ok = held(&[("t1w", &[Some("T1w")]), ("flair", &[Some("FLAIR")])]);
        assert!(roles_missing(&roles, &ok).is_empty());
        let as_flair = held(&[("t1w", &[Some("FLAIR")]), ("flair", &[Some("FLAIR")])]);
        let why = roles_missing(&roles, &as_flair);
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("released as FLAIR, not T1w"), "{why:?}");
        // as the release names them since record 55 C4: a FLAIR is a
        // modifier in `acq-` on its base contrast's suffix
        let named = held(&[
            ("t1w", &[Some("sub-a_ses-b_acq-Sag+3D+MPRAGE_T1w")]),
            ("flair", &[Some("sub-a_ses-b_acq-Ax+2D+FLAIR+IRTSE_T2w")]),
        ]);
        assert!(roles_missing(&roles, &named).is_empty());
        let no_flair = held(&[
            ("t1w", &[Some("sub-a_ses-b_acq-Sag+3D+MPRAGE_T1w")]),
            ("flair", &[Some("sub-a_ses-b_acq-Ax+2D+IRTSE_T2w")]),
        ]);
        let why = roles_missing(&roles, &no_flair);
        assert!(why[0].contains("released as T2w, not FLAIR"), "{why:?}");
        // only a modifier is looked for in `acq-`: a base contrast spelt
        // there beside another suffix is not the role's suffix
        let t2w = vec!["t2w".to_string()];
        let mese = held(&[("t2w", &[Some("sub-a_ses-b_acq-Ax+T2w+2D_MESE")])]);
        let why = roles_missing(&t2w, &mese);
        assert!(why[0].contains("released as MESE, not T2w"), "{why:?}");
        let elsewhere = held(&[("t1w", &[None]), ("flair", &[Some("FLAIR")])]);
        assert!(roles_missing(&roles, &elsewhere)[0].contains("outside"));
        let none = held(&[("flair", &[Some("FLAIR")])]);
        assert!(roles_missing(&roles, &none)[0].contains("no t1w is picked"));
        // two picks of a role, one under its suffix, is the role there
        let two = held(&[
            ("t1w", &[Some("FLAIR"), Some("T1w")]),
            ("flair", &[Some("FLAIR")]),
        ]);
        assert!(roles_missing(&roles, &two).is_empty());
        // a role with no suffix of its own
        let dixon = vec!["dixon".to_string()];
        assert!(roles_missing(&dixon, &held(&[("dixon", &[Some("T1w")])])).is_empty());
        assert!(!roles_missing(&dixon, &held(&[("dixon", &[None])])).is_empty());
        assert_eq!(
            unit_of_dir("sub-a/ses-b/anat", Level::Session).as_deref(),
            Some("sub-a_ses-b")
        );
        assert_eq!(
            unit_of_dir("sub-a/ses-b/anat", Level::Participant).as_deref(),
            Some("sub-a")
        );
        assert_eq!(
            unit_of_dir("sourcedata/sub-a/ses-b/anat", Level::Session),
            None
        );
    }
}
