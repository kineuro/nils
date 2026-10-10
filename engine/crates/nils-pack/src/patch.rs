// SPDX-License-Identifier: AGPL-3.0-only

//! Rule edits as typed operations (record 56, section 5.5): a pack patch.
//!
//! Most changes to the rules are a handful of kinds: a word added to what a
//! value's rule reads, a rule or a rule set moved in the order, a value's
//! priority in its exclusion group, a new value, a new rule from a template,
//! an axis silenced where a condition holds, an axis handed to a model, a
//! value's name in a release, and in the main-scan pick (the 2026-10-10
//! study of the pick borders) which stacks of a role compete, a border kind
//! taken out, a near tie's order and its margin. Each is an operation here,
//! written as data,
//! so a proposal is a reviewable diff of the pack that a form, a person or
//! the assistant can write, and that the engine checks before anything runs.
//!
//! A patch names the pack and the version it was written against, and lists
//! its operations in order, each with its **scope** (the whole pack, or a
//! site, a dataset or a scanner, which is an overlay), its **reason** and
//! its **evidence** (the proposal or the finding it came from). Applying one
//! rewrites the pack's own documents, the manifest, an axis file, a rule
//! set, the BIDS mapping or a pass, as the operation says, and the pack's
//! own loader builds the result from the pack's directory with those
//! documents in place ([`crate::pack::load_patched`]). What the loader
//! refuses, the patch is refused for, with the loader's why; an operation
//! that cannot apply at all (a value the axis does not have, a word already
//! there, a move to where a set already runs) is refused before that, with
//! its own. The pack's corpus, and the patch's own `cases`, judge the result
//! as they judge an overlay: their failures are an answer, not a refusal.
//!
//! An overlay document (pack contract 5) is a patch of word edits scoped to
//! an origin, and reads as one ([`Patch::from_overlay`]): its bucket edits
//! are `add_words` and `remove_words` on the bucket, its list edits the same
//! on `axis.value`. The overlay documents stay what they were and load as
//! they did.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::corpus::Case;
use crate::error::{Error, R};
use crate::pack::Pack;
use crate::yaml::{self, File};

/// The patch format's version, which a patch states as `patch:`.
pub const FORMAT: u64 = 1;

/// The operations, by the name a patch writes.
pub const KINDS: &[&str] = &[
    "add_words",
    "remove_words",
    "move_rule",
    "move_set",
    "set_priority",
    "add_value",
    "add_rule",
    "silence",
    "by_model",
    "map_name",
    "set_candidates",
    "remove_border",
    "set_near_tie",
    "set_runner_up_within",
];

/// The operations on a pick file, which are pack edits only: a pick's file
/// is one for every stack and its population the whole registry's.
const PICK_KINDS: &[&str] = &[
    "set_candidates",
    "remove_border",
    "set_near_tie",
    "set_runner_up_within",
];

/// The border kinds a pick file may declare under `borders`, by the reason
/// a pick raises (as its review item names it) and the key the file writes.
/// `too_close` is the `runner_up_within` every pick declares, set rather
/// than removed.
const BORDERS: &[(&str, &str)] = &[
    ("rare", "rare_within"),
    ("retake", "retake"),
    ("unknown_dim", "unknown_dim"),
    ("slice_count_outlier", "slice_outlier"),
    ("pre_post_twin", "pre_post_twin"),
    ("epimix_fallback", "fallback"),
    ("dixon_vs_plain", "dixon_vs_plain"),
];

/// What every operation says beside its own keys.
const COMMON: &[&str] = &["op", "scope", "reason", "evidence"];

/// The keys each operation reads, beside [`COMMON`]: any other is refused.
fn keys_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "add_words" | "remove_words" => &["axis", "value", "bucket", "words", "field", "rule"],
        "move_rule" => &["rule", "set", "before", "after", "position"],
        "move_set" => &["set", "before", "after", "position"],
        "set_priority" => &["axis", "value", "priority"],
        "add_value" => &[
            "axis",
            "value",
            "label",
            "family",
            "description",
            "terms",
            "aliases",
            "group",
            "priority",
            "words",
            "before",
            "after",
            "position",
            "bids",
        ],
        "add_rule" => &[
            "axis",
            "value",
            "nothing",
            "when",
            "set",
            "rule",
            "before",
            "after",
            "position",
            "confidence",
            "why",
        ],
        "silence" => &["axis", "when"],
        "by_model" => &["axis"],
        "map_name" => &["axis", "value", "bids", "label"],
        "set_candidates" => &["pick", "role", "when", "unless"],
        "remove_border" => &["pick", "border"],
        "set_near_tie" => &["pick", "order"],
        "set_runner_up_within" => &["pick", "within"],
        _ => &[],
    }
}

/// The keys an operation of this kind reads: the ones every operation
/// says, then its own. Empty for a kind that is no operation.
pub fn reads(kind: &str) -> Vec<&'static str> {
    if !KINDS.contains(&kind) {
        return Vec::new();
    }
    COMMON.iter().chain(keys_of(kind).iter()).copied().collect()
}

/// What a patch document says, at its top.
pub fn says() -> &'static [&'static str] {
    SAYS
}

/// What a scanner scope may be keyed on: the fingerprint's origin, as an
/// overlay's scope is (C2), and an overlay's batch.
pub const SCANNER_KEYS: &[&str] = &["manufacturer", "model", "station", "batch"];

/// Where an operation applies (record 56 §5.2, §5.6: an overlay never
/// leaves its scope).
///
/// - `pack`: every stack; a pack edit, which ships as a rules release.
/// - `site:NAME`: the datasets under one source root. A site's deliveries
///   arrive under its root, each folder under it a dataset (Wave 7a), so a
///   site is known to the registry by that root.
/// - `dataset:NAME[,NAME]`: the stacks of these datasets, by name or id.
/// - `scanner:KEY=VALUE[,KEY=VALUE]`: the stacks whose manufacturer, model
///   or station is the one named (each key that is named, case and
///   surrounding spaces aside), and an overlay's `batch`; `batch:ID` alone
///   is the same as `scanner:batch=ID`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Pack,
    Site(String),
    Dataset(Vec<String>),
    Scanner(Vec<(String, String)>),
}

impl Scope {
    /// A scope from its text: `pack`, `site:NAME`, `dataset:NAME[,NAME]`,
    /// `scanner:KEY=VALUE[,KEY=VALUE]` or `batch:ID`.
    pub fn parse(text: &str) -> Result<Scope, String> {
        let text = text.trim();
        if text == "pack" || text.is_empty() {
            return Ok(Scope::Pack);
        }
        let (kind, rest) = text.split_once(':').ok_or_else(|| {
            format!("{text} is not a scope: pack, site:NAME, dataset:NAME, scanner:KEY=VALUE or batch:ID")
        })?;
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(format!("{kind}: names nothing"));
        }
        match kind.trim() {
            "site" => Ok(Scope::Site(rest.to_string())),
            "dataset" => {
                let names: Vec<String> = rest
                    .split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect();
                if names.is_empty() {
                    return Err("dataset: names nothing".into());
                }
                Ok(Scope::Dataset(names))
            }
            "scanner" => {
                let mut keys = Vec::new();
                for part in rest.split(',') {
                    let (k, v) = part.split_once('=').ok_or_else(|| {
                        format!(
                            "scanner:{rest}: each part is KEY=VALUE, a key one of {}",
                            SCANNER_KEYS.join(", ")
                        )
                    })?;
                    keys.push((k.trim().to_string(), v.trim().to_string()));
                }
                Scope::scanner(keys)
            }
            "batch" => Scope::scanner(vec![("batch".into(), rest.to_string())]),
            other => Err(format!(
                "{other}: a scope is pack, site:NAME, dataset:NAME, scanner:KEY=VALUE or batch:ID"
            )),
        }
    }

    fn scanner(mut keys: Vec<(String, String)>) -> Result<Scope, String> {
        for (k, v) in &keys {
            if !SCANNER_KEYS.contains(&k.as_str()) {
                return Err(format!(
                    "{k} is not what a scanner scope is keyed on: {}",
                    SCANNER_KEYS.join(", ")
                ));
            }
            if v.is_empty() {
                return Err(format!("scanner:{k}= names nothing"));
            }
            if k == "batch" && v.parse::<i64>().is_err() {
                return Err(format!("batch {v} is not a batch id"));
            }
        }
        keys.sort();
        keys.dedup();
        if keys.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err("a scanner scope names each key once".into());
        }
        Ok(Scope::Scanner(keys))
    }

    /// A scope as a patch writes it: its text, or a mapping such as
    /// `{dataset: X}`, `{site: X}`, `{scanner: {station: X}}`, `{batch: 7}`.
    pub fn of(v: &Value) -> Result<Scope, String> {
        match v {
            Value::Null => Ok(Scope::Pack),
            Value::String(s) => Scope::parse(s),
            Value::Object(m) => {
                if m.len() != 1 {
                    return Err(
                        "a scope is one of pack, site, dataset, scanner or batch".to_string()
                    );
                }
                let (k, v) = m.iter().next().expect("one key");
                match (k.as_str(), v) {
                    ("pack", _) => Ok(Scope::Pack),
                    ("site", Value::String(s)) => Scope::parse(&format!("site:{s}")),
                    ("dataset", Value::String(s)) => Scope::parse(&format!("dataset:{s}")),
                    ("dataset", Value::Array(a)) => {
                        let names: Vec<String> = a
                            .iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect();
                        Scope::parse(&format!("dataset:{}", names.join(",")))
                    }
                    ("batch", b) => Scope::parse(&format!(
                        "batch:{}",
                        b.as_i64()
                            .map(|n| n.to_string())
                            .or_else(|| b.as_str().map(str::to_string))
                            .unwrap_or_default()
                    )),
                    ("scanner", Value::Object(keys)) => {
                        let mut out = Vec::new();
                        for (k, v) in keys {
                            let v = match v {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                _ => return Err(format!("scanner.{k} is a word")),
                            };
                            out.push((k.clone(), v));
                        }
                        Scope::scanner(out)
                    }
                    ("scanner", Value::String(s)) => Scope::parse(&format!("scanner:{s}")),
                    (other, _) => Err(format!(
                        "{other} is not a scope: pack, site, dataset, scanner or batch"
                    )),
                }
            }
            _ => Err("a scope is a word or a mapping".into()),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Scope::Pack => "pack".into(),
            Scope::Site(s) => format!("site:{s}"),
            Scope::Dataset(d) => format!("dataset:{}", d.join(",")),
            Scope::Scanner(keys) if keys.len() == 1 && keys[0].0 == "batch" => {
                format!("batch:{}", keys[0].1)
            }
            Scope::Scanner(keys) => format!(
                "scanner:{}",
                keys.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    pub fn is_pack(&self) -> bool {
        matches!(self, Scope::Pack)
    }

    /// The kind of reach, in a word: `pack`, `site`, `dataset`, `scanner`.
    pub fn kind(&self) -> &'static str {
        match self {
            Scope::Pack => "pack",
            Scope::Site(_) => "site",
            Scope::Dataset(_) => "dataset",
            Scope::Scanner(_) => "scanner",
        }
    }
}

/// Where a moved rule, rule set or value goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Anchor {
    Before(String),
    After(String),
    First,
    Last,
}

impl Anchor {
    fn of(m: &Map<String, Value>, required: bool) -> Result<Option<Anchor>, String> {
        let before = m.get("before").and_then(Value::as_str);
        let after = m.get("after").and_then(Value::as_str);
        let position = m.get("position").and_then(Value::as_str);
        let named = [before.is_some(), after.is_some(), position.is_some()]
            .iter()
            .filter(|x| **x)
            .count();
        if named > 1 {
            return Err("name one place: before, after or position".into());
        }
        Ok(match (before, after, position) {
            (Some(b), _, _) => Some(Anchor::Before(b.to_string())),
            (_, Some(a), _) => Some(Anchor::After(a.to_string())),
            (_, _, Some("first")) => Some(Anchor::First),
            (_, _, Some("last")) => Some(Anchor::Last),
            (_, _, Some(other)) => {
                return Err(format!("position is first or last, not {other}"));
            }
            _ if required => return Err("say where it goes: before, after or position".into()),
            _ => None,
        })
    }

    fn text(&self) -> String {
        match self {
            Anchor::Before(x) => format!("before {x}"),
            Anchor::After(x) => format!("after {x}"),
            Anchor::First => "first".into(),
            Anchor::Last => "last".into(),
        }
    }

    /// `item` placed in `list` (where it may already stand) at this anchor.
    fn place(&self, list: &[String], item: &str) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = list.iter().filter(|x| *x != item).cloned().collect();
        let at = match self {
            Anchor::First => 0,
            Anchor::Last => out.len(),
            Anchor::Before(x) | Anchor::After(x) => {
                if x == item {
                    return Err(format!("{item} cannot be placed beside itself"));
                }
                let i = out
                    .iter()
                    .position(|y| y == x)
                    .ok_or_else(|| format!("{x} is not in the order"))?;
                if matches!(self, Anchor::After(_)) {
                    i + 1
                } else {
                    i
                }
            }
        };
        out.insert(at, item.to_string());
        Ok(out)
    }
}

/// Which list of words an edit amends.
#[derive(Debug, Clone, PartialEq)]
enum WordsAt {
    Value { axis: String, value: String },
    Bucket(String),
}

/// What an operation does, read and checked for shape.
#[derive(Debug, Clone)]
enum What {
    Words {
        add: bool,
        at: WordsAt,
        words: Vec<String>,
        field: Option<String>,
        rule: Option<String>,
    },
    MoveRule {
        set: String,
        rule: String,
        anchor: Anchor,
    },
    MoveSet {
        set: String,
        anchor: Anchor,
    },
    SetPriority {
        axis: String,
        value: String,
        priority: i64,
    },
    AddValue {
        axis: String,
        value: String,
        spec: Map<String, Value>,
        words: Vec<String>,
        anchor: Option<Anchor>,
        bids: Option<Value>,
    },
    AddRule {
        axis: String,
        value: Option<String>,
        when: Vec<Value>,
        set: Option<String>,
        rule: Option<String>,
        anchor: Option<Anchor>,
        confidence: Option<f64>,
        why: Option<String>,
    },
    Silence {
        axis: Option<String>,
        when: Vec<Value>,
    },
    ByModel {
        axis: String,
    },
    MapName {
        axis: String,
        value: String,
        bids: Option<Value>,
        label: Option<String>,
    },
    SetCandidates {
        pick: Option<String>,
        role: String,
        when: Vec<Value>,
        unless: Vec<Value>,
    },
    RemoveBorder {
        pick: Option<String>,
        /// The reason, as a pick raises it, and the key its file writes.
        border: (&'static str, &'static str),
    },
    SetNearTie {
        pick: Option<String>,
        order: Vec<Value>,
    },
    SetRunnerUpWithin {
        pick: Option<String>,
        within: f64,
    },
}

/// One operation of a patch.
#[derive(Debug, Clone)]
pub struct Op {
    /// Its place in the patch, from 1.
    pub at: usize,
    pub kind: String,
    pub scope: Scope,
    pub reason: String,
    /// Where the change came from: a proposal, a review item, a finding, as
    /// the patch wrote it.
    pub evidence: Value,
    /// The operation as written.
    pub written: Value,
    what: What,
}

impl Op {
    /// The operation's name in a refusal: `operation 2 (move_set)`.
    pub fn name(&self) -> String {
        format!("operation {} ({})", self.at, self.kind)
    }

    fn refuse(&self, why: impl Into<String>) -> Error {
        Error::at(self.name(), why)
    }
}

/// A pack patch (record 56 §5.5): typed operations on one pack version.
#[derive(Debug, Clone)]
pub struct Patch {
    /// What it was read from, which a refusal blames.
    pub name: String,
    pub pack: String,
    /// The pack version it was written against; a pack at another version
    /// is refused.
    pub version: Option<String>,
    /// The version a pack-wide patch ships as (B5's rules release).
    pub ships: Option<String>,
    pub operations: Vec<Op>,
    /// Its own cases, judged against the patched pack as an overlay's are.
    pub cases: Vec<(PathBuf, Case)>,
}

/// What a patch document says.
const SAYS: &[&str] = &[
    "patch",
    "pack",
    "version",
    "ships",
    "reason",
    "evidence",
    "operations",
    "cases",
];

impl Patch {
    /// A patch from its text (YAML, or JSON, which is YAML too).
    pub fn parse(name: &str, text: &str) -> R<Patch> {
        let f = File::from_text(name, text)?;
        Patch::of(&f)
    }

    /// A patch from a document a door received.
    pub fn of_value(name: &str, v: &Value) -> R<Patch> {
        let f = File {
            path: PathBuf::from(name),
            source: String::new(),
            value: v.clone(),
        };
        Patch::of(&f)
    }

    fn of(f: &File) -> R<Patch> {
        let m = f.blame(yaml::obj(&f.value, "patch"))?;
        for key in m.keys() {
            if !SAYS.contains(&key.as_str()) {
                return Err(Error::at(
                    key,
                    format!("is not something a patch says; it says {}", SAYS.join(", ")),
                )
                .in_file(&f.path, Some(&f.source)));
            }
        }
        let format = m.get("patch").and_then(Value::as_u64).ok_or_else(|| {
            Error::at(
                "patch",
                format!("a patch states the format it is written in: patch: {FORMAT}"),
            )
            .in_file(&f.path, Some(&f.source))
        })?;
        if format != FORMAT {
            return Err(Error::at(
                "patch",
                format!("this engine reads patch format {FORMAT}, and this one is {format}"),
            )
            .in_file(&f.path, Some(&f.source)));
        }
        let pack = f.blame(yaml::text(yaml::get(m, "pack", "patch")?, "pack"))?;
        let text_of = |k: &str| -> R<Option<String>> {
            m.get(k).map(|v| f.blame(yaml::text(v, k))).transpose()
        };
        let version = text_of("version")?;
        let ships = text_of("ships")?;
        if let Some(s) = &ships {
            f.blame(crate::version::Version::parse(s, "ships"))?;
        }
        let reason = text_of("reason")?;
        let evidence = m.get("evidence").cloned();
        let list = f.blame(yaml::arr(
            yaml::get(m, "operations", "patch")?,
            "operations",
        ))?;
        if list.is_empty() {
            return Err(
                Error::at("operations", "a patch with no operation changes nothing")
                    .in_file(&f.path, Some(&f.source)),
            );
        }
        let mut operations = Vec::with_capacity(list.len());
        for (i, v) in list.iter().enumerate() {
            let op = read_op(i + 1, v, reason.as_deref(), evidence.as_ref())
                .map_err(|e| Error::at(format!("operations[{i}]"), e))
                .map_err(|e| e.in_file(&f.path, None))?;
            operations.push(op);
        }
        let cases = match m.get("cases") {
            Some(v) => crate::corpus::cases_of(f, v)?,
            None => Vec::new(),
        };
        Ok(Patch {
            name: f.path.display().to_string(),
            pack,
            version,
            ships,
            operations,
            cases,
        })
    }

    /// A list of operations a door received for `pack`, as a patch.
    pub fn of_operations(name: &str, pack: &str, operations: &Value) -> R<Patch> {
        Patch::of_value(
            name,
            &json!({"patch": FORMAT, "pack": pack, "operations": operations}),
        )
    }

    /// An overlay document as the operations it is (pack contract 5 read as
    /// record 56 §5.5): each bucket edit an `add_words` and a `remove_words`
    /// on the bucket, each list edit the same on `axis.value`, all scoped to
    /// the overlay's origin, with the overlay's cases.
    pub fn from_overlay(o: &crate::overlay::Overlay) -> Patch {
        let scope = Scope::Scanner(
            o.scope
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let reason = format!("the overlay {}", o.id);
        let evidence = json!({"overlay": o.id});
        let mut operations = Vec::new();
        let mut push = |kind: &str, what: What, written: Value| {
            operations.push(Op {
                at: operations.len() + 1,
                kind: kind.to_string(),
                scope: scope.clone(),
                reason: reason.clone(),
                evidence: evidence.clone(),
                written,
                what,
            });
        };
        let edits: Vec<(WordsAt, &crate::overlay::Edit)> = o
            .buckets
            .iter()
            .map(|(b, e)| (WordsAt::Bucket(b.clone()), e))
            .chain(o.lists.iter().map(|(name, e)| {
                let (axis, value) = name.split_once('.').unwrap_or((name, ""));
                (
                    WordsAt::Value {
                        axis: axis.to_string(),
                        value: value.to_string(),
                    },
                    e,
                )
            }))
            .collect();
        for (at, edit) in edits {
            for (add, words) in [(true, &edit.add), (false, &edit.remove)] {
                if words.is_empty() {
                    continue;
                }
                let kind = if add { "add_words" } else { "remove_words" };
                let mut written = json!({"op": kind, "words": words, "scope": scope.text()});
                match &at {
                    WordsAt::Bucket(b) => written["bucket"] = json!(b),
                    WordsAt::Value { axis, value } => {
                        written["axis"] = json!(axis);
                        written["value"] = json!(value);
                    }
                }
                push(
                    kind,
                    What::Words {
                        add,
                        at: at.clone(),
                        words: words.clone(),
                        field: None,
                        rule: None,
                    },
                    written,
                );
            }
        }
        Patch {
            name: o.id.clone(),
            pack: o.pack.clone(),
            version: None,
            ships: None,
            operations,
            cases: o.cases.clone(),
        }
    }

    /// The distinct scopes of its operations, the pack first.
    pub fn scopes(&self) -> Vec<Scope> {
        let set: BTreeSet<Scope> = self.operations.iter().map(|o| o.scope.clone()).collect();
        set.into_iter().collect()
    }

    /// Whether every operation is a pack edit.
    pub fn is_pack_edit(&self) -> bool {
        self.operations.iter().all(|o| o.scope.is_pack())
    }

    /// The operations as written, each with its scope as text, for an
    /// answer.
    pub fn written(&self) -> Vec<Value> {
        self.operations
            .iter()
            .map(|o| {
                let mut w = o.written.clone();
                w["scope"] = json!(o.scope.text());
                w["reason"] = json!(o.reason);
                w["evidence"] = o.evidence.clone();
                w
            })
            .collect()
    }
}

fn texts_of(v: &Value, at: &str) -> Result<Vec<String>, String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => Ok(s.clone()),
                Value::Number(n) => Ok(n.to_string()),
                _ => Err(format!("{at} holds words")),
            })
            .collect(),
        Value::String(s) => Ok(vec![s.clone()]),
        _ => Err(format!("{at} is a word or a list of words")),
    }
}

fn word(m: &Map<String, Value>, k: &str) -> Result<Option<String>, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(_) => Err(format!("{k} is a word")),
    }
}

fn needs(m: &Map<String, Value>, k: &str) -> Result<String, String> {
    word(m, k)?.ok_or_else(|| format!("needs {k}"))
}

/// One operation, checked for shape: its kind, its keys, and what each
/// says. What it names is checked when it is applied to a pack.
fn read_op(
    at: usize,
    v: &Value,
    reason: Option<&str>,
    evidence: Option<&Value>,
) -> Result<Op, String> {
    let m = v
        .as_object()
        .ok_or_else(|| "an operation is a mapping".to_string())?;
    let kind = word(m, "op")?.ok_or_else(|| {
        format!(
            "an operation names what it does in op: one of {}",
            KINDS.join(", ")
        )
    })?;
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!(
            "{kind} is not an operation; they are {}",
            KINDS.join(", ")
        ));
    }
    let own = keys_of(&kind);
    for k in m.keys() {
        if !COMMON.contains(&k.as_str()) && !own.contains(&k.as_str()) {
            return Err(format!(
                "{kind} does not read {k}; it reads {}",
                COMMON
                    .iter()
                    .chain(own.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let scope = Scope::of(m.get("scope").unwrap_or(&Value::Null))?;
    if PICK_KINDS.contains(&kind.as_str()) && !scope.is_pack() {
        return Err(format!(
            "{kind} changes the main-scan pick, whose file is one for every stack and whose population is the whole registry's: it is a pack edit, scope pack, never an overlay's"
        ));
    }
    let reason = word(m, "reason")?
        .or_else(|| reason.map(str::to_string))
        .ok_or_else(|| "an operation says why: reason".to_string())?;
    let evidence = match m.get("evidence").or(evidence) {
        None | Some(Value::Null) => {
            return Err(
                "an operation names the evidence it came from (a proposal, a review item, a finding): evidence"
                    .into(),
            );
        }
        Some(Value::String(s)) if s.trim().is_empty() => {
            return Err("evidence names nothing".into());
        }
        Some(e) => e.clone(),
    };
    let what = match kind.as_str() {
        "add_words" | "remove_words" => {
            let words = texts_of(
                m.get("words").ok_or_else(|| "needs words".to_string())?,
                "words",
            )?;
            if words.is_empty() || words.iter().any(|w| w.trim().is_empty()) {
                return Err("words names no word, or an empty one".into());
            }
            let at = match (word(m, "bucket")?, word(m, "axis")?, word(m, "value")?) {
                (Some(b), None, None) => WordsAt::Bucket(b),
                (None, Some(axis), Some(value)) => WordsAt::Value { axis, value },
                (None, Some(axis), None) => {
                    // axis.value written as one word, as an overlay lists it
                    match axis.split_once('.') {
                        Some((a, v)) => WordsAt::Value {
                            axis: a.to_string(),
                            value: v.to_string(),
                        },
                        None => return Err("names the value whose words these are: value".into()),
                    }
                }
                _ => {
                    return Err("names whose words these are: axis and value, or a bucket".into());
                }
            };
            What::Words {
                add: kind == "add_words",
                at,
                words,
                field: word(m, "field")?,
                rule: word(m, "rule")?,
            }
        }
        "move_rule" => {
            let rule = needs(m, "rule")?;
            let (set, rule) = match (word(m, "set")?, rule.split_once('/')) {
                (Some(s), None) => (s, rule),
                (None, Some((s, r))) => (s.to_string(), r.to_string()),
                (Some(s), Some((s2, r))) if s == s2 => (s, r.to_string()),
                _ => return Err("names the rule as set/rule, or set and rule".into()),
            };
            let anchor = Anchor::of(m, true)?.expect("required");
            let anchor = match anchor {
                Anchor::Before(x) => Anchor::Before(strip_set(&x, &set)?),
                Anchor::After(x) => Anchor::After(strip_set(&x, &set)?),
                other => other,
            };
            What::MoveRule { set, rule, anchor }
        }
        "move_set" => What::MoveSet {
            set: needs(m, "set")?,
            anchor: Anchor::of(m, true)?.expect("required"),
        },
        "set_priority" => What::SetPriority {
            axis: needs(m, "axis")?,
            value: needs(m, "value")?,
            priority: m
                .get("priority")
                .and_then(Value::as_i64)
                .ok_or_else(|| "priority is a whole number".to_string())?,
        },
        "add_value" => {
            let mut spec = Map::new();
            for k in ["label", "family", "description"] {
                if let Some(w) = word(m, k)? {
                    spec.insert(k.into(), json!(w));
                }
            }
            for k in ["terms", "aliases"] {
                if let Some(v) = m.get(k) {
                    spec.insert(k.into(), json!(texts_of(v, k)?));
                }
            }
            if let Some(g) = word(m, "group")? {
                spec.insert("group".into(), json!(g));
            }
            if let Some(p) = m.get("priority") {
                let p = p
                    .as_i64()
                    .ok_or_else(|| "priority is a whole number".to_string())?;
                if !spec.contains_key("group") {
                    return Err(
                        "a priority ranks a value in its exclusion group: give the group".into(),
                    );
                }
                spec.insert("priority".into(), json!(p));
            }
            let words = match m.get("words") {
                Some(v) => texts_of(v, "words")?,
                None => Vec::new(),
            };
            What::AddValue {
                axis: needs(m, "axis")?,
                value: needs(m, "value")?,
                spec,
                words,
                anchor: Anchor::of(m, false)?,
                bids: m.get("bids").cloned(),
            }
        }
        "add_rule" => {
            let nothing = m.get("nothing").and_then(Value::as_bool).unwrap_or(false);
            let value = word(m, "value")?;
            if nothing == value.is_some() {
                return Err("sets one value, or nothing: true, and not both".into());
            }
            let when = templates(m.get("when"))?;
            What::AddRule {
                axis: needs(m, "axis")?,
                value,
                when,
                set: word(m, "set")?,
                rule: word(m, "rule")?,
                anchor: Anchor::of(m, false)?,
                confidence: match m.get("confidence") {
                    None => None,
                    Some(c) => Some(
                        c.as_f64()
                            .filter(|c| (0.0..=1.0).contains(c))
                            .ok_or_else(|| "confidence is between 0 and 1".to_string())?,
                    ),
                },
                why: word(m, "why")?,
            }
        }
        "silence" => What::Silence {
            axis: word(m, "axis")?,
            when: templates(m.get("when"))?,
        },
        "by_model" => What::ByModel {
            axis: needs(m, "axis")?,
        },
        "map_name" => {
            let bids = m.get("bids").cloned().filter(|b| !b.is_null());
            let label = word(m, "label")?;
            if bids.is_none() && label.is_none() {
                return Err("says how the value is named: bids, label or both".into());
            }
            What::MapName {
                axis: needs(m, "axis")?,
                value: needs(m, "value")?,
                bids,
                label,
            }
        }
        "set_candidates" => {
            let when = conditions_of(m, "when")?;
            let unless = conditions_of(m, "unless")?;
            if when.is_none() && unless.is_none() {
                return Err(
                    "says when or unless: the conditions a stack holding the role meets to compete for it, or both empty to let every stack holding it compete again"
                        .into(),
                );
            }
            What::SetCandidates {
                pick: word(m, "pick")?,
                role: needs(m, "role")?,
                when: when.unwrap_or_default(),
                unless: unless.unwrap_or_default(),
            }
        }
        "remove_border" => {
            let named = needs(m, "border")?;
            if matches!(named.as_str(), "too_close" | "runner_up_within") {
                return Err(
                    "too_close is the margin every pick declares (runner_up_within): set it with set_runner_up_within, or decide near ties with set_near_tie"
                        .into(),
                );
            }
            let border = BORDERS
                .iter()
                .find(|(reason, key)| named == *reason || named == *key)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "{named} is not a border a pick file declares; they are {}",
                        BORDERS
                            .iter()
                            .map(|(r, _)| *r)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            What::RemoveBorder {
                pick: word(m, "pick")?,
                border,
            }
        }
        "set_near_tie" => {
            let order = match m.get("order") {
                Some(Value::Array(a)) => a.clone(),
                _ => {
                    return Err(
                        "order is a list of steps, best first, or an empty list to take the order out"
                            .into(),
                    );
                }
            };
            for step in &order {
                near_tie_step(step)?;
            }
            What::SetNearTie {
                pick: word(m, "pick")?,
                order,
            }
        }
        "set_runner_up_within" => What::SetRunnerUpWithin {
            pick: word(m, "pick")?,
            within: m
                .get("within")
                .and_then(Value::as_f64)
                .filter(|w| (0.0..=1.0).contains(w))
                .ok_or_else(|| {
                    "within is a fraction of the best score from 0 to 1, the number itself included"
                        .to_string()
                })?,
        },
        _ => unreachable!("the kinds are checked above"),
    };
    Ok(Op {
        at,
        kind,
        scope,
        reason,
        evidence,
        written: v.clone(),
        what,
    })
}

fn strip_set(rule: &str, set: &str) -> Result<String, String> {
    match rule.split_once('/') {
        Some((s, r)) if s == set => Ok(r.to_string()),
        Some((s, _)) => Err(format!(
            "a rule moves inside its set: {rule} is of {s}, the rule moved of {set}"
        )),
        None => Ok(rule.to_string()),
    }
}

/// The conditions of an `add_rule` or a `silence`: one template or a list,
/// all of which must hold.
fn templates(v: Option<&Value>) -> Result<Vec<Value>, String> {
    let list = match v {
        None | Some(Value::Null) => return Err("needs when: the conditions it holds on".into()),
        Some(Value::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
    };
    if list.is_empty() {
        return Err("when names no condition".into());
    }
    for t in &list {
        if !t.is_object() {
            return Err(format!(
                "a condition is a mapping: {{words, field}}, {{tag, is | has | gt | ge | lt | le | between}}, {{flag}}, {{axis, is}} or {{not: ...}}, not {t}"
            ));
        }
    }
    Ok(list)
}

/// The conditions of a role's candidacy under `key`: a condition or a list
/// of them, an empty list saying none. None where the key is not given.
fn conditions_of(m: &Map<String, Value>, key: &str) -> Result<Option<Vec<Value>>, String> {
    let list = match m.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a.clone(),
        Some(other) => vec![other.clone()],
    };
    for c in &list {
        if !c.is_object() {
            return Err(format!(
                "{key} holds conditions, each {{axis, is}} or {{tag, is | lt | le | gt | ge}}, not {c}"
            ));
        }
    }
    Ok(Some(list))
}

/// One step of a near tie's order, checked for shape: `{of, prefer:
/// [values]}`, `{of, avoid: [values]}`, `{of, lowest: true}` or `{of,
/// highest: true}`, with `roles` where it orders some roles only.
fn near_tie_step(step: &Value) -> Result<(), String> {
    let shape = "a step is {of, prefer: [values]}, {of, avoid: [values]}, {of, lowest: true} or {of, highest: true}, with roles: [...] where it orders some roles only";
    let m = step.as_object().ok_or(shape)?;
    if word(m, "of")?.is_none() {
        return Err(shape.into());
    }
    let ranks: Vec<&String> = m
        .keys()
        .filter(|k| !matches!(k.as_str(), "of" | "roles"))
        .collect();
    match ranks.as_slice() {
        [k] if matches!(k.as_str(), "prefer" | "avoid") => {
            let values = texts_of(&m[k.as_str()], k)?;
            if values.is_empty() {
                return Err(format!("{k} names no value"));
            }
        }
        [k] if matches!(k.as_str(), "lowest" | "highest") => {
            if m[k.as_str()] != Value::Bool(true) {
                return Err(shape.into());
            }
        }
        _ => return Err(shape.into()),
    }
    if let Some(r) = m.get("roles") {
        texts_of(r, "roles")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The pack's documents, as an operation edits them.

/// One document of the pack: its text as it is on disk (none for a file a
/// patch adds), and its value now.
#[derive(Debug, Clone)]
struct Doc {
    source: Option<String>,
    value: Value,
    changed: bool,
}

/// The documents of one pack directory that a patch reads or writes.
#[derive(Debug, Clone)]
pub struct Docs {
    dir: PathBuf,
    files: BTreeMap<String, Doc>,
}

/// Where a rule set is written.
#[derive(Debug, Clone, PartialEq)]
enum SetAt {
    /// A rule set written longhand, in its own file.
    Longhand(String),
    /// An axis written compactly: its file's values are its rules.
    Axis(String),
}

/// The fingerprint's fields as a person names them, for a word's field.
fn field_name(f: &str) -> String {
    match f.trim() {
        "series_description" | "description" => "text_series_description".into(),
        "protocol_name" | "protocol" => "text_protocol_name".into(),
        "sequence_name" => "text_sequence_name".into(),
        "body_part_examined" => "text_body_part".into(),
        "image_comments" => "text_image_comments".into(),
        "series_comments" => "text_series_comments".into(),
        "te" => "echo_time".into(),
        "tr" => "repetition_time".into(),
        "ti" => "inversion_time".into(),
        "flip" => "flip_angle".into(),
        other => other.to_string(),
    }
}

/// The acquisition's numbers: a condition on one of them is a physics
/// window, and its rule is of the physics tier.
const PHYSICS: &[&str] = &[
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "echo_train_length",
    "magnetic_field_strength",
    "field_strength_normalized",
    "diffusion_b_value",
    "pixel_bandwidth",
];

impl Docs {
    /// The documents of the pack in `dir`, read as an operation needs them.
    pub fn open(dir: &Path) -> R<Docs> {
        let mut d = Docs {
            dir: dir.to_path_buf(),
            files: BTreeMap::new(),
        };
        d.read("pack.yml")?;
        Ok(d)
    }

    fn read(&mut self, rel: &str) -> R<()> {
        if self.files.contains_key(rel) {
            return Ok(());
        }
        let f = File::read(&self.dir.join(rel))?;
        self.files.insert(
            rel.to_string(),
            Doc {
                source: Some(f.source),
                value: f.value,
                changed: false,
            },
        );
        Ok(())
    }

    fn value(&mut self, rel: &str) -> R<&Value> {
        self.read(rel)?;
        Ok(&self.files[rel].value)
    }

    fn set(&mut self, rel: &str, value: Value) {
        match self.files.get_mut(rel) {
            Some(d) => {
                if d.value != value {
                    d.value = value;
                    d.changed = true;
                }
            }
            None => {
                self.files.insert(
                    rel.to_string(),
                    Doc {
                        source: None,
                        value,
                        changed: true,
                    },
                );
            }
        }
    }

    fn manifest(&self) -> &Map<String, Value> {
        self.files["pack.yml"]
            .value
            .as_object()
            .expect("the manifest was read as a mapping")
    }

    /// The files the manifest lists under `key`.
    fn listed(&self, key: &str) -> Vec<String> {
        match self.manifest().get(key) {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    /// The pack's rule set order.
    fn order(&self) -> Vec<String> {
        self.manifest()
            .get("order")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set_manifest(&mut self, key: &str, value: Value) {
        let mut m = self.files["pack.yml"].value.clone();
        m[key] = value;
        self.set("pack.yml", m);
    }

    /// The file an axis is written in.
    fn axis_rel(&mut self, axis: &str) -> R<Option<String>> {
        for rel in self.listed("axes") {
            if self.value(&rel)?.get("axis").and_then(Value::as_str) == Some(axis) {
                return Ok(Some(rel));
            }
        }
        Ok(None)
    }

    fn axis_names(&mut self) -> R<Vec<String>> {
        let mut out = Vec::new();
        for rel in self.listed("axes") {
            if let Some(a) = self.value(&rel)?.get("axis").and_then(Value::as_str) {
                out.push(a.to_string());
            }
        }
        Ok(out)
    }

    /// Where a rule set is written: a longhand file, or an axis file.
    fn set_at(&mut self, set: &str) -> R<Option<SetAt>> {
        for rel in self.listed("rules") {
            if self.value(&rel)?.get("rule_set").and_then(Value::as_str) == Some(set) {
                return Ok(Some(SetAt::Longhand(rel)));
            }
        }
        Ok(self.axis_rel(set)?.map(SetAt::Axis))
    }

    /// The axes each set of the order decides, in the order.
    fn deciders(&mut self) -> R<Vec<(String, Vec<String>)>> {
        let mut out = Vec::new();
        for name in self.order() {
            let axes = match self.set_at(&name)? {
                Some(SetAt::Longhand(rel)) => self
                    .value(&rel)?
                    .get("decides")
                    .map(|d| texts_of(d, "decides").unwrap_or_default())
                    .unwrap_or_default(),
                Some(SetAt::Axis(_)) => vec![name.clone()],
                None => Vec::new(),
            };
            out.push((name, axes));
        }
        Ok(out)
    }

    /// An axis's file and its value, by its identity, its label or an
    /// identity it had: the identity it has now.
    fn value_id(&mut self, op: &Op, axis: &str, value: &str) -> R<(String, String)> {
        let rel = self.axis_rel(axis)?.ok_or_else(|| {
            let names = self.axis_names().unwrap_or_default();
            op.refuse(format!(
                "the pack has no axis named {axis}; it decides {}",
                names.join(", ")
            ))
        })?;
        let values = self.value(&rel)?["values"].clone();
        let Some(vm) = values.as_object() else {
            return Err(op.refuse(format!("{axis} declares no values")));
        };
        if vm.contains_key(value) {
            return Ok((rel, value.to_string()));
        }
        for (id, body) in vm {
            let label = body.get("label").and_then(Value::as_str);
            let aliases = body
                .get("aliases")
                .map(|a| texts_of(a, "aliases").unwrap_or_default())
                .unwrap_or_default();
            if label == Some(value) || aliases.iter().any(|a| a == value) {
                return Ok((rel, id.clone()));
            }
        }
        Err(op.refuse(format!(
            "{axis} has no value named {value}; its values are {}",
            vm.keys().cloned().collect::<Vec<_>>().join(", ")
        )))
    }

    /// The file of the pick named, or of the pack's one pick where none is.
    fn pick_rel(&mut self, op: &Op, name: Option<&str>) -> R<String> {
        let mut picks: Vec<(String, String)> = Vec::new();
        for rel in self.listed("picks") {
            let n = self
                .value(&rel)?
                .get("pick")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            picks.push((rel, n));
        }
        let names = || {
            picks
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match (name, picks.as_slice()) {
            (Some(want), _) => picks
                .iter()
                .find(|(_, n)| n == want)
                .map(|(rel, _)| rel.clone())
                .ok_or_else(|| {
                    op.refuse(format!(
                        "the pack has no pick named {want}; it has {}",
                        names()
                    ))
                }),
            (None, [(rel, _)]) => Ok(rel.clone()),
            (None, []) => Err(op.refuse("the pack declares no pick")),
            (None, _) => Err(op.refuse(format!(
                "the pack declares several picks ({}): name one with pick",
                names()
            ))),
        }
    }

    /// The pack raised to declare contract `at_least`, where it declares
    /// less: a key of a contract is refused in a pack that declares less.
    /// Answers the change, where there was one.
    fn needs_contract(&mut self, at_least: u64, key: &str) -> Option<String> {
        let now = self
            .manifest()
            .get("contract")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if now >= at_least {
            return None;
        }
        self.set_manifest("contract", json!(at_least));
        Some(format!(
            "the pack declares contract {at_least} now, {now} before: {key} is contract {at_least}'s"
        ))
    }

    /// The changed and new documents, as the loader reads them (JSON, which
    /// is YAML), by their path in the pack.
    pub fn sources(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .filter(|(_, d)| d.changed)
            .map(|(rel, d)| {
                (
                    rel.clone(),
                    serde_json::to_string_pretty(&d.value).expect("a value serializes"),
                )
            })
            .collect()
    }

    /// The documents a patch changed or added, by their path in the pack.
    pub fn changed(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(_, d)| d.changed)
            .map(|(rel, _)| rel.clone())
            .collect()
    }

    /// The pack's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write the patched pack to `out`: every file of the pack copied, the
    /// documents a patch changed written over their copies and the ones it
    /// added beside them, the manifest naming `version`. A document is
    /// rewritten where it changed and only there ([`crate::yaml_edit`]),
    /// so what the pack's author wrote around it, comments included, stays;
    /// where that cannot be done the whole document is written again.
    /// Answers each document written, and whether it was rewritten whole.
    pub fn write(&self, out: &Path, version: Option<&str>) -> R<Vec<(String, bool)>> {
        copy_tree(&self.dir, out)
            .map_err(|e| Error::at("out", format!("{}: {e}", out.display())))?;
        let mut files = self.files.clone();
        if let Some(v) = version {
            let d = files.get_mut("pack.yml").expect("the manifest");
            if d.value["version"] != json!(v) {
                d.value["version"] = json!(v);
                d.changed = true;
            }
        }
        let mut written = Vec::new();
        for (rel, d) in files.iter().filter(|(_, d)| d.changed) {
            let (text, whole) = match &d.source {
                Some(src) => {
                    let original: Value = serde_saphyr::from_str(src)
                        .map_err(|e| Error::at(rel, format!("is not YAML: {e}")))?;
                    match crate::yaml_edit::rewrite(src, &original, &d.value) {
                        Some(t) => (t, false),
                        None => (whole_text(&d.value, true)?, true),
                    }
                }
                None => (whole_text(&d.value, false)?, true),
            };
            let path = out.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| Error::at(rel, format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&path, text)
                .map_err(|e| Error::at(rel, format!("{}: {e}", path.display())))?;
            written.push((rel.clone(), whole));
        }
        Ok(written)
    }
}

/// A whole document as YAML, under the licence line.
fn whole_text(v: &Value, rewritten: bool) -> R<String> {
    let body = serde_saphyr::to_string(v)
        .map_err(|e| Error::at("patch", format!("the document will not serialize: {e}")))?;
    let note = if rewritten {
        "# Written again whole by `nils pack apply` (record 56): its comments are in the version before.\n"
    } else {
        "# Added by `nils pack apply` (record 56).\n"
    };
    Ok(format!(
        "# SPDX-License-Identifier: AGPL-3.0-only\n#\n{note}\n{body}"
    ))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let at = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &at)?;
        } else {
            std::fs::copy(e.path(), &at)?;
        }
    }
    Ok(())
}

/// What one operation changed, in words, for an answer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Applied {
    pub at: usize,
    pub op: String,
    pub scope: String,
    /// What it changed, a line each.
    pub changes: Vec<String>,
    /// The pack's documents it wrote, by their path in the pack.
    pub files: Vec<String>,
}

/// A pack with a patch's operations applied, as the pack's loader built it.
pub struct Patched {
    pub pack: Pack,
    pub applied: Vec<Applied>,
    /// The pack's own cases and the patch's, where any does not hold: an
    /// answer, never a refusal.
    pub cases: Option<Error>,
    pub docs: Docs,
}

/// Apply the operations of `patch` that `keep` keeps, in order, to the pack
/// in `dir`, and build the result with the pack's own loader. An operation
/// that cannot apply is refused with why; a result the loader refuses is
/// refused with the loader's why, naming the operations that wrote it.
pub fn apply(dir: &Path, patch: &Patch, keep: &dyn Fn(&Op) -> bool) -> R<Patched> {
    let mut docs = Docs::open(dir)?;
    let name = docs
        .manifest()
        .get("pack")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name != patch.pack {
        return Err(Error::at(
            "pack",
            format!("the patch amends {}, and this pack is {name}", patch.pack),
        ));
    }
    let version = docs
        .manifest()
        .get("version")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    if let Some(want) = &patch.version
        && *want != version
    {
        return Err(Error::at(
            "version",
            format!(
                "the patch was written against {} {want}, and this pack is {version}",
                patch.pack
            ),
        ));
    }
    let mut applied = Vec::new();
    for op in patch.operations.iter().filter(|o| keep(o)) {
        let before: BTreeMap<String, Value> = docs
            .files
            .iter()
            .map(|(k, d)| (k.clone(), d.value.clone()))
            .collect();
        let changes = apply_one(&mut docs, op)?;
        let files: Vec<String> = docs
            .files
            .iter()
            .filter(|(k, d)| d.changed && before.get(*k) != Some(&d.value))
            .map(|(k, _)| k.clone())
            .collect();
        applied.push(Applied {
            at: op.at,
            op: op.kind.clone(),
            scope: op.scope.text(),
            changes,
            files,
        });
    }
    let (pack, cases) =
        crate::pack::load_patched(dir, &docs.sources(), &patch.cases).map_err(|e| {
            let names: Vec<String> = applied
                .iter()
                .map(|a| format!("operation {} ({})", a.at, a.op))
                .collect();
            Error::at(
                "patch",
                format!(
                    "the pack does not load with {}: {e}",
                    if names.is_empty() {
                        "no operation".to_string()
                    } else {
                        names.join(", ")
                    }
                ),
            )
        })?;
    Ok(Patched {
        pack,
        applied,
        cases,
        docs,
    })
}

fn apply_one(docs: &mut Docs, op: &Op) -> R<Vec<String>> {
    match &op.what {
        What::Words {
            add,
            at,
            words,
            field,
            rule,
        } => words_edit(docs, op, *add, at, words, field.as_deref(), rule.as_deref()),
        What::MoveRule { set, rule, anchor } => move_rule(docs, op, set, rule, anchor),
        What::MoveSet { set, anchor } => move_set(docs, op, set, anchor),
        What::SetPriority {
            axis,
            value,
            priority,
        } => set_priority(docs, op, axis, value, *priority),
        What::AddValue {
            axis,
            value,
            spec,
            words,
            anchor,
            bids,
        } => add_value(
            docs,
            op,
            axis,
            value,
            spec,
            words,
            anchor.as_ref(),
            bids.as_ref(),
        ),
        What::AddRule {
            axis,
            value,
            when,
            set,
            rule,
            anchor,
            confidence,
            why,
        } => add_rule(
            docs,
            op,
            AddRule {
                axis,
                value: value.as_deref(),
                when,
                set: set.as_deref(),
                rule: rule.as_deref(),
                anchor: anchor.as_ref(),
                confidence: *confidence,
                why: why.as_deref(),
            },
        ),
        What::Silence { axis, when } => silence(docs, op, axis.as_deref(), when),
        What::ByModel { axis } => by_model(docs, op, axis),
        What::MapName {
            axis,
            value,
            bids,
            label,
        } => map_name(docs, op, axis, value, bids.as_ref(), label.as_deref()),
        What::SetCandidates {
            pick,
            role,
            when,
            unless,
        } => set_candidates(docs, op, pick.as_deref(), role, when, unless),
        What::RemoveBorder { pick, border } => remove_border(docs, op, pick.as_deref(), *border),
        What::SetNearTie { pick, order } => set_near_tie(docs, op, pick.as_deref(), order),
        What::SetRunnerUpWithin { pick, within } => {
            set_runner_up_within(docs, op, pick.as_deref(), *within)
        }
    }
}

// --- add_words and remove_words

/// One list of words an edit reaches.
#[derive(Debug, Clone, PartialEq)]
enum List {
    Bucket(String),
    /// An axis file's value's own list, where it has one, or the list it
    /// will have (a value tried by a flag or a window only).
    Axis {
        rel: String,
        value: String,
        exists: bool,
    },
    /// A longhand rule's keyword clause.
    Rule {
        rel: String,
        set: String,
        rule: String,
        clause: usize,
    },
}

fn list_name(l: &List) -> String {
    match l {
        List::Bucket(b) => format!("the bucket {b}"),
        List::Axis { rel, value, .. } => format!("the words of {value} in {rel}"),
        List::Rule { set, rule, .. } => format!("the words of the rule {set}/{rule}"),
    }
}

/// Whether a rule's `set` names this value of this axis.
fn sets_value(set: &Value, axis: &str, ids: &[&str]) -> bool {
    let Some(v) = set.get(axis) else {
        return false;
    };
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    items.iter().any(|item| match item {
        Value::String(s) => ids.contains(&s.as_str()),
        Value::Object(m) => m
            .get("value")
            .and_then(Value::as_str)
            .is_some_and(|s| ids.contains(&s)),
        _ => false,
    })
}

/// The lists of words that reach `axis=value`: the axis file's own, every
/// longhand rule's keyword clause that sets it, each as a bucket where it
/// is one; only those reading `field` when one is named, and only `rule`'s
/// when one is.
fn lists_of(
    docs: &mut Docs,
    op: &Op,
    axis: &str,
    value: &str,
    field: Option<&str>,
    rule: Option<&str>,
    adding: bool,
) -> R<Vec<(List, String)>> {
    let (rel, id) = docs.value_id(op, axis, value)?;
    let axis_doc = docs.value(&rel)?.clone();
    let body = axis_doc["values"][&id].clone();
    let label = body
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let ids = [id.as_str(), label.as_str()];
    let search = axis_doc
        .get("search")
        .and_then(Value::as_str)
        .unwrap_or("text_all")
        .to_string();
    let mut out: Vec<(List, String)> = Vec::new();
    // the axis file's own list
    let own = body
        .get("keywords")
        .or_else(|| body.get("detection").and_then(|d| d.get("keywords")));
    let rule_named = |set: &str, r: &str| -> bool {
        rule.is_none_or(|want| want == format!("{set}/{r}") || want == r)
    };
    if rule_named(axis, &id) {
        match own {
            Some(Value::Object(m)) if m.contains_key("bucket") => {
                let b = m["bucket"].as_str().unwrap_or_default().to_string();
                out.push((List::Bucket(b), search.clone()));
            }
            Some(_) => out.push((
                List::Axis {
                    rel: rel.clone(),
                    value: id.clone(),
                    exists: true,
                },
                search.clone(),
            )),
            None => {
                let windowed = axis_doc
                    .get("physics")
                    .and_then(Value::as_array)
                    .is_some_and(|ws| {
                        ws.iter()
                            .any(|w| w.get("value").and_then(Value::as_str) == Some(&id))
                    });
                let tried = body.get("detection").is_some()
                    || body.get("alternative_flags").is_some()
                    || windowed;
                if tried && adding {
                    out.push((
                        List::Axis {
                            rel: rel.clone(),
                            value: id.clone(),
                            exists: false,
                        },
                        search.clone(),
                    ));
                }
            }
        }
    }
    // every longhand rule that sets it
    for rel in docs.listed("rules") {
        let set = docs.value(&rel)?.clone();
        let Some(name) = set.get("rule_set").and_then(Value::as_str) else {
            continue;
        };
        let Some(rules) = set.get("rules").and_then(Value::as_object) else {
            continue;
        };
        for (rid, r) in rules {
            if !rule_named(name, rid) || !sets_value(&r["set"], axis, &ids) {
                continue;
            }
            for (ci, c) in r["clauses"].as_array().into_iter().flatten().enumerate() {
                let Some(kw) = c.get("keywords") else {
                    continue;
                };
                let f = c
                    .get("field")
                    .and_then(Value::as_str)
                    .unwrap_or("search_text")
                    .to_string();
                match kw {
                    Value::Object(m) if m.contains_key("bucket") => out.push((
                        List::Bucket(m["bucket"].as_str().unwrap_or_default().to_string()),
                        f,
                    )),
                    _ => out.push((
                        List::Rule {
                            rel: rel.clone(),
                            set: name.to_string(),
                            rule: rid.clone(),
                            clause: ci,
                        },
                        f,
                    )),
                }
            }
        }
    }
    let mut seen = Vec::new();
    out.retain(|x| {
        let keep = !seen.contains(&x.0);
        seen.push(x.0.clone());
        keep
    });
    if let Some(want) = field {
        let want = field_name(want);
        let read: BTreeSet<String> = out.iter().map(|(_, f)| f.clone()).collect();
        out.retain(|(_, f)| *f == want);
        if out.is_empty() {
            return Err(op.refuse(if read.is_empty() {
                format!("no word list reaches {axis}={value}")
            } else {
                format!(
                    "no word list of {axis}={value} reads {want}; its words are read from {}. Add a rule that reads {want} with add_rule",
                    read.into_iter().collect::<Vec<_>>().join(", ")
                )
            }));
        }
    }
    if out.is_empty() {
        return Err(op.refuse(format!(
            "no word of the pack's reaches {axis}={value}{}: a route sets it, or it is the default. Reach it by words with add_rule",
            rule.map(|r| format!(" through {r}")).unwrap_or_default()
        )));
    }
    Ok(out)
}

fn current_words(docs: &mut Docs, l: &List) -> R<Vec<String>> {
    Ok(match l {
        List::Bucket(b) => docs
            .manifest()
            .get("buckets")
            .and_then(|m| m.get(b))
            .map(|v| texts_of(v, b).unwrap_or_default())
            .unwrap_or_default(),
        List::Axis { rel, value, exists } => {
            if !exists {
                return Ok(Vec::new());
            }
            let body = docs.value(rel)?["values"][value].clone();
            body.get("keywords")
                .or_else(|| body.get("detection").and_then(|d| d.get("keywords")))
                .map(|v| texts_of(v, "keywords").unwrap_or_default())
                .unwrap_or_default()
        }
        List::Rule {
            rel, rule, clause, ..
        } => {
            let v = docs.value(rel)?["rules"][rule]["clauses"][*clause]["keywords"].clone();
            texts_of(&v, "keywords").unwrap_or_default()
        }
    })
}

fn set_words(docs: &mut Docs, l: &List, words: Vec<String>) -> R<()> {
    match l {
        List::Bucket(b) => {
            let mut buckets = docs
                .manifest()
                .get("buckets")
                .cloned()
                .unwrap_or_else(|| json!({}));
            buckets[b] = json!(words);
            docs.set_manifest("buckets", buckets);
        }
        List::Axis { rel, value, .. } => {
            let mut v = docs.value(rel)?.clone();
            let body = &mut v["values"][value];
            if body.get("keywords").is_none()
                && let Some(d) = body.get_mut("detection")
                && d.get("keywords").is_some()
            {
                d["keywords"] = json!(words);
            } else {
                body["keywords"] = json!(words);
            }
            docs.set(rel, v);
        }
        List::Rule {
            rel, rule, clause, ..
        } => {
            let mut v = docs.value(rel)?.clone();
            v["rules"][rule]["clauses"][*clause]["keywords"] = json!(words);
            docs.set(rel, v);
        }
    }
    Ok(())
}

fn words_edit(
    docs: &mut Docs,
    op: &Op,
    add: bool,
    at: &WordsAt,
    words: &[String],
    field: Option<&str>,
    rule: Option<&str>,
) -> R<Vec<String>> {
    let lists: Vec<(List, String)> = match at {
        WordsAt::Bucket(b) => {
            let open: Vec<String> = docs
                .manifest()
                .get("buckets")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            if !open.contains(b) {
                return Err(op.refuse(format!(
                    "the pack opens no bucket named {b}; it opens {}",
                    if open.is_empty() {
                        "none".to_string()
                    } else {
                        open.join(", ")
                    }
                )));
            }
            if field.is_some() || rule.is_some() {
                return Err(op.refuse(
                    "a bucket is read wherever the pack names it: a field or a rule narrows a value's words, not a bucket's",
                ));
            }
            vec![(List::Bucket(b.clone()), String::new())]
        }
        WordsAt::Value { axis, value } => lists_of(docs, op, axis, value, field, rule, add)?,
    };
    let edit = crate::overlay::Edit {
        add: if add { words.to_vec() } else { Vec::new() },
        remove: if add { Vec::new() } else { words.to_vec() },
    };
    let mut changes = Vec::new();
    for (l, _) in &lists {
        let had = current_words(docs, l)?;
        let now = crate::overlay::merge(&had, &edit);
        if now == had {
            continue;
        }
        let moved: Vec<String> = if add {
            now.iter().filter(|w| !had.contains(w)).cloned().collect()
        } else {
            had.iter().filter(|w| !now.contains(w)).cloned().collect()
        };
        set_words(docs, l, now)?;
        changes.push(format!(
            "{} {} {}",
            if add { "added" } else { "removed" },
            moved
                .iter()
                .map(|w| format!("{w:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            if add {
                format!("to {}", list_name(l))
            } else {
                format!("from {}", list_name(l))
            },
        ));
    }
    if changes.is_empty() {
        let lists: Vec<String> = lists.iter().map(|(l, _)| list_name(l)).collect();
        return Err(op.refuse(if add {
            format!(
                "{} already {} {}, so this changes nothing",
                lists.join(" and "),
                if lists.len() == 1 { "holds" } else { "hold" },
                words.join(", ")
            )
        } else {
            format!(
                "{} {} none of {}, so this changes nothing",
                lists.join(" and "),
                if lists.len() == 1 { "holds" } else { "hold" },
                words.join(", ")
            )
        }));
    }
    Ok(changes)
}

// --- moves

fn order_of(v: &Value) -> Vec<String> {
    v.get("order")
        .map(|o| texts_of(o, "order").unwrap_or_default())
        .unwrap_or_default()
}

fn move_rule(docs: &mut Docs, op: &Op, set: &str, rule: &str, anchor: &Anchor) -> R<Vec<String>> {
    let at = docs.set_at(set)?.ok_or_else(|| {
        op.refuse(format!(
            "the pack has no rule set named {set}; its sets are {}",
            docs.order().join(", ")
        ))
    })?;
    let rel = match &at {
        SetAt::Longhand(r) | SetAt::Axis(r) => r.clone(),
    };
    let doc = docs.value(&rel)?.clone();
    let order = order_of(&doc);
    let known = match &at {
        SetAt::Longhand(_) => doc["rules"]
            .as_object()
            .map(|m| m.contains_key(rule))
            .unwrap_or(false),
        SetAt::Axis(_) => doc["values"]
            .as_object()
            .map(|m| m.contains_key(rule))
            .unwrap_or(false),
    };
    if !known {
        return Err(op.refuse(format!(
            "{set} has no rule named {rule}; its rules are {}",
            order.join(", ")
        )));
    }
    if let SetAt::Axis(_) = at
        && order.is_empty()
    {
        return Err(op.refuse(format!(
            "{set} states no order: its values are vocabulary that a longhand set decides"
        )));
    }
    let now = anchor.place(&order, rule).map_err(|e| {
        op.refuse(match &at {
            SetAt::Axis(_) => format!(
                "{e}; the values of {set} outside its order are tried after it, in the order they are written"
            ),
            SetAt::Longhand(_) => format!("{e} of {set}"),
        })
    })?;
    if now == order {
        return Err(op.refuse(format!(
            "{set}/{rule} already stands {}, so this changes nothing",
            anchor.text()
        )));
    }
    let moved = passed(&order, &now, rule);
    let mut v = doc;
    v["order"] = json!(now);
    docs.set(&rel, v);
    Ok(vec![format!(
        "moved {set}/{rule} {} in {set}{moved}",
        anchor.text()
    )])
}

fn move_set(docs: &mut Docs, op: &Op, set: &str, anchor: &Anchor) -> R<Vec<String>> {
    let order = docs.order();
    if order.is_empty() {
        return Err(op.refuse(
            "the pack states no order, so its sets run as declared; give the pack an order first",
        ));
    }
    if !order.iter().any(|s| s == set) {
        return Err(op.refuse(format!(
            "the pack has no rule set named {set}; its order is {}",
            order.join(", ")
        )));
    }
    let now = anchor
        .place(&order, set)
        .map_err(|e| op.refuse(format!("{e} of the pack")))?;
    if now == order {
        return Err(op.refuse(format!(
            "{set} already runs {}, so this changes nothing",
            anchor.text()
        )));
    }
    docs.set_manifest("order", json!(now));
    Ok(vec![format!(
        "moved the rule set {set} {} in the pack's order{}",
        anchor.text(),
        passed(&order, &now, set)
    )])
}

/// Which items `item` passed in a move from `was` to `now`, in words.
fn passed(was: &[String], now: &[String], item: &str) -> String {
    let at = |l: &[String], x: &str| l.iter().position(|y| y == x);
    let (Some(a), Some(b)) = (at(was, item), at(now, item)) else {
        return String::new();
    };
    let mut after_now = Vec::new();
    let mut before_now = Vec::new();
    for x in was.iter().filter(|x| *x != item) {
        let (Some(xa), Some(xb)) = (at(was, x), at(now, x)) else {
            continue;
        };
        if xa < a && xb > b {
            before_now.push(x.clone());
        } else if xa > a && xb < b {
            after_now.push(x.clone());
        }
    }
    let mut out = String::new();
    if !after_now.is_empty() {
        out.push_str(&format!(": it now runs after {}", after_now.join(", ")));
    }
    if !before_now.is_empty() {
        out.push_str(&format!(
            "{} before {}",
            if out.is_empty() {
                ": it now runs"
            } else {
                ", and"
            },
            before_now.join(", ")
        ));
    }
    out
}

// --- set_priority

fn set_priority(
    docs: &mut Docs,
    op: &Op,
    axis: &str,
    value: &str,
    priority: i64,
) -> R<Vec<String>> {
    let (rel, id) = docs.value_id(op, axis, value)?;
    let mut v = docs.value(&rel)?.clone();
    let body = &mut v["values"][&id];
    let Some(group) = body
        .get("group")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Err(op.refuse(format!(
            "{axis}={id} is in no exclusion group, so a priority ranks it above nothing"
        )));
    };
    let had = body.get("priority").and_then(Value::as_i64);
    if had == Some(priority) {
        return Err(op.refuse(format!(
            "{axis}={id} already has priority {priority} in {group}, so this changes nothing"
        )));
    }
    body["priority"] = json!(priority);
    docs.set(&rel, v);
    Ok(vec![format!(
        "{axis}={id}: priority {} to {priority} in the group {group}",
        had.map(|p| p.to_string()).unwrap_or_else(|| "none".into())
    )])
}

// --- add_value

#[allow(clippy::too_many_arguments)]
fn add_value(
    docs: &mut Docs,
    op: &Op,
    axis: &str,
    value: &str,
    spec: &Map<String, Value>,
    words: &[String],
    anchor: Option<&Anchor>,
    bids: Option<&Value>,
) -> R<Vec<String>> {
    let rel = docs.axis_rel(axis)?.ok_or_else(|| {
        let names = docs.axis_names().unwrap_or_default();
        op.refuse(format!(
            "the pack has no axis named {axis}; it decides {}",
            names.join(", ")
        ))
    })?;
    let mut v = docs.value(&rel)?.clone();
    let Some(values) = v["values"].as_object().cloned() else {
        return Err(op.refuse(format!("{axis} declares no values")));
    };
    let label = spec
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or(value)
        .to_string();
    let aliases: Vec<String> = spec
        .get("aliases")
        .map(|a| texts_of(a, "aliases").unwrap_or_default())
        .unwrap_or_default();
    for (id, body) in &values {
        let their_label = body.get("label").and_then(Value::as_str).unwrap_or(id);
        let theirs: Vec<String> = body
            .get("aliases")
            .map(|a| texts_of(a, "aliases").unwrap_or_default())
            .unwrap_or_default();
        for mine in std::iter::once(value)
            .chain(std::iter::once(label.as_str()))
            .chain(aliases.iter().map(String::as_str))
        {
            if mine == id || mine == their_label || theirs.iter().any(|t| t == mine) {
                return Err(op.refuse(format!("{mine} already names the value {id} of {axis}")));
            }
        }
    }
    let mut body = Value::Object(spec.clone());
    let mut changes = vec![format!("added the value {value} to {axis}")];
    if !words.is_empty() {
        let order = order_of(&v);
        if order.is_empty() {
            return Err(op.refuse(format!(
                "{axis} is decided by longhand rules, so its own file tries no value by words: add the value without words, then reach it with add_rule"
            )));
        }
        body["keywords"] = json!(words);
        let anchor = anchor.cloned().unwrap_or(Anchor::Last);
        let now = anchor
            .place(&order, value)
            .map_err(|e| op.refuse(format!("{e} of {axis}")))?;
        v["order"] = json!(now);
        changes.push(format!(
            "{axis} tries {value} by {} word(s), {} in its order",
            words.len(),
            anchor.text()
        ));
    } else if anchor.is_some() {
        return Err(op.refuse(
            "a value without words is vocabulary a rule sets, and takes no place in the order",
        ));
    }
    v["values"][value] = body;
    docs.set(&rel, v);
    if let Some(b) = bids {
        changes.extend(bids_name(docs, op, axis, value, b)?);
    }
    Ok(changes)
}

// --- conditions

/// The expression a template is, what it cites, and the tier it reads at.
struct Cond {
    expr: Value,
    cite: String,
    tier: &'static str,
    source: &'static str,
}

fn number(v: &Value) -> Option<Value> {
    match v {
        Value::Number(_) => Some(v.clone()),
        Value::String(s) => s.trim().parse::<f64>().ok().map(|n| json!(n)),
        _ => None,
    }
}

/// One template of `when`: `{tag, is | has | gt | ge | lt | le | eq | ne |
/// between}`, `{flag}`, `{axis, is}`, or `{not: template}`. Words are read
/// apart ([`split_words`]).
fn condition(t: &Value) -> Result<Cond, String> {
    let m = t.as_object().expect("templates are mappings");
    if let Some(inner) = m.get("not") {
        if m.len() != 1 {
            return Err("not wraps one condition and says nothing else".into());
        }
        if inner.get("words").is_some() {
            return Err("not cannot wrap words; give the words a rule of their own".into());
        }
        let c = condition(inner)?;
        return Ok(Cond {
            expr: json!({"not": c.expr}),
            cite: format!("not {}", c.cite),
            tier: c.tier,
            source: c.source,
        });
    }
    if let Some(flag) = m.get("flag") {
        let name = flag.as_str().ok_or("flag is a flag's name")?;
        if m.len() != 1 {
            return Err("a flag condition names the flag and nothing else".into());
        }
        return Ok(Cond {
            expr: json!(name),
            cite: name.to_string(),
            tier: "exclusive",
            source: "flags",
        });
    }
    if let Some(axis) = m.get("axis") {
        let axis = axis.as_str().ok_or("axis is an axis's name")?;
        let is = m
            .get("is")
            .and_then(Value::as_str)
            .ok_or("an axis condition says which value: is")?;
        if m.len() != 2 {
            return Err("an axis condition is {axis, is}".into());
        }
        return Ok(Cond {
            expr: json!({"axis": axis, "is": is}),
            cite: format!("{axis} is {is}"),
            tier: "stated",
            source: "axes",
        });
    }
    if let Some(tag) = m.get("tag").or_else(|| m.get("field")) {
        let tag = field_name(tag.as_str().ok_or("tag is a field's name")?);
        let physics = PHYSICS.contains(&tag.as_str());
        let mut parts: Vec<Value> = Vec::new();
        let mut said: Vec<String> = Vec::new();
        for (k, v) in m {
            match k.as_str() {
                "tag" | "field" => {}
                "is" => {
                    let s = match v {
                        Value::String(s) => s.trim().to_lowercase(),
                        Value::Number(n) => n.to_string(),
                        _ => return Err("is is a word".into()),
                    };
                    parts.push(json!({"text": tag, "equals": s}));
                    said.push(format!("{tag} is {s}"));
                }
                "has" => {
                    let s = v.as_str().ok_or("has is a word")?.to_lowercase();
                    parts.push(json!({"text": tag, "substring": s}));
                    said.push(format!("{tag} has {s}"));
                }
                "between" => {
                    let range = v.as_array().filter(|a| a.len() == 2).ok_or(
                        "between is two numbers, the lowest and the highest, both included",
                    )?;
                    let lo = number(&range[0]).ok_or("between is two numbers")?;
                    let hi = number(&range[1]).ok_or("between is two numbers")?;
                    parts.push(json!({"field": tag, "ge": lo, "le": hi}));
                    said.push(format!("{tag} in {lo}..{hi}"));
                }
                "gt" | "ge" | "lt" | "le" | "eq" | "ne" => {
                    let n = number(v).ok_or_else(|| format!("{k} is a number"))?;
                    parts.push(json!({"field": tag, (k.as_str()): n}));
                    let op = match k.as_str() {
                        "gt" => ">",
                        "ge" => ">=",
                        "lt" => "<",
                        "le" => "<=",
                        "eq" => "=",
                        _ => "!=",
                    };
                    said.push(format!("{tag} {op} {n}"));
                }
                "present" => {
                    let p = v.as_bool().ok_or("present is true or false")?;
                    parts.push(json!({"field": tag, "present": p}));
                    said.push(format!("{tag} {}", if p { "present" } else { "absent" }));
                }
                other => {
                    return Err(format!(
                        "a tag condition compares with is, has, gt, ge, lt, le, eq, ne, between or present, not {other}"
                    ));
                }
            }
        }
        if parts.is_empty() {
            return Err(format!("the condition on {tag} compares it with nothing"));
        }
        let expr = if parts.len() == 1 {
            parts.pop().expect("one")
        } else {
            json!({"all": parts})
        };
        return Ok(Cond {
            expr,
            cite: said.join(" and "),
            tier: if physics { "physics" } else { "exclusive" },
            source: if physics { "physics" } else { "header" },
        });
    }
    Err(format!(
        "a condition is {{words, field}}, {{tag, ...}}, {{flag}}, {{axis, is}} or {{not: ...}}, not {t}"
    ))
}

/// A rule's words and the field they are read in, where it reads words.
type Words = Option<(Vec<String>, Option<String>)>;

/// The words of a template list, and the rest.
fn split_words(when: &[Value]) -> Result<(Words, Vec<Value>), String> {
    let mut words = None;
    let mut rest = Vec::new();
    for t in when {
        if let Some(w) = t.get("words") {
            if words.is_some() {
                return Err("a rule reads one list of words; give the others a rule each".into());
            }
            let m = t.as_object().expect("a mapping");
            for k in m.keys() {
                if k != "words" && k != "field" {
                    return Err(format!("a words condition is {{words, field}}, not {k}"));
                }
            }
            let list = texts_of(w, "words")?;
            if list.is_empty() {
                return Err("words names no word".into());
            }
            let field = m.get("field").and_then(Value::as_str).map(field_name);
            words = Some((list, field));
        } else {
            rest.push(t.clone());
        }
    }
    Ok((words, rest))
}

/// The axes an expression reads, by name.
fn axes_in(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(a)) = m.get("axis") {
                out.insert(a.clone());
            }
            for x in m.values() {
                axes_in(x, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                axes_in(x, out);
            }
        }
        _ => {}
    }
}

fn axis_doc_of(docs: &mut Docs, op: &Op, axis: &str) -> R<(String, Value)> {
    let rel = docs.axis_rel(axis)?.ok_or_else(|| {
        let names = docs.axis_names().unwrap_or_default();
        op.refuse(format!(
            "the pack has no axis named {axis}; it decides {}",
            names.join(", ")
        ))
    })?;
    let v = docs.value(&rel)?.clone();
    Ok((rel, v))
}

fn multi(axis_doc: &Value) -> bool {
    axis_doc.get("kind").and_then(Value::as_str) == Some("multi")
}

fn disposition_phase(axis_doc: &Value) -> bool {
    axis_doc.get("phase").and_then(Value::as_str) == Some("disposition")
}

/// The id a new rule takes: the one asked for, or `prefix` and the next
/// number free in `taken`.
fn rule_id(
    asked: Option<&str>,
    prefix: &str,
    taken: &Map<String, Value>,
) -> Result<String, String> {
    if let Some(a) = asked {
        if taken.contains_key(a) {
            return Err(format!("the set already has a rule named {a}"));
        }
        return Ok(a.to_string());
    }
    let mut n = 1;
    loop {
        let id = format!("{prefix}_{n}");
        if !taken.contains_key(&id) {
            return Ok(id);
        }
        n += 1;
    }
}

// --- add_rule

struct AddRule<'a> {
    axis: &'a str,
    value: Option<&'a str>,
    when: &'a [Value],
    set: Option<&'a str>,
    rule: Option<&'a str>,
    anchor: Option<&'a Anchor>,
    confidence: Option<f64>,
    why: Option<&'a str>,
}

fn add_rule(docs: &mut Docs, op: &Op, a: AddRule<'_>) -> R<Vec<String>> {
    let (_, axis_doc) = axis_doc_of(docs, op, a.axis)?;
    let set_value = match a.value {
        Some(v) => {
            let (_, id) = docs.value_id(op, a.axis, v)?;
            json!(id)
        }
        None => Value::Null,
    };
    // Nothing, of an axis that holds several values, is that the stack holds
    // none of them where the conditions hold: the rule runs first in the
    // first set that decides the axis, so it closes the axis before any rule
    // sets one of its values (a stack whose body part is a spine's holds no
    // role).
    let none_of_several = a.value.is_none() && multi(&axis_doc);
    if none_of_several && a.anchor.is_some_and(|x| *x != Anchor::First) {
        return Err(op.refuse(format!(
            "a rule that says nothing of {}, which holds several values, runs first in its set, before every rule that sets one of them: leave its place out or say position: first",
            a.axis
        )));
    }
    let (words, rest) = split_words(a.when).map_err(|e| op.refuse(e))?;
    let conds: Vec<Cond> = rest
        .iter()
        .map(condition)
        .collect::<Result<_, _>>()
        .map_err(|e| op.refuse(e))?;
    let mut body = Map::new();
    let mut said = Vec::new();
    match words {
        Some((list, field)) => {
            let field = field.unwrap_or_else(|| {
                axis_doc
                    .get("search")
                    .and_then(Value::as_str)
                    .unwrap_or("search_text")
                    .to_string()
            });
            if !conds.is_empty() {
                let parts: Vec<Value> = conds.iter().map(|c| c.expr.clone()).collect();
                body.insert(
                    "requires".into(),
                    if parts.len() == 1 {
                        parts[0].clone()
                    } else {
                        json!({"all": parts})
                    },
                );
            }
            said.push(format!("the words {} in {field}", list.join(", ")));
            said.extend(conds.iter().map(|c| c.cite.clone()));
            body.insert(
                "clauses".into(),
                json!([{"keywords": list, "field": field, "tier": "keywords"}]),
            );
        }
        None => {
            if conds.is_empty() {
                return Err(op.refuse("when names no condition"));
            }
            let tier = if conds.iter().any(|c| c.tier == "physics") {
                "physics"
            } else if conds.iter().any(|c| c.tier == "exclusive") {
                "exclusive"
            } else {
                "stated"
            };
            let source = conds
                .iter()
                .find(|c| c.tier == tier)
                .map(|c| c.source)
                .unwrap_or("axes");
            let parts: Vec<Value> = conds.iter().map(|c| c.expr.clone()).collect();
            let cite: Vec<String> = conds.iter().map(|c| c.cite.clone()).collect();
            said.extend(cite.clone());
            body.insert(
                "clauses".into(),
                json!([{
                    "when": if parts.len() == 1 { parts[0].clone() } else { json!({"all": parts}) },
                    "cite": cite.join(" and "),
                    "source": source,
                    "tier": tier,
                }]),
            );
        }
    }
    let mut set_map = Map::new();
    set_map.insert(a.axis.to_string(), set_value.clone());
    body.insert("set".into(), Value::Object(set_map));
    if let Some(c) = a.confidence {
        body.insert("confidence".into(), json!(c));
    }
    body.insert(
        "why".into(),
        json!(
            a.why
                .map(str::to_string)
                .unwrap_or_else(|| op.reason.clone())
        ),
    );

    // Where it goes: the set named, or the axis's own longhand set, or a set
    // of the rules added to this axis, run just before what decides it.
    let named = match a.set {
        Some(s) => Some(s.to_string()),
        None => match docs.set_at(a.axis)? {
            Some(SetAt::Longhand(_)) => Some(a.axis.to_string()),
            _ => None,
        },
    };
    let what = format!("{}={}", a.axis, set_value.as_str().unwrap_or("nothing"));
    match named {
        Some(set) => {
            let rel = match docs.set_at(&set)? {
                Some(SetAt::Longhand(rel)) => rel,
                Some(SetAt::Axis(_)) => {
                    return Err(op.refuse(format!(
                        "{set} is an axis written compactly, one rule per value; leave set out and the rule goes in a set of its own before it, or add words to a value with add_words"
                    )));
                }
                None => {
                    return Err(op.refuse(format!(
                        "the pack has no rule set named {set}; its sets are {}",
                        docs.order().join(", ")
                    )));
                }
            };
            let mut doc = docs.value(&rel)?.clone();
            let decides = doc
                .get("decides")
                .map(|d| texts_of(d, "decides").unwrap_or_default())
                .unwrap_or_default();
            if !decides.iter().any(|d| d == a.axis) {
                return Err(op.refuse(format!(
                    "the set {set} does not decide {}; it decides {}",
                    a.axis,
                    decides.join(", ")
                )));
            }
            if none_of_several {
                let first = docs
                    .deciders()?
                    .into_iter()
                    .find(|(_, axes)| axes.iter().any(|x| x == a.axis))
                    .map(|(name, _)| name);
                if first.as_deref() != Some(set.as_str()) {
                    return Err(op.refuse(format!(
                        "{set} is not the first set that decides {}, which holds several values: a rule that says nothing of it goes first in {}, before every rule that sets one of them",
                        a.axis,
                        first.unwrap_or_default()
                    )));
                }
            }
            let rules = doc["rules"].as_object().cloned().unwrap_or_default();
            let id = rule_id(a.rule, "added", &rules).map_err(|e| op.refuse(e))?;
            let order = order_of(&doc);
            let anchor = match a.anchor {
                Some(x) => x.clone(),
                None if order.is_empty() || none_of_several => Anchor::First,
                None => {
                    return Err(op.refuse(format!(
                        "say where in {set} it goes (before:, after: or position: first | last): the first rule of a set that fires decides"
                    )));
                }
            };
            let anchor = match anchor {
                Anchor::Before(x) => Anchor::Before(strip_set(&x, &set).map_err(|e| op.refuse(e))?),
                Anchor::After(x) => Anchor::After(strip_set(&x, &set).map_err(|e| op.refuse(e))?),
                other => other,
            };
            let now = anchor
                .place(&order, &id)
                .map_err(|e| op.refuse(format!("{e} of {set}")))?;
            doc["order"] = json!(now);
            doc["rules"][&id] = Value::Object(body);
            docs.set(&rel, doc);
            Ok(vec![format!(
                "added the rule {set}/{id}, {}: {what} where {}",
                anchor.text(),
                said.join(" and ")
            )])
        }
        None => {
            let set = format!("{}_added", a.axis);
            let rel = format!("rules/{set}.yml");
            if let Some(x) = a.anchor
                && !matches!(x, Anchor::First | Anchor::Last)
            {
                return Err(op.refuse(format!(
                    "the rule goes in {set}, a set of the rules added to {} that runs just before what decides it; a place in another set is named with set:",
                    a.axis
                )));
            }
            let existing = docs.set_at(&set)?;
            let mut doc = match &existing {
                Some(SetAt::Longhand(r)) => docs.value(r)?.clone(),
                Some(SetAt::Axis(_)) => {
                    return Err(op.refuse(format!("{set} names an axis of the pack")));
                }
                None => {
                    let mut d = json!({
                        "rule_set": set,
                        "decides": [a.axis],
                        "order": [],
                        "rules": {},
                    });
                    if multi(&axis_doc) {
                        d["collect"] = json!(true);
                    }
                    d
                }
            };
            let rules = doc["rules"].as_object().cloned().unwrap_or_default();
            let id = rule_id(a.rule, "added", &rules).map_err(|e| op.refuse(e))?;
            let order = order_of(&doc);
            let anchor = a.anchor.cloned().unwrap_or(if none_of_several {
                Anchor::First
            } else {
                Anchor::Last
            });
            doc["order"] = json!(anchor.place(&order, &id).map_err(|e| op.refuse(e))?);
            doc["rules"][&id] = Value::Object(body);
            let rel = match existing {
                Some(SetAt::Longhand(r)) => r,
                _ => {
                    // the set is new: the manifest names its file, and the
                    // order runs it just before what decides the axis
                    let deciders = docs.deciders()?;
                    let before = deciders
                        .iter()
                        .find(|(name, _)| name == a.axis)
                        .or_else(|| {
                            deciders.iter().find(|(name, axes)| {
                                axes.iter().any(|x| x == a.axis) && {
                                    let routed = match docs.set_at(name) {
                                        Ok(Some(SetAt::Longhand(r))) => docs
                                            .value(&r)
                                            .map(|v| v.get("enter_when").is_some())
                                            .unwrap_or(true),
                                        _ => true,
                                    };
                                    !routed
                                }
                            })
                        })
                        .map(|(name, _)| name.clone())
                        .ok_or_else(|| {
                            op.refuse(format!(
                                "nothing in the pack decides {} outside a route; name the set the rule goes in with set:",
                                a.axis
                            ))
                        })?;
                    let mut rules_list = docs.listed("rules");
                    rules_list.push(rel.clone());
                    docs.set_manifest("rules", json!(rules_list));
                    let order = Anchor::Before(before.clone())
                        .place(&docs.order(), &set)
                        .map_err(|e| op.refuse(e))?;
                    docs.set_manifest("order", json!(order));
                    rel
                }
            };
            docs.set(&rel, doc);
            Ok(vec![format!(
                "added the rule {set}/{id}: {what} where {}",
                said.join(" and ")
            )])
        }
    }
}

// --- silence

fn silence(docs: &mut Docs, op: &Op, axis: Option<&str>, when: &[Value]) -> R<Vec<String>> {
    let (words, rest) = split_words(when).map_err(|e| op.refuse(e))?;
    if words.is_some() {
        return Err(op.refuse(
            "a silence holds on what is decided or measured, not on words; reach the stacks by words with add_rule and nothing: true",
        ));
    }
    let conds: Vec<Cond> = rest
        .iter()
        .map(condition)
        .collect::<Result<_, _>>()
        .map_err(|e| op.refuse(e))?;
    let parts: Vec<Value> = conds.iter().map(|c| c.expr.clone()).collect();
    let expr = if parts.len() == 1 {
        parts[0].clone()
    } else {
        json!({"all": parts})
    };
    let cite: Vec<String> = conds.iter().map(|c| c.cite.clone()).collect();
    let Some(axis) = axis else {
        // the stack is nobody's question at all, as an excluded one is
        let mut manifest = Value::Object(docs.manifest().clone());
        let review = manifest.get("review").cloned().unwrap_or_else(|| json!({}));
        let had = review.get("silent_when").cloned();
        if let Some(h) = &had {
            let already = h == &expr
                || h.get("any")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.contains(&expr));
            if already {
                return Err(op.refuse(format!(
                    "nobody is asked about a stack where {} already, so this changes nothing",
                    cite.join(" and ")
                )));
            }
        }
        let now = match had {
            None => expr,
            Some(Value::Object(m)) if m.len() == 1 && m.contains_key("any") => {
                let mut list = m["any"].as_array().cloned().unwrap_or_default();
                list.push(expr);
                json!({"any": list})
            }
            Some(h) => json!({"any": [h, expr]}),
        };
        manifest["review"]["silent_when"] = now;
        docs.set("pack.yml", manifest);
        return Ok(vec![format!(
            "nobody is asked about a stack where {}",
            cite.join(" and ")
        )]);
    };
    let (_, axis_doc) = axis_doc_of(docs, op, axis)?;
    if multi(&axis_doc) {
        return Err(op.refuse(format!(
            "{axis} holds several values; an empty one already says none and is never asked, so there is nothing to silence"
        )));
    }
    if disposition_phase(&axis_doc) {
        return Err(op.refuse(format!(
            "{axis} is decided after the passes from what the stack is; silence what it is decided from"
        )));
    }
    // the condition reads what is decided before it: the set runs after
    // every set that decides the axis or an axis the condition reads
    let mut read = BTreeSet::new();
    axes_in(&expr, &mut read);
    let deciders = docs.deciders()?;
    let set = format!("silence_{axis}");
    let rel = format!("rules/{set}.yml");
    let existing = docs.set_at(&set)?;
    let mut doc = match &existing {
        Some(SetAt::Longhand(r)) => docs.value(r)?.clone(),
        Some(SetAt::Axis(_)) => return Err(op.refuse(format!("{set} names an axis of the pack"))),
        None => json!({
            "rule_set": set,
            "decides": [axis],
            "redecides": [axis],
            "order": [],
            "rules": {},
        }),
    };
    let rules = doc["rules"].as_object().cloned().unwrap_or_default();
    if rules.values().any(|r| r["clauses"][0]["when"] == expr) {
        return Err(op.refuse(format!(
            "{axis} is already silenced where {}, so this changes nothing",
            cite.join(" and ")
        )));
    }
    let id = rule_id(None, "silenced", &rules).map_err(|e| op.refuse(e))?;
    let order = order_of(&doc);
    doc["order"] = json!(Anchor::Last.place(&order, &id).map_err(|e| op.refuse(e))?);
    doc["rules"][&id] = json!({
        "clauses": [{"when": expr, "cite": format!("silenced where {}", cite.join(" and ")), "source": "silence", "tier": "stated"}],
        "set": {(axis): null},
        "why": op.reason,
    });
    let after = deciders
        .iter()
        .rposition(|(name, axes)| {
            name != &set && axes.iter().any(|x| x == axis || read.contains(x))
        })
        .map(|i| deciders[i].0.clone());
    let Some(after) = after else {
        return Err(op.refuse(format!("nothing in the pack decides {axis}")));
    };
    let order = docs.order();
    let at_now = order.iter().position(|s| *s == set);
    let needed = order
        .iter()
        .position(|s| *s == after)
        .expect("in the order");
    match existing {
        Some(SetAt::Longhand(r)) => {
            docs.set(&r, doc);
            if at_now.is_some_and(|i| i < needed) {
                let now = Anchor::After(after.clone())
                    .place(&order, &set)
                    .map_err(|e| op.refuse(e))?;
                docs.set_manifest("order", json!(now));
            }
        }
        _ => {
            let mut rules_list = docs.listed("rules");
            rules_list.push(rel.clone());
            docs.set_manifest("rules", json!(rules_list));
            let now = Anchor::After(after.clone())
                .place(&order, &set)
                .map_err(|e| op.refuse(e))?;
            docs.set_manifest("order", json!(now));
            docs.set(&rel, doc);
        }
    }
    let mut changes = vec![format!(
        "{axis} is decided as nothing where {}, by {set} after {after}",
        cite.join(" and ")
    )];
    // and no pass fills what was silenced: each pass that writes the axis
    // leaves those stacks out of its target
    for prel in docs.listed("passes") {
        let mut p = docs.value(&prel)?.clone();
        let writes: Vec<String> = match p.get("kind").and_then(Value::as_str) {
            Some("session_context") => p["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|r| {
                    r["set"]
                        .as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect(),
            _ => p["decide"]
                .get("writes")
                .map(|w| texts_of(w, "writes").unwrap_or_default())
                .unwrap_or_default(),
        };
        if !writes.iter().any(|w| w == axis) {
            continue;
        }
        let guard = json!({"not": expr});
        let target = p.get("target").and_then(|t| t.get("when")).cloned();
        p["target"]["when"] = match target {
            None => guard,
            Some(Value::Object(m)) if m.len() == 1 && m.contains_key("all") => {
                let mut list = m["all"].as_array().cloned().unwrap_or_default();
                list.push(guard);
                json!({"all": list})
            }
            Some(t) => json!({"all": [t, guard]}),
        };
        let name = p
            .get("pass")
            .and_then(Value::as_str)
            .unwrap_or(&prel)
            .to_string();
        docs.set(&prel, p);
        // a pass writes its axes together, so a stack it leaves out has none
        // of them filled by it: said, since it moves more than the axis
        let others: Vec<&String> = writes.iter().filter(|w| *w != axis).collect();
        changes.push(format!(
            "the pass {name} leaves the stacks where {} out of its target{}",
            cite.join(" and "),
            if others.is_empty() {
                String::new()
            } else {
                format!(
                    ", so it fills neither {axis} nor {} there",
                    others
                        .iter()
                        .map(|o| o.as_str())
                        .collect::<Vec<_>>()
                        .join(" nor ")
                )
            }
        ));
    }
    Ok(changes)
}

// --- by_model

fn by_model(docs: &mut Docs, op: &Op, axis: &str) -> R<Vec<String>> {
    if docs.axis_rel(axis)?.is_none() {
        let names = docs.axis_names()?;
        return Err(op.refuse(format!(
            "the pack has no axis named {axis}; it decides {}",
            names.join(", ")
        )));
    }
    let mut manifest = Value::Object(docs.manifest().clone());
    let mut list: Vec<String> = manifest
        .get("review")
        .and_then(|r| r.get("by_model"))
        .map(|v| texts_of(v, "by_model").unwrap_or_default())
        .unwrap_or_default();
    if list.iter().any(|a| a == axis) {
        return Err(op.refuse(format!(
            "{axis} is already its image model's, so this changes nothing"
        )));
    }
    list.push(axis.to_string());
    if manifest.get("review").is_none() {
        manifest["review"] = json!({});
    }
    manifest["review"]["by_model"] = json!(list);
    docs.set("pack.yml", manifest);
    Ok(vec![format!(
        "{axis} is its model's: sorting asks nothing about it, and what the rules state of it stays"
    )])
}

// --- map_name

fn map_name(
    docs: &mut Docs,
    op: &Op,
    axis: &str,
    value: &str,
    bids: Option<&Value>,
    label: Option<&str>,
) -> R<Vec<String>> {
    let (rel, id) = docs.value_id(op, axis, value)?;
    let mut changes = Vec::new();
    if let Some(l) = label {
        let mut v = docs.value(&rel)?.clone();
        let had = v["values"][&id]
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string();
        if had == l {
            return Err(op.refuse(format!(
                "{axis}={id} is already named {l}, so this changes nothing"
            )));
        }
        if let Some(m) = v["values"].as_object() {
            for (other, body) in m {
                if *other == id {
                    continue;
                }
                let their = body.get("label").and_then(Value::as_str).unwrap_or(other);
                if their == l || other == l {
                    return Err(op.refuse(format!("{l} already names the value {other} of {axis}")));
                }
            }
        }
        v["values"][&id]["label"] = json!(l);
        docs.set(&rel, v);
        let stores_label = docs.value(&rel)?.get("stores").and_then(Value::as_str) == Some("label");
        changes.push(format!(
            "{axis}={id} is named {l}, not {had}, in the descriptive name{}",
            if stores_label {
                ", and stored so: every stack that holds it moves"
            } else {
                ""
            }
        ));
    }
    if let Some(b) = bids {
        changes.extend(bids_name(docs, op, axis, &id, b)?);
    }
    Ok(changes)
}

// --- the main-scan pick (the 2026-10-10 study of the pick borders)

/// The roles a pick file picks for.
fn pick_roles(doc: &Value) -> Vec<String> {
    doc.get("roles")
        .map(|r| texts_of(r, "roles").unwrap_or_default())
        .unwrap_or_default()
}

fn pick_name(doc: &Value) -> String {
    doc.get("pick")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string()
}

/// A name a pick may read: an axis of the pack or a field of the
/// fingerprint, as a person names it.
fn pick_reads(docs: &mut Docs, op: &Op, name: &str) -> R<String> {
    let name = field_name(name);
    if docs.axis_rel(&name)?.is_some() || crate::stack::field_index(&name).is_some() {
        return Ok(name);
    }
    Err(op.refuse(format!(
        "{name} is neither an axis of the pack nor a field of the fingerprint"
    )))
}

/// The values of a name as a pick file writes them: an axis's by their
/// identity, whatever the patch named them by; a field's as written.
fn pick_values(docs: &mut Docs, op: &Op, of: &str, values: &[String]) -> R<Vec<String>> {
    if docs.axis_rel(of)?.is_none() {
        return Ok(values.to_vec());
    }
    values
        .iter()
        .map(|v| docs.value_id(op, of, v).map(|(_, id)| id))
        .collect()
}

/// A role's candidacy conditions, from the patch's templates to the pick
/// file's own: `{axis, is}` (a value or a list of them) is `{of, any}`, and
/// `{tag, is | lt | le | gt | ge}` is `{of, any}` or one `{of, lt ...}` per
/// comparison. Words and flags are the rules': a stack is reached by them
/// with a rule that says nothing of the role.
fn pick_conditions(docs: &mut Docs, op: &Op, list: &[Value]) -> R<Vec<Value>> {
    let mut out = Vec::new();
    for c in list {
        let m = c.as_object().expect("conditions are mappings");
        let not_here = || {
            op.refuse(format!(
                "a pick's candidacy reads what a pick reads: {{axis, is}} or {{tag, is | lt | le | gt | ge}}, not {c}; words and flags are the rules', and add_rule with nothing: true reaches a stack by them"
            ))
        };
        if let Some(axis) = m.get("axis").and_then(Value::as_str) {
            if m.len() != 2 || !m.contains_key("is") {
                return Err(not_here());
            }
            let values = texts_of(&m["is"], "is").map_err(|e| op.refuse(e))?;
            if docs.axis_rel(axis)?.is_none() {
                let names = docs.axis_names()?;
                return Err(op.refuse(format!(
                    "the pack has no axis named {axis}; it decides {}",
                    names.join(", ")
                )));
            }
            out.push(json!({"of": axis, "any": pick_values(docs, op, axis, &values)?}));
            continue;
        }
        let Some(tag) = m
            .get("tag")
            .or_else(|| m.get("field"))
            .and_then(Value::as_str)
        else {
            return Err(not_here());
        };
        let of = pick_reads(docs, op, tag)?;
        let mut said = false;
        for (k, v) in m {
            match k.as_str() {
                "tag" | "field" => {}
                "is" => {
                    let values = texts_of(v, "is").map_err(|e| op.refuse(e))?;
                    out.push(json!({"of": of, "any": pick_values(docs, op, &of, &values)?}));
                    said = true;
                }
                "lt" | "le" | "gt" | "ge" => {
                    let n = number(v).ok_or_else(|| op.refuse(format!("{k} is a number")))?;
                    out.push(json!({"of": of, (k.as_str()): n}));
                    said = true;
                }
                _ => return Err(not_here()),
            }
        }
        if !said {
            return Err(not_here());
        }
    }
    Ok(out)
}

fn said_of(conditions: &[Value]) -> String {
    conditions
        .iter()
        .map(|c| {
            let of = c["of"].as_str().unwrap_or_default();
            match c.get("any") {
                Some(any) => format!(
                    "{of} is {}",
                    texts_of(any, "any").unwrap_or_default().join(" or ")
                ),
                None => {
                    let (k, n) = c
                        .as_object()
                        .and_then(|m| m.iter().find(|(k, _)| k.as_str() != "of"))
                        .map(|(k, n)| (k.as_str(), n.to_string()))
                        .unwrap_or(("", String::new()));
                    let op = match k {
                        "lt" => "<",
                        "le" => "<=",
                        "gt" => ">",
                        _ => ">=",
                    };
                    format!("{of} {op} {n}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

fn set_candidates(
    docs: &mut Docs,
    op: &Op,
    pick: Option<&str>,
    role: &str,
    when: &[Value],
    unless: &[Value],
) -> R<Vec<String>> {
    let rel = docs.pick_rel(op, pick)?;
    let mut doc = docs.value(&rel)?.clone();
    let roles = pick_roles(&doc);
    if !roles.iter().any(|r| r == role) {
        return Err(op.refuse(format!(
            "{role} is not a role the pick {} picks for; it picks {}",
            pick_name(&doc),
            roles.join(", ")
        )));
    }
    let when = pick_conditions(docs, op, when)?;
    let unless = pick_conditions(docs, op, unless)?;
    let mut changes = Vec::new();
    if when.is_empty() && unless.is_empty() {
        let held = doc
            .get_mut("candidates")
            .and_then(Value::as_object_mut)
            .and_then(|c| c.remove(role));
        if held.is_none() {
            return Err(op.refuse(format!(
                "every stack holding {role} competes for it already, so this changes nothing"
            )));
        }
        if doc["candidates"].as_object().is_some_and(Map::is_empty) {
            doc.as_object_mut()
                .expect("a pick file is a mapping")
                .remove("candidates");
        }
        changes.push(format!(
            "{role}: every stack that holds it competes for it again"
        ));
    } else {
        let mut entry = Map::new();
        if !when.is_empty() {
            entry.insert("when".into(), json!(when));
        }
        if !unless.is_empty() {
            entry.insert("unless".into(), json!(unless));
        }
        let entry = Value::Object(entry);
        if doc.get("candidates").and_then(|c| c.get(role)) == Some(&entry) {
            return Err(op.refuse(format!(
                "the candidacy of {role} says this already, so this changes nothing"
            )));
        }
        if !doc.get("candidates").is_some_and(Value::is_object) {
            doc["candidates"] = json!({});
        }
        doc["candidates"][role] = entry;
        let mut said = Vec::new();
        if !when.is_empty() {
            said.push(format!("where {}", said_of(&when)));
        }
        if !unless.is_empty() {
            said.push(format!("unless {}", said_of(&unless)));
        }
        changes.push(format!(
            "{role}: a stack that holds it competes for it, and is of its population, only {}",
            said.join(", and ")
        ));
        changes.extend(docs.needs_contract(9, "candidates"));
    }
    docs.set(&rel, doc);
    Ok(changes)
}

fn remove_border(
    docs: &mut Docs,
    op: &Op,
    pick: Option<&str>,
    (reason, key): (&str, &str),
) -> R<Vec<String>> {
    let rel = docs.pick_rel(op, pick)?;
    let mut doc = docs.value(&rel)?.clone();
    let declared: Vec<String> = doc
        .get("borders")
        .and_then(Value::as_object)
        .map(|b| b.keys().cloned().collect())
        .unwrap_or_default();
    if !declared.iter().any(|k| k == key) {
        return Err(op.refuse(format!(
            "the pick {} raises no {reason}: its borders are {}",
            pick_name(&doc),
            declared.join(", ")
        )));
    }
    doc["borders"]
        .as_object_mut()
        .expect("borders is a mapping")
        .remove(key);
    docs.set(&rel, doc);
    Ok(vec![format!(
        "the pick raises {reason} no more ({key} out of its borders)"
    )])
}

fn set_near_tie(docs: &mut Docs, op: &Op, pick: Option<&str>, order: &[Value]) -> R<Vec<String>> {
    let rel = docs.pick_rel(op, pick)?;
    let mut doc = docs.value(&rel)?.clone();
    let roles = pick_roles(&doc);
    if order.is_empty() {
        if doc.get("near_tie").is_none() {
            return Err(
                op.refuse("the pick has no near-tie order, so taking it out changes nothing")
            );
        }
        doc.as_object_mut()
            .expect("a pick file is a mapping")
            .remove("near_tie");
        docs.set(&rel, doc);
        return Ok(vec![
            "the pick has no near-tie order: a near tie is a border again".into(),
        ]);
    }
    let mut steps = Vec::new();
    let mut said = Vec::new();
    for step in order {
        let m = step.as_object().expect("checked when read");
        let of = pick_reads(docs, op, m["of"].as_str().expect("checked when read"))?;
        let mut out = Map::new();
        out.insert("of".into(), json!(of));
        let mut line = String::new();
        for (k, v) in m {
            match k.as_str() {
                "prefer" | "avoid" => {
                    let values = texts_of(v, k).map_err(|e| op.refuse(e))?;
                    let values = pick_values(docs, op, &of, &values)?;
                    line = format!("{of} {k} {}", values.join(", "));
                    out.insert(k.clone(), json!(values));
                }
                "lowest" | "highest" => {
                    line = format!("{of} {k} first");
                    out.insert(k.clone(), json!(true));
                }
                _ => {}
            }
        }
        if let Some(r) = m.get("roles") {
            let named = texts_of(r, "roles").map_err(|e| op.refuse(e))?;
            for n in &named {
                if !roles.iter().any(|x| x == n) {
                    return Err(op.refuse(format!(
                        "{n} is not a role the pick {} picks for; it picks {}",
                        pick_name(&doc),
                        roles.join(", ")
                    )));
                }
            }
            line.push_str(&format!(" (for {})", named.join(", ")));
            out.insert("roles".into(), json!(named));
        }
        said.push(line);
        steps.push(Value::Object(out));
    }
    let steps = Value::Array(steps);
    if doc.get("near_tie") == Some(&steps) {
        return Err(op.refuse("the pick's near-tie order is this already, so this changes nothing"));
    }
    doc["near_tie"] = steps;
    docs.set(&rel, doc);
    let mut changes = vec![format!(
        "a near tie is decided by {}; one no step decides is a border still",
        said.join(", then ")
    )];
    changes.extend(docs.needs_contract(9, "near_tie"));
    Ok(changes)
}

fn set_runner_up_within(
    docs: &mut Docs,
    op: &Op,
    pick: Option<&str>,
    within: f64,
) -> R<Vec<String>> {
    let rel = docs.pick_rel(op, pick)?;
    let mut doc = docs.value(&rel)?.clone();
    let now = doc
        .get("borders")
        .and_then(|b| b.get("runner_up_within"))
        .and_then(Value::as_f64);
    if now.is_some_and(|n| crate::pack::at_threshold(n, within)) {
        return Err(op.refuse(format!(
            "runner_up_within is {within} already, so this changes nothing"
        )));
    }
    if !doc.get("borders").is_some_and(Value::is_object) {
        doc["borders"] = json!({});
    }
    doc["borders"]["runner_up_within"] = json!(within);
    docs.set(&rel, doc);
    Ok(vec![format!(
        "a near tie is a runner-up within {within} of the best now{}",
        now.map(|n| format!(", {n} before")).unwrap_or_default()
    )])
}

/// The BIDS datatypes a suffix may sit in.
const DATATYPES: &[&str] = &["anat", "dwi", "func", "fmap", "perf"];

/// Where the BIDS mapping names `axis=id`: (path in the mapping, the token).
fn bids_places(bids: &Value, axis: &str, id: &str) -> Vec<(Vec<Value>, String)> {
    let mut out = Vec::new();
    if let Some(s) = bids["suffix"][axis].get(id) {
        out.push((
            vec![json!("suffix"), json!(axis), json!(id), json!("suffix")],
            s["suffix"].as_str().unwrap_or_default().to_string(),
        ));
    }
    for (entity, from) in [("part", "construct"), ("mtransfer", "modifier")] {
        if from == axis
            && let Some(t) = bids["entities"][entity].get(id)
        {
            out.push((
                vec![json!("entities"), json!(entity), json!(id)],
                t.as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    if let Some(t) = bids["entities"]["reconstruction"][axis].get(id) {
        out.push((
            vec![
                json!("entities"),
                json!("reconstruction"),
                json!(axis),
                json!(id),
            ],
            t.as_str().unwrap_or_default().to_string(),
        ));
    }
    for (i, g) in bids["acq"].as_array().into_iter().flatten().enumerate() {
        if g["from"].as_str() == Some(axis)
            && let Some(t) = g["tokens"].get(id)
        {
            out.push((
                vec![json!("acq"), json!(i), json!("tokens"), json!(id)],
                t.as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    out
}

fn place_name(path: &[Value]) -> String {
    match path.first().and_then(Value::as_str) {
        Some("suffix") => "the suffix".into(),
        Some("acq") => "the acq- token".into(),
        Some("entities") => format!(
            "the {} entity",
            path.get(1).and_then(Value::as_str).unwrap_or("")
        ),
        _ => "the mapping".into(),
    }
}

fn set_at_path(v: &mut Value, path: &[Value], to: Value) {
    let mut cur = v;
    for (i, p) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        let next = match p {
            Value::String(k) => {
                if !cur.is_object() {
                    *cur = json!({});
                }
                cur.as_object_mut()
                    .expect("a mapping")
                    .entry(k.clone())
                    .or_insert(if last { Value::Null } else { json!({}) })
            }
            Value::Number(n) => {
                let i = n.as_u64().unwrap_or(0) as usize;
                &mut cur[i]
            }
            _ => return,
        };
        cur = next;
    }
    *cur = to;
}

fn bids_label(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')
}

fn bids_name(docs: &mut Docs, op: &Op, axis: &str, id: &str, b: &Value) -> R<Vec<String>> {
    let Some(rel) = docs.listed("bids").first().cloned() else {
        return Err(op.refuse("the pack has no BIDS mapping to name a value in"));
    };
    let mut bids = docs.value(&rel)?.clone();
    let places = bids_places(&bids, axis, id);
    let (path, token, had): (Vec<Value>, String, Option<String>) = match b {
        Value::String(t) => match places.as_slice() {
            [] => {
                return Err(op.refuse(format!(
                    "{axis}={id} has no BIDS name yet; say where it goes: {{suffix, datatype}}, {{acq}} or {{entity, token}}"
                )));
            }
            [(path, had)] => (path.clone(), t.clone(), Some(had.clone())),
            many => {
                return Err(op.refuse(format!(
                    "{axis}={id} is named in {}; say which: {{suffix}}, {{acq}} or {{entity}}",
                    many.iter()
                        .map(|(p, _)| place_name(p))
                        .collect::<Vec<_>>()
                        .join(" and ")
                )));
            }
        },
        Value::Object(m) => {
            if let Some(s) = m.get("suffix").and_then(Value::as_str) {
                if !["construct", "technique", "modifier", "base"].contains(&axis) {
                    return Err(op.refuse(format!(
                        "a BIDS suffix comes from the construct, the technique, the modifier or the base, not {axis}"
                    )));
                }
                let current = bids["suffix"][axis].get(id).cloned();
                let datatype = m
                    .get("datatype")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        current
                            .as_ref()
                            .and_then(|c| c["datatype"].as_str().map(str::to_string))
                    })
                    .ok_or_else(|| {
                        op.refuse(format!("{axis}={id} has no suffix yet: give its datatype"))
                    })?;
                if !DATATYPES.contains(&datatype.as_str()) {
                    return Err(op.refuse(format!(
                        "{datatype} is not a BIDS datatype a suffix sits in: {}",
                        DATATYPES.join(", ")
                    )));
                }
                if !bids_label(s) {
                    return Err(
                        op.refuse(format!("{s} is not a BIDS label: those are [0-9a-zA-Z+]+"))
                    );
                }
                let had = current.as_ref().map(|c| {
                    format!(
                        "{}/{}",
                        c["datatype"].as_str().unwrap_or_default(),
                        c["suffix"].as_str().unwrap_or_default()
                    )
                });
                let mut entry = current.unwrap_or_else(|| json!({}));
                if entry["suffix"] == json!(s) && entry["datatype"] == json!(datatype) {
                    return Err(op.refuse(format!(
                        "{axis}={id} is already the {datatype} suffix {s}, so this changes nothing"
                    )));
                }
                entry["suffix"] = json!(s);
                entry["datatype"] = json!(datatype);
                if bids.get("suffix").is_none() {
                    bids["suffix"] = json!({});
                }
                if bids["suffix"].get(axis).is_none() {
                    bids["suffix"][axis] = json!({});
                }
                bids["suffix"][axis][id] = entry;
                docs.set(&rel, bids);
                return Ok(vec![format!(
                    "{axis}={id} takes the BIDS suffix {datatype}/{s}{}",
                    had.map(|h| format!(", not {h}")).unwrap_or_default()
                )]);
            }
            if let Some(t) = m.get("acq").and_then(Value::as_str) {
                let i = bids["acq"]
                    .as_array()
                    .and_then(|a| a.iter().position(|g| g["from"].as_str() == Some(axis)))
                    .ok_or_else(|| {
                        op.refuse(format!("no acq- group of the BIDS mapping reads {axis}"))
                    })?;
                let had = bids["acq"][i]["tokens"]
                    .get(id)
                    .and_then(Value::as_str)
                    .map(str::to_string);
                (
                    vec![json!("acq"), json!(i), json!("tokens"), json!(id)],
                    t.to_string(),
                    had,
                )
            } else if let Some(e) = m.get("entity").and_then(Value::as_str) {
                let t = m
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or_else(|| op.refuse("an entity's name is its token"))?;
                let path = match (e, axis) {
                    ("part", "construct") => vec![json!("entities"), json!("part"), json!(id)],
                    ("mt" | "mtransfer", "modifier") => {
                        vec![json!("entities"), json!("mtransfer"), json!(id)]
                    }
                    ("rec" | "reconstruction", "provenance" | "construct") => vec![
                        json!("entities"),
                        json!("reconstruction"),
                        json!(axis),
                        json!(id),
                    ],
                    _ => {
                        return Err(op.refuse(format!(
                            "the {e} entity is not named from {axis}: part from the construct, mt from the modifier, rec from the provenance or the construct"
                        )));
                    }
                };
                let had = places
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, h)| h.clone());
                (path, t.to_string(), had)
            } else {
                return Err(op.refuse(
                    "bids names a suffix {suffix, datatype}, an acq- token {acq} or an entity {entity, token}",
                ));
            }
        }
        _ => {
            return Err(
                op.refuse("bids is the new name, or {suffix, datatype}, {acq} or {entity, token}")
            );
        }
    };
    if !bids_label(&token) {
        return Err(op.refuse(format!(
            "{token} is not a BIDS label: those are [0-9a-zA-Z+]+"
        )));
    }
    if had.as_deref() == Some(token.as_str()) {
        return Err(op.refuse(format!(
            "{axis}={id} is already {token} in {}, so this changes nothing",
            place_name(&path)
        )));
    }
    let name = place_name(&path);
    set_at_path(&mut bids, &path, json!(token));
    docs.set(&rel, bids);
    Ok(vec![format!(
        "{axis}={id} is {token} in {name}{}",
        had.map(|h| format!(", not {h}")).unwrap_or_default()
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_reads_from_its_text_and_its_mapping_alike() {
        for (text, want) in [
            ("pack", Scope::Pack),
            ("site:partner", Scope::Site("partner".into())),
            ("dataset:a, b", Scope::Dataset(vec!["a".into(), "b".into()])),
            (
                "scanner:station=MR1,model=X",
                Scope::Scanner(vec![
                    ("model".into(), "X".into()),
                    ("station".into(), "MR1".into()),
                ]),
            ),
            (
                "batch:7",
                Scope::Scanner(vec![("batch".into(), "7".into())]),
            ),
        ] {
            assert_eq!(Scope::parse(text).unwrap(), want, "{text}");
            assert_eq!(Scope::parse(&want.text()).unwrap(), want);
        }
        assert_eq!(
            Scope::of(&json!({"scanner": {"station": "MR1"}})).unwrap(),
            Scope::parse("scanner:station=MR1").unwrap()
        );
        assert_eq!(Scope::of(&json!({"batch": 3})).unwrap().text(), "batch:3");
        for bad in ["dataset:", "scanner:room=1", "batch:x", "cohort:a"] {
            assert!(Scope::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_anchor_places_and_refuses_beside_itself() {
        let list: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            Anchor::Before("a".into()).place(&list, "c").unwrap(),
            ["c", "a", "b"]
        );
        assert_eq!(
            Anchor::After("c".into()).place(&list, "a").unwrap(),
            ["b", "c", "a"]
        );
        assert_eq!(
            Anchor::Last.place(&list, "x").unwrap(),
            ["a", "b", "c", "x"]
        );
        assert!(Anchor::Before("b".into()).place(&list, "b").is_err());
        assert!(Anchor::Before("z".into()).place(&list, "a").is_err());
    }

    #[test]
    fn an_operation_says_why_and_where_from_and_reads_only_its_keys() {
        let e = Patch::parse(
            "p",
            "patch: 1\npack: mri\noperations:\n  - {op: by_model, axis: post_contrast}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("reason"), "{e}");
        let e = Patch::parse(
            "p",
            "patch: 1\npack: mri\noperations:\n  - {op: by_model, axis: post_contrast, reason: r}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("evidence"), "{e}");
        let e = Patch::parse(
            "p",
            "patch: 1\npack: mri\nreason: r\nevidence: e\noperations:\n  - {op: by_model, axis: x, words: [a]}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("does not read words"), "{e}");
        let e = Patch::parse(
            "p",
            "patch: 1\npack: mri\nreason: r\nevidence: e\noperations:\n  - {op: rename, axis: x}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("not an operation"), "{e}");
        let e = Patch::parse("p", "patch: 2\npack: mri\noperations: []\n")
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("format 1"), "{e}");
        let ok = Patch::parse(
            "p",
            "patch: 1\npack: mri\nreason: r\nevidence: {proposal: 4}\noperations:\n  - {op: by_model, axis: post_contrast, scope: 'dataset:a'}\n",
        )
        .unwrap();
        assert_eq!(ok.operations[0].scope, Scope::Dataset(vec!["a".into()]));
        assert_eq!(ok.operations[0].evidence, json!({"proposal": 4}));
    }

    #[test]
    fn a_template_becomes_the_expression_the_loader_reads() {
        let c = condition(&json!({"tag": "ti", "between": [1800, 2800]})).unwrap();
        assert_eq!(c.tier, "physics");
        assert_eq!(
            c.expr,
            json!({"field": "inversion_time", "ge": 1800, "le": 2800})
        );
        let c = condition(&json!({"tag": "manufacturer", "is": "GE MEDICAL"})).unwrap();
        assert_eq!(c.tier, "exclusive");
        assert_eq!(
            c.expr,
            json!({"text": "manufacturer", "equals": "ge medical"})
        );
        let c = condition(&json!({"not": {"axis": "directory_type", "is": "localizer"}})).unwrap();
        assert_eq!(
            c.expr,
            json!({"not": {"axis": "directory_type", "is": "localizer"}})
        );
        assert!(condition(&json!({"tag": "te", "near": 3})).is_err());
    }
}
