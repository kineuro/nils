// SPDX-License-Identifier: AGPL-3.0-only

//! The pipeline runner of NILS (decision record 09, D9; record 43): the
//! parts of a run that need no registry.
//!
//! - [`descriptor`]: `nils.job.yml` as `contracts/job/v1` fixes it, parsed and
//!   checked, the image pinned by its registry manifest digest or refused
//!   (record 43 R3), the parameters resolved with their defaults filled.
//! - [`words`]: the command line a container runs, split as a shell splits
//!   words and with its value-keys replaced.
//! - [`runtime`]: the container runtimes behind one trait, found in the order
//!   rootless podman, apptainer, then docker only where an operator opted in
//!   (record 43 R2, D18), and the flags every run carries.
//! - [`results`]: `results.json`, what a pipeline says of its units.
//! - [`files`]: what a run left under its output folder, found by the
//!   descriptor's path templates and hashed.
//!
//! The binary owns the registry side: the catalog and run rows, the input it
//! materialises, the derivatives it registers and the review items it raises.

pub mod descriptor;
pub mod files;
pub mod results;
pub mod runtime;
pub mod words;

pub use descriptor::Descriptor;
pub use results::Results;
pub use runtime::{Invocation, Mount, Runtime};

/// The job contract version this crate reads and writes.
pub const CONTRACT: &str = "job/v1";

/// sha256 of some bytes, as `sha256:<hex>`.
pub fn sha256(bytes: &[u8]) -> String {
    format!(
        "sha256:{}",
        hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
    )
}

/// A JSON value written with every object's keys in order, so that two
/// documents that mean the same have the same digest whatever order their
/// keys were written in.
pub fn canonical(value: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String((*k).clone()).to_string());
                    out.push(':');
                    walk(&map[*k], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    walk(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    walk(value, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_form_does_not_depend_on_key_order() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"b":1,"a":{"d":[1,{"z":0,"y":1}],"c":"x"}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"a":{"c":"x","d":[1,{"y":1,"z":0}]},"b":1}"#).unwrap();
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(
            canonical(&a),
            r#"{"a":{"c":"x","d":[1,{"y":1,"z":0}]},"b":1}"#
        );
        assert_eq!(sha256(b"").len(), 7 + 64);
    }
}
