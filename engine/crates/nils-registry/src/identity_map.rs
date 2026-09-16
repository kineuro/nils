// SPDX-License-Identifier: AGPL-3.0-only

//! The identifier map in any shape (record 26, decision 5). A table whose
//! columns are named for what they are: an identifier of a type, the
//! canonical identifier that stands for the person (the subject's code
//! derives from it under the registry's scheme and key, exactly as a
//! digest would derive it), the code itself, or nothing. A row is one
//! subject with any number of identifiers; an empty cell is skipped.
//!
//! The map is validated whole and then applied, and it reports what it
//! will do before anything is written: subjects named, known and new;
//! identifiers filed, known and new; held files released; subjects merged;
//! and the conflicts, on which nothing is written. Counts only: an
//! identifier never appears in the report, and never in the audit.
//!
//! The first shape, one identifier column and one code column
//! ([`crate::linkage::import`]), runs through here.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::linkage::{self, Identity, NewIdentity, Subject, Subkeys};
use crate::merge;
use crate::migrate;
use crate::pseudonym::{self, Scheme};
use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Store};
use crate::time::now_iso;

/// What a column of the map is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// An identifier of the named type.
    Identifier(String),
    /// The identifier that stands for the person, of the named type: filed
    /// like any other, and the subject's code derives from it.
    Canonical(String),
    /// The subject's code, taken as given.
    Code,
    /// A column the map does not read.
    Ignore,
}

impl Role {
    /// `identifier:<type>`, `canonical:<type>`, `code` or `ignore`, as
    /// `--column HEADER=ROLE` and the imports door take it.
    pub fn parse(text: &str) -> Result<Role, String> {
        let (role, id_type) = match text.split_once(':') {
            Some((r, t)) => (r.trim(), Some(t.trim())),
            None => (text.trim(), None),
        };
        match (role, id_type) {
            ("identifier", Some(t)) if !t.is_empty() => Ok(Role::Identifier(t.to_string())),
            ("canonical", Some(t)) if !t.is_empty() => Ok(Role::Canonical(t.to_string())),
            ("identifier" | "canonical", _) => Err(format!("{role} names its type: {role}:<type>")),
            ("code", None) => Ok(Role::Code),
            ("ignore", None) => Ok(Role::Ignore),
            _ => Err(format!(
                "{text:?} is not a column role: identifier:<type>, canonical:<type>, code or ignore"
            )),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Role::Identifier(_) => "identifier",
            Role::Canonical(_) => "canonical",
            Role::Code => "code",
            Role::Ignore => "ignore",
        }
    }

    /// The type an identifier or canonical column files under.
    pub fn id_type(&self) -> Option<&str> {
        match self {
            Role::Identifier(t) | Role::Canonical(t) => Some(t),
            Role::Code | Role::Ignore => None,
        }
    }
}

/// One column of the map: its header, as the file has it, and its role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub header: String,
    pub role: Role,
}

/// One row: its line in the file, for the report, and its cells in column
/// order; a cell the row lacks reads as empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub cells: Vec<String>,
}

/// How a canonical identifier becomes a code: the registry's scheme, key
/// and display length, as the digest has them.
#[derive(Clone, Copy)]
pub struct Derive<'a> {
    pub scheme: Scheme,
    pub key: &'a [u8],
    pub display_length: usize,
}

/// The map and how to apply it.
pub struct Map<'a> {
    pub columns: &'a [Column],
    pub rows: &'a [Row],
    /// Report and write nothing.
    pub dry_run: bool,
    /// Make the types the columns name that the store has not got; a
    /// conflict otherwise.
    pub make_types: bool,
    /// The dataset the map was given for, if one. Recorded on a merge's
    /// audit row, never a filter: filed once, a map holds for every dataset.
    pub place_id: Option<i64>,
    /// Who applies it, for the merges' audit rows.
    pub actor: &'a str,
    pub job_id: Option<i64>,
}

/// Why a row, or the map, is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// A column names a type the store has not got and the map was not
    /// told to make it.
    UnknownType { id_type: String },
    /// The row carries no identifier.
    Empty,
    /// The row names no code, no canonical identifier, and no identifier
    /// the store knows, so it resolves to no subject.
    Unresolved,
    /// The row's identifiers are known on two subjects.
    TwoSubjects { codes: Vec<String> },
    /// The identifier appeared on an earlier line with another subject.
    Repeated { first_line: usize },
    /// The identifier is filed on another subject than the row's code names.
    Mapped { code: String },
    /// The row's code names a subject that was merged into another.
    Merged { code: String, into: String },
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Why::UnknownType { id_type } => write!(
                f,
                "no id type named {id_type}; --make-types makes it, nils linkage id-type list shows them"
            ),
            Why::Empty => f.write_str("an empty identifier or code"),
            Why::Unresolved => f.write_str(
                "no code, no canonical identifier, and no identifier the registry knows",
            ),
            Why::TwoSubjects { codes } => write!(
                f,
                "the identifiers belong to {} subjects ({})",
                codes.len(),
                codes.join(", ")
            ),
            Why::Repeated { first_line } => write!(
                f,
                "the identifier appeared on line {first_line} with another code"
            ),
            Why::Mapped { code } => write!(f, "the identifier already maps to subject {code}"),
            Why::Merged { code, into } => {
                write!(f, "subject {code} was merged into {into}; name {into}")
            }
        }
    }
}

/// One conflict: the line (1 is the header, for a conflict of the map
/// itself) and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub row: usize,
    pub why: Why,
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.row, self.why)
    }
}

impl Conflict {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({ "row": self.row, "why": self.why.to_string() })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subjects {
    /// Distinct subjects the rows resolve to.
    pub named: usize,
    pub known: usize,
    pub new: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identifiers {
    /// Distinct identifiers the rows carry.
    pub filed: usize,
    /// Already on the subject they belong to.
    pub known: usize,
    pub new: usize,
    pub types_new: usize,
}

/// A subject merged by the map: an identifier of the alias mapped to the
/// canonical identifier of another (record 26 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    pub alias: String,
    pub canonical: String,
}

/// Held files the map released, by the type the map named them under and
/// the type the pseudonymiser held them as (lab 26, defect 5): a map may
/// name a held value under another type than the dataset's rule read it
/// as, and the files are released all the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Released {
    pub id_type: String,
    pub held_as: String,
    pub files: u64,
}

impl Released {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({ "type": self.id_type, "held_as": self.held_as, "files": self.files })
    }
}

/// What the map will do, or did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub rows: usize,
    pub subjects: Subjects,
    pub identifiers: Identifiers,
    /// Held files whose identifier the map named, released for the next run.
    pub held_released: u64,
    /// The same, by the type that named them and the type they were held as.
    pub held_released_by: Vec<Released>,
    pub merges: Vec<Merge>,
    pub conflicts: Vec<Conflict>,
    pub dry_run: bool,
    /// Identifiers that joined a subject another identifier of the same
    /// type already names: one person under several identifiers (§7.4).
    pub further: usize,
}

impl Report {
    /// Whether the map was applied: not a dry run, and no conflict.
    pub fn written(&self) -> bool {
        !self.dry_run && self.conflicts.is_empty()
    }

    /// The report as the door and the job result carry it: counts and
    /// codes, never an identifier.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rows": self.rows,
            "subjects": {
                "named": self.subjects.named,
                "known": self.subjects.known,
                "new": self.subjects.new,
            },
            "identifiers": {
                "filed": self.identifiers.filed,
                "known": self.identifiers.known,
                "new": self.identifiers.new,
                "types_new": self.identifiers.types_new,
            },
            "held_released": self.held_released,
            "held_released_by": self.held_released_by.iter().map(Released::as_json).collect::<Vec<_>>(),
            "merges": self.merges.iter().map(|m| serde_json::json!({
                "alias": m.alias, "canonical": m.canonical,
            })).collect::<Vec<_>>(),
            "conflicts": self.conflicts.iter().map(Conflict::as_json).collect::<Vec<_>>(),
            "dry_run": self.dry_run,
            "written": self.written(),
        })
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.written() {
            "filed"
        } else {
            "would file"
        };
        writeln!(
            f,
            "{} row(s): {} subject(s) named, {} known, {} new; {} identifier(s) {verb}, {} known, {} new{}",
            self.rows,
            self.subjects.named,
            self.subjects.known,
            self.subjects.new,
            self.identifiers.filed,
            self.identifiers.known,
            self.identifiers.new,
            match self.identifiers.types_new {
                0 => String::new(),
                n => format!(", {n} type(s) new"),
            }
        )?;
        for m in &self.merges {
            writeln!(f, "  merge {} into {}", m.alias, m.canonical)?;
        }
        if self.held_released > 0 {
            writeln!(
                f,
                "  {} held file(s) released for the next pseudonymise",
                self.held_released
            )?;
            for r in &self.held_released_by {
                if r.id_type == r.held_as {
                    writeln!(f, "    {} named as {}", r.files, r.id_type)?;
                } else {
                    writeln!(
                        f,
                        "    {} named as {}, held as {}",
                        r.files, r.id_type, r.held_as
                    )?;
                }
            }
        }
        if !self.conflicts.is_empty() {
            writeln!(
                f,
                "{} row(s) refused; nothing was written:",
                self.conflicts.len()
            )?;
            for c in &self.conflicts {
                writeln!(f, "  {c}")?;
            }
        } else if self.dry_run {
            writeln!(f, "dry run: nothing was written")?;
        }
        Ok(())
    }
}

/// One identifier a row carries.
struct Ident {
    id_type: String,
    value: String,
    lookup: Vec<u8>,
}

/// Where a row's code came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Code,
    Canonical,
    Known,
}

/// A row that resolved to one subject.
struct Resolved {
    line: usize,
    code: String,
    digest: Option<Vec<u8>>,
    from: Origin,
    idents: Vec<Ident>,
}

/// Apply the map, or report what it would do. Validate then apply: the
/// conflicts are listed and nothing is written when there is one, or
/// when `dry_run` is set. `derive` is needed by a canonical column and
/// refused when absent.
pub fn import(
    registry: &mut Store,
    linkage: &mut Store,
    keys: &Subkeys,
    derive: Option<&Derive<'_>>,
    map: &Map<'_>,
) -> Result<Report, Error> {
    let mut report = Report {
        rows: 0,
        dry_run: map.dry_run,
        ..Report::default()
    };
    // the columns
    let codes = map.columns.iter().filter(|c| c.role == Role::Code).count();
    let canonicals = map
        .columns
        .iter()
        .filter(|c| matches!(c.role, Role::Canonical(_)))
        .count();
    if codes > 1 || canonicals > 1 {
        return Err(Error::Message(
            "a map has at most one code column and one canonical column".to_string(),
        ));
    }
    if map.columns.iter().all(|c| c.role.id_type().is_none()) {
        return Err(Error::Message(
            "the map names no identifier column: a column is identifier:<type> or canonical:<type>"
                .to_string(),
        ));
    }
    if canonicals == 1 && derive.is_none() {
        return Err(Error::Message(
            "a canonical column derives the code under the registry's scheme and key, which this import was not given"
                .to_string(),
        ));
    }
    let mut type_ids: HashMap<String, Option<i64>> = HashMap::new();
    let mut types_to_make: Vec<String> = Vec::new();
    for c in map.columns {
        let Some(name) = c.role.id_type() else {
            continue;
        };
        if type_ids.contains_key(name) {
            continue;
        }
        if !linkage::valid_id_type_name(name) {
            return Err(Error::Message(format!(
                "{name:?} is not an id type name (lower case letters, digits and hyphens, like patient-id)"
            )));
        }
        let id = linkage::id_type_id(linkage, name)?;
        if id.is_none() {
            if map.make_types {
                types_to_make.push(name.to_string());
            } else {
                report.conflicts.push(Conflict {
                    row: 1,
                    why: Why::UnknownType {
                        id_type: name.to_string(),
                    },
                });
            }
        }
        type_ids.insert(name.to_string(), id);
    }
    report.identifiers.types_new = types_to_make.len();

    // the rows, read once
    struct Raw {
        line: usize,
        code: Option<String>,
        canonical: Option<(String, String)>,
        idents: Vec<Ident>,
    }
    let mut raws: Vec<Raw> = Vec::with_capacity(map.rows.len());
    for r in map.rows {
        let cell = |i: usize| r.cells.get(i).map(|c| c.trim()).unwrap_or("");
        let mut raw = Raw {
            line: r.line,
            code: None,
            canonical: None,
            idents: Vec::new(),
        };
        for (i, c) in map.columns.iter().enumerate() {
            let v = cell(i);
            if v.is_empty() {
                continue;
            }
            match &c.role {
                Role::Identifier(t) => raw.idents.push(Ident {
                    id_type: t.clone(),
                    value: v.to_string(),
                    lookup: keys.lookup(t, v),
                }),
                Role::Canonical(t) => {
                    raw.canonical = Some((t.clone(), v.to_string()));
                    raw.idents.push(Ident {
                        id_type: t.clone(),
                        value: v.to_string(),
                        lookup: keys.lookup(t, v),
                    });
                }
                Role::Code => raw.code = Some(v.to_string()),
                Role::Ignore => {}
            }
        }
        if raw.idents.is_empty() && raw.code.is_none() {
            // a blank line
            continue;
        }
        raws.push(raw);
    }
    report.rows = raws.len();

    // against the stores, in one select each
    let lookups: Vec<Vec<u8>> = raws
        .iter()
        .flat_map(|r| r.idents.iter().map(|i| i.lookup.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let existing: HashMap<Vec<u8>, Identity> = linkage::identities_by_lookup(linkage, &lookups)?
        .into_iter()
        .map(|i| (i.lookup.clone(), i))
        .collect();
    let holder_ids: Vec<i64> = existing
        .values()
        .map(|i| i.subject_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut subjects: HashMap<i64, Subject> = linkage::subjects_by_id(registry, &holder_ids)?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    let code_of = |subjects: &HashMap<i64, Subject>, id: i64| -> String {
        subjects
            .get(&id)
            .map(|s| s.code.clone())
            .unwrap_or_else(|| format!("#{id}"))
    };

    // each row to one subject
    let mut resolved: Vec<Resolved> = Vec::with_capacity(raws.len());
    for raw in raws {
        if raw.idents.is_empty() {
            report.conflicts.push(Conflict {
                row: raw.line,
                why: Why::Empty,
            });
            continue;
        }
        let (code, digest, from) = if let Some(code) = raw.code {
            (code, None, Origin::Code)
        } else if let Some((_, value)) = &raw.canonical {
            let d = derive.expect("checked above");
            let c = pseudonym::code(d.scheme, d.key, value, d.display_length);
            (c.code, Some(c.digest), Origin::Canonical)
        } else {
            let holders: BTreeSet<i64> = raw
                .idents
                .iter()
                .filter_map(|i| existing.get(&i.lookup).map(|e| e.subject_id))
                .collect();
            match holders.len() {
                0 => {
                    report.conflicts.push(Conflict {
                        row: raw.line,
                        why: Why::Unresolved,
                    });
                    continue;
                }
                1 => {
                    let id = *holders.iter().next().expect("one");
                    (code_of(&subjects, id), None, Origin::Known)
                }
                _ => {
                    let mut codes: Vec<String> =
                        holders.iter().map(|&id| code_of(&subjects, id)).collect();
                    codes.sort();
                    report.conflicts.push(Conflict {
                        row: raw.line,
                        why: Why::TwoSubjects { codes },
                    });
                    continue;
                }
            }
        };
        resolved.push(Resolved {
            line: raw.line,
            code,
            digest,
            from,
            idents: raw.idents,
        });
    }

    // the subjects the rows name
    let named: Vec<String> = resolved
        .iter()
        .map(|r| r.code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let by_code: HashMap<String, Subject> = linkage::subjects_by_code(registry, &named)?
        .into_iter()
        .map(|s| (s.code.clone(), s))
        .collect();
    for s in by_code.values() {
        subjects.entry(s.id).or_insert_with(|| s.clone());
    }
    // where a named subject was merged, the one it went into, for the words
    let merged_into: Vec<i64> = by_code
        .values()
        .filter_map(|s| s.merged_into)
        .filter(|id| !subjects.contains_key(id))
        .collect();
    for s in linkage::subjects_by_id(registry, &merged_into)? {
        subjects.insert(s.id, s);
    }
    // what each known subject already holds, by type name
    let typed: HashMap<i64, HashSet<String>> = {
        let ids: Vec<i64> = subjects.keys().copied().collect();
        let names: HashMap<i64, String> = linkage::id_types(linkage)?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let mut typed: HashMap<i64, HashSet<String>> = HashMap::new();
        for i in linkage::identities_of_subjects(linkage, &ids)? {
            if let Some(n) = names.get(&i.id_type_id) {
                typed.entry(i.subject_id).or_default().insert(n.clone());
            }
        }
        typed
    };

    // within the map, and against the store
    let mut seen: HashMap<Vec<u8>, (usize, String)> = HashMap::new();
    let mut merges: Vec<Merge> = Vec::new();
    let mut to_file: Vec<(String, Ident)> = Vec::new();
    let mut filed: HashSet<Vec<u8>> = HashSet::new();
    let mut known: HashSet<Vec<u8>> = HashSet::new();
    let mut typed_now: HashMap<String, HashSet<String>> = HashMap::new();
    let mut codes_ok: BTreeSet<String> = BTreeSet::new();
    let mut digests: HashMap<String, Vec<u8>> = HashMap::new();
    // every identifier of a row that stands, as (type, value), for the held
    // files the map releases
    let mut named: BTreeSet<(String, String)> = BTreeSet::new();
    'rows: for r in resolved {
        if let Some(s) = by_code.get(&r.code)
            && let Some(into) = s.merged_into
        {
            report.conflicts.push(Conflict {
                row: r.line,
                why: Why::Merged {
                    code: r.code.clone(),
                    into: code_of(&subjects, into),
                },
            });
            continue;
        }
        for i in &r.idents {
            match seen.get(&i.lookup) {
                Some((first_line, code)) if *code != r.code => {
                    report.conflicts.push(Conflict {
                        row: r.line,
                        why: Why::Repeated {
                            first_line: *first_line,
                        },
                    });
                    continue 'rows;
                }
                Some(_) => {}
                None => {
                    seen.insert(i.lookup.clone(), (r.line, r.code.clone()));
                }
            }
            if let Some(e) = existing.get(&i.lookup) {
                let holder = code_of(&subjects, e.subject_id);
                if holder == r.code {
                    continue;
                }
                if r.from == Origin::Canonical {
                    // the identifier's subject is an alias of the canonical
                    // identifier's (record 26 §6); the identity moves with
                    // the merge
                    continue;
                }
                report.conflicts.push(Conflict {
                    row: r.line,
                    why: Why::Mapped { code: holder },
                });
                continue 'rows;
            }
        }
        // the row stands
        codes_ok.insert(r.code.clone());
        for i in r.idents {
            named.insert((i.id_type.clone(), i.value.clone()));
            if let Some(e) = existing.get(&i.lookup) {
                known.insert(i.lookup.clone());
                let holder = code_of(&subjects, e.subject_id);
                if holder != r.code
                    && !merges.iter().any(|m| m.alias == holder)
                    && !subjects
                        .get(&e.subject_id)
                        .is_some_and(|s| s.merged_into.is_some())
                {
                    merges.push(Merge {
                        alias: holder,
                        canonical: r.code.clone(),
                    });
                }
                continue;
            }
            if !filed.insert(i.lookup.clone()) {
                continue;
            }
            let held_already = by_code
                .get(&r.code)
                .and_then(|s| typed.get(&s.id))
                .is_some_and(|set| set.contains(&i.id_type));
            let in_map = typed_now
                .entry(r.code.clone())
                .or_default()
                .insert(i.id_type.clone());
            if held_already || !in_map {
                report.further += 1;
            }
            to_file.push((r.code.clone(), i));
        }
        if r.from == Origin::Canonical
            && let Some(d) = r.digest
        {
            digests.insert(r.code.clone(), d);
        }
    }
    report.subjects.named = codes_ok.len();
    report.subjects.known = codes_ok.iter().filter(|c| by_code.contains_key(*c)).count();
    report.subjects.new = report.subjects.named - report.subjects.known;
    report.identifiers.known = known.len();
    report.identifiers.new = filed.len();
    report.identifiers.filed = known.len() + filed.len();
    report.merges = merges;
    if !report.conflicts.is_empty() {
        report.conflicts.sort_by_key(|c| c.row);
        return Ok(report);
    }
    // the held files the map names, under the type the map gives each
    // value and under every type the held rows carry
    let named: Vec<(String, String)> = named.into_iter().collect();
    let matches = held_matches(registry, keys, &named)?;
    report.held_released = matches.len() as u64;
    report.held_released_by = released_by(&matches);
    if map.dry_run {
        return Ok(report);
    }

    // apply: the types, the subjects, the merges, the identities and the
    // held files, in one transaction on each store, both written before
    // either commits and both rolled back when anything fails, so a map
    // whose merge fails leaves nothing behind (§9.3; lab 26, defect 1b)
    registry.begin()?;
    if let Err(e) = linkage.begin() {
        let _ = registry.rollback();
        return Err(e);
    }
    let applied = apply(
        registry,
        linkage,
        keys,
        map,
        &report,
        &types_to_make,
        &mut type_ids,
        &by_code,
        &subjects,
        &codes_ok,
        &digests,
        to_file,
        &matches,
    );
    match applied {
        Ok(()) => {
            registry.commit()?;
            linkage.commit()?;
            Ok(report)
        }
        Err(e) => {
            let _ = registry.rollback();
            let _ = linkage.rollback();
            Err(e)
        }
    }
}

/// The writes of an import, inside the transactions [`import`] holds:
/// the types made, the subjects created, the merges, the identities filed,
/// and the held files released.
#[allow(clippy::too_many_arguments)]
fn apply(
    registry: &mut Store,
    linkage: &mut Store,
    keys: &Subkeys,
    map: &Map<'_>,
    report: &Report,
    types_to_make: &[String],
    type_ids: &mut HashMap<String, Option<i64>>,
    by_code: &HashMap<String, Subject>,
    subjects: &HashMap<i64, Subject>,
    codes_ok: &BTreeSet<String>,
    digests: &HashMap<String, Vec<u8>>,
    to_file: Vec<(String, Ident)>,
    matches: &[HeldMatch],
) -> Result<(), Error> {
    for name in types_to_make {
        let t = linkage::add_id_type(linkage, name, None)?;
        type_ids.insert(name.clone(), Some(t.id));
    }
    let now = now_iso();
    let mut ids: HashMap<String, i64> = by_code.iter().map(|(c, s)| (c.clone(), s.id)).collect();
    let to_create: Vec<&String> = codes_ok.iter().filter(|c| !ids.contains_key(*c)).collect();
    if !to_create.is_empty() {
        let t = table("subject");
        let values: Vec<Vec<Param>> = to_create
            .iter()
            .map(|c| {
                vec![
                    Param::from(c.as_str()),
                    digests
                        .get(*c)
                        .map_or(Param::Null, |d| Param::Bytes(d.clone())),
                    Param::from(now.as_str()),
                ]
            })
            .collect();
        let inserted = registry.insert(
            &Insert::new(t, &["code", "code_digest", "created_at"]).returning(&["id", "code"]),
            &values,
        )?;
        for row in &inserted {
            ids.insert(row.text(1)?.to_string(), row.int(0)?);
        }
    }
    for m in &report.merges {
        let alias = *ids
            .get(&m.alias)
            .or_else(|| subjects.values().find(|s| s.code == m.alias).map(|s| &s.id))
            .ok_or_else(|| Error::Message(format!("subject {} is not in the registry", m.alias)))?;
        let canonical = *ids
            .get(&m.canonical)
            .ok_or_else(|| Error::Message(format!("subject {} was not created", m.canonical)))?;
        let why = format!(
            "the identifier map files an identifier of {} under the canonical identifier of {}",
            m.alias, m.canonical
        );
        merge::merge_in(
            registry,
            linkage,
            keys,
            &merge::Ask {
                canonical,
                alias,
                why: &why,
                actor: map.actor,
                job_id: map.job_id,
                place_id: map.place_id,
            },
        )?;
    }
    let mut rows = Vec::with_capacity(to_file.len());
    for (code, i) in to_file {
        let subject_id = *ids
            .get(&code)
            .ok_or_else(|| Error::Message(format!("subject {code} was not created")))?;
        let id_type_id = type_ids
            .get(&i.id_type)
            .copied()
            .flatten()
            .ok_or_else(|| Error::Message(format!("id type {} was not made", i.id_type)))?;
        rows.push(NewIdentity {
            subject_id,
            id_type_id,
            lookup: i.lookup,
            ciphertext: keys.seal(&i.value),
            source: "csv",
            first_batch_id: None,
        });
    }
    linkage::insert_identities(linkage, &rows)?;
    release_matches(registry, matches)
}

/// The table the pseudonymiser records every file in (record 26 §4),
/// declared by the dataset slice; the map reads it when it is there.
const HELD_TABLE: &str = "pseudonym_file";

/// One held file the map names: the row, the type the map named the
/// value under with the lookup under that type, and the type the
/// pseudonymiser held it as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldMatch {
    pub row: i64,
    pub id_type: String,
    pub held_as: String,
    pub lookup: Vec<u8>,
}

/// The held files whose identifier the map names, matched by value: the
/// lookup of each named value is computed under every type the held rows
/// carry, not only the type the map gives it, since a dataset's rule reads
/// a study id in `PatientID` as the rule's own type and the map may name it
/// as what it is (lab 26, defect 5). Where a value is named under a held
/// row's own type, that match stands. Nothing where the table is not there.
pub fn held_matches(
    registry: &mut Store,
    keys: &Subkeys,
    named: &[(String, String)],
) -> Result<Vec<HeldMatch>, Error> {
    if named.is_empty() || !migrate::table_exists(registry, HELD_TABLE)? {
        return Ok(Vec::new());
    }
    let held_types: Vec<String> = registry
        .query(
            &format!(
                "SELECT DISTINCT id_type FROM {} WHERE state = 'held' AND released_at IS NULL \
                 AND id_type IS NOT NULL ORDER BY id_type",
                registry.qualified(HELD_TABLE)
            ),
            &[],
        )?
        .iter()
        .map(|r| Ok(r.text(0)?.to_string()))
        .collect::<Result<_, Error>>()?;
    if held_types.is_empty() {
        return Ok(Vec::new());
    }
    // the lookup under each held type, to the value's type and the value;
    // a value named under the held type itself wins over a cross match
    let mut candidates: HashMap<Vec<u8>, (String, String)> = HashMap::new();
    for held_type in &held_types {
        for (id_type, value) in named {
            if id_type != held_type {
                candidates
                    .entry(keys.lookup(held_type, value))
                    .or_insert_with(|| (id_type.clone(), value.clone()));
            }
        }
        for (id_type, value) in named {
            if id_type == held_type {
                candidates.insert(
                    keys.lookup(held_type, value),
                    (id_type.clone(), value.clone()),
                );
            }
        }
    }
    let lookups: Vec<&Vec<u8>> = candidates.keys().collect();
    let mut out = Vec::new();
    for chunk in lookups.chunks(crate::store::SQLITE_KEY_CHUNK) {
        let d = registry.dialect();
        let marks: Vec<String> = (0..chunk.len())
            .map(|i| d.param(i + 1, Type::Bytes))
            .collect();
        let sql = format!(
            "SELECT id, lookup, id_type FROM {} WHERE state = 'held' AND released_at IS NULL \
             AND lookup IN ({}) ORDER BY id",
            registry.qualified(HELD_TABLE),
            marks.join(", ")
        );
        let params: Vec<Param> = chunk.iter().map(|l| Param::Bytes((*l).clone())).collect();
        for r in registry.query(&sql, &params)? {
            let lookup = r.bytes(1)?.to_vec();
            let Some((id_type, value)) = candidates.get(&lookup) else {
                continue;
            };
            out.push(HeldMatch {
                row: r.int(0)?,
                id_type: id_type.clone(),
                held_as: r.opt_text(2)?.unwrap_or("").to_string(),
                lookup: keys.lookup(id_type, value),
            });
        }
    }
    Ok(out)
}

/// The matches counted by the type that named them and the type they were
/// held as.
fn released_by(matches: &[HeldMatch]) -> Vec<Released> {
    let mut by: Vec<Released> = Vec::new();
    for m in matches {
        match by
            .iter_mut()
            .find(|r| r.id_type == m.id_type && r.held_as == m.held_as)
        {
            Some(r) => r.files += 1,
            None => by.push(Released {
                id_type: m.id_type.clone(),
                held_as: m.held_as.clone(),
                files: 1,
            }),
        }
    }
    by
}

/// Release the held files the map named: `released_at` set, and the row
/// keyed under the type the map named the value as, so that the
/// pseudonymiser's next `--held` run finds the identity the map filed and
/// writes the file (record 26 §4). A match under the held type itself
/// changes nothing but the stamp.
pub fn release_matches(registry: &mut Store, matches: &[HeldMatch]) -> Result<(), Error> {
    if matches.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    let t = table(HELD_TABLE);
    for m in matches {
        registry.update_by_id(
            t,
            &[
                ("released_at", Param::from(now.as_str())),
                ("id_type", Param::from(m.id_type.as_str())),
                ("lookup", Param::Bytes(m.lookup.clone())),
            ],
            "id",
            m.row,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};

    const KEY: &[u8] = b"nils-fixture-key";

    fn stores() -> (Store, Store, Subkeys) {
        let mut registry = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut registry, Kind::Registry).unwrap();
        let mut linkage = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut linkage, Kind::Linkage).unwrap();
        (registry, linkage, Subkeys::derive(KEY))
    }

    fn derive() -> Derive<'static> {
        Derive {
            scheme: Scheme::Blake2b32,
            key: KEY,
            display_length: 12,
        }
    }

    fn columns(spec: &[(&str, &str)]) -> Vec<Column> {
        spec.iter()
            .map(|(h, r)| Column {
                header: h.to_string(),
                role: Role::parse(r).unwrap(),
            })
            .collect()
    }

    fn rows(cells: &[&[&str]]) -> Vec<Row> {
        cells
            .iter()
            .enumerate()
            .map(|(i, r)| Row {
                line: i + 2,
                cells: r.iter().map(|c| c.to_string()).collect(),
            })
            .collect()
    }

    fn run(
        registry: &mut Store,
        linkage: &mut Store,
        keys: &Subkeys,
        columns: &[Column],
        rows: &[Row],
        dry_run: bool,
        make_types: bool,
    ) -> Report {
        import(
            registry,
            linkage,
            keys,
            Some(&derive()),
            &Map {
                columns,
                rows,
                dry_run,
                make_types,
                place_id: None,
                actor: "tester@lab",
                job_id: None,
            },
        )
        .unwrap()
    }

    fn count(store: &mut Store, sql: &str) -> i64 {
        store.query(sql, &[]).unwrap()[0].int(0).unwrap()
    }

    fn codes(store: &mut Store) -> Vec<String> {
        store
            .query(
                "SELECT code FROM subject WHERE merged_into IS NULL ORDER BY code",
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.text(0).unwrap().to_string())
            .collect()
    }

    #[test]
    fn roles_parse_as_the_flag_takes_them() {
        assert_eq!(
            Role::parse("identifier:study-id"),
            Ok(Role::Identifier("study-id".into()))
        );
        assert_eq!(
            Role::parse("canonical:registry-id"),
            Ok(Role::Canonical("registry-id".into()))
        );
        assert_eq!(Role::parse("code"), Ok(Role::Code));
        assert_eq!(Role::parse(" ignore "), Ok(Role::Ignore));
        assert!(Role::parse("identifier").is_err());
        assert!(Role::parse("code:x").is_err());
        assert!(Role::parse("label").is_err());
    }

    #[test]
    fn the_first_shape_files_codes_as_given_and_again_changes_nothing() {
        let (mut registry, mut linkage, keys) = stores();
        // v0's file: PatientID, subject_code
        let cols = columns(&[
            ("PatientID", "identifier:patient-id"),
            ("subject_code", "code"),
        ]);
        let data = rows(&[&["P1", "legacy-0001"], &["P2", "legacy-0002"]]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert!(r.written());
        assert_eq!(
            (
                r.rows,
                r.subjects.named,
                r.subjects.known,
                r.subjects.new,
                r.identifiers.filed,
                r.identifiers.known,
                r.identifiers.new
            ),
            (2, 2, 0, 2, 2, 0, 2)
        );
        assert_eq!(codes(&mut registry), ["legacy-0001", "legacy-0002"]);
        assert_eq!(
            count(
                &mut registry,
                "SELECT COUNT(*) FROM subject WHERE code_digest IS NULL"
            ),
            2
        );
        let again = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(
            (
                again.subjects.known,
                again.identifiers.known,
                again.identifiers.new
            ),
            (2, 2, 0)
        );
        assert_eq!(count(&mut linkage, "SELECT COUNT(*) FROM identity"), 2);
        let text = again.to_string();
        assert!(text.starts_with("2 row(s): 2 subject(s) named, 2 known, 0 new; 2 identifier(s) filed, 2 known, 0 new\n"), "{text}");
    }

    #[test]
    fn a_canonical_column_derives_the_code_as_a_digest_would() {
        let (mut registry, mut linkage, keys) = stores();
        let cols = columns(&[
            ("study_id", "identifier:study-id"),
            ("person", "canonical:registry-id"),
            ("note", "ignore"),
        ]);
        let data = rows(&[
            &["S-1", "PID-0001", "x"],
            &["S-2", "PID-0001", ""],
            &["S-3", "PID-0002", ""],
        ]);
        // the types are new: refused without --make-types, and nothing written
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(r.conflicts.len(), 2, "{r}");
        assert!(matches!(r.conflicts[0].why, Why::UnknownType { .. }));
        assert_eq!(r.conflicts[0].row, 1);
        assert!(!r.written());
        assert_eq!(count(&mut registry, "SELECT COUNT(*) FROM subject"), 0);
        assert_eq!(count(&mut linkage, "SELECT COUNT(*) FROM id_type"), 3);
        // with it: the fixture code of §7.1 for PID-0001
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            true,
        );
        assert!(r.written(), "{r}");
        assert_eq!(r.identifiers.types_new, 2);
        assert_eq!((r.subjects.new, r.identifiers.new), (2, 5));
        assert_eq!(r.further, 1, "S-2 is a second study-id on the one subject");
        let mut got = codes(&mut registry);
        got.sort();
        assert!(got.contains(&"xg5pf9g20xwm".to_string()), "{got:?}");
        let digest = registry
            .query(
                "SELECT code_digest FROM subject WHERE code = 'xg5pf9g20xwm'",
                &[],
            )
            .unwrap()[0]
            .bytes(0)
            .unwrap()
            .to_vec();
        assert_eq!(
            hex::encode(digest),
            "ec0b67a602077942a174a5c8d1683043e58e1b18c44e83769a20be0f4dd43927"
        );
        let one = registry
            .query("SELECT id FROM subject WHERE code = 'xg5pf9g20xwm'", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        let shown = linkage::reveal(&mut linkage, &keys, one, "tester", None).unwrap();
        let mut values: Vec<(String, String)> =
            shown.into_iter().map(|r| (r.id_type, r.value)).collect();
        values.sort();
        assert_eq!(
            values,
            [
                ("registry-id".to_string(), "PID-0001".to_string()),
                ("study-id".to_string(), "S-1".to_string()),
                ("study-id".to_string(), "S-2".to_string()),
            ]
        );
        // the same map again is all known
        let again = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            true,
        );
        assert_eq!(
            (
                again.subjects.known,
                again.identifiers.known,
                again.identifiers.new,
                again.identifiers.types_new
            ),
            (2, 5, 0, 0)
        );
    }

    #[test]
    fn several_identifiers_in_a_row_are_one_subject_and_a_known_one_adds_the_rest() {
        let (mut registry, mut linkage, keys) = stores();
        linkage::add_id_type(&mut linkage, "old-number", None).unwrap();
        let cols = columns(&[
            ("code", "code"),
            ("pid", "identifier:patient-id"),
            ("old", "identifier:old-number"),
            ("older", "identifier:old-number"),
        ]);
        let data = rows(&[
            &["sub-a", "P1", "19000101-0001", ""],
            &["sub-b", "P2", "", ""],
        ]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert!(r.written(), "{r}");
        assert_eq!((r.subjects.new, r.identifiers.new, r.further), (2, 3, 0));
        // a row with no code and one known identifier adds the others to that subject
        let more = columns(&[
            ("pid", "identifier:patient-id"),
            ("old", "identifier:old-number"),
            ("older", "identifier:old-number"),
        ]);
        let data = rows(&[&["P1", "19000101-0002", "19000101-0003"]]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &more,
            &data,
            false,
            false,
        );
        assert!(r.written(), "{r}");
        assert_eq!(
            (r.subjects.named, r.subjects.known, r.subjects.new),
            (1, 1, 0)
        );
        assert_eq!(
            (r.identifiers.known, r.identifiers.new, r.further),
            (1, 2, 2)
        );
        let shown = linkage::reveal(&mut linkage, &keys, 1, "tester", None).unwrap();
        assert_eq!(shown.len(), 4);
        // one no subject holds resolves to nothing; two of two subjects is a conflict
        let data = rows(&[
            &["P9", "", ""],
            &["P1", "", ""],
            &["P2", "19000101-0001", ""],
        ]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &more,
            &data,
            false,
            false,
        );
        assert_eq!(r.conflicts.len(), 2, "{r}");
        assert_eq!(
            r.conflicts[0],
            Conflict {
                row: 2,
                why: Why::Unresolved
            }
        );
        assert_eq!(
            r.conflicts[1],
            Conflict {
                row: 4,
                why: Why::TwoSubjects {
                    codes: vec!["sub-a".into(), "sub-b".into()]
                }
            }
        );
        assert_eq!(count(&mut linkage, "SELECT COUNT(*) FROM identity"), 5);
    }

    #[test]
    fn conflicts_are_listed_whole_and_a_dry_run_writes_nothing() {
        let (mut registry, mut linkage, keys) = stores();
        let cols = columns(&[("identifier", "identifier:patient-id"), ("code", "code")]);
        run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &rows(&[&["P1", "sub-one"]]),
            false,
            false,
        );
        let data = rows(&[
            &["P1", "sub-other"],
            &["P4", "sub-four"],
            &["P4", "sub-five"],
            &["", "sub-seven"],
            &["P8", "sub-eight"],
        ]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(
            r.conflicts,
            vec![
                Conflict {
                    row: 2,
                    why: Why::Mapped {
                        code: "sub-one".into()
                    }
                },
                Conflict {
                    row: 4,
                    why: Why::Repeated { first_line: 3 }
                },
                Conflict {
                    row: 5,
                    why: Why::Empty
                },
            ]
        );
        assert!(!r.written());
        assert_eq!(codes(&mut registry), ["sub-one"]);
        assert_eq!(count(&mut linkage, "SELECT COUNT(*) FROM identity"), 1);
        let text = r.to_string();
        assert!(
            text.contains("3 row(s) refused; nothing was written:"),
            "{text}"
        );
        assert!(
            text.contains("line 2: the identifier already maps to subject sub-one"),
            "{text}"
        );
        assert!(!text.contains("P1"), "{text}");
        // the report's JSON carries counts, codes and reasons, never an identifier
        let json = r.as_json().to_string();
        assert!(!json.contains("P4"), "{json}");
        assert!(json.contains("\"written\":false"));

        let good = rows(&[&["P4", "sub-four"], &["P8", "sub-eight"]]);
        let dry = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &good,
            true,
            false,
        );
        assert!(dry.dry_run && !dry.written());
        assert_eq!((dry.subjects.new, dry.identifiers.new), (2, 2));
        assert_eq!(codes(&mut registry), ["sub-one"]);
        assert_eq!(count(&mut linkage, "SELECT COUNT(*) FROM identity"), 1);
        assert!(dry.to_string().contains("dry run: nothing was written"));
        let done = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &good,
            false,
            false,
        );
        assert!(done.written());
        assert_eq!(codes(&mut registry), ["sub-eight", "sub-four", "sub-one"]);
    }

    #[test]
    fn an_import_releases_the_held_files_it_names() {
        let (mut registry, mut linkage, keys) = stores();
        let held = |store: &mut Store, id: i64, lookup: &[u8], state: &str| {
            store
                .execute(
                    "INSERT INTO pseudonym_file (id, place_id, path, size, mtime, state, shape, lookup, id_type, first_seen, code_anyway) VALUES (?, 1, ?, 0, 0, ?, '99', ?, 'patient-id', 't', 0)",
                    &[
                        Param::Int(id),
                        Param::from(format!("f{id}")),
                        Param::from(state),
                        Param::Bytes(lookup.to_vec()),
                    ],
                )
                .unwrap();
        };
        held(&mut registry, 1, &keys.lookup("patient-id", "P1"), "held");
        held(&mut registry, 2, &keys.lookup("patient-id", "P1"), "held");
        held(&mut registry, 3, &keys.lookup("patient-id", "P2"), "held");
        held(
            &mut registry,
            4,
            &keys.lookup("patient-id", "P1"),
            "written",
        );
        let cols = columns(&[("identifier", "identifier:patient-id"), ("code", "code")]);
        let data = rows(&[&["P1", "sub-one"]]);
        let dry = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            true,
            false,
        );
        assert_eq!(dry.held_released, 2);
        assert_eq!(
            count(
                &mut registry,
                "SELECT COUNT(*) FROM pseudonym_file WHERE released_at IS NOT NULL"
            ),
            0
        );
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(r.held_released, 2);
        assert!(r.to_string().contains("2 held file(s) released"));
        assert_eq!(
            r.held_released_by,
            vec![Released {
                id_type: "patient-id".into(),
                held_as: "patient-id".into(),
                files: 2
            }]
        );
        assert!(r.to_string().contains("2 named as patient-id\n"), "{r}");
        let released: Vec<i64> = registry
            .query(
                "SELECT id FROM pseudonym_file WHERE released_at IS NOT NULL ORDER BY id",
                &[],
            )
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        assert_eq!(released, [1, 2]);
        // a second import of the same identifier releases nothing more
        let again = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(again.held_released, 0);
        // and without the table, nothing to release
        let (mut bare, mut bare_linkage, keys) = stores();
        let r = run(
            &mut bare,
            &mut bare_linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert_eq!(r.held_released, 0);
    }

    #[test]
    fn a_canonical_identifier_of_another_subject_merges_it_first() {
        let (mut registry, mut linkage, keys) = stores();
        linkage::add_id_type(&mut linkage, "registry-id", None).unwrap();
        // the digest met P1 and made a subject of its own for it
        let cols = columns(&[("identifier", "identifier:patient-id"), ("code", "code")]);
        run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &rows(&[&["P1", "split-off"]]),
            false,
            false,
        );
        // the map says P1 is the person whose canonical identifier is PID-0001
        let cols = columns(&[
            ("pid", "identifier:patient-id"),
            ("person", "canonical:registry-id"),
        ]);
        let data = rows(&[&["P1", "PID-0001"]]);
        let dry = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            true,
            false,
        );
        assert_eq!(
            dry.merges,
            vec![Merge {
                alias: "split-off".into(),
                canonical: "xg5pf9g20xwm".into()
            }]
        );
        assert!(
            dry.to_string()
                .contains("merge split-off into xg5pf9g20xwm")
        );
        assert_eq!(codes(&mut registry), ["split-off"]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert!(r.written(), "{r}");
        assert_eq!(r.merges.len(), 1);
        assert_eq!(
            (r.subjects.new, r.identifiers.known, r.identifiers.new),
            (1, 1, 1)
        );
        assert_eq!(codes(&mut registry), ["xg5pf9g20xwm"]);
        let merged = registry
            .query(
                "SELECT merged_into FROM subject WHERE code = 'split-off'",
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(merged, 2);
        // the canonical subject holds P1, PID-0001 and the alias's code
        let shown = linkage::reveal(&mut linkage, &keys, 2, "tester", None).unwrap();
        let mut values: Vec<(String, String)> =
            shown.into_iter().map(|r| (r.id_type, r.value)).collect();
        values.sort();
        assert_eq!(
            values,
            [
                ("patient-id".to_string(), "P1".to_string()),
                ("registry-id".to_string(), "PID-0001".to_string()),
                ("subject-code".to_string(), "split-off".to_string()),
            ]
        );
        // naming the merged code again is a conflict that says where it went
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &columns(&[("identifier", "identifier:patient-id"), ("code", "code")]),
            &rows(&[&["P7", "split-off"]]),
            false,
            false,
        );
        assert_eq!(
            r.conflicts,
            vec![Conflict {
                row: 2,
                why: Why::Merged {
                    code: "split-off".into(),
                    into: "xg5pf9g20xwm".into()
                }
            }]
        );
    }

    /// Lab 26, defect 5: the dataset's rule read two study ids in
    /// `PatientID` as the rule's own type and held their files; a map naming
    /// them as what they are, `study-id`, releases the files all the same,
    /// says under which type, and re-keys the rows to the map's type so the
    /// next run finds the identities the map filed.
    #[test]
    fn a_map_naming_a_held_value_under_another_type_releases_its_files() {
        let (mut registry, mut linkage, keys) = stores();
        linkage::add_id_type(&mut linkage, "personnummer", None).unwrap();
        let held = |store: &mut Store, id: i64, id_type: &str, value: &str| {
            store
                .execute(
                    "INSERT INTO pseudonym_file (id, place_id, path, size, mtime, state, shape, lookup, id_type, first_seen, code_anyway) VALUES (?, 1, ?, 0, 0, 'held', 'AA9999', ?, ?, 't', 0)",
                    &[
                        Param::Int(id),
                        Param::from(format!("f{id}")),
                        Param::Bytes(keys.lookup(id_type, value)),
                        Param::from(id_type),
                    ],
                )
                .unwrap();
        };
        held(&mut registry, 1, "personnummer", "AA1234");
        held(&mut registry, 2, "personnummer", "AA1234");
        held(&mut registry, 3, "personnummer", "AA5678");
        held(&mut registry, 4, "personnummer", "AA9999");
        // the study ids as identifiers of their type, each with the
        // person's number as the canonical identifier
        let cols = columns(&[
            ("study", "identifier:study-id"),
            ("nummer", "canonical:personnummer"),
        ]);
        let data = rows(&[&["AA1234", "199001011234"], &["AA5678", "198502023456"]]);
        let dry = run(&mut registry, &mut linkage, &keys, &cols, &data, true, true);
        assert_eq!(dry.held_released, 3, "{dry}");
        assert_eq!(
            dry.held_released_by,
            vec![Released {
                id_type: "study-id".into(),
                held_as: "personnummer".into(),
                files: 3
            }]
        );
        assert!(
            dry.to_string()
                .contains("3 named as study-id, held as personnummer"),
            "{dry}"
        );
        assert_eq!(
            count(
                &mut registry,
                "SELECT COUNT(*) FROM pseudonym_file WHERE released_at IS NOT NULL"
            ),
            0
        );
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            true,
        );
        assert!(r.written(), "{r}");
        assert_eq!(r.held_released, 3);
        let json = r.as_json();
        assert_eq!(json["held_released_by"][0]["type"], "study-id");
        assert_eq!(json["held_released_by"][0]["held_as"], "personnummer");
        assert_eq!(json["held_released_by"][0]["files"], 3);
        // the released rows are keyed under study-id now, and the fourth,
        // which no row named, is held as it was
        let rows_now = registry
            .query(
                "SELECT id, id_type, lookup, released_at IS NOT NULL FROM pseudonym_file ORDER BY id",
                &[],
            )
            .unwrap();
        for r in &rows_now[..3] {
            assert_eq!(r.text(1).unwrap(), "study-id");
            assert_eq!(r.int(3).unwrap(), 1);
        }
        assert_eq!(
            rows_now[0].bytes(2).unwrap(),
            keys.lookup("study-id", "AA1234")
        );
        assert_eq!(
            rows_now[2].bytes(2).unwrap(),
            keys.lookup("study-id", "AA5678")
        );
        assert_eq!(rows_now[3].text(1).unwrap(), "personnummer");
        assert_eq!(rows_now[3].int(3).unwrap(), 0);
        // the identities the map filed are found under the rows' new lookups
        let found =
            linkage::identities_by_lookup(&mut linkage, &[rows_now[0].bytes(2).unwrap().to_vec()])
                .unwrap();
        assert_eq!(found.len(), 1);
        // the same map again releases nothing more
        let again = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            true,
        );
        assert_eq!(again.held_released, 0);
    }

    /// Lab 26, defects 1 and 1b: the pseudonymiser coded a woman's two
    /// numbers as two subjects and the digest joined both to the dataset's
    /// cohort at one time; the map that names the second number under the
    /// first merges them, and the shared interval does not break the
    /// membership key. A map whose apply fails leaves no subject behind.
    #[test]
    fn a_map_merges_two_subjects_one_digest_joined_and_a_failed_apply_leaves_nothing() {
        let (mut registry, mut linkage, keys) = stores();
        linkage::add_id_type(&mut linkage, "personnummer", None).unwrap();
        // the two subjects as the pseudonymiser would have coded them
        let first = pseudonym::code(Scheme::Blake2b32, KEY, "199001011234", 12).code;
        let second = pseudonym::code(Scheme::Blake2b32, KEY, "199001019876", 12).code;
        let cols = columns(&[("nummer", "canonical:personnummer")]);
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &rows(&[&["199001011234"], &["199001019876"]]),
            false,
            false,
        );
        assert!(r.written(), "{r}");
        assert_eq!(codes(&mut registry), {
            let mut both = vec![first.clone(), second.clone()];
            both.sort();
            both
        });
        registry
            .execute(
                "INSERT INTO cohort (id, name, owner, created_at) VALUES (1, 'north', 'o', 't')",
                &[],
            )
            .unwrap();
        registry
            .execute(
                "INSERT INTO cohort_member (cohort_id, subject_id, joined_at, source, batch_id) \
                 SELECT 1, id, '2026-09-16T10:00:00Z', 'digest', 3 FROM subject",
                &[],
            )
            .unwrap();
        // the second map: her second number is an identifier of the person
        // whose canonical number is the first
        let cols = columns(&[
            ("other", "identifier:personnummer"),
            ("nummer", "canonical:personnummer"),
        ]);
        let data = rows(&[&["199001019876", "199001011234"]]);
        let dry = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            true,
            false,
        );
        assert_eq!(
            dry.merges,
            vec![Merge {
                alias: second.clone(),
                canonical: first.clone()
            }]
        );
        let r = run(
            &mut registry,
            &mut linkage,
            &keys,
            &cols,
            &data,
            false,
            false,
        );
        assert!(r.written(), "{r}");
        assert_eq!(codes(&mut registry), std::slice::from_ref(&first));
        // one open membership, the canonical's; the alias's, the same
        // interval, is gone
        let members = registry
            .query(
                "SELECT subject_id, left_at IS NULL FROM cohort_member ORDER BY id",
                &[],
            )
            .unwrap();
        assert_eq!(members.len(), 1, "{members:?}");
        let canonical_id = registry
            .query("SELECT id FROM subject WHERE merged_into IS NULL", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(members[0].int(0).unwrap(), canonical_id);
        assert_eq!(members[0].int(1).unwrap(), 1);
        let shown = linkage::reveal(&mut linkage, &keys, canonical_id, "tester", None).unwrap();
        let mut values: Vec<(String, String)> =
            shown.into_iter().map(|r| (r.id_type, r.value)).collect();
        values.sort();
        assert_eq!(
            values,
            [
                ("personnummer".to_string(), "199001011234".to_string()),
                ("personnummer".to_string(), "199001019876".to_string()),
                ("subject-code".to_string(), second.clone()),
            ]
        );

        // a map whose apply fails: two new people and a merge, with the
        // linkage store refusing to file; no subject is created and the
        // merge does not happen
        let third = pseudonym::code(Scheme::Blake2b32, KEY, "198001011111", 12).code;
        let cols = columns(&[
            ("other", "identifier:personnummer"),
            ("nummer", "canonical:personnummer"),
        ]);
        let data = rows(&[&["", "198001011111"], &["199001011234", "198001011111"]]);
        linkage
            .execute(
                "CREATE TRIGGER refuse BEFORE INSERT ON identity BEGIN SELECT RAISE(ABORT, 'the store refused'); END",
                &[],
            )
            .unwrap();
        let err = import(
            &mut registry,
            &mut linkage,
            &keys,
            Some(&derive()),
            &Map {
                columns: &cols,
                rows: &data,
                dry_run: false,
                make_types: false,
                place_id: None,
                actor: "tester@lab",
                job_id: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(
            codes(&mut registry),
            std::slice::from_ref(&first),
            "no subject behind"
        );
        assert!(!codes(&mut registry).contains(&third));
        assert_eq!(
            count(
                &mut registry,
                "SELECT COUNT(*) FROM subject WHERE merged_into IS NOT NULL"
            ),
            1,
            "the earlier merge only"
        );
        assert_eq!(
            count(&mut registry, "SELECT COUNT(*) FROM audit"),
            1,
            "the earlier merge's row only"
        );
    }

    #[test]
    fn a_map_without_an_identifier_column_or_with_two_codes_is_an_error() {
        let (mut registry, mut linkage, keys) = stores();
        let err = import(
            &mut registry,
            &mut linkage,
            &keys,
            None,
            &Map {
                columns: &columns(&[("code", "code")]),
                rows: &[],
                dry_run: true,
                make_types: false,
                place_id: None,
                actor: "",
                job_id: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("no identifier column"), "{err}");
        let err = import(
            &mut registry,
            &mut linkage,
            &keys,
            None,
            &Map {
                columns: &columns(&[
                    ("a", "identifier:patient-id"),
                    ("b", "canonical:patient-id"),
                ]),
                rows: &[],
                dry_run: true,
                make_types: false,
                place_id: None,
                actor: "",
                job_id: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("scheme and key"), "{err}");
    }
}
