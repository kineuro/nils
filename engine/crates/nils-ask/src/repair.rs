// SPDX-License-Identifier: AGPL-3.0-only

//! Structural repair for authored input (§4.4, rule 14): a missing `{}` in
//! a clause is inserted, a lone clause where a list belongs is wrapped, and
//! operator and unit aliases are mapped. It never rewrites a value and
//! never guesses among candidates. Every repair is reported with its path.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    pub path: String,
    pub what: String,
}

const OP_ALIASES: &[(&str, &str)] = &[
    ("==", "="),
    ("eq", "="),
    ("!=", "<>"),
    ("ne", "<>"),
    ("gt", ">"),
    ("gte", ">="),
    ("ge", ">="),
    ("lt", "<"),
    ("lte", "<="),
    ("le", "<="),
    ("approx", "~="),
    ("isnull", "is_null"),
    ("notnull", "not_null"),
    ("is_not_null", "not_null"),
    ("startswith", "starts_with"),
    ("param_ref", "param"),
];

const UNIT_ALIASES: &[(&str, &str)] = &[
    ("days", "day"),
    ("d", "day"),
    ("months", "month"),
    ("m", "month"),
    ("years", "year"),
    ("y", "year"),
];

/// The slots whose value is a list of clauses or orders.
const LIST_SLOTS: &[&str] = &["where", "columns", "by", "order"];

/// Repair a document in place and say what was done.
pub fn repair(doc: &mut Value) -> Vec<Repair> {
    let mut out = Vec::new();
    if let Some(sets) = doc.get_mut("sets").and_then(Value::as_object_mut) {
        for (name, set) in sets.iter_mut() {
            repair_set(set, &format!("sets.{name}"), &mut out);
        }
    }
    if let Some(stages) = doc.get_mut("pipeline").and_then(Value::as_array_mut) {
        for (i, stage) in stages.iter_mut().enumerate() {
            repair_set(stage, &format!("pipeline[{i}]"), &mut out);
        }
    }
    if let Some(o) = doc.get_mut("out").and_then(Value::as_object_mut) {
        repair_slots(o, "out", &mut out, true);
    }
    if let Some(params) = doc.get_mut("params").and_then(Value::as_object_mut) {
        for (name, decl) in params.iter_mut() {
            if let Some(v) = decl.get_mut("value") {
                repair_units(v, &format!("params.{name}.value"), &mut out);
            }
        }
    }
    out
}

fn repair_set(set: &mut Value, path: &str, out: &mut Vec<Repair>) {
    let Some(m) = set.as_object_mut() else {
        return;
    };
    repair_slots(m, path, out, true);
    for slot in ["near", "attach", "has", "same", "every"] {
        if let Some(v) = m.get_mut(slot) {
            // a lone relation object where a list belongs
            if v.is_object() {
                let one = v.take();
                *v = Value::Array(vec![one]);
                out.push(Repair {
                    path: format!("{path}.{slot}"),
                    what: "wrapped a lone relation in a list".into(),
                });
            }
            if let Some(items) = v.as_array_mut() {
                for (i, item) in items.iter_mut().enumerate() {
                    if let Some(o) = item.as_object_mut() {
                        // a relation's `by` (same, every) is a list of clauses; its `order` (near) stays order terms
                        repair_slots(o, &format!("{path}.{slot}[{i}]"), out, false);
                        if let Some(w) = o.get_mut("window") {
                            repair_units(w, &format!("{path}.{slot}[{i}].window"), out);
                        }
                    }
                }
            }
        }
    }
    if let Some(p) = m.get_mut("pick").and_then(Value::as_object_mut) {
        repair_slots(p, &format!("{path}.pick"), out, true);
    }
    // a group's `by` is a list of clauses, not order terms: no direction belongs there
    if let Some(g) = m.get_mut("group").and_then(Value::as_object_mut) {
        repair_slots(g, &format!("{path}.group"), out, false);
    }
    if let Some(b) = m.get_mut("bind").and_then(Value::as_object_mut) {
        for (name, c) in b.iter_mut() {
            repair_clause(c, &format!("{path}.bind.{name}"), out);
        }
    }
}

/// The list slots of one object: a lone clause is wrapped, every clause
/// inside is repaired. `by_is_order` says whether `by` holds order terms
/// (`pick`, `out`) or plain clauses (`group`).
fn repair_slots(m: &mut Map<String, Value>, path: &str, out: &mut Vec<Repair>, by_is_order: bool) {
    for slot in LIST_SLOTS {
        let Some(v) = m.get_mut(*slot) else {
            continue;
        };
        if is_clause_shaped(v) {
            let one = v.take();
            *v = Value::Array(vec![one]);
            out.push(Repair {
                path: format!("{path}.{slot}"),
                what: "wrapped a lone clause in a list".into(),
            });
        }
        if let Some(items) = v.as_array_mut() {
            for (i, item) in items.iter_mut().enumerate() {
                let p = format!("{path}.{slot}[{i}]");
                // an order term is [clause, dir]; a bare clause gets asc
                if (*slot == "by" && by_is_order) || *slot == "order" {
                    if is_clause_shaped(item) {
                        let c = item.take();
                        *item = Value::Array(vec![c, Value::String("asc".into())]);
                        out.push(Repair {
                            path: p.clone(),
                            what: "an order term without a direction is ascending".into(),
                        });
                    }
                    if let Some(pair) = item.as_array_mut()
                        && let Some(c) = pair.first_mut()
                    {
                        repair_clause(c, &p, out);
                    }
                } else {
                    repair_clause(item, &p, out);
                }
            }
        }
    }
}

/// An array whose first element is a string and whose second is not an
/// object: a clause written without its options map, or a ref written as
/// `["field", "path"]`.
fn is_clause_shaped(v: &Value) -> bool {
    match v {
        Value::Array(items) => items.first().is_some_and(Value::is_string),
        _ => false,
    }
}

fn repair_clause(v: &mut Value, path: &str, out: &mut Vec<Repair>) {
    let Some(items) = v.as_array_mut() else {
        return;
    };
    let Some(Value::String(op)) = items.first().cloned() else {
        return;
    };
    // the options map
    if items.len() < 2 || !items[1].is_object() {
        items.insert(1, Value::Object(Map::new()));
        out.push(Repair {
            path: path.to_string(),
            what: format!("inserted the missing options map of {op}"),
        });
    }
    // the op alias
    if let Some((_, canonical)) = OP_ALIASES.iter().find(|(a, _)| *a == op) {
        items[0] = Value::String((*canonical).to_string());
        out.push(Repair {
            path: path.to_string(),
            what: format!("{op} is written {canonical}"),
        });
    }
    // units inside the options
    if let Some(opts) = items[1].as_object_mut() {
        for (k, ov) in opts.iter_mut() {
            repair_units(ov, &format!("{path}.{k}"), out);
        }
    }
    // the arguments, recursively
    for (i, a) in items.iter_mut().enumerate().skip(2) {
        if is_clause_shaped(a) {
            repair_clause(a, &format!("{path}[{i}]"), out);
        }
    }
}

/// `unit: days` becomes `unit: day`, wherever a window is.
fn repair_units(v: &mut Value, path: &str, out: &mut Vec<Repair>) {
    if let Some(o) = v.as_object_mut()
        && let Some(Value::String(u)) = o.get("unit")
        && let Some((alias, canonical)) = UNIT_ALIASES.iter().find(|(a, _)| a == u)
    {
        o.insert("unit".into(), Value::String((*canonical).to_string()));
        out.push(Repair {
            path: format!("{path}.unit"),
            what: format!("{alias} is written {canonical}"),
        });
    }
}
