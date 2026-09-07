// SPDX-License-Identifier: AGPL-3.0-only

//! The move catalog (§10, D36): `options` offers a set's typed moves with
//! stable small integer ids, templates with holes and legal fillers, and
//! `apply` edits the document by move id. The catalog is bounded and
//! published (thirty kinds); a question needing a move outside it is
//! composed by `draft`, never by growing the catalog. A move costs zero
//! queries: options reads the document and the catalog, never the store.
//! The error rule is two rules: never enumerate a catalog value, field or
//! kind the principal may not see, and always list the fillers the
//! author's own document defines.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use blake2::digest::consts::U16;
use blake2::{Blake2b, Digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ast::{
    Arg, Ask, Attach, Clause, Dir, Grain, Has, IntSpec, Level, Measure, Near, Order, Pick, Policy,
    Set, Src, Tie, Ties, Unit, Window, WindowSpec,
};
use crate::describe::{clause_text, set_sentence};
use crate::validate::{Class, Issue, Names, Scope, Validated};

/// The published cap on move kinds.
pub const MOVE_KINDS_CAP: usize = 30;

/// The window presets, by name and days.
pub const PRESETS: &[(&str, i64)] = &[
    ("30 days", 30),
    ("3 months", 93),
    ("6 months", 186),
    ("1 year", 366),
    ("2 years", 732),
    ("5 years", 1830),
];

/// Every kind of move, the whole catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AddWhere,
    AddAxisWhere,
    RemoveWhere,
    SetStrict,
    SetWindow,
    SetPolicy,
    SetOptional,
    AddNear,
    AddAttach,
    AddHas,
    SetBound,
    RemoveRelation,
    AddBind,
    RemoveBind,
    AddPick,
    RemovePick,
    AddSet,
    RemoveSet,
    RenameSet,
    SetOut,
    AddColumn,
    RemoveColumn,
    AddOrder,
    SetLimit,
    SetParam,
    SetLevel,
    ExcludeScenario,
    UpdateSelection,
    KeepSet,
    AddMeasure,
}

/// The catalog, published in this order.
pub const CATALOG: &[Kind] = &[
    Kind::AddWhere,
    Kind::AddAxisWhere,
    Kind::RemoveWhere,
    Kind::SetStrict,
    Kind::SetWindow,
    Kind::SetPolicy,
    Kind::SetOptional,
    Kind::AddNear,
    Kind::AddAttach,
    Kind::AddHas,
    Kind::SetBound,
    Kind::RemoveRelation,
    Kind::AddBind,
    Kind::RemoveBind,
    Kind::AddPick,
    Kind::RemovePick,
    Kind::AddSet,
    Kind::RemoveSet,
    Kind::RenameSet,
    Kind::SetOut,
    Kind::AddColumn,
    Kind::RemoveColumn,
    Kind::AddOrder,
    Kind::SetLimit,
    Kind::SetParam,
    Kind::SetLevel,
    Kind::ExcludeScenario,
    Kind::UpdateSelection,
    Kind::KeepSet,
    Kind::AddMeasure,
];

const _: () = assert!(CATALOG.len() <= MOVE_KINDS_CAP);

/// One hole of a template: its name, its type, and the legal fillers when
/// the language bounds them (an empty list means free text or a number).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fillers: Vec<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// One move on offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Move {
    pub id: u32,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    pub template: String,
    pub holes: Vec<Hole>,
}

/// One options response: the set's resolved shape, its sentence, its
/// diagnostics, the moves, the presets, and what to call next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Options {
    /// Names the (document, set, epoch, scheme, scope) this list is for;
    /// `apply` refuses another with `stale_options`.
    pub token: String,
    pub hash: String,
    pub epoch: i64,
    pub scheme_digest: String,
    pub set: String,
    pub grain: String,
    pub describe: String,
    pub exposes: Value,
    pub diagnostics: Vec<Issue>,
    pub moves: Vec<Move>,
    pub presets: Vec<Value>,
    pub next: Vec<String>,
    pub count_on_options: bool,
    pub preview_on_options: bool,
}

/// One move to apply, by id, with its arguments by hole name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveCall {
    pub move_id: u32,
    #[serde(default)]
    pub args: BTreeMap<String, Value>,
}

#[derive(Debug)]
pub enum MoveError {
    NoSuchMove(u32),
    Arg {
        move_id: u32,
        hole: String,
        message: String,
    },
    Message(String),
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveError::NoSuchMove(id) => write!(
                f,
                "no move {id} in this options list; POST /api/ask/options for the current one"
            ),
            MoveError::Arg {
                move_id,
                hole,
                message,
            } => write!(f, "move {move_id}, hole {hole}: {message}"),
            MoveError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for MoveError {}

/// The token of one options list.
pub fn token(hash: &str, epoch: i64, scheme_digest: &str, set: &str, scope: &Scope) -> String {
    let mut h = Blake2b::<U16>::new();
    h.update(hash.as_bytes());
    h.update(epoch.to_string().as_bytes());
    h.update(scheme_digest.as_bytes());
    h.update(set.as_bytes());
    h.update(if scope.federated { b"f" } else { b"l" });
    let mut classes: Vec<String> = scope.classes.iter().map(|c| format!("{c:?}")).collect();
    classes.sort();
    h.update(classes.join(",").as_bytes());
    hex::encode(h.finalize())
}

fn visible(class: Class, scope: &Scope) -> bool {
    match class {
        Class::Identifying => false,
        Class::Sensitive => scope.classes.contains(&Class::Sensitive),
        _ => true,
    }
}

/// The levels a grain reads fields from, own level first.
fn reach(grain: Grain) -> &'static [&'static str] {
    match grain {
        Grain::Cohort => &["cohort"],
        Grain::Subject => &["subject"],
        Grain::Session => &["session", "subject"],
        Grain::Stack => &["stack", "session", "series", "study", "subject"],
        Grain::Instance => &["instance", "stack", "session", "series", "study", "subject"],
        Grain::Event => &["event", "subject"],
        Grain::Group | Grain::Pair => &[],
    }
}

/// The paths a set may name in a predicate: its own level's fields, its
/// ancestors' under their level, its bindings, its partners' dates and
/// bindings, its group's by fields, `pick.*`.
fn paths_of(
    ask: &Ask,
    set_name: &str,
    set: &Set,
    exposed: &Validated,
    names: &dyn Names,
    scope: &Scope,
) -> (Vec<String>, Vec<String>) {
    let mut fields: Vec<String> = Vec::new();
    let mut dated: Vec<String> = Vec::new();
    for (i, level) in reach(set.grain).iter().enumerate() {
        let mut own: Vec<(String, crate::validate::FieldInfo)> = names.fields_of(level);
        own.sort_by(|a, b| a.0.cmp(&b.0));
        for (p, info) in own {
            if !visible(info.class, scope) {
                continue;
            }
            let path = if i == 0 { p } else { format!("{level}.{p}") };
            if info.dated {
                dated.push(path.clone());
            }
            fields.push(path);
        }
    }
    if let Some(e) = exposed.sets.get(set_name) {
        for b in &e.bindings {
            fields.push(b.clone());
        }
        for (as_, partner) in &e.partners {
            // a near partner carries its date and offset; an attached one
            // its fields and bindings only
            if set.near.iter().any(|n| n.as_ == *as_)
                || ask
                    .sets
                    .get(set_name)
                    .is_some_and(|s| s.near.iter().any(|n| n.as_ == *as_))
            {
                fields.push(format!("{as_}.date"));
                fields.push(format!("{as_}.offset_days"));
                dated.push(format!("{as_}.date"));
            }
            if let Some(p) = exposed.sets.get(partner) {
                for b in &p.bindings {
                    fields.push(format!("{as_}.{b}"));
                }
            }
        }
        if let Some(of) = &e.of
            && let Some(a) = exposed.sets.get(of)
        {
            for b in &a.bindings {
                fields.push(format!("{of}.{b}"));
            }
        }
        for g in &e.group_by {
            fields.push(g.clone());
        }
        if e.picked {
            for p in ["pick.tied", "pick.candidates", "pick.rank"] {
                fields.push(p.to_string());
            }
        }
        if set.grain == Grain::Group {
            fields.push("_rows".into());
            fields.push("_subjects".into());
        }
    }
    fields.dedup();
    (fields, dated)
}

fn strings(v: impl IntoIterator<Item = String>) -> Vec<Value> {
    v.into_iter().map(Value::String).collect()
}

fn hole(name: &str, type_: &str, fillers: Vec<Value>) -> Hole {
    Hole {
        name: name.into(),
        type_: type_.into(),
        fillers,
        optional: false,
    }
}

fn optional(mut h: Hole) -> Hole {
    h.optional = true;
    h
}

fn is_descendant(child: Grain, parent: Grain) -> bool {
    matches!(
        (parent, child),
        (
            Grain::Subject,
            Grain::Session | Grain::Stack | Grain::Instance | Grain::Event
        ) | (Grain::Session, Grain::Stack | Grain::Instance)
            | (Grain::Stack, Grain::Instance)
            | (Grain::Cohort, Grain::Subject)
    )
}

fn ancestors(grain: Grain) -> Vec<Grain> {
    match grain {
        Grain::Subject => vec![Grain::Cohort],
        Grain::Session | Grain::Event => vec![Grain::Subject],
        Grain::Stack => vec![Grain::Session, Grain::Subject],
        Grain::Instance => vec![Grain::Stack, Grain::Session, Grain::Subject],
        _ => Vec::new(),
    }
}

/// The options of one set of a validated document. Pure: no query.
#[allow(clippy::too_many_arguments)]
pub fn options(
    ask: &Ask,
    validated: &Validated,
    names: &dyn Names,
    scope: &Scope,
    hash: &str,
    epoch: i64,
    scheme_digest: &str,
    set_name: &str,
    values_cap: usize,
) -> Result<Options, Issue> {
    let set = ask.sets.get(set_name).ok_or_else(|| Issue {
        code: crate::validate::Code::UnknownSet,
        path: "set".into(),
        message: format!("no set named {set_name}"),
        next: format!(
            "one of {}",
            ask.sets
                .keys()
                .filter(|k| !k.contains("__"))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })?;
    let grain = set.grain;
    let (fields, dated) = paths_of(ask, set_name, set, validated, names, scope);
    let visible_sets: Vec<String> = ask
        .sets
        .keys()
        .filter(|k| !k.contains("__"))
        .cloned()
        .collect();
    let others: Vec<String> = visible_sets
        .iter()
        .filter(|k| *k != set_name)
        .cloned()
        .collect();
    let ops = strings(
        [
            "=",
            "<>",
            ">",
            ">=",
            "<",
            "<=",
            "contains",
            "starts_with",
            "not_null",
            "is_null",
        ]
        .map(String::from),
    );
    let dirs = strings(["asc", "desc"].map(String::from));
    let presets: Vec<Value> = PRESETS
        .iter()
        .map(|(n, d)| json!({"name": n, "days": d}))
        .collect();
    let preset_names = strings(PRESETS.iter().map(|(n, _)| (*n).to_string()));
    let mut moves: Vec<Move> = Vec::new();
    let mut push = |kind: Kind, set: Option<&str>, template: &str, holes: Vec<Hole>| {
        moves.push(Move {
            id: moves.len() as u32 + 1,
            kind,
            set: set.map(str::to_string),
            template: template.into(),
            holes,
        });
    };
    let here = Some(set_name);
    if grain != Grain::Group || !fields.is_empty() {
        push(
            Kind::AddWhere,
            here,
            "where {field} {op} {value}",
            vec![
                hole("field", "field", strings(fields.clone())),
                hole("op", "op", ops.clone()),
                optional(hole("value", "value", Vec::new())),
            ],
        );
    }
    if grain == Grain::Event {
        // the kinds a principal may see, never a sensitive one without the class
        let mut kinds: Vec<String> = names
            .kinds()
            .into_iter()
            .filter(|(_, k)| !k.sensitive || scope.classes.contains(&Class::Sensitive))
            .map(|(n, _)| n)
            .collect();
        kinds.sort();
        push(
            Kind::AddWhere,
            here,
            "where kind = {kind}",
            vec![hole("kind", "kind", strings(kinds))],
        );
    }
    if matches!(grain, Grain::Stack | Grain::Instance) {
        let mut axes = names.axes();
        axes.sort();
        for axis in axes {
            let mut values = names.axis_values(&axis).unwrap_or_default();
            values.sort();
            values.truncate(values_cap);
            push(
                Kind::AddAxisWhere,
                here,
                &format!("where {axis} = {{value}}"),
                vec![
                    hole("axis", "axis", vec![Value::String(axis.clone())]),
                    hole("value", "value", strings(values)),
                ],
            );
        }
        let mut dispositions = names.axis_values("disposition").unwrap_or_default();
        dispositions.retain(|v| v != "acquisition");
        dispositions.sort();
        if !dispositions.is_empty() {
            push(
                Kind::ExcludeScenario,
                here,
                "where disposition <> {disposition}",
                vec![hole("disposition", "value", strings(dispositions))],
            );
        }
    }
    if !set.where_.is_empty() {
        let clauses: Vec<Value> = set
            .where_
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"index": i, "clause": clause_text(c)}))
            .collect();
        let indices: Vec<Value> = (0..set.where_.len()).map(|i| json!(i)).collect();
        push(
            Kind::RemoveWhere,
            here,
            "drop where clause {index}",
            vec![Hole {
                name: "index".into(),
                type_: "index".into(),
                fillers: indices.clone(),
                optional: false,
            }],
        );
        push(
            Kind::SetStrict,
            here,
            "read where clause {index} as {strict}",
            vec![
                Hole {
                    name: "index".into(),
                    type_: "index".into(),
                    fillers: indices,
                    optional: false,
                },
                hole("strict", "bool", vec![json!(true), json!(false)]),
            ],
        );
        let _ = clauses;
    }
    let windowed: Vec<String> = set
        .near
        .iter()
        .map(|n| format!("near:{}", n.as_))
        .chain(set.has.iter().map(|h| format!("has:{}", h.set)))
        .collect();
    if !windowed.is_empty() {
        push(
            Kind::SetWindow,
            here,
            "read {relation} within {preset} each way",
            vec![
                hole("relation", "relation", strings(windowed)),
                hole("preset", "preset", preset_names.clone()),
            ],
        );
    }
    if !set.near.is_empty() {
        let nears: Vec<String> = set.near.iter().map(|n| n.as_.clone()).collect();
        push(
            Kind::SetPolicy,
            here,
            "choose {near} by {policy} (tie {tie})",
            vec![
                hole("near", "near", strings(nears.clone())),
                hole(
                    "policy",
                    "policy",
                    strings(["nearest", "first", "last", "any"].map(String::from)),
                ),
                optional(hole(
                    "tie",
                    "tie",
                    strings(["earlier", "later"].map(String::from)),
                )),
            ],
        );
    }
    let relations: Vec<String> = set
        .near
        .iter()
        .map(|n| format!("near:{}", n.as_))
        .chain(set.attach.iter().map(|a| format!("attach:{}", a.as_)))
        .collect();
    if !relations.is_empty() {
        push(
            Kind::SetOptional,
            here,
            "keep rows without {relation}: {optional}",
            vec![
                hole("relation", "relation", strings(relations.clone())),
                hole("optional", "bool", vec![json!(true), json!(false)]),
            ],
        );
    }
    let removable: Vec<String> = relations
        .iter()
        .cloned()
        .chain(set.has.iter().map(|h| format!("has:{}", h.set)))
        .collect();
    if !removable.is_empty() {
        push(
            Kind::RemoveRelation,
            here,
            "drop {relation}",
            vec![hole("relation", "relation", strings(removable))],
        );
    }
    if grain.dated() {
        let partners: Vec<String> = others
            .iter()
            .filter(|o| ask.sets.get(*o).is_some_and(|s| s.grain.dated()))
            .cloned()
            .collect();
        if !partners.is_empty() {
            push(
                Kind::AddNear,
                here,
                "near {partner} as {as} within {preset} each way, {policy}",
                vec![
                    hole("partner", "set", strings(partners)),
                    hole("as", "name", Vec::new()),
                    hole("preset", "preset", preset_names.clone()),
                    hole(
                        "policy",
                        "policy",
                        strings(["nearest", "first", "last", "any"].map(String::from)),
                    ),
                ],
            );
        }
    }
    let attachable: Vec<String> = others
        .iter()
        .filter(|o| {
            ask.sets
                .get(*o)
                .is_some_and(|s| s.pick.is_some() && is_descendant(s.grain, grain))
        })
        .cloned()
        .collect();
    if !attachable.is_empty() {
        push(
            Kind::AddAttach,
            here,
            "with {partner} as {as}",
            vec![
                hole("partner", "set", strings(attachable)),
                hole("as", "name", Vec::new()),
            ],
        );
    }
    let countable: Vec<String> = others
        .iter()
        .filter(|o| {
            ask.sets
                .get(*o)
                .is_some_and(|s| is_descendant(s.grain, grain))
        })
        .cloned()
        .collect();
    if !countable.is_empty() {
        push(
            Kind::AddHas,
            here,
            "at least {min} and at most {max} {child} as {as}",
            vec![
                hole("child", "set", strings(countable)),
                optional(hole("min", "int", Vec::new())),
                optional(hole("max", "int", Vec::new())),
                optional(hole("as", "name", Vec::new())),
            ],
        );
    }
    if !set.has.is_empty() {
        let children: Vec<String> = set.has.iter().map(|h| h.set.clone()).collect();
        push(
            Kind::SetBound,
            here,
            "between {min} and {max} {child}",
            vec![
                hole("child", "set", strings(children)),
                optional(hole("min", "int", Vec::new())),
                optional(hole("max", "int", Vec::new())),
            ],
        );
    }
    if !fields.is_empty() {
        push(
            Kind::AddBind,
            here,
            "let {name} = {field}",
            vec![
                hole("name", "name", Vec::new()),
                hole("field", "field", strings(fields.clone())),
            ],
        );
    }
    let bindings: Vec<String> = set.bind.0.iter().map(|(b, _)| b.clone()).collect();
    if !bindings.is_empty() {
        push(
            Kind::RemoveBind,
            here,
            "drop the binding {binding}",
            vec![hole("binding", "binding", strings(bindings.clone()))],
        );
    }
    let signatures: Vec<String> = set
        .bind
        .0
        .iter()
        .filter(|(_, c)| c.op == "derived" && c.ref_name() == Some("signature"))
        .map(|(b, _)| b.clone())
        .collect();
    if !signatures.is_empty() {
        let mut levels = names.levels();
        levels.sort();
        push(
            Kind::SetLevel,
            here,
            "compare {binding} at the level {level}",
            vec![
                hole("binding", "binding", strings(signatures)),
                hole("level", "level", strings(levels)),
            ],
        );
    }
    let pick_per: Vec<String> = ancestors(grain)
        .iter()
        .map(|g| g.name().to_string())
        .collect();
    if !pick_per.is_empty() && !fields.is_empty() {
        push(
            Kind::AddPick,
            here,
            "one per {per} by {field} {dir}",
            vec![
                hole("per", "grain", strings(pick_per)),
                hole("field", "field", strings(fields.clone())),
                hole("dir", "dir", dirs.clone()),
            ],
        );
    }
    if set.pick.is_some() {
        push(Kind::RemovePick, here, "drop the pick", Vec::new());
    }
    // the document
    let grains =
        strings(["cohort", "subject", "session", "stack", "instance", "event"].map(String::from));
    let sources: Vec<String> = visible_sets
        .iter()
        .flat_map(|s| [format!("of:{s}"), format!("from:{s}")])
        .collect();
    push(
        Kind::AddSet,
        None,
        "a new set {name} of {grain}s, {source}",
        vec![
            hole("name", "name", Vec::new()),
            hole("grain", "grain", grains),
            optional(hole("source", "source", strings(sources))),
        ],
    );
    let unread: Vec<String> = visible_sets
        .iter()
        .filter(|s| {
            *s != &ask.out.set
                && !ask
                    .sets
                    .values()
                    .any(|other| other.reads().contains(&s.as_str()))
        })
        .cloned()
        .collect();
    if !unread.is_empty() {
        push(
            Kind::RemoveSet,
            None,
            "drop the set {set}",
            vec![hole("set", "set", strings(unread))],
        );
    }
    push(
        Kind::RenameSet,
        None,
        "rename {set} to {name}",
        vec![
            hole("set", "set", strings(visible_sets.clone())),
            hole("name", "name", Vec::new()),
        ],
    );
    push(
        Kind::SetOut,
        None,
        "answer with {set} at the {level} level",
        vec![
            hole("set", "set", strings(visible_sets.clone())),
            hole(
                "level",
                "level",
                strings(["boolean", "count", "aggregate", "record"].map(String::from)),
            ),
        ],
    );
    if ask.out.set == set_name && !fields.is_empty() {
        push(
            Kind::AddColumn,
            here,
            "show {field}",
            vec![hole("field", "field", strings(fields.clone()))],
        );
        push(
            Kind::AddOrder,
            here,
            "order by {field} {dir}",
            vec![
                hole("field", "field", strings(fields.clone())),
                hole("dir", "dir", dirs),
            ],
        );
    }
    if ask.out.set == set_name && !ask.out.columns.is_empty() {
        let cols: Vec<Value> = (0..ask.out.columns.len()).map(|i| json!(i)).collect();
        push(
            Kind::RemoveColumn,
            here,
            "hide column {index}",
            vec![Hole {
                name: "index".into(),
                type_: "index".into(),
                fillers: cols,
                optional: false,
            }],
        );
    }
    push(
        Kind::SetLimit,
        None,
        "at most {n} rows",
        vec![hole("n", "int", Vec::new())],
    );
    if !ask.params.is_empty() {
        push(
            Kind::SetParam,
            None,
            "set {param} to {value}",
            vec![
                hole("param", "param", strings(ask.params.keys().cloned())),
                hole("value", "value", Vec::new()),
            ],
        );
    }
    let outdated: Vec<String> = ask
        .sets
        .iter()
        .filter(|(_, s)| match &s.from {
            Some(Src::Selection {
                name,
                version: Some(v),
            }) => names.selection(name).is_some_and(|current| current > *v),
            _ => false,
        })
        .map(|(n, _)| n.clone())
        .collect();
    if !outdated.is_empty() {
        push(
            Kind::UpdateSelection,
            None,
            "read {set} from the selection's current version",
            vec![hole("set", "set", strings(outdated))],
        );
    }
    push(
        Kind::KeepSet,
        None,
        "keep {set} as a handle: {keep}",
        vec![
            hole("set", "set", strings(visible_sets.clone())),
            hole("keep", "bool", vec![json!(true), json!(false)]),
        ],
    );
    let out_columns: Vec<String> = ask
        .out
        .columns
        .iter()
        .filter_map(|c| c.ref_name().map(str::to_string))
        .collect();
    if !out_columns.is_empty() {
        push(
            Kind::AddMeasure,
            None,
            "{measure} of {of} over {over}",
            vec![
                hole(
                    "measure",
                    "measure",
                    strings(["share", "stddev", "median", "percentile"].map(String::from)),
                ),
                hole("of", "column", strings(out_columns)),
                optional(hole("over", "set", strings(visible_sets.clone()))),
                optional(hole("p", "number", Vec::new())),
            ],
        );
    }
    let exposed = validated.sets.get(set_name);
    let exposes = json!({
        "fields": fields,
        "dated": dated,
        "bindings": exposed.map(|e| e.bindings.clone()).unwrap_or_default(),
        "partners": exposed.map(|e| e.partners.clone()).unwrap_or_default(),
        "of": exposed.and_then(|e| e.of.clone()),
        "group_by": exposed.map(|e| e.group_by.clone()).unwrap_or_default(),
        "picked": exposed.is_some_and(|e| e.picked),
    });
    let diagnostics: Vec<Issue> = validated
        .warnings
        .iter()
        .filter(|w| w.path.starts_with(&format!("sets.{set_name}")) || !w.path.starts_with("sets."))
        .cloned()
        .collect();
    let mut next = vec![
        "POST /api/ask/apply with this token and moves".to_string(),
        "POST /api/ask/diagnose for the funnel and the drops".to_string(),
        "POST /api/ask/preview for ten rows".to_string(),
    ];
    if !diagnostics.is_empty() {
        next.insert(0, "read the diagnostics first".into());
    }
    Ok(Options {
        token: token(hash, epoch, scheme_digest, set_name, scope),
        hash: hash.to_string(),
        epoch,
        scheme_digest: scheme_digest.to_string(),
        set: set_name.to_string(),
        grain: grain.name().to_string(),
        describe: set_sentence(set_name, set),
        exposes,
        diagnostics,
        moves,
        presets,
        next,
        count_on_options: false,
        preview_on_options: false,
    })
}

fn arg_text(call: &MoveCall, m: &Move, hole: &str) -> Result<String, MoveError> {
    match call.args.get(hole) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        Some(Value::Bool(b)) => Ok(b.to_string()),
        Some(other) => Err(MoveError::Arg {
            move_id: m.id,
            hole: hole.into(),
            message: format!("{other} is not a text"),
        }),
        None => Err(MoveError::Arg {
            move_id: m.id,
            hole: hole.into(),
            message: "missing".into(),
        }),
    }
}

fn arg_opt(call: &MoveCall, hole: &str) -> Option<Value> {
    call.args.get(hole).cloned().filter(|v| !v.is_null())
}

fn arg_int(call: &MoveCall, m: &Move, hole: &str) -> Result<Option<i64>, MoveError> {
    match call.args.get(hole) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) if n.is_i64() => Ok(n.as_i64()),
        Some(Value::String(s)) => s.parse().map(Some).map_err(|_| MoveError::Arg {
            move_id: m.id,
            hole: hole.into(),
            message: format!("{s} is not an integer"),
        }),
        Some(other) => Err(MoveError::Arg {
            move_id: m.id,
            hole: hole.into(),
            message: format!("{other} is not an integer"),
        }),
    }
}

fn arg_bool(call: &MoveCall, m: &Move, hole: &str) -> Result<bool, MoveError> {
    match call.args.get(hole) {
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" || s == "false" => Ok(s == "true"),
        _ => Err(MoveError::Arg {
            move_id: m.id,
            hole: hole.into(),
            message: "true or false".into(),
        }),
    }
}

fn literal_of(v: &Value) -> Arg {
    match v {
        Value::String(s) => Arg::Text(s.clone()),
        Value::Number(n) if n.is_i64() => Arg::Int(n.as_i64().unwrap_or(0)),
        Value::Number(n) => Arg::Number(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => Arg::Bool(*b),
        Value::Null => Arg::Null,
        Value::Array(items) => Arg::List(items.iter().map(literal_of).collect()),
        Value::Object(_) => Arg::Null,
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__")
}

fn preset_window(name: &str) -> Option<Window> {
    PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| Window {
            from: Some(-d),
            to: Some(*d),
            unit: Unit::Day,
        })
}

/// A field path as a clause: `pick.tied` and friends are fields too.
fn field_clause(path: &str) -> Clause {
    Clause::field(path)
}

/// Check every filled hole against its fillers, and that no required hole
/// is missing.
fn check(call: &MoveCall, m: &Move) -> Result<(), MoveError> {
    for h in &m.holes {
        let given = call.args.get(&h.name).filter(|v| !v.is_null());
        match given {
            None if h.optional => continue,
            None => {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: h.name.clone(),
                    message: "missing".into(),
                });
            }
            Some(v) => {
                if !h.fillers.is_empty() {
                    let matches = h.fillers.iter().any(|f| {
                        f == v || matches!((f, v), (Value::Number(a), Value::String(b)) if a.to_string() == *b)
                    });
                    if !matches {
                        return Err(MoveError::Arg {
                            move_id: m.id,
                            hole: h.name.clone(),
                            message: format!(
                                "{v} is not among the fillers: {}",
                                h.fillers
                                    .iter()
                                    .take(12)
                                    .map(|f| f.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn set_mut<'a>(ask: &'a mut Ask, m: &Move) -> Result<&'a mut Set, MoveError> {
    let name = m
        .set
        .as_deref()
        .ok_or_else(|| MoveError::Message(format!("move {} names no set", m.id)))?;
    ask.sets
        .get_mut(name)
        .ok_or_else(|| MoveError::Message(format!("no set named {name}")))
}

/// Apply a list of moves atomically to a document: every move is checked
/// first, then all are applied; the sets touched are returned.
pub fn apply_moves(
    ask: &mut Ask,
    options: &Options,
    calls: &[MoveCall],
) -> Result<Vec<String>, MoveError> {
    let mut plan: Vec<(&MoveCall, &Move)> = Vec::new();
    for call in calls {
        let m = options
            .moves
            .iter()
            .find(|m| m.id == call.move_id)
            .ok_or(MoveError::NoSuchMove(call.move_id))?;
        check(call, m)?;
        plan.push((call, m));
    }
    let mut changed: BTreeSet<String> = BTreeSet::new();
    for (call, m) in plan {
        let touched = apply_one(ask, call, m)?;
        changed.extend(touched);
    }
    Ok(changed.into_iter().collect())
}

fn apply_one(ask: &mut Ask, call: &MoveCall, m: &Move) -> Result<Vec<String>, MoveError> {
    let set_name = m.set.clone();
    let touched = |s: &Option<String>| s.iter().cloned().collect::<Vec<_>>();
    match m.kind {
        Kind::AddWhere => {
            let s = set_mut(ask, m)?;
            if let Some(kind) = arg_opt(call, "kind") {
                s.where_.push(
                    Clause::new("=")
                        .arg(Arg::Clause(field_clause("kind")))
                        .arg(literal_of(&kind)),
                );
            } else {
                let field = arg_text(call, m, "field")?;
                let op = arg_text(call, m, "op")?;
                let mut c = Clause::new(&op).arg(Arg::Clause(field_clause(&field)));
                if !matches!(op.as_str(), "not_null" | "is_null") {
                    let v = arg_opt(call, "value").ok_or_else(|| MoveError::Arg {
                        move_id: m.id,
                        hole: "value".into(),
                        message: format!("{op} needs a value"),
                    })?;
                    c = c.arg(literal_of(&v));
                }
                s.where_.push(c);
            }
            Ok(touched(&set_name))
        }
        Kind::AddAxisWhere => {
            let axis = arg_text(call, m, "axis")?;
            let value = arg_text(call, m, "value")?;
            let s = set_mut(ask, m)?;
            s.where_.push(
                Clause::new("=")
                    .arg(Arg::Clause(Clause::new("axis").arg(Arg::Text(axis))))
                    .arg(Arg::Text(value)),
            );
            Ok(touched(&set_name))
        }
        Kind::ExcludeScenario => {
            let value = arg_text(call, m, "disposition")?;
            let s = set_mut(ask, m)?;
            s.where_.push(
                Clause::new("<>")
                    .arg(Arg::Clause(
                        Clause::new("axis").arg(Arg::Text("disposition".into())),
                    ))
                    .arg(Arg::Text(value)),
            );
            Ok(touched(&set_name))
        }
        Kind::RemoveWhere => {
            let i = arg_int(call, m, "index")?.unwrap_or(-1);
            let s = set_mut(ask, m)?;
            if i < 0 || i as usize >= s.where_.len() {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "index".into(),
                    message: "no such clause".into(),
                });
            }
            s.where_.remove(i as usize);
            Ok(touched(&set_name))
        }
        Kind::SetStrict => {
            let i = arg_int(call, m, "index")?.unwrap_or(-1);
            let strict = arg_bool(call, m, "strict")?;
            let s = set_mut(ask, m)?;
            let c = s
                .where_
                .get_mut(i.max(0) as usize)
                .ok_or_else(|| MoveError::Arg {
                    move_id: m.id,
                    hole: "index".into(),
                    message: "no such clause".into(),
                })?;
            if strict {
                c.opts.insert("strict".into(), Value::Bool(true));
            } else {
                c.opts.remove("strict");
            }
            Ok(touched(&set_name))
        }
        Kind::SetWindow => {
            let relation = arg_text(call, m, "relation")?;
            let preset = arg_text(call, m, "preset")?;
            let window = preset_window(&preset).ok_or_else(|| MoveError::Arg {
                move_id: m.id,
                hole: "preset".into(),
                message: "not a preset".into(),
            })?;
            let s = set_mut(ask, m)?;
            if let Some(as_) = relation.strip_prefix("near:") {
                let n = s
                    .near
                    .iter_mut()
                    .find(|n| n.as_ == as_)
                    .ok_or_else(|| MoveError::Arg {
                        move_id: m.id,
                        hole: "relation".into(),
                        message: "no such near".into(),
                    })?;
                n.window = WindowSpec::Literal(window);
            } else if let Some(child) = relation.strip_prefix("has:") {
                let h =
                    s.has
                        .iter_mut()
                        .find(|h| h.set == child)
                        .ok_or_else(|| MoveError::Arg {
                            move_id: m.id,
                            hole: "relation".into(),
                            message: "no such has".into(),
                        })?;
                h.window = Some(WindowSpec::Literal(window));
            } else {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "relation".into(),
                    message: "near:<as> or has:<set>".into(),
                });
            }
            Ok(touched(&set_name))
        }
        Kind::SetPolicy => {
            let as_ = arg_text(call, m, "near")?;
            let policy = match arg_text(call, m, "policy")?.as_str() {
                "nearest" => Policy::Nearest,
                "first" => Policy::First,
                "last" => Policy::Last,
                "any" => Policy::Any,
                other => {
                    return Err(MoveError::Arg {
                        move_id: m.id,
                        hole: "policy".into(),
                        message: format!(
                            "{other} is not a policy a move sets; best needs an order, which draft composes"
                        ),
                    });
                }
            };
            let tie = match arg_opt(call, "tie").and_then(|v| v.as_str().map(str::to_string)) {
                Some(t) if t == "later" => Some(Tie::Later),
                Some(_) => Some(Tie::Earlier),
                None => None,
            };
            let s = set_mut(ask, m)?;
            let n = s
                .near
                .iter_mut()
                .find(|n| n.as_ == as_)
                .ok_or_else(|| MoveError::Arg {
                    move_id: m.id,
                    hole: "near".into(),
                    message: "no such near".into(),
                })?;
            n.policy = policy;
            n.tie = tie;
            n.order.clear();
            Ok(touched(&set_name))
        }
        Kind::SetOptional => {
            let relation = arg_text(call, m, "relation")?;
            let optional = arg_bool(call, m, "optional")?;
            let s = set_mut(ask, m)?;
            if let Some(as_) = relation.strip_prefix("near:") {
                if let Some(n) = s.near.iter_mut().find(|n| n.as_ == as_) {
                    n.optional = optional;
                }
            } else if let Some(as_) = relation.strip_prefix("attach:")
                && let Some(a) = s.attach.iter_mut().find(|a| a.as_ == as_)
            {
                a.optional = optional;
            }
            Ok(touched(&set_name))
        }
        Kind::RemoveRelation => {
            let relation = arg_text(call, m, "relation")?;
            let s = set_mut(ask, m)?;
            if let Some(as_) = relation.strip_prefix("near:") {
                s.near.retain(|n| n.as_ != as_);
            } else if let Some(as_) = relation.strip_prefix("attach:") {
                s.attach.retain(|a| a.as_ != as_);
            } else if let Some(child) = relation.strip_prefix("has:") {
                s.has.retain(|h| h.set != child);
            }
            Ok(touched(&set_name))
        }
        Kind::AddNear => {
            let partner = arg_text(call, m, "partner")?;
            let as_ = arg_text(call, m, "as")?;
            if !valid_name(&as_) {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "as".into(),
                    message: "a name is lower case letters, digits and single underscores".into(),
                });
            }
            let preset = arg_text(call, m, "preset")?;
            let window = preset_window(&preset).ok_or_else(|| MoveError::Arg {
                move_id: m.id,
                hole: "preset".into(),
                message: "not a preset".into(),
            })?;
            let policy = match arg_text(call, m, "policy")?.as_str() {
                "first" => Policy::First,
                "last" => Policy::Last,
                "any" => Policy::Any,
                _ => Policy::Nearest,
            };
            let s = set_mut(ask, m)?;
            s.near.push(Near {
                as_,
                set: partner,
                window: WindowSpec::Literal(window),
                on: None,
                policy,
                tie: None,
                order: Vec::new(),
                optional: false,
                strict: false,
            });
            Ok(touched(&set_name))
        }
        Kind::AddAttach => {
            let partner = arg_text(call, m, "partner")?;
            let as_ = arg_text(call, m, "as")?;
            if !valid_name(&as_) {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "as".into(),
                    message: "a name is lower case letters, digits and single underscores".into(),
                });
            }
            let s = set_mut(ask, m)?;
            s.attach.push(Attach {
                as_,
                set: partner,
                optional: false,
            });
            Ok(touched(&set_name))
        }
        Kind::AddHas => {
            let child = arg_text(call, m, "child")?;
            let min = arg_int(call, m, "min")?;
            let max = arg_int(call, m, "max")?;
            let as_ = arg_opt(call, "as").and_then(|v| v.as_str().map(str::to_string));
            if let Some(a) = &as_
                && !valid_name(a)
            {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "as".into(),
                    message: "a name is lower case letters, digits and single underscores".into(),
                });
            }
            let s = set_mut(ask, m)?;
            s.has.push(Has {
                set: child,
                window: None,
                on: None,
                min: min.map(IntSpec::Literal),
                max: max.map(IntSpec::Literal),
                as_,
            });
            Ok(touched(&set_name))
        }
        Kind::SetBound => {
            let child = arg_text(call, m, "child")?;
            let min = arg_int(call, m, "min")?;
            let max = arg_int(call, m, "max")?;
            let s = set_mut(ask, m)?;
            let h = s
                .has
                .iter_mut()
                .find(|h| h.set == child)
                .ok_or_else(|| MoveError::Arg {
                    move_id: m.id,
                    hole: "child".into(),
                    message: "no such has".into(),
                })?;
            h.min = min.map(IntSpec::Literal);
            h.max = max.map(IntSpec::Literal);
            Ok(touched(&set_name))
        }
        Kind::AddBind => {
            let name = arg_text(call, m, "name")?;
            if !valid_name(&name) {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "name".into(),
                    message: "a name is lower case letters, digits and single underscores".into(),
                });
            }
            let field = arg_text(call, m, "field")?;
            let s = set_mut(ask, m)?;
            s.bind.0.retain(|(b, _)| *b != name);
            s.bind.0.push((name, field_clause(&field)));
            Ok(touched(&set_name))
        }
        Kind::RemoveBind => {
            let binding = arg_text(call, m, "binding")?;
            let s = set_mut(ask, m)?;
            s.bind.0.retain(|(b, _)| *b != binding);
            Ok(touched(&set_name))
        }
        Kind::SetLevel => {
            let binding = arg_text(call, m, "binding")?;
            let level = arg_text(call, m, "level")?;
            let s = set_mut(ask, m)?;
            for (b, c) in s.bind.0.iter_mut() {
                if *b == binding {
                    c.opts.insert("level".into(), Value::String(level.clone()));
                }
            }
            Ok(touched(&set_name))
        }
        Kind::AddPick => {
            let per: Grain = serde_json::from_value(Value::String(arg_text(call, m, "per")?))
                .map_err(|_| MoveError::Arg {
                    move_id: m.id,
                    hole: "per".into(),
                    message: "not a grain".into(),
                })?;
            let field = arg_text(call, m, "field")?;
            let dir = if arg_text(call, m, "dir")? == "desc" {
                Dir::Desc
            } else {
                Dir::Asc
            };
            let s = set_mut(ask, m)?;
            s.pick = Some(Pick {
                per,
                by: vec![Order(field_clause(&field), dir)],
                n: None,
                ties: Some(Ties::Report),
            });
            Ok(touched(&set_name))
        }
        Kind::RemovePick => {
            let s = set_mut(ask, m)?;
            s.pick = None;
            Ok(touched(&set_name))
        }
        Kind::AddSet => {
            let name = arg_text(call, m, "name")?;
            if !valid_name(&name) || ask.sets.contains_key(&name) {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "name".into(),
                    message: "a new, lower case name".into(),
                });
            }
            let grain: Grain = serde_json::from_value(Value::String(arg_text(call, m, "grain")?))
                .map_err(|_| MoveError::Arg {
                move_id: m.id,
                hole: "grain".into(),
                message: "not a grain".into(),
            })?;
            let mut s = Set::at(grain);
            if let Some(source) =
                arg_opt(call, "source").and_then(|v| v.as_str().map(str::to_string))
            {
                if let Some(of) = source.strip_prefix("of:") {
                    s.of = Some(of.to_string());
                } else if let Some(from) = source.strip_prefix("from:") {
                    s.from = Some(Src::Set(from.to_string()));
                }
            }
            ask.sets.insert(name.clone(), s);
            Ok(vec![name])
        }
        Kind::RemoveSet => {
            let name = arg_text(call, m, "set")?;
            ask.sets.remove(&name);
            ask.keep.retain(|k| *k != name);
            Ok(vec![name])
        }
        Kind::RenameSet => {
            let old = arg_text(call, m, "set")?;
            let new = arg_text(call, m, "name")?;
            if !valid_name(&new) || ask.sets.contains_key(&new) {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "name".into(),
                    message: "a new, lower case name".into(),
                });
            }
            rename_set(ask, &old, &new);
            Ok(vec![new])
        }
        Kind::SetOut => {
            let set = arg_text(call, m, "set")?;
            let level: Level = serde_json::from_value(Value::String(arg_text(call, m, "level")?))
                .map_err(|_| MoveError::Arg {
                move_id: m.id,
                hole: "level".into(),
                message: "boolean, count, aggregate or record".into(),
            })?;
            if ask.out.set != set {
                ask.out.columns.clear();
                ask.out.order.clear();
                ask.out.measures.clear();
            }
            ask.out.set = set.clone();
            ask.out.level = level;
            Ok(vec![set])
        }
        Kind::AddColumn => {
            let field = arg_text(call, m, "field")?;
            ask.out.columns.push(field_clause(&field));
            Ok(vec![ask.out.set.clone()])
        }
        Kind::RemoveColumn => {
            let i = arg_int(call, m, "index")?.unwrap_or(-1);
            if i < 0 || i as usize >= ask.out.columns.len() {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "index".into(),
                    message: "no such column".into(),
                });
            }
            ask.out.columns.remove(i as usize);
            Ok(vec![ask.out.set.clone()])
        }
        Kind::AddOrder => {
            let field = arg_text(call, m, "field")?;
            let dir = if arg_text(call, m, "dir")? == "desc" {
                Dir::Desc
            } else {
                Dir::Asc
            };
            ask.out.order.push(Order(field_clause(&field), dir));
            Ok(vec![ask.out.set.clone()])
        }
        Kind::SetLimit => {
            let n = arg_int(call, m, "n")?.ok_or_else(|| MoveError::Arg {
                move_id: m.id,
                hole: "n".into(),
                message: "missing".into(),
            })?;
            ask.out.limit = (n > 0).then_some(n as u64);
            Ok(vec![ask.out.set.clone()])
        }
        Kind::SetParam => {
            let name = arg_text(call, m, "param")?;
            let value = arg_opt(call, "value").ok_or_else(|| MoveError::Arg {
                move_id: m.id,
                hole: "value".into(),
                message: "missing".into(),
            })?;
            let d = ask.params.get_mut(&name).ok_or_else(|| MoveError::Arg {
                move_id: m.id,
                hole: "param".into(),
                message: "no such parameter".into(),
            })?;
            d.value = Some(value);
            Ok(Vec::new())
        }
        Kind::UpdateSelection => {
            let set = arg_text(call, m, "set")?;
            let s = ask
                .sets
                .get_mut(&set)
                .ok_or_else(|| MoveError::Message(format!("no set named {set}")))?;
            if let Some(Src::Selection { version, .. }) = &mut s.from {
                *version = None;
            }
            Ok(vec![set])
        }
        Kind::KeepSet => {
            let set = arg_text(call, m, "set")?;
            let keep = arg_bool(call, m, "keep")?;
            ask.keep.retain(|k| *k != set);
            if keep {
                ask.keep.push(set.clone());
            }
            Ok(vec![set])
        }
        Kind::AddMeasure => {
            let kind = arg_text(call, m, "measure")?;
            let of = arg_text(call, m, "of")?;
            let mut spec = serde_json::Map::new();
            spec.insert("of".into(), Value::String(of));
            if let Some(over) = arg_opt(call, "over") {
                spec.insert("over".into(), over);
            }
            if let Some(p) = arg_opt(call, "p") {
                spec.insert("p".into(), p);
            }
            if kind == "share" && !spec.contains_key("over") {
                return Err(MoveError::Arg {
                    move_id: m.id,
                    hole: "over".into(),
                    message: "a share names the set it is over".into(),
                });
            }
            let mut mm = BTreeMap::new();
            mm.insert(kind, Value::Object(spec));
            ask.out.measures.push(Measure(mm));
            Ok(vec![ask.out.set.clone()])
        }
    }
}

/// Rename a set everywhere the document names it.
fn rename_set(ask: &mut Ask, old: &str, new: &str) {
    let Some(set) = ask.sets.remove(old) else {
        return;
    };
    ask.sets.insert(new.to_string(), set);
    let fix = |s: &mut String| {
        if s == old {
            *s = new.to_string();
        }
    };
    for s in ask.sets.values_mut() {
        if let Some(Src::Set(x)) = &mut s.from {
            fix(x);
        }
        if let Some(o) = &mut s.of {
            fix(o);
        }
        if let Some(a) = &mut s.algebra {
            a.sets.iter_mut().for_each(fix);
        }
        if let Some(g) = &mut s.group {
            fix(&mut g.of);
        }
        s.near.iter_mut().for_each(|n| fix(&mut n.set));
        s.attach.iter_mut().for_each(|a| fix(&mut a.set));
        s.has.iter_mut().for_each(|h| fix(&mut h.set));
        s.same.iter_mut().for_each(|x| fix(&mut x.over));
        for e in s.every.iter_mut() {
            fix(&mut e.of);
            fix(&mut e.in_);
        }
        for (_, c) in s.bind.0.iter_mut() {
            rename_in_clause(c, old, new);
        }
        for c in s.where_.iter_mut() {
            rename_in_clause(c, old, new);
        }
    }
    ask.keep.iter_mut().for_each(fix);
    fix(&mut ask.out.set);
    for m in ask.out.measures.iter_mut() {
        for v in m.0.values_mut() {
            if let Some(Value::String(over)) = v.get_mut("over")
                && over == old
            {
                *over = new.to_string();
            }
        }
    }
}

fn rename_in_clause(c: &mut Clause, old: &str, new: &str) {
    for key in ["set", "over"] {
        if let Some(Value::String(s)) = c.opts.get_mut(key)
            && s == old
        {
            *s = new.to_string();
        }
    }
    for a in c.args.iter_mut() {
        if let Arg::Clause(inner) = a {
            rename_in_clause(inner, old, new);
        }
    }
}
