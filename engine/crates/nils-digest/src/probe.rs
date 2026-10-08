// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 4c §6.6: candidate identity rules probed over a bounded sample of a
//! tree, side by side, writing nothing. Per candidate: the shape histogram
//! of each source, whether the first field source is constant
//! (`identity_constant`) with its shape, how many files each source
//! answered, fell through or failed to parse, the subject and study counts
//! under that rule, and the reader's diagnostics. Shapes, never values; no
//! path in the answer; nothing written.
//!
//! Record 55 K9: asked for it ([`probe_with`]), the probe also reads each
//! file's birth date, sex and study date and answers, per candidate, the
//! pairs of identities under that rule whose birth date and sex agree and
//! whose visits overlap. The pairs are kept in memory for the caller, who
//! names them as subjects by the linkage store or not at all; the answer
//! itself carries nothing of them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
/// The pairs of alike identities kept per candidate.
pub const ALIKE_MAX: usize = 200;
/// The keywords the merge reading adds to a probe's read: a person's birth
/// date and sex, and the day of the study (record 55 K9).
pub const ALIKE_KEYWORDS: [&str; 3] = ["PatientBirthDate", "PatientSex", "StudyDate"];

/// One identity under a rule as the merge reading keeps it, in memory only:
/// the identifier type and value, never written into an answer.
pub type Who = (String, String);

/// Two identities under one rule whose birth date and sex agree and whose
/// visits overlap (record 55 K9): `shared` study days in common, of `days`
/// each. Kept for the caller; the probe's answer carries none of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlikePair {
    pub a: Who,
    pub b: Who,
    pub shared: usize,
    pub days: (usize, usize),
}

/// What the merge reading saw of one identity: its birth date and sex as
/// the files say them (a disagreement between its files leaves it out),
/// and the days of its studies.
#[derive(Default)]
struct Person {
    birth: Option<String>,
    sex: Option<String>,
    disagrees: bool,
    days: BTreeSet<String>,
}

impl Person {
    fn note(&mut self, birth: Option<&str>, sex: Option<&str>, day: Option<&str>) {
        for (slot, value) in [(&mut self.birth, birth), (&mut self.sex, sex)] {
            match (slot.as_deref(), value) {
                (_, None) => {}
                (None, Some(v)) => *slot = Some(v.to_string()),
                (Some(had), Some(v)) if had != v => self.disagrees = true,
                _ => {}
            }
        }
        if let Some(d) = day {
            self.days.insert(d.to_string());
        }
    }

    fn merge(&mut self, other: Person) {
        self.note(other.birth.as_deref(), other.sex.as_deref(), None);
        self.disagrees |= other.disagrees;
        self.days.extend(other.days);
    }
}

/// The pairs of a candidate's identities whose birth date and sex agree and
/// whose study days overlap, in a stable order, at most [`ALIKE_MAX`].
fn alike_pairs(people: &HashMap<Who, Person>) -> Vec<AlikePair> {
    let mut by_person: BTreeMap<(&str, &str), Vec<(&Who, &Person)>> = BTreeMap::new();
    for (who, p) in people {
        if p.disagrees || p.days.is_empty() {
            continue;
        }
        if let (Some(b), Some(s)) = (p.birth.as_deref(), p.sex.as_deref()) {
            by_person.entry((b, s)).or_default().push((who, p));
        }
    }
    let mut out = Vec::new();
    for group in by_person.values_mut() {
        group.sort_by(|x, y| x.0.cmp(y.0));
        for i in 0..group.len() {
            for j in i + 1..group.len() {
                let (a, pa) = group[i];
                let (b, pb) = group[j];
                let shared = pa.days.intersection(&pb.days).count();
                if shared > 0 && out.len() < ALIKE_MAX {
                    out.push(AlikePair {
                        a: a.clone(),
                        b: b.clone(),
                        shared,
                        days: (pa.days.len(), pb.days.len()),
                    });
                }
            }
        }
    }
    out
}

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
    /// Record 55 K9: each identity's birth date, sex and days, when asked.
    people: HashMap<Who, Person>,
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
    probe_with(root, sample, candidates, workers, false).map(|(doc, _)| doc)
}

/// [`probe`], and with `alike` the merge reading of record 55 K9: per
/// candidate, in the candidates' order, the pairs of identities whose birth
/// date and sex agree and whose visits overlap, for the caller to name as
/// subjects. The answer is the same either way.
pub fn probe_with(
    root: &Path,
    sample: usize,
    candidates: &[(String, Rule)],
    workers: usize,
    alike: bool,
) -> Result<(Value, Vec<Vec<AlikePair>>), String> {
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
    // the merge reading's three fields go after the rules' own
    let alike_at: Option<[usize; 3]> = alike.then(|| {
        ALIKE_KEYWORDS.map(|k| match union.iter().position(|u| u == k) {
            Some(i) => i,
            None => {
                union.push(k.to_string());
                union.len() - 1
            }
        })
    });
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
                                    if let Some([b, s, d]) = alike_at
                                        && !traced.ident.fell_back
                                    {
                                        let at = |i: usize| {
                                            x.identity.values.get(i).and_then(|v| {
                                                v.as_deref()
                                                    .map(str::trim)
                                                    .filter(|v| !v.is_empty())
                                            })
                                        };
                                        tallies[ci]
                                            .people
                                            .entry((
                                                rule.id_type_of(&traced.ident).to_string(),
                                                traced.ident.value.clone(),
                                            ))
                                            .or_default()
                                            .note(at(b), at(s), at(d));
                                    }
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
            for (who, p) in t.people {
                into.people.entry(who).or_default().merge(p);
            }
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

    let pairs = tallies.iter().map(|t| alike_pairs(&t.people)).collect();
    let doc = json!({
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
    });
    Ok((doc, pairs))
}

/// The sample size a caller asked for, bounded.
pub fn sample_of(asked: Option<i64>) -> usize {
    match asked {
        Some(n) if n > 0 => (n as usize).min(SAMPLE_MAX),
        _ => SAMPLE_DEFAULT,
    }
}
