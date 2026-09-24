// SPDX-License-Identifier: AGPL-3.0-only

//! Grants and detail (the suite contract, version 3, which is version 2
//! with record 42 R7's `models:see` and `models:work`): what a caller may
//! open, and how much of a record it sees. A grant names a page and how far
//! a caller goes there, `see`, or `work`, which includes see; the assistant
//! has `use`. Detail has an order: `plain` sees neither class, `quasi` the
//! quasi-identifying class and the viewer's pixels, `sensitive` the
//! sensitive class, raw identifiers and burned-in annotation.
//!
//! The ladder of Wave 4a §11.2 stays as four names, each standing for a set,
//! wherever one is still met: a `--role` binding, a named token's list, a
//! ceiling. `assist` stands for the assistant.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

/// The vocabulary, sorted by code point, as `grants.schema.json` lists it.
pub(crate) const GRANTS: [&str; 26] = [
    "assistant-settings:see",
    "assistant-settings:work",
    "assistant:use",
    "audit:see",
    // record 42 R7: a rater is not a reviewer of the whole queue
    "campaigns:see",
    "campaigns:work",
    "data:see",
    "data:work",
    "database:see",
    "database:work",
    "identity:see",
    "identity:work",
    "install:see",
    "install:work",
    "kvasir:see",
    "kvasir:work",
    "models:see",
    "models:work",
    "pipelines:see",
    "pipelines:work",
    "places:see",
    "places:work",
    "query:see",
    "query:work",
    "release:see",
    "release:work",
    "review:see",
    "review:work",
];

/// The grant a ceiling always keeps: the assistant checks it before any call.
const ASSISTANT: &str = "assistant:use";

/// What a reader holds; a reviewer holds more, an operator more again.
const READER: &[&str] = &["data:see", "query:see", "query:work"];
const REVIEWER: &[&str] = &["models:see", "pipelines:see", "review:see", "review:work"];
const OPERATOR: &[&str] = &[
    "assistant-settings:see",
    "data:work",
    "install:see",
    "kvasir:see",
    "models:work",
    "pipelines:work",
    "places:see",
    "places:work",
    "release:see",
    "release:work",
];

/// How much of a record a caller sees, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum Detail {
    #[default]
    Plain,
    Quasi,
    Sensitive,
}

impl Detail {
    pub(crate) fn parse(text: &str) -> Option<Detail> {
        Some(match text {
            "plain" => Detail::Plain,
            "quasi" => Detail::Quasi,
            "sensitive" => Detail::Sensitive,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Detail::Plain => "plain",
            Detail::Quasi => "quasi",
            Detail::Sensitive => "sensitive",
        }
    }

    /// The detail a queued job runs under: the detail the door recorded; for
    /// a job queued before grants, the detail of the highest ladder step it
    /// recorded; plain when it recorded neither, so a job with no caller on
    /// it never runs with the reach of a keyboard.
    pub(crate) fn of_job(args: &Value) -> Detail {
        match args["detail"].as_str() {
            Some(d) => Detail::parse(d).unwrap_or_default(),
            None => Detail::of_steps(
                args["roles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            ),
        }
    }

    /// The detail of the highest ladder step named; plain when none is.
    pub(crate) fn of_steps<'a>(names: impl IntoIterator<Item = &'a str>) -> Detail {
        names
            .into_iter()
            .filter_map(Step::parse)
            .map(Step::detail)
            .max()
            .unwrap_or_default()
    }
}

/// A ladder name, as a ceiling or a binding still names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Reader,
    Reviewer,
    Operator,
    Admin,
}

impl Step {
    pub(crate) fn parse(text: &str) -> Option<Step> {
        Some(match text {
            "reader" => Step::Reader,
            "reviewer" => Step::Reviewer,
            "operator" => Step::Operator,
            "admin" => Step::Admin,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Step::Reader => "reader",
            Step::Reviewer => "reviewer",
            Step::Operator => "operator",
            Step::Admin => "admin",
        }
    }

    fn detail(self) -> Detail {
        match self {
            Step::Reader => Detail::Plain,
            Step::Reviewer => Detail::Quasi,
            Step::Operator | Step::Admin => Detail::Sensitive,
        }
    }

    /// The set a ladder name stands for. Admin is every grant but the
    /// assistant, which `assist` gives.
    pub(crate) fn set(self) -> Access {
        let grants: Vec<&'static str> = match self {
            Step::Reader => READER.to_vec(),
            Step::Reviewer => [READER, REVIEWER].concat(),
            Step::Operator => [READER, REVIEWER, OPERATOR].concat(),
            Step::Admin => GRANTS.iter().copied().filter(|g| *g != ASSISTANT).collect(),
        };
        let mut access = Access {
            detail: self.detail(),
            ..Access::default()
        };
        for g in grants {
            access.give(g);
        }
        access
    }
}

/// What a caller holds: a set of grants and a detail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Access {
    pub(crate) grants: BTreeSet<&'static str>,
    pub(crate) detail: Detail,
}

impl Access {
    /// Every grant and detail sensitive: what `--auth off` and a named token
    /// with no list hold.
    pub(crate) fn everything() -> Access {
        let mut access = Access {
            detail: Detail::Sensitive,
            ..Access::default()
        };
        for g in GRANTS {
            access.give(g);
        }
        access
    }

    /// A grant of the vocabulary, or none.
    pub(crate) fn known(text: &str) -> Option<&'static str> {
        GRANTS.iter().copied().find(|g| *g == text)
    }

    /// A grant added, with its see when it is a work grant.
    fn give(&mut self, grant: &'static str) {
        self.grants.insert(grant);
        if let Some(page) = grant.strip_suffix(":work")
            && let Some(see) = Access::known(&format!("{page}:see"))
        {
            self.grants.insert(see);
        }
    }

    /// What another holds, added: the grants joined, the higher detail kept.
    pub(crate) fn add(&mut self, other: &Access) {
        self.grants.extend(other.grants.iter().copied());
        self.detail = self.detail.max(other.detail);
    }

    /// What a binding names: a ladder name or `assist` as its set, or one
    /// grant; none for anything else.
    pub(crate) fn named(text: &str) -> Option<Access> {
        if let Some(step) = Step::parse(text) {
            return Some(step.set());
        }
        let grant = if text == "assist" {
            ASSISTANT
        } else {
            Access::known(text)?
        };
        let mut access = Access::default();
        access.give(grant);
        Some(access)
    }

    /// A named token's list, `reader,query:see`: every name added up. The
    /// name that is not a ladder name, `assist` or a grant is the error.
    pub(crate) fn of_list(list: &str) -> Result<Access, String> {
        let mut access = Access::default();
        for item in list.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            access.add(&Access::named(item).ok_or_else(|| item.to_string())?);
        }
        Ok(access)
    }

    /// A verified token: its `grants` and `detail` claims taken as they are,
    /// a string outside the vocabulary dropped and an absent or unknown
    /// detail read as plain, with what its groups are bound to added.
    pub(crate) fn of_token(
        grants: Option<&Value>,
        detail: Option<&Value>,
        groups: &[String],
        bindings: &HashMap<String, Access>,
    ) -> Access {
        let mut access = Access {
            detail: detail
                .and_then(Value::as_str)
                .and_then(Detail::parse)
                .unwrap_or_default(),
            ..Access::default()
        };
        for g in grants
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(Access::known)
        {
            access.give(g);
        }
        for group in groups {
            if let Some(bound) = bindings.get(group) {
                access.add(bound);
            }
        }
        access
    }

    pub(crate) fn holds(&self, grant: &str) -> bool {
        self.grants.contains(grant)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// The grants, sorted by code point.
    pub(crate) fn list(&self) -> Vec<&'static str> {
        self.grants.iter().copied().collect()
    }

    /// `X-Nils-Ceiling`: the grants the step's set holds are kept, and the
    /// assistant, and detail is lowered to the step's. Admin's set with the
    /// assistant is every grant, so an admin ceiling keeps everything.
    pub(crate) fn narrow(&mut self, ceiling: Step) {
        let set = ceiling.set();
        self.grants
            .retain(|g| *g == ASSISTANT || set.grants.contains(g));
        self.detail = self.detail.min(set.detail);
    }

    /// For one release: the ladder steps up to the detail, never admin.
    pub(crate) fn steps(&self) -> Vec<&'static str> {
        match self.detail {
            Detail::Plain => vec!["reader"],
            Detail::Quasi => vec!["reader", "reviewer"],
            Detail::Sensitive => vec!["reader", "reviewer", "operator"],
        }
    }
}

/// What a door needs beside a detail: any grant, one grant, any of several,
/// or two at once.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Need {
    Any,
    One(&'static str),
    AnyOf(&'static [&'static str]),
    Both(&'static str, &'static str),
}

impl Need {
    pub(crate) fn met(self, access: &Access) -> bool {
        match self {
            Need::Any => !access.is_empty(),
            Need::One(g) => access.holds(g),
            Need::AnyOf(gs) => gs.iter().any(|g| access.holds(g)),
            Need::Both(a, b) => access.holds(a) && access.holds(b),
        }
    }

    /// What a refusal says was needed.
    pub(crate) fn words(self) -> String {
        match self {
            Need::Any => "a grant".to_string(),
            Need::One(g) => format!("the {g} grant"),
            Need::AnyOf(gs) => format!("one of the grants {}", gs.join(", ")),
            Need::Both(a, b) => format!("the {a} and {b} grants"),
        }
    }

    /// A policy row's `grant`: one grant, or an array meaning any of them.
    /// A door open to any grant names every grant.
    pub(crate) fn grant(self) -> Value {
        match self {
            Need::Any => Value::from(GRANTS.to_vec()),
            Need::One(g) | Need::Both(g, _) => Value::from(g),
            Need::AnyOf(gs) => Value::from(gs.to_vec()),
        }
    }

    /// A policy row's `also`: the second grant a door needs.
    pub(crate) fn also(self) -> Option<&'static str> {
        match self {
            Need::Both(_, b) => Some(b),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../../../../contracts/suite/v3/vectors/grants.json");
    const SCHEMA: &str = include_str!("../../../../contracts/suite/v3/grants.schema.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("the grants vectors")
    }

    fn strings(v: &Value) -> Vec<&str> {
        v.as_array()
            .unwrap_or_else(|| panic!("an array: {v}"))
            .iter()
            .map(|s| s.as_str().expect("a string"))
            .collect()
    }

    /// An expectation as the vectors write it: sorted grants and a detail.
    fn expected(v: &Value) -> (Vec<&str>, &str) {
        (
            strings(&v["grants"]),
            v["detail"].as_str().expect("a detail"),
        )
    }

    fn seen(a: &Access) -> (Vec<&str>, &str) {
        (a.list(), a.detail.name())
    }

    #[test]
    fn the_vocabulary_and_the_order_are_the_contract_s() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(strings(&schema["$defs"]["grant"]["enum"]), GRANTS.to_vec());
        let mut sorted = GRANTS.to_vec();
        sorted.sort();
        assert_eq!(sorted, GRANTS.to_vec(), "sorted by code point");
        assert_eq!(
            strings(&schema["order"]),
            ["plain", "quasi", "sensitive"].to_vec()
        );
        assert_eq!(
            strings(&schema["$defs"]["step"]["enum"]),
            ["reader", "reviewer", "operator", "admin"].to_vec()
        );
    }

    #[test]
    fn a_ladder_name_stands_for_the_set_the_vectors_name() {
        let v = vectors();
        for step in [Step::Reader, Step::Reviewer, Step::Operator, Step::Admin] {
            assert_eq!(
                seen(&step.set()),
                expected(&v["sets"][step.name()]),
                "{}",
                step.name()
            );
        }
        assert_eq!(
            seen(&Access::named("assist").unwrap()),
            expected(&v["sets"]["assist"])
        );
        assert_eq!(seen(&Access::everything()), expected(&v["everything"]));
    }

    #[test]
    fn claims_resolve_as_the_vectors_say() {
        let v = vectors();
        let bindings: HashMap<String, Access> = v["roles"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(group, bound)| {
                (
                    group.clone(),
                    Access::named(bound.as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        for case in v["claims"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let claims = &case["claims"];
            let groups: Vec<String> = claims["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| g.as_str().map(String::from))
                .collect();
            let access = Access::of_token(
                claims.get("grants"),
                claims.get("detail"),
                &groups,
                &bindings,
            );
            if case["expect"]["refused"] == true {
                assert!(access.is_empty(), "{name}: {access:?}");
            } else {
                assert_eq!(seen(&access), expected(&case["expect"]), "{name}");
            }
        }
    }

    #[test]
    fn a_ceiling_narrows_as_the_vectors_say() {
        let v = vectors();
        for case in v["ceilings"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut access =
                Access::of_token(case.get("grants"), case.get("detail"), &[], &HashMap::new());
            assert_eq!(seen(&access), expected(case), "{name}: as given");
            access.narrow(Step::parse(case["ceiling"].as_str().unwrap()).unwrap());
            assert_eq!(seen(&access), expected(&case["expect"]), "{name}");
        }
    }

    #[test]
    fn a_named_list_resolves_as_the_vectors_say() {
        let v = vectors();
        for case in v["named"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let access = Access::of_list(case["roles"].as_str().unwrap()).unwrap();
            if case["expect"]["refused"] == true {
                assert!(access.is_empty(), "{name}: {access:?}");
            } else {
                assert_eq!(seen(&access), expected(&case["expect"]), "{name}");
            }
        }
        assert_eq!(Access::of_list("reader,coffee"), Err("coffee".to_string()));
    }

    #[test]
    fn a_job_runs_under_the_detail_it_recorded_and_plain_without_one() {
        let job = |args: Value| Detail::of_job(&args);
        assert_eq!(job(serde_json::json!({"detail": "quasi"})), Detail::Quasi);
        assert_eq!(job(serde_json::json!({"detail": "all"})), Detail::Plain);
        assert_eq!(
            job(serde_json::json!({"roles": ["reader", "reviewer", "operator"]})),
            Detail::Sensitive
        );
        assert_eq!(job(serde_json::json!({"roles": ["reader"]})), Detail::Plain);
        assert_eq!(
            job(serde_json::json!({"argv": ["ask", "run"]})),
            Detail::Plain
        );
    }
}
