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
use nils_registry::personnummer;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;

/// The id type of the default rule: DICOM's PatientID.
pub const DEFAULT_ID_TYPE: &str = "patient-id";

/// The field the fallback reads.
pub const FALLBACK_FIELD: &str = "StudyInstanceUID";

/// The id type the fallback files under.
pub const FALLBACK_ID_TYPE: &str = "study-instance-uid";

/// The field whose value, under a rule that reads an ID that is the same
/// everywhere, is a hospital's reserve number when it is no personnummer
/// (Nima, 2026-10-16).
pub const RESERVE_FIELD: &str = "PatientID";

/// The id type a reserve number is looked up and held under.
pub const RESERVE_ID_TYPE: &str = nils_registry::personnummer::RESERVE_ID_TYPE;

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
    /// The tree the rule reads is this registry's own pseudonymised tree
    /// (record 26 §3), so a value read verbatim there is one of this
    /// registry's codes: a code a subject holds already, or a value of the
    /// shape this registry makes. A value that is neither is no code of
    /// ours, and a code is derived from it as from any identifier. The
    /// engine sets it on the rule it writes for a pseudonymised tree; a
    /// rule file cannot, and its `code: verbatim` files what it reads as
    /// the code as it always did.
    pub own_codes: bool,
    /// The tree is a dataset's pseudonymised tree, read by what the dataset
    /// declares PatientID holds (Wave 7a §5.4): a file whose PatientID holds
    /// a value the declaration cannot read (another identifier where it
    /// declares subject codes, or a personnummer) is held until a map or a
    /// person decides (§18), never filed under its study UID. The fallback
    /// made one person with four studies four subjects where a dataset
    /// declared subject codes and its files held another identifier. A file
    /// whose PatientID is empty says nothing to read, and still falls back
    /// to its study, as v0 filed it. The engine sets it on the rules it
    /// writes for a declaration; a rule file cannot, and keeps the fallback.
    pub holds_unread: bool,
    /// The rule reads an identified dataset's originals for the
    /// pseudonymiser (record 26 §3), where a PatientID that is no
    /// personnummer under a rule of that type is a hospital's reserve number
    /// (Nima, 2026-10-16): looked up under [`RESERVE_ID_TYPE`], held under it,
    /// never coded ([`Ident::reserve`]). The engine sets it on the rule the
    /// pseudonymiser reads a dataset's originals under; a rule file cannot,
    /// and a digest's rule falls back to the study as it always did.
    pub reads_reserve: bool,
    /// Where the rule came from, for `--describe`: a path, or none for the
    /// default.
    pub source: Option<String>,
    fields: IdentityFields,
    /// The day a ten digit personnummer takes its century against, read
    /// once when the rule is made.
    today: personnummer::Day,
}

/// The `n`th directory of `rel`, counted from one. The last component is the
/// file itself and is never a segment: a subject is a directory.
fn segment(rel: &str, n: usize) -> Option<&str> {
    let mut parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    parts.pop()?;
    parts.get(n - 1).copied()
}

/// What one source did on one file (Wave 4c §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It read a value and the rule took it.
    Answered,
    /// It had nothing, and the rule fell through.
    Empty,
    /// It had a value the pattern did not match, and the rule fell through.
    Unparsed,
    /// It had a value, and an earlier source had already answered.
    Unread,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Answered => "answered",
            Outcome::Empty => "empty",
            Outcome::Unparsed => "unparsed",
            Outcome::Unread => "unread",
        }
    }
}

/// One file under one rule, every source accounted for, nothing written.
#[derive(Debug, Clone)]
pub struct Traced {
    /// Per source in rule order: the shape of what it read, and what it did.
    pub sources: Vec<(Option<String>, Outcome)>,
    /// The source that answered, or none when the fallback was taken.
    pub answered_by: Option<usize>,
    pub ident: Ident,
}

/// The identifier of one file as the rule resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub value: String,
    /// What the rule's first *field* source read, whatever answered in the
    /// end. A batch whose files all read one value there is looking at a
    /// placeholder, and only the batch can see that (§3.3).
    pub probe: Option<String>,
    /// The fallback was taken: the value is the study UID, the type
    /// [`FALLBACK_ID_TYPE`].
    pub fell_back: bool,
    /// Under a rule that reads an ID that is the same everywhere in a
    /// dataset's originals ([`Rule::reads_reserve`]), the value of PatientID
    /// when it is none: a hospital's reserve number, in its one written form
    /// (Nima, 2026-10-16). The file is looked up under
    /// [`RESERVE_ID_TYPE`] before its study, and held under it when no
    /// subject holds either; no code is ever made from it. Only with the
    /// fallback, so a number of the rule's own type that answered stands as
    /// it always did.
    pub reserve: Option<String>,
}

impl Ident {
    /// The value the file is filed, and held, under: its reserve number
    /// where it has one, else its identifier.
    pub fn filed(&self) -> &str {
        self.reserve.as_deref().unwrap_or(&self.value)
    }
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
            own_codes: false,
            holds_unread: false,
            reads_reserve: false,
            source: None,
            fields: IdentityFields::default(),
            today: personnummer::today(),
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
        if verbatim && personnummer::is_type(&spec.id_type) {
            return Err(RuleError(format!(
                "identity.code: verbatim reads subject codes, and an ID that is the same \
                 everywhere ({}) is no subject code; a rule for such an ID derives the subject \
                 code from it",
                personnummer::ID_TYPE
            )));
        }
        // no code is ever made from a reserve number (Nima, 2026-10-16): a
        // rule that read one as its own type would make one for a number no
        // map names
        if spec.id_type == RESERVE_ID_TYPE {
            return Err(RuleError(format!(
                "identity.id_type: {RESERVE_ID_TYPE} is read beside an ID that is the same \
                 everywhere, never as a rule's own type: a rule of type {} reads a {RESERVE_FIELD} \
                 that is no personnummer as a reserve number, which only a map gives a subject",
                personnummer::ID_TYPE
            )));
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
            own_codes: false,
            holds_unread: false,
            reads_reserve: false,
            source: None,
            fields,
            today: personnummer::today(),
        })
    }

    /// Whether the rule files personnummer, whose values are normalised to
    /// twelve digits before a code or a lookup is derived from them, and a
    /// value that is no personnummer does not answer.
    pub fn normalises(&self) -> bool {
        personnummer::is_type(&self.id_type)
    }

    /// Whether a file's identifier is a personnummer whose code the subject
    /// code generator derives (record 54, D4): the rule files personnummer,
    /// the fallback was not taken, and the value is not read as a code. Such
    /// a number is its own map: its subject is made under the code, never
    /// held for want of a map and never marked provisional.
    pub fn derives_by_generator(&self, ident: &Ident) -> bool {
        self.normalises() && !self.verbatim && !ident.fell_back
    }

    /// What a source's answer becomes: the value as read, or under a
    /// personnummer rule its twelve digits, or nothing when it is none. A
    /// rule that reads subject codes takes no personnummer as one: an ID
    /// that is the same everywhere is never a subject code (record 54), and
    /// the code alphabet spells its digits.
    fn taken(&self, id: &str) -> Option<String> {
        if self.normalises() {
            personnummer::normalise_on(id, self.today).ok()
        } else if self.verbatim && personnummer::normalise_on(id, self.today).is_ok() {
            None
        } else {
            Some(id.to_string())
        }
    }

    /// The fields the reader must extract, in the rule's order. A path source
    /// is not among them: it is read from the path, not from the file.
    pub fn fields(&self) -> &IdentityFields {
        &self.fields
    }

    /// The id type of an identifier the rule resolved: the type of what the
    /// file is filed under ([`Ident::filed`]).
    pub fn id_type_of(&self, ident: &Ident) -> &str {
        if ident.reserve.is_some() {
            RESERVE_ID_TYPE
        } else if ident.fell_back {
            FALLBACK_ID_TYPE
        } else {
            &self.id_type
        }
    }

    /// Whether a source's value that is no personnummer is a reserve number:
    /// the rule reads an ID that is the same everywhere in a dataset's
    /// originals ([`Rule::reads_reserve`]), and the source is PatientID.
    fn reserve_source(&self, from: &From) -> bool {
        self.reads_reserve
            && self.normalises()
            && matches!(from, From::Field(f) if f == RESERVE_FIELD)
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
            // the generator's type by the words a person reads
            if self.id_type == nils_registry::schema::GENERATOR_ID_TYPE {
                out.push_str("as an ID that is the same everywhere: ");
            } else {
                out.push_str(&format!("as {}: ", self.id_type));
            }
        }
        out.push_str(&fields.join(", "));
        if self.normalises() {
            out.push_str(" (as twelve digits)");
        }
        match self.holds_unread {
            true => out.push_str(&format!(
                "; a value it cannot read is held, an empty one falls back to {FALLBACK_FIELD}"
            )),
            false => out.push_str(&format!(", then {FALLBACK_FIELD}")),
        }
        if self.from.iter().any(|s| self.reserve_source(&s.from)) {
            out.push_str(&format!(
                "; a {RESERVE_FIELD} that is no personnummer is a reserve number, found through a map and never coded"
            ));
        }
        if self.verbatim {
            out.push_str("; the value read is the subject code itself");
            if self.own_codes {
                out.push_str(", a code this registry holds or a value of its own shape");
            }
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
        let mut out = json!({
            "id_type": self.id_type,
            "from": from,
            "fallback": FALLBACK_FIELD,
            "code": if self.verbatim { "verbatim" } else { "derived" },
            "source": self.source,
        });
        if self.normalises() {
            out["normalise"] = json!("personnummer-12");
            if self.from.iter().any(|s| self.reserve_source(&s.from)) {
                out["reserve"] = json!(RESERVE_ID_TYPE);
            }
        }
        if self.holds_unread {
            out["unread"] = json!("held");
        }
        out
    }

    /// Wave 4c §6.6: resolve one file without touching it, and say what
    /// every source did on the way. The probe reads this; the digest reads
    /// [`Rule::apply`]. `values` is one entry per *field* source of this
    /// rule, in order, as the reader extracted them.
    pub fn trace(&self, values: &[Option<String>], study_uid: &str, rel: &str) -> Traced {
        let mut next_field = 0usize;
        let mut probe: Option<String> = None;
        let mut reserve: Option<String> = None;
        let mut sources: Vec<(Option<String>, Outcome)> = Vec::with_capacity(self.from.len());
        let mut answer: Option<(usize, String)> = None;
        for (i, source) in self.from.iter().enumerate() {
            let read: Option<String> = match &source.from {
                From::Field(_) => {
                    let v = values.get(next_field).and_then(|v| v.clone());
                    next_field += 1;
                    if probe.is_none()
                        && let Some(raw) = v.as_deref().map(str::trim)
                        && !raw.is_empty()
                    {
                        probe = Some(raw.to_string());
                    }
                    v
                }
                From::Segment(n) => segment(rel, *n).map(str::to_string),
            };
            let Some(value) = read.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
                sources.push((None, Outcome::Empty));
                continue;
            };
            let shape = Some(nils_dicom::diagnostic::shape(value));
            if answer.is_some() {
                // Read for the record; an earlier source already answered.
                sources.push((shape, Outcome::Unread));
                continue;
            }
            let id = match &source.pattern {
                None => Some(value),
                Some(re) => re
                    .captures(value)
                    .and_then(|c| c.name("id"))
                    .map(|m| m.as_str())
                    .filter(|id| !id.is_empty()),
            };
            match id.and_then(|id| self.taken(id)) {
                Some(id) => {
                    sources.push((shape, Outcome::Answered));
                    answer = Some((i, id));
                }
                None => {
                    sources.push((shape, Outcome::Unparsed));
                    if reserve.is_none() && self.reserve_source(&source.from) {
                        reserve = id.and_then(personnummer::reserve);
                    }
                }
            }
        }
        match answer {
            Some((i, value)) => Traced {
                sources,
                answered_by: Some(i),
                ident: Ident {
                    value,
                    probe,
                    fell_back: false,
                    reserve: None,
                },
            },
            None => Traced {
                sources,
                answered_by: None,
                ident: Ident {
                    value: study_uid.to_string(),
                    probe,
                    fell_back: true,
                    reserve,
                },
            },
        }
    }

    /// The label of every source, in order: `PatientID`, `path segment 1`.
    pub fn source_labels(&self) -> Vec<String> {
        self.from.iter().map(|s| s.from.label()).collect()
    }

    /// The field keywords of the rule, in order.
    pub fn field_keywords(&self) -> Vec<String> {
        self.from
            .iter()
            .filter_map(|s| match &s.from {
                From::Field(f) => Some(f.clone()),
                From::Segment(_) => None,
            })
            .collect()
    }

    pub fn id_type(&self) -> &str {
        &self.id_type
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
        let mut probe: Option<String> = None;
        let mut reserve: Option<String> = None;
        for source in &self.from {
            let read: Option<String> = match &source.from {
                From::Field(_) => {
                    let v = values.get(next_field).and_then(|v| v.clone());
                    next_field += 1;
                    if probe.is_none()
                        && let Some(raw) = v.as_deref().map(str::trim)
                        && !raw.is_empty()
                    {
                        probe = Some(raw.to_string());
                    }
                    v
                }
                From::Segment(n) => segment(rel, *n).map(str::to_string),
            };
            let Some(value) = read.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
                continue;
            };
            let id = match &source.pattern {
                None => Some(value),
                Some(re) => re
                    .captures(value)
                    .and_then(|c| c.name("id"))
                    .map(|m| m.as_str())
                    .filter(|id| !id.is_empty()),
            };
            match id.and_then(|id| self.taken(id)) {
                Some(id) => {
                    return Ident {
                        value: id,
                        probe,
                        fell_back: false,
                        reserve: None,
                    };
                }
                None => {
                    x.diagnostics.push(
                        Diagnostic::new(DiagnosticKind::IdentityUnparsed, source.from.label())
                            .with_shape(value),
                    );
                    if reserve.is_none() && self.reserve_source(&source.from) {
                        reserve = id.and_then(personnummer::reserve);
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
            probe,
            fell_back: true,
            reserve,
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
                probe: Some("P1".into()),
                fell_back: false,
                reserve: None
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

    /// The tax agency's published test numbers, which nobody holds.
    const PN: &str = "198501012382";
    const PN_OTHER: &str = "201501012395";

    #[test]
    fn a_personnummer_rule_files_twelve_digits_and_refuses_what_is_none() {
        let yaml = r"
identity:
  id_type: personnummer
  from:
    - field: PatientID
    - field: OtherPatientIDs
";
        let rule = Rule::parse(yaml).unwrap();
        assert!(rule.normalises());
        assert_eq!(
            rule.describe(),
            "as an ID that is the same everywhere: PatientID, OtherPatientIDs (as twelve \
             digits), then StudyInstanceUID"
        );
        assert_eq!(rule.to_json()["normalise"], "personnummer-12");
        // every written form is the one number
        for written in [
            PN.to_string(),
            format!("{}-{}", &PN[..8], &PN[8..]),
            format!("{}-{}", &PN[2..8], &PN[8..]),
            format!(" {} {} ", &PN[2..8], &PN[8..]),
        ] {
            let mut x = extracted(vec![Some(&written), None]);
            let ident = rule.apply(&mut x, "s/1.dcm");
            assert_eq!(ident.value, PN, "{written}");
            assert!(!ident.fell_back);
            assert!(x.diagnostics.is_empty());
            let traced = rule.trace(&[Some(written.clone()), None], "1.2.3", "s/1.dcm");
            assert_eq!(traced.ident.value, PN);
            assert_eq!(traced.sources[0].1, Outcome::Answered);
        }
        // a study's own id is no personnummer: unparsed, then the next field
        let mut x = extracted(vec![Some("TRIAL-0042"), Some(PN_OTHER)]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, PN_OTHER);
        assert_eq!(x.diagnostics.len(), 1);
        assert_eq!(x.diagnostics[0].kind, DiagnosticKind::IdentityUnparsed);
        // a wrong check digit too, and with nothing else the fallback
        let mut wrong = PN.to_string();
        wrong.replace_range(11.., "3");
        let mut x = extracted(vec![Some(&wrong), None]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert!(ident.fell_back);
        assert_eq!(rule.id_type_of(&ident), FALLBACK_ID_TYPE);
        assert_eq!(x.diagnostics.len(), 2);
        let traced = rule.trace(&[Some(wrong), None], "1.2.3", "s/1.dcm");
        assert!(traced.ident.fell_back);
        assert_eq!(traced.sources[0].1, Outcome::Unparsed);
    }

    /// Nima, 2026-10-16: under a rule that reads an ID that is the same
    /// everywhere in a dataset's originals, a PatientID that is none (a
    /// hospital's reserve number, twelve digits that name no date) is read
    /// as a reserve number in its one written form, beside the fallback,
    /// which the resolver looks up before the study and never codes. A valid
    /// personnummer answers as it always did, wherever it is; no other
    /// source, no other rule and no rule a digest reads reads a reserve
    /// number, and no rule reads one as its own type.
    #[test]
    fn a_patient_id_that_is_no_personnummer_is_a_reserve_number() {
        let mut rule =
            Rule::parse("identity:\n  id_type: personnummer\n  from:\n    - field: PatientID\n")
                .unwrap();
        // a digest's rule, as a file or a dataset stores it: as before
        assert!(!rule.reads_reserve);
        let mut x = extracted(vec![Some("19991324-1234")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert!(ident.fell_back && ident.reserve.is_none());
        assert_eq!(rule.id_type_of(&ident), FALLBACK_ID_TYPE);
        assert!(rule.to_json().get("reserve").is_none());
        assert!(!rule.describe().contains("reserve"));
        // the pseudonymiser's
        rule.reads_reserve = true;
        assert_eq!(
            rule.describe(),
            "as an ID that is the same everywhere: PatientID (as twelve digits), then \
             StudyInstanceUID; a PatientID that is no personnummer is a reserve number, found \
             through a map and never coded"
        );
        assert_eq!(rule.to_json()["reserve"], "reserve-number");
        // made up: twelve digits that name no date
        for written in ["199913241234", "19991324-1234", " 19991324 1234 "] {
            let mut x = extracted(vec![Some(written)]);
            let ident = rule.apply(&mut x, "s/1.dcm");
            assert!(ident.fell_back, "{written}");
            assert_eq!(ident.value, "1.2.826.0.1.3680043.8.498.1");
            assert_eq!(ident.reserve.as_deref(), Some("199913241234"));
            assert_eq!(ident.filed(), "199913241234");
            assert_eq!(rule.id_type_of(&ident), RESERVE_ID_TYPE);
            assert!(!rule.derives_by_generator(&ident));
            // what the rule read is said as before: unparsed, then the fallback
            let kinds: Vec<DiagnosticKind> = x.diagnostics.iter().map(|d| d.kind).collect();
            assert_eq!(
                kinds,
                [
                    DiagnosticKind::IdentityUnparsed,
                    DiagnosticKind::IdentityFallback
                ]
            );
            let traced = rule.trace(&[Some(written.to_string())], "1.2.3", "s/1.dcm");
            assert_eq!(traced.ident.reserve.as_deref(), Some("199913241234"));
            assert_eq!(traced.sources[0].1, Outcome::Unparsed);
        }
        // a valid personnummer is no reserve number; nothing, or nothing
        // that names anyone, is none either and falls back as before
        for (written, reserve) in [(Some(PN), None), (None, None), (Some(" - "), None)] {
            let mut x = extracted(vec![written]);
            let ident = rule.apply(&mut x, "s/1.dcm");
            assert_eq!(ident.reserve, reserve, "{written:?}");
            assert_eq!(ident.fell_back, written != Some(PN), "{written:?}");
        }
        // only PatientID's value is a reserve number, wherever it stands in
        // the rule, and a valid number another field holds answers before it
        let mut both = Rule::parse(
            "identity:\n  id_type: personnummer\n  from:\n    - field: OtherPatientIDs\n    - field: PatientID\n",
        )
        .unwrap();
        both.reads_reserve = true;
        let mut x = extracted(vec![Some("TRIAL-0042"), Some("19991324-1234")]);
        let ident = both.apply(&mut x, "s/1.dcm");
        assert_eq!(ident.reserve.as_deref(), Some("199913241234"));
        let mut x = extracted(vec![Some(PN_OTHER), Some("19991324-1234")]);
        let ident = both.apply(&mut x, "s/1.dcm");
        assert_eq!((ident.value.as_str(), ident.reserve), (PN_OTHER, None));
        let mut first = Rule::parse(
            "identity:\n  id_type: personnummer\n  from:\n    - field: PatientID\n    - field: OtherPatientIDs\n",
        )
        .unwrap();
        first.reads_reserve = true;
        let mut x = extracted(vec![Some("19991324-1234"), Some(PN_OTHER)]);
        let ident = first.apply(&mut x, "s/1.dcm");
        assert_eq!((ident.value.as_str(), ident.reserve), (PN_OTHER, None));
        // a rule of another type reads no reserve number, flag or none
        let other = Rule {
            reads_reserve: true,
            ..Rule::default()
        };
        let mut x = extracted(vec![Some("199913241234")]);
        assert_eq!(other.apply(&mut x, "s/1.dcm").reserve, None);
        assert!(other.to_json().get("reserve").is_none());
        // and no rule reads one as its own type
        let err =
            Rule::parse("identity:\n  id_type: reserve-number\n  from:\n    - field: PatientID\n")
                .unwrap_err()
                .0;
        assert!(err.contains("never as a rule's own type"), "{err}");
    }

    #[test]
    fn only_a_personnummer_rule_normalises() {
        // the default rule, and any other type, hash what they read
        let dashed = format!("{}-{}", &PN[2..8], &PN[8..]);
        let mut x = extracted(vec![Some(&dashed)]);
        let ident = Rule::default().apply(&mut x, "s/1.dcm");
        assert_eq!(ident.value, dashed);
        assert!(!Rule::default().normalises());
        assert!(Rule::default().to_json().get("normalise").is_none());
        // and a verbatim rule cannot be a personnummer rule
        let yaml = r"
identity:
  id_type: personnummer
  from:
    - field: PatientID
      pattern: '^(?<id>\d{12})$'
  code: verbatim
";
        assert!(
            Rule::parse(yaml)
                .unwrap_err()
                .0
                .contains("is no subject code")
        );
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
                .ends_with("; the value read is the subject code itself")
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

    /// Wave 7a §5.4: a rule that reads subject codes never reads a
    /// personnummer as one, although the code alphabet spells its digits; and
    /// a rule the engine writes for a dataset's declaration says that a file
    /// it cannot read is held, not filed under its study.
    #[test]
    fn a_rule_that_reads_codes_takes_no_personnummer_and_says_what_it_holds() {
        let mut rule = Rule::parse(
            "identity:\n  id_type: subject-code\n  from:\n    - field: PatientID\n      pattern: '^(?<id>[0-9a-hjkmnp-tv-z]+)$'\n  code: verbatim\n",
        )
        .unwrap();
        // the tax agency's published test numbers, twelve and ten digits
        for number in ["201501012395", "1501012395"] {
            let mut x = extracted(vec![Some(number)]);
            let ident = rule.apply(&mut x, "s/1.dcm");
            assert!(ident.fell_back, "{number}");
            assert!(
                x.diagnostics
                    .iter()
                    .any(|d| d.kind == DiagnosticKind::IdentityUnparsed),
                "{number}"
            );
        }
        // a code of digits that is no personnummer is still a code
        let mut x = extracted(vec![Some("123456789012")]);
        let ident = rule.apply(&mut x, "s/1.dcm");
        assert!(!ident.fell_back);
        assert_eq!(ident.value, "123456789012");
        // the default rule derives a code from a personnummer as from any ID
        let mut x = extracted(vec![Some("201501012395")]);
        assert!(!Rule::default().apply(&mut x, "s/1.dcm").fell_back);

        assert!(!rule.holds_unread);
        assert!(rule.describe().contains(", then StudyInstanceUID"));
        assert!(rule.to_json().get("unread").is_none());
        rule.holds_unread = true;
        assert!(
            rule.describe().contains(
                "; a value it cannot read is held, an empty one falls back to StudyInstanceUID"
            ),
            "{}",
            rule.describe()
        );
        assert!(!rule.describe().contains(", then StudyInstanceUID"));
        assert_eq!(rule.to_json()["unread"], "held");
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
