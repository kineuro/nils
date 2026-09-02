// SPDX-License-Identifier: AGPL-3.0-only

//! The linkage store (`docs/specs/wave1-parse-and-digest.md`, §4.2, §7.2,
//! §7.4): the only store with identifying data. An identifier is filed as
//! a keyed lookup and a ciphertext, both under subkeys of the registry's one
//! key, so a copied file needs the key; every read of a ciphertext writes a
//! `read_audit` row. Everything here works on both backends through the
//! store, and the import is validate-then-apply.

use std::collections::{HashMap, HashSet};
use std::fmt;

use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::pseudonym::{self, ENCRYPT_DOMAIN, LOOKUP_DOMAIN};
use crate::schema::table;
use crate::store::{Error, Insert, Param, Row, Store};
use crate::time::now_iso;

/// The nonce prefixed to every ciphertext.
pub const NONCE_BYTES: usize = 24;

/// The two subkeys of §7.2, derived once per process and never stored.
pub struct Subkeys {
    k_lookup: [u8; 32],
    k_encrypt: [u8; 32],
}

impl Drop for Subkeys {
    fn drop(&mut self) {
        self.k_lookup.fill(0);
        self.k_encrypt.fill(0);
    }
}

impl fmt::Debug for Subkeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Subkeys(..)")
    }
}

impl Subkeys {
    /// Derive both subkeys from the registry's key.
    pub fn derive(key: &[u8]) -> Subkeys {
        Subkeys {
            k_lookup: pseudonym::subkey(key, LOOKUP_DOMAIN),
            k_encrypt: pseudonym::subkey(key, ENCRYPT_DOMAIN),
        }
    }

    /// The lookup of an identifier of a type (§7.4 step 2).
    pub fn lookup(&self, id_type: &str, value: &str) -> Vec<u8> {
        pseudonym::lookup(&self.k_lookup, id_type, value).to_vec()
    }

    /// The identifier under XChaCha20-Poly1305 with a fresh nonce prefixed.
    pub fn seal(&self, value: &str) -> Vec<u8> {
        let cipher = XChaCha20Poly1305::new(&Key::from(self.k_encrypt));
        let nonce = XNonce::generate();
        let sealed = cipher
            .encrypt(&nonce, value.as_bytes())
            .expect("a value short enough to seal");
        let mut out = Vec::with_capacity(NONCE_BYTES + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// The identifier back, or an error when the ciphertext was not sealed
    /// under this key or was altered.
    pub fn open(&self, ciphertext: &[u8]) -> Result<String, Error> {
        if ciphertext.len() < NONCE_BYTES {
            return Err(Error::Message(
                "the ciphertext is shorter than its nonce".to_string(),
            ));
        }
        let (nonce, sealed) = ciphertext.split_at(NONCE_BYTES);
        let nonce = XNonce::try_from(nonce).expect("a nonce of the fixed length");
        let cipher = XChaCha20Poly1305::new(&Key::from(self.k_encrypt));
        let plain = cipher.decrypt(&nonce, sealed).map_err(|_| {
            Error::Message("the ciphertext does not open under this registry's key".to_string())
        })?;
        String::from_utf8(plain)
            .map_err(|_| Error::Message("the identifier is not UTF-8".to_string()))
    }
}

/// One row of `id_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdType {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
}

/// A name for an identifier type: lower case letters, digits and hyphens,
/// like the seeded `patient-id`.
pub fn valid_id_type_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Every identifier type, by id.
pub fn id_types(store: &mut Store) -> Result<Vec<IdType>, Error> {
    let t = store.qualified("id_type");
    let rows = store.query(
        &format!("SELECT id, name, description FROM {t} ORDER BY id"),
        &[],
    )?;
    rows.iter()
        .map(|r| {
            Ok(IdType {
                id: r.int(0)?,
                name: r.text(1)?.to_string(),
                description: r.opt_text(2)?.map(str::to_string),
            })
        })
        .collect()
}

/// The id of a type by name, if it exists.
pub fn id_type_id(store: &mut Store, name: &str) -> Result<Option<i64>, Error> {
    let t = store.qualified("id_type");
    let sql = format!(
        "SELECT id FROM {t} WHERE name = {}",
        store.dialect().param(1, crate::schema::Type::Text)
    );
    match store.query_opt(&sql, &[Param::from(name)])? {
        Some(r) => Ok(Some(r.int(0)?)),
        None => Ok(None),
    }
}

/// Add an identifier type (`nils linkage id-type add`). Refuses a name that
/// exists or one that is not a name.
pub fn add_id_type(
    store: &mut Store,
    name: &str,
    description: Option<&str>,
) -> Result<IdType, Error> {
    if !valid_id_type_name(name) {
        return Err(Error::Message(format!(
            "{name:?} is not an id type name (lower case letters, digits and hyphens, like patient-id)"
        )));
    }
    if id_type_id(store, name)?.is_some() {
        return Err(Error::Message(format!("id type {name} already exists")));
    }
    let rows = store.insert(
        &Insert::new(table("id_type"), &["name", "description"]).returning(&["id"]),
        &[vec![Param::from(name), Param::from(description)]],
    )?;
    Ok(IdType {
        id: rows[0].int(0)?,
        name: name.to_string(),
        description: description.map(str::to_string),
    })
}

/// One row of `identity` without its ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub id: i64,
    pub subject_id: i64,
    pub id_type_id: i64,
    pub lookup: Vec<u8>,
}

fn identity_of(r: &Row) -> Result<Identity, Error> {
    Ok(Identity {
        id: r.int(0)?,
        subject_id: r.int(1)?,
        id_type_id: r.int(2)?,
        lookup: r.bytes(3)?.to_vec(),
    })
}

const IDENTITY_COLUMNS: [&str; 4] = ["id", "subject_id", "id_type_id", "lookup"];

/// The identity rows whose lookup is one of `lookups` (§7.4 step 3).
pub fn identities_by_lookup(
    store: &mut Store,
    lookups: &[Vec<u8>],
) -> Result<Vec<Identity>, Error> {
    if lookups.is_empty() {
        return Ok(Vec::new());
    }
    let t = table("identity");
    let cols: Vec<_> = IDENTITY_COLUMNS
        .iter()
        .map(|c| t.column(c).unwrap())
        .collect();
    store
        .select_by_bytes(t, &cols, "lookup", lookups)?
        .iter()
        .map(identity_of)
        .collect()
}

/// The identity rows of the listed subjects (§7.4 step 5).
pub fn identities_of_subjects(store: &mut Store, subjects: &[i64]) -> Result<Vec<Identity>, Error> {
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let t = table("identity");
    let cols: Vec<_> = IDENTITY_COLUMNS
        .iter()
        .map(|c| t.column(c).unwrap())
        .collect();
    store
        .select_by_ids(t, &cols, "subject_id", subjects)?
        .iter()
        .map(identity_of)
        .collect()
}

/// An identity row to file.
#[derive(Debug, Clone)]
pub struct NewIdentity {
    pub subject_id: i64,
    pub id_type_id: i64,
    pub lookup: Vec<u8>,
    pub ciphertext: Vec<u8>,
    /// `dicom`, `csv` or `manual`.
    pub source: &'static str,
    pub first_batch_id: Option<i64>,
}

/// File identity rows. A row whose `(id_type_id, lookup)` exists is an error:
/// the caller resolved the identity first and would not write it twice.
pub fn insert_identities(store: &mut Store, rows: &[NewIdentity]) -> Result<u64, Error> {
    if rows.is_empty() {
        return Ok(0);
    }
    let now = now_iso();
    let values: Vec<Vec<Param>> = rows
        .iter()
        .map(|r| {
            vec![
                Param::from(r.subject_id),
                Param::from(r.id_type_id),
                Param::from(r.lookup.clone()),
                Param::from(r.ciphertext.clone()),
                Param::from(r.source),
                Param::from(r.first_batch_id),
                Param::from(now.as_str()),
            ]
        })
        .collect();
    store.insert(&Insert::all(table("identity")), &values)?;
    Ok(rows.len() as u64)
}

/// An identifier read back: the row, its type and the value in clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revealed {
    pub identity_id: i64,
    pub id_type: String,
    pub source: String,
    pub value: String,
}

/// Decrypt every identifier of a subject (`nils linkage show`), writing one
/// `read_audit` row per identifier before the value is returned.
pub fn reveal(
    store: &mut Store,
    keys: &Subkeys,
    subject_id: i64,
    actor: &str,
    why: Option<&str>,
) -> Result<Vec<Revealed>, Error> {
    let identity = store.qualified("identity");
    let id_type = store.qualified("id_type");
    let sql = format!(
        "SELECT i.id, t.name, i.source, i.ciphertext FROM {identity} i JOIN {id_type} t ON t.id = i.id_type_id WHERE i.subject_id = {} ORDER BY i.id",
        store.dialect().param(1, crate::schema::Type::Int)
    );
    let rows = store.query(&sql, &[Param::from(subject_id)])?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let now = now_iso();
    let audit: Vec<Vec<Param>> = rows
        .iter()
        .map(|r| {
            Ok(vec![
                Param::from(now.as_str()),
                Param::from(actor),
                Param::from(r.int(0)?),
                Param::from(why),
            ])
        })
        .collect::<Result<_, Error>>()?;
    store.begin()?;
    let written = store.insert(&Insert::all(table("read_audit")), &audit);
    match written {
        Ok(_) => store.commit()?,
        Err(e) => {
            let _ = store.rollback();
            return Err(e);
        }
    }
    rows.iter()
        .map(|r| {
            Ok(Revealed {
                identity_id: r.int(0)?,
                id_type: r.text(1)?.to_string(),
                source: r.text(2)?.to_string(),
                value: keys.open(r.bytes(3)?)?,
            })
        })
        .collect()
}

/// Record that two subjects are one person (`nils linkage link`): `a` is
/// canonical, `b` the alias. Returns the linkage id.
pub fn link(
    store: &mut Store,
    subject_a: i64,
    subject_b: i64,
    evidence: &str,
    actor: &str,
) -> Result<i64, Error> {
    if subject_a == subject_b {
        return Err(Error::Message(
            "a subject cannot be linked to itself".to_string(),
        ));
    }
    let evidence = serde_json::json!({ "text": evidence }).to_string();
    let rows = store.insert(
        &Insert::new(
            table("linkage"),
            &[
                "subject_a",
                "subject_b",
                "kind",
                "evidence",
                "actor",
                "created_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(subject_a),
            Param::from(subject_b),
            Param::from("same-person"),
            Param::from(evidence),
            Param::from(actor),
            Param::from(now_iso()),
        ]],
    )?;
    rows[0].int(0)
}

/// Reverse a linkage (`nils linkage unlink`): a column, never row surgery.
/// Returns false when no open linkage has the id.
pub fn unlink(store: &mut Store, id: i64, actor: &str) -> Result<bool, Error> {
    let t = store.qualified("linkage");
    let d = store.dialect();
    let sql = format!(
        "UPDATE {t} SET reversed_at = {}, reversed_by = {} WHERE id = {} AND reversed_at IS NULL",
        d.param(1, crate::schema::Type::Timestamp),
        d.param(2, crate::schema::Type::Text),
        d.param(3, crate::schema::Type::Int)
    );
    let n = store.execute(
        &sql,
        &[Param::from(now_iso()), Param::from(actor), Param::from(id)],
    )?;
    Ok(n > 0)
}

/// What `purge` removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Purged {
    pub identities: u64,
    pub linkages: u64,
}

/// What the linkage store holds, for the custody table: identity rows, the
/// subjects they belong to, open linkages, audited reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Holdings {
    pub identities: u64,
    pub subjects: u64,
    pub linkages: u64,
    pub reads: u64,
}

pub fn holdings(store: &mut Store) -> Result<Holdings, Error> {
    let count = |store: &mut Store, sql: String| -> Result<u64, Error> {
        Ok(u64::try_from(store.query(&sql, &[])?[0].int(0)?).unwrap_or(0))
    };
    let identity = store.qualified("identity");
    let linkage = store.qualified("linkage");
    let audit = store.qualified("read_audit");
    Ok(Holdings {
        identities: count(store, format!("SELECT COUNT(*) FROM {identity}"))?,
        subjects: count(
            store,
            format!("SELECT COUNT(DISTINCT subject_id) FROM {identity}"),
        )?,
        linkages: count(
            store,
            format!("SELECT COUNT(*) FROM {linkage} WHERE reversed_at IS NULL"),
        )?,
        reads: count(store, format!("SELECT COUNT(*) FROM {audit}"))?,
    })
}

/// Delete the identity rows and the linkages of one subject, or of every
/// subject when `subject` is none (§13 `linkage purge`). The id types and
/// the read audit stay: the audit is the record that a read happened, not
/// what was read. The registry's subjects are untouched, so a digest of the
/// same sources files the identities again.
pub fn purge(store: &mut Store, subject: Option<i64>) -> Result<Purged, Error> {
    let identity = store.qualified("identity");
    let linkage = store.qualified("linkage");
    let d = store.dialect();
    store.begin()?;
    let result = (|| -> Result<Purged, Error> {
        let (linkages, identities) = match subject {
            Some(id) => (
                store.execute(
                    &format!(
                        "DELETE FROM {linkage} WHERE subject_a = {} OR subject_b = {}",
                        d.param(1, crate::schema::Type::Int),
                        d.param(2, crate::schema::Type::Int)
                    ),
                    &[Param::from(id), Param::from(id)],
                )?,
                store.execute(
                    &format!(
                        "DELETE FROM {identity} WHERE subject_id = {}",
                        d.param(1, crate::schema::Type::Int)
                    ),
                    &[Param::from(id)],
                )?,
            ),
            None => (
                store.execute(&format!("DELETE FROM {linkage}"), &[])?,
                store.execute(&format!("DELETE FROM {identity}"), &[])?,
            ),
        };
        Ok(Purged {
            identities,
            linkages,
        })
    })();
    match result {
        Ok(p) => {
            store.commit()?;
            Ok(p)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// One row of `linkage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linkage {
    pub id: i64,
    pub subject_a: i64,
    pub subject_b: i64,
    pub kind: String,
    pub evidence: String,
    pub actor: Option<String>,
    pub created_at: String,
    pub reversed_at: Option<String>,
    pub reversed_by: Option<String>,
}

/// The linkages that name a subject, open and reversed, oldest first.
pub fn linkages_of(store: &mut Store, subject_id: i64) -> Result<Vec<Linkage>, Error> {
    let t = table("linkage");
    let cols: Vec<_> = [
        "id",
        "subject_a",
        "subject_b",
        "kind",
        "evidence",
        "actor",
        "created_at",
        "reversed_at",
        "reversed_by",
    ]
    .iter()
    .map(|c| t.column(c).unwrap())
    .collect();
    let d = store.dialect();
    let exprs: Vec<String> = cols.iter().map(|c| d.text_of(c)).collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE subject_a = {} OR subject_b = {} ORDER BY id",
        exprs.join(", "),
        store.qualified("linkage"),
        d.param(1, crate::schema::Type::Int),
        d.param(2, crate::schema::Type::Int)
    );
    let rows = store.query(&sql, &[Param::from(subject_id), Param::from(subject_id)])?;
    rows.iter()
        .map(|r| {
            Ok(Linkage {
                id: r.int(0)?,
                subject_a: r.int(1)?,
                subject_b: r.int(2)?,
                kind: r.text(3)?.to_string(),
                evidence: r.text(4)?.to_string(),
                actor: r.opt_text(5)?.map(str::to_string),
                created_at: r.text(6)?.to_string(),
                reversed_at: r.opt_text(7)?.map(str::to_string),
                reversed_by: r.opt_text(8)?.map(str::to_string),
            })
        })
        .collect()
}

/// A subject as the registry has it: id and code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub id: i64,
    pub code: String,
}

/// The subjects with the given codes.
pub fn subjects_by_code(registry: &mut Store, codes: &[String]) -> Result<Vec<Subject>, Error> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    let t = table("subject");
    let cols = [t.column("id").unwrap(), t.column("code").unwrap()];
    registry
        .select_by_keys(t, &cols, "code", codes)?
        .iter()
        .map(|r| {
            Ok(Subject {
                id: r.int(0)?,
                code: r.text(1)?.to_string(),
            })
        })
        .collect()
}

/// The subjects with the given ids.
pub fn subjects_by_id(registry: &mut Store, ids: &[i64]) -> Result<Vec<Subject>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let t = table("subject");
    let cols = [t.column("id").unwrap(), t.column("code").unwrap()];
    registry
        .select_by_ids(t, &cols, "id", ids)?
        .iter()
        .map(|r| {
            Ok(Subject {
                id: r.int(0)?,
                code: r.text(1)?.to_string(),
            })
        })
        .collect()
}

/// What an import found wrong, listed before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportFault {
    /// The identifier is empty, or the code is.
    Empty { line: usize },
    /// The identifier appears twice in the file with two codes.
    IdentifierRepeated { line: usize, first_line: usize },
    /// Two identifiers of the file share one code.
    CodeRepeated { line: usize, first_line: usize },
    /// The identifier already maps to another code in the registry.
    IdentifierMapped { line: usize, code: String },
    /// The code exists under another identifier of the same type.
    CodeTaken { line: usize },
}

impl fmt::Display for ImportFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportFault::Empty { line } => write!(f, "line {line}: an empty identifier or code"),
            ImportFault::IdentifierRepeated { line, first_line } => write!(
                f,
                "line {line}: the identifier appeared on line {first_line} with another code"
            ),
            ImportFault::CodeRepeated { line, first_line } => write!(
                f,
                "line {line}: the code appeared on line {first_line} with another identifier"
            ),
            ImportFault::IdentifierMapped { line, code } => write!(
                f,
                "line {line}: the identifier already maps to subject {code}"
            ),
            ImportFault::CodeTaken { line } => write!(
                f,
                "line {line}: the code exists under another identifier of this type"
            ),
        }
    }
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub rows: usize,
    pub subjects_created: usize,
    pub identities_added: usize,
    pub unchanged: usize,
}

/// The rows of an import: `(identifier, code)` pairs with their line numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRow {
    pub line: usize,
    pub identifier: String,
    pub code: String,
}

/// `nils linkage import` (§7.4): the subject per code exactly as given, the
/// identifier filed under `id_type` with source `csv`. Validate-then-apply:
/// every fault is returned and nothing is written when there is one.
pub fn import(
    registry: &mut Store,
    linkage: &mut Store,
    keys: &Subkeys,
    id_type: &str,
    rows: &[ImportRow],
) -> Result<ImportReport, ImportError> {
    let type_id = id_type_id(linkage, id_type)?.ok_or_else(|| {
        Error::Message(format!(
            "no id type named {id_type}; nils linkage id-type list shows them, id-type add creates one"
        ))
    })?;
    let mut faults = Vec::new();
    // within the file
    let mut by_identifier: HashMap<&str, (usize, &str)> = HashMap::new();
    let mut by_code: HashMap<&str, (usize, &str)> = HashMap::new();
    let mut distinct: Vec<&ImportRow> = Vec::new();
    for r in rows {
        if r.identifier.is_empty() || r.code.is_empty() {
            faults.push(ImportFault::Empty { line: r.line });
            continue;
        }
        match by_identifier.get(r.identifier.as_str()) {
            Some(&(first_line, code)) => {
                if code != r.code {
                    faults.push(ImportFault::IdentifierRepeated {
                        line: r.line,
                        first_line,
                    });
                }
                continue;
            }
            None => {
                by_identifier.insert(&r.identifier, (r.line, &r.code));
            }
        }
        match by_code.get(r.code.as_str()) {
            Some(&(first_line, _)) => {
                faults.push(ImportFault::CodeRepeated {
                    line: r.line,
                    first_line,
                });
                continue;
            }
            None => {
                by_code.insert(&r.code, (r.line, &r.identifier));
            }
        }
        distinct.push(r);
    }
    // against the stores
    let lookups: Vec<Vec<u8>> = distinct
        .iter()
        .map(|r| keys.lookup(id_type, &r.identifier))
        .collect();
    let existing: HashMap<Vec<u8>, Identity> = identities_by_lookup(linkage, &lookups)?
        .into_iter()
        .filter(|i| i.id_type_id == type_id)
        .map(|i| (i.lookup.clone(), i))
        .collect();
    let codes: Vec<String> = distinct.iter().map(|r| r.code.clone()).collect();
    let subjects: HashMap<String, i64> = subjects_by_code(registry, &codes)?
        .into_iter()
        .map(|s| (s.code, s.id))
        .collect();
    let subject_ids: Vec<i64> = subjects.values().copied().collect();
    let subject_codes: HashMap<i64, String> =
        subjects.iter().map(|(c, id)| (*id, c.clone())).collect();
    let mut typed: HashMap<i64, HashSet<Vec<u8>>> = HashMap::new();
    for i in identities_of_subjects(linkage, &subject_ids)? {
        if i.id_type_id == type_id {
            typed.entry(i.subject_id).or_default().insert(i.lookup);
        }
    }
    let mut to_create: Vec<&ImportRow> = Vec::new();
    let mut to_file: Vec<(&ImportRow, i64, Vec<u8>)> = Vec::new();
    let mut unchanged = 0;
    for (r, lookup) in distinct.iter().zip(lookups) {
        if let Some(i) = existing.get(&lookup) {
            match subject_codes.get(&i.subject_id) {
                Some(code) if *code == r.code => unchanged += 1,
                Some(code) => faults.push(ImportFault::IdentifierMapped {
                    line: r.line,
                    code: code.clone(),
                }),
                None => {
                    // the identity names a subject with another code
                    let code = subjects_by_id(registry, &[i.subject_id])?
                        .into_iter()
                        .next()
                        .map(|s| s.code)
                        .unwrap_or_else(|| format!("#{}", i.subject_id));
                    faults.push(ImportFault::IdentifierMapped { line: r.line, code });
                }
            }
            continue;
        }
        match subjects.get(&r.code) {
            Some(&subject_id) => {
                let taken = typed
                    .get(&subject_id)
                    .is_some_and(|set| !set.is_empty() && !set.contains(&lookup));
                if taken {
                    faults.push(ImportFault::CodeTaken { line: r.line });
                } else {
                    to_file.push((r, subject_id, lookup));
                }
            }
            None => {
                to_create.push(r);
                to_file.push((r, 0, lookup));
            }
        }
    }
    if !faults.is_empty() {
        faults.sort_by_key(|f| match f {
            ImportFault::Empty { line }
            | ImportFault::IdentifierRepeated { line, .. }
            | ImportFault::CodeRepeated { line, .. }
            | ImportFault::IdentifierMapped { line, .. }
            | ImportFault::CodeTaken { line } => *line,
        });
        return Err(ImportError::Faults(faults));
    }
    // apply: the registry first, then the linkage store (§9.3)
    let now = now_iso();
    let mut created: HashMap<String, i64> = HashMap::new();
    if !to_create.is_empty() {
        let t = table("subject");
        let values: Vec<Vec<Param>> = to_create
            .iter()
            .map(|r| vec![Param::from(r.code.as_str()), Param::from(now.as_str())])
            .collect();
        registry.begin()?;
        let inserted = registry.insert(
            &Insert::new(t, &["code", "created_at"]).returning(&["id", "code"]),
            &values,
        );
        let inserted = match inserted {
            Ok(rows) => rows,
            Err(e) => {
                let _ = registry.rollback();
                return Err(e.into());
            }
        };
        registry.commit()?;
        for row in &inserted {
            created.insert(row.text(1)?.to_string(), row.int(0)?);
        }
    }
    let mut new_rows = Vec::with_capacity(to_file.len());
    for (r, subject_id, lookup) in to_file {
        let subject_id = if subject_id == 0 {
            *created.get(&r.code).ok_or_else(|| {
                Error::Message(format!("line {}: the subject was not created", r.line))
            })?
        } else {
            subject_id
        };
        new_rows.push(NewIdentity {
            subject_id,
            id_type_id: type_id,
            lookup,
            ciphertext: keys.seal(&r.identifier),
            source: "csv",
            first_batch_id: None,
        });
    }
    linkage.begin()?;
    match insert_identities(linkage, &new_rows) {
        Ok(_) => linkage.commit()?,
        Err(e) => {
            let _ = linkage.rollback();
            return Err(e.into());
        }
    }
    Ok(ImportReport {
        rows: rows.len(),
        subjects_created: created.len(),
        identities_added: new_rows.len(),
        unchanged,
    })
}

/// Why an import did not happen.
#[derive(Debug)]
pub enum ImportError {
    /// The rows listed are wrong; nothing was written.
    Faults(Vec<ImportFault>),
    Store(Error),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Faults(faults) => {
                writeln!(f, "{} row(s) refused; nothing was written:", faults.len())?;
                for fault in faults {
                    writeln!(f, "  {fault}")?;
                }
                Ok(())
            }
            ImportError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<Error> for ImportError {
    fn from(e: Error) -> ImportError {
        ImportError::Store(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};

    const KEY: &[u8] = b"nils-fixture-key";

    #[test]
    fn the_subkeys_match_the_fixture_of_7_2() {
        let keys = Subkeys::derive(KEY);
        assert_eq!(
            hex::encode(keys.k_lookup),
            "d7d3eeb7a8fb4fc9c1cdd83c215c93fabef487366ee678717f8edd0935336fa0"
        );
        assert_eq!(
            hex::encode(keys.k_encrypt),
            "1313a85029438352d9ebb2b8f4b03f32390dfd160355b1ace070bb40f87aabc2"
        );
        assert_eq!(
            hex::encode(keys.lookup("patient-id", "PID-0001")),
            "a548a6fa8cf22772d1de1ee342ff8bd7460c15b1c01e0e189f297cf8a168bd0c"
        );
    }

    #[test]
    fn a_sealed_identifier_opens_under_the_same_key_only() {
        let keys = Subkeys::derive(KEY);
        let a = keys.seal("PID-0001");
        let b = keys.seal("PID-0001");
        assert_ne!(a, b, "a fresh nonce every time");
        assert_eq!(a.len(), NONCE_BYTES + 8 + 16);
        assert_eq!(keys.open(&a).unwrap(), "PID-0001");
        assert_eq!(keys.open(&b).unwrap(), "PID-0001");
        let other = Subkeys::derive(b"another-key");
        assert!(other.open(&a).is_err());
        let mut altered = a.clone();
        altered[NONCE_BYTES] ^= 1;
        assert!(keys.open(&altered).is_err());
        assert!(keys.open(&a[..10]).is_err());
    }

    #[test]
    fn id_types_are_seeded_and_added() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Linkage).unwrap();
        let names: Vec<String> = id_types(&mut store)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["patient-id", "study-instance-uid"]);
        assert_eq!(id_type_id(&mut store, "patient-id").unwrap(), Some(1));
        assert_eq!(id_type_id(&mut store, "nope").unwrap(), None);
        let t = add_id_type(&mut store, "personal-number", Some("the Swedish one")).unwrap();
        assert_eq!(t.id, 3);
        assert!(add_id_type(&mut store, "personal-number", None).is_err());
        assert!(add_id_type(&mut store, "Personal Number", None).is_err());
        assert!(valid_id_type_name("a1-b2"));
        assert!(!valid_id_type_name("-a"));
        assert!(!valid_id_type_name(""));
    }

    #[test]
    fn identities_are_filed_looked_up_and_revealed_with_an_audit_row() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Linkage).unwrap();
        let keys = Subkeys::derive(KEY);
        let lookup = keys.lookup("patient-id", "PID-0001");
        insert_identities(
            &mut store,
            &[NewIdentity {
                subject_id: 7,
                id_type_id: 1,
                lookup: lookup.clone(),
                ciphertext: keys.seal("PID-0001"),
                source: "dicom",
                first_batch_id: Some(1),
            }],
        )
        .unwrap();
        let found = identities_by_lookup(&mut store, &[lookup.clone(), vec![0; 32]]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].subject_id, 7);
        assert_eq!(found[0].lookup, lookup);
        assert_eq!(identities_of_subjects(&mut store, &[7]).unwrap().len(), 1);
        assert!(identities_of_subjects(&mut store, &[8]).unwrap().is_empty());
        // the same (type, lookup) twice is refused
        assert!(
            insert_identities(
                &mut store,
                &[NewIdentity {
                    subject_id: 8,
                    id_type_id: 1,
                    lookup: lookup.clone(),
                    ciphertext: keys.seal("PID-0001"),
                    source: "dicom",
                    first_batch_id: Some(1),
                }],
            )
            .is_err()
        );
        let shown = reveal(&mut store, &keys, 7, "tester", Some("a test")).unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id_type, "patient-id");
        assert_eq!(shown[0].source, "dicom");
        assert_eq!(shown[0].value, "PID-0001");
        let audit = store
            .query("SELECT actor, identity_id, why FROM read_audit", &[])
            .unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].text(0).unwrap(), "tester");
        assert_eq!(audit[0].int(1).unwrap(), shown[0].identity_id);
        assert_eq!(audit[0].text(2).unwrap(), "a test");
        assert!(
            reveal(&mut store, &keys, 9, "tester", None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn linkages_are_recorded_and_reversed_in_place() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Linkage).unwrap();
        assert!(link(&mut store, 1, 1, "x", "tester").is_err());
        let id = link(&mut store, 1, 2, "same birth date and initials", "tester").unwrap();
        let of_1 = linkages_of(&mut store, 1).unwrap();
        assert_eq!(of_1.len(), 1);
        assert_eq!(of_1[0].kind, "same-person");
        assert_eq!(of_1[0].reversed_at, None);
        assert!(of_1[0].evidence.contains("initials"));
        assert!(unlink(&mut store, id, "tester").unwrap());
        assert!(
            !unlink(&mut store, id, "tester").unwrap(),
            "already reversed"
        );
        assert!(!unlink(&mut store, 99, "tester").unwrap());
        let of_2 = linkages_of(&mut store, 2).unwrap();
        assert_eq!(of_2[0].reversed_by.as_deref(), Some("tester"));
        assert!(of_2[0].reversed_at.is_some());
    }

    fn rows(pairs: &[(&str, &str)]) -> Vec<ImportRow> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, (identifier, code))| ImportRow {
                line: i + 2,
                identifier: identifier.to_string(),
                code: code.to_string(),
            })
            .collect()
    }

    #[test]
    fn an_import_validates_then_applies_and_reproduces_its_codes() {
        let mut registry = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut registry, Kind::Registry).unwrap();
        let mut linkage = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut linkage, Kind::Linkage).unwrap();
        let keys = Subkeys::derive(KEY);
        let unknown = import(
            &mut registry,
            &mut linkage,
            &keys,
            "nope",
            &rows(&[("a", "b")]),
        );
        assert!(
            unknown
                .unwrap_err()
                .to_string()
                .contains("no id type named nope")
        );

        let report = import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[("PID-0001", "771c4326c89c082c"), ("PID-0002", "sub-two")]),
        )
        .unwrap();
        assert_eq!(
            report,
            ImportReport {
                rows: 2,
                subjects_created: 2,
                identities_added: 2,
                unchanged: 0
            }
        );
        let subjects = registry
            .query(
                "SELECT code, code_digest, first_batch_id FROM subject ORDER BY id",
                &[],
            )
            .unwrap();
        assert_eq!(subjects[0].text(0).unwrap(), "771c4326c89c082c");
        assert!(matches!(subjects[0].get(1), crate::store::Cell::Null));
        assert_eq!(subjects[0].opt_int(2).unwrap(), None);
        assert_eq!(subjects[1].text(0).unwrap(), "sub-two");
        let shown = reveal(&mut linkage, &keys, 2, "tester", None).unwrap();
        assert_eq!(shown[0].value, "PID-0002");
        assert_eq!(shown[0].source, "csv");

        // the same file again changes nothing
        let again = import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[("PID-0001", "771c4326c89c082c"), ("PID-0002", "sub-two")]),
        )
        .unwrap();
        assert_eq!(again.unchanged, 2);
        assert_eq!(again.subjects_created, 0);
        assert_eq!(again.identities_added, 0);

        // every fault is listed and nothing is written
        let err = import(
            &mut registry,
            &mut linkage,
            &keys,
            "patient-id",
            &rows(&[
                ("PID-0001", "sub-other"),
                ("PID-0003", "sub-two"),
                ("PID-0004", "sub-four"),
                ("PID-0004", "sub-five"),
                ("PID-0006", "sub-four"),
                ("", "sub-seven"),
            ]),
        )
        .unwrap_err();
        let ImportError::Faults(faults) = err else {
            panic!("expected faults")
        };
        assert_eq!(
            faults,
            vec![
                ImportFault::IdentifierMapped {
                    line: 2,
                    code: "771c4326c89c082c".to_string()
                },
                ImportFault::CodeTaken { line: 3 },
                ImportFault::IdentifierRepeated {
                    line: 5,
                    first_line: 4
                },
                ImportFault::CodeRepeated {
                    line: 6,
                    first_line: 4
                },
                ImportFault::Empty { line: 7 },
            ]
        );
        let n = registry.query("SELECT COUNT(*) FROM subject", &[]).unwrap();
        assert_eq!(n[0].int(0).unwrap(), 2);
        let n = linkage.query("SELECT COUNT(*) FROM identity", &[]).unwrap();
        assert_eq!(n[0].int(0).unwrap(), 2);

        // a second type attaches to an existing subject
        add_id_type(&mut linkage, "personal-number", None).unwrap();
        let report = import(
            &mut registry,
            &mut linkage,
            &keys,
            "personal-number",
            &rows(&[("19800101-1234", "sub-two")]),
        )
        .unwrap();
        assert_eq!(report.subjects_created, 0);
        assert_eq!(report.identities_added, 1);
        let shown = reveal(&mut linkage, &keys, 2, "tester", None).unwrap();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[1].id_type, "personal-number");
    }
}
