// SPDX-License-Identifier: AGPL-3.0-only

//! One declarative importer (`docs/specs/wave4a-engine-completes.md`, §7.2).
//!
//! v0 has thirteen importers, each a preview-then-apply CSV importer with its
//! own field parsers and validation. They differ in their targets and not in
//! their shape, so here there is one: a **mapping** names the target table,
//! how a row names its subject, the columns and their parsers, the key a row
//! is idempotent on, and what to do with a row whose key exists. A preview
//! shows what would change and applies nothing; an apply writes under a
//! principal; a re-run changes nothing.
//!
//! A date is parsed under a declared format and never guessed: v0 accepted
//! twelve formats, three of which cannot be told apart on most days of the
//! month, and a birth date read the wrong way round is an age that is wrong
//! for a lifetime.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::Deserialize;

use crate::day::Day;
use crate::home::{HomeError, Registry};
use crate::linkage::{self, Subkeys};
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// What went wrong with an import as a whole; a row that is wrong is an
/// outcome, not an error.
#[derive(Debug)]
pub enum Error {
    /// The mapping file does not describe an import the engine can run.
    Mapping(String),
    /// The CSV could not be read as one.
    Csv(String),
    Store(StoreError),
    Home(HomeError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Mapping(m) => write!(f, "mapping: {m}"),
            Error::Csv(m) => write!(f, "csv: {m}"),
            Error::Store(e) => write!(f, "{e}"),
            Error::Home(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

impl From<HomeError> for Error {
    fn from(e: HomeError) -> Error {
        Error::Home(e)
    }
}

/// What a mapping writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Event,
    Subject,
    Cohort,
    CohortMember,
    SubjectDisease,
    SubjectDiseaseType,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Event => "event",
            Target::Subject => "subject",
            Target::Cohort => "cohort",
            Target::CohortMember => "cohort_member",
            Target::SubjectDisease => "subject_disease",
            Target::SubjectDiseaseType => "subject_disease_type",
        }
    }

    /// The fields a row of this target may carry, and which are required.
    fn fields(self) -> &'static [(&'static str, bool)] {
        match self {
            Target::Event => &[
                ("event_date", true),
                ("event_time", false),
                ("value", false),
                ("unit", false),
                ("notes", false),
            ],
            Target::Subject => &[
                ("birth_date", false),
                ("sex", false),
                ("deceased_at", false),
            ],
            Target::Cohort => &[("name", true), ("owner", false), ("description", false)],
            Target::CohortMember => &[("notes", false)],
            Target::SubjectDisease => &[
                ("onset_date", false),
                ("diagnosis_date", false),
                ("notes", false),
                ("family_history", false),
            ],
            Target::SubjectDiseaseType => &[("assigned_on", false), ("notes", false)],
        }
    }

    /// Whether a row names a subject.
    fn has_subject(self) -> bool {
        self != Target::Cohort
    }
}

/// How a row names its subject.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SubjectRef {
    pub column: String,
    #[serde(default)]
    pub by: By,
    /// The identifier's type, when `by` is `identifier`.
    #[serde(default)]
    pub id_type: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// The registry's own code.
    #[default]
    Code,
    /// An identifier the linkage store holds (Wave 1 §7): the hospital's,
    /// the study's, whatever type the mapping names.
    Identifier,
}

/// A reference to a thing the registry names: a constant for every row, or a
/// column read per row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Ref {
    Value(String),
    Column { column: String },
}

/// Where one field of the target comes from.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Source {
    #[serde(default)]
    pub column: Option<String>,
    /// A constant, for every row; also the fallback when the column is empty.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub parser: Parser,
    /// For a date or a time: the format, as `%Y-%m-%d` or `%H:%M:%S`.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Parser {
    #[default]
    Text,
    Int,
    Float,
    Bool,
    Date,
    Time,
}

/// What to do with a row whose key the registry already holds.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnExisting {
    /// Leave it: the default, and what makes a re-run change nothing.
    #[default]
    Skip,
    /// Overwrite its fields in place.
    Update,
    /// Write a new row and mark the old one superseded by it (§13.2).
    Supersede,
}

/// The mapping, as `import.yml` declares it.
#[derive(Debug, Clone, Deserialize)]
pub struct Mapping {
    pub target: Target,
    #[serde(default)]
    pub subject: Option<SubjectRef>,
    #[serde(default)]
    pub observation_type: Option<Ref>,
    #[serde(default)]
    pub cohort: Option<Ref>,
    #[serde(default)]
    pub disease: Option<Ref>,
    #[serde(default)]
    pub disease_type: Option<Ref>,
    #[serde(default)]
    pub columns: BTreeMap<String, Source>,
    /// The fields, beside the subject and the reference, that make a row the
    /// same row as one the registry holds. The target's default when empty.
    #[serde(default)]
    pub key: Vec<String>,
    #[serde(default)]
    pub on_existing: OnExisting,
    /// A label recorded on every row written, for `event.source`.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Deserialize)]
struct File {
    import: Mapping,
}

impl Mapping {
    /// Parse the mapping and refuse one the engine could not run: a target
    /// with no subject, a required field with no source, a source with
    /// neither a column nor a value, a key naming a field the mapping does
    /// not have, a date with no format.
    pub fn parse(yaml: &str) -> Result<Mapping, Error> {
        let file: File = serde_saphyr::from_str(yaml).map_err(|e| Error::Mapping(e.to_string()))?;
        let m = file.import;
        if m.target.has_subject() && m.subject.is_none() {
            return Err(Error::Mapping(format!(
                "a {} row names a subject, and the mapping says how under `subject`",
                m.target.name()
            )));
        }
        if let Some(s) = &m.subject
            && s.by == By::Identifier
            && s.id_type.as_deref().unwrap_or("").is_empty()
        {
            return Err(Error::Mapping(
                "subject.by is identifier, so subject.id_type names the identifier's type".into(),
            ));
        }
        let needs = |what: &str, r: &Option<Ref>| -> Result<(), Error> {
            match r {
                Some(_) => Ok(()),
                None => Err(Error::Mapping(format!(
                    "a {} row needs `{what}`, a constant or a column",
                    m.target.name()
                ))),
            }
        };
        match m.target {
            Target::Event => needs("observation_type", &m.observation_type)?,
            Target::CohortMember => needs("cohort", &m.cohort)?,
            Target::SubjectDisease => needs("disease", &m.disease)?,
            Target::SubjectDiseaseType => {
                needs("disease", &m.disease)?;
                needs("disease_type", &m.disease_type)?;
            }
            Target::Subject | Target::Cohort => {}
        }
        let known: Vec<&str> = m.target.fields().iter().map(|(f, _)| *f).collect();
        for (field, src) in &m.columns {
            if !known.contains(&field.as_str()) {
                return Err(Error::Mapping(format!(
                    "{field} is not a field of {}; those are {}",
                    m.target.name(),
                    known.join(", ")
                )));
            }
            if src.column.as_deref().unwrap_or("").is_empty()
                && src.value.as_deref().unwrap_or("").is_empty()
            {
                return Err(Error::Mapping(format!(
                    "columns.{field} has neither a column nor a value"
                )));
            }
            if matches!(src.parser, Parser::Date | Parser::Time)
                && src.format.as_deref().unwrap_or("").is_empty()
            {
                return Err(Error::Mapping(format!(
                    "columns.{field} is a {}, so it declares its format; a date is never guessed",
                    match src.parser {
                        Parser::Date => "date",
                        _ => "time",
                    }
                )));
            }
        }
        for (field, required) in m.target.fields() {
            if *required && !m.columns.contains_key(*field) {
                return Err(Error::Mapping(format!(
                    "a {} row needs {field}",
                    m.target.name()
                )));
            }
        }
        if m.target == Target::Subject && m.columns.is_empty() {
            return Err(Error::Mapping(
                "a subject row carries at least one of birth_date, sex, deceased_at".into(),
            ));
        }
        for k in &m.key {
            if !m.columns.contains_key(k) {
                return Err(Error::Mapping(format!(
                    "key names {k}, which the mapping does not have"
                )));
            }
        }
        // A date field is a date whatever the mapping said.
        for (field, src) in &m.columns {
            let is_date =
                field.ends_with("_date") || *field == "deceased_at" || *field == "assigned_on";
            if is_date && src.parser != Parser::Date {
                return Err(Error::Mapping(format!(
                    "columns.{field} is a date: parser: date, with its format"
                )));
            }
            if *field == "event_time" && src.parser != Parser::Time {
                return Err(Error::Mapping(
                    "columns.event_time is a time: parser: time, with its format".into(),
                ));
            }
        }
        Ok(m)
    }

    /// The key the target is idempotent on, beside the subject and the
    /// reference: the mapping's, or the target's default.
    fn key_fields(&self) -> Vec<String> {
        if !self.key.is_empty() {
            return self.key.clone();
        }
        match self.target {
            Target::Event => vec!["event_date".to_string()],
            Target::Cohort => vec!["name".to_string()],
            _ => Vec::new(),
        }
    }
}

/// A parsed field.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// `YYYY-MM-DD`.
    Date(String),
    /// `HH:MM:SS`.
    Time(String),
}

impl Value {
    fn text(&self) -> String {
        match self {
            Value::Text(s) | Value::Date(s) | Value::Time(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{f}"),
            Value::Bool(b) => b.to_string(),
        }
    }

    fn param(&self) -> Param {
        match self {
            Value::Text(s) | Value::Date(s) | Value::Time(s) => Param::from(s.as_str()),
            Value::Int(i) => Param::Int(*i),
            Value::Float(f) => Param::Double(*f),
            Value::Bool(b) => Param::Bool(*b),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Text(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
}

/// Parse one raw cell under its source's parser.
fn parse(src: &Source, raw: &str) -> Result<Value, String> {
    let raw = raw.trim();
    match src.parser {
        Parser::Text => Ok(Value::Text(raw.to_string())),
        Parser::Int => raw
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("{raw} is not an integer")),
        Parser::Float => raw
            .replace(',', ".")
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("{raw} is not a number")),
        Parser::Bool => match raw.to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "yes" | "y" => Ok(Value::Bool(true)),
            "0" | "false" | "f" | "no" | "n" => Ok(Value::Bool(false)),
            _ => Err(format!("{raw} is not a yes or a no")),
        },
        Parser::Date => {
            let f = src.format.as_deref().unwrap_or("%Y-%m-%d");
            let parts = strptime(raw, f)?;
            let (y, m, d) = (
                parts.get(&'Y').copied().ok_or("the format has no %Y")?,
                parts.get(&'m').copied().ok_or("the format has no %m")?,
                parts.get(&'d').copied().ok_or("the format has no %d")?,
            );
            Day::new(y as i32, m as u32, d as u32)
                .map(|_| Value::Date(format!("{y:04}-{m:02}-{d:02}")))
                .ok_or_else(|| format!("{raw} is not a day of the calendar"))
        }
        Parser::Time => {
            let f = src.format.as_deref().unwrap_or("%H:%M:%S");
            let parts = strptime(raw, f)?;
            let (h, mi, s) = (
                parts.get(&'H').copied().ok_or("the format has no %H")?,
                parts.get(&'M').copied().unwrap_or(0),
                parts.get(&'S').copied().unwrap_or(0),
            );
            if h > 23 || mi > 59 || s > 59 {
                return Err(format!("{raw} is not a time of day"));
            }
            Ok(Value::Time(format!("{h:02}:{mi:02}:{s:02}")))
        }
    }
}

/// The digits of each `%` token of `format`, read off `text`. Tokens: `%Y`
/// (four digits), `%m`, `%d`, `%H`, `%M`, `%S` (one or two); anything else
/// in the format is a literal the text must carry.
fn strptime(text: &str, format: &str) -> Result<HashMap<char, i64>, String> {
    let mut out = HashMap::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let mut f = format.chars().peekable();
    while let Some(c) = f.next() {
        if c == '%' {
            let Some(token) = f.next() else {
                return Err(format!("format {format} ends in %"));
            };
            let width = if token == 'Y' { 4 } else { 2 };
            let mut digits = String::new();
            while at < bytes.len() && bytes[at].is_ascii_digit() && digits.len() < width {
                digits.push(bytes[at]);
                at += 1;
            }
            if digits.is_empty() {
                return Err(format!("{text} does not read as {format}"));
            }
            let n: i64 = digits
                .parse()
                .map_err(|_| format!("{text} does not read as {format}"))?;
            out.insert(token, n);
        } else {
            if at >= bytes.len() || bytes[at] != c {
                return Err(format!("{text} does not read as {format}"));
            }
            at += 1;
        }
    }
    if at != bytes.len() {
        return Err(format!("{text} does not read as {format}"));
    }
    Ok(out)
}

/// What became of one row.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Added,
    Skipped {
        existing: i64,
    },
    Updated {
        existing: i64,
    },
    Superseded {
        existing: i64,
    },
    /// A field the registry already holds another value for: a review item,
    /// and the registry's value stands (§13.3).
    Reviewed {
        field: String,
    },
    Refused(String),
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Added => "added",
            Verdict::Skipped { .. } => "skipped",
            Verdict::Updated { .. } => "updated",
            Verdict::Superseded { .. } => "superseded",
            Verdict::Reviewed { .. } => "reviewed",
            Verdict::Refused(_) => "refused",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The row's number in the file, from one, the header not counted.
    pub row: usize,
    /// The subject's code, when the row named one the registry knows.
    pub subject: Option<String>,
    pub verdict: Verdict,
}

/// What an import did, or would do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub target: String,
    pub applied: bool,
    pub rows: usize,
    pub added: usize,
    pub skipped: usize,
    pub updated: usize,
    pub superseded: usize,
    pub reviewed: usize,
    /// Refused rows by reason, so that a file with a thousand bad dates is
    /// one line and not a thousand.
    pub refused: BTreeMap<String, usize>,
    /// The first rows' outcomes, for a person to read before applying.
    pub samples: Vec<Outcome>,
}

impl Report {
    pub fn refused_total(&self) -> usize {
        self.refused.values().sum()
    }

    /// Whether an apply would change anything.
    pub fn changes(&self) -> usize {
        self.added + self.updated + self.superseded + self.reviewed
    }
}

/// How many outcomes a report keeps in full.
const SAMPLES: usize = 20;

/// Show what an import would do, and write nothing.
pub fn preview(
    registry: &mut Registry,
    mapping: &Mapping,
    csv: &str,
    actor: &str,
) -> Result<Report, Error> {
    run(registry, mapping, csv, actor, false)
}

/// Apply an import under a principal, in one transaction.
pub fn apply(
    registry: &mut Registry,
    mapping: &Mapping,
    csv: &str,
    actor: &str,
) -> Result<Report, Error> {
    run(registry, mapping, csv, actor, true)
}

/// One row, parsed: the subject's key as written, the reference names, and
/// the fields.
struct Parsed {
    row: usize,
    subject: Option<String>,
    reference: Option<String>,
    reference2: Option<String>,
    fields: BTreeMap<String, Value>,
}

fn read_rows(mapping: &Mapping, csv_text: &str) -> Result<(Vec<Parsed>, Vec<Outcome>), Error> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(csv_text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| Error::Csv(e.to_string()))?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let index = |name: &str| -> Option<usize> {
        headers
            .iter()
            .position(|h| h == name)
            .or_else(|| headers.iter().position(|h| h.eq_ignore_ascii_case(name)))
    };
    // Every column the mapping names must be there, before a row is read.
    let mut named: Vec<&str> = mapping
        .columns
        .values()
        .filter_map(|s| s.column.as_deref())
        .collect();
    if let Some(s) = &mapping.subject {
        named.push(&s.column);
    }
    for r in [
        &mapping.observation_type,
        &mapping.cohort,
        &mapping.disease,
        &mapping.disease_type,
    ]
    .into_iter()
    .flatten()
    {
        if let Ref::Column { column } = r {
            named.push(column);
        }
    }
    for c in &named {
        if index(c).is_none() {
            return Err(Error::Csv(format!(
                "the file has no column {c}; its columns are {}",
                headers.join(", ")
            )));
        }
    }
    let cell = |record: &csv::StringRecord, name: &str| -> String {
        index(name)
            .and_then(|i| record.get(i))
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let reference = |record: &csv::StringRecord, r: &Option<Ref>| -> Option<String> {
        match r {
            Some(Ref::Value(v)) => Some(v.clone()),
            Some(Ref::Column { column }) => Some(cell(record, column)),
            None => None,
        }
    };
    let mut rows = Vec::new();
    let mut refused = Vec::new();
    for (i, record) in reader.records().enumerate() {
        let n = i + 1;
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                refused.push(Outcome {
                    row: n,
                    subject: None,
                    verdict: Verdict::Refused(format!("not a row: {e}")),
                });
                continue;
            }
        };
        let subject = mapping.subject.as_ref().map(|s| cell(&record, &s.column));
        if let Some(s) = &subject
            && s.is_empty()
        {
            refused.push(Outcome {
                row: n,
                subject: None,
                verdict: Verdict::Refused("no subject".into()),
            });
            continue;
        }
        let (reference, reference2) = match mapping.target {
            Target::Event => (reference(&record, &mapping.observation_type), None),
            Target::CohortMember => (reference(&record, &mapping.cohort), None),
            Target::SubjectDisease => (reference(&record, &mapping.disease), None),
            Target::SubjectDiseaseType => (
                reference(&record, &mapping.disease),
                reference(&record, &mapping.disease_type),
            ),
            Target::Subject | Target::Cohort => (None, None),
        };
        let mut fields = BTreeMap::new();
        let mut bad = None;
        for (field, src) in &mapping.columns {
            let mut raw = src
                .column
                .as_deref()
                .map(|c| cell(&record, c))
                .unwrap_or_default();
            if raw.is_empty()
                && let Some(v) = &src.value
            {
                raw = v.clone();
            }
            if raw.is_empty() {
                continue;
            }
            match parse(src, &raw) {
                Ok(v) => {
                    fields.insert(field.clone(), v);
                }
                Err(why) => {
                    bad = Some(format!("{field}: {why}"));
                    break;
                }
            }
        }
        if let Some(why) = bad {
            refused.push(Outcome {
                row: n,
                subject: None,
                verdict: Verdict::Refused(why),
            });
            continue;
        }
        for (field, required) in mapping.target.fields() {
            if *required && !fields.contains_key(*field) {
                bad = Some(format!("no {field}"));
                break;
            }
        }
        if let Some(why) = bad {
            refused.push(Outcome {
                row: n,
                subject: None,
                verdict: Verdict::Refused(why),
            });
            continue;
        }
        rows.push(Parsed {
            row: n,
            subject,
            reference,
            reference2,
            fields,
        });
    }
    Ok((rows, refused))
}

/// The subjects the rows name, resolved to ids: by the registry's code, or
/// by an identifier through the linkage store, which is the Wave 1 way and
/// the only one that works without the key that made the codes.
fn resolve_subjects(
    registry: &mut Registry,
    mapping: &Mapping,
    rows: &[Parsed],
) -> Result<HashMap<String, (i64, String)>, Error> {
    let Some(subject) = &mapping.subject else {
        return Ok(HashMap::new());
    };
    let mut wanted: Vec<String> = rows.iter().filter_map(|r| r.subject.clone()).collect();
    wanted.sort();
    wanted.dedup();
    let mut out: HashMap<String, (i64, String)> = HashMap::new();
    match subject.by {
        By::Code => {
            for s in linkage::subjects_by_code(registry.store(), &wanted)? {
                out.insert(s.code.clone(), (s.id, s.code));
            }
        }
        By::Identifier => {
            let id_type = subject.id_type.clone().unwrap_or_default();
            let key = registry.pseudonym_key()?;
            let keys = Subkeys::derive(&key);
            let mut linkage_store = registry.open_linkage()?;
            let Some(type_id) = linkage::id_type_id(&mut linkage_store, &id_type)? else {
                return Err(Error::Mapping(format!(
                    "the linkage store has no identifier type named {id_type}"
                )));
            };
            let lookups: Vec<Vec<u8>> = wanted.iter().map(|v| keys.lookup(&id_type, v)).collect();
            let by_lookup: HashMap<Vec<u8>, i64> =
                linkage::identities_by_lookup(&mut linkage_store, &lookups)?
                    .into_iter()
                    .filter(|i| i.id_type_id == type_id)
                    .map(|i| (i.lookup, i.subject_id))
                    .collect();
            let ids: Vec<i64> = by_lookup.values().copied().collect();
            let codes: HashMap<i64, String> = linkage::subjects_by_id(registry.store(), &ids)?
                .into_iter()
                .map(|s| (s.id, s.code))
                .collect();
            for (value, lookup) in wanted.iter().zip(&lookups) {
                if let Some(id) = by_lookup.get(lookup)
                    && let Some(code) = codes.get(id)
                {
                    out.insert(value.clone(), (*id, code.clone()));
                }
            }
        }
    }
    Ok(out)
}

/// A named thing's row id, by name, folded on case.
fn id_by_name(store: &mut Store, t: &str, name: &str) -> Result<Option<i64>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE LOWER(name) = LOWER({})",
        store.qualified(t),
        d.param(1, Type::Text)
    );
    Ok(store
        .query_opt(&sql, &[Param::from(name)])?
        .map(|r| r.int(0))
        .transpose()?)
}

fn run(
    registry: &mut Registry,
    mapping: &Mapping,
    csv_text: &str,
    actor: &str,
    do_apply: bool,
) -> Result<Report, Error> {
    let (rows, refused_rows) = read_rows(mapping, csv_text)?;
    let subjects = resolve_subjects(registry, mapping, &rows)?;
    let mut report = Report {
        target: mapping.target.name().to_string(),
        applied: do_apply,
        rows: rows.len() + refused_rows.len(),
        ..Report::default()
    };
    let mut outcomes: Vec<Outcome> = refused_rows;

    // The references, resolved once each.
    let mut kinds: HashMap<String, (i64, Option<String>)> = HashMap::new();
    let mut cohorts: HashMap<String, i64> = HashMap::new();
    let mut diseases: HashMap<String, i64> = HashMap::new();
    let mut disease_types: HashMap<(i64, String), i64> = HashMap::new();
    {
        let store = registry.store();
        let d = store.dialect();
        for k in crate::clinical::observation_types(store)? {
            kinds.insert(k.name.to_lowercase(), (k.id, k.value_type.clone()));
        }
        let mut names: Vec<String> = rows.iter().filter_map(|r| r.reference.clone()).collect();
        names.sort();
        names.dedup();
        for name in &names {
            match mapping.target {
                Target::CohortMember => {
                    if let Some(id) = id_by_name(store, "cohort", name)? {
                        cohorts.insert(name.to_lowercase(), id);
                    }
                }
                Target::SubjectDisease | Target::SubjectDiseaseType => {
                    if let Some(id) = id_by_name(store, "disease", name)? {
                        diseases.insert(name.to_lowercase(), id);
                    }
                }
                _ => {}
            }
        }
        if mapping.target == Target::SubjectDiseaseType {
            let sql = format!(
                "SELECT id FROM {} WHERE disease_id = {} AND LOWER(name) = LOWER({})",
                store.qualified("disease_type"),
                d.param(1, Type::Int),
                d.param(2, Type::Text)
            );
            let mut pairs: Vec<(String, String)> = rows
                .iter()
                .filter_map(|r| Some((r.reference.clone()?, r.reference2.clone()?)))
                .collect();
            pairs.sort();
            pairs.dedup();
            for (disease, kind) in pairs {
                if let Some(did) = diseases.get(&disease.to_lowercase())
                    && let Some(r) =
                        store.query_opt(&sql, &[Param::Int(*did), Param::from(kind.as_str())])?
                {
                    disease_types.insert((*did, kind.to_lowercase()), r.int(0)?);
                }
            }
        }
    }

    let now = now_iso();
    let key_fields = mapping.key_fields();
    let store = registry.store();
    if do_apply {
        store.begin()?;
    }
    let result = (|| -> Result<(), Error> {
        for r in &rows {
            let subject = match (&r.subject, mapping.target.has_subject()) {
                (Some(s), true) => match subjects.get(s) {
                    Some(found) => Some(found.clone()),
                    None => {
                        outcomes.push(Outcome {
                            row: r.row,
                            subject: None,
                            verdict: Verdict::Refused(
                                "a subject the registry does not know".into(),
                            ),
                        });
                        continue;
                    }
                },
                _ => None,
            };
            let code = subject.as_ref().map(|(_, c)| c.clone());
            let verdicts = match mapping.target {
                Target::Event => {
                    let name = r.reference.clone().unwrap_or_default();
                    match kinds.get(&name.to_lowercase()) {
                        Some((kind, value_type)) => vec![write_event(
                            store,
                            mapping,
                            r,
                            subject.as_ref().map(|(id, _)| *id).unwrap_or(0),
                            *kind,
                            value_type.as_deref(),
                            &key_fields,
                            actor,
                            &now,
                            do_apply,
                        )?],
                        None => vec![Verdict::Refused(format!(
                            "no observation kind named {name}; load the vocabulary first"
                        ))],
                    }
                }
                Target::Subject => write_subject(
                    store,
                    r,
                    subject.as_ref().map(|(id, _)| *id).unwrap_or(0),
                    actor,
                    &now,
                    do_apply,
                )?,
                Target::Cohort => vec![write_cohort(store, mapping, r, &now, do_apply)?],
                Target::CohortMember => {
                    let name = r.reference.clone().unwrap_or_default();
                    match cohorts.get(&name.to_lowercase()) {
                        Some(cohort) => vec![write_member(
                            store,
                            r,
                            *cohort,
                            subject.as_ref().map(|(id, _)| *id).unwrap_or(0),
                            &now,
                            do_apply,
                        )?],
                        None => vec![Verdict::Refused(format!("no cohort named {name}"))],
                    }
                }
                Target::SubjectDisease => {
                    let name = r.reference.clone().unwrap_or_default();
                    match diseases.get(&name.to_lowercase()) {
                        Some(disease) => vec![write_subject_disease(
                            store,
                            mapping,
                            r,
                            subject.as_ref().map(|(id, _)| *id).unwrap_or(0),
                            *disease,
                            &kinds,
                            actor,
                            &now,
                            do_apply,
                        )?],
                        None => vec![Verdict::Refused(format!("no disease named {name}"))],
                    }
                }
                Target::SubjectDiseaseType => {
                    let name = r.reference.clone().unwrap_or_default();
                    let kind = r.reference2.clone().unwrap_or_default();
                    match diseases.get(&name.to_lowercase()).and_then(|d| {
                        disease_types
                            .get(&(*d, kind.to_lowercase()))
                            .map(|t| (*d, *t))
                    }) {
                        Some((disease, dtype)) => vec![write_subject_disease_type(
                            store,
                            mapping,
                            r,
                            subject.as_ref().map(|(id, _)| *id).unwrap_or(0),
                            disease,
                            dtype,
                            actor,
                            &now,
                            do_apply,
                        )?],
                        None => vec![Verdict::Refused(format!("no type {kind} of {name}"))],
                    }
                }
            };
            for verdict in verdicts {
                outcomes.push(Outcome {
                    row: r.row,
                    subject: code.clone(),
                    verdict,
                });
            }
        }
        Ok(())
    })();
    match (result, do_apply) {
        (Ok(()), true) => store.commit()?,
        (Err(e), true) => {
            store.rollback().ok();
            return Err(e);
        }
        (Err(e), false) => return Err(e),
        (Ok(()), false) => {}
    }
    outcomes.sort_by_key(|o| o.row);
    for o in &outcomes {
        match &o.verdict {
            Verdict::Added => report.added += 1,
            Verdict::Skipped { .. } => report.skipped += 1,
            Verdict::Updated { .. } => report.updated += 1,
            Verdict::Superseded { .. } => report.superseded += 1,
            Verdict::Reviewed { .. } => report.reviewed += 1,
            Verdict::Refused(why) => *report.refused.entry(why.clone()).or_insert(0) += 1,
        }
    }
    report.samples = outcomes.into_iter().take(SAMPLES).collect();
    Ok(report)
}

fn opt_param(v: Option<&Value>) -> Param {
    match v {
        Some(v) => v.param(),
        None => Param::Null,
    }
}

fn opt_text(v: Option<&Value>) -> Param {
    match v {
        Some(v) => Param::from(v.text()),
        None => Param::Null,
    }
}

#[allow(clippy::too_many_arguments)]
fn write_event(
    store: &mut Store,
    mapping: &Mapping,
    r: &Parsed,
    subject: i64,
    kind: i64,
    value_type: Option<&str>,
    key_fields: &[String],
    actor: &str,
    now: &str,
    do_apply: bool,
) -> Result<Verdict, Error> {
    let date = r
        .fields
        .get("event_date")
        .map(Value::text)
        .unwrap_or_default();
    let value = r.fields.get("value");
    if value_type.is_some() && value.is_none() {
        return Ok(Verdict::Refused(format!(
            "the kind carries a {} value and the row has none",
            value_type.unwrap_or("")
        )));
    }
    let number = match value_type {
        Some("numeric") => match value.and_then(Value::number) {
            Some(n) => Some(n),
            None => {
                return Ok(Verdict::Refused(
                    "the kind is numeric and the value is not a number".into(),
                ));
            }
        },
        _ => None,
    };
    let d = store.dialect();
    // The existing row: the subject, the kind, the date, and whatever else
    // the key names.
    let mut sql = format!(
        "SELECT id FROM {} WHERE subject_id = {} AND observation_type_id = {} AND event_date = {} \
         AND superseded_by IS NULL",
        store.qualified("event"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Date),
    );
    let mut params = vec![
        Param::Int(subject),
        Param::Int(kind),
        Param::from(date.as_str()),
    ];
    for k in key_fields.iter().filter(|k| *k != "event_date") {
        let (column, ty) = match k.as_str() {
            "event_time" => ("event_time", Type::Time),
            "value" => ("value", Type::Text),
            "unit" => ("unit", Type::Text),
            "notes" => ("notes", Type::Text),
            _ => continue,
        };
        params.push(opt_text(r.fields.get(k)));
        sql.push_str(&format!(" AND {column} = {}", d.param(params.len(), ty)));
    }
    sql.push_str(" ORDER BY id DESC LIMIT 1");
    let existing = store
        .query_opt(&sql, &params)?
        .map(|row| row.int(0))
        .transpose()?;
    let columns = [
        "subject_id",
        "observation_type_id",
        "event_date",
        "event_time",
        "value",
        "number",
        "unit",
        "source",
        "notes",
        "created_at",
        "actor",
    ];
    let row = || {
        vec![
            Param::Int(subject),
            Param::Int(kind),
            Param::from(date.as_str()),
            opt_text(r.fields.get("event_time")),
            opt_text(value),
            match number {
                Some(n) => Param::Double(n),
                None => Param::Null,
            },
            opt_text(r.fields.get("unit")),
            match &mapping.source {
                Some(s) => Param::from(s.as_str()),
                None => Param::Null,
            },
            opt_text(r.fields.get("notes")),
            Param::from(now),
            Param::from(actor),
        ]
    };
    match (existing, mapping.on_existing) {
        (Some(id), OnExisting::Skip) => Ok(Verdict::Skipped { existing: id }),
        (Some(id), OnExisting::Update) => {
            if do_apply {
                store.update_by_id(
                    table("event"),
                    &[
                        ("event_time", opt_text(r.fields.get("event_time"))),
                        ("value", opt_text(value)),
                        (
                            "number",
                            match number {
                                Some(n) => Param::Double(n),
                                None => Param::Null,
                            },
                        ),
                        ("unit", opt_text(r.fields.get("unit"))),
                        ("notes", opt_text(r.fields.get("notes"))),
                        ("actor", Param::from(actor)),
                    ],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Updated { existing: id })
        }
        (Some(id), OnExisting::Supersede) => {
            if do_apply {
                let new = insert_returning(store, "event", &columns, row())?;
                store.update_by_id(
                    table("event"),
                    &[("superseded_by", Param::Int(new))],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Superseded { existing: id })
        }
        (None, _) => {
            if do_apply {
                insert_returning(store, "event", &columns, row())?;
            }
            Ok(Verdict::Added)
        }
    }
}

fn insert_returning(
    store: &mut Store,
    t: &str,
    columns: &[&str],
    row: Vec<Param>,
) -> Result<i64, Error> {
    let rows = store.insert(&Insert::new(table(t), columns).returning(&["id"]), &[row])?;
    Ok(rows.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

/// Demographics: a null is filled, an equal value is a skip, a different
/// value is a review item and the registry's value stands (§13.3).
fn write_subject(
    store: &mut Store,
    r: &Parsed,
    subject: i64,
    actor: &str,
    now: &str,
    do_apply: bool,
) -> Result<Vec<Verdict>, Error> {
    let d = store.dialect();
    let t = table("subject");
    let birth = d.text_of(t.column("birth_date").expect("subject.birth_date"));
    let died = d.text_of(t.column("deceased_at").expect("subject.deceased_at"));
    let sql = format!(
        "SELECT {birth}, sex, {died} FROM {} WHERE id = {}",
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    let Some(row) = store.query_opt(&sql, &[Param::Int(subject)])? else {
        return Ok(vec![Verdict::Refused(
            "a subject the registry does not know".into(),
        )]);
    };
    let held: [(&str, Option<String>); 3] = [
        ("birth_date", row.opt_text(0)?.map(str::to_string)),
        ("sex", row.opt_text(1)?.map(str::to_string)),
        ("deceased_at", row.opt_text(2)?.map(str::to_string)),
    ];
    let mut out = Vec::new();
    let mut sets: Vec<(&str, Param)> = Vec::new();
    for (field, current) in &held {
        let Some(new) = r.fields.get(*field) else {
            continue;
        };
        let new_text = match field {
            &"sex" => new.text().trim().to_uppercase(),
            _ => new.text(),
        };
        match current {
            None => {
                sets.push((field, Param::from(new_text.as_str())));
                out.push(Verdict::Added);
            }
            Some(c) if c.eq_ignore_ascii_case(&new_text) => {
                out.push(Verdict::Skipped { existing: subject });
            }
            Some(c) => {
                if do_apply {
                    store.insert(
                        &Insert::new(
                            table("review_item"),
                            &["kind", "scope", "ref", "evidence", "status", "created_at"],
                        ),
                        &[vec![
                            Param::from("subject.demographics"),
                            Param::from("subject"),
                            Param::from(serde_json::json!({"subject_id": subject}).to_string()),
                            Param::from(
                                serde_json::json!({
                                    "field": field,
                                    "registry": c,
                                    "file": new_text,
                                    "actor": actor,
                                    "at": now,
                                })
                                .to_string(),
                            ),
                            Param::from("open"),
                            Param::from(now),
                        ]],
                    )?;
                }
                out.push(Verdict::Reviewed {
                    field: field.to_string(),
                });
            }
        }
    }
    if do_apply && !sets.is_empty() {
        store.update_by_id(t, &sets, "id", subject)?;
    }
    if out.is_empty() {
        out.push(Verdict::Refused("the row carries no demographic".into()));
    }
    Ok(out)
}

fn write_cohort(
    store: &mut Store,
    mapping: &Mapping,
    r: &Parsed,
    now: &str,
    do_apply: bool,
) -> Result<Verdict, Error> {
    let name = r.fields.get("name").map(Value::text).unwrap_or_default();
    let existing = id_by_name(store, "cohort", &name)?;
    let owner = r
        .fields
        .get("owner")
        .map(Value::text)
        .unwrap_or_else(|| "unknown".to_string());
    match (existing, mapping.on_existing) {
        (Some(id), OnExisting::Skip | OnExisting::Supersede) => {
            Ok(Verdict::Skipped { existing: id })
        }
        (Some(id), OnExisting::Update) => {
            if do_apply {
                store.update_by_id(
                    table("cohort"),
                    &[
                        ("owner", Param::from(owner.as_str())),
                        ("description", opt_text(r.fields.get("description"))),
                    ],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Updated { existing: id })
        }
        (None, _) => {
            if do_apply {
                store.insert(
                    &Insert::new(
                        table("cohort"),
                        &["name", "owner", "description", "created_at"],
                    ),
                    &[vec![
                        Param::from(name.as_str()),
                        Param::from(owner.as_str()),
                        opt_text(r.fields.get("description")),
                        Param::from(now),
                    ]],
                )?;
            }
            Ok(Verdict::Added)
        }
    }
}

fn write_member(
    store: &mut Store,
    r: &Parsed,
    cohort: i64,
    subject: i64,
    now: &str,
    do_apply: bool,
) -> Result<Verdict, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE cohort_id = {} AND subject_id = {} AND left_at IS NULL",
        store.qualified("cohort_member"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    if let Some(row) = store.query_opt(&sql, &[Param::Int(cohort), Param::Int(subject)])? {
        return Ok(Verdict::Skipped {
            existing: row.int(0)?,
        });
    }
    if do_apply {
        store.insert(
            &Insert::new(
                table("cohort_member"),
                &["cohort_id", "subject_id", "joined_at", "notes"],
            ),
            &[vec![
                Param::Int(cohort),
                Param::Int(subject),
                Param::from(now),
                opt_text(r.fields.get("notes")),
            ]],
        )?;
    }
    Ok(Verdict::Added)
}

/// An event of a kind, for a subject on a date, made if absent: how a
/// subject's disease points at its onset and its diagnosis.
fn event_of(
    store: &mut Store,
    subject: i64,
    kind: i64,
    date: &str,
    actor: &str,
    now: &str,
    do_apply: bool,
) -> Result<Option<i64>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE subject_id = {} AND observation_type_id = {} AND event_date = {} \
         AND superseded_by IS NULL ORDER BY id DESC LIMIT 1",
        store.qualified("event"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Date)
    );
    if let Some(row) = store.query_opt(
        &sql,
        &[Param::Int(subject), Param::Int(kind), Param::from(date)],
    )? {
        return Ok(Some(row.int(0)?));
    }
    if !do_apply {
        return Ok(None);
    }
    let id = insert_returning(
        store,
        "event",
        &[
            "subject_id",
            "observation_type_id",
            "event_date",
            "created_at",
            "actor",
        ],
        vec![
            Param::Int(subject),
            Param::Int(kind),
            Param::from(date),
            Param::from(now),
            Param::from(actor),
        ],
    )?;
    Ok(Some(id))
}

#[allow(clippy::too_many_arguments)]
fn write_subject_disease(
    store: &mut Store,
    mapping: &Mapping,
    r: &Parsed,
    subject: i64,
    disease: i64,
    kinds: &HashMap<String, (i64, Option<String>)>,
    actor: &str,
    now: &str,
    do_apply: bool,
) -> Result<Verdict, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE subject_id = {} AND disease_id = {} AND superseded_by IS NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("subject_disease"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let existing = store
        .query_opt(&sql, &[Param::Int(subject), Param::Int(disease)])?
        .map(|row| row.int(0))
        .transpose()?;
    // The onset and the diagnosis are events of their kinds, made if absent,
    // so the dates are where every other date is.
    let mut onset = None;
    let mut diagnosis = None;
    for (field, kind_name, slot) in [
        ("onset_date", "disease onset", &mut onset),
        ("diagnosis_date", "diagnosis", &mut diagnosis),
    ] {
        if let Some(date) = r.fields.get(field).map(Value::text) {
            let Some((kind, _)) = kinds.get(kind_name) else {
                return Ok(Verdict::Refused(format!(
                    "no observation kind named {kind_name}; load the vocabulary first"
                )));
            };
            *slot = event_of(store, subject, *kind, &date, actor, now, do_apply)?;
        }
    }
    let columns = [
        "subject_id",
        "disease_id",
        "onset_event_id",
        "diagnosis_event_id",
        "notes",
        "family_history",
        "created_at",
        "actor",
    ];
    let row = |onset: Option<i64>, diagnosis: Option<i64>| {
        vec![
            Param::Int(subject),
            Param::Int(disease),
            onset.map_or(Param::Null, Param::Int),
            diagnosis.map_or(Param::Null, Param::Int),
            opt_text(r.fields.get("notes")),
            opt_text(r.fields.get("family_history")),
            Param::from(now),
            Param::from(actor),
        ]
    };
    match (existing, mapping.on_existing) {
        (Some(id), OnExisting::Skip) => Ok(Verdict::Skipped { existing: id }),
        (Some(id), OnExisting::Update) => {
            if do_apply {
                store.update_by_id(
                    table("subject_disease"),
                    &[
                        ("onset_event_id", onset.map_or(Param::Null, Param::Int)),
                        (
                            "diagnosis_event_id",
                            diagnosis.map_or(Param::Null, Param::Int),
                        ),
                        ("notes", opt_text(r.fields.get("notes"))),
                        ("family_history", opt_text(r.fields.get("family_history"))),
                        ("actor", Param::from(actor)),
                    ],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Updated { existing: id })
        }
        (Some(id), OnExisting::Supersede) => {
            if do_apply {
                let new =
                    insert_returning(store, "subject_disease", &columns, row(onset, diagnosis))?;
                store.update_by_id(
                    table("subject_disease"),
                    &[("superseded_by", Param::Int(new))],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Superseded { existing: id })
        }
        (None, _) => {
            if do_apply {
                insert_returning(store, "subject_disease", &columns, row(onset, diagnosis))?;
            }
            Ok(Verdict::Added)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_subject_disease_type(
    store: &mut Store,
    mapping: &Mapping,
    r: &Parsed,
    subject: i64,
    disease: i64,
    dtype: i64,
    actor: &str,
    now: &str,
    do_apply: bool,
) -> Result<Verdict, Error> {
    let d = store.dialect();
    // The subject's disease row, made if absent: a type is of a disease the
    // subject has.
    let sd_sql = format!(
        "SELECT id FROM {} WHERE subject_id = {} AND disease_id = {} AND superseded_by IS NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("subject_disease"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let subject_disease = match store
        .query_opt(&sd_sql, &[Param::Int(subject), Param::Int(disease)])?
        .map(|row| row.int(0))
        .transpose()?
    {
        Some(id) => id,
        None if do_apply => insert_returning(
            store,
            "subject_disease",
            &["subject_id", "disease_id", "created_at", "actor"],
            vec![
                Param::Int(subject),
                Param::Int(disease),
                Param::from(now),
                Param::from(actor),
            ],
        )?,
        None => 0,
    };
    let sql = format!(
        "SELECT id FROM {} WHERE subject_disease_id = {} AND disease_type_id = {} \
         AND superseded_by IS NULL ORDER BY id DESC LIMIT 1",
        store.qualified("subject_disease_type"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let existing = match subject_disease {
        0 => None,
        sd => store
            .query_opt(&sql, &[Param::Int(sd), Param::Int(dtype)])?
            .map(|row| row.int(0))
            .transpose()?,
    };
    let columns = [
        "subject_disease_id",
        "disease_type_id",
        "assigned_on",
        "notes",
        "created_at",
        "actor",
    ];
    let row = || {
        vec![
            Param::Int(subject_disease),
            Param::Int(dtype),
            opt_param(r.fields.get("assigned_on")),
            opt_text(r.fields.get("notes")),
            Param::from(now),
            Param::from(actor),
        ]
    };
    match (existing, mapping.on_existing) {
        (Some(id), OnExisting::Skip) => Ok(Verdict::Skipped { existing: id }),
        (Some(id), OnExisting::Update) => {
            if do_apply {
                store.update_by_id(
                    table("subject_disease_type"),
                    &[
                        ("assigned_on", opt_param(r.fields.get("assigned_on"))),
                        ("notes", opt_text(r.fields.get("notes"))),
                        ("actor", Param::from(actor)),
                    ],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Updated { existing: id })
        }
        (Some(id), OnExisting::Supersede) => {
            if do_apply {
                let new = insert_returning(store, "subject_disease_type", &columns, row())?;
                store.update_by_id(
                    table("subject_disease_type"),
                    &[("superseded_by", Param::Int(new))],
                    "id",
                    id,
                )?;
            }
            Ok(Verdict::Superseded { existing: id })
        }
        (None, _) => {
            if do_apply {
                insert_returning(store, "subject_disease_type", &columns, row())?;
            }
            Ok(Verdict::Added)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_read_under_its_format_and_never_guessed() {
        let src = |f: &str| Source {
            column: Some("d".into()),
            value: None,
            parser: Parser::Date,
            format: Some(f.into()),
        };
        assert_eq!(
            parse(&src("%Y-%m-%d"), "2022-01-15").unwrap(),
            Value::Date("2022-01-15".into())
        );
        assert_eq!(
            parse(&src("%d/%m/%Y"), "15/01/2022").unwrap(),
            Value::Date("2022-01-15".into())
        );
        assert_eq!(
            parse(&src("%m/%d/%Y"), "01/15/2022").unwrap(),
            Value::Date("2022-01-15".into())
        );
        assert_eq!(
            parse(&src("%Y%m%d"), "20220115").unwrap(),
            Value::Date("2022-01-15".into())
        );
        // The same text under the other reading is another day, which is the
        // whole reason the format is declared.
        assert_eq!(
            parse(&src("%d/%m/%Y"), "01/02/2022").unwrap(),
            Value::Date("2022-02-01".into())
        );
        assert_eq!(
            parse(&src("%m/%d/%Y"), "01/02/2022").unwrap(),
            Value::Date("2022-01-02".into())
        );
        assert!(parse(&src("%Y-%m-%d"), "2022-02-30").is_err(), "not a day");
        assert!(
            parse(&src("%Y-%m-%d"), "15/01/2022").is_err(),
            "not the format"
        );
        let time = Source {
            column: Some("t".into()),
            value: None,
            parser: Parser::Time,
            format: Some("%H:%M".into()),
        };
        assert_eq!(
            parse(&time, "09:05").unwrap(),
            Value::Time("09:05:00".into())
        );
    }

    #[test]
    fn a_mapping_is_refused_when_the_engine_could_not_run_it() {
        let ok = Mapping::parse(
            "import:\n  target: event\n  subject: {column: code}\n  observation_type: EDSS\n  columns:\n    event_date: {column: date, parser: date, format: '%Y-%m-%d'}\n    value: {column: edss, parser: float}\n",
        )
        .unwrap();
        assert_eq!(ok.target, Target::Event);
        assert_eq!(ok.key_fields(), vec!["event_date".to_string()]);
        let e = |y: &str| Mapping::parse(y).unwrap_err().to_string();
        assert!(e("import:\n  target: event\n  observation_type: EDSS\n  columns:\n    event_date: {column: d, parser: date, format: '%Y'}\n").contains("subject"));
        assert!(e("import:\n  target: event\n  subject: {column: c}\n  columns:\n    event_date: {column: d, parser: date, format: '%Y'}\n").contains("observation_type"));
        assert!(e("import:\n  target: event\n  subject: {column: c}\n  observation_type: EDSS\n  columns:\n    event_date: {column: d, parser: date}\n").contains("format"));
        assert!(e("import:\n  target: event\n  subject: {column: c}\n  observation_type: EDSS\n  columns:\n    event_date: {column: d}\n").contains("is a date"));
        assert!(e("import:\n  target: event\n  subject: {column: c}\n  observation_type: EDSS\n  columns:\n    wobble: {column: d}\n").contains("not a field"));
        assert!(e("import:\n  target: subject\n  subject: {column: c, by: identifier}\n  columns:\n    sex: {column: s}\n").contains("id_type"));
        assert!(
            e("import:\n  target: cohort\n  columns:\n    owner: {column: o}\n")
                .contains("needs name")
        );
    }
}
