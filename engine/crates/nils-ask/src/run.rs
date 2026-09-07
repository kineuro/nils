// SPDX-License-Identifier: AGPL-3.0-only

//! The pipeline end to end (§11.1): parse and desugar, pin, validate,
//! inline the selections, re-evaluate a drifted handle, compile, execute,
//! the post pass, the identifiers, the handle and the kept sets. One
//! function the CLI and the doors both call, so the same document leaves
//! the same handle either way.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

use nils_registry::dialect::Dialect;
use nils_registry::home::{HomeError, Registry};
use nils_registry::linkage::{self, Subkeys};
use nils_registry::session::Scheme;
use nils_registry::store::{Cell, Error as StoreError, Row, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{Ask, Level, Out, Src};

use crate::compile::{Compiled, Context, compile};
use crate::exec::{Answer, Bounds, ExecError, run as execute};
use crate::handle::{self, Handle, HandleError, Provenance, Spec};
use crate::measure::{self, MeasureError, Measured};
use crate::selection::{self, Inlined, SelectionError};
use crate::validate::{Names, Scope, Validated, validate};
use crate::{Error as AskError, prepare};

#[derive(Debug)]
pub enum RunError {
    Ask(AskError),
    Store(StoreError),
    Home(HomeError),
    Exec(ExecError),
    Handle(HandleError),
    Selection(SelectionError),
    Measure(MeasureError),
    /// The principal may not project identifiers.
    Forbidden(String),
    Message(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Ask(e) => write!(f, "{e}"),
            RunError::Store(e) => write!(f, "{e}"),
            RunError::Home(e) => write!(f, "{e}"),
            RunError::Exec(e) => write!(f, "{e}"),
            RunError::Handle(e) => write!(f, "{e}"),
            RunError::Selection(e) => write!(f, "{e}"),
            RunError::Measure(e) => write!(f, "{e}"),
            RunError::Forbidden(m) | RunError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RunError {}

macro_rules! from_error {
    ($($t:ty => $v:ident),* $(,)?) => {
        $(impl From<$t> for RunError {
            fn from(e: $t) -> Self {
                RunError::$v(e)
            }
        })*
    };
}

from_error!(
    AskError => Ask,
    StoreError => Store,
    HomeError => Home,
    ExecError => Exec,
    HandleError => Handle,
    SelectionError => Selection,
    MeasureError => Measure,
);

/// One run's request.
pub struct Request<'a> {
    pub ask: Ask,
    pub names: &'a dyn Names,
    pub scope: &'a Scope,
    pub principal: &'a str,
    pub node: &'a str,
    pub pack_version: Option<&'a str>,
    /// The session scheme the ask is read under; the caller resolves the
    /// document's `scheme` to it.
    pub scheme: &'a Scheme,
    pub bounds: Bounds,
    pub page_rows: usize,
    /// A name for the answer's handle; a named handle stores its ask.
    pub name: Option<&'a str>,
    /// Honour the document's `keep`.
    pub keep: bool,
    pub after: Option<i64>,
    pub limit: Option<u64>,
    /// The door's decision for `out.identifiers` (§9).
    pub may_project_raw: bool,
    pub purpose: Option<&'a str>,
    /// The store the compiled statement runs on when it is not the
    /// registry's own: the door's read only reader (§12.4). The handle and
    /// the audit are written through the registry either way.
    pub reader: Option<&'a mut Store>,
}

/// A handle source read at another epoch than it was made, or expired:
/// the stored ask was re-evaluated and the keys compared (§8.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drift {
    pub set: String,
    pub handle: i64,
    pub replaced_by: i64,
    pub expired: bool,
    pub added: usize,
    pub removed: usize,
    pub epoch_then: i64,
    pub epoch_now: i64,
}

/// What a run produced.
#[derive(Debug)]
pub struct Outcome {
    /// The content hash of the ask's core, as stored and pinned.
    pub hash: String,
    pub compiled: Compiled,
    /// The answer as returned: measures and identifiers appended.
    pub answer: Answer,
    /// The handle saved, from the answer before the identifiers.
    pub handle: Handle,
    pub kept: Vec<Handle>,
    pub drift: Vec<Drift>,
    pub measured: Measured,
    pub identifiers: Vec<String>,
    pub inlined: Vec<Inlined>,
}

/// Both SQL texts of an ask, for `explain`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Explained {
    pub hash: String,
    pub sqlite: String,
    pub postgres: String,
    pub params: usize,
    pub columns: Vec<String>,
    pub inlined: Vec<Inlined>,
}

const MAX_DEPTH: usize = 4;

pub(crate) fn inline_selections(
    registry: &mut Registry,
    ask: &mut Ask,
) -> Result<Vec<Inlined>, RunError> {
    let store = registry.store();
    let mut load = |name: &str, version: u64| -> Result<Option<Ask>, SelectionError> {
        Ok(selection::get(store, name, Some(version))?.map(|v| v.ask))
    };
    Ok(selection::inline(ask, &mut load)?)
}

pub(crate) fn count_level(ask: &Ask, set: &str) -> Ask {
    let mut a = ask.clone();
    a.out = Out {
        set: set.to_string(),
        level: Level::Count,
        columns: Vec::new(),
        measures: Vec::new(),
        identifiers: Vec::new(),
        order: Vec::new(),
        limit: None,
    };
    a.keep = Vec::new();
    a
}

pub(crate) fn keys_level(ask: &Ask, set: &str) -> Ask {
    let mut a = ask.clone();
    a.out = Out {
        set: set.to_string(),
        level: Level::Record,
        columns: Vec::new(),
        measures: Vec::new(),
        identifiers: Vec::new(),
        order: Vec::new(),
        limit: None,
    };
    a.keep = Vec::new();
    a
}

pub(crate) struct Runner<'a> {
    pub(crate) names: &'a dyn Names,
    pub(crate) scheme: &'a Scheme,
    pub(crate) bounds: Bounds,
    /// The door's reader, when the statement runs elsewhere than the
    /// registry's own connection.
    pub(crate) reader: Option<&'a mut Store>,
}

impl Runner<'_> {
    /// Compile and execute one document as it stands, no handle.
    pub(crate) fn answer(
        &mut self,
        registry: &mut Registry,
        ask: &Ask,
        validated: &Validated,
        after: Option<i64>,
        limit: Option<u64>,
    ) -> Result<(Compiled, Answer), RunError> {
        let store: &mut Store = match self.reader.as_deref_mut() {
            Some(r) => r,
            None => registry.store(),
        };
        let ctx = Context {
            names: self.names,
            dialect: store.dialect(),
            schema: store.schema().map(str::to_string),
            window_days: self.scheme.window_days,
            scheme_digest: self.scheme.digest(),
            after,
            limit,
        };
        let compiled =
            compile(ask, validated, &ctx).map_err(|e| RunError::Message(e.to_string()))?;
        let answer = execute(store, &compiled, self.bounds)?;
        Ok((compiled, answer))
    }
}

/// A reborrow of the door's reader for one more run; clippy reads the
/// reborrow as needless, but the reader is used again after it.
#[allow(clippy::needless_option_as_deref)]
fn lend<'b>(reader: &'b mut Option<&mut Store>) -> Option<&'b mut Store> {
    reader.as_deref_mut()
}

/// Run an ask to a handle.
pub fn run(registry: &mut Registry, req: Request<'_>) -> Result<Outcome, RunError> {
    run_at(registry, req, 0)
}

fn run_at(registry: &mut Registry, req: Request<'_>, depth: usize) -> Result<Outcome, RunError> {
    if depth > MAX_DEPTH {
        return Err(RunError::Message(
            "handles reading handles more than four deep, or in a cycle".into(),
        ));
    }
    let mut reader = req.reader;
    let prepared = prepare(req.ask.clone(), req.names, req.scope)?;
    let hash = prepared.hash.clone();
    let mut ask = prepared.ask;
    let inlined = inline_selections(registry, &mut ask)?;
    let validated = if inlined.is_empty() {
        prepared.validated
    } else {
        validate(&ask, req.names, req.scope).map_err(AskError::Invalid)?
    };
    // a handle source at another epoch, or expired, is re-evaluated
    let epoch_now = registry.meta().epoch;
    let mut drift = Vec::new();
    let handle_sources: Vec<(String, String, bool)> = ask
        .sets
        .iter()
        .filter_map(|(n, s)| match &s.from {
            Some(Src::Handle { id, pin }) => Some((n.clone(), id.clone(), *pin)),
            _ => None,
        })
        .collect();
    for (set_name, id, pin) in handle_sources {
        let hid: i64 = id
            .parse()
            .map_err(|_| RunError::Message(format!("handle {id} is not a number")))?;
        let h = handle::get(registry.store(), hid)?.ok_or(HandleError::NotFound(hid))?;
        if h.withdrawn_at.is_some() {
            return Err(HandleError::Withdrawn(hid).into());
        }
        let expired = !h.has_rows();
        let stale = h.epoch != epoch_now;
        if pin && expired {
            return Err(HandleError::Expired(hid).into());
        }
        if pin || (!stale && !expired) {
            handle::touch(registry.store(), hid)?;
            continue;
        }
        let stored = h.ask.clone().ok_or_else(|| {
            RunError::Message(format!("handle {hid} stores no ask to re-evaluate"))
        })?;
        let fresh = run_at(
            registry,
            Request {
                ask: stored,
                names: req.names,
                scope: req.scope,
                principal: req.principal,
                node: req.node,
                pack_version: req.pack_version,
                scheme: req.scheme,
                bounds: req.bounds,
                page_rows: req.page_rows,
                name: None,
                keep: false,
                after: None,
                limit: None,
                may_project_raw: false,
                purpose: None,
                reader: lend(&mut reader),
            },
            depth + 1,
        )?;
        let (added, removed) = if expired {
            (0, 0)
        } else {
            let before: std::collections::BTreeSet<i64> = handle::keys(registry.store(), hid)?
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            let after: std::collections::BTreeSet<i64> =
                handle::keys(registry.store(), fresh.handle.id)?
                    .into_iter()
                    .map(|(k, _)| k)
                    .collect();
            (
                after.difference(&before).count(),
                before.difference(&after).count(),
            )
        };
        drift.push(Drift {
            set: set_name.clone(),
            handle: hid,
            replaced_by: fresh.handle.id,
            expired,
            added,
            removed,
            epoch_then: h.epoch,
            epoch_now,
        });
        if let Some(s) = ask.sets.get_mut(&set_name) {
            s.from = Some(Src::Handle {
                id: fresh.handle.id.to_string(),
                pin: true,
            });
        }
    }
    // the answer
    let mut runner = Runner {
        names: req.names,
        scheme: req.scheme,
        bounds: req.bounds,
        reader: lend(&mut reader),
    };
    let (compiled, mut answer) = runner.answer(registry, &ask, &validated, req.after, req.limit)?;
    // the post pass
    let mut over: BTreeMap<String, f64> = BTreeMap::new();
    for m in &ask.out.measures {
        for (kind, spec) in &m.0 {
            if kind == "share"
                && let Some(set) = spec.get("over").and_then(Value::as_str)
                && !over.contains_key(set)
            {
                let counted = count_level(&ask, set);
                let v = validate(&counted, req.names, req.scope).map_err(AskError::Invalid)?;
                let (_, a) = runner.answer(registry, &counted, &v, None, None)?;
                let subjects = a.rows.first().map(|r| r.int(1)).transpose()?.unwrap_or(0);
                over.insert(set.to_string(), subjects as f64);
            }
        }
    }
    let stored_answer = answer.clone();
    // a scalar the answer lacks but a group's child exposes: per group
    let mut group_columns = group_scalars(
        registry,
        &mut runner,
        &ask,
        req.names,
        req.scope,
        &mut answer,
    )?;
    let remaining: Vec<crate::ast::Measure> = ask
        .out
        .measures
        .iter()
        .filter(|m| {
            m.0.iter().all(|(kind, spec)| {
                !measure::is_scalar(kind)
                    || spec
                        .get("of")
                        .and_then(Value::as_str)
                        .is_some_and(|of| stored_answer.columns.iter().any(|c| c == of))
            })
        })
        .cloned()
        .collect();
    let mut measured = measure::apply(&mut answer, &remaining, &over)?;
    // the columns in the answer's order: the group's scalars, then the shares
    group_columns.append(&mut measured.columns);
    measured.columns = group_columns;
    // the handle, from the answer before any identifier
    let grain = ask
        .sets
        .get(&ask.out.set)
        .map(|s| s.grain)
        .ok_or_else(|| RunError::Message(format!("no set {}", ask.out.set)))?;
    let params: Value = ask
        .params
        .iter()
        .map(|(k, d)| (k.clone(), d.value.clone().unwrap_or(Value::Null)))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    // every selection read, pinned at validate or already pinned in the
    // document, once per set
    let mut pinned: Vec<Value> = inlined
        .iter()
        .map(|i| json!({"set": i.set, "selection": i.name, "version": i.version}))
        .collect();
    for (set, name, v) in &prepared.pinned {
        if !inlined.iter().any(|i| i.set == *set) {
            pinned.push(json!({"set": set, "selection": name, "version": v}));
        }
    }
    let mut unresolved = serde_json::Map::new();
    for decl in ask.values.values() {
        if let Some(shape) = crate::values::shape(registry.store(), &decl.upload)
            .map_err(|e| RunError::Message(e.to_string()))?
        {
            unresolved.insert(decl.upload.clone(), shape);
        }
    }
    let disclosure = if req.scope.federated {
        "federated"
    } else {
        "local"
    };
    let suppression = json!({"classes": req.scope.classes});
    let scheme_digest = req.scheme.digest();
    let provenance = Provenance {
        principal: req.principal,
        node: req.node,
        pack_version: req.pack_version,
        epoch: epoch_now,
        scheme_digest: Some(&scheme_digest),
        disclosure,
        suppression,
    };
    let store = registry.store();
    store.begin()?;
    let saved = (|| -> Result<(Handle, Vec<Handle>), RunError> {
        let spec = Spec {
            name: req.name,
            grain,
            ask: Some(&ask),
            params: params.clone(),
            selection_versions: Value::Array(pinned.clone()),
            values_unresolved: Value::Object(unresolved.clone()),
            provenance: provenance.clone(),
            page_rows: req.page_rows,
        };
        let h = handle::save(store, &spec, &stored_answer)?;
        Ok((h, Vec::new()))
    })();
    let (h, mut kept) = match saved {
        Ok(v) => v,
        Err(e) => {
            registry.store().rollback().ok();
            return Err(e);
        }
    };
    registry.store().commit()?;
    // the kept sets, each its own handle of keys
    if req.keep {
        let keeps: Vec<String> = ask
            .keep
            .iter()
            .filter(|k| **k != ask.out.set && ask.sets.contains_key(*k))
            .cloned()
            .collect();
        for set in keeps {
            let variant = keys_level(&ask, &set);
            let v = validate(&variant, req.names, req.scope).map_err(AskError::Invalid)?;
            let (_, a) = runner.answer(registry, &variant, &v, None, None)?;
            let name = req.name.map(|n| format!("{n}/{set}"));
            let spec = Spec {
                name: name.as_deref(),
                grain: variant.sets[&set].grain,
                ask: Some(&variant),
                params: params.clone(),
                selection_versions: Value::Array(pinned.clone()),
                values_unresolved: Value::Object(unresolved.clone()),
                provenance: provenance.clone(),
                page_rows: req.page_rows,
            };
            let store = registry.store();
            store.begin()?;
            match handle::save(store, &spec, &a) {
                Ok(k) => {
                    store.commit()?;
                    kept.push(k);
                }
                Err(e) => {
                    store.rollback().ok();
                    return Err(e.into());
                }
            }
        }
    }
    // the identifiers, role gated, with an audit row
    let identifiers = ask.out.identifiers.clone();
    if !identifiers.is_empty() {
        if !req.may_project_raw {
            return Err(RunError::Forbidden(
                "identifiers are projected only by a role that may read them".into(),
            ));
        }
        reveal_into(
            registry,
            &mut answer,
            &identifiers,
            req.principal,
            req.purpose,
        )?;
        handle::read_audit(
            registry.store(),
            req.principal,
            h.id,
            &identifiers,
            answer.rows.len(),
            req.purpose,
            epoch_now,
        )?;
    }
    Ok(Outcome {
        hash,
        compiled,
        answer,
        handle: h,
        kept,
        drift,
        measured,
        identifiers,
        inlined,
    })
}

/// The scalar measures over a group answer's child (gold C's `stddev {of:
/// age}` over `people`, per cohort): the child's rows are read with the
/// group's by tuple, grouped in Rust, and one column per measure joins
/// the answer by its key.
fn group_scalars(
    registry: &mut Registry,
    runner: &mut Runner<'_>,
    ask: &Ask,
    names: &dyn Names,
    scope: &Scope,
    answer: &mut Answer,
) -> Result<Vec<String>, RunError> {
    let set = &ask.sets[&ask.out.set];
    let Some(g) = &set.group else {
        return Ok(Vec::new());
    };
    let wanted: Vec<(String, String, Option<f64>)> = ask
        .out
        .measures
        .iter()
        .flat_map(|m| m.0.iter())
        .filter(|(kind, _)| measure::is_scalar(kind))
        .filter_map(|(kind, spec)| {
            let of = spec.get("of").and_then(Value::as_str)?;
            let p = spec.get("p").and_then(Value::as_f64);
            (!answer.columns.iter().any(|c| c == of)).then(|| (kind.clone(), of.to_string(), p))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let tuple = |row: &Row, from: usize, n: usize| -> String {
        row.0[from..from + n]
            .iter()
            .map(crate::exec::render)
            .collect::<Vec<_>>()
            .join("\t")
    };
    // the group's by tuple per answer key
    let mut gv = ask.clone();
    gv.out = Out {
        set: ask.out.set.clone(),
        level: Level::Record,
        columns: g.by.clone(),
        measures: Vec::new(),
        identifiers: Vec::new(),
        order: Vec::new(),
        limit: None,
    };
    gv.keep = Vec::new();
    let v = validate(&gv, names, scope).map_err(AskError::Invalid)?;
    let (_, groups) = runner.answer(registry, &gv, &v, None, None)?;
    let by_n = g.by.len();
    let key_of: BTreeMap<String, i64> = groups
        .rows
        .iter()
        .filter_map(|r| Some((tuple(r, 2, by_n), r.opt_int(0).ok().flatten()?)))
        .collect();
    // the child's rows with the by tuple and every measured field
    let ofs: Vec<String> = {
        let mut o: Vec<String> = wanted.iter().map(|(_, of, _)| of.clone()).collect();
        o.sort();
        o.dedup();
        o
    };
    let mut cv = ask.clone();
    let mut columns = g.by.clone();
    columns.extend(ofs.iter().map(|o| crate::ast::Clause::field(o)));
    cv.out = Out {
        set: g.of.clone(),
        level: Level::Record,
        columns,
        measures: Vec::new(),
        identifiers: Vec::new(),
        order: Vec::new(),
        limit: None,
    };
    cv.keep = Vec::new();
    let v = validate(&cv, names, scope).map_err(AskError::Invalid)?;
    let (_, child) = runner.answer(registry, &cv, &v, None, None)?;
    if child.truncated {
        return Err(MeasureError::Truncated.into());
    }
    let mut values: BTreeMap<(String, i64), Vec<f64>> = BTreeMap::new();
    for r in &child.rows {
        let Some(k) = key_of.get(&tuple(r, 2, by_n)) else {
            continue;
        };
        for (i, of) in ofs.iter().enumerate() {
            if let Some(x) = r.0.get(2 + by_n + i).and_then(measure::number) {
                values.entry((of.clone(), *k)).or_default().push(x);
            }
        }
    }
    let mut added = Vec::new();
    for (kind, of, p) in &wanted {
        let name = measure::name_of(kind, of, *p);
        for row in answer.rows.iter_mut() {
            let k = row.opt_int(0).ok().flatten();
            let cell = match k.and_then(|k| values.get(&(of.clone(), k)).cloned()) {
                Some(mut xs) => {
                    measure::scalar(kind, &mut xs, *p)?.map_or(Cell::Null, Cell::Double)
                }
                None => Cell::Null,
            };
            row.0.push(cell);
        }
        answer.columns.push(name.clone());
        added.push(name);
    }
    Ok(added)
}

/// Append one column per namespace to every row, the subject's identifier
/// decrypted through the linkage store (which writes its own read audit).
fn reveal_into(
    registry: &mut Registry,
    answer: &mut Answer,
    namespaces: &[String],
    principal: &str,
    purpose: Option<&str>,
) -> Result<(), RunError> {
    let subject_col = answer
        .columns
        .iter()
        .position(|c| c == "_subject")
        .ok_or_else(|| RunError::Message("the answer carries no subject to identify".into()))?;
    let key = registry.pseudonym_key()?;
    let keys = Subkeys::derive(&key);
    let mut lk = registry.open_linkage()?;
    let mut cache: BTreeMap<i64, Vec<(String, String)>> = BTreeMap::new();
    for row in answer.rows.iter_mut() {
        let subject = row.0.get(subject_col).and_then(|c| match c {
            Cell::Int(i) => Some(*i),
            _ => None,
        });
        let none: Vec<(String, String)> = Vec::new();
        let revealed: &Vec<(String, String)> = match subject {
            Some(s) => match cache.entry(s) {
                Entry::Occupied(o) => o.into_mut(),
                Entry::Vacant(v) => {
                    let r = linkage::reveal(&mut lk, &keys, s, principal, purpose)?
                        .into_iter()
                        .map(|r| (r.id_type, r.value))
                        .collect();
                    v.insert(r)
                }
            },
            None => &none,
        };
        for ns in namespaces {
            let values: Vec<&str> = revealed
                .iter()
                .filter(|(t, _)| t == ns)
                .map(|(_, v)| v.as_str())
                .collect();
            row.0.push(if values.is_empty() {
                Cell::Null
            } else {
                Cell::Text(values.join(";"))
            });
        }
    }
    answer.columns.extend(namespaces.iter().cloned());
    Ok(())
}

/// Both SQL texts of a document, with its selections inlined: `nils ask
/// explain --dialect`.
pub fn explain(
    registry: &mut Registry,
    ask: Ask,
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
) -> Result<Explained, RunError> {
    let prepared = prepare(ask, names, scope)?;
    let mut ask = prepared.ask;
    let inlined = inline_selections(registry, &mut ask)?;
    let validated = if inlined.is_empty() {
        prepared.validated
    } else {
        validate(&ask, names, scope).map_err(AskError::Invalid)?
    };
    let schema = registry.store().schema().map(str::to_string);
    let mut texts = Vec::new();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ctx = Context {
            names,
            dialect,
            schema: (dialect == Dialect::Postgres)
                .then(|| schema.clone())
                .flatten(),
            window_days: scheme.window_days,
            scheme_digest: scheme.digest(),
            after: None,
            limit: None,
        };
        texts.push(compile(&ask, &validated, &ctx).map_err(|e| RunError::Message(e.to_string()))?);
    }
    let pg = texts.pop().expect("postgres");
    let lite = texts.pop().expect("sqlite");
    Ok(Explained {
        hash: prepared.hash,
        params: lite.params.len(),
        columns: lite.columns.clone(),
        sqlite: lite.sql,
        postgres: pg.sql,
        inlined,
    })
}

/// The first cell of a count level answer, for callers reading a count.
pub fn count_of(row: &Row) -> Result<i64, RunError> {
    Ok(row.int(0)?)
}
