// SPDX-License-Identifier: AGPL-3.0-only

//! The identity rule (`docs/specs/wave1-parse-and-digest.md`, §7.3): data in
//! the batch's config, read once, applied to every file in the parser. The
//! fields are tried in order and the first value wins; a pattern takes its
//! named group `id` as the identifier; when nothing yields, the study UID is
//! the identifier and the file is filed under `study-instance-uid`, which is
//! v0's behaviour. The default rule is v0's two lines.
//!
//! What the rule reads never travels past it: [`Rule::apply`] takes the
//! identifying values out of the extracted file and hands back the one
//! identifier the writer derives the code from.

use std::fmt;

use nils_dicom::{Diagnostic, DiagnosticKind, Extracted, IdentityFields, tag_of};
use nils_registry::linkage::valid_id_type_name;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;

/// The id type of the default rule: DICOM's PatientID.
pub const DEFAULT_ID_TYPE: &str = "patient-id";

/// The field the fallback reads.
pub const FALLBACK_FIELD: &str = "StudyInstanceUID";

/// The id type the fallback files under.
pub const FALLBACK_ID_TYPE: &str = "study-instance-uid";

/// Where one source of the rule reads from.
#[derive(Debug, Clone)]
pub enum From {
    /// A DICOM keyword, read out of the file.
    Field(String),
    /// A directory of the file's path under the batch root, counted from one.
    /// v0 calls this `depth_after_root`, and it is the only identity several
    /// archives have: the sender pseudonymised and put the code in the path
    /// rather than in the tag (spec section 3).
    Segment(usize),
}

impl From {
    /// What a message calls it.
    pub fn label(&self) -> String {
        match self {
            From::Field(f) => f.clone(),
            From::Segment(n) => format!("path segment {n}"),
        }
    }
}

/// One source of the rule, with its pattern when the value carries more than
/// one thing.
#[derive(Debug, Clone)]
pub struct Source {
    pub from: From,
    pub pattern: Option<Regex>,
}

/// The rule as resolved: keywords checked, patterns compiled.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id_type: String,
    pub from: Vec<Source>,
    /// The value the rule reads is the subject code itself, not an
    /// identifier to derive one from (§7.3): data pseudonymized before it
    /// reaches us, where the code was decided by whoever holds the key. A
    /// fallback identifier is derived as always, since a study UID is no
    /// code.
    pub verbatim: bool,
    /// Where the rule came from, for `--describe`: a path, or none for the
    /// default.
    pub source: Option<String>,
    fields: IdentityFields,
}

/// The `n`th directory of `rel`, counted from one. The last component is the
/// file itself and is never a segment: a subject is a directory.
fn segment(rel: &str, n: usize) -> Option<&str> {
    let mut parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    parts.pop()?;
    parts.get(n - 1).copied()
}

/// The identifier of one file as the rule resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub value: String,
    /// The fallback was taken: the value is the study UID, the type
    /// [`FALLBACK_ID_TYPE`].
    pub fell_back: bool,
}

/// Why a rule file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError(pub String);

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RuleError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    identity: Spec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    id_type: String,
    from: Vec<SourceSpec>,
    fallback: Option<String>,
    /// `derived` (the default) or `verbatim`.
    code: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSpec {
    field: Option<String>,
    path: Option<PathSpec>,
    pattern: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathSpec {
    /// Counted from one, which is v0's `depth_after_root`.
    segment: usize,
    pattern: Option<String>,
}

impl Default for Rule {
    /// v0's two lines: PatientID, then the fallback.
    fn default() -> Rule {
        Rule {
            id_type: DEFAULT_ID_TYPE.into(),
            from: vec![Source {
                from: From::Field("PatientID".into()),
                pattern: None,
            }],
            verbatim: false,
            source: None,
            fields: IdentityFields::default(),
        }
    }
}

impl Rule {
    /// A rule from its YAML, checked: the id type name is valid, every field
    /// is a DICOM keyword, every pattern compiles and names a group `id`, and
    /// the fallback, when written, is the only one there is.
    pub fn parse(yaml: &str) -> Result<Rule, RuleError> {
        let file: File = serde_saphyr::from_str(yaml)
            .map_err(|e| RuleError(format!("the identity rule does not parse: {e}")))?;
        let spec = file.identity;
        if !valid_id_type_name(&spec.id_type) {
            return Err(RuleError(format!(
                "identity.id_type: {} is not a valid id type name (lowercase letters, digits and hyphens)",
                spec.id_type
            )));
        }
        if spec.from.is_empty() {
            return Err(RuleError(
                "identity.from is empty; the rule needs at least one field".into(),
            ));
        }
        let mut from = Vec::with_capacity(spec.from.len());
        for (i, s) in spec.from.into_iter().enumerate() {
            let (kind, raw_pattern) = match (s.field, s.path) {
                (Some(_), Some(_)) => {
                    return Err(RuleError(format!(
                        "identity.from[{i}]: a source reads a field or a path, not both"
                    )));
                }
                (None, None) => {
                    return Err(RuleError(format!(
                        "identity.from[{i}]: a source needs a field or a path"
                    )));
                }
                (Some(field), None) => {
                    if tag_of(&field).is_none() {
                        return Err(RuleError(format!(
                            "identity.from[{i}].field: {field} is not a DICOM keyword"
                        )));
                    }
                    (From::Field(field), s.pattern)
                }
                (None, Some(path)) => {
                    if path.segment == 0 {
                        return Err(RuleError(format!(
                            "identity.from[{i}].path.segment: segments are counted from one"
                        )));
                    }
                    if s.pattern.is_some() && path.pattern.is_some() {
                        return Err(RuleError(format!(
                            "identity.from[{i}]: the pattern belongs either beside the path or \
                             inside it, not in both places"
                        )));
                    }
                    (From::Segment(path.segment), path.pattern.or(s.pattern))
                }
            };
            let pattern = match raw_pattern {
                Some(p) => {
                    let re = Regex::new(&p)
                        .map_err(|e| RuleError(format!("identity.from[{i}].pattern: {e}")))?;
                    if !re.capture_names().any(|n| n == Some("id")) {
                        return Err(RuleError(format!(
                            "identity.from[{i}].pattern has no named group `id`; write (?<id>...) around the identifier"
                        )));
                    }
                    Some(re)
                }
                None => None,
            };
            from.push(Source {
                from: kind,
                pattern,
            });
        }
        if let Some(fb) = &spec.fallback
            && fb != FALLBACK_FIELD
        {
            return Err(RuleError(format!(
                "identity.fallback: {fb}; the only fallback is {FALLBACK_FIELD}"
            )));
        }
        let verbatim = match spec.code.as_deref() {
            None | Some("derived") => false,
            Some("verbatim") => true,
            Some(other) => {
                return Err(RuleError(format!(
                    "identity.code: {other}; the code is either derived (the default) or verbatim"
                )));
            }
        };
        if verbatim && from.iter().any(|s| s.pattern.is_none()) {
            return Err(RuleError(
                "identity.code: verbatim needs a pattern on every source, so that a value which \
                 is not shaped like a subject code is never filed as one"
                    .into(),
            ));
        }
        let keywords: Vec<&str> = from
            .iter()
            .filter_map(|s| match &s.from {
                From::Field(f) => Some(f.as_str()),
                From::Segment(_) => None,
            })
            .collect();
        let fields = IdentityFields::new(&keywords).map_err(|e| RuleError(e.to_string()))?;
        Ok(Rule {
            id_type: spec.id_type,
            from,
            verbatim,
            source: None,
            fields,
        })
    }

    /// The fields the reader must extract, in the rule's order. A path source
    /// is not among them: it is read from the path, not from the file.
    pub fn fields(&self) -> &IdentityFields {
        &self.fields
    }

    /// The id type of an identifier the rule resolved.
    pub fn id_type_of(&self, ident: &Ident) -> &str {
        if ident.fell_back {
            FALLBACK_ID_TYPE
        } else {
            &self.id_type
        }
    }

    /// The rule in words, as `--describe` prints it.
    pub fn describe(&self) -> String {
        let fields: Vec<String> = self
            .from
            .iter()
            .map(|s| {
                if s.pattern.is_some() {
                    format!("{} (pattern)", s.from.label())
                } else {
                    s.from.label()
                }
            })
            .collect();
        let mut out = String::new();
        if self.id_type != DEFAULT_ID_TYPE {
            out.push_str(&format!("as {}: ", self.id_type));
        }
        out.push_str(&fields.join(", "));
        out.push_str(&format!(", then {FALLBACK_FIELD}"));
        if self.verbatim {
            out.push_str("; the value read is the code itself");
        }
        if let Some(src) = &self.source {
            out.push_str(&format!(" (from {src})"));
        }
        out
    }

    /// The rule as `ingest_batch.config` records it.
    pub fn to_json(&self) -> serde_json::Value {
        let from: Vec<serde_json::Value> = self
            .from
            .iter()
            .map(|s| {
                let mut o = match &s.from {
                    From::Field(f) => json!({ "field": f }),
                    From::Segment(n) => json!({ "path": { "segment": n } }),
                };
                if let Some(re) = &s.pattern {
                    o["pattern"] = json!(re.as_str());
                }
                o
            })
            .collect();
        json!({
            "id_type": self.id_type,
            "from": from,
            "fallback": FALLBACK_FIELD,
            "code": if self.verbatim { "verbatim" } else { "derived" },
            "source": self.source,
        })
    }

    /// Resolve one file: the first field that yields, else the fallback; the
    /// diagnostics of §7.3 go on the file, and the identifying values are
    /// taken out of it.
    pub fn apply(&self, x: &mut Extracted, rel: &str) -> Ident {
        let values = std::mem::take(&mut x.identity.values);
        // The reader extracted one value per *field* source, in order, so the
        // two lists are walked together rather than zipped: a path source
        // consumes no extracted value.
        let mut next_field = 0usize;
        for source in &self.from {
            let read: Option<String> = match &source.from {
                From::Field(_) => {
                    let v = values.get(next_field).and_then(|v| v.clone());
                    next_field += 1;
                    v
                }
                From::Segment(n) => segment(rel, *n).map(str::to_string),
            };
            let Some(value) = read.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
                continue;
            };
            match &source.pattern {
                None => {
                    return Ident {
                        value: value.to_string(),
                        fell_back: false,
                    };
                }
                Some(re) => {
                    let id = re
                        .captures(value)
                        .and_then(|c| c.name("id"))
                        .map(|m| m.as_str())
                        .filter(|id| !id.is_empty());
                    match id {
                        Some(id) => {
                            return Ident {
                                value: id.to_string(),
                                fell_back: false,
                            };
                        }
                        None => x.diagnostics.push(
                            Diagnostic::new(DiagnosticKind::IdentityUnparsed, source.from.label())
                                .with_shape(value),
                        ),
                    }
                }
            }
        }
        let tried: Vec<String> = self.from.iter().map(|s| s.from.label()).collect();
        x.diagnostics.push(Diagnostic::new(
            DiagnosticKind::IdentityFallback,
            tried.join(", "),
        ));
        Ident {
            value: x.study_uid.clone(),
            fell_back: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::Identity;

    /// A minimal MR file read back, its identity values replaced.
    fn extracted(values: Vec<Option<&str>>) -> Extracted {
        use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10};
        let dir = TempDir::new("rule");
        let path = dir.file(
            "a.dcm",
            &part10(
                &MetaFields::mr("1.2.826.0.1.3680043.8.498.3"),
                &minimal_mr(
                    "1.2.826.0.1.3680043.8.498.1",
                    "1.2.826.0.1.3680043.8.498.2",
                    "1.2.826.0.1.3680043.8.498.3",
                ),
                true,
            ),
        );
        let mut x = nils_dicom::extract(&path).unwrap();
        x.identity = Identity {
            values: values.into_iter().map(|v| v.map(str::to_string)).collect(),
        };
        x
    }

    #[test]
    fn the_default_rule_is_patient_id_then_the_fallback() {
        let rule = Rule::default();
        assert_eq!(rule.describe(), "PatientID, then StudyInstanceUID");
        assert_eq!(rule.fields().keywords().collect::<Vec<_>>(), ["PatientID"]);
        let mut x = extracted(vec![Some(" P1 ")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(
            ident,
            Ident {
                value: "P1".into(),
                fell_back: false
            }
        );
        assert_eq!(rule.id_type_of(&ident), "patient-id");
        assert!(x.identity.values.is_empty(), "the values are taken out");
        assert!(x.diagnostics.is_empty());

        let mut x = extracted(vec![Some("   ")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, "1.2.826.0.1.3680043.8.498.1");
        assert!(ident.fell_back);
        assert_eq!(rule.id_type_of(&ident), "study-instance-uid");
        assert_eq!(x.diagnostics.len(), 1);
        assert_eq!(x.diagnostics[0].kind, DiagnosticKind::IdentityFallback);
        assert_eq!(x.diagnostics[0].subject, "PatientID");
        assert_eq!(rule.to_json()["fallback"], "StudyInstanceUID");
        assert_eq!(rule.to_json()["from"][0]["field"], "PatientID");
    }

    #[test]
    fn a_rule_file_is_checked_then_applied() {
        let yaml = r"
identity:
  id_type: personal-number
  from:
    - field: PatientName
      pattern: '^(?<id>\d{12})[-_ ](?<date>\d{8})$'
    - field: PatientID
  fallback: StudyInstanceUID
";
        let rule = Rule::parse(yaml).unwrap();
        assert_eq!(
            rule.describe(),
            "as personal-number: PatientName (pattern), PatientID, then StudyInstanceUID"
        );
        assert_eq!(
            rule.fields().keywords().collect::<Vec<_>>(),
            ["PatientName", "PatientID"]
        );
        // the pattern yields
        let mut x = extracted(vec![Some("199001011234-20240131"), Some("P1")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, "199001011234");
        assert!(!ident.fell_back);
        assert!(x.diagnostics.is_empty());
        // the pattern does not: unparsed, with the shape, then the next field
        let mut x = extracted(vec![Some("Doe^Jane"), Some("P1")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, "P1");
        assert_eq!(x.diagnostics.len(), 1);
        assert_eq!(x.diagnostics[0].kind, DiagnosticKind::IdentityUnparsed);
        assert_eq!(x.diagnostics[0].sample(), "PatientName=Aaa^Aaaa");
        // nothing yields: the fallback, after the unparsed one
        let mut x = extracted(vec![Some("Doe^Jane"), None]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert!(ident.fell_back);
        assert_eq!(x.diagnostics.len(), 2);
        assert_eq!(x.diagnostics[1].sample(), "PatientName, PatientID");
        let json = rule.to_json();
        assert_eq!(json["id_type"], "personal-number");
        assert!(
            json["from"][0]["pattern"]
                .as_str()
                .unwrap()
                .contains("(?<id>")
        );
        assert!(json["from"][1].get("pattern").is_none());
    }

    #[test]
    fn a_verbatim_rule_reads_the_code_itself() {
        let yaml = r"
identity:
  id_type: subject-code
  from:
    - field: PatientID
      pattern: '^(?<id>[0-9a-f]{16})$'
  code: verbatim
";
        let rule = Rule::parse(yaml).unwrap();
        assert!(rule.verbatim);
        assert!(
            rule.describe()
                .ends_with("; the value read is the code itself")
        );
        assert_eq!(rule.to_json()["code"], "verbatim");
        let mut x = extracted(vec![Some("771c4326c89c082c")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, "771c4326c89c082c");
        assert!(!ident.fell_back);
        // a value that is not shaped like a code is not one: the file falls
        // back to its study UID, which is derived like any identifier
        let mut x = extracted(vec![Some("19800101-1234")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert!(ident.fell_back);
        // the default rule derives
        assert!(!Rule::default().verbatim);
        assert_eq!(Rule::default().to_json()["code"], "derived");
    }

    #[test]
    fn a_rule_file_is_refused_with_the_reason() {
        let err = |yaml: &str| Rule::parse(yaml).unwrap_err().to_string();
        assert!(err("identity: [").starts_with("the identity rule does not parse"));
        assert!(
            err("identity:\n  id_type: Bad Name\n  from:\n    - field: PatientID\n")
                .contains("identity.id_type: Bad Name is not a valid")
        );
        assert_eq!(
            err("identity:\n  id_type: x\n  from: []\n"),
            "identity.from is empty; the rule needs at least one field"
        );
        assert_eq!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientId\n"),
            "identity.from[0].field: PatientId is not a DICOM keyword"
        );
        assert!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientID\n      pattern: '('\n")
                .starts_with("identity.from[0].pattern: ")
        );
        assert!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientID\n      pattern: '(\\d+)'\n")
                .contains("has no named group `id`")
        );
        assert!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientID\n  code: taken\n")
                .contains("identity.code: taken; the code is either derived")
        );
        assert!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientID\n  code: verbatim\n")
                .contains("verbatim needs a pattern on every source")
        );
        assert_eq!(
            err(
                "identity:\n  id_type: x\n  from:\n    - field: PatientID\n  fallback: AccessionNumber\n"
            ),
            "identity.fallback: AccessionNumber; the only fallback is StudyInstanceUID"
        );
        assert!(
            err("identity:\n  id_type: x\n  from:\n    - field: PatientID\n  extra: 1\n")
                .starts_with("the identity rule does not parse")
        );
        // the fallback may be left out: it is the only one
        assert!(Rule::parse("identity:\n  id_type: x\n  from:\n    - field: PatientID\n").is_ok());
    }
}
