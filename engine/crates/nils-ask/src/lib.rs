// SPDX-License-Identifier: AGPL-3.0-only

//! The ask of NILS (`docs/specs/wave4b-the-ask.md`): the question as a
//! document, its desugar, its validation, its schemas and its hash. Nothing
//! here touches a registry; the catalog is reached through
//! [`validate::Names`], and the compiler and the executor are the next
//! slices.

pub mod ast;
pub mod compile;
pub mod exec;
pub mod hash;
pub mod repair;
pub mod schema;
pub mod sugar;
pub mod validate;

pub use ast::{Ask, Clause, Grain, Set, Src};
pub use hash::content_hash;
pub use repair::Repair;
pub use sugar::{SugarError, desugar};
pub use validate::{Code, Issue, Names, Scope, Validated, validate};

use serde_json::Value;

#[derive(Debug)]
pub enum Error {
    /// The text is neither JSON nor YAML of an ask.
    Parse(String),
    Sugar(SugarError),
    Invalid(Vec<Issue>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Parse(m) => write!(f, "the ask will not parse: {m}"),
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

/// Read a document as JSON, else as YAML, into a JSON value.
pub fn read(text: &str) -> Result<Value, Error> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        return serde_json::from_str(text).map_err(|e| Error::Parse(e.to_string()));
    }
    serde_saphyr::from_str::<Value>(text).map_err(|e| Error::Parse(e.to_string()))
}

/// A document parsed strictly: no repair, unknown keys refused.
pub fn parse(text: &str) -> Result<Ask, Error> {
    let v = read(text)?;
    serde_json::from_value(v).map_err(|e| Error::Parse(e.to_string()))
}

/// A document parsed after structural repair, with what was repaired.
pub fn parse_repaired(text: &str) -> Result<(Ask, Vec<Repair>), Error> {
    let mut v = read(text)?;
    let repairs = repair::repair(&mut v);
    let ask = serde_json::from_value(v).map_err(|e| Error::Parse(e.to_string()))?;
    Ok((ask, repairs))
}

/// What a door or a runner holds once a document is accepted: the
/// desugared ask, its content hash, what was pinned, and the warnings.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub ask: Ask,
    pub hash: String,
    pub pinned: Vec<(String, String, u64)>,
    pub validated: Validated,
}

/// Desugar, pin, validate, hash: the pipeline's first four steps (§11.1).
pub fn prepare(mut ask: Ask, names: &dyn Names, scope: &Scope) -> Result<Prepared, Error> {
    desugar(&mut ask).map_err(Error::Sugar)?;
    let pinned = validate::pin_selections(&mut ask, names).map_err(|i| Error::Invalid(vec![i]))?;
    let validated = validate(&ask, names, scope).map_err(Error::Invalid)?;
    let hash = content_hash(&ask);
    Ok(Prepared {
        ask,
        hash,
        pinned,
        validated,
    })
}

/// The YAML rendering of a document, for a person.
pub fn to_yaml(ask: &Ask) -> Result<String, Error> {
    let v = serde_json::to_value(ask).map_err(|e| Error::Parse(e.to_string()))?;
    serde_saphyr::to_string(&v).map_err(|e| Error::Parse(e.to_string()))
}
