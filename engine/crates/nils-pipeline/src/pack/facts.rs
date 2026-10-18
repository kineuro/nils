// SPDX-License-Identifier: AGPL-3.0-only

//! The facts a pack's expressions read (spec section 5.4), their types and
//! vocabularies, and a unit's facts as a provider fills them.
//!
//! Facts come from the registry, never from a pack's own reading of a
//! header. The provider is the binary's (it reads the fingerprint rows, the
//! classification axes, the post-contrast label, the clinical imports and
//! the host); this module fixes the names, the types and the words, so the
//! type-check at load and the evaluation at pre-flight read one vocabulary.
//! A fact the registry cannot tell is [`Val::Unknown`].

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

/// The facts of a stack in a role, each as `<role>.<name>`.
pub const STACK_FACTS: [&str; 11] = [
    "base",
    "dimension",
    "voxel_max_mm",
    "voxel_min_mm",
    "slice_thickness_mm",
    "field_strength_t",
    "post_contrast",
    "vendor",
    "body_part",
    "skull_stripped",
    "defaced",
];

/// `dimension`: how the stack was acquired.
pub const DIMENSIONS: [&str; 2] = ["2D", "3D"];

/// `post_contrast`: the post-contrast label of Wave 7a (record 55 F2, F5).
pub const POST_CONTRAST: [&str; 2] = ["given", "not_given"];

/// `base`: the MRI pack's base axis (its values as of pack 1.2); the engine
/// widens it from the pack it reads at load, so a newer pack's word is not
/// refused.
pub const BASES: [&str; 11] = [
    "DWI", "PWI", "ASL", "SWI", "T1rho", "T2rho", "T2starw", "T2w", "PDw", "T1w", "MTw",
];

/// `body_part`: the MRI pack's body-part axis.
pub const BODY_PARTS: [&str; 6] = ["neck", "spine", "brain", "brain-neck", "chest", "other"];

/// `vendor`: the scanner's maker, from the fingerprint's manufacturer text
/// (2026-10-18, this slice's default: the registry keeps the text and no
/// vocabulary of its own).
pub const VENDORS: [&str; 8] = [
    "siemens",
    "ge",
    "philips",
    "canon",
    "hitachi",
    "fujifilm",
    "united_imaging",
    "other",
];

/// `host.runtime`: the container runtimes the engine runs.
pub const RUNTIMES: [&str; 3] = ["podman", "apptainer", "docker"];

/// The person's `goal`, where a pack names none.
pub const GOALS: [&str; 3] = ["fast", "standard", "research"];

/// The pick roles every pack may ask `present` and `aligned` about, beside
/// the descriptor's own; `stack` is a unit of the stacks layout, whose one
/// stack has no pick role.
pub const ROLES: [&str; 4] = ["t1w", "flair", "t2w", "stack"];

/// The vendor a manufacturer text names.
pub fn vendor_of(manufacturer: &str) -> &'static str {
    let m = manufacturer.to_ascii_lowercase();
    if m.contains("siemens") {
        "siemens"
    } else if m.contains("ge ") || m.starts_with("ge") || m.contains("general electric") {
        "ge"
    } else if m.contains("philips") {
        "philips"
    } else if m.contains("canon") || m.contains("toshiba") {
        "canon"
    } else if m.contains("hitachi") {
        "hitachi"
    } else if m.contains("fuji") {
        "fujifilm"
    } else if m.contains("united imaging") || m.contains("uih") {
        "united_imaging"
    } else {
        "other"
    }
}

/// What type a fact has, and for a text with a vocabulary, its words.
#[derive(Debug, Clone, PartialEq)]
pub enum FactType {
    Number,
    Bool,
    Text(Option<Vec<String>>),
}

/// The person's choices a pack offers (`goals`): the goal's values and the
/// switches, each with its values; the first value of each is its default.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Goals {
    pub goal: Vec<String>,
    pub switches: BTreeMap<String, Vec<Value>>,
}

impl Goals {
    /// The goal a run takes when nobody chose: `standard` where the pack
    /// offers it (57-Q3), else the first it names.
    pub fn default_goal(&self) -> String {
        if self.goal.iter().any(|g| g == "standard") || self.goal.is_empty() {
            "standard".into()
        } else {
            self.goal[0].clone()
        }
    }
}

/// The vocabulary expressions are checked against: every fact by name with
/// its type, and the roles `present` and `aligned` may name.
#[derive(Debug, Clone, PartialEq)]
pub struct Vocabulary {
    pub facts: BTreeMap<String, FactType>,
    pub roles: BTreeSet<String>,
}

fn words(list: &[&str]) -> FactType {
    FactType::Text(Some(list.iter().map(|w| w.to_string()).collect()))
}

impl Vocabulary {
    /// The vocabulary for a pipeline whose descriptor names `roles`
    /// (`x-nils.input.roles`), with the pick roles of [`ROLES`] beside
    /// them, and the goals a pack offers.
    pub fn new(roles: &[String], goals: &Goals) -> Vocabulary {
        let mut all: BTreeSet<String> = roles.iter().cloned().collect();
        all.extend(ROLES.iter().map(|r| r.to_string()));
        let mut facts: BTreeMap<String, FactType> = BTreeMap::new();
        for role in &all {
            for fact in STACK_FACTS {
                let ty = match fact {
                    "base" => words(&BASES),
                    "dimension" => words(&DIMENSIONS),
                    "post_contrast" => words(&POST_CONTRAST),
                    "vendor" => words(&VENDORS),
                    "body_part" => words(&BODY_PARTS),
                    "skull_stripped" | "defaced" => FactType::Bool,
                    _ => FactType::Number,
                };
                facts.insert(format!("{role}.{fact}"), ty);
            }
        }
        facts.insert("age_years".into(), FactType::Number);
        facts.insert("diagnosis".into(), FactType::Text(None));
        facts.insert("host.cores".into(), FactType::Number);
        facts.insert("host.memory_gb".into(), FactType::Number);
        facts.insert("host.gpu".into(), FactType::Bool);
        facts.insert("host.gpu_memory_gb".into(), FactType::Number);
        facts.insert("host.runtime".into(), words(&RUNTIMES));
        let goal: Vec<String> = if goals.goal.is_empty() {
            GOALS.iter().map(|g| g.to_string()).collect()
        } else {
            goals.goal.clone()
        };
        facts.insert("goal".into(), FactType::Text(Some(goal)));
        for (name, values) in &goals.switches {
            let ty = if values.iter().all(Value::is_boolean) {
                FactType::Bool
            } else if values.iter().all(Value::is_number) {
                FactType::Number
            } else {
                FactType::Text(Some(values.iter().map(text_of).collect()))
            };
            facts.insert(name.clone(), ty);
        }
        Vocabulary { facts, roles: all }
    }

    /// The vocabulary with a stack fact's words widened by `more` for every
    /// role: how the engine lets a pack use the words of the MRI pack it
    /// reads (`base`, `body_part`) rather than this crate's list.
    pub fn widened(mut self, fact: &str, more: &[String]) -> Vocabulary {
        for role in self.roles.clone() {
            if let Some(FactType::Text(Some(list))) = self.facts.get_mut(&format!("{role}.{fact}")) {
                for w in more {
                    if !list.contains(w) {
                        list.push(w.clone());
                    }
                }
            }
        }
        self
    }

    /// Whether a role may be named by `present` or `aligned`.
    pub fn role(&self, role: &str) -> Result<(), String> {
        if self.roles.contains(role) {
            Ok(())
        } else {
            Err(format!(
                "`{role}` is not a role here; the roles are {}",
                self.roles.iter().cloned().collect::<Vec<_>>().join(", ")
            ))
        }
    }

    /// The facts, named for a refusal: the stack facts once as
    /// `<role>.<fact>`, then the bare names.
    pub fn facts_named(&self) -> String {
        let mut out: Vec<String> = vec![format!("<role>.{}", STACK_FACTS.join(", <role>."))];
        out.extend(
            self.facts
                .keys()
                .filter(|k| !self.roles.iter().any(|r| k.starts_with(&format!("{r}."))))
                .cloned(),
        );
        format!(
            "{} (the roles: {})",
            out.join(", "),
            self.roles.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    }
}

/// A switch's value as text, for a vocabulary.
pub fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A value an expression evaluates to.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Number(f64),
    Text(String),
    Bool(bool),
    List(Vec<Val>),
    /// A fact the registry cannot tell.
    Unknown,
}

impl Val {
    pub fn to_json(&self) -> Value {
        match self {
            Val::Number(n) => json!(n),
            Val::Text(t) => json!(t),
            Val::Bool(b) => json!(b),
            Val::List(items) => Value::Array(items.iter().map(Val::to_json).collect()),
            Val::Unknown => Value::Null,
        }
    }

    pub fn from_json(v: &Value) -> Val {
        match v {
            Value::Null => Val::Unknown,
            Value::Bool(b) => Val::Bool(*b),
            Value::Number(n) => n.as_f64().map_or(Val::Unknown, Val::Number),
            Value::String(s) => Val::Text(s.clone()),
            Value::Array(items) => Val::List(items.iter().map(Val::from_json).collect()),
            Value::Object(_) => Val::Unknown,
        }
    }

    /// A number as a fact, where the registry holds one.
    pub fn number(n: Option<f64>) -> Val {
        n.filter(|n| n.is_finite()).map_or(Val::Unknown, Val::Number)
    }

    /// A text as a fact, where the registry holds one that is not empty.
    pub fn text(t: Option<&str>) -> Val {
        t.map(str::trim)
            .filter(|t| !t.is_empty())
            .map_or(Val::Unknown, |t| Val::Text(t.to_string()))
    }
}

/// A unit's facts, as the provider filled them: each fact by its resolved
/// name, the roles present, and what is known of each pair's alignment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    pub values: BTreeMap<String, Val>,
    pub roles: BTreeSet<String>,
    /// `(a, b)` to whether a is aligned to b: true, false, or unknown
    /// where no registration step or check has said.
    pub aligned: BTreeMap<(String, String), Val>,
}

impl Facts {
    pub fn get(&self, name: &str) -> Val {
        self.values.get(name).cloned().unwrap_or(Val::Unknown)
    }

    pub fn set(&mut self, name: &str, v: Val) {
        self.values.insert(name.to_string(), v);
    }

    pub fn present(&self, role: &str) -> bool {
        self.roles.contains(role)
    }

    pub fn aligned(&self, a: &str, b: &str) -> Val {
        self.aligned
            .get(&(a.to_string(), b.to_string()))
            .or_else(|| self.aligned.get(&(b.to_string(), a.to_string())))
            .cloned()
            .unwrap_or(Val::Unknown)
    }

    pub fn set_aligned(&mut self, a: &str, b: &str, v: Val) {
        self.aligned.insert((a.to_string(), b.to_string()), v);
    }

    /// The facts as a unit records them: `{values: {name: value}, roles:
    /// [..], aligned: {"a:b": value}}`; a fact the registry could not tell
    /// is null.
    pub fn to_json(&self) -> Value {
        json!({
            "values": self.values.iter().map(|(k, v)| (k.clone(), v.to_json())).collect::<serde_json::Map<_, _>>(),
            "roles": self.roles,
            "aligned": self.aligned.iter().map(|((a, b), v)| (format!("{a}:{b}"), v.to_json())).collect::<serde_json::Map<_, _>>(),
        })
    }

    /// The facts a unit recorded, read back for a replay (G3).
    pub fn from_json(v: &Value) -> Facts {
        let mut f = Facts::default();
        if let Some(values) = v["values"].as_object() {
            for (k, x) in values {
                f.values.insert(k.clone(), Val::from_json(x));
            }
        }
        for r in v["roles"].as_array().into_iter().flatten() {
            if let Some(r) = r.as_str() {
                f.roles.insert(r.to_string());
            }
        }
        if let Some(pairs) = v["aligned"].as_object() {
            for (k, x) in pairs {
                if let Some((a, b)) = k.split_once(':') {
                    f.set_aligned(a, b, Val::from_json(x));
                }
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_names_every_fact_of_the_spec_and_widens() {
        let v = Vocabulary::new(&["t1w".into()], &Goals::default());
        for fact in STACK_FACTS {
            assert!(v.facts.contains_key(&format!("t1w.{fact}")), "{fact}");
            assert!(v.facts.contains_key(&format!("stack.{fact}")), "{fact}");
        }
        for bare in ["age_years", "diagnosis", "goal", "host.cores", "host.memory_gb", "host.gpu", "host.gpu_memory_gb", "host.runtime"] {
            assert!(v.facts.contains_key(bare), "{bare}");
        }
        assert_eq!(v.facts["goal"], FactType::Text(Some(GOALS.iter().map(|g| g.to_string()).collect())));
        let v = v.widened("base", &["T2map".into()]);
        let FactType::Text(Some(words)) = &v.facts["t1w.base"] else {
            panic!()
        };
        assert!(words.contains(&"T2map".to_string()) && words.contains(&"T1w".to_string()));
        assert_eq!(vendor_of("SIEMENS"), "siemens");
        assert_eq!(vendor_of("GE MEDICAL SYSTEMS"), "ge");
        assert_eq!(vendor_of("Philips Medical Systems"), "philips");
        assert_eq!(vendor_of("TOSHIBA"), "canon");
        assert_eq!(vendor_of("SYNTHETIC"), "other");
    }

    #[test]
    fn facts_round_trip_through_json() {
        let mut f = Facts::default();
        f.set("t1w.dimension", Val::Text("3D".into()));
        f.set("t1w.field_strength_t", Val::Unknown);
        f.roles.insert("t1w".into());
        f.set_aligned("flair", "t1w", Val::Bool(true));
        let back = Facts::from_json(&f.to_json());
        assert_eq!(back, f);
        assert_eq!(back.aligned("t1w", "flair"), Val::Bool(true));
        assert_eq!(back.get("t1w.field_strength_t"), Val::Unknown);
    }
}
