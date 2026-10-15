// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56 §5.4: a rule change shows its effect on the sorting.
//!
//! A patch of typed operations (`nils_pack::patch`) is applied to a copy of
//! the pack, and the registry in scope is sorted twice from what it holds,
//! once by the pack as it is and once by the patched pack, without a row
//! written: the rules, the decisions in force, the session passes, the
//! physics vote, the questions a sort asks, the disposition, and the main
//! scans picked. What differs between the two is the change's effect:
//!
//! 1. per axis, how many scans move and from what to what, each row with its
//!    datasets, sites, makes and scanners and example scans;
//! 2. the names that change, descriptive and BIDS, before and after;
//! 3. the main-scan picks that change, by role;
//! 4. the Review questions that appear and disappear, by kind;
//! 5. against the settled answers the registry holds (the decisions people
//!    made, which is where Review answers and a campaign's close land),
//!    right to wrong and wrong to right, on the rules' own answer;
//! 6. the scope it reaches and the version it would ship as.
//!
//! An operation scoped to a site, a dataset or a scanner (an overlay) moves
//! only the stacks in its scope: each stack is sorted after by the pack with
//! the pack-wide operations and those whose scope holds it (record 56 §5.6,
//! an overlay never leaves its scope). The registry is read as it is sorted
//! (record 56 §6.5): the pack's adopted scoped changes are applied on both
//! sides, each to the stacks its scope holds, and the patch's operations
//! after them; where a scope holds a stack is [`crate::scoped`]'s answer,
//! the one a scoped re-sort and every sort read too.
//!
//! A stack of a sample sealed now is never read (record 48, D1 of the move):
//! it is in no scope, no pool, no session and no answer set, and the report
//! says how many were left out. Like the archive replay, the stacks are read
//! once and sorted in parallel chunks; the physics vote and the picks read
//! the registry's stored answers for the stacks outside the scope, as a
//! re-sort of the scope does.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use nils_pack::pass::Corpus;
use nils_pack::patch::{Patch, Scope};
use nils_pack::rules::AxisPhase;
use nils_pack::session::{InForce, Sib, TIER};
use nils_pack::stack::{FIELDS as PACK_FIELDS, Stack, Value as FieldValue};
use nils_pack::{Evaluated, Pack};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Param, Store};
use serde::Serialize;
use serde_json::{Value, json};

use crate::classify::{Decisions, FIELDS, Ids, cell_text, field_sql, select_stacks, to_stack};

/// Why a report could not be made: the patch or the scope (the caller's to
/// mend), or the registry.
#[derive(Debug)]
pub enum Error {
    Refused(String),
    Store(String),
    /// Its progress asked it to stop, between two chunks.
    Stopped,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(m) | Error::Store(m) => f.write_str(m),
            Error::Stopped => f.write_str("the rehearsal was stopped"),
        }
    }
}

impl std::error::Error for Error {}

impl From<nils_registry::store::Error> for Error {
    fn from(e: nils_registry::store::Error) -> Error {
        Error::Store(e.to_string())
    }
}

impl From<crate::job::Error> for Error {
    fn from(e: crate::job::Error) -> Error {
        Error::Store(e.to_string())
    }
}

impl From<nils_pack::Error> for Error {
    fn from(e: nils_pack::Error) -> Error {
        Error::Refused(e.to_string())
    }
}

/// The fingerprint's facts a name is built from, beside the axes.
#[derive(Debug, Clone, Default)]
pub struct NameFacts {
    /// How many stacks its series was split into: an echo is named where
    /// there are several.
    pub stacks_in_series: Option<i64>,
    pub orientation: Option<String>,
    pub echo_numbers: Option<String>,
    pub mr_acquisition_type: Option<String>,
    pub dwi_pe_direction: Option<String>,
    pub dwi_b_value: Option<f64>,
    pub dwi_directions: Option<i64>,
    pub series_number: Option<i64>,
    /// The acquisition family the pack's routes recognised (record 55,
    /// ruling 23), which a name spells where nothing else in it says it.
    pub acquisition_family: Option<String>,
}

/// A stack's names from its axes as stored and its facts: the descriptive
/// name, and the BIDS name where the standard has one. The release's
/// grammar, handed in by the caller, which this crate does not hold.
pub type Namer<'a> =
    &'a (dyn Fn(&Pack, &BTreeMap<String, String>, &NameFacts) -> (String, Option<String>) + Sync);

/// What a report is asked for.
pub struct Settings {
    /// The stacks replayed and counted. None: the operations' own scopes,
    /// or the whole registry where one operation is a pack edit.
    pub scope: Option<Scope>,
    /// Example scans per transition row, and per kind of change.
    pub examples: usize,
    /// Threads for the replay; 0 is the machine's.
    pub workers: usize,
    /// The most stacks one report replays; a scope holding more is refused,
    /// naming the cap (#451). None: no cap.
    pub cap: Option<usize>,
    /// Told the stacks replayed and in scope after each chunk; answering
    /// false stops the report, which then fails with [`Error::Stopped`].
    pub progress: Option<Progress>,
    /// Name every answer the change breaks (right to wrong), not only the
    /// first examples: a proposal's approval card says each one (record 56
    /// section 5.6).
    pub all_breaks: bool,
}

/// What a report says between its chunks (#451), and whether it goes on.
pub type Progress = std::sync::Arc<dyn Fn(usize, usize) -> bool + Send + Sync>;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            scope: None,
            examples: 5,
            workers: 0,
            cap: None,
            progress: None,
            all_breaks: false,
        }
    }
}

/// The confidence an axis involved in a broken constraint is written at,
/// as a classify writes it.
const BROKEN: f64 = crate::classify::BROKEN_CONFIDENCE;

/// How many stacks are read and replayed at once.
const CHUNK: usize = 2_048;

// ---------------------------------------------------------------------------
// The registry, as the report reads it: where each stack sits, and what a
// scope holds, as `crate::scoped` answers it for every reader.

type Place = crate::scoped::Where;

fn scoped_error(e: crate::scoped::Error) -> Error {
    match e {
        crate::scoped::Error::Refused(m) => Error::Refused(m),
        crate::scoped::Error::Store(m) => Error::Store(m),
    }
}

// ---------------------------------------------------------------------------
// One stack sorted by one pack.

/// What the rules and the decisions in force made of one stack.
#[derive(Debug, Clone, Default)]
struct Class {
    /// Per axis of the pack: what is in force after the rules and the
    /// decisions, as a pass reads it.
    axes: Vec<InForce>,
    /// Per axis: the rules' own values, before any decision.
    own: Vec<Vec<String>>,
    /// The axes a rule decided, as a verdict lists them.
    said: Vec<bool>,
    unresolved: Vec<usize>,
    silent: bool,
    /// The questions the sort asks of it: `classify.<kind>` for a broken
    /// constraint, `<axis>:decision`, then the passes' and `<axis>:missing`.
    questions: Vec<String>,
    /// The axes a decision holds.
    decided: Vec<usize>,
    /// The session passes, by their place among the pack's passes, whose
    /// target holds on the stack as the rules and the decisions left it.
    targets: Vec<usize>,
    /// What each family's route said of it, which with the decisions in
    /// force and the provenance make its acquisition family (record 55,
    /// ruling 23), as a release reads them off the stack's evidence.
    routes: Vec<crate::family::Said>,
}

fn split(stored: &str) -> Vec<String> {
    stored
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

fn classify(
    pack: &Pack,
    constraints: &Value,
    decisions: &Decisions,
    ids: Ids,
    stack: &Stack,
    private: &[String],
) -> Class {
    let n = pack.axes.len();
    let e = Evaluated::with_private(pack, stack, private.to_vec());
    let v = e.classify();
    let mut c = Class {
        axes: vec![InForce::default(); n],
        own: vec![Vec::new(); n],
        said: vec![false; n],
        silent: v.silent,
        ..Class::default()
    };
    for e in &v.evidence {
        if !crate::family::ROUTE_FAMILIES
            .iter()
            .any(|(set, _)| *set == e.rule_set)
        {
            continue;
        }
        let at = match c.routes.iter().position(|s| s.route == e.rule_set) {
            Some(i) => i,
            None => {
                c.routes.push(crate::family::Said {
                    route: e.rule_set.clone(),
                    axes: BTreeMap::new(),
                });
                c.routes.len() - 1
            }
        };
        let values = c.routes[at].axes.entry(e.axis.clone()).or_default();
        if !e.value.is_empty() && !values.contains(&e.value) {
            values.push(e.value.clone());
        }
    }
    let doubted = if v.silent {
        Vec::new()
    } else {
        crate::classify::broken_constraints(pack, constraints, &v)
    };
    for b in &doubted {
        c.questions.push(format!("classify.{}", b.kind));
    }
    let any = decisions.any();
    for a in &v.axes {
        let Some(i) = pack.axis_index(&a.axis) else {
            continue;
        };
        let mut value = a.stored();
        let mut tier = a.tier.clone();
        let written = if doubted.iter().any(|b| b.axes.contains(&a.axis)) {
            a.confidence.min(BROKEN)
        } else {
            a.confidence
        };
        c.own[i] = split(&value);
        c.said[i] = true;
        if let Some(d) = any
            .then(|| decisions.for_stack(ids, stack, &a.axis))
            .flatten()
        {
            let decided = d.value.clone().unwrap_or_default();
            if decided != value && !pack.review.by_model.contains(&a.axis) {
                c.questions.push(format!("{}:decision", a.axis));
            }
            value = decided;
            tier = "decision".into();
            c.decided.push(i);
        }
        c.axes[i] = InForce {
            values: split(&value),
            tier,
            confidence: written,
        };
    }
    if any {
        for (i, a) in pack.axes.iter().enumerate() {
            if a.phase != AxisPhase::Class || c.said[i] {
                continue;
            }
            if let Some(d) = decisions.for_stack(ids, stack, &a.name) {
                c.axes[i] = InForce {
                    values: split(d.value.as_deref().unwrap_or("")),
                    tier: "decision".into(),
                    confidence: 1.0,
                };
                c.decided.push(i);
            }
        }
    }
    c.unresolved = v
        .unresolved
        .iter()
        .filter_map(|a| pack.axis_index(a))
        .collect();
    // which session passes target it, read now while its fields are at
    // hand; a pass that writes it is the only thing that changes this
    let decided: Vec<Vec<String>> = c.axes.iter().map(|a| a.values.clone()).collect();
    for (pi, pass) in pack.passes.iter().enumerate() {
        if pass.session().is_none() {
            continue;
        }
        if pass
            .target
            .as_ref()
            .is_none_or(|t| e.holds_with(t, &decided))
        {
            c.targets.push(pi);
        }
    }
    c
}

// ---------------------------------------------------------------------------
// What the report holds per stack.

/// One stack, both ways.
struct Two {
    id: i64,
    /// Which pack sorts it, before and after: the pack with the adopted
    /// scoped changes whose scope holds it, then also the patch's
    /// operations whose scope holds it.
    keys: [usize; 2],
    before: Class,
    after: Class,
    /// Its fields and private elements, kept where a session pass may
    /// target it, which reads them again.
    kept: Option<(Stack, Vec<String>)>,
}

/// The sides of a stack: 0 before, 1 after.
fn side(t: &mut Two, s: usize) -> &mut Class {
    if s == 0 { &mut t.before } else { &mut t.after }
}

/// A transition of one axis, gathered.
#[derive(Default)]
struct Moves {
    stacks: i64,
    datasets: BTreeMap<String, i64>,
    sites: BTreeMap<String, i64>,
    makes: BTreeMap<String, i64>,
    scanners: BTreeMap<String, i64>,
    examples: Vec<i64>,
}

#[derive(Default)]
struct AxisMoves {
    moved: i64,
    held_by_decision: i64,
    rows: BTreeMap<(String, String), Moves>,
}

/// One stack's names before and after.
#[derive(Debug, Clone, Serialize)]
pub struct Renamed {
    pub stack: i64,
    pub before: Value,
    pub after: Value,
}

/// Where every stack sits, what is sealed, the operations' scopes as the
/// registry names them, and the stacks a report replays.
pub struct Scoping {
    resolver: crate::scoped::Resolver,
    place: BTreeMap<i64, Place>,
    sealed: BTreeSet<i64>,
    op_scopes: Vec<Scope>,
    adopted: crate::scoped::Adopted,
    adopted_scopes: Vec<Scope>,
    in_scope: Vec<i64>,
    scope_text: String,
}

fn scoping(
    store: &mut Store,
    base: &Pack,
    patch: &Patch,
    settings: &Settings,
) -> Result<Scoping, Error> {
    let adopted = crate::scoped::adopted(store, base).map_err(scoped_error)?;

    // --- where everything sits, and what is sealed
    let resolver = crate::scoped::Resolver::load(store, &base.modality).map_err(scoped_error)?;
    let mut place = resolver.at.clone();
    let all: Vec<i64> = place.keys().copied().collect();
    let (sealed, _) = nils_registry::labels::sealed_now(store, &all, &[])
        .map_err(|e| Error::Store(e.to_string()))?;
    for s in &sealed {
        place.remove(s);
    }
    // the operations' scopes, named as the registry knows them
    let mut op_scopes: Vec<Scope> = Vec::new();
    for op in &patch.operations {
        op_scopes.push(resolver.known(&op.scope, store).map_err(scoped_error)?);
    }
    let report_scope = match &settings.scope {
        Some(s) => resolver.known(s, store).map_err(scoped_error)?,
        None => {
            if op_scopes.iter().any(Scope::is_pack) {
                Scope::Pack
            } else {
                // the union of the operations' scopes, as one when they are one
                let distinct: BTreeSet<&Scope> = op_scopes.iter().collect();
                if distinct.len() == 1 {
                    op_scopes[0].clone()
                } else {
                    Scope::Pack
                }
            }
        }
    };
    let union_only = settings.scope.is_none()
        && !op_scopes.iter().any(Scope::is_pack)
        && op_scopes.iter().collect::<BTreeSet<_>>().len() > 1;
    let in_scope: Vec<i64> = place
        .iter()
        .filter(|(_, p)| {
            if union_only {
                op_scopes.iter().any(|s| resolver.holds_at(s, p))
            } else {
                resolver.holds_at(&report_scope, p)
            }
        })
        .map(|(id, _)| *id)
        .collect();
    let scope_text = if union_only {
        op_scopes
            .iter()
            .map(Scope::text)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" + ")
    } else {
        report_scope.text()
    };

    let adopted_scopes: Vec<Scope> = adopted
        .ops
        .iter()
        .map(|a| resolver.kept(&a.op.scope, store))
        .collect();
    Ok(Scoping {
        resolver,
        place,
        sealed,
        op_scopes,
        adopted,
        adopted_scopes,
        in_scope,
        scope_text,
    })
}

impl Scoping {
    /// How many stacks the report replays.
    pub fn stacks(&self) -> usize {
        self.in_scope.len()
    }
}

/// Where everything sits and what a report of the patch would replay, read
/// once: a door counts it to answer at once, queue a job or refuse over the
/// cap, and hands it to [`run_scoped`] rather than reading it again.
pub fn scope(
    store: &mut Store,
    base: &Pack,
    patch: &Patch,
    scope: Option<Scope>,
) -> Result<Scoping, Error> {
    let settings = Settings {
        scope,
        ..Settings::default()
    };
    scoping(store, base, patch, &settings)
}

/// How many stacks a report of a patch would replay (#451): what a door
/// reads to answer at once, queue a job, or refuse over the cap.
pub fn scope_size(
    store: &mut Store,
    base: &Pack,
    patch: &Patch,
    scope: Option<Scope>,
) -> Result<usize, Error> {
    let settings = Settings {
        scope,
        ..Settings::default()
    };
    Ok(scoping(store, base, patch, &settings)?.in_scope.len())
}

/// The refusal of a scope over the cap, in words.
pub fn over_cap(stacks: usize, cap: usize) -> String {
    format!(
        "{stacks} stacks are in scope, more than the {cap} one rehearsal replays; narrow the scope (a site, a dataset, a scanner or a batch)"
    )
}

/// The effect of a patch: the report, as a document.
pub fn run(
    store: &mut Store,
    dir: &Path,
    base: &Pack,
    patch: &Patch,
    settings: &Settings,
    namer: Namer<'_>,
) -> Result<Value, Error> {
    let scoped = scoping(store, base, patch, settings)?;
    run_scoped(store, dir, base, patch, settings, namer, scoped)
}

/// [`run`] over a scope [`scope`] read already, with the same settings'
/// scope.
pub fn run_scoped(
    store: &mut Store,
    dir: &Path,
    base: &Pack,
    patch: &Patch,
    settings: &Settings,
    namer: Namer<'_>,
    scoped: Scoping,
) -> Result<Value, Error> {
    let started = Instant::now();
    let mut seconds: BTreeMap<&str, f64> = BTreeMap::new();
    let mut lap = Instant::now();
    fn tick(seconds: &mut BTreeMap<&str, f64>, name: &'static str, lap: &mut Instant) {
        seconds.insert(name, lap.elapsed().as_secs_f64());
        *lap = Instant::now();
    }

    // --- the patch, whole: it applies and the loader builds it
    let whole = nils_pack::patch::apply(dir, patch, &|_| true)?;
    tick(&mut seconds, "patch", &mut lap);

    // --- where everything sits, what is sealed, and what is in scope
    let Scoping {
        resolver,
        place,
        sealed,
        op_scopes,
        adopted,
        adopted_scopes,
        in_scope,
        scope_text,
    } = scoped;
    // record 56 §6.5: the registry is sorted with the pack's adopted scoped
    // changes, each on the stacks its scope holds; both sides apply them,
    // and the patch's operations come after them
    let combined = adopted.with(&base.name, Some(patch));
    let first = adopted.ops.len();
    if let Some(cap) = settings.cap
        && in_scope.len() > cap
    {
        return Err(Error::Refused(over_cap(in_scope.len(), cap)));
    }

    // --- which pack sorts each stack, before and after: before, the pack
    // with the adopted scoped changes whose scope holds it; after, those and
    // the patch's operations whose scope holds it (the pack-wide ones hold
    // every stack)
    let mut keys: [Vec<BTreeSet<usize>>; 2] = Default::default();
    let mut key_of: HashMap<i64, [usize; 2]> = HashMap::new();
    let mut by_op: Vec<i64> = vec![0; patch.operations.len()];
    for id in &in_scope {
        let p = &place[id];
        let before: BTreeSet<usize> = adopted_scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| resolver.holds_at(s, p))
            .map(|(i, _)| i)
            .collect();
        let mut after = before.clone();
        for (j, s) in op_scopes.iter().enumerate() {
            if resolver.holds_at(s, p) {
                by_op[j] += 1;
                after.insert(first + j);
            }
        }
        let mut k = [0usize; 2];
        for (side, set) in [before, after].into_iter().enumerate() {
            k[side] = match keys[side].iter().position(|x| *x == set) {
                Some(i) => i,
                None => {
                    keys[side].push(set);
                    keys[side].len() - 1
                }
            };
        }
        key_of.insert(*id, k);
    }
    let build = |set: &BTreeSet<usize>| -> Result<Pack, Error> {
        // a stack nothing reaches is sorted as the pack is
        if set.is_empty() {
            return Ok(base.clone());
        }
        if first == 0 && set.len() == patch.operations.len() {
            return Ok(whole.pack.clone());
        }
        nils_pack::patch::apply(dir, &combined, &|op| set.contains(&(op.at - 1)))
            .map(|p| p.pack)
            .map_err(|e| {
                Error::Refused(format!(
                    "the operations do not apply apart, as their scopes would apply them to some stacks: {e}"
                ))
            })
    };
    let bpacks: Vec<Pack> = keys[0].iter().map(build).collect::<Result<_, _>>()?;
    let packs: Vec<Pack> = keys[1].iter().map(build).collect::<Result<_, _>>()?;
    let sides: [&[Pack]; 2] = [&bpacks, &packs];
    tick(&mut seconds, "packs", &mut lap);

    // --- the rules and the decisions, both ways, in parallel chunks
    let decisions = Decisions::load(store)?;
    let constraints_before: Vec<Value> = bpacks
        .iter()
        .map(nils_pack::legal::class_constraints)
        .collect();
    let constraints_after: Vec<Value> = packs
        .iter()
        .map(nils_pack::legal::class_constraints)
        .collect();
    let workers = match settings.workers {
        0 => std::thread::available_parallelism().map_or(4, |n| n.get()),
        n => n,
    }
    .clamp(1, 64);
    let mut two: Vec<Two> = Vec::with_capacity(in_scope.len());
    let mut read_s = 0.0;
    for chunk in in_scope.chunks(CHUNK) {
        if let Some(go_on) = &settings.progress
            && !go_on(two.len(), in_scope.len())
        {
            return Err(Error::Stopped);
        }
        let t = Instant::now();
        let rows = store.query(&select_stacks(store, chunk), &[])?;
        let mut stacks: Vec<(Ids, Stack, Vec<String>)> = Vec::with_capacity(rows.len());
        for r in &rows {
            let (mut ids, s, private) = to_stack(r, false, base)?;
            let p = &place[&ids.stack];
            ids.series = p.series;
            ids.subject = p.subject;
            stacks.push((ids, s, private));
        }
        read_s += t.elapsed().as_secs_f64();
        let per = stacks.len().div_ceil(workers).max(1);
        let out: Vec<Vec<Two>> = std::thread::scope(|scope| {
            let handles: Vec<_> = stacks
                .chunks(per)
                .map(|part| {
                    let packs = &packs;
                    let bpacks = &bpacks;
                    let constraints_after = &constraints_after;
                    let constraints_before = &constraints_before;
                    let decisions = &decisions;
                    let key_of = &key_of;
                    scope.spawn(move || {
                        part.iter()
                            .map(|(ids, s, private)| {
                                let k = key_of[&ids.stack];
                                let before = classify(
                                    &bpacks[k[0]],
                                    &constraints_before[k[0]],
                                    decisions,
                                    *ids,
                                    s,
                                    private,
                                );
                                let after = classify(
                                    &packs[k[1]],
                                    &constraints_after[k[1]],
                                    decisions,
                                    *ids,
                                    s,
                                    private,
                                );
                                let keep = !before.targets.is_empty() || !after.targets.is_empty();
                                Two {
                                    id: ids.stack,
                                    keys: k,
                                    before,
                                    after,
                                    kept: keep.then(|| (s.clone(), private.clone())),
                                }
                            })
                            .collect::<Vec<Two>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a replay thread"))
                .collect()
        });
        two.extend(out.into_iter().flatten());
    }
    tick(&mut seconds, "rules", &mut lap);
    seconds.insert("rules", seconds["rules"] - read_s);
    seconds.insert("read", read_s);
    let at: HashMap<i64, usize> = two.iter().enumerate().map(|(i, t)| (t.id, i)).collect();

    // --- the passes, in the pack's order, each way
    let stored = stored_axes(store, base, &place, &at)?;
    let mut sibling_rows: HashMap<i64, (Stack, Vec<String>)> = HashMap::new();
    for s in 0..2 {
        let mut filled: HashSet<(i64, usize)> = HashSet::new();
        // the stacks a session pass of this run wrote to
        let mut written: HashSet<i64> = HashSet::new();
        let pass_count = base.passes.len();
        // the vote reads the corpus as it stood before any pass of the run
        let pre: Vec<Vec<Vec<String>>> = two
            .iter_mut()
            .map(|t| side(t, s).axes.iter().map(|a| a.values.clone()).collect())
            .collect();
        for pi in 0..pass_count {
            if base.passes[pi].session().is_some() {
                session_pass(
                    store,
                    s,
                    pi,
                    base,
                    sides,
                    &place,
                    &sealed,
                    &mut two,
                    &mut sibling_rows,
                    &mut filled,
                    &mut written,
                )?;
            } else if base.passes[pi].vote().is_some() {
                vote_pass(
                    s, pi, sides, &place, &stored, &pre, &mut two, &filled, store,
                )?;
            }
        }
        // the questions a missing answer asks, once the passes had their say
        for t in two.iter_mut() {
            let pack = &sides[s][t.keys[s]];
            let asked = nils_pack::matters::missing_asked(pack);
            let c = side(t, s);
            if c.silent {
                continue;
            }
            for a in c.unresolved.clone() {
                let name = &pack.axes[a].name;
                // a decision answers the axis, one that it holds nothing too
                if asked.contains(name) && c.axes[a].values.is_empty() && !c.decided.contains(&a) {
                    c.questions.push(format!("{name}:missing"));
                }
            }
        }
    }
    tick(&mut seconds, "passes", &mut lap);

    // --- the settled answers, read now so their stacks keep what they end as
    let settled = settled(store, &two)?;
    let labelled: HashSet<i64> = settled.0.iter().filter_map(|l| l.stack_id).collect();

    // --- what to do with each stack, from what was decided, and what moved
    let mut axes_moves: BTreeMap<String, AxisMoves> = BTreeMap::new();
    let mut questions: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    let (mut asked_before, mut asked_after) = (0i64, 0i64);
    let mut moved_stacks = 0i64;
    let mut finals: HashMap<i64, [BTreeMap<String, String>; 2]> = HashMap::new();
    let mut renamed: Vec<Renamed> = Vec::new();
    let (mut descriptive_changed, mut bids_changed) = (0i64, 0i64);
    // a stack with files in two datasets counts in both, one in none under
    // `(none)`
    let sites_of = |p: &Place| -> Vec<String> {
        if p.sites.is_empty() {
            return vec!["(none)".into()];
        }
        p.sites.iter().map(|i| resolver.sites[*i].clone()).collect()
    };
    let datasets_of = |p: &Place| -> Vec<String> {
        if p.datasets.is_empty() {
            return vec!["(none)".into()];
        }
        p.datasets
            .iter()
            .map(|i| resolver.datasets[*i].0.clone())
            .collect()
    };
    for chunk in in_scope.chunks(CHUNK) {
        let rows = store.query(&select_stacks(store, chunk), &[])?;
        let facts = name_facts(store, chunk)?;
        let mut stacks: Vec<(i64, Stack, Vec<String>)> = Vec::with_capacity(rows.len());
        for r in &rows {
            let (ids, s, private) = to_stack(r, false, base)?;
            stacks.push((ids.stack, s, private));
        }
        let per = stacks.len().div_ceil(workers).max(1);
        type Disposed = (
            i64,
            [BTreeMap<String, String>; 2],
            Option<[(String, Option<String>); 2]>,
        );
        let out: Vec<Vec<Disposed>> = std::thread::scope(|scope| {
            let handles: Vec<_> = stacks
                .chunks(per)
                .map(|part| {
                    let two = &two;
                    let at = &at;
                    let facts = &facts;
                    scope.spawn(move || {
                        part.iter()
                            .map(|(id, s, private)| {
                                let t = &two[at[id]];
                                let mut maps: [BTreeMap<String, String>; 2] = Default::default();
                                for (si, c) in [&t.before, &t.after].into_iter().enumerate() {
                                    let pack = &sides[si][t.keys[si]];
                                    let seed: Vec<Vec<String>> =
                                        c.axes.iter().map(|a| a.values.clone()).collect();
                                    let e = Evaluated::with_private(pack, s, private.clone());
                                    let d = e.dispose(&seed);
                                    let m = &mut maps[si];
                                    for (i, a) in pack.axes.iter().enumerate() {
                                        if a.phase == AxisPhase::Class {
                                            let v = c.axes[i].values.join(",");
                                            if !v.is_empty() {
                                                m.insert(a.name.clone(), v);
                                            }
                                        }
                                    }
                                    for a in &d.axes {
                                        let v = a.stored();
                                        if !v.is_empty() {
                                            m.insert(a.axis.clone(), v);
                                        }
                                    }
                                }
                                // each side's acquisition family, from the
                                // routes its own rules decided by, so a rule
                                // that brings a stack into a family moves its
                                // name although no axis moves
                                // and with the decisions in force, as the
                                // classification stands
                                let family =
                                    |pack: &Pack, c: &Class, m: &BTreeMap<String, String>| {
                                        let decided: crate::family::InForce = c
                                            .decided
                                            .iter()
                                            .map(|i| {
                                                let v = c.axes[*i].values.join(",");
                                                (
                                                    pack.axes[*i].name.clone(),
                                                    (!v.is_empty()).then_some(v),
                                                )
                                            })
                                            .collect();
                                        crate::family::acquisition_family(
                                            Some(pack),
                                            &c.routes,
                                            &decided,
                                            m.get("provenance").map(String::as_str),
                                        )
                                        .map(str::to_string)
                                    };
                                let families = [
                                    family(&sides[0][t.keys[0]], &t.before, &maps[0]),
                                    family(&sides[1][t.keys[1]], &t.after, &maps[1]),
                                ];
                                // a name moves by a word alone too: the
                                // patched pack's word for a value it holds
                                let words = |pack: &Pack, m: &BTreeMap<String, String>| {
                                    m.iter()
                                        .map(|(axis, v)| {
                                            let w = pack
                                                .axes
                                                .iter()
                                                .find(|a| a.name == *axis)
                                                .map_or_else(
                                                    || v.clone(),
                                                    |a| a.words_of_stored(v),
                                                );
                                            (axis.clone(), w)
                                        })
                                        .collect::<BTreeMap<String, String>>()
                                };
                                let moved = maps[0] != maps[1]
                                    || families[0] != families[1]
                                    || words(&sides[0][t.keys[0]], &maps[0])
                                        != words(&sides[1][t.keys[1]], &maps[1]);
                                let names = moved.then(|| {
                                    let f = facts.get(id).cloned().unwrap_or_default();
                                    let [fb, fa] = families.map(|acquisition_family| NameFacts {
                                        acquisition_family,
                                        ..f.clone()
                                    });
                                    [
                                        namer(&sides[0][t.keys[0]], &maps[0], &fb),
                                        namer(&sides[1][t.keys[1]], &maps[1], &fa),
                                    ]
                                });
                                (*id, maps, names)
                            })
                            .collect::<Vec<Disposed>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a replay thread"))
                .collect()
        });
        for (id, maps, names) in out.into_iter().flatten() {
            let t = &two[at[&id]];
            let p = &place[&id];
            // questions: by kind, what appears and what goes
            let qb: BTreeSet<&String> = t.before.questions.iter().collect();
            let qa: BTreeSet<&String> = t.after.questions.iter().collect();
            asked_before += i64::from(!qb.is_empty());
            asked_after += i64::from(!qa.is_empty());
            for q in qa.difference(&qb) {
                questions.entry((*q).clone()).or_default().0 += 1;
            }
            for q in qb.difference(&qa) {
                questions.entry((*q).clone()).or_default().1 += 1;
            }
            // the rules moved an axis a decision holds: no move, said
            for i in t
                .before
                .decided
                .iter()
                .filter(|i| t.after.decided.contains(i))
            {
                if t.before.own[*i] != t.after.own[*i] {
                    axes_moves
                        .entry(base.axes[*i].name.clone())
                        .or_default()
                        .held_by_decision += 1;
                }
            }
            let role = nils_pack::matters::PICK_CANDIDATES;
            let mut rename = |names: Option<[(String, Option<String>); 2]>| {
                if let Some([b, a]) = names {
                    let d = b.0 != a.0;
                    let bi = b.1 != a.1;
                    descriptive_changed += i64::from(d);
                    bids_changed += i64::from(bi);
                    if d || bi {
                        renamed.push(Renamed {
                            stack: id,
                            before: json!({"name": b.0, "bids": b.1}),
                            after: json!({"name": a.0, "bids": a.1}),
                        });
                    }
                }
            };
            if maps[0] == maps[1] {
                // no axis moved, and a name may still have: the family
                rename(names);
                if maps[0].contains_key(role) || labelled.contains(&id) {
                    finals.insert(id, maps);
                }
                continue;
            }
            moved_stacks += 1;
            let names_of: BTreeSet<&String> = maps[0].keys().chain(maps[1].keys()).collect();
            for axis in names_of {
                let from = maps[0].get(axis).cloned().unwrap_or_default();
                let to = maps[1].get(axis).cloned().unwrap_or_default();
                if from == to {
                    continue;
                }
                // in the words a person reads, each side's pack's own: a
                // value whose label changed reads as the label, not the
                // spelling its rows keep (record 55, ruling 23)
                let read =
                    |pack: &Pack, text: String| match pack.axes.iter().find(|a| a.name == *axis) {
                        Some(a) => a.words_of_stored(&text),
                        None => text,
                    };
                let (from, to) = (
                    read(&sides[0][t.keys[0]], from),
                    read(&sides[1][t.keys[1]], to),
                );
                let am = axes_moves.entry(axis.clone()).or_default();
                am.moved += 1;
                let row = am.rows.entry((from, to)).or_default();
                row.stacks += 1;
                for d in datasets_of(p) {
                    *row.datasets.entry(d).or_insert(0) += 1;
                }
                for s in sites_of(p) {
                    *row.sites.entry(s).or_insert(0) += 1;
                }
                let make = if p.make.is_empty() {
                    "(none)".to_string()
                } else {
                    p.make.clone()
                };
                *row.makes.entry(make).or_insert(0) += 1;
                let scanner = if p.station.is_empty() {
                    "(none)".to_string()
                } else {
                    p.station.clone()
                };
                *row.scanners.entry(scanner).or_insert(0) += 1;
                if row.examples.len() < settings.examples {
                    row.examples.push(id);
                }
            }
            rename(names);
            finals.insert(id, maps);
        }
    }
    tick(&mut seconds, "dispose", &mut lap);

    // --- the main scans
    let picks = picks(
        store,
        base,
        &whole.pack,
        &at,
        &finals,
        &place,
        &sealed,
        settings,
    )?;
    tick(&mut seconds, "picks", &mut lap);

    // --- the settled answers
    let answers = answers(base, sides, &two, &at, &finals, settled, settings);
    tick(&mut seconds, "answers", &mut lap);

    // --- the report
    let example_ids: BTreeSet<i64> = axes_moves
        .values()
        .flat_map(|am| am.rows.values().flat_map(|r| r.examples.iter().copied()))
        .collect();
    let names_by: HashMap<i64, &Renamed> = renamed.iter().map(|r| (r.stack, r)).collect();
    let axes_doc: Vec<Value> = base
        .axes
        .iter()
        .filter_map(|a| axes_moves.get(&a.name).map(|m| (a.name.clone(), m)))
        .map(|(axis, m)| {
            let mut rows: Vec<(&(String, String), &Moves)> = m.rows.iter().collect();
            rows.sort_by(|a, b| b.1.stacks.cmp(&a.1.stacks).then(a.0.cmp(b.0)));
            json!({
                "axis": axis,
                "moved": m.moved,
                "held_by_decision": m.held_by_decision,
                "transitions": rows.iter().map(|((from, to), r)| json!({
                    "from": from,
                    "to": to,
                    "stacks": r.stacks,
                    "datasets": r.datasets,
                    "sites": r.sites,
                    "makes": r.makes,
                    "scanners": r.scanners,
                    "examples": r.examples.iter().map(|id| json!({
                        "stack": id,
                        "names": names_by.get(id).map(|n| json!({"before": n.before, "after": n.after})),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut name_examples: Vec<&Renamed> = renamed
        .iter()
        .filter(|r| example_ids.contains(&r.stack))
        .collect();
    for r in &renamed {
        if name_examples.len() >= settings.examples.max(1) * 4 {
            break;
        }
        if !name_examples.iter().any(|x| x.stack == r.stack) {
            name_examples.push(r);
        }
    }
    let ships = if patch.is_pack_edit() {
        let next = patch.ships.clone().unwrap_or_else(|| {
            let mut v = base.version;
            v.patch += 1;
            v.to_string()
        });
        json!({"as": "rules release", "pack": base.name, "version": next, "from": base.version.to_string()})
    } else {
        json!({
            "as": "overlay",
            "pack": base.name,
            "on": base.version.to_string(),
            "scopes": patch.scopes().iter().filter(|s| !s.is_pack()).map(Scope::text).collect::<Vec<_>>(),
            "with_pack_edits": patch.operations.iter().any(|o| o.scope.is_pack()),
        })
    };
    let mut question_rows: Vec<(&String, &(i64, i64))> = questions.iter().collect();
    question_rows.sort_by(|a, b| (b.1.0 + b.1.1).cmp(&(a.1.0 + a.1.1)).then(a.0.cmp(b.0)));
    let total = started.elapsed().as_secs_f64();
    let mut timing: serde_json::Map<String, Value> = seconds
        .iter()
        .map(|(k, v)| (k.to_string(), json!((v * 1000.0).round() / 1000.0)))
        .collect();
    timing.insert("total".into(), json!((total * 1000.0).round() / 1000.0));
    Ok(json!({
        "pack": base.id(),
        "patch": {
            "operations": patch.written(),
            "applied": whole.applied,
            "files": whole.docs.changed(),
            "cases": {
                "held": whole.cases.is_none(),
                "failures": whole.cases.as_ref().map(|e| e.to_string()),
            },
        },
        "ships": ships,
        "scope": {
            "replayed": scope_text,
            "stacks": in_scope.len(),
            "sealed_left_out": sealed.len(),
            "by_operation": patch.operations.iter().zip(&by_op).map(|(o, n)| json!({
                "at": o.at, "op": o.kind, "scope": op_scopes[o.at - 1].text(), "stacks": n,
            })).collect::<Vec<_>>(),
        },
        "replay": "the rules, the decisions in force, the session passes, the physics vote, the questions a sort asks, the disposition and the main-scan picks; each stack sorted after by the pack with the pack-wide operations and those whose scope holds it; the vote's pool and the pick's population outside the scope as the registry holds them",
        "adopted": {
            "changes": adopted.ops.iter().zip(&adopted_scopes).map(|(a, s)| json!({
                "proposal": a.proposal, "at": a.at, "op": a.op.kind, "scope": s.text(),
            })).collect::<Vec<_>>(),
            "skipped": adopted.skipped,
        },
        "moved": {"stacks": moved_stacks},
        "axes": axes_doc,
        "names": {
            "descriptive": {"changed": descriptive_changed},
            "bids": {"changed": bids_changed},
            "examples": name_examples.iter().map(|r| json!({"stack": r.stack, "before": r.before, "after": r.after})).collect::<Vec<_>>(),
        },
        "picks": picks,
        "review": {
            "stacks_asked": {"before": asked_before, "after": asked_after},
            "by_kind": question_rows.iter().map(|(k, (appear, disappear))| json!({
                "kind": k, "appear": appear, "disappear": disappear,
            })).collect::<Vec<_>>(),
        },
        "answers": answers,
        "seconds": timing,
    }))
}

// ---------------------------------------------------------------------------
// The passes.

/// What the registry holds for the stacks outside the scope: every axis a
/// rule or a person decided (the vote's pool), and every axis (the picks').
struct Stored {
    /// stack → axis index → values, without what a pass wrote
    decided: HashMap<i64, Vec<String>>,
    /// stack → axis name → stored, everything
    all: HashMap<i64, BTreeMap<String, String>>,
}

fn stored_axes(
    store: &mut Store,
    pack: &Pack,
    place: &BTreeMap<i64, Place>,
    at: &HashMap<i64, usize>,
) -> Result<Stored, Error> {
    let mut out = Stored {
        decided: HashMap::new(),
        all: HashMap::new(),
    };
    if place.keys().all(|id| at.contains_key(id)) {
        return Ok(out);
    }
    let mut by_a_pass: HashSet<(i64, String)> = HashSet::new();
    for r in store.query(
        &format!(
            "SELECT stack_id, axis FROM {} WHERE pass IS NOT NULL",
            store.qualified("classification_evidence")
        ),
        &[],
    )? {
        by_a_pass.insert((r.int(0)?, r.text(1)?.to_string()));
    }
    for r in store.query(
        &format!(
            "SELECT stack_id, axis, value FROM {} ORDER BY id",
            store.qualified("classification_axis")
        ),
        &[],
    )? {
        let id = r.int(0)?;
        if at.contains_key(&id) || !place.contains_key(&id) {
            continue;
        }
        let axis = r.text(1)?.to_string();
        let Some(v) = r.opt_text(2)?.map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        let all = out.all.entry(id).or_default();
        all.entry(axis.clone())
            .and_modify(|h| {
                h.push(',');
                h.push_str(v);
            })
            .or_insert_with(|| v.to_string());
        if let Some(a) = pack.axis_index(&axis)
            && !by_a_pass.contains(&(id, axis.clone()))
        {
            let slot = &mut out
                .decided
                .entry(id)
                .or_insert_with(|| vec![String::new(); pack.axes.len()])[a];
            if slot.is_empty() {
                *slot = v.to_string();
            } else {
                slot.push(',');
                slot.push_str(v);
            }
        }
    }
    Ok(out)
}

/// The fields of these stacks a session pass reads of a sibling.
fn sibling(
    pack: &Pack,
    session: &nils_pack::session::Session,
    stack: &Stack,
    private: &[String],
) -> (Stack, Vec<String>) {
    let reads = nils_pack::session::sibling_reads(session);
    let mut s = Stack::new();
    let first_private = PACK_FIELDS.len() + pack.derived.len();
    let mut p = vec![String::new(); pack.ingest.len()];
    for f in reads {
        if f < PACK_FIELDS.len() {
            let text = stack.as_text(f).into_owned();
            s.set(PACK_FIELDS[f], FieldValue::Text(Some(&text)))
                .expect("a field of the fingerprint");
        } else if f >= first_private
            && let Some(v) = private.get(f - first_private)
        {
            p[f - first_private] = v.clone();
        }
    }
    (s, p)
}

#[allow(clippy::too_many_arguments)]
fn session_pass(
    store: &mut Store,
    s: usize,
    pi: usize,
    base: &Pack,
    sides: [&[Pack]; 2],
    place: &BTreeMap<i64, Place>,
    sealed: &BTreeSet<i64>,
    two: &mut [Two],
    rows: &mut HashMap<i64, (Stack, Vec<String>)>,
    filled: &mut HashSet<(i64, usize)>,
    written: &mut HashSet<i64>,
) -> Result<(), Error> {
    // the targets: read when the stack was sorted, and read again where an
    // earlier pass of this run wrote to the stack
    let mut targets: Vec<usize> = Vec::new();
    for (i, t) in two.iter().enumerate() {
        let Some((stack, private)) = &t.kept else {
            continue;
        };
        let pack = &sides[s][t.keys[s]];
        let c = if s == 0 { &t.before } else { &t.after };
        let holds = if written.contains(&t.id) {
            nils_pack::session::targets(
                pack,
                pack.passes[pi].target.as_ref(),
                stack,
                private,
                &c.axes,
            )
        } else {
            c.targets.contains(&pi)
        };
        if holds {
            targets.push(i);
        }
    }
    if targets.is_empty() {
        return Ok(());
    }
    // their studies' stacks, read once for every pass and both sides
    let studies: BTreeSet<i64> = targets.iter().map(|i| place[&two[*i].id].study).collect();
    let members: Vec<i64> = place
        .iter()
        .filter(|(id, p)| studies.contains(&p.study) && !sealed.contains(id))
        .map(|(id, _)| *id)
        .collect();
    let missing: Vec<i64> = members
        .iter()
        .copied()
        .filter(|m| !rows.contains_key(m))
        .collect();
    for chunk in missing.chunks(500) {
        for r in store.query(&select_stacks(store, chunk), &[])? {
            let (ids, stack, private) = to_stack(&r, false, base)?;
            rows.insert(ids.stack, (stack, private));
        }
    }
    for i in targets {
        let id = two[i].id;
        let me = &place[&id];
        let pack = &sides[s][two[i].keys[s]];
        let pass = &pack.passes[pi];
        let Some(session) = pass.session() else {
            continue;
        };
        let siblings: Vec<Sib> = members
            .iter()
            .filter(|m| **m != id && place[*m].study == me.study)
            .filter_map(|m| {
                let (stack, private) = rows.get(m)?;
                let (seen, seen_private) = sibling(pack, session, stack, private);
                let p = &place[m];
                Some(Sib {
                    id: *m,
                    stack: seen,
                    private: seen_private,
                    same_series: p.series == me.series,
                    same_frame_of_reference: match (&me.frame, &p.frame) {
                        (Some(a), Some(b)) => Some(a == b),
                        _ => None,
                    },
                })
            })
            .collect();
        let t = &mut two[i];
        let Some((stack, private)) = t.kept.clone() else {
            continue;
        };
        let c = side(t, s);
        let Some(answer) = nils_pack::session::decide(
            pack,
            pass.target.as_ref(),
            session,
            &stack,
            &private,
            &c.axes,
            &siblings,
        ) else {
            continue;
        };
        if answer.held.is_some() {
            continue;
        }
        for (axis, values) in &answer.writes {
            c.axes[*axis] = InForce {
                values: values.clone(),
                tier: TIER.to_string(),
                confidence: answer.confidence,
            };
            c.questions
                .push(format!("{}:session", pack.axes[*axis].name));
            filled.insert((id, *axis));
            written.insert(id);
        }
    }
    Ok(())
}

/// The fields every pass of the pack reads, for the stacks given.
fn corpus_fields(
    store: &mut Store,
    needed: &[usize],
    place: &BTreeMap<i64, Place>,
    modality: &str,
) -> Result<HashMap<i64, Vec<String>>, Error> {
    let mut out = HashMap::new();
    if needed.is_empty() {
        return Ok(out);
    }
    let columns: Vec<String> = std::iter::once("stack_id".to_string())
        .chain(needed.iter().map(|f| field_sql(store, None, FIELDS[*f])))
        .collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE modality = {} ORDER BY stack_id",
        columns.join(", "),
        store.qualified("stack_fingerprint"),
        store.dialect().param(1, Type::Text),
    );
    for r in store.query(&sql, &[Param::from(modality)])? {
        let id = r.int(0)?;
        if !place.contains_key(&id) {
            continue;
        }
        let cells: Vec<String> = (0..needed.len())
            .map(|i| cell_text(r.get(i + 1)).unwrap_or_default())
            .collect();
        out.insert(id, cells);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn vote_pass(
    s: usize,
    pi: usize,
    sides: [&[Pack]; 2],
    place: &BTreeMap<i64, Place>,
    stored: &Stored,
    pre: &[Vec<Vec<String>>],
    two: &mut [Two],
    filled: &HashSet<(i64, usize)>,
    store: &mut Store,
) -> Result<(), Error> {
    let at: HashMap<i64, usize> = two.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    // each pack in play answers for its own stacks: its target and the
    // fields it reads are its own
    let keys: Vec<usize> = two
        .iter()
        .map(|t| t.keys[s])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for key in keys {
        let pack = &sides[s][key];
        let pass = &pack.passes[pi];
        let Some(vote) = pass.vote() else { continue };
        // the corpus: every unsealed stack of the modality, the scope's as
        // the rules and the decisions left them this time, the rest as stored
        let mut corpus = Corpus::new(pack);
        let needed = corpus.needed();
        let fields = corpus_fields(store, &needed, place, &pack.modality)?;
        let empty_axes = vec![String::new(); pack.axes.len()];
        for id in place.keys() {
            let cells = fields.get(id);
            let axes: Vec<String> = match at.get(id) {
                Some(i) => pre[*i].iter().map(|v| v.join(",")).collect(),
                None => stored
                    .decided
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| empty_axes.clone()),
            };
            corpus.push(
                *id,
                |f| {
                    needed
                        .iter()
                        .position(|n| *n == f)
                        .and_then(|i| cells.map(|c| c[i].clone()))
                        .unwrap_or_default()
                },
                |a| axes.get(a).cloned().unwrap_or_default(),
            );
        }
        let (answers, _, _) = nils_pack::pass::run_vote(pack, pass, vote, &corpus, false);
        for a in &answers {
            if a.writes.is_empty() {
                continue;
            }
            let id = corpus.ids[a.at];
            let Some(i) = at.get(&id).copied() else {
                continue;
            };
            if two[i].keys[s] != key {
                continue;
            }
            let confidence = a.outcome.confidence();
            let c = side(&mut two[i], s);
            for (axis, value) in &a.writes {
                let current = corpus.axis_of(a.at, *axis);
                let fills = current.is_empty() || vote.write_when.iter().any(|w| w == current);
                if !fills || filled.contains(&(id, *axis)) {
                    continue;
                }
                c.axes[*axis] = InForce {
                    values: split(value),
                    tier: "vote".into(),
                    confidence,
                };
                if nils_pack::weaker_than(confidence, pass.emit.review_below)
                    || pass.emit.review_all_touched
                {
                    c.questions.push(format!("{}:vote", pack.axes[*axis].name));
                }
            }
        }
    }
    Ok(())
}

/// The facts the names of these stacks are built from.
fn name_facts(store: &mut Store, stacks: &[i64]) -> Result<HashMap<i64, NameFacts>, Error> {
    let mut out = HashMap::new();
    let t = table("stack_fingerprint");
    let d = store.dialect();
    let text = |c: &str| d.text_of_qualified(Some("f"), t.column(c).expect("a column"));
    let head = format!(
        "SELECT f.stack_id, {}, {}, {}, {}, f.dwi_b_value, f.dwi_directions, f.series_number, f.stacks_in_series FROM {} f",
        text("orientation"),
        text("echo_numbers"),
        text("mr_acquisition_type"),
        text("dwi_pe_direction"),
        store.qualified("stack_fingerprint"),
    );
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for r in store.query(&format!("{head} WHERE f.stack_id IN ({list})"), &[])? {
            out.insert(
                r.int(0)?,
                NameFacts {
                    orientation: r.opt_text(1)?.map(str::to_string),
                    echo_numbers: r.opt_text(2)?.map(str::to_string),
                    mr_acquisition_type: r.opt_text(3)?.map(str::to_string),
                    dwi_pe_direction: r.opt_text(4)?.map(str::to_string),
                    dwi_b_value: r.double(5).ok(),
                    dwi_directions: r.opt_int(6)?,
                    series_number: r.opt_int(7)?,
                    stacks_in_series: r.opt_int(8)?,
                    // each side's own, from its verdict's routes
                    acquisition_family: None,
                },
            );
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The main scans.

/// Per (role, subject, session): the stacks the pick chose, and its borders.
type Occasions = BTreeMap<(String, i64, i64), (Option<Vec<i64>>, Vec<String>)>;

/// The main-scan picks both ways: each side by its own pack's pick (the
/// 2026-10-10 study of the pick borders: a change to the pick file shows its
/// effect too), over the axes each side decided, and per role and pick how
/// many picks are kept, changed, removed and added and how many occasions'
/// borders are raised, settled or still there.
#[allow(clippy::too_many_arguments)]
fn picks(
    store: &mut Store,
    base: &Pack,
    after: &Pack,
    at: &HashMap<i64, usize>,
    finals: &HashMap<i64, [BTreeMap<String, String>; 2]>,
    place: &BTreeMap<i64, Place>,
    sealed: &BTreeSet<i64>,
    settings: &Settings,
) -> Result<Value, Error> {
    if base.picks.is_empty() && after.picks.is_empty() {
        return Ok(json!({"models": [], "why": "the pack declares no picks"}));
    }
    let scheme = nils_registry::session::Scheme::default();
    let labels =
        nils_session::labels_by_study(store, &scheme).map_err(|e| Error::Store(e.to_string()))?;
    if labels.is_empty() {
        return Ok(json!({
            "models": [],
            "why": "no session is built under the default scheme: a pick run builds them",
        }));
    }
    // a person's pick that stands on an occasion holds it, as a pick run
    // leaves it standing
    let d = store.dialect();
    let day = d.text_of(
        table("pick")
            .column("session_day")
            .expect("pick.session_day"),
    );
    let mut standing: HashSet<(String, String, i64, String)> = HashSet::new();
    for r in store.query(
        &format!(
            "SELECT model, role, subject_id, {day} FROM {} WHERE author_kind = 'person' AND withdrawn_at IS NULL \
             AND rules_scope_kind IS NULL",
            store.qualified("pick")
        ),
        &[],
    )? {
        standing.insert((
            r.text(0)?.to_string(),
            r.text(1)?.to_string(),
            r.int(2)?,
            r.opt_text(3)?.unwrap_or("").to_string(),
        ));
    }
    // the registry's stored axes for the stacks outside the scope
    let mut outside: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    if place.keys().any(|id| !at.contains_key(id)) {
        for r in store.query(
            &format!(
                "SELECT stack_id, axis, value FROM {} ORDER BY id",
                store.qualified("classification_axis")
            ),
            &[],
        )? {
            let id = r.int(0)?;
            if at.contains_key(&id) || !place.contains_key(&id) {
                continue;
            }
            let Some(v) = r.opt_text(2)?.map(str::trim).filter(|v| !v.is_empty()) else {
                continue;
            };
            outside
                .entry(id)
                .or_default()
                .entry(r.text(1)?.to_string())
                .and_modify(|h| {
                    h.push(',');
                    h.push_str(v);
                })
                .or_insert_with(|| v.to_string());
        }
    }
    let _ = sealed;
    // what a pick reads of a stack beside the model's names, for every stack
    // that holds a role on either side
    let holds_a_role = |a: &BTreeMap<String, String>| {
        a.get(nils_pack::matters::PICK_CANDIDATES)
            .is_some_and(|r| !split(r).is_empty())
    };
    let holding: Vec<i64> = place
        .keys()
        .copied()
        .filter(|id| match finals.get(id) {
            Some(m) => holds_a_role(&m[0]) || holds_a_role(&m[1]),
            None => outside.get(id).is_some_and(holds_a_role),
        })
        .collect();
    let scans = crate::picking::scans(store, &holding)?;
    let mut names: Vec<&str> = base.picks.iter().map(|m| m.name.as_str()).collect();
    for m in &after.picks {
        if !names.contains(&m.name.as_str()) {
            names.push(&m.name);
        }
    }
    let mut models = Vec::new();
    let mut total = Counts::default();
    for name in names {
        // each side by its own pack's pick, where it has one
        let pair = [
            base.picks.iter().find(|m| m.name == name),
            after.picks.iter().find(|m| m.name == name),
        ];
        let mut reads: Vec<String> = pair
            .iter()
            .flatten()
            .flat_map(|m| crate::picking::carried(m))
            .collect();
        reads.sort();
        reads.dedup();
        // the fields either side reads, for every stack
        let fields: Vec<&str> = reads
            .iter()
            .filter(|n| FIELDS.iter().any(|(f, _)| f == n))
            .map(String::as_str)
            .collect();
        let mut field_values: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
        if !fields.is_empty() {
            let columns: Vec<String> = std::iter::once("stack_id".to_string())
                .chain(fields.iter().map(|n| {
                    let f = FIELDS.iter().find(|(x, _)| x == n).expect("a field");
                    field_sql(store, None, *f)
                }))
                .collect();
            let sql = format!(
                "SELECT {} FROM {} ORDER BY stack_id",
                columns.join(", "),
                store.qualified("stack_fingerprint")
            );
            for r in store.query(&sql, &[])? {
                let id = r.int(0)?;
                if !place.contains_key(&id) {
                    continue;
                }
                let mut m = BTreeMap::new();
                for (i, n) in fields.iter().enumerate() {
                    if let Some(v) = cell_text(r.get(i + 1)).filter(|v| !v.is_empty()) {
                        m.insert(n.to_string(), v);
                    }
                }
                field_values.insert(id, m);
            }
        }
        let row_of = |id: i64, axes: &BTreeMap<String, String>| -> Option<crate::picking::Row> {
            let roles: Vec<String> = axes
                .get(nils_pack::matters::PICK_CANDIDATES)
                .map(|r| split(r))
                .unwrap_or_default();
            if roles.is_empty() {
                return None;
            }
            let p = &place[&id];
            let mut values = field_values.get(&id).cloned().unwrap_or_default();
            for n in &reads {
                if let Some(v) = axes.get(n) {
                    values.insert(n.clone(), v.clone());
                }
            }
            let row = crate::picking::Row {
                stack: id,
                subject: p.subject,
                study: p.study,
                values,
                roles,
                scan: Default::default(),
            };
            Some(row.with_scan(scans.get(&id).cloned().unwrap_or_default()))
        };
        // a stack of the scope holding a role on either side kept its final
        // axes; one outside the scope is read as stored
        let mut sides: [Vec<crate::picking::Row>; 2] = Default::default();
        for (si, rows) in sides.iter_mut().enumerate() {
            for id in place.keys() {
                let axes = match finals.get(id) {
                    Some(m) => &m[si],
                    None if at.contains_key(id) => continue,
                    None => match outside.get(id) {
                        Some(a) => a,
                        None => continue,
                    },
                };
                if let Some(r) = row_of(*id, axes) {
                    rows.push(r);
                }
            }
        }
        // the occasions a stack of the scope sits in
        let scoped_studies: BTreeSet<i64> = at.keys().map(|id| place[id].study).collect();
        let mut result: [Occasions; 2] = Default::default();
        for si in 0..2 {
            let Some(model) = pair[si] else {
                continue;
            };
            // record 55, decision 6: a run picks with the rules, the pack's
            // defaults (version 1) in the pack's scope, so the rehearsal
            // does: each subject's visits in order, the kind across them,
            // then the scan within it. An occasion where the rules allow
            // nothing has no pick, as a run writes none.
            let rules = nils_pack::pick_rules::Rules::defaults(model);
            let chooser = nils_pack::pick_rules::Chooser::new(&rules, model);
            let names = crate::picking::carried(model);
            for role in &model.roles {
                let mine = crate::picking::of_role(model, role, &sides[si]);
                let reference = crate::picking::build_reference(model, "registry", &mine);
                let mut by_subject: BTreeMap<
                    i64,
                    BTreeMap<i64, (nils_registry::day::Day, Vec<&crate::picking::Row>)>,
                > = BTreeMap::new();
                for r in &mine {
                    let Some(l) = labels.get(&r.study) else {
                        continue;
                    };
                    by_subject
                        .entry(r.subject)
                        .or_default()
                        .entry(l.session_id)
                        .or_insert_with(|| (l.first, Vec::new()))
                        .1
                        .push(r);
                }
                for (subject, occasions) in by_subject {
                    let in_scope = |rows: &[&crate::picking::Row]| {
                        rows.iter().any(|r| scoped_studies.contains(&r.study))
                    };
                    if !occasions.values().any(|(_, rows)| in_scope(rows)) {
                        continue;
                    }
                    let mut visits: Vec<(i64, nils_registry::day::Day, Vec<&crate::picking::Row>)> =
                        occasions
                            .into_iter()
                            .map(|(s, (d, rows))| (s, d, rows))
                            .collect();
                    visits.sort_by_key(|(_, day, _)| *day);
                    let candidates: Vec<Vec<nils_pack::pick::Candidate>> = visits
                        .iter()
                        .map(|(_, _, rows)| crate::picking::group_with(model, rows, &names))
                        .collect();
                    let chosen = chooser.choose(role, &candidates, Some(&reference));
                    for ((session, _, rows), c) in visits.iter().zip(&chosen) {
                        let Some(w) = &c.winner else {
                            continue;
                        };
                        if !in_scope(rows) {
                            continue;
                        }
                        let mut borders: Vec<String> = Vec::new();
                        if c.decided.is_none()
                            && c.considered.len() >= 2
                            && c.considered[0].1 == c.considered[1].1
                        {
                            borders.push("tie".into());
                        }
                        result[si].insert(
                            (role.clone(), subject, *session),
                            (Some(w.stacks.clone()), borders),
                        );
                    }
                }
            }
        }
        // compare, occasion by occasion
        let day_of: HashMap<i64, String> = labels
            .values()
            .map(|l| (l.session_id, l.first.to_string()))
            .collect();
        let keys: BTreeSet<&(String, i64, i64)> =
            result[0].keys().chain(result[1].keys()).collect();
        let mut by_role: BTreeMap<String, Counts> = BTreeMap::new();
        let mut counts = Counts::default();
        let mut examples = Vec::new();
        for key in keys {
            let (role, subject, session) = key;
            let none = (None, Vec::new());
            let b = result[0].get(key).unwrap_or(&none);
            let a = result[1].get(key).unwrap_or(&none);
            let held = standing.contains(&(
                name.to_string(),
                role.clone(),
                *subject,
                day_of.get(session).cloned().unwrap_or_default(),
            ));
            let one = Counts::of(b, a, held);
            by_role.entry(role.clone()).or_default().add(&one);
            counts.add(&one);
            if !held && (b.0 != a.0 || b.1 != a.1) && examples.len() < settings.examples {
                examples.push(json!({
                    "role": role, "subject": subject, "session": session,
                    "before": b.0, "after": a.0,
                    "borders": {"before": b.1, "after": a.1},
                }));
            }
        }
        total.add(&counts);
        let mut doc = counts.json();
        doc["model"] = json!(name);
        doc["by_role"] = Value::Object(
            by_role
                .iter()
                .map(|(role, c)| (role.clone(), c.json()))
                .collect(),
        );
        doc["examples"] = json!(examples);
        models.push(doc);
    }
    let mut doc = total.json();
    doc["scheme"] = json!("default");
    doc["models"] = json!(models);
    Ok(doc)
}

/// What a pick change did on some occasions: the picks kept, changed,
/// removed and added, and the borders raised, settled and still there.
/// `changed` alone is a pick on both sides naming other stacks; `moved` is
/// every occasion whose pick differs. An occasion a person's pick stands on
/// is held by it: counted there where its pick would differ, and in nothing
/// else.
#[derive(Debug, Clone, Default)]
struct Counts {
    occasions: i64,
    kept: i64,
    changed: i64,
    removed: i64,
    added: i64,
    held_by_a_person: i64,
    raised: i64,
    settled: i64,
    still: i64,
}

impl Counts {
    /// One occasion's: its pick and borders before, after, and whether a
    /// person's pick holds it.
    fn of(
        before: &(Option<Vec<i64>>, Vec<String>),
        after: &(Option<Vec<i64>>, Vec<String>),
        held: bool,
    ) -> Counts {
        let mut c = Counts {
            occasions: 1,
            ..Counts::default()
        };
        if held {
            if before.0 != after.0 {
                c.held_by_a_person = 1;
            }
            return c;
        }
        match (&before.0, &after.0) {
            (Some(x), Some(y)) if x == y => c.kept = 1,
            (Some(_), Some(_)) => c.changed = 1,
            (Some(_), None) => c.removed = 1,
            (None, Some(_)) => c.added = 1,
            (None, None) => {}
        }
        match (before.1.is_empty(), after.1.is_empty()) {
            (true, false) => c.raised = 1,
            (false, true) => c.settled = 1,
            (false, false) => c.still = 1,
            (true, true) => {}
        }
        c
    }

    fn add(&mut self, o: &Counts) {
        self.occasions += o.occasions;
        self.kept += o.kept;
        self.changed += o.changed;
        self.removed += o.removed;
        self.added += o.added;
        self.held_by_a_person += o.held_by_a_person;
        self.raised += o.raised;
        self.settled += o.settled;
        self.still += o.still;
    }

    fn json(&self) -> Value {
        json!({
            "occasions": self.occasions,
            "kept": self.kept,
            "changed": self.changed,
            "removed": self.removed,
            "added": self.added,
            "moved": self.changed + self.removed + self.added,
            "held_by_a_person": self.held_by_a_person,
            "borders": {"raised": self.raised, "settled": self.settled, "still": self.still},
        })
    }
}

// ---------------------------------------------------------------------------
// The answers we know.

/// A value as an identity of its axis, for comparing answers written either
/// way (a decision may name a value by its label, as a row stores it).
fn ids_of(pack: &Pack, axis: &str, values: &[String]) -> BTreeSet<String> {
    let a = pack.axes.iter().find(|x| x.name == axis);
    values
        .iter()
        .map(|v| {
            a.and_then(|a| a.value_named(v).map(|i| a.values[i].id.clone()))
                .unwrap_or_else(|| v.to_lowercase())
        })
        .collect()
}

/// A person's decisions in force on the stacks replayed, as a classify
/// resolves them, and how many were left out for a sealed sample: never one
/// on a stack of a sample sealed now.
fn settled(
    store: &mut Store,
    two: &[Two],
) -> Result<(Vec<nils_registry::labels::Label>, usize), Error> {
    let stacks: Vec<i64> = two.iter().map(|t| t.id).collect();
    if stacks.is_empty() {
        return Ok((Vec::new(), 0));
    }
    nils_registry::labels::training_labels(
        store,
        None,
        Some(&stacks),
        &["person".to_string()],
        None,
    )
    .map_err(|e| Error::Store(e.to_string()))
}

fn answers(
    base: &Pack,
    sides: [&[Pack]; 2],
    two: &[Two],
    at: &HashMap<i64, usize>,
    finals: &HashMap<i64, [BTreeMap<String, String>; 2]>,
    (labels, withheld): (Vec<nils_registry::labels::Label>, usize),
    settings: &Settings,
) -> Value {
    #[derive(Default)]
    struct Tally {
        answers: i64,
        right_to_wrong: i64,
        wrong_to_right: i64,
        unanswered_to_right: i64,
        unanswered_to_wrong: i64,
        right_to_unanswered: i64,
        wrong_to_unanswered: i64,
        wrong_to_wrong: i64,
        unchanged: i64,
    }
    let mut by_axis: BTreeMap<String, Tally> = BTreeMap::new();
    let mut breaks = Vec::new();
    let mut fixes = Vec::new();
    for l in &labels {
        let Some(stack) = l.stack_id else { continue };
        let Some(i) = at.get(&stack).copied() else {
            continue;
        };
        let t = &two[i];
        let Some(ai) = base.axis_index(&l.what) else {
            continue;
        };
        let axis = &base.axes[ai];
        // the rules' own answer: a class axis as the rules decided it before
        // any decision; a disposition axis as it was disposed
        let own = |si: usize| -> Vec<String> {
            if axis.phase == AxisPhase::Class {
                let c = if si == 0 { &t.before } else { &t.after };
                c.own[ai].clone()
            } else {
                finals
                    .get(&stack)
                    .and_then(|m| m[si].get(&axis.name).map(|v| split(v)))
                    .unwrap_or_default()
            }
        };
        let pack_of = |si: usize| &sides[si][t.keys[si]];
        let want = ids_of(base, &axis.name, &split(l.value.as_deref().unwrap_or("")));
        let status = |si: usize| -> &'static str {
            let got = own(si);
            if got.is_empty() {
                if want.is_empty() {
                    "right"
                } else {
                    "unanswered"
                }
            } else if ids_of(pack_of(si), &axis.name, &got) == want {
                "right"
            } else {
                "wrong"
            }
        };
        let (b, a) = (status(0), status(1));
        let tally = by_axis.entry(axis.name.clone()).or_default();
        tally.answers += 1;
        let changed = own(0) != own(1);
        match (b, a) {
            ("right", "wrong") => tally.right_to_wrong += 1,
            ("wrong", "right") => tally.wrong_to_right += 1,
            ("unanswered", "right") => tally.unanswered_to_right += 1,
            ("unanswered", "wrong") => tally.unanswered_to_wrong += 1,
            ("right", "unanswered") => tally.right_to_unanswered += 1,
            ("wrong", "unanswered") => tally.wrong_to_unanswered += 1,
            ("wrong", "wrong") if changed => tally.wrong_to_wrong += 1,
            _ => tally.unchanged += 1,
        }
        let example = || {
            json!({
                "stack": stack,
                "axis": axis.name,
                "answer": l.value,
                "before": own(0).join(","),
                "after": own(1).join(","),
            })
        };
        if b == "right"
            && a != "right"
            && (settings.all_breaks || breaks.len() < settings.examples.max(1) * 4)
        {
            breaks.push(example());
        }
        if b != "right" && a == "right" && fixes.len() < settings.examples.max(1) * 4 {
            fixes.push(example());
        }
    }
    let total = |f: fn(&Tally) -> i64| by_axis.values().map(f).sum::<i64>();
    let fixed = total(|t| t.wrong_to_right + t.unanswered_to_right);
    let broke = total(|t| t.right_to_wrong + t.right_to_unanswered);
    json!({
        "measured_on": "the rules' own answer, before any decision, pass or vote; an axis the rules left empty is unanswered",
        "sets": [{
            "name": "person decisions in force",
            "holds": "the answers people gave: Review answers and a campaign's close, each as a decision",
            "answers": labels.len(),
            "sealed_left_out": withheld,
        }],
        "fixes": fixed,
        "breaks": broke,
        "net": fixed - broke,
        "by_axis": by_axis.iter().map(|(axis, t)| (axis.clone(), json!({
            "answers": t.answers,
            "right_to_wrong": t.right_to_wrong,
            "wrong_to_right": t.wrong_to_right,
            "unanswered_to_right": t.unanswered_to_right,
            "unanswered_to_wrong": t.unanswered_to_wrong,
            "right_to_unanswered": t.right_to_unanswered,
            "wrong_to_unanswered": t.wrong_to_unanswered,
            "wrong_to_wrong": t.wrong_to_wrong,
            "unchanged": t.unchanged,
        }))).collect::<serde_json::Map<String, Value>>(),
        "right_to_wrong": breaks,
        "wrong_to_right": fixes,
    })
}
