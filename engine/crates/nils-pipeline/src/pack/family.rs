// SPDX-License-Identifier: AGPL-3.0-only

//! A family of pipelines (record 57 P6; Wave 7b spec section 7):
//! `nils-family.yaml`, under `contracts/pipeline-pack/v1` beside the pack's
//! schema. A family is one entry for the person and several pipelines
//! behind it, its members, each a pair of descriptor and pack in a folder
//! beside the family document and a pipeline of its own in the catalog.
//! The family document holds the roles its members read, the goals (the
//! goal, a `task` switch and the family's other switches), the tasks with
//! the unit level each runs at, the family-level needs (the refusals, in
//! the pack's shape) and the ordered rules that route a unit to a member
//! from the registry's facts and the person's choices, in the pack's
//! expression language. A member's own pack keeps its knobs, checks,
//! resources, licence and source.
//!
//! [`route`] judges one unit: its needs, then the first rule that holds
//! names the member, and the unit records the member and the rule that
//! chose (the pack record's `variant`, as a pack's route does). Two facts
//! of the unit itself join the vocabulary for the rules that read a
//! person's visits: [`UNIT_VISITS`] and [`UNIT_SAME_PROTOCOL`].

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::expr::{self, Expr};
use super::facts::{self, FactType, Facts, Goals, Val, Vocabulary};
use super::rules::{Caveat, Person, Status};
use super::{HOST, LINK, PipelinePack, RoleNeeds, expression, opt_text, text};
use crate::descriptor::Descriptor;

/// The family's file name.
pub const FILE: &str = "nils-family.yaml";

/// The switch that names the task: the family's tasks are its values.
pub const TASK: &str = "task";

/// The needs key for the unit itself (beside the roles, `link` and
/// `host`): its needs read the unit facts and any role's, unscoped.
pub const UNIT: &str = "unit";

/// How many sessions a unit holds: 1 for a session unit, the person's
/// visits for a participant unit.
pub const UNIT_VISITS: &str = "unit.visits";

/// Whether every visit of a participant unit was acquired alike: the T1w
/// of each shares base, dimension, vendor and field strength. True for a
/// session unit.
pub const UNIT_SAME_PROTOCOL: &str = "unit.same_protocol";

/// One task of the family, with the unit level it runs at.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// `session` or `participant`.
    pub level: String,
    pub words: Option<String>,
    /// The members a rule of this task may route to; each has a descriptor
    /// at the task's level.
    pub members: Vec<String>,
}

/// One member: a pipeline of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub pipeline: String,
    pub method: Option<String>,
    pub words: Option<String>,
}

/// One routing rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub when: Expr,
    pub text: String,
    pub route: String,
    pub why: Option<String>,
    pub source: Option<String>,
}

/// The family document as loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name: String,
    pub version: Option<String>,
    pub words: Option<String>,
    /// The roles the members read, which the needs and rules may name.
    pub roles: Vec<String>,
    pub goals: Goals,
    pub tasks: BTreeMap<String, Task>,
    pub members: BTreeMap<String, Member>,
    pub needs: BTreeMap<String, RoleNeeds>,
    pub rules: Vec<Rule>,
    /// The document as it parsed, kept whole.
    pub document: Value,
}

/// Parse a family from its YAML and check its shape and its expressions'
/// grammar; [`load_dir`] checks it against its members too.
pub fn parse(text_: &str) -> Result<Family, String> {
    let document: Value =
        serde_saphyr::from_str(text_).map_err(|e| format!("not a YAML document: {e}"))?;
    from_value(document)
}

/// [`parse`] of a document already read.
pub fn from_value(document: Value) -> Result<Family, String> {
    if !document.is_object() {
        return Err("a family is a mapping".into());
    }
    match document["nils_family"].as_i64() {
        Some(1) => {}
        Some(other) => {
            return Err(format!(
                "nils_family: {other} is not a version this engine reads; it reads 1 ({})",
                super::CONTRACT
            ));
        }
        None => return Err("nils_family: 1 is missing".into()),
    }
    let f = &document["family"];
    if !f.is_object() {
        return Err("family is a mapping: name, version, words".into());
    }
    let name = text(f, "name", "family.")?;
    let roles = match &document["roles"] {
        Value::Array(items) if !items.is_empty() => items
            .iter()
            .map(|r| {
                r.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| "roles is a list of role names".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("roles names the roles the members read, at least one".into()),
    };
    let goals = super::goals_of(&document)?;
    let Some(task_values) = goals.switches.get(TASK) else {
        return Err(format!("goals.{TASK} names the family's tasks"));
    };
    let mut tasks = BTreeMap::new();
    match &document["tasks"] {
        Value::Object(map) => {
            for (k, v) in map {
                let at = format!("tasks.{k}.");
                if !v.is_object() {
                    return Err(format!("tasks.{k} is a mapping: level, words, members"));
                }
                let level = text(v, "level", &at)?;
                if level != "session" && level != "participant" {
                    return Err(format!("{at}level is session or participant, not {level}"));
                }
                let members = match &v["members"] {
                    Value::Array(items) if !items.is_empty() => items
                        .iter()
                        .map(|m| {
                            m.as_str()
                                .map(str::to_string)
                                .ok_or_else(|| format!("{at}members is a list of member names"))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => return Err(format!("{at}members names the members the task routes to")),
                };
                if !task_values.iter().any(|t| facts::text_of(t) == *k) {
                    return Err(format!("tasks.{k} is not a value of goals.{TASK}"));
                }
                tasks.insert(
                    k.clone(),
                    Task {
                        level,
                        words: opt_text(v, "words", &at)?,
                        members,
                    },
                );
            }
        }
        _ => return Err("tasks is a mapping of the family's tasks".into()),
    }
    for t in task_values {
        let t = facts::text_of(t);
        if !tasks.contains_key(&t) {
            return Err(format!(
                "goals.{TASK} names {t}, which tasks does not describe"
            ));
        }
    }
    let mut members = BTreeMap::new();
    match &document["members"] {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                let at = format!("members.{k}.");
                if !v.is_object() {
                    return Err(format!("members.{k} is a mapping: pipeline, method, words"));
                }
                members.insert(
                    k.clone(),
                    Member {
                        pipeline: text(v, "pipeline", &at)?,
                        method: opt_text(v, "method", &at)?,
                        words: opt_text(v, "words", &at)?,
                    },
                );
            }
        }
        _ => return Err("members is a mapping of the family's members, at least one".into()),
    }
    for (k, t) in &tasks {
        for m in &t.members {
            if !members.contains_key(m) {
                return Err(format!(
                    "tasks.{k}.members names {m}, which members does not"
                ));
            }
        }
    }
    for m in members.keys() {
        if !tasks.values().any(|t| t.members.contains(m)) {
            return Err(format!("member {m} belongs to no task"));
        }
    }
    let needs = super::needs_of(&document)?;
    let mut rules = Vec::new();
    match &document["rules"] {
        Value::Array(items) if !items.is_empty() => {
            for (i, v) in items.iter().enumerate() {
                let at = format!("rules[{i}].");
                let id = text(v, "id", &at)?;
                if rules.iter().any(|r: &Rule| r.id == id) {
                    return Err(format!("rules[{i}].id {id} is used twice"));
                }
                let (when, text_) = expression(v, "when", &at)?;
                let route = text(v, "route", &at)?;
                if !members.contains_key(&route) {
                    return Err(format!("{at}route names {route}, which members does not"));
                }
                rules.push(Rule {
                    id,
                    when,
                    text: text_,
                    route,
                    why: opt_text(v, "why", &at)?,
                    source: opt_text(v, "source", &at)?,
                });
            }
        }
        _ => return Err("rules is a list of routing rules, at least one".into()),
    }
    let family = Family {
        name,
        version: opt_text(f, "version", "family.")?,
        words: opt_text(f, "words", "family.")?,
        roles,
        goals,
        tasks,
        members,
        needs,
        rules,
        document,
    };
    family.check_expressions()?;
    Ok(family)
}

impl Family {
    /// The digest of the family: sha256 of its canonical JSON.
    pub fn digest(&self) -> String {
        crate::sha256(crate::canonical(&self.document).as_bytes())
    }

    /// The vocabulary the family's expressions are checked against: the
    /// stack facts of its roles, the goals and the two unit facts.
    pub fn vocabulary(&self) -> Vocabulary {
        let mut v = Vocabulary::new(&self.roles, &self.goals);
        v.facts.insert(UNIT_VISITS.into(), FactType::Number);
        v.facts.insert(UNIT_SAME_PROTOCOL.into(), FactType::Bool);
        v
    }

    /// The goal and switch values nobody chose: the goal's default and
    /// each switch's first value.
    pub fn default_choices(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        out.insert("goal".into(), json!(self.goals.default_goal()));
        for (k, values) in &self.goals.switches {
            if let Some(v) = values.first() {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    /// A person's choice checked against the goals: the goal, or a switch
    /// by name, with the value as the family writes it.
    pub fn choice(&self, name: &str, value: &Value) -> Result<Value, String> {
        if name == "goal" {
            let goal = value.as_str().unwrap_or_default();
            let offered: Vec<String> = if self.goals.goal.is_empty() {
                facts::GOALS.iter().map(|g| g.to_string()).collect()
            } else {
                self.goals.goal.clone()
            };
            return if offered.iter().any(|g| g == goal) {
                Ok(json!(goal))
            } else {
                Err(format!(
                    "the family {} offers the goals {}, not {goal}",
                    self.name,
                    offered.join(", ")
                ))
            };
        }
        let Some(values) = self.goals.switches.get(name) else {
            return Err(format!(
                "the family {} has no switch {name}; its switches are {}",
                self.name,
                self.goals
                    .switches
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        let wanted = facts::text_of(value);
        values
            .iter()
            .find(|v| facts::text_of(v) == wanted)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{name} takes {}, not {wanted}",
                    values
                        .iter()
                        .map(facts::text_of)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    /// The task a person's choices name, or the default.
    pub fn task_of(&self, choices: &BTreeMap<String, Value>) -> String {
        choices.get(TASK).map(facts::text_of).unwrap_or_else(|| {
            self.default_choices()[TASK]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
    }

    fn check_expressions(&self) -> Result<(), String> {
        let vocab = self.vocabulary();
        for (role, rn) in &self.needs {
            let scope = (role != LINK && role != HOST && role != UNIT).then_some(role.as_str());
            if let Some(s) = scope {
                vocab.role(s).map_err(|e| format!("needs.{role}: {e}"))?;
            }
            if let Some(h) = &rn.hard_if {
                super::typed(h, scope, &vocab, &format!("needs.{role}.hard_if"))?;
            }
            for (kind, list) in [("hard", &rn.hard), ("soft", &rn.soft)] {
                for (i, n) in list.iter().enumerate() {
                    super::typed(
                        &n.when,
                        scope,
                        &vocab,
                        &format!("needs.{role}.{kind}[{i}].when"),
                    )?;
                }
            }
        }
        for (i, r) in self.rules.iter().enumerate() {
            super::typed(
                &r.when,
                None,
                &vocab,
                &format!("rules[{i}].when ({})", r.id),
            )?;
        }
        Ok(())
    }

    /// Check the family against its members' descriptors (and packs where
    /// they have one): every member named is there, a member a task routes
    /// to runs at the task's level, and every member reads only roles the
    /// family names.
    pub fn check(&self, members: &Members) -> Result<(), String> {
        for (k, m) in &self.members {
            let Some((d, _)) = members.get(k) else {
                return Err(format!(
                    "member {k} ({}) has no folder beside the family",
                    m.pipeline
                ));
            };
            if d.name != m.pipeline {
                return Err(format!(
                    "member {k} names the pipeline {}, but its descriptor is {}",
                    m.pipeline, d.name
                ));
            }
            for r in &d.roles {
                if !self.roles.contains(r) {
                    return Err(format!(
                        "member {k} reads the role {r}, which the family's roles do not name"
                    ));
                }
            }
        }
        for (t, task) in &self.tasks {
            for k in &task.members {
                if let Some((d, _)) = members.get(k)
                    && d.level.name() != task.level
                {
                    return Err(format!(
                        "task {t} runs at the {} level, but its member {k} is a {} pipeline",
                        task.level,
                        d.level.name()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Where a family sent one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Routing {
    pub status: Status,
    /// The hard need it lacks, `<role>: <when>`, where it is skipped.
    pub need: Option<String>,
    /// The family's words for it, or "cannot tell: <fact>".
    pub words: Option<String>,
    pub cannot_tell: Option<String>,
    pub let_run_by: Option<String>,
    pub member: Option<String>,
    pub pipeline: Option<String>,
    pub rule: Option<String>,
    pub why: Option<String>,
    pub caveats: Vec<Caveat>,
    /// The facts the needs and the rules read.
    pub facts: Facts,
}

impl Routing {
    /// As a unit records it: the `variant` of its pack record.
    pub fn variant_doc(&self) -> Value {
        match (&self.member, &self.pipeline, &self.rule) {
            (Some(m), Some(p), Some(r)) => {
                json!({"variant": m, "pipeline": p, "rule": r, "why": self.why})
            }
            _ => Value::Null,
        }
    }

    pub fn doc(&self) -> Value {
        json!({
            "status": self.status.name(),
            "need": self.need,
            "words": self.words,
            "cannot_tell": self.cannot_tell,
            "let_run_by": self.let_run_by,
            "member": self.member,
            "pipeline": self.pipeline,
            "rule": self.rule,
            "why": self.why,
            "caveats": self.caveats.iter().map(|c| c.words.clone()).collect::<Vec<_>>(),
            "facts": self.facts.to_json(),
        })
    }
}

/// The facts of a unit with the person's choices (and the family's
/// defaults) set.
pub fn with_person(f: &Family, facts: &Facts, person: &Person) -> Facts {
    let mut out = facts.clone();
    for (k, v) in f.default_choices() {
        out.set(&k, Val::from_json(&v));
    }
    for (k, v) in &person.choices {
        out.set(k, Val::from_json(v));
    }
    out
}

/// Route one unit: its needs (hard first, as a pack's are judged), then
/// the first rule that holds names the member; a unit no rule takes, or
/// whose rule names a member the task does not run, is skipped in words.
pub fn route(f: &Family, facts: &Facts, person: &Person) -> Routing {
    let vocab = f.vocabulary();
    let all = with_person(f, facts, person);
    let task = f.task_of(&person.choices);
    let mut read = Facts {
        roles: all.roles.clone(),
        ..Facts::default()
    };
    let mut note = |e: &Expr| {
        for (name, v) in expr::read(e, &all) {
            if let Some(r) = name.strip_prefix("present:") {
                if v == Value::Bool(true) {
                    read.roles.insert(r.to_string());
                }
            } else if let Some(pair) = name.strip_prefix("aligned:") {
                if let Some((a, b)) = pair.split_once(':') {
                    read.set_aligned(a, b, Val::from_json(&v));
                }
            } else {
                read.set(&name, Val::from_json(&v));
            }
        }
    };
    let mut r = Routing {
        status: Status::Ready,
        need: None,
        words: None,
        cannot_tell: None,
        let_run_by: None,
        member: None,
        pipeline: None,
        rule: None,
        why: None,
        caveats: Vec::new(),
        facts: Facts::default(),
    };
    for (role, rn) in &f.needs {
        let scope = (role != LINK && role != HOST && role != UNIT).then_some(role.as_str());
        let hard_on = match &rn.hard_if {
            None => true,
            Some(h) => {
                let h = h.scoped(scope, &vocab);
                note(&h);
                expr::eval(&h, &all) == Val::Bool(true)
            }
        };
        if hard_on {
            for n in &rn.hard {
                let e = n.when.scoped(scope, &vocab);
                note(&e);
                match expr::eval(&e, &all) {
                    Val::Bool(true) => {}
                    Val::Bool(false) => {
                        if r.status == Status::Ready {
                            r.status = Status::Skipped;
                            r.need = Some(format!("{role}: {}", n.text));
                            r.words = Some(n.words.clone());
                        }
                    }
                    _ => {
                        let fact = expr::first_unknown(&e, &all).unwrap_or_else(|| n.text.clone());
                        if person.let_run {
                            r.let_run_by = Some(format!("person:{}", person.user));
                            r.caveats.push(Caveat {
                                role: role.clone(),
                                need: n.text.clone(),
                                words: format!("cannot tell: {fact}; {} let it run", person.user),
                                source: n.source.clone(),
                                cannot_tell: Some(fact),
                            });
                        } else if r.status == Status::Ready {
                            r.status = Status::Skipped;
                            r.need = Some(format!("{role}: {}", n.text));
                            r.words = Some(format!("cannot tell: {fact}"));
                            r.cannot_tell = Some(fact);
                        }
                    }
                }
            }
        }
        for n in &rn.soft {
            let e = n.when.scoped(scope, &vocab);
            note(&e);
            match expr::eval(&e, &all) {
                Val::Bool(true) => {}
                Val::Bool(false) => r.caveats.push(Caveat {
                    role: role.clone(),
                    need: n.text.clone(),
                    words: n.words.clone(),
                    source: n.source.clone(),
                    cannot_tell: None,
                }),
                _ => {
                    let fact = expr::first_unknown(&e, &all).unwrap_or_else(|| n.text.clone());
                    r.caveats.push(Caveat {
                        role: role.clone(),
                        need: n.text.clone(),
                        words: format!("cannot tell: {fact}; {}", n.words),
                        source: n.source.clone(),
                        cannot_tell: Some(fact),
                    });
                }
            }
        }
    }
    let members_of_task: Vec<String> = f
        .tasks
        .get(&task)
        .map(|t| t.members.clone())
        .unwrap_or_default();
    let mut chosen = None;
    for rule in &f.rules {
        let e = rule.when.scoped(None, &vocab);
        note(&e);
        if expr::eval(&e, &all) == Val::Bool(true) {
            chosen = Some(rule);
            break;
        }
    }
    match chosen {
        Some(rule) if members_of_task.contains(&rule.route) => {
            r.member = Some(rule.route.clone());
            r.pipeline = f.members.get(&rule.route).map(|m| m.pipeline.clone());
            r.rule = Some(rule.id.clone());
            r.why = rule
                .why
                .clone()
                .or_else(|| f.members.get(&rule.route).and_then(|m| m.words.clone()));
            if r.status == Status::Ready {
                r.status = Status::Routed;
            }
        }
        Some(rule) => {
            if r.status == Status::Ready {
                r.status = Status::Skipped;
                r.need = Some(format!("rule {}: {}", rule.id, rule.text));
                r.words = Some(format!(
                    "the rule {} routes to {}, which the task {task} does not run",
                    rule.id, rule.route
                ));
            }
        }
        None => {
            if r.status == Status::Ready {
                r.status = Status::Skipped;
                r.need = Some("rules: none holds".into());
                r.words = Some(format!(
                    "no rule of the family {} routes this unit under the task {task}",
                    f.name
                ));
            }
        }
    }
    for (k, v) in f.default_choices() {
        read.set(&k, Val::from_json(&v));
    }
    for (k, v) in &person.choices {
        read.set(k, Val::from_json(v));
    }
    r.facts = read;
    r
}

/// The two unit facts set on a unit's facts: its visits and whether they
/// share a protocol (unknown where the registry cannot tell).
pub fn set_unit_facts(f: &mut Facts, visits: usize, same_protocol: Option<bool>) {
    f.set(UNIT_VISITS, Val::Number(visits as f64));
    f.set(
        UNIT_SAME_PROTOCOL,
        match same_protocol {
            Some(b) => Val::Bool(b),
            None => Val::Unknown,
        },
    );
}

/// The members of a family as loaded from their folders: each a
/// descriptor and its pack, by member key.
pub type Members = BTreeMap<String, (Descriptor, Option<PipelinePack>)>;

/// A family's folder read: `nils-family.yaml`, and each member's folder
/// beside it, named as the member is, each a pair of descriptor and pack
/// checked as [`super::load_dir`] checks it; then the family against its
/// members.
pub fn load_dir(dir: &Path, vocab: Option<&Vocabulary>) -> Result<(Family, Members), String> {
    let path = dir.join(FILE);
    let text_ = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let family = parse(&text_).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut members = BTreeMap::new();
    for k in family.members.keys() {
        let folder = dir.join(k);
        if !folder.is_dir() {
            return Err(format!(
                "{}: member {k} has no folder {}",
                path.display(),
                folder.display()
            ));
        }
        members.insert(k.clone(), super::load_dir(&folder, vocab)?);
    }
    family
        .check(&members)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((family, members))
}

/// Whether a folder holds a family rather than one pipeline.
pub fn is_family_dir(dir: &Path) -> bool {
    dir.join(FILE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> &'static str {
        r#"
nils_family: 1
family: {name: registration, version: "1.0"}
roles: [t1w, flair]
goals:
  goal: [fast, standard, research]
  task: [within_session, across_visits]
tasks:
  within_session: {level: session, members: [ants, synthmorph]}
  across_visits: {level: participant, members: [robust]}
members:
  ants: {pipeline: registration-ants}
  synthmorph: {pipeline: registration-synthmorph}
  robust: {pipeline: registration-robust}
needs:
  t1w:
    hard:
      - {when: "body_part in ['brain', 'brain-neck']", words: "not a head", source: policy}
rules:
  - {id: fine-3d, when: "task == 'within_session' and t1w.dimension == '3D' and flair.dimension == '3D' and max(t1w.voxel_max_mm, flair.voxel_max_mm) <= 1.5", route: ants}
  - {id: within-rest, when: "task == 'within_session'", route: synthmorph}
  - {id: visits, when: "task == 'across_visits' and unit.visits >= 2 and unit.same_protocol", route: robust}
"#
    }

    fn person(choices: &[(&str, Value)]) -> Person {
        Person {
            user: "tester".into(),
            choices: choices
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            given: Default::default(),
            proposed: Default::default(),
            let_run: false,
        }
    }

    fn facts(dim: &str, voxel: f64) -> Facts {
        let mut f = Facts::default();
        for role in ["t1w", "flair"] {
            f.roles.insert(role.into());
            f.set(&format!("{role}.dimension"), Val::Text(dim.into()));
            f.set(&format!("{role}.voxel_max_mm"), Val::Number(voxel));
            f.set(&format!("{role}.body_part"), Val::Text("brain".into()));
        }
        set_unit_facts(&mut f, 1, Some(true));
        f
    }

    #[test]
    fn the_first_rule_that_holds_names_the_member_and_the_rule() {
        let f = parse(doc()).unwrap();
        let r = route(&f, &facts("3D", 1.0), &person(&[]));
        assert_eq!(r.status, Status::Routed);
        assert_eq!(r.member.as_deref(), Some("ants"));
        assert_eq!(r.pipeline.as_deref(), Some("registration-ants"));
        assert_eq!(r.rule.as_deref(), Some("fine-3d"));
        let r = route(&f, &facts("2D", 1.0), &person(&[]));
        assert_eq!(r.member.as_deref(), Some("synthmorph"));
        assert_eq!(r.rule.as_deref(), Some("within-rest"));
        assert_eq!(r.facts.get("task"), Val::Text("within_session".into()));
    }

    #[test]
    fn a_hard_need_skips_in_the_family_s_words_and_unknown_says_cannot_tell() {
        let f = parse(doc()).unwrap();
        let mut bad = facts("3D", 1.0);
        bad.set("t1w.body_part", Val::Text("spine".into()));
        let r = route(&f, &bad, &person(&[]));
        assert_eq!(r.status, Status::Skipped);
        assert_eq!(r.words.as_deref(), Some("not a head"));
        bad.set("t1w.body_part", Val::Unknown);
        let r = route(&f, &bad, &person(&[]));
        assert_eq!(r.words.as_deref(), Some("cannot tell: t1w.body_part"));
        assert_eq!(r.cannot_tell.as_deref(), Some("t1w.body_part"));
    }

    #[test]
    fn a_task_with_no_rule_that_holds_is_skipped_and_a_member_of_another_task_is_refused() {
        let f = parse(doc()).unwrap();
        let mut one = facts("3D", 1.0);
        set_unit_facts(&mut one, 1, Some(true));
        let r = route(&f, &one, &person(&[("task", json!("across_visits"))]));
        assert_eq!(r.status, Status::Skipped);
        assert!(r.words.as_deref().unwrap().contains("no rule"), "{r:?}");
        let mut two = facts("3D", 1.0);
        set_unit_facts(&mut two, 3, Some(true));
        let r = route(&f, &two, &person(&[("task", json!("across_visits"))]));
        assert_eq!(r.member.as_deref(), Some("robust"));
        assert_eq!(r.facts.get(UNIT_VISITS), Val::Number(3.0));
    }

    #[test]
    fn the_document_is_checked_at_load() {
        let bad = doc().replace("route: robust", "route: nobody");
        assert!(parse(&bad).unwrap_err().contains("members does not"));
        let bad = doc().replace("unit.visits >= 2", "unit.visits == 'two'");
        assert!(parse(&bad).is_err());
        let bad = doc().replace(
            "  across_visits: {level: participant, members: [robust]}\n",
            "",
        );
        assert!(parse(&bad).unwrap_err().contains("tasks does not describe"));
        let bad = doc().replace("members: [robust]", "members: [ants]");
        assert!(parse(&bad).unwrap_err().contains("belongs to no task"));
    }
}
