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

/// How a run's units meet their containers (`x-nils.units`, record 49 A1):
/// all in one container, or each unit in a container of its own, so the
/// lane runs them side by side within its budget.
pub const UNITS: [&str; 2] = ["together", "apart"];

/// What a unit is taken to need where the descriptor says nothing: one core
/// and 2 GB of memory; a GPU unit 8 GB of the card's memory.
pub const DEFAULT_CORES: u32 = 1;
pub const DEFAULT_MEMORY_GB: f64 = 2.0;
pub const DEFAULT_GPU_MEMORY_GB: f64 = 8.0;

/// The container paths a secret may not be mounted at or under: the
/// runner's own.
pub const RUNNER_PATHS: [&str; 4] = ["/input", "/inputs", "/output", "/source"];

/// The parameter types (the Boutiques inputs a runner takes).
pub const PARAM_TYPES: [&str; 3] = ["Number", "String", "Flag"];

/// The typed inputs beside the selection, `derivative:<kind>` besides.
pub const INPUT_TYPES: [&str; 2] = ["model", "label_set"];

/// The derivative kinds an output may be declared as; each is one of the
/// registry's. `model` is a run-level output only (record 43): a model the
/// run fitted, which the runner registers from its card. `table` (record 49
/// A3) is a file of numbers with declared columns, one row per unit, whose
/// rows the runner loads for the ask. The registry's `seeds` is the
/// runner's own, written from the results, never declared.
pub const OUTPUT_KINDS: [&str; 6] = ["mask", "embedding", "pyramid", "output", "model", "table"];

/// The file formats a table output may be written in (record 49 A3).
pub const TABLE_FORMATS: [&str; 3] = ["csv", "tsv", "json"];

/// The types of a table's columns.
pub const COLUMN_TYPES: [&str; 3] = ["number", "integer", "text"];

/// The comparisons a declared check may make (`x-nils.qc`, record 49 A3).
pub const CHECK_OPS: [&str; 4] = [">=", "<=", ">", "<"];

/// The column names a table may not take: the ask's own word for a run.
pub const RESERVED_COLUMNS: [&str; 1] = ["run"];

/// Where an output belongs (`x-nils.outputs[].level`).
pub const OUTPUT_LEVELS: [&str; 2] = ["unit", "run"];

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

/// How a run's units meet their containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    /// One container for the whole run (record 43, and what a descriptor
    /// that says nothing gets): a pipeline that fits a model over every
    /// unit, or loops over them itself.
    Together,
    /// A container per unit, each seeing only its own unit's input, so the
    /// lane runs several at once and a run that stopped resumes with the
    /// units it had not finished (record 49 A1).
    Apart,
}

impl Units {
    pub fn name(self) -> &'static str {
        match self {
            Units::Together => "together",
            Units::Apart => "apart",
        }
    }
}

/// What a scheduling unit needs (`x-nils.needs`): a unit where units run
/// apart, the whole run where they run together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Needs {
    pub cores: u32,
    pub memory_gb: f64,
    /// Of the card's memory, for a unit that takes the GPU.
    pub gpu_memory_gb: f64,
}

/// A secret input (`x-nils.secrets`, record 49 R3): a file the site keeps,
/// such as a licence, that the engine reads at run time and mounts
/// read-only into this pipeline's containers alone, never into an output, a
/// log, the run's record or its results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    pub id: String,
    /// Where the container sees the file.
    pub mount: String,
    /// An environment variable that names `mount`, such as FS_LICENSE.
    pub env: Option<String>,
    pub optional: bool,
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
    /// One file for the whole run rather than one per unit (record 43).
    pub run_level: bool,
    /// For a model output, where its card lies under `/output`.
    pub card: Option<String>,
    /// For an embedding output, the encoders whose embeddings it may make,
    /// by weight digest; the runner takes an embedding by no other, unless
    /// the run was given that encoder (record 43 second review).
    pub encoders: Vec<String>,
    /// For a table output (record 49 A3): its format and typed columns.
    pub table: Option<Table>,
}

/// The type of a table's column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Number,
    Integer,
    Text,
}

impl ColumnType {
    pub fn name(self) -> &'static str {
        match self {
            ColumnType::Number => "number",
            ColumnType::Integer => "integer",
            ColumnType::Text => "text",
        }
    }

    /// Whether its values are numbers.
    pub fn numeric(self) -> bool {
        self != ColumnType::Text
    }
}

/// One declared column of a table output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The measure's name: what the ask calls it, under the pipeline.
    pub name: String,
    /// The header (csv, tsv) or key (json) the value is read from; absent,
    /// the header whose folded form is the name (lowercase, every run of
    /// other characters one `_`), so `left hippocampus` is
    /// `left_hippocampus`.
    pub from: Option<String>,
    pub ty: ColumnType,
    pub unit: Option<String>,
    pub description: Option<String>,
}

/// A table output: a file of one row per unit, its columns declared and
/// typed (record 49 A3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// `csv`, `tsv` or `json` (one object, or a list of objects).
    pub format: String,
    pub columns: Vec<Column>,
    /// For a run-level table, the column that names each row's unit, as
    /// `/inputs/manifest.json` names it or with the unit as its prefix
    /// (`sub-01_ses-a_T1w` is unit `sub-01_ses-a`).
    pub unit_column: Option<String>,
}

/// A declared check on a unit's metric (`x-nils.qc`, record 49 A3): a unit
/// whose value breaks it is one `pipeline:qc` review item naming the
/// metric and the value.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// A metric the unit's results carry, or a column of one of its tables.
    pub metric: String,
    /// `>=`, `<=`, `>` or `<`: what a good value is.
    pub op: String,
    pub value: f64,
    pub description: Option<String>,
}

impl Check {
    /// Whether a value keeps the check.
    pub fn holds(&self, v: f64) -> bool {
        match self.op.as_str() {
            ">=" => v >= self.value,
            "<=" => v <= self.value,
            ">" => v > self.value,
            "<" => v < self.value,
            _ => false,
        }
    }

    /// The check as a person writes it: `snr >= 8`.
    pub fn text(&self) -> String {
        format!("{} {} {}", self.metric, self.op, number_text(self.value))
    }
}

/// A number as a person writes it: `8`, not `8.0`.
pub fn number_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A header folded to a column name: lowercase, every run of characters
/// other than letters and digits one `_`, none at either end.
pub fn fold(header: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in header.trim().chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
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
    /// The cores, memory and card memory a scheduling unit needs.
    pub needs: Needs,
    /// Whether units run in one container or each in its own.
    pub units: Units,
    /// The secret inputs it reads.
    pub secrets: Vec<Secret>,
    /// The axes its results may propose values on.
    pub proposals: Vec<String>,
    /// Its declared checks (record 49 A3).
    pub checks: Vec<Check>,
    /// For a bids input, the pick roles each unit needs (`x-nils.input.roles`,
    /// record 49 A3): a session without a pick of one is named by the
    /// pre-flight, with the role it lacks.
    pub roles: Vec<String>,
    /// Typical minutes a unit takes on the CPU (`x-nils.needs.unit-minutes`),
    /// which the pre-flight estimates from until the pipeline has run here.
    pub unit_minutes: Option<f64>,
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
        let run_level = match opt_text(o, "level", &at)?.as_deref() {
            None | Some("unit") => false,
            Some("run") => true,
            Some(other) => {
                return Err(format!(
                    "{at}level is one of {}, not {other}",
                    OUTPUT_LEVELS.join(", ")
                ));
            }
        };
        let card = opt_text(o, "card", &at)?;
        if kind == "model" && !run_level {
            return Err(format!(
                "{at}a model output is one file for the whole run: level run"
            ));
        }
        if run_level {
            check_run_template(&template).map_err(|e| format!("{at}path-template: {e}"))?;
        } else {
            check_template(&template, level).map_err(|e| format!("{at}path-template: {e}"))?;
        }
        if kind == "model" {
            let Some(c) = &card else {
                return Err(format!(
                    "{at}a model output names its card (contracts/model/v1): card"
                ));
            };
            check_run_template(c).map_err(|e| format!("{at}card: {e}"))?;
            if c.contains('*') {
                return Err(format!("{at}card is one file, with no *"));
            }
        } else if card.is_some() {
            return Err(format!("{at}card belongs to a model output"));
        }
        let mut encoders = Vec::new();
        for (j, e) in array(o, "encoders", &at)?.iter().enumerate() {
            let d = e.as_str().unwrap_or("");
            let hex = d.strip_prefix("sha256:").unwrap_or("");
            if hex.len() != 64
                || !hex
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            {
                return Err(format!(
                    "{at}encoders[{j}] is an encoder's weight digest, sha256:<64 hex>"
                ));
            }
            encoders.push(d.to_string());
        }
        if !encoders.is_empty() && kind != "embedding" {
            return Err(format!("{at}encoders belong to an embedding output"));
        }
        let table = if kind == "table" {
            Some(table_of(o, &at, &template, run_level)?)
        } else {
            if let Some(key) = ["columns", "format", "unit-column"]
                .iter()
                .find(|k| !o[**k].is_null())
            {
                return Err(format!("{at}{key} belongs to a table output"));
            }
            None
        };
        outputs.push(Output {
            id,
            kind,
            template,
            media_type: opt_text(o, "media-type", &at)?,
            run_level,
            card,
            encoders,
            table,
        });
    }
    if outputs.is_empty() {
        return Err(
            "x-nils.outputs names at least one output, by derivative kind and path template".into(),
        );
    }
    // a measure is named once in a pipeline: the ask reads it by that name
    let mut named: Vec<&str> = Vec::new();
    for o in &outputs {
        for c in o.table.iter().flat_map(|t| &t.columns) {
            if named.contains(&c.name.as_str()) {
                return Err(format!(
                    "x-nils.outputs: the column {} is declared by two tables; the ask reads a measure by its name",
                    c.name
                ));
            }
            named.push(&c.name);
        }
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
    let n = &x["needs"];
    if !n.is_null() && !n.is_object() {
        return Err("x-nils.needs is a mapping: gpu, cores, memory-gb, gpu-memory-gb".into());
    }
    let cores = match &n["cores"] {
        Value::Null => DEFAULT_CORES,
        Value::Number(c) => match c.as_u64() {
            Some(c) if (1..=4096).contains(&c) => c as u32,
            _ => {
                return Err(format!(
                    "x-nils.needs.cores is a whole number from 1, not {c}"
                ));
            }
        },
        other => return Err(format!("x-nils.needs.cores is a whole number, not {other}")),
    };
    let positive = |key: &str, default: f64| -> Result<f64, String> {
        match opt_number(n, key, "x-nils.needs.")? {
            None => Ok(default),
            Some(v) if v > 0.0 && v.is_finite() => Ok(v),
            Some(v) => Err(format!("x-nils.needs.{key} is a number above 0, not {v}")),
        }
    };
    let needs = Needs {
        cores,
        memory_gb: positive("memory-gb", DEFAULT_MEMORY_GB)?,
        gpu_memory_gb: positive("gpu-memory-gb", DEFAULT_GPU_MEMORY_GB)?,
    };
    let units = match opt_text(x, "units", "x-nils.")?.as_deref() {
        None | Some("together") => Units::Together,
        Some("apart") => Units::Apart,
        Some(other) => {
            return Err(format!(
                "x-nils.units is one of {}, not {other}",
                UNITS.join(", ")
            ));
        }
    };
    if units == Units::Apart
        && let Some(o) = outputs.iter().find(|o| o.run_level)
    {
        return Err(format!(
            "x-nils.units apart runs each unit in a container of its own, and the output {} is the whole run's: a pipeline with a run-level output runs its units together",
            o.id
        ));
    }
    let mut secrets: Vec<Secret> = Vec::new();
    for (i, s) in array(x, "secrets", "x-nils.")?.iter().enumerate() {
        let at = format!("x-nils.secrets[{i}].");
        let id = text(s, "id", &at)?.to_string();
        if !is_ident(&id, true) {
            return Err(format!("{at}id {id} is lowercase letters, digits and _"));
        }
        if secrets.iter().any(|q| q.id == id) {
            return Err(format!("{at}id {id} is declared twice"));
        }
        let mount = opt_text(s, "mount", &at)?.unwrap_or_else(|| format!("/secrets/{id}"));
        let clean = mount.starts_with('/')
            && !mount.contains(':')
            && !mount.contains(',')
            && !mount.contains('\\')
            && mount[1..]
                .split('/')
                .all(|seg| !seg.is_empty() && seg != "." && seg != "..");
        if !clean {
            return Err(format!(
                "{at}mount {mount} is an absolute container path, with no empty, . or .. segment and no ':' or ','"
            ));
        }
        if let Some(own) = RUNNER_PATHS
            .iter()
            .find(|p| mount == **p || mount.starts_with(&format!("{p}/")))
        {
            return Err(format!(
                "{at}mount {mount} is under {own}, which the runner mounts itself"
            ));
        }
        let env = opt_text(s, "env", &at)?;
        if let Some(e) = &env
            && !(is_ident(e, false) && e.chars().all(|c| !c.is_ascii_lowercase()))
        {
            return Err(format!("{at}env {e} is an upper-case variable name"));
        }
        secrets.push(Secret {
            id,
            mount,
            env,
            optional: opt_bool(s, "optional", &at)?.unwrap_or(false),
        });
    }
    let mut proposals: Vec<String> = Vec::new();
    for (i, p) in array(x, "proposals", "x-nils.")?.iter().enumerate() {
        let axis = text(p, "axis", &format!("x-nils.proposals[{i}]."))?;
        if !axis.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err(format!("x-nils.proposals[{i}].axis {axis} is a pack axis"));
        }
        proposals.push(axis.to_string());
    }
    // record 49 A3: the declared checks, the roles a unit needs, and the
    // typical minutes a unit takes
    let mut checks: Vec<Check> = Vec::new();
    for (i, c) in array(x, "qc", "x-nils.")?.iter().enumerate() {
        let check = check_of(c).map_err(|e| format!("x-nils.qc[{i}]: {e}"))?;
        if checks
            .iter()
            .any(|k| k.metric == check.metric && k.op == check.op)
        {
            return Err(format!(
                "x-nils.qc[{i}]: {} {} is declared twice",
                check.metric, check.op
            ));
        }
        checks.push(check);
    }
    let mut roles: Vec<String> = Vec::new();
    for (i, r) in array(&x["input"], "roles", "x-nils.input.")?
        .iter()
        .enumerate()
    {
        let role = r.as_str().filter(|r| is_ident(r, true)).ok_or_else(|| {
            format!("x-nils.input.roles[{i}] is a pick role of the pack, such as t1w")
        })?;
        if !roles.iter().any(|q| q == role) {
            roles.push(role.to_string());
        }
    }
    if !roles.is_empty() && layout != Layout::Bids {
        return Err(
            "x-nils.input.roles are the picks a bids input carries; a stacks input has none".into(),
        );
    }
    let unit_minutes = match opt_number(&x["needs"], "unit-minutes", "x-nils.needs.")? {
        None => None,
        Some(m) if m > 0.0 && m.is_finite() => Some(m),
        Some(m) => return Err(format!("x-nils.needs.unit-minutes is above 0, not {m}")),
    };
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
        needs,
        units,
        secrets,
        proposals,
        checks,
        roles,
        unit_minutes,
        document,
    })
}

/// A table output's format and columns (record 49 A3).
fn table_of(o: &Value, at: &str, template: &str, run_level: bool) -> Result<Table, String> {
    let format = match opt_text(o, "format", at)? {
        Some(f) => f,
        None => {
            let lower = template.to_ascii_lowercase();
            TABLE_FORMATS
                .iter()
                .find(|f| lower.ends_with(&format!(".{f}")))
                .map(|f| f.to_string())
                .ok_or_else(|| {
                    format!(
                        "{at}format is one of {}; the path template's extension names none",
                        TABLE_FORMATS.join(", ")
                    )
                })?
        }
    };
    if !TABLE_FORMATS.contains(&format.as_str()) {
        return Err(format!(
            "{at}format is one of {}, not {format}",
            TABLE_FORMATS.join(", ")
        ));
    }
    let mut columns: Vec<Column> = Vec::new();
    for (j, c) in array(o, "columns", at)?.iter().enumerate() {
        let cat = format!("{at}columns[{j}].");
        let name = text(c, "name", &cat)?.to_string();
        if !is_ident(&name, true) {
            return Err(format!(
                "{cat}name {name} is lowercase letters, digits and _, starting with a letter"
            ));
        }
        if RESERVED_COLUMNS.contains(&name.as_str()) {
            return Err(format!(
                "{cat}name {name} is the ask's own word for the run a value came from"
            ));
        }
        if columns.iter().any(|q| q.name == name) {
            return Err(format!("{cat}name {name} is declared twice"));
        }
        let ty = match c["type"].as_str() {
            None | Some("number") => ColumnType::Number,
            Some("integer") => ColumnType::Integer,
            Some("text") => ColumnType::Text,
            Some(other) => {
                return Err(format!(
                    "{cat}type is one of {}, not {other}",
                    COLUMN_TYPES.join(", ")
                ));
            }
        };
        columns.push(Column {
            name,
            from: opt_text(c, "from", &cat)?,
            ty,
            unit: opt_text(c, "unit", &cat)?,
            description: opt_text(c, "description", &cat)?,
        });
    }
    if columns.is_empty() {
        return Err(format!(
            "{at}a table declares its columns: name, type (number, integer or text), unit"
        ));
    }
    let unit_column = opt_text(o, "unit-column", at)?;
    match (run_level, &unit_column) {
        (true, None) => {
            return Err(format!(
                "{at}a run's table names the column that says each row's unit: unit-column"
            ));
        }
        (false, Some(_)) => {
            return Err(format!(
                "{at}unit-column belongs to a run's table; a unit's table is that unit's rows"
            ));
        }
        _ => {}
    }
    Ok(Table {
        format,
        columns,
        unit_column,
    })
}

/// A declared check, as a mapping `{metric, op, value}` or as the text
/// `snr >= 8`.
fn check_of(v: &Value) -> Result<Check, String> {
    let (metric, op, value, description) = match v {
        Value::String(t) => {
            let words: Vec<&str> = t.split_whitespace().collect();
            let [metric, op, value] = words.as_slice() else {
                return Err(format!("{t} is written metric op value, such as snr >= 8"));
            };
            let value: f64 = value
                .parse()
                .map_err(|_| format!("{t}: {value} is not a number"))?;
            (metric.to_string(), op.to_string(), value, None)
        }
        Value::Object(_) => (
            text(v, "metric", "")?.to_string(),
            text(v, "op", "")?.to_string(),
            opt_number(v, "value", "")?.ok_or("value is required, as a number")?,
            opt_text(v, "description", "")?,
        ),
        _ => return Err("a check is {metric, op, value} or the text metric op value".into()),
    };
    if !is_ident(&metric, true) {
        return Err(format!(
            "the metric {metric} is lowercase letters, digits and _"
        ));
    }
    if !CHECK_OPS.contains(&op.as_str()) {
        return Err(format!(
            "the comparison is one of {}, not {op}",
            CHECK_OPS.join(", ")
        ));
    }
    if !value.is_finite() {
        return Err(format!("the value {value} is not a finite number"));
    }
    Ok(Check {
        metric,
        op,
        value,
        description,
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
    if wildcard_touches_a_word(template) {
        return Err(format!(
            "{template}: a * beside a unit's word would let one unit take another's file (stack 4 and 45.nii); put a character between them"
        ));
    }
    Ok(())
}

/// Whether a `*` stands right beside a `{word}` of a template.
pub(crate) fn wildcard_touches_a_word(template: &str) -> bool {
    template.contains("}*") || template.contains("*{")
}

/// A run-level template: relative, no `..`, and no unit placeholder, since
/// the file is the run's.
fn check_run_template(template: &str) -> Result<(), String> {
    if template.starts_with('/') || template.contains('\\') {
        return Err(format!("{template} is a path relative to /output"));
    }
    if template
        .split('/')
        .any(|s| s == ".." || s == "." || s.is_empty())
    {
        return Err(format!("{template} steps outside or has an empty segment"));
    }
    if template.contains('{') || template.contains('}') {
        return Err(format!(
            "{template} is the run's own file and names no unit: no placeholder"
        ));
    }
    if template.contains("**") || template.contains('?') || template.contains('[') {
        return Err(format!(
            "{template}: * is the one wildcard, inside one segment"
        ));
    }
    if template == crate::results::FILE || template == crate::results::SEEDS_FILE {
        return Err(format!("{template} is the runner's own name"));
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

    /// Record 43's ruling: an output may be the run's own file, such as a
    /// model a train entry point fitted, with its card beside it.
    #[test]
    fn a_run_level_output_names_no_unit_and_a_model_names_its_card() {
        let base = doc(&format!("antsx/ants@sha256:{HEX}"));
        let with = |extra: &str| {
            base.replace(
                "  needs: {gpu: optional}",
                &format!("{extra}  needs: {{gpu: optional}}"),
            )
        };
        let model = "    - id: head\n      kind: model\n      level: run\n      path-template: \"head/head.*\"\n      card: head/card.json\n";
        let d = parse(&with(model)).unwrap();
        let head = d.outputs.iter().find(|o| o.id == "head").unwrap();
        assert!(head.run_level);
        assert_eq!(head.card.as_deref(), Some("head/card.json"));
        assert!(!d.outputs[0].run_level);
        for (bad, words) in [
            (model.replace("level: run", "level: unit"), "level run"),
            (model.replace("      card: head/card.json\n", ""), "names its card"),
            (model.replace("head/head.*", "head/{subject}.json"), "no placeholder"),
            (model.replace("level: run", "level: study"), "level is one of"),
            (model.replace("head/head.*", "results.json"), "runner's own"),
            (
                "    - id: extra\n      kind: output\n      path-template: \"sub-{subject}/ses-{session}*.nii\"\n".to_string(),
                "beside a unit's word",
            ),
            (
                "    - id: extra\n      kind: output\n      path-template: \"sub-{subject}/ses-{session}/x\"\n      card: c.json\n".to_string(),
                "belongs to a model",
            ),
        ] {
            let e = parse(&with(&bad)).unwrap_err();
            assert!(e.contains(words), "{words}: {e}");
        }
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

    /// Record 49 A1 and A2: what a unit needs, whether units run apart, and
    /// the secrets a pipeline reads, each checked where it is declared.
    #[test]
    fn needs_units_and_secrets_are_declared_and_checked() {
        let base = doc(&format!("antsx/ants@sha256:{HEX}"));
        let d = parse(&base).unwrap();
        assert_eq!(d.units, Units::Together);
        assert_eq!(d.needs.cores, DEFAULT_CORES);
        assert_eq!(d.needs.memory_gb, DEFAULT_MEMORY_GB);
        assert!(d.secrets.is_empty());
        let with = |needs: &str, extra: &str| {
            base.replace(
                "  needs: {gpu: optional}",
                &format!("  needs: {needs}\n{extra}"),
            )
        };
        let d = parse(&with(
            "{gpu: required, cores: 4, memory-gb: 12, gpu-memory-gb: 6}",
            "  units: apart\n  secrets:\n    - id: freesurfer_license\n      mount: /opt/fs/license.txt\n      env: FS_LICENSE\n",
        ))
        .unwrap();
        assert_eq!(d.units, Units::Apart);
        assert_eq!(
            d.needs,
            Needs {
                cores: 4,
                memory_gb: 12.0,
                gpu_memory_gb: 6.0
            }
        );
        assert_eq!(
            d.secrets,
            [Secret {
                id: "freesurfer_license".into(),
                mount: "/opt/fs/license.txt".into(),
                env: Some("FS_LICENSE".into()),
                optional: false,
            }]
        );
        let d = parse(&with("{gpu: none}", "  secrets: [{id: key}]\n")).unwrap();
        assert_eq!(d.secrets[0].mount, "/secrets/key");
        for (needs, extra, words) in [
            ("{cores: 0}", "", "whole number from 1"),
            ("{cores: 1.5}", "", "whole number from 1"),
            ("{memory-gb: -2}", "", "above 0"),
            ("{gpu-memory-gb: 0}", "", "above 0"),
            (
                "{gpu: none}",
                "  units: sometimes\n",
                "x-nils.units is one of",
            ),
            ("{gpu: none}", "  secrets: [{id: Key}]\n", "lowercase"),
            (
                "{gpu: none}",
                "  secrets: [{id: a}, {id: a}]\n",
                "declared twice",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, mount: /output/lic}]\n",
                "runner mounts itself",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, mount: /inputs}]\n",
                "runner mounts itself",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, mount: relative}]\n",
                "absolute container path",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, mount: \"/a:b\"}]\n",
                "absolute container path",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, mount: /a/../b}]\n",
                "absolute container path",
            ),
            (
                "{gpu: none}",
                "  secrets: [{id: a, env: fs_license}]\n",
                "upper-case",
            ),
        ] {
            let e = parse(&with(needs, extra)).unwrap_err();
            assert!(e.contains(words), "{needs} {extra}: {e}");
        }
        // a run-level output is the whole run's: its units cannot run apart
        let model = "    - id: head\n      kind: model\n      level: run\n      path-template: \"head/head.*\"\n      card: head/card.json\n";
        let e = parse(&base.replace(
            "  needs: {gpu: optional}",
            &format!("{model}  needs: {{gpu: optional}}\n  units: apart"),
        ))
        .unwrap_err();
        assert!(e.contains("runs its units together"), "{e}");
    }

    /// Record 49 A3: a table output declares its format and typed columns,
    /// a check its metric, comparison and value, and a bids input the pick
    /// roles each unit needs.
    #[test]
    fn a_table_its_checks_and_the_roles_a_unit_needs_are_declared_and_checked() {
        let base = doc(&format!("antsx/ants@sha256:{HEX}"));
        let with = |extra: &str, x: &str| {
            let base = if x.contains("  input:") {
                base.replace("  input: {layout: bids}\n", "")
            } else {
                base.clone()
            };
            base.replace(
                "  needs: {gpu: optional}",
                &format!("{extra}  needs: {{gpu: optional, unit-minutes: 5}}\n{x}"),
            )
        };
        let table = "    - id: vols\n      kind: table\n      path-template: \"sub-{subject}/ses-{session}/vols.csv\"\n      columns:\n        - {name: total_intracranial, unit: mm3}\n        - {name: third_ventricle, from: 3rd ventricle}\n        - {name: site, type: text}\n";
        let d = parse(&with(
            table,
            "  qc: [\"total_intracranial >= 900000\", {metric: third_ventricle, op: \"<=\", value: 5000}]\n  input: {layout: bids, roles: [t1w, flair]}\n",
        ))
        .unwrap();
        let t = d.outputs[1].table.as_ref().unwrap();
        assert_eq!(t.format, "csv");
        assert_eq!(t.columns[0].ty, ColumnType::Number);
        assert_eq!(t.columns[1].from.as_deref(), Some("3rd ventricle"));
        assert_eq!(t.columns[2].ty, ColumnType::Text);
        assert_eq!(d.checks.len(), 2);
        assert_eq!(d.checks[0].text(), "total_intracranial >= 900000");
        assert!(d.checks[0].holds(1.0e6) && !d.checks[0].holds(8.0e5));
        assert!(d.checks[1].holds(5000.0) && !d.checks[1].holds(5000.5));
        assert_eq!(d.roles, ["t1w", "flair"]);
        assert_eq!(d.unit_minutes, Some(5.0));
        assert_eq!(fold(" Left-Hippocampus "), "left_hippocampus");
        assert_eq!(fold("putamen+pallidum"), "putamen_pallidum");
        for (extra, x, words) in [
            (table.replace("vols.csv", "vols.txt"), String::new(), "extension names none"),
            (table.replace("      columns:\n        - {name: total_intracranial, unit: mm3}\n        - {name: third_ventricle, from: 3rd ventricle}\n        - {name: site, type: text}\n", ""), String::new(), "declares its columns"),
            (table.replace("name: site, type: text", "name: run"), String::new(), "ask's own word"),
            (table.replace("type: text", "type: date"), String::new(), "type is one of"),
            (table.replace("kind: table", "kind: output"), String::new(), "belongs to a table output"),
            (format!("{table}      unit-column: bids_name\n"), String::new(), "belongs to a run's table"),
            (table.replace("      path-template: \"sub-{subject}/ses-{session}/vols.csv\"", "      level: run\n      path-template: all.csv"), String::new(), "unit-column"),
            (String::new(), "  qc: [\"snr => 8\"]\n".to_string(), "comparison is one of"),
            (String::new(), "  qc: [\"snr >= high\"]\n".to_string(), "not a number"),
            (String::new(), "  qc: [{metric: SNR, op: \">=\", value: 8}]\n".to_string(), "lowercase"),
            (String::new(), "  qc: [\"snr >= 8\", \"snr >= 9\"]\n".to_string(), "declared twice"),
            (String::new(), "  input: {layout: bids, roles: [T1w]}\n".to_string(), "pick role"),
        ] {
            let e = parse(&with(&extra, &x)).unwrap_err();
            assert!(e.contains(words), "{words}: {e}");
        }
        // two tables may not both name one measure
        let twice = format!(
            "{table}{}",
            table
                .replace("id: vols", "id: more")
                .replace("vols.csv", "more.csv")
        );
        assert!(
            parse(&with(&twice, ""))
                .unwrap_err()
                .contains("declared by two tables")
        );
    }
}
