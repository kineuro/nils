// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 4c §5.5: who is acting for the principal on this call. A door sets
//! it for the thread that handles a request, from the `X-Nils-Actor`
//! header it verified; a worker hands it to the verb it runs as `NILS_ACTOR`;
//! every writer that records provenance reads it here, so no call site has
//! to thread it through. An absent actor is its own value, because "no
//! actor" and "a person acting alone" must never be confused.
use std::cell::RefCell;

use serde_json::{Value, json};

/// The environment variable a worker sets for the verb it runs.
pub const VAR: &str = "NILS_ACTOR";

thread_local! {
    static CURRENT: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// The actor of the request this thread is handling.
pub fn set(actor: Value) {
    CURRENT.with(|c| *c.borrow_mut() = Some(actor));
}

/// The request is over.
pub fn clear() {
    CURRENT.with(|c| *c.borrow_mut() = None);
}

/// Nobody acts for the principal: a person alone, or a verb at a terminal.
pub fn absent() -> Value {
    json!({ "kind": "absent" })
}

/// The actor to record now: the thread's, else the worker's environment,
/// else absent.
pub fn current() -> Value {
    CURRENT
        .with(|c| c.borrow().clone())
        .or_else(|| {
            std::env::var(VAR)
                .ok()
                .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                .filter(Value::is_object)
        })
        .unwrap_or_else(absent)
}
