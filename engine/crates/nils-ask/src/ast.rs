// SPDX-License-Identifier: AGPL-3.0-only

//! The ask as a document (`docs/specs/wave4b-the-ask.md`, §4): a graph of
//! named sets at one grain each, JSON canonical, YAML the human rendering.
//! Every type here is serialised exactly as the spec's grammar writes it,
//! and the JSON Schema is generated from these types.

use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::{SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of this grammar. A document of an older version is upgraded
/// on read (§4.5).
pub const AST_VERSION: u32 = 1;

/// A grain: what one row of a set is (§4.2).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Grain {
    Cohort,
    Subject,
    Session,
    Stack,
    Instance,
    Event,
    Group,
    /// Reserved and staged (§3): refused at validate.
    Pair,
}

impl Grain {
    pub fn name(self) -> &'static str {
        match self {
            Grain::Cohort => "cohort",
            Grain::Subject => "subject",
            Grain::Session => "session",
            Grain::Stack => "stack",
            Grain::Instance => "instance",
            Grain::Event => "event",
            Grain::Group => "group",
            Grain::Pair => "pair",
        }
    }

    /// The ancestor grain one step up the catalog tree, for `of` (§4.4,
    /// rule 2). A cohort has none; a subject's is the cohort, through the
    /// membership edge; an event's is the subject.
    pub fn parent(self) -> Option<Grain> {
        match self {
            Grain::Cohort | Grain::Group | Grain::Pair => None,
            Grain::Subject => Some(Grain::Cohort),
            Grain::Session => Some(Grain::Subject),
            Grain::Stack => Some(Grain::Session),
            Grain::Instance => Some(Grain::Stack),
            Grain::Event => Some(Grain::Subject),
        }
    }

    /// Whether `descendant` sits below `self` on the tree, at any depth.
    pub fn is_ancestor_of(self, descendant: Grain) -> bool {
        let mut g = descendant.parent();
        while let Some(p) = g {
            if p == self {
                return true;
            }
            g = p.parent();
        }
        false
    }

    /// Whether every row of this grain carries a day (§4.2).
    pub fn dated(self) -> bool {
        matches!(
            self,
            Grain::Session | Grain::Stack | Grain::Instance | Grain::Event
        )
    }
}

impl fmt::Display for Grain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One clause, `[op, {opts}, ...args]`, the options map mandatory (§4.5).
/// A ref is a clause too: `["field", {}, path]`, `["axis", {}, name]`,
/// `["derived", {params}, name]`, `["param", {}, name]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub op: String,
    pub opts: BTreeMap<String, Value>,
    pub args: Vec<Arg>,
}

/// One argument of a clause: another clause, or a literal.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Clause(Clause),
    Text(String),
    Int(i64),
    Number(f64),
    Bool(bool),
    Null,
    List(Vec<Arg>),
}

impl Clause {
    pub fn new(op: &str) -> Clause {
        Clause {
            op: op.to_string(),
            opts: BTreeMap::new(),
            args: Vec::new(),
        }
    }

    pub fn field(path: &str) -> Clause {
        Clause::new("field").arg(Arg::Text(path.to_string()))
    }

    pub fn param(name: &str) -> Clause {
        Clause::new("param").arg(Arg::Text(name.to_string()))
    }

    pub fn arg(mut self, a: Arg) -> Clause {
        self.args.push(a);
        self
    }

    pub fn opt(mut self, key: &str, v: Value) -> Clause {
        self.opts.insert(key.to_string(), v);
        self
    }

    /// The path of a `field` ref, the name of an `axis`, `derived` or
    /// `param` ref.
    pub fn ref_name(&self) -> Option<&str> {
        match (self.op.as_str(), self.args.first()) {
            ("field" | "axis" | "derived" | "param", Some(Arg::Text(s))) => Some(s),
            _ => None,
        }
    }

    pub fn is_ref(&self) -> bool {
        matches!(self.op.as_str(), "field" | "axis" | "derived" | "param")
    }

    /// Every clause inside this one, itself first, depth first.
    pub fn walk<'a>(&'a self, out: &mut Vec<&'a Clause>) {
        out.push(self);
        for a in &self.args {
            a.walk(out);
        }
    }
}

impl Arg {
    fn walk<'a>(&'a self, out: &mut Vec<&'a Clause>) {
        match self {
            Arg::Clause(c) => c.walk(out),
            Arg::List(items) => {
                for i in items {
                    i.walk(out);
                }
            }
            _ => {}
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Arg::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for Clause {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(2 + self.args.len()))?;
        seq.serialize_element(&self.op)?;
        seq.serialize_element(&self.opts)?;
        for a in &self.args {
            seq.serialize_element(a)?;
        }
        seq.end()
    }
}

impl Serialize for Arg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Arg::Clause(c) => c.serialize(s),
            Arg::Text(t) => s.serialize_str(t),
            Arg::Int(i) => s.serialize_i64(*i),
            Arg::Number(n) => s.serialize_f64(*n),
            Arg::Bool(b) => s.serialize_bool(*b),
            Arg::Null => s.serialize_none(),
            Arg::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for i in items {
                    seq.serialize_element(i)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Clause {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Clause, D::Error> {
        let v = Value::deserialize(d)?;
        clause_of(&v).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for Arg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Arg, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Arg;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a clause or a literal")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, _: A) -> Result<Arg, A::Error> {
                unreachable!("arguments are read through a JSON value")
            }
        }
        let _ = V;
        let v = Value::deserialize(d)?;
        arg_of(&v).map_err(de::Error::custom)
    }
}

/// A JSON array whose first element is a string and second an object is a
/// clause; every other array is a list literal.
fn looks_like_clause(v: &Value) -> bool {
    matches!(v, Value::Array(items) if items.len() >= 2 && items[0].is_string() && items[1].is_object())
}

pub fn clause_of(v: &Value) -> Result<Clause, String> {
    let Value::Array(items) = v else {
        return Err(format!(
            "a clause is an array [op, {{opts}}, ...args], not {}",
            kind_of(v)
        ));
    };
    let Some(Value::String(op)) = items.first() else {
        return Err("a clause starts with its op, a string".into());
    };
    let Some(Value::Object(opts)) = items.get(1) else {
        return Err(format!(
            "clause {op}: the options map is mandatory, even when empty"
        ));
    };
    let mut args = Vec::with_capacity(items.len().saturating_sub(2));
    for a in items.iter().skip(2) {
        args.push(arg_of(a)?);
    }
    Ok(Clause {
        op: op.clone(),
        opts: opts.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        args,
    })
}

fn arg_of(v: &Value) -> Result<Arg, String> {
    Ok(match v {
        Value::Null => Arg::Null,
        Value::Bool(b) => Arg::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Arg::Int(i),
            None => Arg::Number(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => Arg::Text(s.clone()),
        Value::Array(_) if looks_like_clause(v) => Arg::Clause(clause_of(v)?),
        Value::Array(items) => Arg::List(items.iter().map(arg_of).collect::<Result<_, _>>()?),
        Value::Object(_) => {
            return Err("an object is not an argument; a ref is [\"field\", {}, path]".into());
        }
    })
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

impl JsonSchema for Clause {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Clause".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "array",
            "description": "One clause: [op, {opts}, ...args]. The options map is mandatory, even when empty. A ref is a clause: [\"field\", {}, path], [\"axis\", {}, name], [\"derived\", {params}, name], [\"param\", {}, name].",
            "prefixItems": [
                { "type": "string", "description": "the op" },
                { "type": "object", "description": "the options of the op" }
            ],
            "items": { "$ref": "#/$defs/Arg" },
            "minItems": 2
        })
    }
}

impl JsonSchema for Arg {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Arg".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "An argument of a clause: another clause, or a literal (a string, a number, a boolean, null, or a list of literals).",
            "anyOf": [
                { "$ref": "#/$defs/Clause" },
                { "type": ["string", "number", "boolean", "null"] },
                { "type": "array", "items": { "$ref": "#/$defs/Arg" } }
            ]
        })
    }
}

/// A sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Asc,
    Desc,
}

/// One ordering term, `[clause, dir]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Order(pub Clause, pub Dir);

/// Where a set starts (§4.3, `from`): a same grain set, or a library set, a
/// handle, a saved ask or an uploaded list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Src {
    Set(String),
    Role(String),
    Handle { id: String, pin: bool },
    Selection { name: String, version: Option<u64> },
    Values(String),
}

impl Src {
    pub fn parse(text: &str) -> Src {
        if let Some(r) = text.strip_prefix("role:") {
            Src::Role(r.to_string())
        } else if let Some(h) = text.strip_prefix("handle:") {
            Src::Handle {
                id: h.to_string(),
                pin: false,
            }
        } else if let Some(s) = text.strip_prefix("selection:") {
            match s.rsplit_once('@') {
                Some((name, v)) => match v.parse::<u64>() {
                    Ok(version) => Src::Selection {
                        name: name.to_string(),
                        version: Some(version),
                    },
                    Err(_) => Src::Selection {
                        name: s.to_string(),
                        version: None,
                    },
                },
                None => Src::Selection {
                    name: s.to_string(),
                    version: None,
                },
            }
        } else if let Some(v) = text.strip_prefix("values:") {
            Src::Values(v.to_string())
        } else {
            Src::Set(text.to_string())
        }
    }
}

impl fmt::Display for Src {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Src::Set(s) => f.write_str(s),
            Src::Role(r) => write!(f, "role:{r}"),
            Src::Handle { id, .. } => write!(f, "handle:{id}"),
            Src::Selection {
                name,
                version: Some(v),
            } => write!(f, "selection:{name}@{v}"),
            Src::Selection {
                name,
                version: None,
            } => write!(f, "selection:{name}"),
            Src::Values(v) => write!(f, "values:{v}"),
        }
    }
}

impl Serialize for Src {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Src::Handle { id, pin: true } => {
                let mut m = serde_json::Map::new();
                m.insert("handle".into(), Value::String(id.clone()));
                m.insert("pin".into(), Value::Bool(true));
                Value::Object(m).serialize(s)
            }
            other => s.serialize_str(&other.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for Src {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Src, D::Error> {
        let v = Value::deserialize(d)?;
        match &v {
            Value::String(s) => Ok(Src::parse(s)),
            Value::Object(m) => {
                let id = m
                    .get("handle")
                    .and_then(Value::as_str)
                    .ok_or_else(|| de::Error::custom("a source object is {handle, pin}"))?;
                Ok(Src::Handle {
                    id: id.to_string(),
                    pin: m.get("pin").and_then(Value::as_bool).unwrap_or(false),
                })
            }
            other => Err(de::Error::custom(format!(
                "a source is a name, role:<r>, handle:<id>, selection:<n>@<v>, values:<v> or {{handle, pin}}, not {}",
                kind_of(other)
            ))),
        }
    }
}

impl JsonSchema for Src {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Src".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Where a set starts: a same grain set by name, role:<r>, handle:<id>, selection:<n>@<v>, values:<v>, or {handle, pin} to replay a handle from its stored rows.",
            "anyOf": [
                { "type": "string" },
                { "type": "object", "properties": { "handle": { "type": "string" }, "pin": { "type": "boolean" } }, "required": ["handle"], "additionalProperties": false }
            ]
        })
    }
}

/// A window unit (§5.1): `day` is integer arithmetic, `month` is 31 days,
/// `year` is 366, both ends inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Day,
    Month,
    Year,
}

impl Unit {
    /// The unit in days (§5.1).
    pub fn days(self) -> i64 {
        match self {
            Unit::Day => 1,
            Unit::Month => 31,
            Unit::Year => 366,
        }
    }
}

/// A signed window with a declared unit; a null bound is unbounded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Window {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    pub unit: Unit,
}

impl Window {
    /// The window in days, both ends inclusive.
    pub fn days(&self) -> (Option<i64>, Option<i64>) {
        let d = self.unit.days();
        (self.from.map(|f| f * d), self.to.map(|t| t * d))
    }
}

/// A window, or a `param` ref to one, which desugar inlines (§4.4, rule 13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WindowSpec {
    Literal(Window),
    Param(Clause),
}

/// An integer, or a `param` ref to one, which binds at run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IntSpec {
    Literal(i64),
    Param(Clause),
}

/// The policy of `near` (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    Nearest,
    First,
    Last,
    Best,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tie {
    Earlier,
    Later,
}

/// The one row of a dated set of the same subject inside a signed window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Near {
    #[serde(rename = "as")]
    pub as_: String,
    pub set: String,
    pub window: WindowSpec,
    /// The binding the window is measured from, instead of this set's day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    pub policy: Policy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tie: Option<Tie>,
    /// For `best`: an explicit order over anchor and partner bindings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<Order>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// The certain reading against a coarse date (§5.3); forgiving by
    /// default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
}

/// The one row of a descendant set picked per this grain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attach {
    #[serde(rename = "as")]
    pub as_: String,
    pub set: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

/// The count of related rows, optionally windowed, bounded (§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Has {
    pub set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<IntSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<IntSpec>,
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub as_: Option<String>,
}

/// Sugar (§6): at least `min` rows of a descendant set sharing one value of
/// the `by` tuple; `all` asks that every row shares one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Same {
    #[serde(rename = "as")]
    pub as_: String,
    pub over: String,
    pub by: Vec<Clause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<IntSpec>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
}

/// Sugar (§4.3): every row of `of` is in `in`; three clauses, so it is not
/// vacuously true on an empty universe unless `vacuous` says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Every {
    pub of: String,
    #[serde(rename = "in")]
    pub in_: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub vacuous: bool,
}

/// Sugar (§4.3, §13.2): the later row of this set inside a window, through
/// a hidden clone and `near best`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pairs {
    #[serde(rename = "as")]
    pub as_: String,
    pub window: WindowSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<Order>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Ties {
    Report,
    Refuse,
}

/// The first n rows per ancestor under an explicit preference list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pick {
    pub per: Grain,
    pub by: Vec<Order>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<IntSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ties: Option<Ties>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AlgOp {
    Union,
    Intersect,
    Except,
}

/// Set algebra at one grain; `tag` unpivots one row per operand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Algebra {
    pub op: AlgOp,
    pub sets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// The distinct by-tuples of a child set, with aggregates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    pub of: String,
    pub by: Vec<Clause>,
}

/// Bindings in the order written, because `where` reads everything bound
/// before it (§4.4, rule 5).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bindings(pub Vec<(String, Clause)>);

impl Bindings {
    pub fn get(&self, name: &str) -> Option<&Clause> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _)| n.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Bindings {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Bindings {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Bindings, D::Error> {
        let v = Value::deserialize(d)?;
        let Value::Object(m) = v else {
            return Err(de::Error::custom("bind is a map of name to clause"));
        };
        let mut out = Vec::with_capacity(m.len());
        for (k, v) in m {
            out.push((k, clause_of(&v).map_err(de::Error::custom)?));
        }
        Ok(Bindings(out))
    }
}

impl JsonSchema for Bindings {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Bindings".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "description": "Bindings, in the order written: a name to a clause. A later binding and the where clauses read every earlier one.",
            "additionalProperties": { "$ref": "#/$defs/Clause" }
        })
    }
}

/// One named set (§4.3). Inside a set the order is source, near, attach,
/// has, same, bind, where, pick (§4.4, rule 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub grain: Grain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Src>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algebra: Option<Algebra>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub near: Vec<Near>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attach: Vec<Attach>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub has: Vec<Has>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub same: Vec<Same>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub every: Vec<Every>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairs: Option<Pairs>,
    #[serde(default, skip_serializing_if = "Bindings::is_empty")]
    pub bind: Bindings,
    #[serde(rename = "where", default, skip_serializing_if = "Vec::is_empty")]
    pub where_: Vec<Clause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<Pick>,
}

impl Set {
    pub fn at(grain: Grain) -> Set {
        Set {
            grain,
            from: None,
            of: None,
            algebra: None,
            group: None,
            near: Vec::new(),
            attach: Vec::new(),
            has: Vec::new(),
            same: Vec::new(),
            every: Vec::new(),
            pairs: None,
            bind: Bindings::default(),
            where_: Vec::new(),
            pick: None,
        }
    }

    /// The other sets this one reads, for the DAG.
    pub fn reads(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        if let Some(Src::Set(s)) = &self.from {
            out.push(s);
        }
        if let Some(o) = &self.of {
            out.push(o);
        }
        if let Some(a) = &self.algebra {
            out.extend(a.sets.iter().map(String::as_str));
        }
        if let Some(g) = &self.group {
            out.push(&g.of);
        }
        out.extend(self.near.iter().map(|n| n.set.as_str()));
        out.extend(self.attach.iter().map(|a| a.set.as_str()));
        out.extend(self.has.iter().map(|h| h.set.as_str()));
        out.extend(self.same.iter().map(|s| s.over.as_str()));
        for e in &self.every {
            out.push(&e.of);
            out.push(&e.in_);
        }
        // aggregates in bind name a set in their options
        for (_, c) in &self.bind.0 {
            let mut all = Vec::new();
            c.walk(&mut all);
            for c in all {
                if let Some(Value::String(s)) = c.opts.get("set") {
                    out.push(s);
                }
                if let Some(Value::String(s)) = c.opts.get("over") {
                    out.push(s);
                }
            }
        }
        out
    }
}

/// The type of a parameter (§4.5). The last three are structural and
/// desugar into the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Text,
    Integer,
    Number,
    Date,
    List,
    Cohort,
    Window,
    Rounding,
    Level,
}

impl ParamType {
    pub fn structural(self) -> bool {
        matches!(
            self,
            ParamType::Window | ParamType::Rounding | ParamType::Level
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    #[serde(rename = "type")]
    pub type_: ParamType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// An uploaded identifier list, by reference (§4.3, C41).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValuesDecl {
    pub upload: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Boolean,
    Count,
    Aggregate,
    Record,
}

/// A post pass measure on the answer: `{share: {of, over}}`, `{stddev: {of}}`,
/// `{median: {of}}`, `{percentile: {of, p}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Measure(pub BTreeMap<String, Value>);

/// The answer's shape (§4.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Out {
    pub set: String,
    pub level: Level,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Clause>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<Measure>,
    /// Identifier namespaces to project raw, role gated with an audit row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<Order>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// The one session scheme the whole ask is read under (§4.4, rule 12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemeRef {
    Name(String),
    Inline(BTreeMap<String, Value>),
}

/// One stage of the `pipeline` sugar: a set whose source is the stage
/// before it (C17's chain shaped graph).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub set: Set,
}

/// The ask (§4.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub ast_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<SchemeRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamDecl>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, ValuesDecl>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<Stage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sets: BTreeMap<String, Set>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep: Vec<String>,
    pub out: Out,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Ask {
    /// The canonical JSON text: keys sorted, no whitespace.
    pub fn canonical(&self) -> String {
        let v = serde_json::to_value(self).expect("an ask serializes");
        canonical_json(&v)
    }
}

/// JSON with every object's keys sorted and no whitespace, so that a hash
/// covers the document and not the writer's order.
pub fn canonical_json(v: &Value) -> String {
    fn write(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                out.push('{');
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k).expect("a key"));
                    out.push(':');
                    write(&m[*k], out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).expect("a scalar")),
        }
    }
    let mut out = String::new();
    write(v, &mut out);
    out
}
