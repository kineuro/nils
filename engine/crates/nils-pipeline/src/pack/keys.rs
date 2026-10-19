// SPDX-License-Identifier: AGPL-3.0-only

//! The two keys every result carries (record 57 P3; spec section 5.6),
//! computed by the engine from the pack and the run, and the words every
//! door says where results of several comparability keys would be pooled.
//!
//! The **comparability key** says which results may be pooled: the tool,
//! its version, the image digest (or a registered install's fingerprint),
//! the pack's digest, the variant, every knob whose comparability is
//! `breaks` with its value, the input contrast set, the converter and its
//! version for a BIDS input, and, recursively, the comparability keys of
//! the steps it read. The **result key** is the comparability key plus
//! every remaining knob value and the input stacks by content; it is what
//! reuse matches. Both are `sha256:<hex>` of the canonical JSON of their
//! members, which is kept beside them so a person can read what made a key.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// The members of a comparability key.
#[derive(Debug, Clone, Default)]
pub struct Members {
    pub tool: String,
    pub tool_version: String,
    /// The image's registry manifest digest, or a registered install's
    /// fingerprint.
    pub image: String,
    /// The pack's digest; none for a version with no pack.
    pub pack: Option<String>,
    pub variant: Option<String>,
    /// Every knob whose comparability is `breaks`, with its value.
    pub breaks: BTreeMap<String, Value>,
    /// The input contrast set: the roles present, sorted; a stacks unit's
    /// own base where the pack names it under `comparability.facts`.
    pub contrasts: Vec<String>,
    /// The facts the pack names under `comparability.facts`, with values.
    pub facts: BTreeMap<String, Value>,
    /// The converter and its version, for a BIDS input.
    pub converter: Option<(String, String)>,
    /// The comparability keys of the steps whose results this unit read.
    pub upstream: Vec<String>,
}

/// The comparability key and its members as a document.
pub fn comparability(m: &Members) -> (String, Value) {
    let mut contrasts = m.contrasts.clone();
    contrasts.sort();
    contrasts.dedup();
    let mut upstream = m.upstream.clone();
    upstream.sort();
    upstream.dedup();
    let doc = json!({
        "tool": m.tool,
        "tool_version": m.tool_version,
        "image": m.image,
        "pack": m.pack,
        "variant": m.variant,
        "breaks": m.breaks,
        "contrasts": contrasts,
        "facts": m.facts,
        "converter": m.converter.as_ref().map(|(n, v)| json!({"name": n, "version": v})),
        "upstream": upstream,
    });
    (crate::sha256(crate::canonical(&doc).as_bytes()), doc)
}

/// The result key: the comparability key, every knob's value, and the
/// inputs by content (each input file's identity, sorted).
pub fn result(
    comparability_key: &str,
    knobs: &Map<String, Value>,
    inputs: &[String],
) -> (String, Value) {
    let mut inputs: Vec<&String> = inputs.iter().collect();
    inputs.sort();
    inputs.dedup();
    let doc = json!({
        "comparability": comparability_key,
        "knobs": knobs,
        "inputs": inputs,
    });
    (crate::sha256(crate::canonical(&doc).as_bytes()), doc)
}

/// A key as a person reads it: its first twelve hex characters.
pub fn short(key: &str) -> String {
    let hex = key.strip_prefix("sha256:").unwrap_or(key);
    hex.chars().take(12).collect()
}

/// What every door says where a measure would pool results of more than
/// one comparability key (spec section 5.6, G5): the same words at the ask,
/// a run's numbers, a release and an export.
pub fn pooling_words(keys: &[String]) -> String {
    let mut keys: Vec<&String> = keys.iter().collect();
    keys.sort();
    keys.dedup();
    format!(
        "these results hold {} comparability keys ({}): they are answered per key and never summed together, since results of different tools, versions, variants or settings that break comparability do not compare",
        keys.len(),
        keys.iter().map(|k| short(k)).collect::<Vec<_>>().join(", ")
    )
}

/// Values grouped by comparability key, as a door answers them: one entry
/// a key, each with its members' count, and whether they may be pooled
/// (one key) or not (several, with [`pooling_words`]).
pub fn by_key<T: Clone>(items: &[(String, T)]) -> (Vec<(String, Vec<T>)>, Option<String>) {
    let mut groups: BTreeMap<String, Vec<T>> = BTreeMap::new();
    for (k, v) in items {
        groups.entry(k.clone()).or_default().push(v.clone());
    }
    let keys: Vec<String> = groups.keys().cloned().collect();
    let words = (keys.len() > 1).then(|| pooling_words(&keys));
    (groups.into_iter().collect(), words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_follow_their_members_and_the_words_name_every_key() {
        let mut m = Members {
            tool: "samseg-lesions".into(),
            tool_version: "freesurfer-8.2.0".into(),
            image: "sha256:aaaa".into(),
            pack: Some("sha256:bbbb".into()),
            variant: None,
            breaks: [("lesion".to_string(), json!("on"))].into_iter().collect(),
            contrasts: vec!["t1w".into(), "flair".into()],
            facts: BTreeMap::new(),
            converter: Some(("dcm2niix".into(), "v1.0.20240202".into())),
            upstream: Vec::new(),
        };
        let (a, doc) = comparability(&m);
        assert_eq!(doc["contrasts"], json!(["flair", "t1w"]));
        m.contrasts = vec!["flair".into(), "t1w".into(), "t1w".into()];
        assert_eq!(comparability(&m).0, a, "order and repeats do not matter");
        m.breaks.insert("lesion".into(), json!("off"));
        let (b, _) = comparability(&m);
        assert_ne!(a, b, "a breaking knob's value changes the key");
        m.breaks.insert("lesion".into(), json!("on"));
        m.upstream.push("sha256:cccc".into());
        assert_ne!(comparability(&m).0, a, "a step read changes the key");
        let knobs: Map<String, Value> = [("threads".to_string(), json!(4))].into_iter().collect();
        let (r1, _) = result(&a, &knobs, &["x".into(), "y".into()]);
        let (r2, _) = result(&a, &knobs, &["y".into(), "x".into()]);
        assert_eq!(r1, r2);
        let (r3, _) = result(&a, &knobs, &["x".into()]);
        assert_ne!(r1, r3);
        let words = pooling_words(&[a.clone(), b.clone(), a.clone()]);
        assert!(
            words.starts_with("these results hold 2 comparability keys ("),
            "{words}"
        );
        assert!(words.contains(&short(&a)) && words.contains(&short(&b)));
        assert!(words.contains("never summed together"));
        let (groups, w) = by_key(&[(a.clone(), 1), (b.clone(), 2), (a.clone(), 3)]);
        assert_eq!(groups.len(), 2);
        assert!(w.is_some());
        let (groups, w) = by_key(&[(a.clone(), 1), (a.clone(), 3)]);
        assert_eq!(groups[0].1, vec![1, 3]);
        assert!(w.is_none());
        assert_eq!(short("sha256:0123456789abcdef"), "0123456789ab");
    }
}
