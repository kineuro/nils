// SPDX-License-Identifier: AGPL-3.0-only

//! The ask of NILS (`docs/specs/wave4b-the-ask.md`): the question as a
//! document, its desugar, its validation, its schemas and its hash. Nothing
//! here touches a registry; the catalog is reached through
//! [`validate::Names`], and the compiler and the executor are the next
//! slices.

pub mod affordance;
pub mod ast;
pub mod compile;
pub mod describe;
pub mod diagnose;
pub mod diff;
pub mod document;
pub mod exec;
pub mod handle;
pub mod hash;
pub mod measure;
pub mod moves;
pub mod promote;
pub mod repair;
pub mod run;
pub mod schema;
pub mod selection;
pub mod sugar;
pub mod validate;
pub mod values;

pub use ast::{Ask, Clause, Grain, Set, Src};
pub use hash::content_hash;
pub use repair::Repair;
pub use sugar::{SugarError, desugar};
pub use validate::{Code, Issue, Names, Scope, Validated, validate};

use serde_json::Value;

#[derive(Debug)]
pub enum Error {
    /// The text is neither JSON nor YAML.
    Parse(String),
    /// The text reads as JSON or YAML and is not an ask: where in the
    /// document its reading stopped, as the dotted path an issue of the
    /// validator names (`sets.good.attach[0]`, `document` for the whole),
    /// and why.
    Shape {
        path: String,
        message: String,
    },
    Sugar(SugarError),
    Invalid(Vec<Issue>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(m) => write!(f, "the ask will not parse: {m}"),
            // the same words as before the path was known: the path is told
            // beside them, by `Error::path`
            Error::Shape { message, .. } => write!(f, "the ask will not parse: {message}"),
            Error::Sugar(e) => write!(f, "the ask will not desugar: {e}"),
            Error::Invalid(issues) => {
                write!(f, "the ask is refused:")?;
                for i in issues {
                    write!(f, "\n  {i}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Where in the document a parse stopped, when the text read as JSON or
    /// YAML: the path a caller changes the document at.
    pub fn path(&self) -> Option<&str> {
        match self {
            Error::Shape { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// Read a document as JSON, else as YAML, into a JSON value.
///
/// YAML is read under the 1.2 core schema's booleans: only `true` and
/// `false` are booleans, and a plain `n`, `y`, `yes`, `no`, `on` or `off`
/// is the text it says (kineuro/nils#98). A person, and a small model,
/// names a count `n` more often than anything else, and under the 1.1
/// rules `{of: n}` named no binding at all.
pub fn read(text: &str) -> Result<Value, Error> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(text).map_err(|e| Error::Parse(e.to_string()));
    }
    let options = serde_saphyr::options! { strict_booleans: true };
    serde_saphyr::from_str_with_options::<Value>(text, options)
        .map_err(|e| Error::Parse(e.to_string()))
}

/// What a refusal of a key inside `pick` adds: a pick only orders a set's
/// rows, and the main scans NILS picked are a condition on a stack. A
/// small model once wrote `pick: {model: main}` eight times (2026-10-11).
pub const PICK_IS_NOT_THE_MAIN_SCANS: &str = "pick keeps the first rows of a set by an order (per, by, n, ties); \
the main scans NILS picked are a stack set's where: [[\"picked\", {role: t1w}]], one role per clause";

/// Where a reading stopped, as the dotted path an issue of the validator
/// names: a key after a dot, a place in a list in brackets, `document` for
/// the whole. An unknown key is told at the part that holds it, which is
/// the part to change, so the key itself is left off the path.
fn path_of(path: &serde_path_to_error::Path, message: &str) -> String {
    use serde_path_to_error::Segment;
    let mut segments: Vec<&Segment> = path.iter().collect();
    if let Some(Segment::Map { key }) = segments.last()
        && message.starts_with(&format!("unknown field `{key}`"))
    {
        segments.pop();
    }
    let mut out = String::new();
    for s in segments {
        match s {
            Segment::Seq { index } => out.push_str(&format!("[{index}]")),
            Segment::Map { key } | Segment::Enum { variant: key } => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Segment::Unknown => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push('?');
            }
        }
    }
    if out.is_empty() {
        out.push_str("document");
    }
    out
}

/// The ask a value holds, or where its reading stopped and why, with the
/// next step where the mistake is known. A YAML document and a JSON one are
/// both read from the value, so both are told the same path. A clause is
/// read whole, so a fault inside one is told at the clause.
fn ask_of(v: Value) -> Result<Ask, Error> {
    serde_path_to_error::deserialize(v).map_err(|e| {
        let mut message = e.inner().to_string();
        let path = path_of(e.path(), &message);
        if message.contains("expected one of `per`, `by`, `n`, `ties`") {
            message = format!("{message}; {PICK_IS_NOT_THE_MAIN_SCANS}");
        }
        Error::Shape { path, message }
    })
}

/// A document parsed strictly: no repair, unknown keys refused.
pub fn parse(text: &str) -> Result<Ask, Error> {
    ask_of(read(text)?)
}

/// A document parsed after structural repair, with what was repaired.
pub fn parse_repaired(text: &str) -> Result<(Ask, Vec<Repair>), Error> {
    let mut v = read(text)?;
    let repairs = repair::repair(&mut v);
    Ok((ask_of(v)?, repairs))
}

/// What a door or a runner holds once a document is accepted: the
/// desugared ask, its content hash, what was pinned, and the warnings.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub ask: Ask,
    pub hash: String,
    /// The hash of what it selects, its parameters' values bound in
    /// ([`hash::bound_hash`]): a saved selection's.
    pub bound_hash: String,
    pub pinned: Vec<(String, String, u64)>,
    pub validated: Validated,
}

/// Desugar, pin, validate, hash: the pipeline's first four steps (§11.1).
pub fn prepare(mut ask: Ask, names: &dyn Names, scope: &Scope) -> Result<Prepared, Error> {
    desugar(&mut ask).map_err(Error::Sugar)?;
    let pinned = validate::pin_selections(&mut ask, names).map_err(|i| Error::Invalid(vec![i]))?;
    let validated = validate(&ask, names, scope).map_err(Error::Invalid)?;
    let hash = hash::content_hash_under(&ask, &names.locale());
    let bound_hash = hash::bound_hash_under(&ask, &names.locale());
    Ok(Prepared {
        ask,
        hash,
        bound_hash,
        pinned,
        validated,
    })
}

/// The YAML rendering of a document, for a person.
pub fn to_yaml(ask: &Ask) -> Result<String, Error> {
    let v = serde_json::to_value(ask).map_err(|e| Error::Parse(e.to_string()))?;
    serde_saphyr::to_string(&v).map_err(|e| Error::Parse(e.to_string()))
}
