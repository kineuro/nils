// SPDX-License-Identifier: AGPL-3.0-only

//! `nils.job.yml`, as `contracts/job/v1` fixes it (record 43 S1).
//!
//! v0's descriptor is kept: a Boutiques 0.5 subset with an `x-nils` block.
//! What changes is that the rules are checked where v0 only warned: the image
//! is pinned by its registry manifest digest or the descriptor is refused
//! (R3, and v0's own comment that a config id is not a digest), a parameter is
//! typed and ranged, the analysis level agrees with the input layout, and
//! every output is a derivative kind with a path template the runner can find
//! a unit's files by. A key the contract does not name is kept, so a later
//! version can add one; a key it names is checked.

use serde_json::{Map, Value};

/// The work units (`x-nils.analysis-level`).
pub const LEVELS: [&str; 3] = ["participant", "session", "stack"];

/// The input layouts (`x-nils.input.layout`).
pub const LAYOUTS: [&str; 2] = ["bids", "stacks"];

/// What a pipeline needs of a GPU (`x-nils.needs.gpu`).
pub const GPU: [&str; 3] = ["none", "optional", "required"];

/// The parameter types (the Boutiques inputs a runner takes).
pub const PARAM_TYPES: [&str; 3] = ["Number", "String", "Flag"];

/// The typed inputs beside the selection, `derivative:<kind>` besides.
pub const INPUT_TYPES: [&str; 2] = ["model", "label_set"];

/// The derivative kinds an output may be; the registry's own list.
pub const OUTPUT_KINDS: [&str; 4] = ["mask", "embedding", "pyramid", "output"];

/// The value-keys the engine fills, which no parameter may take.
pub const RESERVED_KEYS: [&str; 6] = [
    "[InputDataset]",
    "[OutputLocation]",
    "[AnalysisLevel]",
    "[ParticipantLabels]",
    "[Manifest]",
    "[Inputs]",
];

/// The command a descriptor without one runs: the BIDS-Apps shape (D9).
pub const BIDS_APP_COMMAND: &str =
    "[InputDataset] [OutputLocation] [AnalysisLevel] --participant_label [ParticipantLabels]";

/// The work unit of a pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Participant,
    Session,
    Stack,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Participant => "participant",
            Level::Session => "session",
            Level::Stack => "stack",
        }
    }

    fn parse(text: &str) -> Option<Level> {
        Some(match text {
            "participant" => Level::Participant,
            "session" => Level::Session,
            "stack" => Level::Stack,
            _ => return None,
        })
    }
}

/// How the selection reaches the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Bids,
    Stacks,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::Bids => "bids",
            Layout::Stacks => "stacks",
        }
    }
}

/// What a pipeline needs of a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    None,
    Optional,
    Required,
}

impl Gpu {
    pub fn name(self) -> &'static str {
        match self {
            Gpu::None => "none",
            Gpu::Optional => "optional",
            Gpu::Required => "required",
        }
    }
}

/// The image, by its registry manifest digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// As the descriptor writes it: `repository@sha256:<hex>`.
    pub reference: String,
    pub repository: String,
    /// `sha256:<hex>`.
    pub digest: String,
}

/// The type of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Number,
    String,
    Flag,
}

/// One declared parameter (a Boutiques input).
#[derive(Debug, Clone)]
pub struct Param {
    pub id: String,
    pub name: String,
    pub ty: ParamType,
    pub value_key: Option<String>,
    pub flag: Option<String>,
    pub default: Option<Value>,
    pub optional: bool,
    pub integer: bool,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub choices: Option<Vec<Value>>,
    pub unit: Option<String>,
}

/// One typed input beside the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedInput {
    pub id: String,
    /// `model`, `label_set` or `derivative:<kind>`.
    pub ty: String,
    pub optional: bool,
}

/// One declared output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub id: String,
    pub kind: String,
    pub template: String,
    pub media_type: Option<String>,
}

/// A descriptor, parsed and checked.
#[derive(Debug, Clone)]
pub struct Descriptor {
    pub name: String,
    pub tool_version: String,
    pub image: Image,
    pub params: Vec<Param>,
    pub command_line: Option<String>,
    pub level: Level,
    pub layout: Layout,
    pub inputs: Vec<TypedInput>,
    pub outputs: Vec<Output>,
    pub gpu: Gpu,
    /// The axes its results may propose values on.
    pub proposals: Vec<String>,
    /// The document as it parsed, kept whole.
    pub document: Value,
}

/// Parse a descriptor from its YAML (JSON is YAML) and check it.
pub fn parse(text: &str) -> Result<Descriptor, String> {
    let value: Value =
        serde_saphyr::from_str(text).map_err(|e| format!("not a YAML document: {e}"))?;
    from_value(value)
}

/// Whether an image reference is pinned by a registry manifest digest:
/// `repository@sha256:<64 lowercase hex>`. Answers the image, or the
/// sentence that says why it is refused.
pub fn pinned(reference: &str) -> Result<Image, String> {
    let refuse = || {
        format!(
            "the image {reference} is not pinned by its registry manifest digest: write it as \
             repository@sha256:<64 hex>, the digest a registry serves the manifest under \
             (docker buildx imagetools inspect, skopeo inspect, or RepoDigests after a pull). \
             A tag moves, and an image id is a config digest no registry can serve (record 43 R3)"
        )
    };
    let Some((repository, digest)) = reference.split_once('@') else {
        return Err(refuse());
    };
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(refuse());
    };
    let repo_ok = !repository.is_empty()
        && repository
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && repository.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '/' | ':' | '-')
        });
    let hex_ok = hex.len() == 64
        && hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !repo_ok || !hex_ok {
        return Err(refuse());
    }
    Ok(Image {
        reference: reference.to_string(),
        repository: repository.to_string(),
        digest: digest.to_string(),
    })
}

fn text<'a>(v: &'a Value, key: &str, at: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{at}{key} is required, as text"))
}

fn opt_text(v: &Value, key: &str, at: &str) -> Result<Option<String>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(format!("{at}{key} is text")),
    }
}

fn opt_bool(v: &Value, key: &str, at: &str) -> Result<Option<bool>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        _ => Err(format!("{at}{key} is true or false")),
    }
}

fn opt_number(v: &Value, key: &str, at: &str) -> Result<Option<f64>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(n.as_f64()),
        _ => Err(format!("{at}{key} is a number")),
    }
}

fn array<'a>(v: &'a Value, key: &str, at: &str) -> Result<&'a [Value], String> {
    match &v[key] {
        Value::Null => Ok(&[]),
        Value::Array(a) => Ok(a.as_slice()),
        _ => Err(format!("{at}{key} is a list")),
    }
}

fn is_ident(s: &str, lower: bool) -> bool {
    let mut chars = s.chars();
    let first_ok = chars.next().is_some_and(|c| {
        if lower {
            c.is_ascii_lowercase()
        } else {
            c.is_ascii_alphabetic() || c == '_'
        }
    });
    first_ok
        && s.chars().all(|c| {
            if lower {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || c == '_'
            }
        })
}

fn is_value_key(s: &str) -> bool {
    s.len() > 2
        && s.starts_with('[')
        && s.ends_with(']')
        && s[1..s.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Check a parsed descriptor and read it into its parts.
pub fn from_value(document: Value) -> Result<Descriptor, String> {
    if !document.is_object() {
        return Err("a descriptor is a mapping".into());
    }
    let d = &document;
    let name = text(d, "name", "")?.to_string();
    let name_ok = name.len() <= 63
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !name_ok {
        return Err(format!(
            "name {name} is lowercase letters, digits and dashes, starting with a letter or a digit, at most 63"
        ));
    }
    if OUTPUT_KINDS.contains(&name.as_str()) || name == "incoming" {
        return Err(format!(
            "name {name} is a word the derivatives folder keeps for itself"
        ));
    }
    match &d["schema-version"] {
        Value::String(s) if s == "0.5" => {}
        Value::Number(n) if n.as_f64() == Some(0.5) => {}
        other => {
            return Err(format!(
                "schema-version is \"0.5\", the Boutiques band a descriptor keeps, not {other}"
            ));
        }
    }
    let tool_version = match &d["tool-version"] {
        Value::String(s) if !s.trim().is_empty() => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("tool-version is required, as text".into()),
    };

    // the image, by its registry manifest digest (R3)
    let ci = &d["container-image"];
    if !ci.is_object() {
        return Err(
            "container-image is required: {type: docker, image: repository@sha256:<hex>}".into(),
        );
    }
    if !ci["container-hash"].is_null() {
        return Err(
            "container-image.container-hash is not a key: the image's own reference pins it, and a hash beside it poisons the run's provenance (v0 learned this)".into(),
        );
    }
    if let Some(key) = ci
        .as_object()
        .and_then(|m| m.keys().find(|k| *k != "type" && *k != "image"))
    {
        return Err(format!(
            "container-image.{key} is not a key; type and image are"
        ));
    }
    match ci["type"].as_str() {
        Some("docker") => {}
        other => {
            return Err(format!(
                "container-image.type is docker (an OCI image), not {}",
                other.unwrap_or("absent")
            ));
        }
    }
    let image = pinned(text(ci, "image", "container-image.")?)?;

    // the parameters
    let mut params: Vec<Param> = Vec::new();
    for (i, p) in array(d, "inputs", "")?.iter().enumerate() {
        let at = format!("inputs[{i}].");
        let id = text(p, "id", &at)?.to_string();
        if !is_ident(&id, false) {
            return Err(format!(
                "{at}id {id} is letters, digits and _, not starting with a digit"
            ));
        }
        if params.iter().any(|q| q.id == id) {
            return Err(format!("{at}id {id} is declared twice"));
        }
        let ty = match p["type"].as_str() {
            Some("Number") => ParamType::Number,
            Some("String") => ParamType::String,
            Some("Flag") => ParamType::Flag,
            Some("File") => {
                return Err(format!(
                    "{at}type File: a file reaches a pipeline as a typed input under x-nils.inputs, never as a parameter"
                ));
            }
            other => {
                return Err(format!(
                    "{at}type is one of {}, not {}",
                    PARAM_TYPES.join(", "),
                    other.unwrap_or("absent")
                ));
            }
        };
        let value_key = opt_text(p, "value-key", &at)?;
        if let Some(k) = &value_key {
            if !is_value_key(k) {
                return Err(format!(
                    "{at}value-key {k} is a word in brackets, like [DIM]"
                ));
            }
            if RESERVED_KEYS.contains(&k.as_str()) {
                return Err(format!("{at}value-key {k} is the engine's own"));
            }
            if params.iter().any(|q| q.value_key.as_deref() == Some(k)) {
                return Err(format!("{at}value-key {k} is taken by another parameter"));
            }
        }
        let choices = match &p["value-choices"] {
            Value::Null => None,
            Value::Array(a) if !a.is_empty() => Some(a.clone()),
            _ => return Err(format!("{at}value-choices is a list of one value or more")),
        };
        let param = Param {
            name: opt_text(p, "name", &at)?.unwrap_or_else(|| id.clone()),
            id,
            ty,
            value_key,
            flag: opt_text(p, "command-line-flag", &at)?,
            default: match &p["default-value"] {
                Value::Null => None,
                v => Some(v.clone()),
            },
            optional: opt_bool(p, "optional", &at)?.unwrap_or(false),
            integer: opt_bool(p, "integer", &at)?.unwrap_or(false),
            minimum: opt_number(p, "minimum", &at)?,
            maximum: opt_number(p, "maximum", &at)?,
            choices,
            unit: opt_text(p, "unit", &at)?,
        };
        if let Some(def) = &param.default {
            check_value(&param, def).map_err(|e| format!("{at}default-value: {e}"))?;
        }
        params.push(param);
    }
    let command_line = opt_text(d, "command-line", "")?;
    if let Some(cl) = &command_line {
        crate::words::split(cl).map_err(|e| format!("command-line: {e}"))?;
    }

    // the x-nils block
    let x = &d["x-nils"];
    if !x.is_object() {
        return Err(
            "x-nils is required: the analysis level, the input layout and the outputs".into(),
        );
    }
    if !x["contract"].is_null() && x["contract"].as_i64() != Some(1) {
        return Err(format!("x-nils.contract is 1, not {}", x["contract"]));
    }
    let level_text = text(x, "analysis-level", "x-nils.")?;
    let level = Level::parse(level_text).ok_or_else(|| {
        format!(
            "x-nils.analysis-level is one of {}, not {level_text}{}",
            LEVELS.join(", "),
            if level_text == "subject" {
                " (v0's subject is participant)"
            } else {
                ""
            }
        )
    })?;
    let layout = match x["input"]["layout"].as_str() {
        Some("bids") => Layout::Bids,
        Some("stacks") => Layout::Stacks,
        other => {
            return Err(format!(
                "x-nils.input.layout is one of {}, not {}",
                LAYOUTS.join(", "),
                other.unwrap_or("absent")
            ));
        }
    };
    match (layout, level) {
        (Layout::Bids, Level::Participant | Level::Session) | (Layout::Stacks, Level::Stack) => {}
        _ => {
            return Err(format!(
                "x-nils: a {} layout has {} units, not {}",
                layout.name(),
                if layout == Layout::Bids {
                    "participant or session"
                } else {
                    "stack"
                },
                level.name()
            ));
        }
    }
    let mut inputs: Vec<TypedInput> = Vec::new();
    for (i, t) in array(x, "inputs", "x-nils.")?.iter().enumerate() {
        let at = format!("x-nils.inputs[{i}].");
        let id = text(t, "id", &at)?.to_string();
        if !is_ident(&id, true) {
            return Err(format!("{at}id {id} is lowercase letters, digits and _"));
        }
        if inputs.iter().any(|q| q.id == id) {
            return Err(format!("{at}id {id} is declared twice"));
        }
        let ty = text(t, "type", &at)?.to_string();
        let known = INPUT_TYPES.contains(&ty.as_str())
            || ty
                .strip_prefix("derivative:")
                .is_some_and(|k| OUTPUT_KINDS.contains(&k));
        if !known {
            return Err(format!(
                "{at}type is model, label_set or derivative:<{}>, not {ty}",
                OUTPUT_KINDS.join("|")
            ));
        }
        inputs.push(TypedInput {
            id,
            ty,
            optional: opt_bool(t, "optional", &at)?.unwrap_or(false),
        });
    }
    if inputs.iter().filter(|t| t.ty == "label_set").count() > 1 {
        return Err("x-nils.inputs: a run reads one label set".into());
    }
    let mut outputs: Vec<Output> = Vec::new();
    for (i, o) in array(x, "outputs", "x-nils.")?.iter().enumerate() {
        let at = format!("x-nils.outputs[{i}].");
        let id = text(o, "id", &at)?.to_string();
        if !is_ident(&id, true) {
            return Err(format!("{at}id {id} is lowercase letters, digits and _"));
        }
        if outputs.iter().any(|q| q.id == id) {
            return Err(format!("{at}id {id} is declared twice"));
        }
        let kind = text(o, "kind", &at)?.to_string();
        if !OUTPUT_KINDS.contains(&kind.as_str()) {
            return Err(format!(
                "{at}kind is one of {}, not {kind}",
                OUTPUT_KINDS.join(", ")
            ));
        }
        let template = text(o, "path-template", &at)?.to_string();
        check_template(&template, level).map_err(|e| format!("{at}path-template: {e}"))?;
        outputs.push(Output {
            id,
            kind,
            template,
            media_type: opt_text(o, "media-type", &at)?,
        });
    }
    if outputs.is_empty() {
        return Err(
            "x-nils.outputs names at least one output, by derivative kind and path template".into(),
        );
    }
    let gpu = match &x["needs"]["gpu"] {
        Value::Null => Gpu::None,
        Value::String(s) if s == "none" => Gpu::None,
        Value::String(s) if s == "optional" => Gpu::Optional,
        Value::String(s) if s == "required" => Gpu::Required,
        // v0 wrote a boolean
        Value::Bool(false) => Gpu::None,
        Value::Bool(true) => Gpu::Required,
        other => {
            return Err(format!(
                "x-nils.needs.gpu is one of {}, not {other}",
                GPU.join(", ")
            ));
        }
    };
    let mut proposals: Vec<String> = Vec::new();
    for (i, p) in array(x, "proposals", "x-nils.")?.iter().enumerate() {
        let axis = text(p, "axis", &format!("x-nils.proposals[{i}]."))?;
        if !axis.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err(format!("x-nils.proposals[{i}].axis {axis} is a pack axis"));
        }
        proposals.push(axis.to_string());
    }
    Ok(Descriptor {
        name,
        tool_version,
        image,
        params,
        command_line,
        level,
        layout,
        inputs,
        outputs,
        gpu,
        proposals,
        document,
    })
}

/// A path template: relative, no `..`, the placeholders its level has, and
/// the unit named in it so a unit's files can be found without results.
fn check_template(template: &str, level: Level) -> Result<(), String> {
    if template.starts_with('/') || template.contains('\\') {
        return Err(format!("{template} is a path relative to /output"));
    }
    if template
        .split('/')
        .any(|s| s == ".." || s == "." || s.is_empty())
    {
        return Err(format!("{template} steps outside or has an empty segment"));
    }
    let mut rest = template;
    let mut seen: Vec<&str> = Vec::new();
    while let Some(open) = rest.find('{') {
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| format!("{template} opens a brace it does not close"))?;
        let word = &rest[open + 1..open + close];
        let allowed: &[&str] = match level {
            Level::Participant => &["subject"],
            Level::Session => &["subject", "session"],
            Level::Stack => &["stack"],
        };
        if !allowed.contains(&word) {
            return Err(format!(
                "{{{word}}} is not a placeholder of a {} unit; those are {}",
                level.name(),
                allowed
                    .iter()
                    .map(|w| format!("{{{w}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        seen.push(word);
        rest = &rest[open + close + 1..];
    }
    let needs: &[&str] = match level {
        Level::Participant => &["subject"],
        Level::Session => &["subject", "session"],
        Level::Stack => &["stack"],
    };
    for n in needs {
        if !seen.contains(n) {
            return Err(format!(
                "{template} does not name the unit: a {} unit's files are found by {{{n}}}",
                level.name()
            ));
        }
    }
    if template.contains("**") || template.contains('?') || template.contains('[') {
        return Err(format!(
            "{template}: * is the one wildcard, inside one segment"
        ));
    }
    Ok(())
}

/// Whether a value is one a parameter takes.
fn check_value(p: &Param, v: &Value) -> Result<(), String> {
    match p.ty {
        ParamType::Flag => {
            if !v.is_boolean() {
                return Err(format!("{} is a flag, true or false", p.id));
            }
        }
        ParamType::String => {
            if !v.is_string() {
                return Err(format!("{} is text", p.id));
            }
        }
        ParamType::Number => {
            let n = v.as_f64().ok_or_else(|| format!("{} is a number", p.id))?;
            if p.integer && n.fract() != 0.0 {
                return Err(format!("{} is a whole number, not {n}", p.id));
            }
            if let Some(min) = p.minimum
                && n < min
            {
                return Err(format!("{} is at least {min}, not {n}", p.id));
            }
            if let Some(max) = p.maximum
                && n > max
            {
                return Err(format!("{} is at most {max}, not {n}", p.id));
            }
        }
    }
    if let Some(choices) = &p.choices {
        let same = |c: &Value| match (c.as_f64(), v.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => c == v,
        };
        if !choices.iter().any(same) {
            return Err(format!(
                "{} is one of {}, not {v}",
                p.id,
                choices
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

impl Descriptor {
    /// The digest of the descriptor: sha256 of its canonical JSON, so the
    /// same descriptor written with its keys in another order is the same.
    pub fn digest(&self) -> String {
        crate::sha256(crate::canonical(&self.document).as_bytes())
    }

    /// Every declared parameter with its value: what was given as
    /// `id=value`, else the default, else null for an optional one. An id
    /// not declared, a value of the wrong type or range, and a required
    /// parameter with neither are refused. This is what a run records.
    pub fn resolve(&self, given: &[(String, String)]) -> Result<Map<String, Value>, String> {
        for (id, _) in given {
            if !self.params.iter().any(|p| &p.id == id) {
                let known: Vec<&str> = self.params.iter().map(|p| p.id.as_str()).collect();
                return Err(format!(
                    "{} has no parameter {id}; its parameters are {}",
                    self.name,
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
        }
        let mut out = Map::new();
        for p in &self.params {
            let asked: Vec<&String> = given
                .iter()
                .filter(|(id, _)| id == &p.id)
                .map(|(_, v)| v)
                .collect();
            if asked.len() > 1 {
                return Err(format!("{} is given twice", p.id));
            }
            let value = match asked.first() {
                Some(text) => {
                    let v = match p.ty {
                        ParamType::String => Value::String((*text).clone()),
                        ParamType::Flag => match text.as_str() {
                            "true" | "1" | "yes" => Value::Bool(true),
                            "false" | "0" | "no" => Value::Bool(false),
                            other => return Err(format!("{} is true or false, not {other}", p.id)),
                        },
                        ParamType::Number => {
                            let n: f64 = text
                                .trim()
                                .parse()
                                .map_err(|_| format!("{} is a number, not {text}", p.id))?;
                            number(n)
                        }
                    };
                    check_value(p, &v)?;
                    v
                }
                None => match &p.default {
                    Some(d) => d.clone(),
                    None if p.optional || p.ty == ParamType::Flag => Value::Null,
                    None => {
                        return Err(format!(
                            "{} needs --param {}=<value>: it is required and has no default",
                            self.name, p.id
                        ));
                    }
                },
            };
            out.insert(p.id.clone(), value);
        }
        Ok(out)
    }

    /// The command line a run's container is given, as words: the
    /// descriptor's, or the BIDS-Apps shape, with every value-key replaced.
    pub fn argv(
        &self,
        params: &Map<String, Value>,
        participants: &[String],
    ) -> Result<Vec<String>, String> {
        let line = self.command_line.as_deref().unwrap_or(BIDS_APP_COMMAND);
        let words = crate::words::split(line)?;
        let mut keys: Vec<(String, Vec<String>)> = vec![
            ("[InputDataset]".into(), vec!["/input".into()]),
            ("[OutputLocation]".into(), vec!["/output".into()]),
            // BIDS Apps know participant and group; a session unit is a
            // participant level run whose outputs are found per session
            ("[AnalysisLevel]".into(), vec!["participant".into()]),
            ("[ParticipantLabels]".into(), participants.to_vec()),
            ("[Manifest]".into(), vec!["/input/stacks.json".into()]),
            ("[Inputs]".into(), vec!["/inputs".into()]),
        ];
        for p in &self.params {
            let Some(key) = &p.value_key else { continue };
            let v = params.get(&p.id).cloned().unwrap_or(Value::Null);
            let expansion: Vec<String> = match (p.ty, &v) {
                (_, Value::Null) => Vec::new(),
                (ParamType::Flag, Value::Bool(true)) => vec![p.flag.clone().unwrap_or_default()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect(),
                (ParamType::Flag, _) => Vec::new(),
                (_, v) => {
                    let text = value_text(v);
                    match &p.flag {
                        Some(f) => vec![f.clone(), text],
                        None => vec![text],
                    }
                }
            };
            keys.push((key.clone(), expansion));
        }
        Ok(crate::words::substitute(&words, &keys))
    }
}

/// A number as JSON: an integer where it is whole, so `3` reads `3`.
fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// A value as the word a command line carries.
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => (f as i64).to_string(),
            _ => n.to_string(),
        },
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "59c45f54a1f1dc69134f63bec91a726e41c71c64a16cc21cda0b54526910a3c3";

    fn doc(image: &str) -> String {
        format!(
            r#"
name: n4
schema-version: "0.5"
tool-version: "2.6"
container-image:
  type: docker
  image: "{image}"
inputs:
  - id: dimension
    name: Dimensionality
    type: Number
    value-key: "[DIM]"
    default-value: 3
    integer: true
    minimum: 2
    maximum: 4
  - id: shrink
    name: Shrink
    type: Number
    value-key: "[SHRINK]"
    command-line-flag: "-s"
    optional: true
  - id: verbose
    name: Verbose
    type: Flag
    value-key: "[VERBOSE]"
    command-line-flag: "--verbose"
command-line: "bash -c 'N4 -d [DIM] -i [InputDataset]/x' [SHRINK] [VERBOSE] --labels [ParticipantLabels]"
x-nils:
  analysis-level: session
  input: {{layout: bids}}
  outputs:
    - id: n4
      kind: output
      path-template: "sub-{{subject}}/ses-{{session}}/anat/*_desc-n4_T1w.nii.gz"
  needs: {{gpu: optional}}
  proposals: [{{axis: body_part}}]
"#
        )
    }

    #[test]
    fn a_pinned_descriptor_parses_and_an_unpinned_one_is_refused() {
        let d = parse(&doc(&format!("docker.io/antsx/ants@sha256:{HEX}"))).unwrap();
        assert_eq!(d.name, "n4");
        assert_eq!(d.image.digest, format!("sha256:{HEX}"));
        assert_eq!(d.image.repository, "docker.io/antsx/ants");
        assert_eq!(d.level, Level::Session);
        assert_eq!(d.layout, Layout::Bids);
        assert_eq!(d.gpu, Gpu::Optional);
        assert_eq!(d.proposals, ["body_part"]);
        assert!(d.digest().starts_with("sha256:"));
        for bad in [
            "antsx/ants:latest".to_string(),
            "antsx/ants".to_string(),
            format!("antsx/ants@sha256:{}", &HEX[..60]),
            format!("antsx/ants@sha512:{HEX}"),
            format!("Antsx/ants@sha256:{HEX}"),
            format!("@sha256:{HEX}"),
            format!("antsx/ants@sha256:{}", HEX.to_uppercase()),
        ] {
            let e = parse(&doc(&bad)).unwrap_err();
            assert!(
                e.contains("not pinned by its registry manifest digest"),
                "{bad}: {e}"
            );
        }
        // v0's container-hash is refused, not ignored
        let with_hash = doc(&format!("antsx/ants@sha256:{HEX}")).replace(
            "  type: docker\n",
            "  type: docker\n  container-hash: sha256:abc\n",
        );
        assert!(parse(&with_hash).unwrap_err().contains("container-hash"));
    }

    #[test]
    fn parameters_are_typed_ranged_and_recorded_whole() {
        let d = parse(&doc(&format!("antsx/ants@sha256:{HEX}"))).unwrap();
        let all = d.resolve(&[]).unwrap();
        assert_eq!(
            serde_json::Value::Object(all.clone()),
            serde_json::json!({"dimension": 3, "shrink": null, "verbose": null})
        );
        let given = d
            .resolve(&[
                ("shrink".into(), "4".into()),
                ("verbose".into(), "true".into()),
            ])
            .unwrap();
        assert_eq!(given["shrink"], 4);
        assert_eq!(given["verbose"], true);
        assert!(
            d.resolve(&[("dimension".into(), "5".into())])
                .unwrap_err()
                .contains("at most 4")
        );
        assert!(
            d.resolve(&[("dimension".into(), "2.5".into())])
                .unwrap_err()
                .contains("whole")
        );
        assert!(
            d.resolve(&[("nope".into(), "1".into())])
                .unwrap_err()
                .contains("no parameter nope")
        );
        assert!(d.resolve(&[("verbose".into(), "maybe".into())]).is_err());

        let argv = d.argv(&given, &["a".into(), "b".into()]).unwrap();
        assert_eq!(
            argv,
            [
                "bash",
                "-c",
                "N4 -d 3 -i /input/x",
                "-s",
                "4",
                "--verbose",
                "--labels",
                "a",
                "b"
            ]
        );
        let argv = d.argv(&all, &["a".into()]).unwrap();
        assert_eq!(argv, ["bash", "-c", "N4 -d 3 -i /input/x", "--labels", "a"]);
    }

    #[test]
    fn the_level_the_layout_and_the_templates_agree() {
        let base = doc(&format!("antsx/ants@sha256:{HEX}"));
        let e =
            parse(&base.replace("analysis-level: session", "analysis-level: stack")).unwrap_err();
        assert!(
            e.contains("a bids layout has participant or session units"),
            "{e}"
        );
        let e =
            parse(&base.replace("analysis-level: session", "analysis-level: subject")).unwrap_err();
        assert!(e.contains("v0's subject is participant"), "{e}");
        let e = parse(&base.replace("ses-{session}/", "")).unwrap_err();
        assert!(e.contains("{session}"), "{e}");
        let e = parse(&base.replace("sub-{subject}", "sub-{stack}")).unwrap_err();
        assert!(e.contains("not a placeholder"), "{e}");
        let e = parse(&base.replace("sub-{subject}/", "../sub-{subject}/")).unwrap_err();
        assert!(e.contains("steps outside"), "{e}");
        let e = parse(&base.replace("kind: output", "kind: nifti")).unwrap_err();
        assert!(e.contains("kind is one of"), "{e}");
        let e = parse(&base.replace("value-key: \"[DIM]\"", "value-key: \"[InputDataset]\""))
            .unwrap_err();
        assert!(e.contains("the engine's own"), "{e}");
        let e = parse(&base.replace("name: n4\n", "name: mask\n")).unwrap_err();
        assert!(e.contains("keeps for itself"), "{e}");
        let e = parse(&base.replace("type: Flag", "type: File")).unwrap_err();
        assert!(e.contains("typed input"), "{e}");
    }

    #[test]
    fn a_descriptor_without_a_command_is_a_bids_app() {
        let base = doc(&format!("antsx/ants@sha256:{HEX}"));
        let start = base.find("command-line:").unwrap();
        let end = start + base[start..].find('\n').unwrap() + 1;
        let plain = format!("{}{}", &base[..start], &base[end..]);
        let d = parse(&plain).unwrap();
        let argv = d
            .argv(&d.resolve(&[]).unwrap(), &["01".into(), "02".into()])
            .unwrap();
        assert_eq!(
            argv,
            [
                "/input",
                "/output",
                "participant",
                "--participant_label",
                "01",
                "02"
            ]
        );
    }
}
