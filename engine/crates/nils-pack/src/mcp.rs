// SPDX-License-Identifier: AGPL-3.0-only

//! The model facing content of the MCP door (`docs/specs/wave4b-the-ask.md`,
//! §12.3): which doors are opted in as tools, what each tool says about
//! itself, the grounding rules every call carries, and the few-shot
//! documents an assistant is shown. It ships in the pack, versioned like
//! axes and picks, so a model behaviour change does not cut an engine
//! release. The tool list is not the endpoint list: a door reaches a model
//! only when a pack names it here.

use crate::error::{Error, R};
use crate::yaml::{self, File};

/// One tool the pack opts in, by the operation it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The operation, as the engine knows it: `catalog`, `validate`,
    /// `explain`, `options`, `apply`, `diagnose`, `preview`, `describe`,
    /// `run`, `handle`, `rows`, `selections`.
    pub operation: String,
    /// The name the model sees.
    pub name: String,
    /// What the model is told the tool does.
    pub description: String,
    /// Rules for this tool alone, after the grounding rules.
    pub rules: Vec<String>,
}

/// One worked example: a question in words and the document that answers
/// it. Never a real document: the gallery is built against the synthetic
/// registry, or it is not built.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub question: String,
    pub document: String,
    /// Why this document and not another.
    pub note: String,
}

/// Everything the pack says to a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    /// The version of this content, printed beside the pack's own so a
    /// deployment can see a model change that cut no engine release.
    pub version: String,
    /// Rules carried on every tool listing.
    pub grounding: Vec<String>,
    pub tools: Vec<Tool>,
    pub examples: Vec<Example>,
}

impl Model {
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// Every operation a pack may opt in. A name outside this list is refused
/// by the loader: a pack cannot invent a door.
pub const OPERATIONS: &[&str] = &[
    "catalog",
    "validate",
    "explain",
    "options",
    "apply",
    "diagnose",
    "preview",
    "describe",
    "run",
    "handle",
    "rows",
    "selections",
];

pub fn load(f: &File) -> R<Model> {
    // The file says what it is: `mcp:` at the root, then the content.
    let root = f.blame(yaml::obj(&f.value, "mcp"))?;
    let m = f.blame(yaml::obj(yaml::get(root, "mcp", "mcp")?, "mcp"))?;
    let version = match m.get("version") {
        Some(v) => f.blame(yaml::text(v, "version"))?,
        None => String::new(),
    };
    let grounding = match m.get("grounding") {
        Some(v) => f.blame(yaml::texts(v, "grounding"))?,
        None => Vec::new(),
    };
    let mut tools = Vec::new();
    for (i, v) in f
        .blame(yaml::arr(yaml::get(m, "tools", "mcp")?, "tools"))?
        .iter()
        .enumerate()
    {
        let at = format!("tools[{i}]");
        let t = f.blame(yaml::obj(v, &at))?;
        let operation = f.blame(yaml::text(yaml::get(t, "operation", &at)?, "operation"))?;
        if !OPERATIONS.contains(&operation.as_str()) {
            return Err(Error::at(
                format!("{at}.operation"),
                format!(
                    "{operation} is not an operation the door serves; those are {}",
                    OPERATIONS.join(", ")
                ),
            )
            .in_file(&f.path, Some(&f.source)));
        }
        let name = match t.get("name") {
            Some(v) => f.blame(yaml::text(v, "name"))?,
            None => format!("nils_{operation}"),
        };
        if tools.iter().any(|x: &Tool| x.name == name) {
            return Err(
                Error::at(format!("{at}.name"), format!("{name} is opted in twice"))
                    .in_file(&f.path, Some(&f.source)),
            );
        }
        let description = f.blame(yaml::text(yaml::get(t, "description", &at)?, "description"))?;
        let rules = match t.get("rules") {
            Some(v) => f.blame(yaml::texts(v, "rules"))?,
            None => Vec::new(),
        };
        tools.push(Tool {
            operation,
            name,
            description,
            rules,
        });
    }
    let mut examples = Vec::new();
    for (i, v) in m
        .get("examples")
        .map(|v| f.blame(yaml::arr(v, "examples")))
        .transpose()?
        .into_iter()
        .flatten()
        .enumerate()
    {
        let at = format!("examples[{i}]");
        let e = f.blame(yaml::obj(v, &at))?;
        examples.push(Example {
            question: f.blame(yaml::text(yaml::get(e, "question", &at)?, "question"))?,
            document: f.blame(yaml::text(yaml::get(e, "document", &at)?, "document"))?,
            note: match e.get("note") {
                Some(v) => f.blame(yaml::text(v, "note"))?,
                None => String::new(),
            },
        });
    }
    Ok(Model {
        version,
        grounding,
        tools,
        examples,
    })
}
