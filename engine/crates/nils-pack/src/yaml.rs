// SPDX-License-Identifier: AGPL-3.0-only

//! Reading a pack's YAML into a generic value, and the small accessors the
//! loader uses. Every accessor takes the path it is at, so a refusal says
//! where.

use crate::error::{Error, R};
use serde_json::Value;
use std::path::Path;

/// One pack file: its text (for the line of a refusal) and its value.
pub struct File {
    pub path: std::path::PathBuf,
    pub source: String,
    pub value: Value,
}

impl File {
    pub fn read(path: &Path) -> R<File> {
        let source = std::fs::read_to_string(path).map_err(|e| Error {
            file: Some(path.to_path_buf()),
            line: None,
            path: String::new(),
            message: format!("cannot be read: {e}"),
        })?;
        let value: Value = serde_saphyr::from_str(&source).map_err(|e| Error {
            file: Some(path.to_path_buf()),
            line: None,
            path: String::new(),
            message: format!("is not YAML: {e}"),
        })?;
        Ok(File {
            path: path.to_path_buf(),
            source,
            value,
        })
    }

    /// Attach this file to a refusal raised while reading it.
    pub fn blame<T>(&self, r: R<T>) -> R<T> {
        r.map_err(|e| e.in_file(&self.path, Some(&self.source)))
    }
}

pub fn obj<'a>(v: &'a Value, at: &str) -> R<&'a serde_json::Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| Error::at(at, "expected a mapping"))
}

pub fn arr<'a>(v: &'a Value, at: &str) -> R<&'a Vec<Value>> {
    v.as_array().ok_or_else(|| Error::at(at, "expected a list"))
}

pub fn get<'a>(m: &'a serde_json::Map<String, Value>, key: &str, at: &str) -> R<&'a Value> {
    m.get(key)
        .ok_or_else(|| Error::at(at, format!("needs {key}")))
}

pub fn text(v: &Value, at: &str) -> R<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(Error::at(at, "expected a string")),
    }
}

pub fn number(v: &Value, at: &str) -> R<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| Error::at(at, "expected a number"))
}

/// A string, or a list of them. A bucket reference is resolved by the caller,
/// which is why this does not know about buckets.
pub fn texts(v: &Value, at: &str) -> R<Vec<String>> {
    match v {
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| text(x, &format!("{at}[{i}]")))
            .collect(),
        _ => Ok(vec![text(v, at)?]),
    }
}
