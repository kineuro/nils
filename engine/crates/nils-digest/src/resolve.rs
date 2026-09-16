// SPDX-License-Identifier: AGPL-3.0-only

//! Who a file is about (§7.4), in one place: the digest's writer resolves
//! every parsed file through this, and so does the pseudonymiser (record 26
//! §3), so that the same registry and key name the same person in a
//! dataset's originals and in its pseudonymised tree.
//!
//! The lookup of a file's identifier names its subject when the linkage
//! store has met it (step 3); otherwise the code is derived and the subject
//! created (step 4), or found by its code and the identity attached, or
//! refused as a collision (step 5). The identity rows are filed after the
//! registry's transaction commits (§9.3). A caller may ask that no subject
//! be made, in which case an identifier no subject holds comes back as
//! unknown, with nothing written: that is how the pseudonymiser holds a
//! file for want of a map (record 26 §4).

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;

use lru::LruCache;
use nils_registry::dialect::Conflict;
use nils_registry::linkage::{self, NewIdentity, Subkeys};
use nils_registry::schema::table;
use nils_registry::store::{Insert, Param, Store};
use nils_registry::{HomeError, Registry, Scheme, pseudonym};

use crate::batch::Fields;
use crate::rule::{FALLBACK_ID_TYPE, Ident, Rule};

/// Rows the lookup cache holds (§9.1).
pub const CACHE_ROWS: usize = 200_000;

/// An id type the rule files under, with its row in the linkage store.
pub struct IdType {
    pub name: String,
    pub id: i64,
}

/// A collision met in a batch (§7.4 step 5): the review item to open once
/// the batch is rolled back.
#[derive(Debug, Clone)]
pub struct Collision {
    pub code: String,
    pub subject_id: Option<i64>,
    pub id_type: String,
    /// `identity`: the subject holds another identifier of the type;
    /// `display-code`: its digest is another one's (blake2b-32, §7.1);
    /// `batch`: two identifiers of this batch derive the one code.
    pub reason: &'static str,
}

/// Why a batch could not be resolved.
#[derive(Debug)]
pub enum ResolveError {
    Collision(Collision),
    Home(HomeError),
}

impl From<HomeError> for ResolveError {
    fn from(e: HomeError) -> ResolveError {
        ResolveError::Home(e)
    }
}

impl From<nils_registry::Error> for ResolveError {
    fn from(e: nils_registry::Error) -> ResolveError {
        ResolveError::Home(HomeError::Store(e))
    }
}

/// One file to resolve: its identifier as the rule read it, and the
/// subject row it would make, as the catalogue's subject columns in order;
/// nulls from a caller that reads none, which the digest fills later.
pub struct Who<'a> {
    pub ident: &'a Ident,
    pub subject: Vec<Param>,
}

/// How a file's subject was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// By its lookup or by its code: the subject stood.
    Known(i64),
    /// Made in this call.
    Created(i64),
    /// No subject holds the identifier, and the call made none.
    Unknown,
}

impl Found {
    pub fn id(self) -> Option<i64> {
        match self {
            Found::Known(id) | Found::Created(id) => Some(id),
            Found::Unknown => None,
        }
    }
}

/// What one call did.
#[derive(Debug, Default)]
pub struct Resolved {
    pub found: Vec<Found>,
    /// Files whose lookup the linkage store held.
    pub matched: u64,
    pub created: u64,
    /// Identities attached to a subject found by its code.
    pub attached: u64,
}

/// Whether a subject may be made for an identifier no subject holds, and
/// how it is marked when it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Make {
    /// Never: an unknown identifier is answered as such.
    Nothing,
    /// A subject as the digest makes one.
    Subject,
    /// A subject marked provisional (record 26 §4): coded from an
    /// identifier no map named.
    Provisional,
}

pub struct Resolver {
    /// The linkage store, on its own connection (§9.3).
    linkage: Store,
    /// The pseudonym key: read from the key store, written nowhere (§7.2).
    key: Vec<u8>,
    /// The subkeys of the linkage store, derived once (§7.2).
    keys: Subkeys,
    scheme: Scheme,
    display_length: usize,
    /// The rule's id type and the fallback's.
    id_type: IdType,
    fallback: IdType,
    /// The rule reads the code itself, not an identifier to derive one from.
    verbatim: bool,
    batch_id: i64,
    /// Lookup → subject, for the identities met (§7.4 step 3).
    identities: LruCache<Vec<u8>, i64>,
    /// The identity rows of the batch in flight, filed once it commits.
    pending: Vec<NewIdentity>,
    /// The catalogue's subject columns, in order.
    subject_columns: Vec<&'static str>,
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

impl Resolver {
    pub fn new(registry: &mut Registry, rule: &Rule, batch_id: i64) -> Result<Resolver, HomeError> {
        let key = registry.pseudonym_key()?;
        let keys = Subkeys::derive(&key);
        let mut linkage = registry.open_linkage()?;
        let id_type = id_type_of(&mut linkage, &rule.id_type)?;
        let fallback = id_type_of(&mut linkage, FALLBACK_ID_TYPE)?;
        let meta = registry.meta();
        let cap = NonZeroUsize::new(CACHE_ROWS).unwrap_or(NonZeroUsize::MIN);
        Ok(Resolver {
            linkage,
            key,
            keys,
            scheme: meta.pseudonym_scheme,
            display_length: meta.display_length,
            id_type,
            fallback,
            verbatim: rule.verbatim,
            batch_id,
            identities: LruCache::new(cap),
            pending: Vec::new(),
            subject_columns: Fields::subject().names,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    pub fn display_length(&self) -> usize {
        self.display_length
    }

    /// The id type a file's identifier is filed under.
    pub fn type_of(&self, ident: &Ident) -> &IdType {
        if ident.fell_back {
            &self.fallback
        } else {
            &self.id_type
        }
    }

    /// The keyed lookup of a file's identifier (§7.4 step 2), which is what
    /// a held file keeps in place of the identifier.
    pub fn lookup(&self, ident: &Ident) -> Vec<u8> {
        self.keys.lookup(&self.type_of(ident).name, &ident.value)
    }

    /// An identifier sealed under the store's encrypt key, for the one door
    /// that reveals it.
    pub fn seal(&self, value: &str) -> Vec<u8> {
        self.keys.seal(value)
    }

    /// The subject of every file (§7.4): by the lookup of its identifier
    /// when the linkage store has met it (step 3); else by the code, created
    /// (step 4), or found and the identity attached, or a collision (step
    /// 5). The subject rows go into the caller's transaction on `store`;
    /// the identity rows wait for [`Resolver::file_identities`].
    pub fn resolve(
        &mut self,
        store: &mut Store,
        who: &[Who<'_>],
        now: &str,
        make: Make,
    ) -> Result<Resolved, ResolveError> {
        let t = table("subject");
        let n = who.len();
        let mut out = Resolved {
            found: vec![Found::Unknown; n],
            ..Resolved::default()
        };
        // the lookups the cache does not hold, with the first file of each
        let mut misses: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut lookups: Vec<Vec<u8>> = Vec::with_capacity(n);
        for (i, w) in who.iter().enumerate() {
            let lookup = self.lookup(w.ident);
            match self.identities.get(&lookup) {
                Some(&id) => out.found[i] = Found::Known(id),
                None => {
                    misses.entry(lookup.clone()).or_insert(i);
                }
            }
            lookups.push(lookup);
        }
        if !misses.is_empty() {
            let keys: Vec<Vec<u8>> = misses.keys().cloned().collect();
            for row in linkage::identities_by_lookup(&mut self.linkage, &keys)? {
                self.identities.put(row.lookup.clone(), row.subject_id);
                out.matched += 1;
                misses.remove(&row.lookup);
            }
        }
        if make == Make::Nothing {
            // the rest are identifiers no subject holds, and none is made
            for (i, lookup) in lookups.iter().enumerate() {
                if let Some(&id) = self.identities.get(lookup) {
                    out.found[i] = Found::Known(id);
                }
            }
            return Ok(out);
        }
        // the rest are identifiers no subject holds: by code, deduplicated
        // in the batch; two identifiers of one type on one code collide
        struct Group {
            digest: Vec<u8>,
            /// The misses on the code: (lookup, first file).
            members: Vec<(Vec<u8>, usize)>,
        }
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for (lookup, &i) in &misses {
            let w = &who[i];
            let code = if self.verbatim && !w.ident.fell_back {
                // the value the rule read is the code itself (§7.3)
                pseudonym::verbatim(self.scheme, &self.key, &w.ident.value)
            } else {
                pseudonym::code(self.scheme, &self.key, &w.ident.value, self.display_length)
            };
            let g = groups.entry(code.code).or_insert_with(|| Group {
                digest: code.digest,
                members: Vec::new(),
            });
            g.members.push((lookup.clone(), i));
        }
        for (code, g) in &mut groups {
            g.members.sort_by_key(|(_, i)| *i);
            for (k, (_, i)) in g.members.iter().enumerate() {
                let ty = &self.type_of(who[*i].ident).name;
                if g.members[..k]
                    .iter()
                    .any(|(_, j)| &self.type_of(who[*j].ident).name == ty)
                {
                    return Err(ResolveError::Collision(Collision {
                        code: code.clone(),
                        subject_id: None,
                        id_type: ty.clone(),
                        reason: "batch",
                    }));
                }
            }
        }
        if !groups.is_empty() {
            let mut columns: Vec<&str> = vec!["code", "code_digest"];
            columns.extend(self.subject_columns.iter().copied());
            columns.extend(["first_batch_id", "created_at", "provisional"]);
            let mut rows = Vec::with_capacity(groups.len());
            for (code, g) in &groups {
                let w = &who[g.members[0].1];
                let mut row = vec![Param::from(code.as_str()), Param::Bytes(g.digest.clone())];
                if w.subject.len() == self.subject_columns.len() {
                    row.extend(w.subject.iter().cloned());
                } else {
                    row.extend(std::iter::repeat_n(Param::Null, self.subject_columns.len()));
                }
                row.push(Param::Int(self.batch_id));
                row.push(Param::from(now));
                row.push(match make {
                    Make::Provisional => Param::Int(1),
                    _ => Param::Null,
                });
                rows.push(row);
            }
            let spec = Insert::new(t, &columns)
                .on_conflict(Conflict::Nothing(&["code"]))
                .returning(&["id", "code"]);
            let returned = store.insert(&spec, &rows)?;
            // code → subject id, for the created and then the found
            let mut by_code: HashMap<String, i64> = HashMap::with_capacity(groups.len());
            for r in &returned {
                let id = r.int(0)?;
                let code = r.text(1)?;
                by_code.insert(code.to_string(), id);
                if let Some(g) = groups.get(code) {
                    for (lookup, i) in &g.members {
                        self.attach(id, who[*i].ident, lookup.clone());
                        out.found[*i] = Found::Created(id);
                    }
                }
            }
            out.created += returned.len() as u64;
            let existing: Vec<String> = groups
                .keys()
                .filter(|c| !by_code.contains_key(*c))
                .cloned()
                .collect();
            if !existing.is_empty() {
                let cols = [
                    t.column("id").expect("subject.id"),
                    t.column("code").expect("subject.code"),
                    t.column("code_digest").expect("subject.code_digest"),
                ];
                let found = store.select_by_keys(t, &cols, "code", &existing)?;
                let mut subject_ids = Vec::with_capacity(found.len());
                let mut digests: HashMap<i64, Option<Vec<u8>>> = HashMap::new();
                for r in &found {
                    let id = r.int(0)?;
                    by_code.insert(r.text(1)?.to_string(), id);
                    digests.insert(id, r.opt_bytes(2)?.map(<[u8]>::to_vec));
                    subject_ids.push(id);
                }
                let held = linkage::identities_of_subjects(&mut self.linkage, &subject_ids)?;
                for code in &existing {
                    let g = &groups[code];
                    let id = *by_code.get(code).ok_or_else(|| missing_row("subject"))?;
                    for (lookup, i) in &g.members {
                        let w = &who[*i];
                        let ty = self.type_of(w.ident);
                        // A code read verbatim is the subject's code by the
                        // rule's own word (§7.3): the digest kept on the row
                        // is the identifier's the code was derived from,
                        // which is not this code's, and that is no collision.
                        // The check is for two identifiers truncating to one
                        // display code, which a verbatim read cannot be.
                        let verbatim = self.verbatim && !w.ident.fell_back;
                        let other_digest = !verbatim
                            && digests
                                .get(&id)
                                .and_then(|d| d.as_ref())
                                .is_some_and(|d| *d != g.digest);
                        let other_identity = held
                            .iter()
                            .any(|h| h.subject_id == id && h.id_type_id == ty.id);
                        if other_digest || other_identity {
                            return Err(ResolveError::Collision(Collision {
                                code: code.clone(),
                                subject_id: Some(id),
                                id_type: ty.name.clone(),
                                reason: if other_digest {
                                    "display-code"
                                } else {
                                    "identity"
                                },
                            }));
                        }
                        self.attach(id, w.ident, lookup.clone());
                        out.found[*i] = Found::Known(id);
                        out.attached += 1;
                    }
                }
            }
        }
        for (i, lookup) in lookups.iter().enumerate() {
            if out.found[i] == Found::Unknown
                && let Some(&id) = self.identities.get(lookup)
            {
                out.found[i] = Found::Known(id);
            }
        }
        if out.found.iter().any(|f| *f == Found::Unknown) {
            return Err(missing_row("subject").into());
        }
        Ok(out)
    }

    /// An identity row for the subject, filed after the commit (§9.3), and
    /// the lookup cached.
    fn attach(&mut self, subject_id: i64, ident: &Ident, lookup: Vec<u8>) {
        let ty = self.type_of(ident);
        self.pending.push(NewIdentity {
            subject_id,
            id_type_id: ty.id,
            lookup: lookup.clone(),
            ciphertext: self.keys.seal(&ident.value),
            source: "dicom",
            first_batch_id: Some(self.batch_id),
        });
        self.identities.put(lookup, subject_id);
    }

    /// The identity rows of the batch just committed (§9.3).
    pub fn file_identities(&mut self) -> Result<(), HomeError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let rows = std::mem::take(&mut self.pending);
        self.linkage.begin()?;
        match linkage::insert_identities(&mut self.linkage, &rows) {
            Ok(_) => {
                self.linkage.commit()?;
                Ok(())
            }
            Err(e) => {
                let _ = self.linkage.rollback();
                Err(e.into())
            }
        }
    }

    /// The batch rolled back: its identity rows are not filed, and the
    /// lookups it cached for subjects that were never made are forgotten.
    pub fn abandon(&mut self) {
        for row in self.pending.drain(..) {
            self.identities.pop(&row.lookup);
        }
    }
}

/// The id type of a name, which the linkage store must hold.
fn id_type_of(linkage: &mut Store, name: &str) -> Result<IdType, HomeError> {
    match linkage::id_type_id(linkage, name)? {
        Some(id) => Ok(IdType {
            name: name.to_string(),
            id,
        }),
        None => Err(HomeError::Message(format!(
            "no id type named {name}; nils linkage id-type list shows them, id-type add creates one"
        ))),
    }
}

pub(crate) fn missing_row(what: &str) -> HomeError {
    HomeError::Message(format!(
        "a {what} row was neither inserted nor found; the store changed under the writer"
    ))
}

/// The error a collision ends the job with: the code, the type and the item,
/// never an identifier.
pub fn collision_message(c: &Collision, item: i64) -> String {
    let what = match c.reason {
        "batch" => format!(
            "two identifiers of this batch derive the one code {}",
            c.code
        ),
        "display-code" => format!(
            "code {} is another identifier's (its subject holds a different digest)",
            c.code
        ),
        _ => format!(
            "the subject with code {} already holds another identifier of type {}",
            c.code, c.id_type
        ),
    };
    format!(
        "identity collision under {}: {what}; review item {item} is open. A blake2b-32 registry takes a longer display length (re-create it with --display-length); a blake2b-8 one has two identifiers on one code, which the review decides",
        c.id_type
    )
}
