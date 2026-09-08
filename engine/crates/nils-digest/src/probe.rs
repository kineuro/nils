// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: candidate identity rules probed over a bounded sample of a
//! tree, side by side, writing nothing. Per candidate: the shape histogram
//! of each source, whether the first field source is constant
//! (`identity_constant`) with its shape, how many files each source
//! answered, fell through or failed to parse, the subject and study counts
//! under that rule, and the reader's diagnostics. Shapes, never values; no
//! path in the answer; nothing written.

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

use nils_dicom::extract::IdentityFields;
use serde_json::{Value, json};

use crate::rule::{Outcome, Rule};

/// The most files a probe reads, and how many when nobody says.
pub const SAMPLE_MAX: usize = 20_000;
pub const SAMPLE_DEFAULT: usize = 2_000;
/// Distinct shapes kept per source; past this, the rest are one bucket.
const SHAPES_MAX: usize = 64;
/// Samples kept per diagnostic kind.
const SAMPLES_MAX: usize = 10;

fn hash_of(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

#[derive(Default)]
struct SourceTally {
    answered: u64,
    empty: u64,
    unparsed: u64,
    unread: u64,
    shapes: BTreeMap<String, u64>,
    other_shapes: u64,
}

#[derive(Default)]
struct CandidateTally {
    files: u64,
    sources: Vec<SourceTally>,
    first_values: HashSet<u64>,
    first_shape: Option<String>,
    first_probed: u64,
    subjects: HashSet<u64>,
    studies: HashSet<u64>,
    fell_back: u64,
    diagnostics: BTreeMap<String, (u64, Vec<String>)>,
}

impl CandidateTally {
    fn new(sources: usize) -> CandidateTally {
        CandidateTally {
            sources: (0..sources).map(|_| SourceTally::default()).collect(),
            ..Default::default()
        }
    }

    fn note(
        &mut self,
        rule: &Rule,
        traced: &crate::rule::Traced,
        diagnostics: &[nils_dicom::Diagnostic],
    ) {
        self.files += 1;
        for (i, (shape, outcome)) in traced.sources.iter().enumerate() {
            let t = &mut self.sources[i];
            match outcome {
                Outcome::Answered => t.answered += 1,
                Outcome::Empty => t.empty += 1,
                Outcome::Unparsed => t.unparsed += 1,
                Outcome::Unread => t.unread += 1,
            }
            if let Some(s) = shape {
                if let Some(n) = t.shapes.get_mut(s) {
                    *n += 1;
                } else if t.shapes.len() < SHAPES_MAX {
                    t.shapes.insert(s.clone(), 1);
                } else {
                    t.other_shapes += 1;
                }
            }
        }
        if let Some(p) = &traced.ident.probe {
            self.first_probed += 1;
            if self.first_values.len() < SHAPES_MAX {
                self.first_values.insert(hash_of(p));
            }
            if self.first_shape.is_none() {
                self.first_shape = Some(nils_dicom::diagnostic::shape(p));
            }
        }
        if traced.ident.fell_back {
            self.fell_back += 1;
        }
        self.subjects.insert(hash_of(&format!(
            "{}\0{}",
            rule.id_type_of(&traced.ident),
            traced.ident.value
        )));
        for d in diagnostics {
            let e = self
                .diagnostics
                .entry(d.kind.name().to_string())
                .or_insert((0, Vec::new()));
            e.0 += 1;
            let sample = d.sample();
            if e.1.len() < SAMPLES_MAX && !e.1.contains(&sample) {
                e.1.push(sample);
            }
        }
    }

    fn as_json(&self, rule: &Rule) -> Value {
        let labels = rule.source_labels();
        let sources: Vec<Value> = self
            .sources
            .iter()
            .enumerate()
            .map(|(i, t)| {
                json!({
                    "source": labels.get(i).cloned().unwrap_or_default(),
                    "answered": t.answered,
                    "empty": t.empty,
                    "unparsed": t.unparsed,
                    "unread": t.unread,
                    "shapes": t.shapes,
                    "other_shapes": t.other_shapes,
                })
            })
            .collect();
        let constant =
            self.first_probed >= crate::report::PROBE_MIN && self.first_values.len() == 1;
        json!({
            "rule": {
                "id_type": rule.id_type(),
                "sources": labels,
            },
            "files": self.files,
            "sources": sources,
            "identity_constant": {
                "constant": constant,
                "shape": if self.first_probed > 0 { self.first_shape.clone() } else { None },
                "files": self.first_probed,
                "distinct": if self.first_values.len() >= SHAPES_MAX { json!(format!("{SHAPES_MAX}+")) } else { json!(self.first_values.len()) },
            },
            "subjects": self.subjects.len(),
            "studies": self.studies.len(),
            "fell_back": self.fell_back,
            "diagnostics": self.diagnostics.iter().map(|(k, (n, s))| (k.clone(), json!({"count": n, "samples": s}))).collect::<BTreeMap<_, _>>(),
        })
    }
}

/// Probe `candidates` over up to `sample` files under `root`. The root
/// never appears in the answer.
pub fn probe(
    root: &Path,
    sample: usize,
    candidates: &[(String, Rule)],
    workers: usize,
) -> Result<Value, String> {
    if candidates.is_empty() {
        return Err("no candidate rule to probe".into());
    }
    let sample = sample.clamp(1, SAMPLE_MAX);
    std::fs::read_dir(root).map_err(|e| format!("the location cannot be read: {e}"))?;
    let files = nils_dicom::survey::files_under(root, sample);

    // One read per file, for the union of every candidate's fields; each
    // candidate then sees its own fields in its own order.
    let mut union: Vec<String> = Vec::new();
    for (_, rule) in candidates {
        for k in rule.field_keywords() {
            if !union.contains(&k) {
                union.push(k);
            }
        }
    }
    let union_refs: Vec<&str> = union.iter().map(String::as_str).collect();
    let fields = IdentityFields::new(&union_refs).map_err(|e| e.to_string())?;
    let index: Vec<Vec<usize>> = candidates
        .iter()
        .map(|(_, r)| {
            r.field_keywords()
                .iter()
                .map(|k| union.iter().position(|u| u == k).expect("in the union"))
                .collect()
        })
        .collect();

    let workers = workers.max(1);
    let chunk = files.len().div_ceil(workers).max(1);
    let results: Vec<(Vec<CandidateTally>, BTreeMap<String, u64>, u64)> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                let fields = &fields;
                let index = &index;
                s.spawn(move || {
                    let mut tallies: Vec<CandidateTally> = candidates
                        .iter()
                        .map(|(_, r)| CandidateTally::new(r.source_labels().len()))
                        .collect();
                    let mut refused: BTreeMap<String, u64> = BTreeMap::new();
                    let mut parsed = 0u64;
                    for path in part {
                        let rel = path
                            .strip_prefix(root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .into_owned();
                        match nils_dicom::extract_with(path, fields, &[]) {
                            Ok(x) => {
                                parsed += 1;
                                for (ci, (_, rule)) in candidates.iter().enumerate() {
                                    let values: Vec<Option<String>> = index[ci]
                                        .iter()
                                        .map(|&u| x.identity.values.get(u).cloned().flatten())
                                        .collect();
                                    let traced = rule.trace(&values, &x.study_uid, &rel);
                                    tallies[ci].studies.insert(hash_of(&x.study_uid));
                                    tallies[ci].note(rule, &traced, &x.diagnostics);
                                }
                            }
                            Err(r) => *refused.entry(r.class.name().to_string()).or_insert(0) += 1,
                        }
                    }
                    (tallies, refused, parsed)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a probe thread"))
            .collect()
    });

    let mut tallies: Vec<CandidateTally> = candidates
        .iter()
        .map(|(_, r)| CandidateTally::new(r.source_labels().len()))
        .collect();
    let mut refused: BTreeMap<String, u64> = BTreeMap::new();
    let mut parsed = 0u64;
    for (part, part_refused, part_parsed) in results {
        parsed += part_parsed;
        for (k, n) in part_refused {
            *refused.entry(k).or_insert(0) += n;
        }
        for (ci, t) in part.into_iter().enumerate() {
            let into = &mut tallies[ci];
            into.files += t.files;
            for (i, s) in t.sources.into_iter().enumerate() {
                let d = &mut into.sources[i];
                d.answered += s.answered;
                d.empty += s.empty;
                d.unparsed += s.unparsed;
                d.unread += s.unread;
                d.other_shapes += s.other_shapes;
                for (shape, n) in s.shapes {
                    if let Some(m) = d.shapes.get_mut(&shape) {
                        *m += n;
                    } else if d.shapes.len() < SHAPES_MAX {
                        d.shapes.insert(shape, n);
                    } else {
                        d.other_shapes += n;
                    }
                }
            }
            into.first_probed += t.first_probed;
            if into.first_values.len() < SHAPES_MAX {
                into.first_values.extend(t.first_values);
            }
            if into.first_shape.is_none() {
                into.first_shape = t.first_shape;
            }
            into.subjects.extend(t.subjects);
            into.studies.extend(t.studies);
            into.fell_back += t.fell_back;
            for (k, (n, samples)) in t.diagnostics {
                let e = into.diagnostics.entry(k).or_insert((0, Vec::new()));
                e.0 += n;
                for s in samples {
                    if e.1.len() < SAMPLES_MAX && !e.1.contains(&s) {
                        e.1.push(s);
                    }
                }
            }
        }
    }

    Ok(json!({
        "sample": {"asked": sample, "files": files.len(), "parsed": parsed, "refused": refused},
        "candidates": candidates
            .iter()
            .zip(tallies.iter())
            .map(|((label, rule), t)| {
                let mut doc = t.as_json(rule);
                doc["label"] = json!(label);
                doc
            })
            .collect::<Vec<_>>(),
    }))
}

/// The sample size a caller asked for, bounded.
pub fn sample_of(asked: Option<i64>) -> usize {
    match asked {
        Some(n) if n > 0 => (n as usize).min(SAMPLE_MAX),
        _ => SAMPLE_DEFAULT,
    }
}
