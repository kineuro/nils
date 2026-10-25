// SPDX-License-Identifier: AGPL-3.0-only

//! The workflow document, `nils.workflow.yml` (contracts/workflow/v1, Wave
//! 7b section 8.2, record 57 P7): steps, each a pipeline or a family of the
//! catalog at a version or `current`, with the goal and the knobs a person
//! may set, and links, each an output of one step wired to an input of a
//! later one. The loader checks the document's shape and says what is wrong
//! in a sentence; the engine's plan checks the links against the catalog's
//! descriptors and packs (section 8.3), since what a link means depends on
//! the versions the catalog holds when the workflow is planned.

use std::collections::BTreeMap;

use serde::Serialize;
use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::descriptor::{Descriptor, Output, TypedInput};
use crate::pack::PipelinePack;

/// The file a workflow folder holds.
pub const FILE: &str = "nils.workflow.yml";

/// The contract version this loader reads.
pub const CONTRACT: i64 = 1;

/// The verdicts a plan gives a link (section 8.3, G12).
pub const VERDICTS: [&str; 5] = ["satisfied", "missing", "redundant", "refused", "fed"];

/// One step: a pipeline or a family, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: String,
    /// `pipeline` or `family`.
    pub of: String,
    pub name: String,
    /// `current`, or a version number of the name.
    pub version: Option<i64>,
    pub goal: Option<String>,
    /// The knobs a person may set when the workflow runs.
    pub knobs: Vec<String>,
    /// The pack's switches fixed for the step (`lesions: on`).
    pub switches: Map<String, Value>,
}

impl Step {
    /// `name`, `name@version`.
    pub fn reference(&self) -> String {
        match self.version {
            Some(v) => format!("{}@{v}", self.name),
            None => self.name.clone(),
        }
    }
}

/// One link: `<step>.<output>` to `<step>.<input>`, where the input is a pick
/// role of the step's descriptor (`flair`) or one of its typed inputs
/// (`synthseg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub from_step: String,
    pub output: String,
    pub to_step: String,
    pub input: String,
}

impl Link {
    pub fn words(&self) -> String {
        format!(
            "{}.{} -> {}.{}",
            self.from_step, self.output, self.to_step, self.input
        )
    }
}

/// What a version replaces (`replaces`, as a descriptor's `x-nils.replaces`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaces {
    pub versions: Option<Vec<i64>>,
    pub why: String,
}

/// A workflow as loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub name: String,
    pub title: String,
    pub what: String,
    pub replaces: Option<Replaces>,
    pub steps: Vec<Step>,
    pub links: Vec<Link>,
    /// The document as it parsed, kept whole.
    pub document: Value,
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn text(v: &Value, key: &str, at: &str) -> Result<String, String> {
    match &v[key] {
        Value::String(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        Value::Null => Err(format!("{at}{key} is missing")),
        _ => Err(format!("{at}{key} is text")),
    }
}

fn opt_text(v: &Value, key: &str, at: &str) -> Result<Option<String>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.trim().to_string())),
        _ => Err(format!("{at}{key} is text")),
    }
}

/// Parse a workflow from its YAML (JSON is YAML) and check its shape.
pub fn parse(text_: &str) -> Result<Workflow, String> {
    let document: Value =
        serde_saphyr::from_str(text_).map_err(|e| format!("not a YAML document: {e}"))?;
    from_value(document)
}

/// [`parse`] of a document already read.
pub fn from_value(document: Value) -> Result<Workflow, String> {
    if !document.is_object() {
        return Err("a workflow is a mapping: nils_workflow, name, steps, links".into());
    }
    match document["nils_workflow"].as_i64() {
        Some(CONTRACT) => {}
        Some(other) => {
            return Err(format!(
                "nils_workflow {other} is not one this engine reads; it reads {CONTRACT}"
            ));
        }
        None => return Err("nils_workflow is missing: the contract version, 1".into()),
    }
    let name = text(&document, "name", "")?;
    if !is_ident(&name) {
        return Err(format!(
            "name {name} is lowercase letters, digits, _ and -, starting with a letter"
        ));
    }
    let title = opt_text(&document, "title", "")?.unwrap_or_else(|| name.clone());
    let what = opt_text(&document, "what", "")?.unwrap_or_default();
    let replaces = match &document["replaces"] {
        Value::Null => None,
        Value::Object(_) => {
            let r = &document["replaces"];
            let why = text(r, "why", "replaces.")?;
            let versions = match &r["versions"] {
                Value::Null => None,
                Value::Array(list) if !list.is_empty() => {
                    let mut out = Vec::new();
                    for (i, v) in list.iter().enumerate() {
                        match v.as_i64() {
                            Some(n) if n >= 1 => {
                                if !out.contains(&n) {
                                    out.push(n);
                                }
                            }
                            _ => {
                                return Err(format!(
                                    "replaces.versions[{i}] is a version number, 1 or more"
                                ));
                            }
                        }
                    }
                    out.sort_unstable();
                    Some(out)
                }
                _ => {
                    return Err(
                        "replaces.versions is a list of earlier version numbers; leave it out for every earlier version".into(),
                    );
                }
            };
            Some(Replaces { versions, why })
        }
        _ => {
            return Err(
                "replaces is a mapping: why, and versions where not every earlier one".into(),
            );
        }
    };
    let Some(list) = document["steps"].as_array() else {
        return Err("steps is a list of steps, each a pipeline or a family".into());
    };
    if list.is_empty() {
        return Err("steps holds at least one step".into());
    }
    let mut steps: Vec<Step> = Vec::new();
    for (i, s) in list.iter().enumerate() {
        let at = format!("steps[{i}].");
        if !s.is_object() {
            return Err(format!(
                "{at}is a mapping: id, pipeline or family, version, goal, knobs, switches"
            ));
        }
        let id = text(s, "id", &at)?;
        if !is_ident(&id) {
            return Err(format!("{at}id {id} is lowercase letters, digits, _ and -"));
        }
        if steps.iter().any(|q| q.id == id) {
            return Err(format!("{at}id {id} is used twice"));
        }
        let (of, name) = match (opt_text(s, "pipeline", &at)?, opt_text(s, "family", &at)?) {
            (Some(p), None) => ("pipeline", p),
            (None, Some(f)) => ("family", f),
            (Some(_), Some(_)) => {
                return Err(format!("{at}names a pipeline or a family, not both"));
            }
            (None, None) => return Err(format!("{at}names a pipeline or a family")),
        };
        if !is_ident(&name) {
            return Err(format!("{at}{of} {name} is a catalog name"));
        }
        let version = match &s["version"] {
            Value::Null => None,
            Value::String(v) if v.trim() == "current" => None,
            Value::Number(n) if n.as_i64().is_some_and(|n| n >= 1) => n.as_i64(),
            Value::String(v) => match v.trim().parse::<i64>() {
                Ok(n) if n >= 1 => Some(n),
                _ => {
                    return Err(format!(
                        "{at}version is current or a version number, not {v}"
                    ));
                }
            },
            other => {
                return Err(format!(
                    "{at}version is current or a version number, not {other}"
                ));
            }
        };
        let goal = opt_text(s, "goal", &at)?;
        if let Some(g) = &goal
            && !crate::pack::facts::GOALS.contains(&g.as_str())
        {
            return Err(format!(
                "{at}goal is one of {}, not {g}",
                crate::pack::facts::GOALS.join(", ")
            ));
        }
        let knobs = match &s["knobs"] {
            Value::Null => Vec::new(),
            Value::Array(items) => items
                .iter()
                .map(|k| {
                    k.as_str()
                        .filter(|k| !k.trim().is_empty())
                        .map(|k| k.trim().to_string())
                        .ok_or_else(|| format!("{at}knobs is a list of knob ids a person may set"))
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(format!("{at}knobs is a list of knob ids a person may set")),
        };
        let switches = match &s["switches"] {
            Value::Null => Map::new(),
            Value::Object(o) => o.clone(),
            _ => {
                return Err(format!(
                    "{at}switches is a mapping of a switch to its value"
                ));
            }
        };
        steps.push(Step {
            id,
            of: of.to_string(),
            name,
            version,
            goal,
            knobs,
            switches,
        });
    }
    let mut links: Vec<Link> = Vec::new();
    match &document["links"] {
        Value::Null => {}
        Value::Array(list) => {
            for (i, l) in list.iter().enumerate() {
                let at = format!("links[{i}].");
                let ends = |key: &str| -> Result<(String, String), String> {
                    let t = text(l, key, &at)?;
                    let (step, name) = t.split_once('.').ok_or_else(|| {
                        format!(
                            "{at}{key} is <step>.<{}>, not {t}",
                            if key == "from" { "output" } else { "input" }
                        )
                    })?;
                    if !steps.iter().any(|s| s.id == step) {
                        return Err(format!(
                            "{at}{key} names the step {step}, which steps does not hold"
                        ));
                    }
                    if name.trim().is_empty() {
                        return Err(format!(
                            "{at}{key} names no {}",
                            if key == "from" { "output" } else { "input" }
                        ));
                    }
                    Ok((step.to_string(), name.trim().to_string()))
                };
                let (from_step, output) = ends("from")?;
                let (to_step, input) = ends("to")?;
                let pos = |id: &str| steps.iter().position(|s| s.id == id).unwrap_or(0);
                if from_step == to_step {
                    return Err(format!("{at}links the step {from_step} to itself"));
                }
                if pos(&from_step) > pos(&to_step) {
                    return Err(format!(
                        "{at}from {from_step} comes after to {to_step}: a link runs forward through the steps"
                    ));
                }
                if links
                    .iter()
                    .any(|k| k.to_step == to_step && k.input == input)
                {
                    return Err(format!("{at}{to_step}.{input} is fed by two links"));
                }
                links.push(Link {
                    from_step,
                    output,
                    to_step,
                    input,
                });
            }
        }
        _ => return Err("links is a list of {from: <step>.<output>, to: <step>.<input>}".into()),
    }
    Ok(Workflow {
        name,
        title,
        what,
        replaces,
        steps,
        links,
        document,
    })
}

/// Read a folder's `nils.workflow.yml` (or the file itself).
pub fn load_dir(path: &std::path::Path) -> Result<Workflow, String> {
    let file = if path.is_dir() {
        path.join(FILE)
    } else {
        path.to_path_buf()
    };
    let text_ = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    parse(&text_).map_err(|e| format!("{}: {e}", file.display()))
}

impl Workflow {
    /// The digest of the document: sha256 of its canonical JSON.
    pub fn digest(&self) -> String {
        crate::sha256(crate::canonical(&self.document).as_bytes())
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// The links into a step, by its input.
    pub fn links_into(&self, step: &str) -> BTreeMap<String, &Link> {
        self.links
            .iter()
            .filter(|l| l.to_step == step)
            .map(|l| (l.input.clone(), l))
            .collect()
    }

    /// The links out of a step.
    pub fn links_from(&self, step: &str) -> Vec<&Link> {
        self.links.iter().filter(|l| l.from_step == step).collect()
    }

    /// The step a step's typed inputs or roles are fed from, each once, in
    /// step order: what the run waits for.
    pub fn fed_by(&self, step: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in &self.links {
            if l.to_step == step && !out.contains(&l.from_step) {
                out.push(l.from_step.clone());
            }
        }
        out
    }

    /// As the doors answer it: `{name, title, what, replaces, steps, links}`.
    pub fn doc(&self) -> Value {
        serde_json::json!({
            "name": self.name, "title": self.title, "what": self.what,
            "replaces": self.replaces.as_ref().map(|r| serde_json::json!({"versions": r.versions, "why": r.why})),
            "steps": self.steps.iter().map(|s| serde_json::json!({
                "id": s.id, "of": s.of, "name": s.name, "version": s.version,
                "reference": s.reference(), "goal": s.goal, "knobs": s.knobs, "switches": s.switches,
            })).collect::<Vec<_>>(),
            "links": self.links.iter().map(|l| serde_json::json!({
                "from": format!("{}.{}", l.from_step, l.output), "to": format!("{}.{}", l.to_step, l.input),
            })).collect::<Vec<_>>(),
        })
    }
}

/// One step as the catalog answers it for a plan: its pipeline's descriptor
/// and pack where the catalog holds them, every member's descriptor where
/// the step is a family (Wave 7b section 7), else why the step is refused
/// (a family the catalog does not hold, a pipeline it does not hold).
#[derive(Debug, Clone)]
pub struct Member<'a> {
    pub step: &'a Step,
    /// `name@version` as resolved, `family <name>` for a family, or the
    /// step's reference where refused.
    pub label: String,
    /// The descriptors the step resolves to: a pipeline's one, or each
    /// member's where the step is a family, whose typed outputs are the
    /// sources of the links out of it and whose roles and typed inputs the
    /// links into it feed. Empty where the step is refused.
    pub descriptors: Vec<&'a Descriptor>,
    /// The pipeline's pack; a family has none of its own (its members'
    /// packs judge each member's run).
    pub pack: Option<&'a PipelinePack>,
    pub refusal: Option<String>,
}

impl<'a> Member<'a> {
    /// The one descriptor of a pipeline step, or a family's first member's.
    fn descriptor(&self) -> Option<&'a Descriptor> {
        self.descriptors.first().copied()
    }

    /// The output a link reads from the step: declared by its pipeline, or
    /// by the family's members, which must type it alike. `Err` says what
    /// the step declares instead, or which members type it differently.
    fn output(&self, id: &str) -> Result<&'a Output, String> {
        let declared: Vec<(&Descriptor, &Output)> = self
            .descriptors
            .iter()
            .filter_map(|d| d.outputs.iter().find(|o| o.id == id).map(|o| (*d, o)))
            .collect();
        let Some((first_of, first)) = declared.first() else {
            let ids: BTreeSet<&str> = self
                .descriptors
                .iter()
                .flat_map(|d| d.outputs.iter().map(|o| o.id.as_str()))
                .collect();
            return Err(format!(
                "declares no output {id}; it declares {}",
                ids.into_iter().collect::<Vec<_>>().join(", ")
            ));
        };
        if let Some((d, o)) = declared.iter().find(|(_, o)| o.typing() != first.typing()) {
            return Err(format!(
                "the members type the output {id} differently: {} declares {}, {} {}",
                first_of.name,
                type_words(&first.typing()),
                d.name,
                type_words(&o.typing())
            ));
        }
        Ok(first)
    }

    /// The typed input of this id, of the pipeline or of the first member
    /// declaring it.
    fn input(&self, id: &str) -> Option<&'a TypedInput> {
        self.descriptors
            .iter()
            .find_map(|d| d.inputs.iter().find(|t| t.id == id))
    }

    fn has_role(&self, role: &str) -> bool {
        self.descriptors
            .iter()
            .any(|d| d.roles.iter().any(|r| r == role))
    }

    /// The roles the step reads, each once, in the order declared.
    fn roles(&self) -> Vec<&'a str> {
        let mut out: Vec<&str> = Vec::new();
        for d in &self.descriptors {
            for r in &d.roles {
                if !out.contains(&r.as_str()) {
                    out.push(r);
                }
            }
        }
        out
    }

    /// The typed inputs the step takes, each id once.
    fn inputs(&self) -> Vec<&'a TypedInput> {
        let mut out: Vec<&TypedInput> = Vec::new();
        for d in &self.descriptors {
            for t in &d.inputs {
                if !out.iter().any(|x| x.id == t.id) {
                    out.push(t);
                }
            }
        }
        out
    }
}

/// One link's verdict (section 8.3, G12).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    /// `<step>.<output> -> <step>.<input>`.
    pub link: String,
    pub from: Value,
    pub to: Value,
    /// One of [`VERDICTS`].
    pub verdict: &'static str,
    pub words: String,
    /// True where the plan raised it over the steps' order or an input no
    /// link feeds, not from a link the document wrote.
    pub implicit: bool,
}

fn type_words(t: &Value) -> String {
    let mut w = Vec::new();
    if let Some(c) = t["contrast"].as_str() {
        w.push(c.to_string());
    }
    if let Some(k) = t["kind"].as_str() {
        w.push(k.to_string());
    }
    if let Some(r) = t["role"].as_str() {
        w.push(format!("of the {r}"));
    }
    if let Some(g) = t["grid"].as_str() {
        w.push(format!("on the {g} grid"));
    }
    if let Some(sp) = t["space"].as_str() {
        w.push(format!("in {sp}"));
    }
    if w.is_empty() {
        "an untyped output".into()
    } else {
        format!("a {}", w.join(" "))
    }
}

/// Whether two derivative kinds meet: `output` is the untyped image of
/// before, which an `image` meets either way.
fn kinds_meet(out: &str, takes: &str) -> bool {
    out == takes || matches!((out, takes), ("image", "output") | ("output", "image"))
}

/// Whether a declared pair meets: both declared and different is a
/// refusal; either undeclared meets.
fn declared_differ(a: Option<&str>, b: Option<&str>) -> Option<(String, String)> {
    match (a, b) {
        (Some(a), Some(b)) if !a.eq_ignore_ascii_case(b) => Some((a.to_string(), b.to_string())),
        _ => None,
    }
}

/// A pack's needs that read `aligned(a, b)`, each as (role of the need, the
/// expression's text, the words, whether hard).
fn alignment_needs(pack: &PipelinePack) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    for (role, needs) in &pack.needs {
        for (n, hard) in needs
            .hard
            .iter()
            .map(|n| (n, true))
            .chain(needs.soft.iter().map(|n| (n, false)))
        {
            if n.text.contains("aligned(") {
                out.push((role.clone(), n.text.clone(), n.words.clone(), hard));
            }
        }
    }
    out
}

/// The roles an `aligned(a, b)` expression names, in order.
fn aligned_roles(text_: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text_;
    while let Some(i) = rest.find("aligned(") {
        let after = &rest[i + "aligned(".len()..];
        if let Some(j) = after.find(')') {
            for r in after[..j].split(',') {
                let r = r.trim();
                if !r.is_empty() && !out.contains(&r.to_string()) {
                    out.push(r.to_string());
                }
            }
            rest = &after[j..];
        } else {
            break;
        }
    }
    out
}

/// Whether a pack checks the alignment of its inputs by their headers
/// (SAMSEG's and segcsvd's brain-mask Dice): a check whose metric names an
/// alignment dice.
fn header_alignment_check(pack: &PipelinePack) -> Option<&crate::pack::Check> {
    pack.checks.iter().find(|c| {
        c.metric.contains("dice") && (c.metric.contains("flair") || c.metric.contains("align"))
    })
}

/// What a step's key will carry of a step it is fed by: the upstream's
/// label and its breaking knobs at their defaults, as the pack's rules may
/// still set them per session.
fn carries(m: &Member<'_>) -> String {
    let mut words = m.label.clone();
    if let (Some(d), Some(p)) = (m.descriptor(), m.pack) {
        let defaults = d.resolve(&[]).unwrap_or_default();
        let knobs: Vec<String> = p
            .breaking_knobs()
            .iter()
            .map(|k| {
                let v = m
                    .step
                    .switches
                    .get(k)
                    .or_else(|| defaults.get(k))
                    .cloned()
                    .unwrap_or(Value::Null);
                format!(
                    "{k} {}",
                    match &v {
                        Value::String(s) => s.clone(),
                        Value::Null => "unset".into(),
                        other => other.to_string(),
                    }
                )
            })
            .collect();
        if !knobs.is_empty() {
            words.push_str(&format!(
                " ({}; the rules may set them per session)",
                knobs.join(", ")
            ));
        }
    }
    words
}

/// Plan the links of a workflow against the catalog's members (section 8.3,
/// G12): each link the document wrote gets its verdict, and the plan adds
/// what the steps' order and the inputs no link feeds imply, marked
/// `implicit`. Nothing is run and nothing is read of the registry. A link
/// between steps of different grains (a stack step into a session step, or
/// the reverse) needs no verdict of its own: the chain feeds a unit by the
/// stacks it holds, which the registry keeps for every grain (a stack unit
/// is its stack; a session's or a subject's units run the bids layout,
/// whose release places the stacks, since the loader refuses session or
/// subject units over the stacks layout).
pub fn plan_links(w: &Workflow, members: &[Member<'_>]) -> Vec<Verdict> {
    let member = |id: &str| members.iter().find(|m| m.step.id == id);
    let mut out: Vec<Verdict> = Vec::new();
    for l in &w.links {
        let (Some(from), Some(to)) = (member(&l.from_step), member(&l.to_step)) else {
            continue;
        };
        let link = l.words();
        let mut from_doc = json!({"step": l.from_step, "output": l.output, "pipeline": from.label});
        let mut to_doc = json!({"step": l.to_step, "input": l.input, "pipeline": to.label});
        let refused = |words: String, from_doc: Value, to_doc: Value| Verdict {
            link: link.clone(),
            from: from_doc,
            to: to_doc,
            verdict: "refused",
            words,
            implicit: false,
        };
        if let Some(r) = &from.refusal {
            out.push(refused(
                format!("{} is refused: {r}", l.from_step),
                from_doc,
                to_doc,
            ));
            continue;
        }
        if let Some(r) = &to.refusal {
            out.push(refused(
                format!("{} is refused: {r}", l.to_step),
                from_doc,
                to_doc,
            ));
            continue;
        }
        if from.descriptors.is_empty() || to.descriptors.is_empty() {
            continue;
        }
        let output = match from.output(&l.output) {
            Ok(o) => o,
            Err(e) => {
                out.push(refused(
                    format!("{} ({}) {e}", l.from_step, from.label),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
        };
        let out_type = output.typing();
        from_doc["type"] = out_type.clone();
        // the preprocessing the step before provides, and what the step
        // after says of it
        let redundancy = from.pack.and_then(|fp| {
            to.pack.and_then(|tp| {
                fp.links.provides.iter().find_map(|x| {
                    tp.links
                        .redundant
                        .get(x)
                        .map(|w| (false, w.clone()))
                        .or_else(|| tp.links.harmful.get(x).map(|w| (true, w.clone())))
                })
            })
        });
        if let Some(input) = to.input(&l.input) {
            let takes = input.ty.trim_start_matches("derivative:");
            let in_type = json!({
                "kind": takes, "role": input.role, "contrast": input.contrast, "grid": input.grid,
            });
            to_doc["type"] = in_type.clone();
            if !input.ty.starts_with("derivative:") {
                out.push(refused(
                    format!(
                        "{} into {}.{}, which takes a {}, not a derivative",
                        type_words(&out_type),
                        l.to_step,
                        l.input,
                        input.ty
                    ),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
            if !kinds_meet(&output.kind, takes) {
                out.push(refused(
                    format!(
                        "{} into {}.{}, which takes {}: the kinds differ",
                        type_words(&out_type),
                        l.to_step,
                        l.input,
                        type_words(&in_type)
                    ),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
            if let Some((a, b)) = declared_differ(output.role.as_deref(), input.role.as_deref())
                .or_else(|| declared_differ(output.contrast.as_deref(), input.contrast.as_deref()))
                .or_else(|| declared_differ(output.grid.as_deref(), input.grid.as_deref()))
            {
                out.push(refused(
                    format!(
                        "{} into {}.{}, which takes {}: {a} does not meet {b}",
                        type_words(&out_type),
                        l.to_step,
                        l.input,
                        type_words(&in_type)
                    ),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
            if let Some((harm, words)) = &redundancy {
                out.push(Verdict {
                    link,
                    from: from_doc,
                    to: to_doc,
                    verdict: "redundant",
                    words: if *harm {
                        format!("possibly harmful: {words}")
                    } else {
                        words.clone()
                    },
                    implicit: false,
                });
                continue;
            }
            // a need the typed input meets once fed: aligned(a, b) where the
            // input stands for a on b's grid (the registration's aligned
            // FLAIR into SAMSEG's aligned_flair, Wave 7b section 7)
            let meets = to.pack.and_then(|tp| {
                alignment_needs(tp).into_iter().find(|(_, text_, _, _)| {
                    let roles = aligned_roles(text_);
                    input
                        .role
                        .as_deref()
                        .is_some_and(|r| roles.iter().any(|x| x == r))
                        && input
                            .grid
                            .as_deref()
                            .is_none_or(|g| roles.iter().any(|x| x == g))
                })
            });
            let mut words = format!(
                "{}.{} is fed by {}: the {} key carries {}",
                l.to_step,
                l.input,
                l.from_step,
                to.label,
                carries(from)
            );
            if let Some((_, text_, need_words, _)) = meets {
                words.push_str(&format!(
                    "; {} meets {}'s {}: {need_words}",
                    type_words(&out_type),
                    to.label,
                    text_.trim()
                ));
            }
            out.push(Verdict {
                link,
                from: from_doc,
                to: to_doc,
                verdict: "fed",
                words,
                implicit: false,
            });
            continue;
        }
        if to.has_role(&l.input) {
            to_doc["type"] = json!({"kind": "image", "role": l.input});
            if !matches!(output.kind.as_str(), "image" | "output") {
                out.push(refused(
                    format!(
                        "{} into the {} role of {}: a role's scan is an image",
                        type_words(&out_type),
                        l.input,
                        l.to_step
                    ),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
            if let Some(r) = &output.role
                && r != &l.input
            {
                out.push(refused(
                    format!(
                        "{} into the {} role of {}: {r} does not meet {}",
                        type_words(&out_type),
                        l.input,
                        l.to_step,
                        l.input
                    ),
                    from_doc,
                    to_doc,
                ));
                continue;
            }
            if let Some((harm, words)) = &redundancy {
                out.push(Verdict {
                    link,
                    from: from_doc,
                    to: to_doc,
                    verdict: "redundant",
                    words: if *harm {
                        format!("possibly harmful: {words}")
                    } else {
                        words.clone()
                    },
                    implicit: false,
                });
                continue;
            }
            // a need the output meets: aligned(a, b) where the output stands
            // for a on b's grid
            let met = to.pack.and_then(|tp| {
                alignment_needs(tp).into_iter().find(|(_, text_, _, _)| {
                    let roles = aligned_roles(text_);
                    roles.contains(&l.input)
                        && output.grid.as_ref().is_none_or(|g| roles.contains(g))
                })
            });
            let words = match met {
                Some((_, text_, need_words, _)) => format!(
                    "{} meets {}'s {}: {need_words}",
                    type_words(&out_type),
                    to.label,
                    text_.trim()
                ),
                None => format!(
                    "{} reads its {} from {}, {}",
                    to.label,
                    l.input,
                    l.from_step,
                    type_words(&out_type)
                ),
            };
            out.push(Verdict {
                link,
                from: from_doc,
                to: to_doc,
                verdict: "satisfied",
                words,
                implicit: false,
            });
            continue;
        }
        let roles = to.roles();
        let inputs = to.inputs();
        out.push(refused(
            format!(
                "{} ({}) has no input {}: its roles are {} and its typed inputs {}",
                l.to_step,
                to.label,
                l.input,
                if roles.is_empty() {
                    "none".into()
                } else {
                    roles.join(", ")
                },
                if inputs.is_empty() {
                    "none".into()
                } else {
                    inputs
                        .iter()
                        .map(|t| t.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            from_doc,
            to_doc,
        ));
    }
    // what the steps' order implies: a step that provides a preprocessing
    // before one that does it itself or is harmed by it, with no link
    for (i, from) in members.iter().enumerate() {
        let Some(fp) = from.pack else { continue };
        for to in &members[i + 1..] {
            let Some(tp) = to.pack else { continue };
            if w.links
                .iter()
                .any(|l| l.from_step == from.step.id && l.to_step == to.step.id)
            {
                continue;
            }
            for x in &fp.links.provides {
                let found = tp
                    .links
                    .redundant
                    .get(x)
                    .map(|w| (false, w))
                    .or_else(|| tp.links.harmful.get(x).map(|w| (true, w)));
                if let Some((harm, words)) = found {
                    out.push(Verdict {
                        link: format!("{} -> {}", from.step.id, to.step.id),
                        from: json!({"step": from.step.id, "pipeline": from.label, "provides": x}),
                        to: json!({"step": to.step.id, "pipeline": to.label}),
                        verdict: "redundant",
                        words: if harm {
                            format!("possibly harmful: {words}")
                        } else {
                            words.clone()
                        },
                        implicit: true,
                    });
                }
            }
        }
    }
    // what no link feeds: an alignment need met by no step, a typed input
    // fed by no step
    for m in members {
        if m.descriptors.is_empty() {
            continue;
        }
        let into = w.links_into(&m.step.id);
        if let Some(p) = m.pack {
            for (role, text_, need_words, hard) in alignment_needs(p) {
                let roles = aligned_roles(&text_);
                let moving = roles.first().cloned().unwrap_or(role.clone());
                // met by a link into the moving role, or into a typed input
                // that stands for it on the other's grid (aligned_flair)
                let met = roles.iter().any(|r| into.contains_key(r))
                    || into.keys().any(|i| {
                        m.input(i).is_some_and(|t| {
                            t.role.as_deref() == Some(moving.as_str())
                                && t.grid
                                    .as_deref()
                                    .is_none_or(|g| roles.iter().any(|x| x == g))
                        })
                    });
                if met {
                    continue;
                }
                if !m.has_role(&moving) {
                    continue;
                }
                let link = format!("(no step) -> {}.{moving}", m.step.id);
                match header_alignment_check(p) {
                    Some(c) => out.push(Verdict {
                        link,
                        from: Value::Null,
                        to: json!({"step": m.step.id, "input": moving, "pipeline": m.label, "need": text_.trim()}),
                        verdict: "missing",
                        words: format!(
                            "no registration step before {}: {} is met by nothing, so the header alignment is trusted; accepted only with its header-alignment check on, which it is ({} {} {}: {})",
                            m.label,
                            text_.trim(),
                            c.metric,
                            c.op,
                            c.value.map(crate::descriptor::number_text).unwrap_or_default(),
                            c.words.clone().unwrap_or_else(|| need_words.clone())
                        ),
                        implicit: true,
                    }),
                    None => out.push(Verdict {
                        link,
                        from: Value::Null,
                        to: json!({"step": m.step.id, "input": moving, "pipeline": m.label, "need": text_.trim()}),
                        verdict: if hard { "refused" } else { "missing" },
                        words: format!(
                            "no registration step before {} and no header-alignment check: {} is met by nothing ({need_words})",
                            m.label,
                            text_.trim()
                        ),
                        implicit: true,
                    }),
                }
            }
        }
        for t in m
            .inputs()
            .into_iter()
            .filter(|t| t.ty.starts_with("derivative:"))
        {
            if into.contains_key(&t.id) {
                continue;
            }
            let kind = t.ty.trim_start_matches("derivative:");
            out.push(Verdict {
                link: format!("(no step) -> {}.{}", m.step.id, t.id),
                from: Value::Null,
                to: json!({"step": m.step.id, "input": t.id, "pipeline": m.label, "type": {"kind": kind, "role": t.role, "contrast": t.contrast, "grid": t.grid}}),
                verdict: "missing",
                words: if t.optional {
                    format!(
                        "{}.{} is fed by no step: where a session has a kept {kind} the run takes the newest, else it runs without one",
                        m.step.id, t.id
                    )
                } else {
                    format!(
                        "{}.{} is fed by no step: the run takes the newest kept {kind} of each session and skips a session with none",
                        m.step.id, t.id
                    )
                },
                implicit: true,
            });
        }
    }
    out
}

/// The verdicts counted, `{satisfied, missing, redundant, refused, fed}`.
pub fn verdict_counts(verdicts: &[Verdict]) -> Value {
    let count = |v: &str| verdicts.iter().filter(|x| x.verdict == v).count();
    json!({
        "satisfied": count("satisfied"), "missing": count("missing"), "redundant": count("redundant"),
        "refused": count("refused"), "fed": count("fed"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "nils_workflow: 1\nname: two\nsteps:\n  - {id: a, pipeline: synthseg}\n  - {id: b, pipeline: segcsvd, goal: standard, switches: {x: 1}}\nlinks:\n  - {from: a.labels, to: b.synthseg}\n";

    #[test]
    fn a_workflow_parses_and_its_digest_is_of_the_document() {
        let w = parse(GOOD).unwrap();
        assert_eq!(w.steps.len(), 2);
        assert_eq!(w.links[0].words(), "a.labels -> b.synthseg");
        assert_eq!(w.steps[1].goal.as_deref(), Some("standard"));
        assert!(w.digest().starts_with("sha256:"));
        assert_eq!(w.fed_by("b"), vec!["a".to_string()]);
        assert_eq!(w.title, "two");
    }

    #[test]
    fn the_loader_says_what_is_wrong() {
        let cases = [
            (
                "nils_workflow: 2\nname: x\nsteps: [{id: a, pipeline: p}]\n",
                "nils_workflow 2",
            ),
            (
                "nils_workflow: 1\nname: X\nsteps: [{id: a, pipeline: p}]\n",
                "name X",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: []\n",
                "at least one step",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p, family: f}]\n",
                "not both",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a}]\n",
                "names a pipeline or a family",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p}, {id: a, pipeline: q}]\n",
                "used twice",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p, version: latest}]\n",
                "current or a version number",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p, goal: quick}]\n",
                "goal is one of",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p}, {id: b, pipeline: q}]\nlinks: [{from: b.o, to: a.i}]\n",
                "runs forward",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p}, {id: b, pipeline: q}]\nlinks: [{from: a.o, to: c.i}]\n",
                "names the step c",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p}, {id: b, pipeline: q}]\nlinks: [{from: a, to: b.i}]\n",
                "<step>.<output>",
            ),
            (
                "nils_workflow: 1\nname: x\nsteps: [{id: a, pipeline: p}, {id: b, pipeline: q}]\nlinks: [{from: a.o, to: b.i}, {from: a.p, to: b.i}]\n",
                "fed by two links",
            ),
        ];
        for (text_, expect) in cases {
            let e = parse(text_).unwrap_err();
            assert!(e.contains(expect), "{text_}: {e}");
        }
    }
}
