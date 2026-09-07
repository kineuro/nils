// SPDX-License-Identifier: AGPL-3.0-only

//! Strict validation (§4.4, rule 14): what storage and execution accept.
//! Every refusal carries one code of the fixed taxonomy, the path of the
//! slot it is about (`sets.<name>.<slot>[i]`), and the next call that would
//! settle it. The catalog is reached through [`Names`], which the catalog
//! crate implements over a registry and a test implements over a fixture.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::{Arg, Ask, Clause, Grain, Policy, SchemeRef, Set, Src};

/// The class of a field (§4.4, rule 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Technical,
    QuasiIdentifying,
    Clinical,
    Sensitive,
    Identifying,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    pub class: Class,
    /// Whether the field is a date.
    pub dated: bool,
    /// Whether the field may leave the node in a federated answer.
    pub federated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KindInfo {
    pub precision: String,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DerivedInfo {
    pub grain: Grain,
    pub params: Vec<String>,
}

/// What validation asks the catalog (§9).
pub trait Names {
    /// A field of a level (`subject`, `study`, `series`, `session`, `stack`,
    /// `instance`, `event`, `cohort`), by its catalog path.
    fn field(&self, level: &str, path: &str) -> Option<FieldInfo>;
    /// The values of a classification axis; none for an unknown axis.
    fn axis_values(&self, axis: &str) -> Option<Vec<String>>;
    fn kind(&self, name: &str) -> Option<KindInfo>;
    fn level(&self, name: &str) -> bool;
    /// A library set shipped by the pack, and its grain.
    fn role(&self, name: &str) -> Option<Grain>;
    /// A stored scheme's digest.
    fn scheme(&self, name: &str) -> Option<String>;
    fn cohort(&self, name: &str) -> bool;
    /// A saved ask's current version.
    fn selection(&self, name: &str) -> Option<u64>;
    fn handle(&self, id: &str) -> Option<Grain>;
    fn upload(&self, id: &str) -> bool;
    fn derived(&self, name: &str) -> Option<DerivedInfo>;
}

/// The principal's scope, as the catalog's policy sees it.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub federated: bool,
    /// Classes the principal may project raw (§9).
    pub classes: BTreeSet<Class>,
}

/// The taxonomy (§4.4, rule 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    UnknownSet,
    UnknownField,
    AmbiguousPath,
    GrainMismatch,
    Cycle,
    NotDated,
    NotFunctional,
    AmbiguousParent,
    UnknownValue,
    UnknownLevel,
    ForbiddenField,
    FederatedScope,
    SchemeMismatch,
    SelectionOutdated,
    BindingDropped,
    MissingOrder,
    NotReleasable,
    Truncated,
    StaleOptions,
}

impl Code {
    pub fn name(self) -> &'static str {
        match self {
            Code::UnknownSet => "unknown_set",
            Code::UnknownField => "unknown_field",
            Code::AmbiguousPath => "ambiguous_path",
            Code::GrainMismatch => "grain_mismatch",
            Code::Cycle => "cycle",
            Code::NotDated => "not_dated",
            Code::NotFunctional => "not_functional",
            Code::AmbiguousParent => "ambiguous_parent",
            Code::UnknownValue => "unknown_value",
            Code::UnknownLevel => "unknown_level",
            Code::ForbiddenField => "forbidden_field",
            Code::FederatedScope => "federated_scope",
            Code::SchemeMismatch => "scheme_mismatch",
            Code::SelectionOutdated => "selection_outdated",
            Code::BindingDropped => "binding_dropped",
            Code::MissingOrder => "missing_order",
            Code::NotReleasable => "not_releasable",
            Code::Truncated => "truncated",
            Code::StaleOptions => "stale_options",
        }
    }

    /// A warning leaves the document valid; an error refuses it.
    pub fn is_warning(self) -> bool {
        matches!(
            self,
            Code::SelectionOutdated | Code::BindingDropped | Code::NotReleasable
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub code: Code,
    pub path: String,
    pub message: String,
    /// What would settle it: a call, a move, a name to change.
    pub next: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}: {} ({})",
            self.code.name(),
            self.path,
            self.message,
            self.next
        )
    }
}

/// What a set exposes to the sets that read it and to its own clauses.
#[derive(Debug, Clone, Default)]
pub struct Exposed {
    pub grain: Option<Grain>,
    /// Bindings by name, in order.
    pub bindings: Vec<String>,
    /// Partners by `as`, each a set name.
    pub partners: BTreeMap<String, String>,
    /// The `of` ancestor, by set name.
    pub of: Option<String>,
    /// The sources the set narrows (`from` a set), which pass every binding.
    pub from: Option<String>,
    /// `pick` exposes `pick.tied`, `pick.candidates`, `pick.rank`.
    pub picked: bool,
    /// A group exposes its `by` paths, `_rows`, `_subjects` and aggregates.
    pub group_by: Vec<String>,
    /// Bindings that are a `change` pair, which expose `from_date`,
    /// `to_date`, `precision` and `gap_days` under their name.
    pub change_bindings: Vec<String>,
}

/// The result of a validation: every set's exposure, for the compiler, and
/// the warnings that did not refuse the document.
#[derive(Debug, Clone, Default)]
pub struct Validated {
    pub sets: BTreeMap<String, Exposed>,
    /// Topological order of the sets.
    pub order: Vec<String>,
    pub warnings: Vec<Issue>,
}

const ARITHMETIC: &[&str] = &["+", "-", "*", "/"];
const COMPARISONS: &[&str] = &["=", "<>", ">", ">=", "<", "<=", "~=", "in", "not_in"];
const PREDICATES: &[&str] = &[
    "has",
    "not_null",
    "is_null",
    "contains",
    "starts_with",
    "and",
    "or",
    "not",
];
const FUNCTIONS: &[&str] = &[
    "abs",
    "round",
    "coalesce",
    "case",
    "concat",
    "days_between",
    "shift",
    "age_at",
    "bucket",
    "part",
    "ordinal",
    "prev",
    "next",
    "change",
    "share",
];
const AGGREGATES: &[&str] = &["count", "distinct", "min", "max", "sum", "avg", "list"];
const REFS: &[&str] = &["field", "axis", "derived", "param"];

const LEVELS: &[&str] = &[
    "cohort", "subject", "study", "series", "session", "stack", "instance", "event",
];

fn issue(
    code: Code,
    path: impl Into<String>,
    message: impl Into<String>,
    next: impl Into<String>,
) -> Issue {
    Issue {
        code,
        path: path.into(),
        message: message.into(),
        next: next.into(),
    }
}

/// Validate a desugared ask. Errors refuse; warnings ride on the result.
pub fn validate(ask: &Ask, names: &dyn Names, scope: &Scope) -> Result<Validated, Vec<Issue>> {
    let mut issues: Vec<Issue> = Vec::new();
    let mut out = Validated::default();

    if !ask.pipeline.is_empty() {
        issues.push(issue(
            Code::GrainMismatch,
            "pipeline",
            "the document is not desugared",
            "desugar first",
        ));
    }
    if ask.sets.is_empty() {
        issues.push(issue(
            Code::UnknownSet,
            "sets",
            "an ask names at least one set",
            "add a set",
        ));
        return Err(issues);
    }
    for name in ask.sets.keys() {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            issues.push(issue(
                Code::UnknownSet,
                format!("sets.{name}"),
                "a set name is letters, digits and underscores",
                "rename the set",
            ));
        }
    }

    // the scheme
    match &ask.scheme {
        None => {}
        Some(SchemeRef::Name(n)) if n == "default" || n == "day" => {}
        Some(SchemeRef::Name(n)) => {
            if names.scheme(n).is_none() {
                issues.push(issue(
                    Code::SchemeMismatch,
                    "scheme",
                    format!("no scheme named {n} is kept; `study` is not offered"),
                    "GET /api/ask/catalog for the schemes kept",
                ));
            }
        }
        Some(SchemeRef::Inline(m)) => {
            let text = serde_json::to_string(m).unwrap_or_default();
            if let Err(e) = nils_registry::session::Scheme::from_json(&text) {
                issues.push(issue(
                    Code::SchemeMismatch,
                    "scheme",
                    format!("the inline scheme will not parse: {e}"),
                    "fix the scheme",
                ));
            }
        }
    }

    // the DAG
    let order = match topological(ask) {
        Ok(o) => o,
        Err(cycle) => {
            issues.push(issue(
                Code::Cycle,
                format!("sets.{}", cycle.first().cloned().unwrap_or_default()),
                format!(
                    "the sets read each other in a cycle: {}",
                    cycle.join(" -> ")
                ),
                "break the cycle",
            ));
            return Err(issues);
        }
    };
    out.order = order.clone();

    // the sets, in topological order, so a reader sees what it reads
    for name in &order {
        let set = &ask.sets[name];
        let exposed = validate_set(ask, name, set, names, scope, &out, &mut issues);
        out.sets.insert(name.clone(), exposed);
    }

    // keep and out
    for (i, k) in ask.keep.iter().enumerate() {
        match ask.sets.get(k) {
            None => issues.push(issue(Code::UnknownSet, format!("keep[{i}]"), format!("no set named {k}"), "name a set of this document")),
            Some(s) if !reaches_release(s.grain) => issues.push(issue(
                Code::NotReleasable,
                format!("keep[{i}]"),
                format!("{k} is at grain {}, which does not reach the release; its handle is a table to read and export, never to release", s.grain),
                "keep a subject, session or stack set for a release",
            )),
            _ => {}
        }
    }
    match ask.sets.get(&ask.out.set) {
        None => issues.push(issue(
            Code::UnknownSet,
            "out.set",
            format!("no set named {}", ask.out.set),
            "name a set of this document",
        )),
        Some(set) => {
            let exposed = out.sets.get(&ask.out.set).cloned().unwrap_or_default();
            for (i, c) in ask.out.columns.iter().enumerate() {
                check_clause(
                    c,
                    &format!("out.columns[{i}]"),
                    set,
                    &ask.out.set,
                    ask,
                    &out,
                    &exposed,
                    names,
                    scope,
                    &mut issues,
                );
            }
            for (i, o) in ask.out.order.iter().enumerate() {
                check_clause(
                    &o.0,
                    &format!("out.order[{i}]"),
                    set,
                    &ask.out.set,
                    ask,
                    &out,
                    &exposed,
                    names,
                    scope,
                    &mut issues,
                );
            }
            for (i, m) in ask.out.measures.iter().enumerate() {
                for (k, v) in &m.0 {
                    if !matches!(k.as_str(), "share" | "stddev" | "median" | "percentile") {
                        issues.push(issue(
                            Code::UnknownField,
                            format!("out.measures[{i}]"),
                            format!(
                                "{k} is not a measure; those are share, stddev, median, percentile"
                            ),
                            "rename the measure",
                        ));
                    }
                    if k == "share"
                        && let Some(over) = v.get("over").and_then(Value::as_str)
                        && !ask.sets.contains_key(over)
                    {
                        issues.push(issue(
                            Code::UnknownSet,
                            format!("out.measures[{i}].share.over"),
                            format!("no set named {over}"),
                            "name the denominator's set",
                        ));
                    }
                }
            }
            if !ask.out.identifiers.is_empty() && set.grain != Grain::Subject {
                issues.push(issue(
                    Code::GrainMismatch,
                    "out.identifiers",
                    "identifiers are projected at subject grain",
                    "set out.set to a subject set or drop identifiers",
                ));
            }
        }
    }

    let (warnings, errors): (Vec<Issue>, Vec<Issue>) =
        issues.into_iter().partition(|i| i.code.is_warning());
    out.warnings = warnings;
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// Whether a grain reaches the release (§4.2).
pub fn reaches_release(g: Grain) -> bool {
    matches!(
        g,
        Grain::Cohort | Grain::Subject | Grain::Session | Grain::Stack | Grain::Instance
    )
}

/// Kahn's order over `reads()`, or the cycle.
fn topological(ask: &Ask) -> Result<Vec<String>, Vec<String>> {
    let mut indegree: BTreeMap<&str, usize> = ask.sets.keys().map(|k| (k.as_str(), 0)).collect();
    let mut readers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, set) in &ask.sets {
        let mut seen = HashSet::new();
        for r in set.reads() {
            if ask.sets.contains_key(r) && seen.insert(r) {
                *indegree.get_mut(name.as_str()).expect("a set") += 1;
                readers.entry(r).or_default().push(name);
            }
        }
    }
    let mut ready: Vec<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    ready.sort();
    let mut order = Vec::new();
    while let Some(n) = ready.pop() {
        order.push(n.to_string());
        if let Some(rs) = readers.get(n) {
            for r in rs {
                let d = indegree.get_mut(r).expect("a set");
                *d -= 1;
                if *d == 0 {
                    ready.push(r);
                    ready.sort_by(|a, b| b.cmp(a));
                }
            }
        }
    }
    if order.len() == ask.sets.len() {
        Ok(order)
    } else {
        let stuck: Vec<String> = indegree
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(n, _)| n.to_string())
            .collect();
        Err(stuck)
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_set(
    ask: &Ask,
    name: &str,
    set: &Set,
    names: &dyn Names,
    scope: &Scope,
    so_far: &Validated,
    issues: &mut Vec<Issue>,
) -> Exposed {
    let path = |slot: &str| format!("sets.{name}.{slot}");
    let mut exposed = Exposed {
        grain: Some(set.grain),
        ..Exposed::default()
    };
    if set.grain == Grain::Pair {
        issues.push(issue(
            Code::GrainMismatch,
            path("grain"),
            "pair is reserved and staged (§3)",
            "use near best and pick per: subject (Appendix B, gold B)",
        ));
    }
    let grain_of = |s: &str| ask.sets.get(s).map(|x| x.grain);

    // the source
    let sources = [
        set.from.is_some(),
        set.of.is_some(),
        set.algebra.is_some(),
        set.group.is_some(),
    ];
    if set.grain == Grain::Group && set.group.is_none() {
        issues.push(issue(
            Code::GrainMismatch,
            path("group"),
            "a group set needs group: {of, by}",
            "add group",
        ));
    }
    if set.grain != Grain::Group && set.group.is_some() {
        issues.push(issue(
            Code::GrainMismatch,
            path("grain"),
            "group: {of, by} makes a set of grain group",
            "set grain: group",
        ));
    }
    if sources.iter().filter(|b| **b).count() > 1 && !(set.from.is_some() && set.of.is_some()) {
        issues.push(issue(
            Code::AmbiguousParent,
            path("from"),
            "a set starts from one source: from, of, algebra or group",
            "keep one",
        ));
    }
    match &set.from {
        None => {}
        Some(Src::Set(s)) => match grain_of(s) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("from"),
                format!("no set named {s}"),
                "name a set of this document",
            )),
            Some(g) if g != set.grain => issues.push(issue(
                Code::GrainMismatch,
                path("from"),
                format!(
                    "{s} is at grain {g}, this set at {}; from narrows a same grain set",
                    set.grain
                ),
                "use of for an ancestor, has or attach for a descendant",
            )),
            Some(_) => {
                exposed.from = Some(s.clone());
                if let Some(e) = so_far.sets.get(s) {
                    exposed.bindings.extend(e.bindings.iter().cloned());
                    exposed.partners.extend(e.partners.clone());
                    exposed.of.clone_from(&e.of);
                    exposed.picked = e.picked;
                    exposed
                        .change_bindings
                        .extend(e.change_bindings.iter().cloned());
                }
            }
        },
        Some(Src::Role(r)) => match names.role(r) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("from"),
                format!("no role named {r} in the pack"),
                "GET /api/ask/catalog for the roles",
            )),
            Some(g) if g != set.grain => issues.push(issue(
                Code::GrainMismatch,
                path("from"),
                format!("role {r} is a {g} set"),
                "set the grain to match",
            )),
            Some(_) => {}
        },
        Some(Src::Handle { id, .. }) => match names.handle(id) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("from"),
                format!("no handle {id}"),
                "GET /api/ask/handles",
            )),
            Some(g) if g != set.grain => issues.push(issue(
                Code::GrainMismatch,
                path("from"),
                format!("handle {id} is at grain {g}"),
                "set the grain to match",
            )),
            Some(_) => {}
        },
        Some(Src::Selection { name: sel, version }) => match names.selection(sel) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("from"),
                format!("no selection named {sel}"),
                "GET /api/ask/selections",
            )),
            Some(current) => match version {
                None => issues.push(issue(
                    Code::SelectionOutdated,
                    path("from"),
                    format!("selection:{sel} is not pinned to a version"),
                    "validate pins it; run validate before storing",
                )),
                Some(v) if *v > current => issues.push(issue(
                    Code::UnknownSet,
                    path("from"),
                    format!("selection {sel} has no version {v}; the current is {current}"),
                    "pin a version that exists",
                )),
                Some(v) if *v < current => issues.push(issue(
                    Code::SelectionOutdated,
                    path("from"),
                    format!("selection {sel} is at version {current}; this document pins {v}"),
                    format!("apply the move `update {sel} to version {current}`, or keep the pin"),
                )),
                Some(_) => {}
            },
        },
        Some(Src::Values(v)) => {
            if !ask.values.contains_key(v) {
                issues.push(issue(
                    Code::UnknownSet,
                    path("from"),
                    format!("values:{v} is not declared in values"),
                    "declare the upload under values",
                ));
            } else if let Some(decl) = ask.values.get(v)
                && !names.upload(&decl.upload)
            {
                issues.push(issue(
                    Code::UnknownSet,
                    path("from"),
                    format!(
                        "upload {} is not known; uploads live only until resolved",
                        decl.upload
                    ),
                    "POST /api/ask/values again",
                ));
            }
            if set.grain != Grain::Subject {
                issues.push(issue(
                    Code::GrainMismatch,
                    path("from"),
                    "an uploaded list resolves to subjects",
                    "set grain: subject",
                ));
            }
            if scope.federated {
                issues.push(issue(
                    Code::FederatedScope,
                    path("from"),
                    "a values source is refused under a federated run",
                    "run at home",
                ));
            }
        }
    }
    if let Some(o) = &set.of {
        match grain_of(o) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("of"),
                format!("no set named {o}"),
                "name a set of this document",
            )),
            Some(g) if !g.is_ancestor_of(set.grain) => issues.push(issue(
                Code::GrainMismatch,
                path("of"),
                format!(
                    "{o} is at grain {g}, which is not an ancestor of {}",
                    set.grain
                ),
                "of names an ancestor set; has and attach name a descendant",
            )),
            Some(_) => exposed.of = Some(o.clone()),
        }
    }
    if let Some(a) = &set.algebra {
        if a.sets.len() < 2 {
            issues.push(issue(
                Code::GrainMismatch,
                path("algebra.sets"),
                "set algebra takes two or more sets",
                "add an operand",
            ));
        }
        if a.op == crate::ast::AlgOp::Except && a.sets.len() != 2 {
            issues.push(issue(
                Code::GrainMismatch,
                path("algebra.sets"),
                "except takes exactly two sets",
                "left except right",
            ));
        }
        if a.tag.is_some() && a.op != crate::ast::AlgOp::Union {
            issues.push(issue(
                Code::GrainMismatch,
                path("algebra.tag"),
                "tag unpivots a union only",
                "drop tag or use union",
            ));
        }
        for (i, s) in a.sets.iter().enumerate() {
            match grain_of(s) {
                None => issues.push(issue(
                    Code::UnknownSet,
                    format!("{}[{i}]", path("algebra.sets")),
                    format!("no set named {s}"),
                    "name a set of this document",
                )),
                Some(g) if g != set.grain => issues.push(issue(
                    Code::GrainMismatch,
                    format!("{}[{i}]", path("algebra.sets")),
                    format!(
                        "{s} is at grain {g}, this set at {}; algebra stays at one grain",
                        set.grain
                    ),
                    "match the grains",
                )),
                Some(_) => {}
            }
        }
        // the left operand's bindings, the common ones under union
        if let Some(first) = a.sets.first()
            && let Some(e) = so_far.sets.get(first)
        {
            let mut kept: Vec<String> = e.bindings.clone();
            if a.op == crate::ast::AlgOp::Union {
                for other in a.sets.iter().skip(1) {
                    if let Some(oe) = so_far.sets.get(other) {
                        let dropped: Vec<String> = kept
                            .iter()
                            .filter(|b| !oe.bindings.contains(b))
                            .cloned()
                            .collect();
                        if !dropped.is_empty() {
                            issues.push(issue(
                                Code::BindingDropped,
                                path("algebra.sets"),
                                format!(
                                    "the union keeps the common bindings; {} drops {}",
                                    other,
                                    dropped.join(", ")
                                ),
                                "bind it on every operand, or read it before the union",
                            ));
                        }
                        kept.retain(|b| oe.bindings.contains(b));
                    }
                }
            }
            exposed.bindings.extend(kept);
            exposed
                .change_bindings
                .extend(e.change_bindings.iter().cloned());
            if let Some(t) = &a.tag {
                exposed.bindings.push(t.clone());
            }
        }
    }
    if let Some(g) = &set.group {
        match ask.sets.get(&g.of) {
            None => issues.push(issue(
                Code::UnknownSet,
                path("group.of"),
                format!("no set named {}", g.of),
                "name a set of this document",
            )),
            Some(child) => {
                let child_exposed = so_far.sets.get(&g.of).cloned().unwrap_or_default();
                if g.by.is_empty() {
                    issues.push(issue(
                        Code::MissingOrder,
                        path("group.by"),
                        "a group needs at least one by",
                        "add a by",
                    ));
                }
                for (i, c) in g.by.iter().enumerate() {
                    check_clause(
                        c,
                        &format!("{}[{i}]", path("group.by")),
                        child,
                        &g.of,
                        ask,
                        so_far,
                        &child_exposed,
                        names,
                        scope,
                        issues,
                    );
                    if let Some(p) = c.ref_name() {
                        exposed.group_by.push(p.to_string());
                    }
                }
                exposed.bindings.push("_rows".into());
                exposed.bindings.push("_subjects".into());
            }
        }
    }

    // near, attach, has: descendant and sideways relations
    for (i, n) in set.near.iter().enumerate() {
        let p = format!("{}[{i}]", path("near"));
        match grain_of(&n.set) {
            None => issues.push(issue(
                Code::UnknownSet,
                format!("{p}.set"),
                format!("no set named {}", n.set),
                "name a set of this document",
            )),
            Some(g) => {
                if !set.grain.dated() || !g.dated() {
                    issues.push(issue(
                        Code::NotDated,
                        format!("{p}.set"),
                        format!(
                            "near compares days; {} is not a dated grain",
                            if set.grain.dated() { g } else { set.grain }
                        ),
                        "near stays between two dated sets of one subject",
                    ));
                }
                if n.policy == Policy::Best && n.order.is_empty() {
                    issues.push(issue(
                        Code::MissingOrder,
                        format!("{p}.order"),
                        "best needs an explicit order",
                        "add order: [[clause, asc|desc]]",
                    ));
                }
                if n.tie.is_some() && n.policy != Policy::Nearest {
                    issues.push(issue(
                        Code::MissingOrder,
                        format!("{p}.tie"),
                        "tie applies to nearest only",
                        "drop tie",
                    ));
                }
                if let Some(on) = &n.on
                    && !exposed.bindings.iter().any(|b| b == on)
                {
                    issues.push(issue(
                        Code::UnknownField,
                        format!("{p}.on"),
                        format!("{on} is not a binding of this set"),
                        "bind it first",
                    ));
                }
                exposed.partners.insert(n.as_.clone(), n.set.clone());
            }
        }
    }
    for (i, a) in set.attach.iter().enumerate() {
        let p = format!("{}[{i}]", path("attach"));
        match ask.sets.get(&a.set) {
            None => issues.push(issue(
                Code::UnknownSet,
                format!("{p}.set"),
                format!("no set named {}", a.set),
                "name a set of this document",
            )),
            Some(partner) => {
                if !set.grain.is_ancestor_of(partner.grain) {
                    issues.push(issue(
                        Code::GrainMismatch,
                        format!("{p}.set"),
                        format!(
                            "{} is at grain {}, not below {}",
                            a.set, partner.grain, set.grain
                        ),
                        "attach names a descendant set",
                    ));
                } else {
                    let functional = partner.pick.as_ref().is_some_and(|pk| pk.per == set.grain);
                    if !functional {
                        issues.push(issue(
                            Code::NotFunctional,
                            format!("{p}.set"),
                            format!(
                                "{} may hold several rows per {}; attach takes one",
                                a.set, set.grain
                            ),
                            format!("give {} a pick: {{per: {}, by: [...]}}", a.set, set.grain),
                        ));
                    }
                }
                exposed.partners.insert(a.as_.clone(), a.set.clone());
            }
        }
    }
    for (i, h) in set.has.iter().enumerate() {
        let p = format!("{}[{i}]", path("has"));
        match ask.sets.get(&h.set) {
            None => issues.push(issue(
                Code::UnknownSet,
                format!("{p}.set"),
                format!("no set named {}", h.set),
                "name a set of this document",
            )),
            Some(child) => {
                let below = set.grain.is_ancestor_of(child.grain);
                let group_here = child.grain == Grain::Group
                    && so_far.sets.get(&h.set).is_some_and(|e| {
                        e.group_by
                            .iter()
                            .any(|b| b == &format!("{}.id", set.grain.name()))
                    });
                let same = child.grain == set.grain;
                if !(below || group_here || same) {
                    issues.push(issue(
                        Code::GrainMismatch,
                        format!("{p}.set"),
                        format!(
                            "{} is at grain {}, which this {} set cannot count",
                            h.set, child.grain, set.grain
                        ),
                        "has counts a descendant, a group keyed by this grain, or a same grain set",
                    ));
                }
                if h.window.is_some() && (!set.grain.dated() || !child.grain.dated()) {
                    issues.push(issue(
                        Code::NotDated,
                        format!("{p}.window"),
                        "a windowed count needs two dated grains",
                        "drop the window or count a dated set",
                    ));
                }
                if h.min.is_none() && h.max.is_none() && h.as_.is_none() {
                    issues.push(issue(
                        Code::MissingOrder,
                        p.clone(),
                        "has needs min, max or as",
                        "add min: 1 for exists",
                    ));
                }
                if let Some(as_) = &h.as_ {
                    exposed.bindings.push(as_.clone());
                }
            }
        }
    }

    // bind, then where, then pick, each reading what came before
    for (bname, c) in set.bind.0.iter() {
        let bpath = format!("{}.{bname}", path("bind"));
        if set.grain != Grain::Group && names.field(set.grain.name(), bname).is_some() {
            issues.push(issue(
                Code::AmbiguousPath,
                bpath.clone(),
                format!(
                    "{bname} is a field of {}; a binding may not shadow a field",
                    set.grain
                ),
                "rename the binding",
            ));
        }
        if exposed.bindings.iter().any(|b| b == bname) {
            issues.push(issue(
                Code::AmbiguousPath,
                bpath.clone(),
                format!("{bname} is already bound"),
                "rename the binding",
            ));
        }
        check_clause(
            c, &bpath, set, name, ask, so_far, &exposed, names, scope, issues,
        );
        if c.op == "change" {
            exposed.change_bindings.push(bname.clone());
        }
        exposed.bindings.push(bname.clone());
    }
    for (i, c) in set.where_.iter().enumerate() {
        check_clause(
            c,
            &format!("{}[{i}]", path("where")),
            set,
            name,
            ask,
            so_far,
            &exposed,
            names,
            scope,
            issues,
        );
    }
    if let Some(pk) = &set.pick {
        if pk.by.is_empty() {
            issues.push(issue(
                Code::MissingOrder,
                path("pick.by"),
                "a pick orders by an explicit preference list",
                "add by: [[clause, asc|desc]]",
            ));
        }
        if !pk.per.is_ancestor_of(set.grain) {
            issues.push(issue(
                Code::GrainMismatch,
                path("pick.per"),
                format!(
                    "per names an ancestor grain of {}; {} is not one",
                    set.grain, pk.per
                ),
                "pick per: session for one stack per session",
            ));
        }
        for (i, o) in pk.by.iter().enumerate() {
            check_clause(
                &o.0,
                &format!("{}[{i}]", path("pick.by")),
                set,
                name,
                ask,
                so_far,
                &exposed,
                names,
                scope,
                issues,
            );
        }
        exposed.picked = true;
    }
    exposed
}

/// A path's first segment, when it names a level whose fields a set of
/// this grain carries (§4.2: the ancestor keys and fields).
fn level_prefix(grain: Grain, first: &str) -> bool {
    let ancestors: &[&str] = match grain {
        Grain::Cohort => &[],
        Grain::Subject => &["cohort", "subject"],
        Grain::Session => &["subject", "session"],
        Grain::Stack => &["subject", "study", "series", "session", "stack"],
        Grain::Instance => &["subject", "study", "series", "session", "stack", "instance"],
        Grain::Event => &["subject", "event"],
        Grain::Group | Grain::Pair => &[],
    };
    ancestors.contains(&first)
}

/// The exposed names of `near` and `attach` partners beyond their sets'
/// own fields and bindings.
const PARTNER_EXTRAS: &[&str] = &["date", "precision", "offset_days", "tied", "candidates"];
const PICK_EXTRAS: &[&str] = &["tied", "candidates", "rank"];

/// Resolve a `field` path in the namespace of a set: its own fields, its
/// bindings, `<as>.<...>` of a partner, `<of set>.<...>` of the ancestor,
/// `<level>.<field>` of a carried ancestor, `pick.*`, and a group's names.
#[allow(clippy::too_many_arguments)]
fn resolve_field(
    path: &str,
    set: &Set,
    set_name: &str,
    ask: &Ask,
    so_far: &Validated,
    exposed: &Exposed,
    names: &dyn Names,
    depth: u8,
) -> Result<Option<FieldInfo>, String> {
    if depth > 6 {
        return Err(format!("{path} is too deep"));
    }
    let grain = set.grain;
    // a binding, by its whole name (bindings may carry dots: comparable.largest)
    if exposed.bindings.iter().any(|b| b == path) {
        return Ok(None);
    }
    // a change pair's fields under its binding's name
    for b in &exposed.change_bindings {
        if let Some(rest) = path
            .strip_prefix(b.as_str())
            .and_then(|r| r.strip_prefix('.'))
        {
            return if matches!(rest, "from_date" | "to_date" | "precision" | "gap_days") {
                Ok(None)
            } else {
                Err(format!(
                    "{b} is a change pair; it exposes from_date, to_date, precision and gap_days"
                ))
            };
        }
    }
    if grain == Grain::Group {
        if exposed.group_by.iter().any(|b| b == path) || path == "_rows" || path == "_subjects" {
            return Ok(None);
        }
        // a group's by paths keep their names, so `cohort.id` on a group is
        // the by field
        return Err(format!(
            "{path} is not a by field, an aggregate, _rows or _subjects of group {set_name}"
        ));
    }
    let (first, rest) = match path.split_once('.') {
        Some((f, r)) => (f, Some(r)),
        None => (path, None),
    };
    // pick.*
    if first == "pick" {
        return match rest {
            Some(r) if PICK_EXTRAS.contains(&r) && exposed.picked => Ok(None),
            Some(r) if PICK_EXTRAS.contains(&r) => {
                Err(format!("{set_name} has no pick, so pick.{r} is nothing"))
            }
            _ => Err(format!("pick exposes {}", PICK_EXTRAS.join(", "))),
        };
    }
    // a partner: <as>.<rest>
    if let Some(partner_name) = exposed.partners.get(first) {
        let Some(rest) = rest else {
            return Err(format!(
                "{first} is a partner; name one of its fields, {first}.<field>"
            ));
        };
        if PARTNER_EXTRAS.contains(&rest) {
            return Ok(None);
        }
        let partner = ask
            .sets
            .get(partner_name)
            .ok_or_else(|| format!("no set named {partner_name}"))?;
        let partner_exposed = so_far.sets.get(partner_name).cloned().unwrap_or_default();
        return resolve_field(
            rest,
            partner,
            partner_name,
            ask,
            so_far,
            &partner_exposed,
            names,
            depth + 1,
        );
    }
    // the of ancestor: <of set>.<rest>
    if let Some(of) = &exposed.of
        && of == first
    {
        let Some(rest) = rest else {
            return Err(format!(
                "{first} is the ancestor set; name one of its fields, {first}.<field>"
            ));
        };
        let ancestor = ask
            .sets
            .get(of)
            .ok_or_else(|| format!("no set named {of}"))?;
        let ancestor_exposed = so_far.sets.get(of).cloned().unwrap_or_default();
        return resolve_field(
            rest,
            ancestor,
            of,
            ask,
            so_far,
            &ancestor_exposed,
            names,
            depth + 1,
        );
    }
    // a carried level: subject.birth_date, study.id, cohort.id
    if let Some(rest) = rest
        && level_prefix(grain, first)
    {
        if first == "cohort" && grain == Grain::Subject {
            // the cohort edge exposes the cohort key alone (rule 4)
            return if rest == "id" {
                Ok(None)
            } else {
                Err("the cohort edge exposes cohort.id alone".into())
            };
        }
        return match names.field(first, rest) {
            Some(info) => Ok(Some(info)),
            None => Err(format!("{first} has no field {rest}")),
        };
    }
    // the set's own grain
    if let Some(info) = names.field(grain.name(), path) {
        return Ok(Some(info));
    }
    if let Some(r) = rest
        && level_prefix(grain, first)
    {
        return Err(format!("{first} has no field {r}"));
    }
    Err(format!(
        "{path} is not a field of {grain}, a binding, a partner or an ancestor of {set_name}"
    ))
}

#[allow(clippy::too_many_arguments)]
fn check_clause(
    c: &Clause,
    path: &str,
    set: &Set,
    set_name: &str,
    ask: &Ask,
    so_far: &Validated,
    exposed: &Exposed,
    names: &dyn Names,
    scope: &Scope,
    issues: &mut Vec<Issue>,
) {
    let mut all: Vec<&Clause> = Vec::new();
    c.walk(&mut all);
    // an equality on an axis checks the value against the pack
    for cl in &all {
        if (COMPARISONS.contains(&cl.op.as_str()) || cl.op == "has")
            && let (Some(Arg::Clause(l)), Some(r)) = (cl.args.first(), cl.args.get(1))
            && l.op == "axis"
            && let Some(axis) = l.ref_name()
            && let Some(values) = names.axis_values(axis)
        {
            let literals: Vec<&str> = match r {
                Arg::Text(t) => vec![t.as_str()],
                Arg::List(items) => items.iter().filter_map(Arg::as_text).collect(),
                _ => Vec::new(),
            };
            for v in literals {
                if !values.iter().any(|x| x == v) {
                    issues.push(issue(
                        Code::UnknownValue,
                        path,
                        format!("{v} is not a value of axis {axis}"),
                        format!("GET /api/ask/catalog for the values of {axis}"),
                    ));
                }
            }
        }
    }
    // an event kind named by its literal is checked against the vocabulary,
    // and a sensitive kind is refused without the class (rule 15)
    if set.grain == Grain::Event {
        for cl in &all {
            if (cl.op == "=" || cl.op == "in")
                && let (Some(Arg::Clause(l)), Some(r)) = (cl.args.first(), cl.args.get(1))
                && l.op == "field"
                && l.ref_name() == Some("kind")
            {
                let literals: Vec<&str> = match r {
                    Arg::Text(t) => vec![t.as_str()],
                    Arg::List(items) => items.iter().filter_map(Arg::as_text).collect(),
                    _ => Vec::new(),
                };
                for name in literals {
                    match names.kind(name) {
                        None => issues.push(issue(Code::UnknownValue, path, format!("{name} is not an observation kind the registry holds"), "GET /api/ask/catalog for the kinds")),
                        Some(k) if k.sensitive && !scope.classes.contains(&Class::Sensitive) => issues.push(issue(Code::ForbiddenField, path, format!("{name} is a sensitive kind and this principal holds no class for it"), "ask for the class, or drop the kind")),
                        Some(_) => {}
                    }
                }
            }
        }
    }
    for cl in all {
        let op = cl.op.as_str();
        match op {
            "field" => {
                let Some(p) = cl.ref_name() else {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        "a field ref names a path",
                        "[\"field\", {}, \"<path>\"]",
                    ));
                    continue;
                };
                match resolve_field(p, set, set_name, ask, so_far, exposed, names, 0) {
                    Ok(Some(info)) => check_class(&info, p, path, scope, issues),
                    Ok(None) => {}
                    Err(m) => issues.push(issue(
                        Code::UnknownField,
                        path,
                        m,
                        "GET /api/ask/options for what this set exposes",
                    )),
                }
            }
            "axis" => {
                let Some(a) = cl.ref_name() else {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        "an axis ref names an axis",
                        "[\"axis\", {}, \"<name>\"]",
                    ));
                    continue;
                };
                if set.grain != Grain::Stack {
                    issues.push(issue(
                        Code::GrainMismatch,
                        path,
                        format!(
                            "axis {a} is a stack fact; this set is at grain {}",
                            set.grain
                        ),
                        "read it on a stack set and has or attach it",
                    ));
                } else if names.axis_values(a).is_none() {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        format!("no axis named {a} in the pack"),
                        "GET /api/ask/catalog for the axes",
                    ));
                }
            }
            "derived" => {
                let Some(d) = cl.ref_name() else {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        "a derived ref names a derived field",
                        "[\"derived\", {params}, \"<name>\"]",
                    ));
                    continue;
                };
                match names.derived(d) {
                    None => issues.push(issue(
                        Code::UnknownField,
                        path,
                        format!("no derived field named {d}"),
                        "GET /api/ask/catalog for the derived fields",
                    )),
                    Some(info) => {
                        if info.grain != set.grain {
                            issues.push(issue(
                                Code::GrainMismatch,
                                path,
                                format!(
                                    "{d} is derived at grain {}; this set is at {}",
                                    info.grain, set.grain
                                ),
                                "read it at its grain",
                            ));
                        }
                        for k in cl.opts.keys() {
                            if !info.params.iter().any(|p| p == k) {
                                issues.push(issue(
                                    Code::UnknownField,
                                    path,
                                    format!(
                                        "{d} takes no parameter {k}; it takes {}",
                                        info.params.join(", ")
                                    ),
                                    "drop the option",
                                ));
                            }
                        }
                        if d == "signature"
                            && let Some(Value::String(level)) = cl.opts.get("level")
                            && !names.level(level)
                        {
                            issues.push(issue(
                                Code::UnknownLevel,
                                path,
                                format!("{level} is not a comparability level of the pack"),
                                "GET /api/ask/catalog for the levels",
                            ));
                        }
                    }
                }
            }
            "param" => {
                let Some(p) = cl.ref_name() else {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        "a param ref names a parameter",
                        "[\"param\", {}, \"<name>\"]",
                    ));
                    continue;
                };
                match ask.params.get(p) {
                    None => issues.push(issue(Code::UnknownField, path, format!("no parameter named {p}"), "declare it under params")),
                    Some(decl) if decl.type_.structural() => issues.push(issue(Code::UnknownField, path, format!("{p} is a structural parameter and desugars into the document; it may not be read as a scalar here"), "desugar first")),
                    Some(_) => {}
                }
            }
            "change" => {
                if set.grain != Grain::Subject {
                    issues.push(issue(
                        Code::GrainMismatch,
                        path,
                        "change is a subject's course or event history",
                        "bind it on a subject set",
                    ));
                }
                for k in ["of", "from", "to"] {
                    if !cl.opts.contains_key(k)
                        && !cl
                            .args
                            .iter()
                            .any(|a| matches!(a, Arg::Clause(c) if c.op == "param"))
                    {
                        issues.push(issue(
                            Code::UnknownField,
                            path,
                            format!(
                                "change takes {{of, from, to, disease?, adjacent?}}; {k} is missing"
                            ),
                            "add it",
                        ));
                    }
                }
                if let Some(Value::String(of)) = cl.opts.get("of")
                    && of != "course"
                    && names.kind(of).is_none()
                {
                    issues.push(issue(
                        Code::UnknownField,
                        path,
                        format!("change of {of}: neither course nor an event kind"),
                        "GET /api/ask/catalog for the kinds",
                    ));
                }
            }
            "share" => match cl.opts.get("over").and_then(Value::as_str) {
                None => issues.push(issue(
                    Code::UnknownSet,
                    path,
                    "share names its denominator: {over: <set>}",
                    "add over",
                )),
                Some(over) if !ask.sets.contains_key(over) => issues.push(issue(
                    Code::UnknownSet,
                    path,
                    format!("no set named {over}"),
                    "name the denominator's set",
                )),
                Some(_) => {}
            },
            op if AGGREGATES.contains(&op) => {
                match cl.opts.get("set").and_then(Value::as_str) {
                    None => issues.push(issue(
                        Code::UnknownSet,
                        path,
                        format!("{op} aggregates over a set: {{set: <name>}}"),
                        "add set",
                    )),
                    Some(s) => match ask.sets.get(s) {
                        None => issues.push(issue(
                            Code::UnknownSet,
                            path,
                            format!("no set named {s}"),
                            "name a set of this document",
                        )),
                        Some(target) => {
                            let below = set.grain.is_ancestor_of(target.grain);
                            let group_here = target.grain == Grain::Group;
                            let same = set.grain == target.grain;
                            let own_child = set.grain == Grain::Group
                                && set.group.as_ref().is_some_and(|g| g.of == *s);
                            if !(below || group_here || same || own_child) {
                                issues.push(issue(
                                    Code::GrainMismatch,
                                    path,
                                    format!(
                                        "{op} over {s} at grain {} from a {} set",
                                        target.grain, set.grain
                                    ),
                                    "aggregate a descendant, a group or a same grain set",
                                ));
                            }
                            // the aggregated clause reads the target's names
                            if let Some(Arg::Clause(inner)) = cl.args.first() {
                                let target_exposed =
                                    so_far.sets.get(s).cloned().unwrap_or_default();
                                let mut inner_issues = Vec::new();
                                check_clause(
                                    inner,
                                    path,
                                    target,
                                    s,
                                    ask,
                                    so_far,
                                    &target_exposed,
                                    names,
                                    scope,
                                    &mut inner_issues,
                                );
                                issues.extend(inner_issues);
                            }
                        }
                    },
                }
            }
            op if ARITHMETIC.contains(&op)
                || COMPARISONS.contains(&op)
                || PREDICATES.contains(&op)
                || FUNCTIONS.contains(&op)
                || REFS.contains(&op) =>
            {
                if op == "~=" && !cl.opts.contains_key("tol") {
                    issues.push(issue(
                        Code::MissingOrder,
                        path,
                        "~= needs tol",
                        "add {tol: <number>}",
                    ));
                }
                if op == "part" && !cl.opts.contains_key("unit") {
                    issues.push(issue(
                        Code::MissingOrder,
                        path,
                        "part needs {unit: year | month | day | dow | hour}",
                        "add unit",
                    ));
                }
            }
            other => {
                issues.push(issue(
                    Code::UnknownField,
                    path,
                    format!("{other} is not an op of the language"),
                    "GET /api/ask/catalog for the function table",
                ));
            }
        }
        // an aggregate's inner clause was checked in the target's namespace
        if AGGREGATES.contains(&op) {
            break;
        }
    }
}

fn check_class(info: &FieldInfo, field: &str, path: &str, scope: &Scope, issues: &mut Vec<Issue>) {
    match info.class {
        Class::Identifying => issues.push(issue(Code::ForbiddenField, path, format!("{field} is an identifier; identifiers enter through values and leave through out.identifiers"), "use values or out.identifiers")),
        Class::Sensitive if !scope.classes.contains(&Class::Sensitive) => issues.push(issue(Code::ForbiddenField, path, format!("{field} is sensitive and this principal holds no class for it"), "ask for the class, or drop the field")),
        _ => {}
    }
    if scope.federated && !info.federated {
        issues.push(issue(
            Code::FederatedScope,
            path,
            format!("{field} is local and never leaves the node"),
            "run at home, or drop the field",
        ));
    }
}

/// Pin every bare `selection:<name>` to its current version (§8.2), inside
/// the document, so that the stored and hashed core says which question it
/// read. Returns what was pinned.
pub fn pin_selections(
    ask: &mut Ask,
    names: &dyn Names,
) -> Result<Vec<(String, String, u64)>, Issue> {
    let mut pinned = Vec::new();
    for (set_name, set) in ask.sets.iter_mut() {
        if let Some(Src::Selection { name, version }) = &mut set.from
            && version.is_none()
        {
            let current = names.selection(name).ok_or_else(|| {
                issue(
                    Code::UnknownSet,
                    format!("sets.{set_name}.from"),
                    format!("no selection named {name}"),
                    "GET /api/ask/selections",
                )
            })?;
            *version = Some(current);
            pinned.push((set_name.clone(), name.clone(), current));
        }
    }
    Ok(pinned)
}

/// Every level a path may start with, for the catalog's own listing.
pub fn levels() -> &'static [&'static str] {
    LEVELS
}
