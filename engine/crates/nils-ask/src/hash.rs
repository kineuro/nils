// SPDX-License-Identifier: AGPL-3.0-only

//! The content hash of an ask (§4.4, rule 13): BLAKE2b over the desugared
//! core, canonical JSON, with parameters unbound and options sorted. The
//! declarations of scalar parameters are part of the core (their names and
//! types are what the question means); their values are not.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use serde_json::Value;

use crate::ast::{Ask, canonical_json};

/// The core of a desugared ask, as a JSON value with parameter values
/// removed and every object's keys sorted.
pub fn core(ask: &Ask) -> Value {
    let mut v = serde_json::to_value(ask).expect("an ask serializes");
    if let Some(Value::Object(params)) = v.get_mut("params") {
        for (_, decl) in params.iter_mut() {
            if let Value::Object(d) = decl {
                d.remove("value");
                d.remove("description");
            }
        }
    }
    if let Value::Object(top) = &mut v {
        top.remove("name");
    }
    v
}

/// The hash, hex, 64 characters.
pub fn content_hash(ask: &Ask) -> String {
    let text = canonical_json(&core(ask));
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}
