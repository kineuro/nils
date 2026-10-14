// SPDX-License-Identifier: AGPL-3.0-only

//! Where a rule change reaches, and the pack's adopted scoped changes
//! (record 56, sections 5.2, 5.6 and 6.5).
//!
//! A typed operation is scoped to the whole pack or to a site, a dataset or
//! a scanner (`nils_pack::patch::Scope`). This module answers, from what the
//! registry holds, which stacks a scope holds, one way for everything that
//! asks: the rehearsal ([`crate::effect`]), a scoped re-sort and the sort
//! that applies an adopted scoped change.
//!
//! - **A dataset** holds the stacks its tree holds a file of (Nima,
//!   2026-10-12: a dataset is a folder of stacks; a stack with files in two
//!   datasets is in both), as `source_stack` records them, else the stack's
//!   first batch's source.
//! - **A site** holds the stacks of the datasets under its root.
//! - **A scanner** holds the stacks whose manufacturer, model or station is
//!   the one named (each key named, case and surrounding spaces aside), and
//!   a batch the stacks it first brought.
//!
//! **An adopted scoped change** is a proposal a person adopted
//! (`nils_registry::rule_proposal`): every sort applies each of its
//! operations to the stacks its scope holds and to no other, so an overlay
//! never leaves its scope (section 5.6). [`adopted`] reads them in the
//! order they were adopted, each checked to apply to the pack as it is; one
//! that no longer applies (the pack moved under it) is left out and said.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use nils_pack::Pack;
use nils_pack::patch::{Op, Patch, Scope};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Param, Store};
use serde_json::{Value, json};

/// Why a scope could not be read: it names what the registry does not hold
/// (the caller's to mend), or the registry failed.
#[derive(Debug)]
pub enum Error {
    Refused(String),
    Store(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(m) | Error::Store(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<nils_registry::store::Error> for Error {
    fn from(e: nils_registry::store::Error) -> Error {
        Error::Store(e.to_string())
    }
}

/// One stack of the modality, and where it sits.
#[derive(Debug, Clone, Default)]
pub(crate) struct Where {
    pub series: i64,
    pub subject: i64,
    pub study: i64,
    pub batch: i64,
    pub frame: Option<String>,
    /// The datasets that hold it, by their place in [`Resolver::datasets`].
    pub datasets: Vec<usize>,
    /// The sites those datasets are under, by their place in
    /// [`Resolver::sites`].
    pub sites: Vec<usize>,
    pub make: String,
    pub model: String,
    pub station: String,
}

/// The registry's datasets, sites and stacks, read once, for answering what
/// a scope holds.
pub struct Resolver {
    /// Each active dataset: its name and its site's name.
    pub(crate) datasets: Vec<(String, Option<String>)>,
    pub(crate) sites: Vec<String>,
    /// Every stack of the modality, by id.
    pub(crate) at: BTreeMap<i64, Where>,
}

impl Resolver {
    /// What the registry holds of the stacks of one modality.
    pub fn load(store: &mut Store, modality: &str) -> Result<Resolver, Error> {
        let places: Vec<nils_registry::place::Place> = nils_registry::place::list(store)?
            .into_iter()
            .filter(|p| p.role == nils_registry::place::Role::Source && p.retired_at.is_none())
            .collect();
        let is_root = |p: &nils_registry::place::Place| p.dataset["kind"] == "root";
        let sites: Vec<String> = places
            .iter()
            .filter(|p| is_root(p))
            .map(|p| p.name.clone())
            .collect();
        // a source row is of one dataset at most, as the ask reads it
        let owned = nils_registry::place::dataset_sources(store)?;
        let mut datasets = Vec::with_capacity(owned.len());
        let mut by_source: HashMap<i64, usize> = HashMap::new();
        for (i, d) in owned.iter().enumerate() {
            let site = places.iter().find(|p| p.id == d.id).and_then(|p| {
                p.dataset["root"].as_str().map(str::to_string).or_else(|| {
                    places
                        .iter()
                        .filter(|r| is_root(r) && r.holds_path(Path::new(&p.path)))
                        .map(|r| r.name.clone())
                        .next()
                })
            });
            datasets.push((d.name.clone(), site));
            for s in &d.sources {
                by_source.insert(*s, i);
            }
        }
        // the datasets that hold each stack: every source with a file of it
        let mut held: HashMap<i64, BTreeSet<usize>> = HashMap::new();
        for r in store.query(
            &format!(
                "SELECT stack_id, source_id FROM {}",
                store.qualified("source_stack")
            ),
            &[],
        )? {
            if let Some(i) = by_source.get(&r.int(1)?) {
                held.entry(r.int(0)?).or_default().insert(*i);
            }
        }
        let d = store.dialect();
        let fp = table("stack_fingerprint");
        let text = |c: &str| d.text_of_qualified(Some("f"), fp.column(c).expect("a column"));
        let sql = format!(
            "SELECT f.stack_id, f.series_id, f.subject_id, f.study_id, k.first_batch_id, b.source_id, \
             {}, {}, {}, s.frame_of_reference_uid FROM {} f JOIN {} k ON k.id = f.stack_id \
             JOIN {} s ON s.id = f.series_id LEFT JOIN {} b ON b.id = k.first_batch_id \
             WHERE f.modality = {} ORDER BY f.stack_id",
            text("manufacturer"),
            text("manufacturer_model_name"),
            text("station_name"),
            store.qualified("stack_fingerprint"),
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("ingest_batch"),
            d.param(1, Type::Text),
        );
        let mut at = BTreeMap::new();
        for r in store.query(&sql, &[Param::from(modality)])? {
            let id = r.int(0)?;
            let of: Vec<usize> = match held.get(&id) {
                Some(s) => s.iter().copied().collect(),
                // a stack no source records holding: its first batch's
                None => r
                    .opt_int(5)?
                    .and_then(|s| by_source.get(&s).copied())
                    .into_iter()
                    .collect(),
            };
            let mut site_of: Vec<usize> = of
                .iter()
                .filter_map(|i| datasets[*i].1.as_ref())
                .filter_map(|s| sites.iter().position(|x| x == s))
                .collect();
            site_of.sort_unstable();
            site_of.dedup();
            at.insert(
                id,
                Where {
                    series: r.opt_int(1)?.unwrap_or(0),
                    subject: r.opt_int(2)?.unwrap_or(0),
                    study: r.opt_int(3)?.unwrap_or(0),
                    batch: r.opt_int(4)?.unwrap_or(0),
                    frame: r.opt_text(9)?.filter(|f| !f.is_empty()).map(str::to_string),
                    datasets: of,
                    sites: site_of,
                    make: r.opt_text(6)?.unwrap_or("").trim().to_string(),
                    model: r.opt_text(7)?.unwrap_or("").trim().to_string(),
                    station: r.opt_text(8)?.unwrap_or("").trim().to_string(),
                },
            );
        }
        Ok(Resolver {
            datasets,
            sites,
            at,
        })
    }

    /// Whether a stack is in a scope.
    pub(crate) fn holds_at(&self, scope: &Scope, w: &Where) -> bool {
        match scope {
            Scope::Pack => true,
            Scope::Site(s) => w.sites.iter().any(|i| self.sites[*i] == *s),
            Scope::Dataset(names) => w
                .datasets
                .iter()
                .any(|i| names.iter().any(|n| *n == self.datasets[*i].0)),
            Scope::Scanner(keys) => keys.iter().all(|(k, v)| {
                let have = match k.as_str() {
                    "manufacturer" => &w.make,
                    "model" => &w.model,
                    "station" => &w.station,
                    "batch" => return v.parse::<i64>().ok() == Some(w.batch),
                    _ => return false,
                };
                have.eq_ignore_ascii_case(v.trim())
            }),
        }
    }

    /// Whether the stack is in a scope; a stack of another modality is in
    /// none.
    pub fn holds(&self, scope: &Scope, stack: i64) -> bool {
        self.at.get(&stack).is_some_and(|w| self.holds_at(scope, w))
    }

    /// The stacks a scope holds.
    pub fn stacks(&self, scope: &Scope) -> BTreeSet<i64> {
        self.at
            .iter()
            .filter(|(_, w)| self.holds_at(scope, w))
            .map(|(id, _)| *id)
            .collect()
    }

    /// The names of the datasets a scope names or lies over: a dataset
    /// scope's own, a site's datasets; none for the pack or a scanner.
    pub fn datasets_in(&self, scope: &Scope) -> Vec<String> {
        match scope {
            Scope::Dataset(names) => names.clone(),
            Scope::Site(s) => self
                .datasets
                .iter()
                .filter(|(_, site)| site.as_deref() == Some(s.as_str()))
                .map(|(n, _)| n.clone())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The names of the datasets that hold a stack.
    pub fn datasets_of(&self, stack: i64) -> Vec<String> {
        self.at
            .get(&stack)
            .map(|w| {
                w.datasets
                    .iter()
                    .map(|i| self.datasets[*i].0.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A scope named by what the registry holds: a site or a dataset is
    /// named by its name or its place id, and one it does not know is
    /// refused with what it knows.
    pub fn known(&self, scope: &Scope, store: &mut Store) -> Result<Scope, Error> {
        let by_id = |store: &mut Store, n: &str| {
            n.parse::<i64>()
                .ok()
                .and_then(|id| nils_registry::place::show(store, id).ok().flatten())
                .map(|p| p.name)
        };
        match scope {
            Scope::Site(s) => {
                let name = by_id(store, s).unwrap_or_else(|| s.clone());
                if self.sites.contains(&name) {
                    Ok(Scope::Site(name))
                } else {
                    Err(Error::Refused(format!(
                        "no site named {s}: a site is the source root its datasets arrive under, and the registry's are {}",
                        if self.sites.is_empty() {
                            "none".to_string()
                        } else {
                            self.sites.join(", ")
                        }
                    )))
                }
            }
            Scope::Dataset(names) => {
                let mut out = Vec::new();
                for n in names {
                    let name = by_id(store, n).unwrap_or_else(|| n.clone());
                    if !self.datasets.iter().any(|(d, _)| *d == name) {
                        return Err(Error::Refused(format!(
                            "no dataset named {n}; the registry's are {}",
                            self.datasets
                                .iter()
                                .map(|(d, _)| d.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )));
                    }
                    out.push(name);
                }
                Ok(Scope::Dataset(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// As [`Resolver::known`], for a scope adopted earlier: a site or a
    /// dataset since gone holds no stack, and nothing is refused.
    pub(crate) fn kept(&self, scope: &Scope, store: &mut Store) -> Scope {
        self.known(scope, store).unwrap_or_else(|_| scope.clone())
    }
}

/// One operation of an adopted scoped change.
#[derive(Debug, Clone)]
pub struct AdoptedOp {
    /// The proposal it is part of.
    pub proposal: i64,
    /// Its place in that proposal, from 1.
    pub at: usize,
    pub op: Op,
}

/// The pack's adopted scoped changes, as a sort applies them.
#[derive(Debug, Clone, Default)]
pub struct Adopted {
    /// Their operations, in the order the changes were adopted and each
    /// change's operations in its own order.
    pub ops: Vec<AdoptedOp>,
    /// The changes left out, each {proposal, why}: one that no longer
    /// applies to the pack as it is.
    pub skipped: Vec<Value>,
}

impl Adopted {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Their operations followed by `then`'s, as one patch of the pack,
    /// numbered in that order: what the pack is applied with, each stack
    /// keeping the operations whose scope holds it.
    pub fn with(&self, pack: &str, then: Option<&Patch>) -> Patch {
        let mut operations: Vec<Op> = self.ops.iter().map(|a| a.op.clone()).collect();
        if let Some(p) = then {
            operations.extend(p.operations.iter().cloned());
        }
        for (i, op) in operations.iter_mut().enumerate() {
            op.at = i + 1;
        }
        Patch {
            name: "the adopted scoped changes".into(),
            pack: pack.to_string(),
            version: None,
            ships: None,
            operations,
            cases: then.map(|p| p.cases.clone()).unwrap_or_default(),
        }
    }
}

/// A proposal's patch as the registry keeps it, read again: the operations
/// as written, against whatever version the pack is now.
pub fn patch_of(p: &nils_registry::rule_proposal::Proposal) -> Result<Patch, String> {
    let mut doc = p.patch.clone();
    if let Some(m) = doc.as_object_mut() {
        m.remove("version");
        m.remove("ships");
        m.remove("cases");
    }
    Patch::of_value(&format!("proposal {}", p.id), &doc).map_err(|e| e.to_string())
}

/// The pack's adopted scoped changes, each read again and checked to apply
/// to the pack as it is now. The pack's own name keys them; a pack read
/// under an overlay (`pack.overlay`) applies none of them, said in
/// `skipped`, since a change is applied to the pack's own documents.
pub fn adopted(store: &mut Store, pack: &Pack) -> Result<Adopted, Error> {
    let rows = nils_registry::rule_proposal::adopted_scoped(store, &pack.name)?;
    let mut out = Adopted::default();
    if rows.is_empty() {
        return Ok(out);
    }
    if let Some(o) = &pack.overlay {
        for p in rows {
            out.skipped.push(json!({
                "proposal": p.id,
                "why": format!("the sort runs under the overlay {o}, and a scoped change applies to the pack's own documents"),
            }));
        }
        return Ok(out);
    }
    for p in rows {
        let patch = match patch_of(&p) {
            Ok(patch) => patch,
            Err(why) => {
                out.skipped.push(json!({"proposal": p.id, "why": why}));
                continue;
            }
        };
        // it applies to the pack as it is, after the ones adopted before it
        let trial = Adopted {
            ops: out
                .ops
                .iter()
                .cloned()
                .chain(patch.operations.iter().map(|op| AdoptedOp {
                    proposal: p.id,
                    at: op.at,
                    op: op.clone(),
                }))
                .collect(),
            skipped: Vec::new(),
        };
        let whole = trial.with(&pack.name, None);
        match nils_pack::patch::apply(&pack.dir, &whole, &|_| true) {
            Ok(_) => out.ops = trial.ops,
            Err(e) => out.skipped.push(json!({
                "proposal": p.id,
                "why": format!("it no longer applies to {} {}: {e}", pack.name, pack.version),
            })),
        }
    }
    Ok(out)
}

/// The stacks a sort judges, split by the adopted operations that hold
/// them: one group per set of operations, each sorted by the pack with
/// those operations applied.
pub struct Plan {
    /// Each group's operations (their places in [`Adopted::ops`]) and its
    /// stacks; the stacks no operation holds are in no group.
    pub groups: Vec<(BTreeSet<usize>, BTreeSet<i64>)>,
    /// Per adopted operation, the stacks of the sort its scope holds.
    pub reach: Vec<i64>,
}

/// Which judged stacks each adopted operation reaches. `judged` says
/// whether a stack is one the sort judges.
pub fn plan(
    store: &mut Store,
    resolver: &Resolver,
    adopted: &Adopted,
    judged: &dyn Fn(i64) -> bool,
) -> Plan {
    let scopes: Vec<Scope> = adopted
        .ops
        .iter()
        .map(|a| resolver.kept(&a.op.scope, store))
        .collect();
    let mut reach = vec![0i64; scopes.len()];
    let mut by_key: BTreeMap<BTreeSet<usize>, BTreeSet<i64>> = BTreeMap::new();
    for (id, w) in &resolver.at {
        if !judged(*id) {
            continue;
        }
        let key: BTreeSet<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| resolver.holds_at(s, w))
            .map(|(i, _)| i)
            .collect();
        if key.is_empty() {
            continue;
        }
        for i in &key {
            reach[*i] += 1;
        }
        by_key.entry(key).or_default().insert(*id);
    }
    Plan {
        groups: by_key.into_iter().collect(),
        reach,
    }
}

/// The pack with the adopted operations of one group applied.
pub fn variant(pack: &Pack, adopted: &Adopted, ops: &BTreeSet<usize>) -> Result<Pack, String> {
    let whole = adopted.with(&pack.name, None);
    nils_pack::patch::apply(&pack.dir, &whole, &|op| ops.contains(&(op.at - 1)))
        .map(|p| p.pack)
        .map_err(|e| e.to_string())
}

/// The stacks of a dataset, by name: those its tree holds a file of.
pub fn stacks_of_dataset(
    store: &mut Store,
    modality: &str,
    dataset: &str,
) -> Result<BTreeSet<i64>, Error> {
    let r = Resolver::load(store, modality)?;
    let scope = r.known(&Scope::Dataset(vec![dataset.to_string()]), store)?;
    Ok(r.stacks(&scope))
}
