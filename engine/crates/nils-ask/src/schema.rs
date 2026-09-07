// SPDX-License-Identifier: AGPL-3.0-only

//! The two JSON Schemas of the ask (§4.5): the one generated from the Rust
//! types, with a description on every node, and the hand tightened one for
//! guided decoding, every property listed, `additionalProperties: false`
//! wherever an object is a struct, and the required lists kept.

use blake2::digest::consts::U16;
use blake2::{Blake2b, Digest};
use serde_json::Value;

use crate::ast::{Ask, canonical_json};

/// The generated schema.
pub fn generated() -> Value {
    let schema = schemars::schema_for!(Ask);
    serde_json::to_value(schema).expect("a schema serializes")
}

/// The tightened schema: the generated one with `additionalProperties:
/// false` on every object schema that lists properties and does not say
/// otherwise, and with the op enums of the fixed function table on the
/// clause's first slot.
pub fn tightened() -> Value {
    let mut v = generated();
    tighten(&mut v);
    if let Some(defs) = v.get_mut("$defs").and_then(Value::as_object_mut)
        && let Some(clause) = defs.get_mut("Clause")
        && let Some(prefix) = clause.get_mut("prefixItems").and_then(Value::as_array_mut)
        && let Some(op) = prefix.first_mut()
    {
        op["enum"] = Value::Array(
            OPS.iter()
                .map(|o| Value::String((*o).to_string()))
                .collect(),
        );
    }
    v
}

/// The digest of the tightened schema, which `capabilities.ask` carries and
/// the app checks before it runs (§12.3).
pub fn digest() -> String {
    let mut hasher = Blake2b::<U16>::new();
    hasher.update(canonical_json(&tightened()).as_bytes());
    hex::encode(hasher.finalize())
}

/// Every op a clause may start with (§4.3).
pub const OPS: &[&str] = &[
    "field",
    "axis",
    "derived",
    "param",
    "+",
    "-",
    "*",
    "/",
    "=",
    "<>",
    ">",
    ">=",
    "<",
    "<=",
    "~=",
    "in",
    "not_in",
    "has",
    "not_null",
    "is_null",
    "contains",
    "starts_with",
    "and",
    "or",
    "not",
    "picked",
    "abs",
    "round",
    "coalesce",
    "case",
    "concat",
    "days_between",
    "shift",
    "age_at",
    "bucket",
    "part",
    "ordinal",
    "prev",
    "next",
    "count",
    "distinct",
    "min",
    "max",
    "sum",
    "avg",
    "list",
    "change",
    "share",
];

fn tighten(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let is_struct = m.get("type").and_then(Value::as_str) == Some("object")
                && m.contains_key("properties")
                && !m.contains_key("additionalProperties");
            if is_struct {
                m.insert("additionalProperties".into(), Value::Bool(false));
            }
            for (_, child) in m.iter_mut() {
                tighten(child);
            }
        }
        Value::Array(items) => {
            for i in items {
                tighten(i);
            }
        }
        _ => {}
    }
}
