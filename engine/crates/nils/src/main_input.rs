// SPDX-License-Identifier: AGPL-3.0-only

//! A run straight over a dataset's or a cohort's main scans by role (record
//! 55, rulings of 2026-10-12, item 6): "run SynthSeg on study-anon's main T1
//! scans", without making a selection first. It is written where a
//! selection is, `main:<role>@dataset:<name>` or `main:<role>@cohort:<name>`,
//! and taken by `nils run --select`, by the pre-flight door's `select` and
//! by a run queued at the jobs door, all through `ask_cli::freeze`.
//!
//! - **Which scans.** The scope's scans as every reader of them counts them
//!   (a dataset's: the stacks its folder holds a file of; a cohort's: every
//!   stack of its open members) that the live pick of their visit for the
//!   role names. The picks are the ones the one resolver says
//!   ([`nils_registry::pick_truth`]): the scope's own once a run of its
//!   saved rules finished over all its subjects, else the pack's, of the
//!   pack's pick for the role under the default scheme, as the Main scans
//!   page shows them. One pick a visit, with every stack it names: the two
//!   echoes of one acquisition are one main scan.
//! - **When.** At the time the run is made, frozen into a handle the run
//!   pins, as a selection is. The handle records where its stacks came from
//!   (its `selection_versions`): the spec, the role, the scope, the picks
//!   read, with the version of the scope's rules, their digest and the run
//!   that made them read ([`Of::record`]), and the visits counted. A run's
//!   bids input is released under those picks again (`nils release
//!   --picks-read`), whatever the scope's rules say by then, so the run
//!   reproduces; the release freezes each stack of the list as the main
//!   scan of its role, beside what it read of the picks of its visits.
//! - **The visits with none.** A visit of the scope with no main scan of the
//!   role is counted and named, with why: it holds no scan of the role; it
//!   holds one and none is picked; a person said none stands for it; or, of
//!   a dataset, its main scan is one only another dataset's folder holds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use nils_ask::ast::Grain;
use nils_registry::Registry;
use nils_registry::home::Home;
use nils_registry::pick_rules::{Scope, ScopeKind};
use nils_registry::pick_truth::Of;
use nils_registry::schema::{Type, table};
use nils_registry::session::Scheme;
use nils_registry::store::{Cell, Param, Row, Store};
use serde_json::{Value, json};

use crate::ask_cli::{Freeze, Frozen};
use crate::{Exit, fail, usage};

/// What a scope's main scans are written with.
pub(crate) const PREFIX: &str = "main:";

/// How many of the visits with no main scan a pre-flight names; the rest
/// are counted.
const NAMED_MOST: usize = 200;

/// `main:<role>@dataset:<name>` or `main:<role>@cohort:<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spec {
    pub(crate) role: String,
    pub(crate) kind: ScopeKind,
    pub(crate) name: String,
}

impl Spec {
    /// The scope's main scans a text names; none where it does not start
    /// with `main:`, and why where it starts so and is not one.
    pub(crate) fn parse(text: &str) -> Option<Result<Spec, String>> {
        let rest = text.trim().strip_prefix(PREFIX)?;
        let form = || {
            format!(
                "{}: a dataset's or a cohort's main scans are main:<role>@dataset:<name> or main:<role>@cohort:<name>",
                text.trim()
            )
        };
        Some((|| {
            let (role, scope) = rest.split_once('@').ok_or_else(form)?;
            let (kind, name) = scope.split_once(':').ok_or_else(form)?;
            let kind = ScopeKind::parse(&kind.trim().to_ascii_lowercase()).ok_or_else(form)?;
            let role = role.trim().to_ascii_lowercase();
            let name = name.trim();
            if role.is_empty() || name.is_empty() {
                return Err(form());
            }
            Ok(Spec {
                role,
                kind,
                name: name.to_string(),
            })
        })())
    }

    /// As it is written.
    pub(crate) fn text(&self) -> String {
        format!("{PREFIX}{}@{}:{}", self.role, self.kind.name(), self.name)
    }
}

/// Why a visit of the scope has no main scan of the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Why {
    /// None of its scans in the scope holds the role.
    NoScan,
    /// It holds a scan of the role, and no pick names one.
    Unpicked,
    /// A person's pick says no scan stands for the role on it.
    Person,
    /// Its pick names a scan the dataset's folder holds no file of.
    Elsewhere,
}

impl Why {
    pub(crate) fn words(self, role: &str) -> String {
        match self {
            Why::NoScan => format!("it holds no {role} scan"),
            Why::Unpicked => format!("it holds a {role} scan and none is picked"),
            Why::Person => format!("a person said no {role} stands for it"),
            Why::Elsewhere => format!("its main {role} is a scan only another dataset holds"),
        }
    }
}

/// What a visit's pick says of it: the stacks the live pick names, and
/// whether a person made it; none where no live pick of the role is on it.
type Picked<'a> = Option<(&'a [i64], bool)>;

/// One visit: the stacks of its main scan of the role among the scope's, or
/// why it has none. `in_scope` says whether the scope holds a stack;
/// `holds_role` whether one of the visit's stacks in the scope holds the role.
fn judge(
    picked: Picked<'_>,
    in_scope: impl Fn(i64) -> bool,
    holds_role: bool,
) -> Result<Vec<i64>, Why> {
    match picked {
        Some((stacks, _)) if !stacks.is_empty() => {
            let mine: Vec<i64> = stacks.iter().copied().filter(|s| in_scope(*s)).collect();
            if mine.is_empty() {
                Err(Why::Elsewhere)
            } else {
                Ok(mine)
            }
        }
        Some((_, true)) => Err(Why::Person),
        _ if holds_role => Err(Why::Unpicked),
        _ => Err(Why::NoScan),
    }
}

/// A visit of the scope with no main scan of the role.
#[derive(Debug, Clone)]
pub(crate) struct Without {
    pub(crate) subject_id: i64,
    pub(crate) subject: String,
    pub(crate) day: String,
    pub(crate) why: Why,
}

/// A scope's main scans for a role, resolved now.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    /// As written, with the scope's own name.
    pub(crate) spec: Spec,
    pub(crate) scope: Scope,
    /// The picks read, pinned to the version read now.
    pub(crate) of: Of,
    /// What a release records of them ([`Of::record`]).
    pub(crate) picks: Value,
    pub(crate) pack_version: String,
    /// The main scans, each with its subject, in stack order.
    pub(crate) stacks: Vec<(i64, i64)>,
    /// The scope's visits, and how many have a main scan of the role.
    pub(crate) visits: usize,
    pub(crate) main_visits: usize,
    pub(crate) without: Vec<Without>,
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn said(e: impl std::fmt::Display) -> Exit {
    fail(e.to_string())
}

/// Resolve a scope's main scans for a role now, as a run takes them.
pub(crate) fn resolve(
    registry: &mut Registry,
    pack: &nils_pack::Pack,
    spec: &Spec,
) -> Result<Resolved, Exit> {
    let role = spec.role.as_str();
    let model = pack
        .picks
        .iter()
        .find(|m| m.roles.iter().any(|r| r == role))
        .ok_or_else(|| {
            let roles: Vec<&str> = pack
                .picks
                .iter()
                .flat_map(|m| m.roles.iter().map(String::as_str))
                .collect();
            usage(if roles.is_empty() {
                format!("{} declares no picks, so it has no main scans", pack.id())
            } else {
                format!(
                    "{role} is not a main-scan role of {}: {}",
                    pack.id(),
                    roles.join(", ")
                )
            })
        })?;
    let store = registry.store();
    let scope = match spec.kind {
        ScopeKind::Dataset => Scope::dataset(store, &spec.name)
            .map_err(said)?
            .ok_or_else(|| usage(format!("no dataset named {}", spec.name)))?,
        ScopeKind::Cohort => Scope::cohort(store, &spec.name)
            .map_err(said)?
            .ok_or_else(|| usage(format!("no cohort named {}", spec.name)))?,
    };
    // a root holds datasets and is none itself (Wave 7a): each folder under
    // it is a dataset of its own
    if scope.kind == ScopeKind::Dataset
        && nils_registry::place::show(store, scope.id)
            .map_err(said)?
            .is_some_and(|p| nils_registry::place::is_root(&p.dataset))
    {
        return Err(usage(format!(
            "{} is a root, not a dataset: each folder under it is a dataset of its own",
            scope.name
        )));
    }
    // the picks read now, pinned: the scope's own at the version read, or
    // the pack's of its subjects
    let of = Of::scope(store, &scope)
        .and_then(|o| o.pinned(store))
        .map_err(said)?;
    let pack_version = pack.version.to_string();
    let picks = of.record(store, &pack.name, &pack_version).map_err(said)?;
    let sources = match &of {
        Of::Scope { sources, .. } => sources.clone(),
        _ => Vec::new(),
    };

    // each study's visit under the default scheme, as the picks name them
    let scheme = Scheme::default();
    let anchors =
        nils_session::Anchors::resolve(registry, &scheme, BTreeMap::new()).map_err(said)?;
    nils_session::ensure(registry, &scheme, &anchors, None, false).map_err(said)?;
    let labels = nils_session::labels_by_study(registry.store(), &scheme).map_err(said)?;
    let store = registry.store();
    let d = store.dialect();

    // the scope's scans, by visit
    let ids = list(&sources);
    let held = match scope.kind {
        ScopeKind::Dataset => crate::operations::Scope::Sources(&ids),
        ScopeKind::Cohort => crate::operations::Scope::Cohort(scope.id),
    };
    let holds = held.holds(store);
    let (stack, series) = (store.qualified("stack"), store.qualified("series"));
    let sql = format!(
        "SELECT x.id, xse.study_id FROM {stack} x JOIN {series} xse ON xse.id = x.series_id \
         WHERE {holds} ORDER BY x.id"
    );
    let mut in_scope: BTreeSet<i64> = BTreeSet::new();
    let mut visits: BTreeMap<(i64, String), Vec<i64>> = BTreeMap::new();
    for r in store.query(&sql, &[]).map_err(said)? {
        let stack = r.int(0).map_err(said)?;
        in_scope.insert(stack);
        if let Some(l) = labels.get(&r.int(1).map_err(said)?) {
            visits
                .entry((l.subject_id, l.first.to_string()))
                .or_default()
                .push(stack);
        }
    }

    // which of them hold the role
    let sql = format!(
        "SELECT DISTINCT a.stack_id FROM {} a JOIN {stack} x ON x.id = a.stack_id \
         WHERE a.axis = 'role' AND a.value = {} AND {holds}",
        store.qualified("classification_axis"),
        d.param(1, Type::Text),
    );
    let mut holding: BTreeSet<i64> = BTreeSet::new();
    for r in store.query(&sql, &[Param::from(role)]).map_err(said)? {
        holding.insert(r.int(0).map_err(said)?);
    }

    // the live picks of the role on the scope's subjects, as the resolver
    // reads them, each with the stacks it names
    let relation = of.picks(store);
    let day = d.text_of_qualified(
        Some("p"),
        table("pick")
            .column("session_day")
            .expect("pick.session_day"),
    );
    let sql = format!(
        "SELECT p.id, p.subject_id, {day}, p.author_kind, ps.stack_id FROM {relation} p \
         LEFT JOIN {} ps ON ps.pick_id = p.id \
         WHERE p.role = {} AND p.model = {} AND p.scheme_digest = {} \
         AND p.subject_id IN (SELECT xse.subject_id FROM {stack} x \
         JOIN {series} xse ON xse.id = x.series_id WHERE {holds}) \
         ORDER BY p.id, ps.stack_id",
        store.qualified("pick_stack"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        d.param(3, Type::Text),
    );
    let params = [
        Param::from(role),
        Param::from(model.name.as_str()),
        Param::from(scheme.digest()),
    ];
    let mut found: Vec<((i64, String), i64, bool)> = Vec::new();
    let mut named: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for r in store.query(&sql, &params).map_err(said)? {
        let id = r.int(0).map_err(said)?;
        if found.last().is_none_or(|(_, last, _)| *last != id) {
            let day = r.text(2).map_err(said)?;
            found.push((
                (
                    r.int(1).map_err(said)?,
                    day[..day.len().min(10)].to_string(),
                ),
                id,
                r.text(3).map_err(said)? == "person",
            ));
        }
        if let Some(s) = r.opt_int(4).map_err(said)? {
            named.entry(id).or_default().push(s);
        }
    }
    let mut on: BTreeMap<(i64, String), (i64, bool)> = BTreeMap::new();
    for (key, id, person) in found {
        match on.get(&key) {
            Some((_, true)) if !person => {}
            _ => {
                on.insert(key, (id, person));
            }
        }
    }

    // each visit: its main scan, or why it has none
    let empty: Vec<i64> = Vec::new();
    let mut stacks: Vec<(i64, i64)> = Vec::new();
    let mut without: Vec<Without> = Vec::new();
    let mut main_visits = 0usize;
    for ((subject, day), here) in &visits {
        let picked = on
            .get(&(*subject, day.clone()))
            .map(|(id, person)| (named.get(id).unwrap_or(&empty).as_slice(), *person));
        let holds_role = here.iter().any(|s| holding.contains(s));
        match judge(picked, |s| in_scope.contains(&s), holds_role) {
            Ok(mine) => {
                main_visits += 1;
                stacks.extend(mine.into_iter().map(|s| (s, *subject)));
            }
            Err(why) => without.push(Without {
                subject_id: *subject,
                subject: String::new(),
                day: day.clone(),
                why,
            }),
        }
    }
    stacks.sort_unstable();
    stacks.dedup();
    // the subjects' codes, for the visits a pre-flight names
    let theirs: Vec<i64> = without
        .iter()
        .map(|w| w.subject_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut codes: BTreeMap<i64, String> = BTreeMap::new();
    for chunk in theirs.chunks(500) {
        let sql = format!(
            "SELECT id, code FROM {} WHERE id IN ({})",
            store.qualified("subject"),
            list(chunk)
        );
        for r in store.query(&sql, &[]).map_err(said)? {
            codes.insert(
                r.int(0).map_err(said)?,
                r.text(1).map_err(said)?.to_string(),
            );
        }
    }
    for w in &mut without {
        w.subject = codes.get(&w.subject_id).cloned().unwrap_or_default();
    }
    Ok(Resolved {
        spec: Spec {
            name: scope.name.clone(),
            ..spec.clone()
        },
        scope,
        of,
        picks,
        pack_version,
        stacks,
        visits: visits.len(),
        main_visits,
        without,
    })
}

impl Resolved {
    /// The visits with none, counted by why, the most first.
    fn why_counts(&self) -> Value {
        let mut by: BTreeMap<Why, usize> = BTreeMap::new();
        for w in &self.without {
            *by.entry(w.why).or_default() += 1;
        }
        let mut out: Vec<(Why, usize)> = by.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        json!(
            out.iter()
                .map(|(why, n)| json!({"why": why.words(&self.spec.role), "visits": n}))
                .collect::<Vec<_>>()
        )
    }

    /// What the frozen list records of where it came from, in the handle's
    /// `selection_versions`: the spec, the role, the scope, the picks read
    /// (the rules' version, digest and run, or the pack's defaults), the
    /// visits and the visits with none by why. No subject, no day.
    pub(crate) fn entry(&self) -> Value {
        json!({
            "main": self.spec.role,
            "of": self.spec.text(),
            "scope": self.scope.to_json(),
            "picks": self.picks,
            "read": self.of.describe(),
            "visits": self.visits,
            "main_visits": self.main_visits,
            "stacks": self.stacks.len(),
            "without": {"visits": self.without.len(), "why": self.why_counts()},
        })
    }

    /// The pre-flight's `main`: the entry, with the visits with none named
    /// (their subject and day, at most [`NAMED_MOST`]).
    pub(crate) fn preflight(&self) -> Value {
        let mut doc = self.entry();
        doc["without"]["named"] = json!(
            self.without
                .iter()
                .take(NAMED_MOST)
                .map(|w| json!({
                    "subject_id": w.subject_id, "subject": w.subject,
                    "session_day": w.day, "why": w.why.words(&self.spec.role),
                }))
                .collect::<Vec<_>>()
        );
        doc
    }
}

/// The main-scans entry among a handle's pins, where its stacks were a
/// scope's main scans.
pub(crate) fn entry_in(pins: &Value) -> Option<&Value> {
    pins.as_array()?
        .iter()
        .find(|p| p["main"].is_string() && p["picks"].is_object())
}

/// The main-scans entry a handle records, where it was frozen from a scope's
/// main scans.
pub(crate) fn entry_of(store: &mut Store, handle: i64) -> Option<Value> {
    let h = nils_ask::handle::get(store, handle).ok().flatten()?;
    entry_in(&h.selection_versions).cloned()
}

/// The picks a handle of a scope's main scans was resolved from, as a
/// release records them, with `main`, the role its stacks are the main
/// scans of: what a run over it releases its input under, and what the
/// release freezes each of its stacks as (record 55, rulings of
/// 2026-10-12, item 6, carried to releases).
pub(crate) fn reading_of(store: &mut Store, handle: i64) -> Option<Value> {
    entry_of(store, handle).map(|e| {
        let mut picks = e["picks"].clone();
        if picks.is_object() {
            picks["main"] = e["main"].clone();
        }
        picks
    })
}

/// A main-scans entry in a line: what was read, and the visits with none.
pub(crate) fn line(doc: &Value) -> String {
    let role = doc["main"].as_str().unwrap_or("main");
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let mut out = format!(
        "{}: {} {role} scans of {} visits",
        doc["read"].as_str().unwrap_or_default(),
        n(&doc["stacks"]),
        n(&doc["main_visits"])
    );
    let none = n(&doc["without"]["visits"]);
    if none > 0 {
        let why: Vec<String> = doc["without"]["why"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                format!(
                    "{}: {}",
                    n(&w["visits"]),
                    w["why"].as_str().unwrap_or_default()
                )
            })
            .collect();
        out.push_str(&format!(
            "; {none} {} no main {role} ({})",
            if none == 1 {
                "visit has"
            } else {
                "visits have"
            },
            why.join("; ")
        ));
    }
    out
}

/// Why a scope's main scans hold nothing to run over.
pub(crate) fn nothing(doc: &Value) -> String {
    let role = doc["main"].as_str().unwrap_or("main");
    let visits = doc["visits"].as_u64().unwrap_or(0);
    let scope = doc["of"].as_str().unwrap_or("the scope");
    if visits == 0 {
        return format!("{scope} holds no visit, so there is no main {role} scan to run over");
    }
    format!("{scope} has no main {role} scan to run over: {}", line(doc))
}

/// Save stacks with their subjects as a handle a run pins, recording the
/// entry they came from.
fn save(
    store: &mut Store,
    r: &Resolved,
    stacks: &[(i64, i64)],
    part: Option<(usize, usize)>,
    epoch: i64,
) -> Result<Frozen, Exit> {
    let mut entry = r.entry();
    if let Some((i, of)) = part {
        entry["part"] = json!({"part": i, "of": of});
    }
    let rows: Vec<Row> = stacks
        .iter()
        .map(|(s, subject)| Row(vec![Cell::Int(*s), Cell::Int(*subject)]))
        .collect();
    let scheme = Scheme::default().digest();
    let (handle, keys) = crate::operations::save_stacks(
        store,
        rows,
        &crate::ask_cli::principal(),
        epoch,
        &crate::operations::Pinned {
            versions: json!([entry]),
            pack_version: Some(&r.pack_version),
            scheme_digest: Some(&scheme),
        },
    )
    .map_err(fail)?;
    Ok(Frozen {
        handle,
        keys,
        last: stacks.last().map(|(s, _)| *s),
    })
}

/// The pack and the registry a freeze reads, and the main scans resolved.
fn resolve_at(
    home: &Home,
    spec: &Spec,
    pack_dir: Option<PathBuf>,
    pack_name: &str,
) -> Result<(Resolved, Registry), Exit> {
    let dir = crate::pack_dir(home, pack_dir).map_err(|_| {
        usage("no packs here: pass --pack-dir DIR to read a dataset's or a cohort's main scans")
    })?;
    let pack = crate::ask_cli::load_pack(&dir, pack_name)?;
    let mut registry = crate::open(home)?;
    let r = resolve(&mut registry, &pack, spec)?;
    Ok((r, registry))
}

/// `ask_cli::freeze` of a scope's main scans: resolved now and frozen into
/// one handle, whole, or as one page in stack order (the stacks after
/// `after`, at most `limit`). A campaign or a label set is made from a
/// saved selection, so a freeze under the ask's caps is refused.
pub(crate) fn freeze(
    home: &Home,
    spec: &Spec,
    want: Grain,
    pack_dir: Option<PathBuf>,
    pack_name: &str,
    how: Freeze,
) -> Result<(Frozen, Resolved), Exit> {
    if want != Grain::Stack {
        return Err(usage(format!(
            "{} names stacks, the main scans of each visit; a set of {}s is not made from it",
            spec.text(),
            want.name()
        )));
    }
    if matches!(how, Freeze::Answer) {
        return Err(usage(format!(
            "{} is a run's input, frozen when the run is made; a campaign or a label set is made from a saved selection",
            spec.text()
        )));
    }
    let (r, mut registry) = resolve_at(home, spec, pack_dir, pack_name)?;
    let stacks: Vec<(i64, i64)> = match how {
        Freeze::Page { after, limit } => r
            .stacks
            .iter()
            .filter(|(s, _)| after.is_none_or(|a| *s > a))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .copied()
            .collect(),
        _ => r.stacks.clone(),
    };
    let epoch = registry.meta().epoch;
    let frozen = save(registry.store(), &r, &stacks, None, epoch)?;
    Ok((frozen, r))
}

/// A run's `--select main:...`: resolved once, and frozen whole, or with
/// `--chunk` in parts of at most that many stacks in stack order, each a
/// handle of its own that records the same entry and its part. Refused
/// where there is no main scan to run over, with the visits that have none.
pub(crate) fn freeze_parts(
    home: &Home,
    spec: &Spec,
    pack_dir: Option<PathBuf>,
    pack_name: &str,
    chunk: Option<u64>,
) -> Result<Vec<i64>, Exit> {
    if chunk == Some(0) {
        return Err(usage("--chunk is a number of stacks, at least 1"));
    }
    let (r, mut registry) = resolve_at(home, spec, pack_dir, pack_name)?;
    if r.stacks.is_empty() {
        return Err(usage(nothing(&r.entry())));
    }
    let size = chunk.map_or(r.stacks.len(), |c| usize::try_from(c).unwrap_or(usize::MAX));
    let parts: Vec<&[(i64, i64)]> = r.stacks.chunks(size.max(1)).collect();
    let epoch = registry.meta().epoch;
    let mut out = Vec::with_capacity(parts.len());
    for (i, stacks) in parts.iter().enumerate() {
        let part = (parts.len() > 1).then_some((i + 1, parts.len()));
        out.push(save(registry.store(), &r, stacks, part, epoch)?.handle);
    }
    if out.len() > 1 {
        eprintln!(
            "{}: {} stack(s) in {} parts of at most {size}, handles {}",
            r.spec.text(),
            r.stacks.len(),
            out.len(),
            list(&out)
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_names_a_role_and_a_dataset_or_a_cohort() {
        let s = Spec::parse("main:T1w@dataset:study-anon").unwrap().unwrap();
        assert_eq!(
            s,
            Spec {
                role: "t1w".into(),
                kind: ScopeKind::Dataset,
                name: "study-anon".into()
            }
        );
        assert_eq!(s.text(), "main:t1w@dataset:study-anon");
        let c = Spec::parse(" main:flair@cohort:ms 2026 ").unwrap().unwrap();
        assert_eq!((c.kind, c.name.as_str()), (ScopeKind::Cohort, "ms 2026"));
        // a name may hold what the spec's own words are written with
        let odd = Spec::parse("main:t2w@dataset:a@b:c").unwrap().unwrap();
        assert_eq!(odd.name, "a@b:c");
        // not a scope's main scans at all: a selection, a handle
        assert!(Spec::parse("selection:x@2").is_none());
        assert!(Spec::parse("x@2").is_none());
        assert!(Spec::parse("handle:7").is_none());
        assert!(Spec::parse(" main:t1w@cohort:x").is_some_and(|s| s.is_ok()));
        for bad in [
            "main:t1w",
            "main:t1w@study-anon",
            "main:t1w@datasets:x",
            "main:@dataset:x",
            "main:t1w@cohort:",
        ] {
            let e = Spec::parse(bad).unwrap().unwrap_err();
            assert!(e.contains("main:<role>@dataset:<name>"), "{bad}: {e}");
        }
    }

    #[test]
    fn a_visit_is_its_pick_among_the_scope_s_scans_or_says_why_not() {
        let scope = |s: i64| s < 100;
        // a run's pick, and a person's, of scans the scope holds
        assert_eq!(judge(Some((&[3, 4], false)), scope, true), Ok(vec![3, 4]));
        assert_eq!(judge(Some((&[5], true)), scope, true), Ok(vec![5]));
        // of a dataset: only the scans its folder holds
        assert_eq!(judge(Some((&[5, 105], false)), scope, true), Ok(vec![5]));
        assert_eq!(
            judge(Some((&[105], false)), scope, true),
            Err(Why::Elsewhere)
        );
        // a person's pick of no scan says none stands for the role
        assert_eq!(judge(Some((&[], true)), scope, true), Err(Why::Person));
        // no pick: a scan of the role is there, or none is
        assert_eq!(judge(None, scope, true), Err(Why::Unpicked));
        assert_eq!(judge(None, scope, false), Err(Why::NoScan));
        assert_eq!(judge(Some((&[], false)), scope, false), Err(Why::NoScan));
        assert_eq!(
            Why::Unpicked.words("t1w"),
            "it holds a t1w scan and none is picked"
        );
    }

    #[test]
    fn a_handle_s_entry_is_found_among_its_pins_and_said_in_a_line() {
        let entry = json!({
            "main": "t1w", "of": "main:t1w@dataset:a", "picks": {"scope": null, "defaults": true},
            "read": "the pack's main scans of dataset a", "visits": 4, "main_visits": 3, "stacks": 3,
            "without": {"visits": 1, "why": [{"why": "it holds no t1w scan", "visits": 1}]},
        });
        let pins = json!([{"set": "start", "selection": "x", "version": 2}, entry]);
        assert_eq!(entry_in(&pins), Some(&entry));
        assert_eq!(entry_in(&json!([{"set": "start"}])), None);
        assert_eq!(entry_in(&Value::Null), None);
        assert_eq!(
            line(&entry),
            "the pack's main scans of dataset a: 3 t1w scans of 3 visits; 1 visit has no main t1w (1: it holds no t1w scan)"
        );
        let none = json!({"main": "t1w", "of": "main:t1w@cohort:c", "visits": 0});
        assert!(
            nothing(&none).contains("holds no visit"),
            "{}",
            nothing(&none)
        );
    }
}
