// SPDX-License-Identifier: AGPL-3.0-only

//! A unit judged by its pack at pre-flight (spec sections 5.4, 5.5, 5.7):
//! the hard needs it fails (skipped, in the pack's words, or "cannot tell:
//! <fact>" where the registry cannot say), the soft needs it fails
//! (caveats that travel with its results), the rules in the pack's order
//! (first match per knob wins; a route to a variant), and every knob with
//! `{value, by, rule, facts}`, `by` one of `default`, `rule:<id>`,
//! `person:<user>` (with `proposed: assistant` where a person accepted the
//! assistant's proposal). [`replay`] runs the rules again on the facts a
//! unit recorded and gives the same values (G3).
//!
//! Precedence per knob (this slice's default, 2026-10-18): a value the
//! person gave wins; the rules fill the rest; the descriptor's default the
//! remainder.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::expr::{self, Expr};
use super::facts::{Facts, Val};
use super::{HOST, LINK, PipelinePack, SetBy};
use crate::descriptor::Descriptor;

/// The person at the run: who, what they chose, and whether they let a
/// unit run whose hard need the registry cannot tell.
#[derive(Debug, Clone, Default)]
pub struct Person {
    pub user: String,
    /// The goal and the switches, as given; the pack's defaults fill the
    /// rest.
    pub choices: BTreeMap<String, Value>,
    /// The knobs the person gave a value, by id (their values are in the
    /// resolved parameters).
    pub given: BTreeSet<String>,
    /// Of those, the ones the assistant proposed and the person accepted.
    pub proposed: BTreeSet<String>,
    /// Whether a hard need on a fact the registry cannot tell lets the
    /// unit run (the person's choice, recorded as such).
    pub let_run: bool,
}

/// One knob as a unit records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub value: Value,
    /// `default`, `rule:<id>` or `person:<user>`.
    pub by: String,
    pub rule: Option<String>,
    /// The facts the rule read, each with its value.
    pub facts: BTreeMap<String, Value>,
    /// `assistant` where a person accepted the assistant's proposal.
    pub proposed: Option<String>,
}

impl Chosen {
    pub fn doc(&self) -> Value {
        let mut v =
            json!({"value": self.value, "by": self.by, "rule": self.rule, "facts": self.facts});
        if let Some(p) = &self.proposed {
            v["proposed"] = json!(p);
        }
        v
    }
}

/// A soft need a unit fails, which travels with its results.
#[derive(Debug, Clone, PartialEq)]
pub struct Caveat {
    pub role: String,
    pub need: String,
    pub words: String,
    pub source: String,
    /// The fact the registry could not tell, where that is why.
    pub cannot_tell: Option<String>,
}

impl Caveat {
    pub fn doc(&self) -> Value {
        json!({
            "role": self.role, "need": self.need, "words": self.words,
            "source": self.source, "cannot_tell": self.cannot_tell,
        })
    }
}

/// Where a rule sent the unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub variant: String,
    pub pipeline: String,
    pub rule: String,
    pub why: Option<String>,
}

/// How a unit stands after its pack judged it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    Skipped,
    Routed,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Skipped => "skipped",
            Status::Routed => "routed",
        }
    }
}

/// The judgement of one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgement {
    pub status: Status,
    /// The hard need it lacks, `<role>: <when>`, where it is skipped.
    pub need: Option<String>,
    /// The pack's words for it, or "cannot tell: <fact>".
    pub words: Option<String>,
    /// The fact a hard need could not be told from.
    pub cannot_tell: Option<String>,
    /// `person:<user>` where the person let it run though a hard need
    /// could not be told.
    pub let_run_by: Option<String>,
    pub variant: Option<Routed>,
    pub caveats: Vec<Caveat>,
    pub knobs: BTreeMap<String, Chosen>,
    /// Every fact the needs and the rules read, with its value, kept with
    /// the unit for the replay and shown at the reader's detail level.
    pub facts: Facts,
}

impl Judgement {
    /// The parameters the unit runs with: each knob's value.
    pub fn params(&self) -> Map<String, Value> {
        self.knobs
            .iter()
            .map(|(k, c)| (k.clone(), c.value.clone()))
            .collect()
    }

    /// As a unit records it and the doors answer it: `{status, need,
    /// words, cannot_tell, let_run_by, variant, caveats, knobs, facts}`.
    pub fn doc(&self) -> Value {
        json!({
            "status": self.status.name(),
            "need": self.need,
            "words": self.words,
            "cannot_tell": self.cannot_tell,
            "let_run_by": self.let_run_by,
            "variant": self.variant.as_ref().map(|r| json!({
                "variant": r.variant, "pipeline": r.pipeline, "rule": r.rule, "why": r.why,
            })),
            "caveats": self.caveats.iter().map(Caveat::doc).collect::<Vec<_>>(),
            "knobs": self.knobs.iter().map(|(k, c)| (k.clone(), c.doc())).collect::<Map<_, _>>(),
            "facts": self.facts.to_json(),
        })
    }

    /// The knobs alone, `{id: {value, by, rule}}`, as the pre-flight lists
    /// them per unit.
    pub fn knobs_brief(&self) -> Value {
        Value::Object(
            self.knobs
                .iter()
                .map(|(k, c)| {
                    (
                        k.clone(),
                        json!({"value": c.value, "by": c.by, "rule": c.rule}),
                    )
                })
                .collect(),
        )
    }
}

/// The facts a unit's judgement reads: the registry's, with the person's
/// goal and switches laid over them (the pack's defaults where nobody
/// chose).
pub fn with_person(pack: &PipelinePack, facts: &Facts, person: &Person) -> Facts {
    let mut f = facts.clone();
    for (k, v) in pack.default_choices() {
        f.set(&k, Val::from_json(&v));
    }
    for (k, v) in &person.choices {
        f.set(k, Val::from_json(v));
    }
    f
}

/// Judge one unit: its needs, then its rules, over `facts` (the registry's
/// facts of the unit, which `person` completes), with `resolved` the
/// parameters as the descriptor resolves the person's values and its
/// defaults.
pub fn judge(
    pack: &PipelinePack,
    d: &Descriptor,
    resolved: &Map<String, Value>,
    facts: &Facts,
    person: &Person,
) -> Judgement {
    let vocab = pack.vocabulary(d);
    let f = with_person(pack, facts, person);
    let mut read = Facts {
        roles: f.roles.clone(),
        ..Facts::default()
    };
    let mut note = |e: &Expr| {
        for (name, v) in expr::read(e, &f) {
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
    let mut j = Judgement {
        status: Status::Ready,
        need: None,
        words: None,
        cannot_tell: None,
        let_run_by: None,
        variant: None,
        caveats: Vec::new(),
        knobs: BTreeMap::new(),
        facts: Facts::default(),
    };
    // the needs, hard first: the first hard need the unit fails names it
    for (role, rn) in &pack.needs {
        let scope = (role != LINK && role != HOST).then_some(role.as_str());
        let hard_on = match &rn.hard_if {
            None => true,
            Some(h) => {
                let h = h.scoped(scope, &vocab);
                note(&h);
                expr::eval(&h, &f) == Val::Bool(true)
            }
        };
        if hard_on {
            for n in &rn.hard {
                let e = n.when.scoped(scope, &vocab);
                note(&e);
                match expr::eval(&e, &f) {
                    Val::Bool(true) => {}
                    Val::Bool(false) => {
                        if j.status == Status::Ready {
                            j.status = Status::Skipped;
                            j.need = Some(format!("{role}: {}", n.text));
                            j.words = Some(n.words.clone());
                        }
                    }
                    _ => {
                        let fact = expr::first_unknown(&e, &f).unwrap_or_else(|| n.text.clone());
                        if person.let_run {
                            j.let_run_by = Some(format!("person:{}", person.user));
                            j.caveats.push(Caveat {
                                role: role.clone(),
                                need: n.text.clone(),
                                words: format!("cannot tell: {fact}; {} let it run", person.user),
                                source: n.source.clone(),
                                cannot_tell: Some(fact),
                            });
                        } else if j.status == Status::Ready {
                            j.status = Status::Skipped;
                            j.need = Some(format!("{role}: {}", n.text));
                            j.words = Some(format!("cannot tell: {fact}"));
                            j.cannot_tell = Some(fact);
                        }
                    }
                }
            }
        }
        for n in &rn.soft {
            let e = n.when.scoped(scope, &vocab);
            note(&e);
            match expr::eval(&e, &f) {
                Val::Bool(true) => {}
                Val::Bool(false) => j.caveats.push(Caveat {
                    role: role.clone(),
                    need: n.text.clone(),
                    words: n.words.clone(),
                    source: n.source.clone(),
                    cannot_tell: None,
                }),
                _ => {
                    let fact = expr::first_unknown(&e, &f).unwrap_or_else(|| n.text.clone());
                    j.caveats.push(Caveat {
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
    // the person's knobs first, then the rules, first match per knob
    for id in &person.given {
        if let Some(v) = resolved.get(id) {
            j.knobs.insert(
                id.clone(),
                Chosen {
                    value: v.clone(),
                    by: format!("person:{}", person.user),
                    rule: None,
                    facts: BTreeMap::new(),
                    proposed: person
                        .proposed
                        .contains(id)
                        .then(|| "assistant".to_string()),
                },
            );
        }
    }
    for r in &pack.rules {
        let e = r.when.scoped(None, &vocab);
        note(&e);
        if expr::eval(&e, &f) != Val::Bool(true) {
            continue;
        }
        let read_facts = expr::read(&e, &f);
        if let Some(v) = &r.route
            && j.variant.is_none()
            && let Some(variant) = pack.variants.get(v)
        {
            j.variant = Some(Routed {
                variant: v.clone(),
                pipeline: variant.pipeline.clone(),
                rule: r.id.clone(),
                why: r.why.clone().or_else(|| variant.words.clone()),
            });
            if j.status == Status::Ready {
                j.status = Status::Routed;
            }
        }
        for (k, v) in &r.set {
            if j.knobs.contains_key(k) {
                continue;
            }
            j.knobs.insert(
                k.clone(),
                Chosen {
                    value: v.clone(),
                    by: format!("rule:{}", r.id),
                    rule: Some(r.id.clone()),
                    facts: read_facts.clone(),
                    proposed: None,
                },
            );
        }
    }
    // the rest: the pack's default where it has one, else the descriptor's
    for (k, v) in resolved {
        if j.knobs.contains_key(k) {
            continue;
        }
        let value = pack
            .knobs
            .get(k)
            .and_then(|knob| knob.default.clone())
            .unwrap_or_else(|| v.clone());
        j.knobs.insert(
            k.clone(),
            Chosen {
                value,
                by: "default".into(),
                rule: None,
                facts: BTreeMap::new(),
                proposed: None,
            },
        );
    }
    for (k, v) in pack.default_choices() {
        read.set(&k, Val::from_json(&v));
    }
    for (k, v) in &person.choices {
        read.set(k, Val::from_json(v));
    }
    j.facts = read;
    j
}

/// The knobs a person gave that the pack does not let a person set: a
/// `set_by: fixed` knob is refused, and so is a `set_by: rule` knob, since
/// a rule chooses it from the facts; the sentence says which.
pub fn person_may_set(pack: &PipelinePack, given: &BTreeSet<String>) -> Result<(), String> {
    for id in given {
        if let Some(k) = pack.knobs.get(id) {
            match k.set_by {
                SetBy::Fixed => {
                    return Err(format!(
                        "{id} is fixed by {}'s pack and cannot be set",
                        pack.name
                    ));
                }
                SetBy::Rule => {
                    return Err(format!(
                        "{id} is set by {}'s rules from the scan's facts, not by hand",
                        pack.name
                    ));
                }
                SetBy::Person => {}
            }
        }
    }
    Ok(())
}

/// Judge a unit again from what it recorded (`{knobs, facts}` as
/// [`Judgement::doc`] wrote it): the facts read back, the person's values
/// read from the knobs recorded `person:<user>`, the goal and the switches
/// from the facts. The replay of G3 and the archive replay of section 10
/// both read it; under an unchanged pack it is the recorded judgement.
pub fn rejudge(pack: &PipelinePack, d: &Descriptor, recorded: &Value) -> Result<Judgement, String> {
    let facts = Facts::from_json(&recorded["facts"]);
    let mut person = Person::default();
    let mut given: Vec<(String, String)> = Vec::new();
    for (k, c) in recorded["knobs"].as_object().into_iter().flatten() {
        if let Some(user) = c["by"].as_str().and_then(|b| b.strip_prefix("person:")) {
            person.user = user.to_string();
            person.given.insert(k.clone());
            given.push((
                k.clone(),
                match &c["value"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            ));
        }
    }
    for (k, v) in &facts.values {
        if k == "goal" || pack.goals.switches.contains_key(k) {
            person.choices.insert(k.clone(), v.to_json());
        }
    }
    let resolved = d.resolve(&given)?;
    Ok(judge(pack, d, &resolved, &facts, &person))
}

/// Replay the rules on the facts a unit recorded (G3): the knobs as the
/// rules choose them now from `recorded`. Equal to the recorded values
/// where the pack has not changed.
pub fn replay(
    pack: &PipelinePack,
    d: &Descriptor,
    recorded: &Value,
) -> Result<BTreeMap<String, Value>, String> {
    let j = rejudge(pack, d, recorded)?;
    Ok(j.knobs.into_iter().map(|(k, c)| (k, c.value)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::tests::loaded;

    fn facts(flair: bool, dim: &str, thick: f64) -> Facts {
        let mut f = Facts::default();
        f.roles.insert("t1w".into());
        if flair {
            f.roles.insert("flair".into());
        }
        f.set("t1w.dimension", Val::Text(dim.into()));
        f.set("t1w.slice_thickness_mm", Val::Number(thick));
        f.set("t1w.field_strength_t", Val::Number(3.0));
        f.set("host.memory_gb", Val::Number(64.0));
        f.set_aligned("flair", "t1w", Val::Bool(true));
        f
    }

    fn person(user: &str) -> Person {
        Person {
            user: user.into(),
            ..Person::default()
        }
    }

    #[test]
    fn a_unit_with_everything_runs_with_the_rules_values_and_who_chose() {
        let (d, p) = loaded();
        let resolved = d.resolve(&[]).unwrap();
        let j = judge(&p, &d, &resolved, &facts(true, "3D", 1.0), &person("anna"));
        assert_eq!(j.status, Status::Ready, "{:?}", j);
        assert!(j.caveats.is_empty(), "{:?}", j.caveats);
        assert_eq!(j.knobs["lesion"].value, "on");
        assert_eq!(j.knobs["lesion"].by, "default");
        assert_eq!(j.knobs["pallidum_separate"].value, true);
        assert_eq!(j.knobs["threads"].by, "default");
        assert_eq!(j.facts.get("goal"), Val::Text("standard".into()));
        assert_eq!(j.facts.get("lesions"), Val::Text("on".into()));
    }

    #[test]
    fn hard_needs_skip_in_the_pack_s_words_and_a_false_hard_if_turns_them_off() {
        let (d, p) = loaded();
        let resolved = d.resolve(&[]).unwrap();
        let j = judge(&p, &d, &resolved, &facts(true, "2D", 5.0), &person("anna"));
        assert_eq!(j.status, Status::Skipped);
        assert_eq!(
            j.need.as_deref(),
            Some("t1w: dimension == '3D' and slice_thickness_mm <= 1.5")
        );
        assert!(j.words.as_deref().unwrap().starts_with("the T1 is 2D"));
        // no FLAIR with lesions on: skipped by the FLAIR's need
        let j = judge(&p, &d, &resolved, &facts(false, "3D", 1.0), &person("anna"));
        assert_eq!(j.status, Status::Skipped);
        assert_eq!(j.words.as_deref(), Some("no FLAIR picked, so no lesions"));
        // lesions off: the FLAIR is not needed, the rules set lesion off,
        // and the pallidum rule reads no FLAIR and no T2w
        let mut off = person("anna");
        off.choices.insert("lesions".into(), json!("off"));
        let j = judge(&p, &d, &resolved, &facts(false, "3D", 1.0), &off);
        assert_eq!(j.status, Status::Ready, "{:?}", j);
        assert_eq!(j.knobs["lesion"].value, "off");
        assert_eq!(j.knobs["lesion"].by, "rule:lesions-off-without-flair");
        assert_eq!(j.knobs["pallidum_separate"].value, false);
        assert_eq!(
            j.knobs["pallidum_separate"].by,
            "rule:pallidum-needs-t2-or-flair"
        );
        assert_eq!(
            j.knobs["pallidum_separate"].facts,
            [
                ("present:flair".to_string(), json!(false)),
                ("present:t2w".to_string(), json!(false))
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn an_unknown_fact_skips_with_cannot_tell_unless_the_person_lets_it_run() {
        let (d, p) = loaded();
        let resolved = d.resolve(&[]).unwrap();
        let mut f = facts(true, "3D", 1.0);
        f.aligned.clear();
        let j = judge(&p, &d, &resolved, &f, &person("anna"));
        assert_eq!(j.status, Status::Skipped);
        assert_eq!(j.words.as_deref(), Some("cannot tell: aligned:flair:t1w"));
        assert_eq!(j.cannot_tell.as_deref(), Some("aligned:flair:t1w"));
        let mut let_run = person("anna");
        let_run.let_run = true;
        let j = judge(&p, &d, &resolved, &f, &let_run);
        assert_eq!(j.status, Status::Ready);
        assert_eq!(j.let_run_by.as_deref(), Some("person:anna"));
        assert!(
            j.caveats
                .iter()
                .any(|c| c.cannot_tell.as_deref() == Some("aligned:flair:t1w"))
        );
        // a soft need on an unknown fact is a caveat
        let mut f = facts(true, "3D", 1.0);
        f.values.remove("t1w.field_strength_t");
        let j = judge(&p, &d, &resolved, &f, &person("anna"));
        assert_eq!(j.status, Status::Ready);
        assert_eq!(j.caveats.len(), 1);
        assert_eq!(
            j.caveats[0].words,
            "cannot tell: t1w.field_strength_t; the T1 was acquired below 1.5 T"
        );
        // a soft need that fails is a caveat in the pack's words
        let mut f = facts(true, "3D", 1.0);
        f.set("t1w.field_strength_t", Val::Number(1.0));
        f.set("host.memory_gb", Val::Number(8.0));
        let j = judge(&p, &d, &resolved, &f, &person("anna"));
        assert_eq!(
            j.caveats
                .iter()
                .map(|c| c.words.as_str())
                .collect::<Vec<_>>(),
            [
                "the host has less than 16 GB",
                "the T1 was acquired below 1.5 T"
            ]
        );
    }

    #[test]
    fn a_person_s_value_wins_a_rule_fills_the_rest_and_the_replay_agrees() {
        let (d, p) = loaded();
        let given = vec![("threads".to_string(), "8".to_string())];
        let resolved = d.resolve(&given).unwrap();
        let mut anna = person("anna");
        anna.given.insert("threads".into());
        anna.proposed.insert("threads".into());
        anna.choices.insert("goal".into(), json!("fast"));
        let j = judge(&p, &d, &resolved, &facts(true, "3D", 1.0), &anna);
        assert_eq!(j.knobs["threads"].value, 8);
        assert_eq!(j.knobs["threads"].by, "person:anna");
        assert_eq!(j.knobs["threads"].proposed.as_deref(), Some("assistant"));
        assert_eq!(j.knobs["threads"].doc()["proposed"], "assistant");
        let doc = j.doc();
        assert_eq!(doc["knobs"]["threads"]["by"], "person:anna");
        assert_eq!(doc["facts"]["values"]["goal"], "fast");
        let again = replay(&p, &d, &doc).unwrap();
        assert_eq!(again, j.params().into_iter().collect::<BTreeMap<_, _>>());
        // without the person's value the fast goal's rule sets the threads
        let j = judge(
            &p,
            &d,
            &d.resolve(&[]).unwrap(),
            &facts(true, "3D", 1.0),
            &Person {
                choices: [("goal".to_string(), json!("fast"))].into_iter().collect(),
                ..person("anna")
            },
        );
        assert_eq!(j.knobs["threads"].value, 2);
        assert_eq!(j.knobs["threads"].by, "rule:fast-goal-fewer-threads");
        assert_eq!(
            j.knobs["threads"].facts,
            [("goal".to_string(), json!("fast"))].into_iter().collect()
        );
        let again = replay(&p, &d, &j.doc()).unwrap();
        assert_eq!(again["threads"], 2);
        assert!(
            person_may_set(&p, &["lesion".to_string()].into_iter().collect())
                .unwrap_err()
                .contains("rules")
        );
        assert!(person_may_set(&p, &["threads".to_string()].into_iter().collect()).is_ok());
    }
}
