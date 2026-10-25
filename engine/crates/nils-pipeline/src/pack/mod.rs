// SPDX-License-Identifier: AGPL-3.0-only

//! The pipeline pack (record 57 P1 to P4; Wave 7b spec section 5):
//! `nils-pack.yaml` beside `nils.job.yml`, under `contracts/pipeline-pack/v1`.
//!
//! The descriptor stays the knob layer; the pack holds what NILS adds: the
//! needs a session must meet (hard ones skip it in the pack's words, soft
//! ones travel as caveats), what each knob does and whether it breaks
//! comparability, the ordered rules that choose knob values or a variant
//! per session from the facts and the person's goal, the checks, the
//! licence, the resources, the comparability key's members, the tests and
//! the source. It pins the descriptor by the digest the catalog computes,
//! and **a pipeline version is the pair**: [`version_digest`] covers both.
//!
//! - [`expr`]: the expression language, parsed, typed and evaluated once
//!   for every reader.
//! - [`facts`]: the facts' names, types and vocabularies, and a unit's
//!   facts as the binary's provider fills them from the registry.
//! - [`rules`]: a unit judged at pre-flight: its needs, its knobs with who
//!   chose each, its variant, its caveats; and the replay that proves the
//!   record (G3).
//! - [`keys`]: the comparability key and the result key, and the words
//!   every door says where results of several keys would be pooled.
//!
//! [`load`] parses a pack and checks it against its descriptor: an unknown
//! fact, a wrong type, a value outside a fact's vocabulary, a knob the
//! descriptor has not, or a rule whose `set` would break the descriptor's
//! Boutiques constraints refuses the pack with a sentence that says which.

pub mod expr;
pub mod facts;
pub mod family;
pub mod keys;
pub mod rules;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::descriptor::{self, Descriptor, Param};
use expr::Expr;
use facts::{Goals, Vocabulary};

/// The contract this module reads.
pub const CONTRACT: &str = "pipeline-pack/v1";

/// The pack's file name beside `nils.job.yml`.
pub const FILE: &str = "nils-pack.yaml";

/// The descriptor's file name.
pub const DESCRIPTOR_FILE: &str = "nils.job.yml";

/// The roles a need may name beside the descriptor's: the links between
/// roles and the host.
pub const LINK: &str = "link";
pub const HOST: &str = "host";

/// One need of a role.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    pub when: Expr,
    /// As written.
    pub text: String,
    pub words: String,
    pub source: String,
}

/// A role's needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoleNeeds {
    /// Where given, the hard needs are hard only where it holds; where it
    /// does not, they are not checked (the SAMSEG example: lesions off
    /// needs no FLAIR).
    pub hard_if: Option<Expr>,
    pub hard: Vec<Need>,
    pub soft: Vec<Need>,
}

/// Whether results that differ in a knob compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparability {
    None,
    Additive,
    Breaks,
}

impl Comparability {
    pub fn name(self) -> &'static str {
        match self {
            Comparability::None => "none",
            Comparability::Additive => "additive",
            Comparability::Breaks => "breaks",
        }
    }
}

/// Who may set a knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetBy {
    Person,
    Rule,
    Fixed,
}

impl SetBy {
    pub fn name(self) -> &'static str {
        match self {
            SetBy::Person => "person",
            SetBy::Rule => "rule",
            SetBy::Fixed => "fixed",
        }
    }
}

/// What the pack says of one Boutiques input.
#[derive(Debug, Clone, PartialEq)]
pub struct Knob {
    pub what: Option<String>,
    pub default: Option<Value>,
    pub cost: Option<String>,
    pub changes: Vec<String>,
    pub comparability: Comparability,
    pub set_by: SetBy,
}

/// A sibling pipeline a rule may route a unit to.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub pipeline: String,
    pub words: Option<String>,
}

/// One rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub when: Expr,
    pub text: String,
    pub set: BTreeMap<String, Value>,
    pub route: Option<String>,
    pub why: Option<String>,
    pub source: Option<String>,
}

/// What a check does to a unit that breaks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Fail,
    Flag,
    Drop,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Fail => "fail",
            Action::Flag => "flag",
            Action::Drop => "drop",
        }
    }
}

/// The ops a check takes.
pub const CHECK_OPS: [&str; 6] = [">=", "<=", ">", "<", "==", "!="];

/// The strata a cohort check may be run within.
pub const STRATA: [&str; 5] = [
    "site",
    "vendor",
    "field_strength",
    "sequence",
    "post_contrast",
];

/// One check of the pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub metric: String,
    pub op: String,
    /// The threshold of a fixed check; none for a cohort check.
    pub value: Option<f64>,
    pub action: Action,
    pub words: Option<String>,
    pub source: Option<String>,
    pub cohort: bool,
    pub stratum: Vec<String>,
    pub min_sessions: usize,
    /// The robust z above which a cohort check flags.
    pub z: f64,
}

impl Check {
    /// Whether a value keeps a fixed check.
    pub fn holds(&self, v: f64) -> bool {
        let Some(t) = self.value else { return true };
        match self.op.as_str() {
            ">=" => v >= t,
            "<=" => v <= t,
            ">" => v > t,
            "<" => v < t,
            "==" => v == t,
            "!=" => v != t,
            _ => false,
        }
    }

    /// The check as a person writes it: `snr >= 8`, or for a cohort check
    /// `snr within its stratum (robust z at most 3)`.
    pub fn text(&self) -> String {
        match self.value {
            Some(v) => format!("{} {} {}", self.metric, self.op, descriptor::number_text(v)),
            None => format!(
                "{} within its stratum (robust z at most {})",
                self.metric,
                descriptor::number_text(self.z)
            ),
        }
    }
}

/// A pack, parsed and, after [`load`], checked against its descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelinePack {
    pub name: String,
    /// `sha256:<hex>`, as the catalog computes it.
    pub descriptor_sha256: String,
    pub version: Option<String>,
    pub needs: BTreeMap<String, RoleNeeds>,
    pub knobs: BTreeMap<String, Knob>,
    pub goals: Goals,
    pub variants: BTreeMap<String, Variant>,
    pub rules: Vec<Rule>,
    pub checks: Vec<Check>,
    pub licence: Value,
    pub resources: Value,
    pub comparability_knobs: Vec<String>,
    pub comparability_facts: Vec<String>,
    pub tests: Value,
    pub source: Value,
    /// The document as it parsed, kept whole.
    pub document: Value,
}

fn text(v: &Value, key: &str, at: &str) -> Result<String, String> {
    match &v[key] {
        Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
        Value::Null => Err(format!("{at}{key} is missing")),
        _ => Err(format!("{at}{key} is text")),
    }
}

fn opt_text(v: &Value, key: &str, at: &str) -> Result<Option<String>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(format!("{at}{key} is text")),
    }
}

fn expression(v: &Value, key: &str, at: &str) -> Result<(Expr, String), String> {
    let t = text(v, key, at)?;
    let e = expr::parse(&t).map_err(|e| format!("{at}{key}: {e}"))?;
    Ok((e, t))
}

fn need(v: &Value, at: &str) -> Result<Need, String> {
    if !v.is_object() {
        return Err(format!("{at} is a mapping: when, words, source"));
    }
    let (when, text_) = expression(v, "when", at)?;
    Ok(Need {
        when,
        text: text_,
        words: text(v, "words", at)?,
        source: text(v, "source", at)?,
    })
}

fn needs_list(v: &Value, key: &str, at: &str) -> Result<Vec<Need>, String> {
    match &v[key] {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, n)| need(n, &format!("{at}{key}[{i}]")))
            .collect(),
        _ => Err(format!("{at}{key} is a list of needs")),
    }
}

fn string_list(v: &Value, key: &str, at: &str) -> Result<Vec<String>, String> {
    match &v[key] {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{at}{key} is a list of names"))
            })
            .collect(),
        _ => Err(format!("{at}{key} is a list of names")),
    }
}


/// The `needs` of a document, per role (a pack's, or a family's: the
/// family document reads the same shape).
pub(crate) fn needs_of(document: &Value) -> Result<BTreeMap<String, RoleNeeds>, String> {
    let mut needs = BTreeMap::new();
    match &document["needs"] {
        Value::Null => {}
        Value::Object(roles) => {
            for (role, v) in roles {
                let at = format!("needs.{role}.");
                if !v.is_object() {
                    return Err(format!("needs.{role} is a mapping: hard_if, hard, soft"));
                }
                let hard_if = match &v["hard_if"] {
                    Value::Null => None,
                    _ => Some(expression(v, "hard_if", &at)?.0),
                };
                needs.insert(
                    role.clone(),
                    RoleNeeds {
                        hard_if,
                        hard: needs_list(v, "hard", &at)?,
                        soft: needs_list(v, "soft", &at)?,
                    },
                );
            }
        }
        _ => return Err("needs is a mapping of roles".into()),
    }
    Ok(needs)
}

/// The `goals` of a document: the goal's values and the switches.
pub(crate) fn goals_of(document: &Value) -> Result<Goals, String> {
    let mut goals = Goals::default();
    match &document["goals"] {
        Value::Null => {}
        Value::Object(map) => {
            for (k, v) in map {
                let Value::Array(items) = v else {
                    return Err(format!("goals.{k} is a list of its values"));
                };
                if items.is_empty() {
                    return Err(format!("goals.{k} names at least one value"));
                }
                if k == "goal" {
                    for g in items {
                        match g.as_str() {
                            Some(w) if facts::GOALS.contains(&w) => goals.goal.push(w.to_string()),
                            _ => {
                                return Err(format!(
                                    "goals.goal takes {}; not {g}",
                                    facts::GOALS.join(", ")
                                ));
                            }
                        }
                    }
                } else {
                    goals.switches.insert(k.clone(), items.clone());
                }
            }
        }
        _ => return Err("goals is a mapping: goal and the pack's switches".into()),
    }
    Ok(goals)
}

/// Parse a pack from its YAML (JSON is YAML) and check its shape and its
/// expressions' grammar; [`load`] checks it against its descriptor too.
pub fn parse(text_: &str) -> Result<PipelinePack, String> {
    let document: Value =
        serde_saphyr::from_str(text_).map_err(|e| format!("not a YAML document: {e}"))?;
    from_value(document)
}

/// [`parse`] of a document already read.
pub fn from_value(document: Value) -> Result<PipelinePack, String> {
    if !document.is_object() {
        return Err("a pack is a mapping".into());
    }
    match document["nils_pack"].as_i64() {
        Some(1) => {}
        Some(other) => {
            return Err(format!(
                "nils_pack: {other} is not a version this engine reads; it reads 1 ({CONTRACT})"
            ));
        }
        None => return Err("nils_pack: 1 is missing".into()),
    }
    let p = &document["pipeline"];
    if !p.is_object() {
        return Err("pipeline is a mapping: name, descriptor_sha256".into());
    }
    let name = text(p, "name", "pipeline.")?;
    let digest = text(p, "descriptor_sha256", "pipeline.")?;
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    if hex.len() != 64
        || !hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(
            "pipeline.descriptor_sha256 is the descriptor's digest, sha256:<64 lowercase hex>, as the catalog computes it (nils pipelines validate <dir> prints it)".into(),
        );
    }
    let descriptor_sha256 = format!("sha256:{hex}");
    let version = opt_text(p, "version", "pipeline.")?;

    let needs = needs_of(&document)?;

    let mut knobs = BTreeMap::new();
    match &document["knobs"] {
        Value::Null => {}
        Value::Object(map) => {
            for (id, v) in map {
                let at = format!("knobs.{id}.");
                if !v.is_object() {
                    return Err(format!("knobs.{id} is a mapping"));
                }
                let comparability = match v["comparability"].as_str() {
                    None | Some("none") => Comparability::None,
                    Some("additive") => Comparability::Additive,
                    Some("breaks") => Comparability::Breaks,
                    Some(other) => {
                        return Err(format!(
                            "{at}comparability is none, additive or breaks, not {other}"
                        ));
                    }
                };
                let set_by = match v["set_by"].as_str() {
                    None | Some("person") => SetBy::Person,
                    Some("rule") => SetBy::Rule,
                    Some("fixed") => SetBy::Fixed,
                    Some(other) => {
                        return Err(format!("{at}set_by is person, rule or fixed, not {other}"));
                    }
                };
                knobs.insert(
                    id.clone(),
                    Knob {
                        what: opt_text(v, "what", &at)?,
                        default: (!v["default"].is_null()).then(|| v["default"].clone()),
                        cost: opt_text(v, "cost", &at)?,
                        changes: string_list(v, "changes", &at)?,
                        comparability,
                        set_by,
                    },
                );
            }
        }
        _ => return Err("knobs is a mapping of the descriptor's inputs".into()),
    }

    let goals = goals_of(&document)?;

    let mut variants = BTreeMap::new();
    match &document["variants"] {
        Value::Null => {}
        Value::Object(map) => {
            for (k, v) in map {
                let at = format!("variants.{k}.");
                variants.insert(
                    k.clone(),
                    Variant {
                        pipeline: text(v, "pipeline", &at)?,
                        words: opt_text(v, "words", &at)?,
                    },
                );
            }
        }
        _ => return Err("variants is a mapping of sibling pipelines".into()),
    }

    let mut rules = Vec::new();
    match &document["rules"] {
        Value::Null => {}
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let at = format!("rules[{i}].");
                let id = text(v, "id", &at)?;
                if rules.iter().any(|r: &Rule| r.id == id) {
                    return Err(format!("rules[{i}].id {id} is used twice"));
                }
                let (when, text_) = expression(v, "when", &at)?;
                let set: BTreeMap<String, Value> = match &v["set"] {
                    Value::Null => BTreeMap::new(),
                    Value::Object(m) => m.iter().map(|(k, x)| (k.clone(), x.clone())).collect(),
                    _ => return Err(format!("{at}set is a mapping of knob values")),
                };
                let route = opt_text(v, "route", &at)?;
                if set.is_empty() && route.is_none() {
                    return Err(format!("rule {id} sets no knob and routes nowhere"));
                }
                rules.push(Rule {
                    id,
                    when,
                    text: text_,
                    set,
                    route,
                    why: opt_text(v, "why", &at)?,
                    source: opt_text(v, "source", &at)?,
                });
            }
        }
        _ => return Err("rules is a list".into()),
    }

    let mut checks = Vec::new();
    match &document["checks"] {
        Value::Null => {}
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let at = format!("checks[{i}].");
                let op = text(v, "op", &at)?;
                if !CHECK_OPS.contains(&op.as_str()) {
                    return Err(format!("{at}op is one of {}", CHECK_OPS.join(", ")));
                }
                let action = match v["action"].as_str() {
                    Some("fail") => Action::Fail,
                    Some("flag") => Action::Flag,
                    Some("drop") => Action::Drop,
                    _ => return Err(format!("{at}action is fail, flag or drop")),
                };
                let cohort = v["cohort"].as_bool().unwrap_or(false);
                let value = match &v["value"] {
                    Value::Null if cohort => None,
                    Value::Null => return Err(format!("{at}value is the check's threshold")),
                    n => Some(n.as_f64().ok_or_else(|| format!("{at}value is a number"))?),
                };
                let stratum = string_list(v, "stratum", &at)?;
                for s in &stratum {
                    if !STRATA.contains(&s.as_str()) {
                        return Err(format!("{at}stratum takes {}; not {s}", STRATA.join(", ")));
                    }
                }
                let min_sessions = match &v["min_sessions"] {
                    Value::Null => 30,
                    n => n
                        .as_u64()
                        .filter(|n| *n >= 2)
                        .ok_or_else(|| format!("{at}min_sessions is a count, 2 or more"))?
                        as usize,
                };
                let z = match &v["z"] {
                    Value::Null => 3.0,
                    n => n
                        .as_f64()
                        .filter(|z| *z > 0.0)
                        .ok_or_else(|| format!("{at}z is a number above 0"))?,
                };
                checks.push(Check {
                    metric: text(v, "metric", &at)?,
                    op,
                    value,
                    action,
                    words: opt_text(v, "words", &at)?,
                    source: opt_text(v, "source", &at)?,
                    cohort,
                    stratum: if cohort && stratum.is_empty() {
                        STRATA.iter().map(|s| s.to_string()).collect()
                    } else {
                        stratum
                    },
                    min_sessions,
                    z,
                });
            }
        }
        _ => return Err("checks is a list".into()),
    }

    let comparability_knobs = string_list(&document["comparability"], "knobs", "comparability.")?;
    let comparability_facts = string_list(&document["comparability"], "facts", "comparability.")?;
    Ok(PipelinePack {
        name,
        descriptor_sha256,
        version,
        needs,
        knobs,
        goals,
        variants,
        rules,
        checks,
        licence: document["licence"].clone(),
        resources: document["resources"].clone(),
        comparability_knobs,
        comparability_facts,
        tests: document["tests"].clone(),
        source: document["source"].clone(),
        document,
    })
}

impl PipelinePack {
    /// The digest of the pack: sha256 of its canonical JSON.
    pub fn digest(&self) -> String {
        crate::sha256(crate::canonical(&self.document).as_bytes())
    }

    /// The vocabulary this pack's expressions are checked against: the
    /// descriptor's roles and the pack's goals.
    pub fn vocabulary(&self, d: &Descriptor) -> Vocabulary {
        Vocabulary::new(&d.roles, &self.goals)
    }

    /// The knobs whose comparability is `breaks`, and those the pack names
    /// under `comparability.knobs`.
    pub fn breaking_knobs(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .knobs
            .iter()
            .filter(|(_, k)| k.comparability == Comparability::Breaks)
            .map(|(id, _)| id.clone())
            .collect();
        for k in &self.comparability_knobs {
            if !out.contains(k) {
                out.push(k.clone());
            }
        }
        out.sort();
        out
    }

    /// The default goal and switches, as the person's facts, where nobody
    /// chose.
    pub fn default_choices(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        out.insert("goal".into(), json!(self.goals.default_goal()));
        for (k, values) in &self.goals.switches {
            out.insert(k.clone(), values[0].clone());
        }
        out
    }

    /// A person's goal or switch value, checked against the pack's goals.
    pub fn choice(&self, name: &str, value: &Value) -> Result<Value, String> {
        if name == "goal" {
            let v = value.as_str().unwrap_or_default();
            let offered: Vec<String> = if self.goals.goal.is_empty() {
                facts::GOALS.iter().map(|g| g.to_string()).collect()
            } else {
                self.goals.goal.clone()
            };
            return if offered.iter().any(|g| g == v) {
                Ok(value.clone())
            } else {
                Err(format!(
                    "the goal is one of {}, not {v}",
                    offered.join(", ")
                ))
            };
        }
        let Some(values) = self.goals.switches.get(name) else {
            return Err(format!(
                "{} has no switch {name}; its switches are {}",
                self.name,
                if self.goals.switches.is_empty() {
                    "none".to_string()
                } else {
                    self.goals
                        .switches
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ));
        };
        let same = values
            .iter()
            .find(|v| *v == value || facts::text_of(v) == facts::text_of(value));
        same.cloned().ok_or_else(|| {
            format!(
                "{name} is one of {}, not {}",
                values
                    .iter()
                    .map(facts::text_of)
                    .collect::<Vec<_>>()
                    .join(", "),
                facts::text_of(value)
            )
        })
    }

    /// Check the pack against its descriptor and the vocabulary: the
    /// digest it pins, every expression's type, every knob and variant it
    /// names, and every rule's `set` against the descriptor's constraints.
    pub fn check(&self, d: &Descriptor, vocab: &Vocabulary) -> Result<(), String> {
        if self.name != d.name {
            return Err(format!(
                "the pack is of {}, and the descriptor beside it is {}",
                self.name, d.name
            ));
        }
        let digest = d.digest();
        if self.descriptor_sha256 != digest {
            return Err(format!(
                "the pack pins descriptor {}, and the descriptor beside it is {digest}: the descriptor changed after the pack was written, so write the new digest into pipeline.descriptor_sha256 (nils pipelines validate <dir> prints it)",
                self.descriptor_sha256
            ));
        }
        for (role, rn) in &self.needs {
            let scope = (role != LINK && role != HOST).then_some(role.as_str());
            if let Some(s) = scope {
                vocab.role(s).map_err(|e| format!("needs.{role}: {e}"))?;
            }
            if let Some(h) = &rn.hard_if {
                typed(h, scope, vocab, &format!("needs.{role}.hard_if"))?;
            }
            for (kind, list) in [("hard", &rn.hard), ("soft", &rn.soft)] {
                for (i, n) in list.iter().enumerate() {
                    typed(
                        &n.when,
                        scope,
                        vocab,
                        &format!("needs.{role}.{kind}[{i}].when"),
                    )?;
                }
            }
        }
        for id in self.knobs.keys() {
            if !d.params.iter().any(|p| &p.id == id) {
                return Err(format!(
                    "knobs.{id}: {} has no input {id}; its inputs are {}",
                    d.name,
                    params_named(d)
                ));
            }
        }
        for (id, k) in &self.knobs {
            if let Some(v) = &k.default {
                let p = d
                    .params
                    .iter()
                    .find(|p| &p.id == id)
                    .expect("checked above");
                descriptor::check_value(p, v).map_err(|e| format!("knobs.{id}.default: {e}"))?;
            }
        }
        for k in &self.comparability_knobs {
            if !d.params.iter().any(|p| &p.id == k) {
                return Err(format!(
                    "comparability.knobs names {k}, which {} has no input of",
                    d.name
                ));
            }
        }
        for f in &self.comparability_facts {
            if !vocab.facts.contains_key(f) {
                return Err(format!("comparability.facts names {f}, which is no fact"));
            }
        }
        let defaults = d.resolve(&[])?;
        for r in &self.rules {
            let at = format!("rule {}", r.id);
            typed(&r.when, None, vocab, &format!("{at}: when"))?;
            if let Some(v) = &r.route
                && !self.variants.contains_key(v)
            {
                return Err(format!("{at} routes to {v}, which variants does not name"));
            }
            if r.set.is_empty() {
                continue;
            }
            let mut params = defaults.clone();
            for (k, v) in &r.set {
                let Some(p) = d.params.iter().find(|p| &p.id == k) else {
                    return Err(format!(
                        "{at} sets {k}, which {} has no input of; its inputs are {}",
                        d.name,
                        params_named(d)
                    ));
                };
                if let Some(knob) = self.knobs.get(k)
                    && knob.set_by == SetBy::Fixed
                {
                    return Err(format!(
                        "{at} sets {k}, which the pack fixes (set_by: fixed)"
                    ));
                }
                descriptor::check_value(p, v).map_err(|e| {
                    format!("{at} sets {k} to {v}, which the descriptor forbids: {e}")
                })?;
                params.insert(k.clone(), v.clone());
            }
            invocation_holds(d, &params).map_err(|e| {
                format!(
                    "{at} sets {}, which the descriptor forbids: {e}",
                    set_words(&r.set)
                )
            })?;
        }
        for (i, c) in self.checks.iter().enumerate() {
            let known = d
                .outputs
                .iter()
                .filter_map(|o| o.table.as_ref())
                .flat_map(|t| t.columns.iter().map(|c| c.name.as_str()))
                .any(|n| n == c.metric);
            // a metric may also come from results.json, which no descriptor
            // declares, so an unknown one is not refused; a check on a
            // declared text column is
            let is_text = d
                .outputs
                .iter()
                .filter_map(|o| o.table.as_ref())
                .flat_map(|t| t.columns.iter())
                .any(|col| col.name == c.metric && !col.ty.numeric());
            if known && is_text {
                return Err(format!(
                    "checks[{i}] holds {} to a number, and {} declares it as text",
                    c.metric, d.name
                ));
            }
        }
        Ok(())
    }
}

fn typed(e: &Expr, scope: Option<&str>, vocab: &Vocabulary, at: &str) -> Result<(), String> {
    let scoped = e.scoped(scope, vocab);
    match expr::check(&scoped, vocab) {
        Ok(expr::Ty::Bool) => Ok(()),
        Ok(other) => Err(format!("{at}: `{e}` is {other}, not true or false")),
        Err(err) => Err(format!("{at}: {err}")),
    }
}

fn params_named(d: &Descriptor) -> String {
    if d.params.is_empty() {
        "none".into()
    } else {
        d.params
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn set_words(set: &BTreeMap<String, Value>) -> String {
    set.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse a pack and check it against the descriptor beside it, under the
/// vocabulary given (the engine's, widened by the MRI pack's words) or the
/// pack's own.
pub fn load(
    text_: &str,
    d: &Descriptor,
    vocab: Option<&Vocabulary>,
) -> Result<PipelinePack, String> {
    let pack = parse(text_)?;
    let own;
    let vocab = match vocab {
        Some(v) => v,
        None => {
            own = pack.vocabulary(d);
            &own
        }
    };
    pack.check(d, vocab)?;
    Ok(pack)
}

/// A pipeline's folder read: its descriptor, and its pack where
/// `nils-pack.yaml` lies beside the descriptor, each checked.
pub fn load_dir(
    dir: &Path,
    vocab: Option<&Vocabulary>,
) -> Result<(Descriptor, Option<PipelinePack>), String> {
    let d_path = dir.join(DESCRIPTOR_FILE);
    let text_ =
        std::fs::read_to_string(&d_path).map_err(|e| format!("{}: {e}", d_path.display()))?;
    let d = descriptor::parse(&text_).map_err(|e| format!("{}: {e}", d_path.display()))?;
    let p_path = dir.join(FILE);
    if !p_path.is_file() {
        return Ok((d, None));
    }
    let p_text =
        std::fs::read_to_string(&p_path).map_err(|e| format!("{}: {e}", p_path.display()))?;
    let pack = load(&p_text, &d, vocab).map_err(|e| format!("{}: {e}", p_path.display()))?;
    Ok((d, Some(pack)))
}

/// The digest of a pipeline version, the pair: sha256 of the canonical
/// JSON `{descriptor, pack}` of the two digests, `pack` null for a version
/// with no pack. Two versions with the same descriptor and different packs
/// differ, as do two with the same pack and different descriptors.
pub fn version_digest(descriptor_digest: &str, pack_digest: Option<&str>) -> String {
    crate::sha256(
        crate::canonical(&json!({"descriptor": descriptor_digest, "pack": pack_digest})).as_bytes(),
    )
}

/// Whether a parameter is given, as Boutiques reads it for its constraints:
/// a flag that is true, or any other value that is not null.
fn given(v: Option<&Value>) -> bool {
    !matches!(v, None | Some(Value::Null) | Some(Value::Bool(false)))
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Whether an invocation keeps the descriptor's Boutiques constraints: each
/// value its input's type, range and choices; `requires-inputs` and
/// `disables-inputs` of every input given; and the `groups`'
/// `mutually-exclusive`, `one-is-required` and `all-or-none`. The sentence
/// says which constraint fails.
pub fn invocation_holds(d: &Descriptor, params: &Map<String, Value>) -> Result<(), String> {
    for p in &d.params {
        if let Some(v) = params.get(&p.id)
            && !v.is_null()
        {
            descriptor::check_value(p, v)?;
        }
    }
    let inputs: Vec<&Value> = d.document["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let on = |id: &str| given(params.get(id));
    for input in &inputs {
        let Some(id) = input["id"].as_str() else {
            continue;
        };
        if !on(id) {
            continue;
        }
        for r in ids(&input["requires-inputs"]) {
            if !on(&r) {
                return Err(format!("{id} requires {r}, which is not given"));
            }
        }
        for x in ids(&input["disables-inputs"]) {
            if on(&x) {
                return Err(format!("{id} disables {x}, which is given too"));
            }
        }
    }
    for g in d.document["groups"].as_array().into_iter().flatten() {
        let members = ids(&g["members"]);
        let name = g["id"].as_str().or(g["name"].as_str()).unwrap_or("a group");
        let given_n = members.iter().filter(|m| on(m)).count();
        if g["mutually-exclusive"].as_bool() == Some(true) && given_n > 1 {
            return Err(format!(
                "{name} takes one of {}, and {given_n} are given",
                members.join(", ")
            ));
        }
        if g["one-is-required"].as_bool() == Some(true) && given_n == 0 {
            return Err(format!(
                "{name} needs one of {}, and none is given",
                members.join(", ")
            ));
        }
        if g["all-or-none"].as_bool() == Some(true) && given_n != 0 && given_n != members.len() {
            return Err(format!(
                "{name} takes all of {} or none, and {given_n} are given",
                members.join(", ")
            ));
        }
    }
    Ok(())
}

/// The parameter a knob names, where the descriptor has it.
pub fn param_of<'a>(d: &'a Descriptor, id: &str) -> Option<&'a Param> {
    d.params.iter().find(|p| p.id == id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A descriptor with two inputs and the Boutiques constraints the
    /// loader checks: `lesion` on or off, `pallidum_separate` only with the
    /// lesions, `threads` a bounded whole number, and a group of one.
    pub(crate) const DESCRIPTOR: &str = r#"name: samseg-test
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/samseg@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
inputs:
  - {id: lesion, name: Lesions, type: String, value-key: "[LESION]", value-choices: ["on", "off"], default-value: "on"}
  - {id: pallidum_separate, name: Pallidum, type: Flag, value-key: "[PALLIDUM]", command-line-flag: "--pallidum-separate", default-value: true, requires-inputs: [lesion]}
  - {id: threads, name: Threads, type: Number, value-key: "[THREADS]", default-value: 4, integer: true, minimum: 1, maximum: 32}
  - {id: fast, name: Fast, type: Flag, value-key: "[FAST]", command-line-flag: "--fast", disables-inputs: [pallidum_separate]}
groups:
  - {id: speed, members: [fast, threads], mutually-exclusive: false}
command-line: "run [LESION] [PALLIDUM] [THREADS] [FAST]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w, flair]}
  units: apart
  outputs:
    - id: volumes
      kind: table
      format: json
      path-template: "sub-{subject}/ses-{session}/anat/*_volumes.json"
      columns:
        - {name: intracranial, unit: mm3}
        - {name: flair_t1_brain_dice}
        - {name: note, type: text}
"#;

    pub(crate) fn pack_text(digest: &str) -> String {
        format!(
            r#"nils_pack: 1
pipeline: {{name: samseg-test, descriptor_sha256: "{digest}", version: "0.1"}}
goals: {{goal: [fast, standard, research], lesions: ["on", "off"]}}
needs:
  t1w:
    hard:
      - {{when: "dimension == '3D' and slice_thickness_mm <= 1.5", words: "the T1 is 2D or thicker than 1.5 mm; SAMSEG wants a 3D T1 at 1.5 mm or finer", source: policy}}
    soft:
      - {{when: "field_strength_t >= 1.5", words: "the T1 was acquired below 1.5 T", source: policy}}
  flair:
    hard_if: "lesions == 'on'"
    hard: [{{when: "present(flair)", words: "no FLAIR picked, so no lesions", source: "a paper"}}]
  link:
    hard: [{{when: "aligned(flair, t1w)", words: "the FLAIR is not registered to the T1w", source: "a paper"}}]
  host:
    soft: [{{when: "host.memory_gb >= 16", words: "the host has less than 16 GB", source: policy}}]
knobs:
  lesion: {{what: "segment lesions from the FLAIR", comparability: breaks, set_by: rule}}
  pallidum_separate: {{comparability: breaks, set_by: rule}}
  threads: {{comparability: none, set_by: person}}
  fast: {{comparability: additive, set_by: person}}
rules:
  - {{id: lesions-off-without-flair, when: "not present(flair)", set: {{lesion: "off"}}, why: "no FLAIR, so no lesions", source: policy}}
  - {{id: lesions-follow-the-switch, when: "lesions == 'off'", set: {{lesion: "off"}}, source: policy}}
  - {{id: pallidum-needs-t2-or-flair, when: "not present(flair) and not present(t2w)", set: {{pallidum_separate: false}}, source: "a paper"}}
  - {{id: fast-goal-fewer-threads, when: "goal == 'fast'", set: {{threads: 2}}, source: policy}}
checks:
  - {{metric: flair_t1_brain_dice, op: ">=", value: 0.85, action: flag, source: policy}}
  - {{metric: intracranial, op: ">=", value: 900000, action: fail, source: policy}}
  - {{metric: intracranial, op: "<=", action: flag, cohort: true, source: policy}}
comparability: {{facts: [t1w.base]}}
"#
        )
    }

    pub(crate) fn descriptor() -> Descriptor {
        descriptor::parse(DESCRIPTOR).unwrap()
    }

    pub(crate) fn loaded() -> (Descriptor, PipelinePack) {
        let d = descriptor();
        let p = load(&pack_text(&d.digest()), &d, None).unwrap_or_else(|e| panic!("{e}"));
        (d, p)
    }

    #[test]
    fn a_good_pack_loads_and_its_digests_tell_the_pair_apart() {
        let (d, p) = loaded();
        assert_eq!(p.name, "samseg-test");
        assert_eq!(p.rules.len(), 4);
        assert_eq!(p.breaking_knobs(), vec!["lesion", "pallidum_separate"]);
        assert_eq!(p.goals.default_goal(), "standard");
        assert_eq!(p.default_choices()["lesions"], "on");
        let dd = d.digest();
        let a = version_digest(&dd, None);
        let b = version_digest(&dd, Some(&p.digest()));
        assert_ne!(a, b);
        assert_eq!(a, version_digest(&dd, None));
        assert!(p.digest().starts_with("sha256:"));
        assert_eq!(
            p.checks[2].text(),
            "intracranial within its stratum (robust z at most 3)"
        );
        assert_eq!(p.checks[0].text(), "flair_t1_brain_dice >= 0.85");
        assert_eq!(p.choice("lesions", &json!("off")).unwrap(), json!("off"));
        assert!(p.choice("lesions", &json!("maybe")).is_err());
        assert!(p.choice("goal", &json!("slow")).is_err());
    }

    fn refused(edit: impl Fn(String) -> String) -> String {
        let d = descriptor();
        let text_ = edit(pack_text(&d.digest()));
        load(&text_, &d, None)
            .err()
            .unwrap_or_else(|| panic!("taken: {text_}"))
    }

    #[test]
    fn g1_a_pack_with_an_unknown_fact_a_wrong_type_or_a_forbidden_set_is_refused() {
        let e = refused(|t| t.replace("dimension == '3D'", "colour == 'red'"));
        assert!(
            e.contains("needs.t1w.hard[0].when") && e.contains("unknown fact"),
            "{e}"
        );
        let e = refused(|t| t.replace("slice_thickness_mm <= 1.5", "slice_thickness_mm <= 'thin'"));
        assert!(e.contains("compares a number with text"), "{e}");
        let e = refused(|t| t.replace("dimension == '3D'", "dimension == '4D'"));
        assert!(e.contains("outside the vocabulary"), "{e}");
        let e = refused(|t| t.replace("set: {threads: 2}", "set: {threads: 64}"));
        assert!(
            e.contains("rule fast-goal-fewer-threads sets threads to 64")
                && e.contains("at most 32"),
            "{e}"
        );
        let e = refused(|t| {
            t.replace(
                "set: {threads: 2}",
                "set: {fast: true, pallidum_separate: true}",
            )
        });
        assert!(e.contains("fast disables pallidum_separate"), "{e}");
        let e = refused(|t| t.replace("set: {threads: 2}", "set: {colour: 2}"));
        assert!(
            e.contains("sets colour, which samseg-test has no input of"),
            "{e}"
        );
        let e = refused(|t| {
            t.replace(
                "set: {lesion: \"off\"}, why",
                "set: {lesion: \"maybe\"}, why",
            )
        });
        assert!(e.contains("one of") && e.contains("forbids"), "{e}");
        let e = refused(|t| t.replace("present(flair)\"", "present(elbow)\""));
        assert!(e.contains("elbow"), "{e}");
        let d = descriptor();
        let other = format!("sha256:{}", "0".repeat(64));
        let e = load(&pack_text(&other), &d, None).unwrap_err();
        assert!(
            e.contains("pins descriptor") && e.contains("changed after"),
            "{e}"
        );
        let e = refused(|t| t.replace("nils_pack: 1", "nils_pack: 2"));
        assert!(e.contains("not a version this engine reads"), "{e}");
        let e = refused(|t| t.replace("set: {threads: 2}", "route: elsewhere"));
        assert!(e.contains("variants does not name"), "{e}");
        let e = refused(|t| {
            t.replace(
                "knobs:\n  lesion",
                "knobs:\n  colour: {comparability: none}\n  lesion",
            )
        });
        assert!(e.contains("knobs.colour"), "{e}");
        let e = refused(|t| {
            t.replace(
                "metric: intracranial, op: \">=\", value: 900000",
                "metric: note, op: \">=\", value: 1",
            )
        });
        assert!(e.contains("declares it as text"), "{e}");
    }

    #[test]
    fn the_boutiques_constraints_hold_an_invocation() {
        let d = descriptor();
        let mut params = d.resolve(&[]).unwrap();
        assert!(invocation_holds(&d, &params).is_ok());
        params.insert("fast".into(), json!(true));
        let e = invocation_holds(&d, &params).unwrap_err();
        assert!(e.contains("fast disables pallidum_separate"), "{e}");
        params.insert("pallidum_separate".into(), json!(false));
        assert!(invocation_holds(&d, &params).is_ok());
        params.insert("lesion".into(), Value::Null);
        params.insert("pallidum_separate".into(), json!(true));
        params.insert("fast".into(), json!(false));
        let e = invocation_holds(&d, &params).unwrap_err();
        assert!(e.contains("pallidum_separate requires lesion"), "{e}");
    }

    #[test]
    fn the_shipped_example_loads_against_the_shipped_samseg_descriptor() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let d_text =
            std::fs::read_to_string(root.join("pipelines/samseg-lesions/nils.job.yml")).unwrap();
        let d = descriptor::parse(&d_text).unwrap();
        let p_text = std::fs::read_to_string(
            root.join("contracts/pipeline-pack/v1/examples/samseg-lesions/nils-pack.yaml"),
        )
        .unwrap();
        let p = load(&p_text, &d, None)
            .unwrap_or_else(|e| panic!("{e}; the descriptor's digest is {}", d.digest()));
        assert_eq!(p.name, "samseg-lesions");
        assert!(p.rules.iter().any(|r| r.id == "pallidum-needs-t2-or-flair"));
    }
}
