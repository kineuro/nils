// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a, the dataset viewer (2026-10-09): a dataset, or a cohort over its
//! members, as subjects, their visits and one visit's scans, for the desk's
//! Grid. The subjects door lists the subjects as folders with what a person
//! filters them by (visits, scans, how many scans need a look, the body
//! regions, the makers, the main scans picked); the visits door lists one
//! subject's visits with their scan kinds; the scans door, narrowed to one
//! visit, gives the scans with their pictures.
//!
//! A scope is a dataset (its stacks are the ones its digests created first,
//! as the scans door and the sources door count them) or a cohort (every
//! current member, and every stack of theirs from any dataset). A stack of a
//! sample sealed now is in no scope for a caller who does not read sealed
//! stacks (record 48): never listed and never counted.
//!
//! A scan needs a look while a question the sort itself asks about it waits
//! for a person, the one definition the Data card's certainty, the sources
//! door and the scans doors count by ([`crate::certainty::Asks::sort`]), so
//! every number agrees.
//!
//! A visit is a session the cache holds (under the window it was built
//! with) with a scan in the scope; a study the cache holds under no session
//! yet is a visit of its own day with the other such studies of that day,
//! and a study with no day at all is the one visit with no day. Visits are
//! numbered over the subject's whole timeline, every study of every dataset,
//! so a visit keeps its number whichever dataset shows it.
//!
//! Record 55 K7: below detail quasi the subject's code, every date and what
//! is worked out from dates (the days from the first visit, a label made of
//! a date or of days, a span) come back as their shapes; ids, counts, visit
//! numbers, roles, regions, makers and kinds never do. The order is the same
//! at every detail. An identifier of a type the person chooses to show
//! subjects by is opened only for the page's subjects, each read audited;
//! it is shown as it is to a caller who may read identifiers (data:work at
//! detail sensitive, record 26) and as its shape to any other, but for the
//! alias a merge files (`subject-code`), which is a code and is shown as
//! codes are.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nils_registry::Registry;
use nils_registry::cohort::Cohort;
use nils_registry::day::Day;
use nils_registry::linkage::{self, Subkeys};
use nils_registry::place::Place;
use nils_registry::schema::SUBJECT_CODE_TYPE;
use nils_registry::session::Scheme;
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::grants::{Access, Detail};
use crate::scans::shape;
use crate::serve::{Caller, Reply};

/// A page of subjects when none is asked for.
pub(crate) const SUBJECTS_PAGE: usize = 60;
/// The most a page of subjects holds.
pub(crate) const SUBJECTS_MOST: usize = 200;
/// The most studies one visit's scans are asked for by.
pub(crate) const VISIT_STUDIES_MOST: usize = 100;

/// Why a read of an identifier happened, in the audit.
const WHY: &str = "the dataset viewer";

/// How many ids one `IN` list holds.
const IN_CHUNKS: usize = 500;

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the viewer shows: a dataset, or a cohort over its members.
pub(crate) enum Scope {
    /// An active source place and its `source` rows; a dataset nothing has
    /// read has none and holds no scan.
    Dataset {
        place: Box<Place>,
        sources: Vec<i64>,
    },
    Cohort(Cohort),
}

impl Scope {
    /// The dataset a door names, by its name or its id.
    pub(crate) fn dataset(registry: &mut Registry, name: &str) -> Result<Scope, Reply> {
        let place = crate::scans::dataset_named(registry, name)?;
        let sources = crate::sources::source_ids(registry.store(), &place).map_err(failed)?;
        Ok(Scope::Dataset {
            place: Box::new(place),
            sources,
        })
    }

    /// The cohort a door names, retired or not.
    pub(crate) fn cohort(registry: &mut Registry, name: &str) -> Result<Scope, Reply> {
        match nils_registry::cohort::by_name(registry.store(), name).map_err(failed)? {
            Some(c) => Ok(Scope::Cohort(c)),
            None => Err(Reply::error(404, format!("no cohort named {name}"))),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Scope::Dataset { .. } => "dataset",
            Scope::Cohort(_) => "cohort",
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Scope::Dataset { place, .. } => &place.name,
            Scope::Cohort(c) => &c.name,
        }
    }

    pub(crate) fn id(&self) -> i64 {
        match self {
            Scope::Dataset { place, .. } => place.id,
            Scope::Cohort(c) => c.id,
        }
    }

    pub(crate) fn as_json(&self) -> Value {
        json!({"kind": self.kind(), "name": self.name(), "id": self.id()})
    }

    /// The condition that keeps the scope's stacks, over a stack aliased
    /// `st` and its series `se`, with the sealed rule for this caller; none
    /// for a dataset nothing has read. A dataset's are the stacks its tree
    /// holds a file of (record 55, 2026-10-10).
    pub(crate) fn holds(
        &self,
        store: &Store,
        access: &Access,
        st: &str,
        se: &str,
    ) -> Option<String> {
        let mut out = match self {
            // record 55 (2026-10-10): every scan the dataset's tree has a
            // file of, whoever read it first
            Scope::Dataset { sources, .. } => {
                if sources.is_empty() {
                    return None;
                }
                crate::operations::held_by(store, st, &list(sources))
            }
            Scope::Cohort(c) => format!(
                "{se}.subject_id IN (SELECT cm.subject_id FROM {} cm \
                 WHERE cm.cohort_id = {} AND cm.left_at IS NULL)",
                store.qualified("cohort_member"),
                c.id
            ),
        };
        if !crate::sealed::reads(access) {
            out.push_str(&format!(
                " AND NOT EXISTS (SELECT 1 FROM {} sst WHERE sst.stack_id = {st}.id \
                 AND sst.unsealed_at IS NULL)",
                store.qualified("sealed_stack")
            ));
        }
        Some(out)
    }

    /// The current members of a cohort, every one of them whether or not a
    /// scan of theirs is in the scope; none for a dataset.
    fn members(&self, store: &mut Store) -> Result<Option<Vec<i64>>, StoreError> {
        match self {
            Scope::Dataset { .. } => Ok(None),
            Scope::Cohort(c) => Ok(Some(
                nils_registry::cohort::open_members(store, c.id)?
                    .into_iter()
                    .collect(),
            )),
        }
    }
}

/// The scope's stacks as a join: a stack `st`, its first batch `b` and its
/// series `se`.
fn stacks_from(store: &Store) -> String {
    format!(
        "{} st JOIN {} b ON b.id = st.first_batch_id JOIN {} se ON se.id = st.series_id",
        store.qualified("stack"),
        store.qualified("ingest_batch"),
        store.qualified("series"),
    )
}

/// A study's day as text on either backend: the measured date, else the
/// one the date vote filled in.
fn study_day(store: &Store, alias: &str) -> String {
    let d = store.dialect();
    let t = nils_registry::schema::table("study");
    let column = |c: &str| d.text_of_qualified(Some(alias), t.column(c).expect("study column"));
    format!(
        "COALESCE({}, {})",
        column("date_filled"),
        column("study_date")
    )
}

/// The window every visit is of: the one the session cache was built
/// under, the default scheme's where it holds nothing.
fn window(store: &mut Store) -> Result<i64, StoreError> {
    Ok(nils_registry::cohort::built_window(store)?.unwrap_or(Scheme::default().window_days))
}

/// The stacks of the scope that need a look, each with its subject and
/// study: the stacks a question the sort asks waits on a person for, as
/// every look is counted ([`crate::certainty::Asks::sort`]). `only` keeps
/// one subject's.
fn looked(
    store: &mut Store,
    holds: &str,
    only: Option<i64>,
) -> Result<HashMap<i64, (i64, i64)>, StoreError> {
    let from = stacks_from(store);
    let subject = only
        .map(|s| format!(" AND se.subject_id = {s}"))
        .unwrap_or_default();
    let asks = crate::certainty::Asks::sort(store)?;
    let scope = format!("x.id IN (SELECT st.id FROM {from} WHERE {holds}{subject})");
    let stacks: Vec<i64> = asks.stacks_in(store, &scope)?.into_iter().collect();
    let mut out = HashMap::new();
    for chunk in stacks.chunks(IN_CHUNKS) {
        for r in store.query(
            &format!(
                "SELECT st.id, se.subject_id, se.study_id FROM {from} WHERE st.id IN ({})",
                list(chunk)
            ),
            &[],
        )? {
            out.insert(r.int(0)?, (r.int(1)?, r.int(2)?));
        }
    }
    Ok(out)
}

/// What makes a visit: a session the cache holds, a day it holds no
/// session for, or no day at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum VisitKey {
    Session(i64),
    Day(String),
    Undated,
}

fn visit_key(session: Option<i64>, day: Option<&str>) -> VisitKey {
    match (session, day.and_then(Day::parse)) {
        (Some(s), _) => VisitKey::Session(s),
        (None, Some(d)) => VisitKey::Day(d.to_string()),
        (None, None) => VisitKey::Undated,
    }
}

/// The body regions a body part names: `brain-neck` is the brain and the
/// neck.
pub(crate) fn regions_of(body_part: &str) -> Vec<String> {
    match body_part.trim() {
        "" => Vec::new(),
        "brain-neck" => vec!["brain".to_string(), "neck".to_string()],
        other => vec![other.to_string()],
    }
}

/// The order regions are listed in: brain, neck, spine, chest, other, then
/// any other the pack names, by name.
fn region_rank(region: &str) -> (usize, String) {
    let known = ["brain", "neck", "spine", "chest", "other"];
    (
        known
            .iter()
            .position(|k| *k == region)
            .unwrap_or(known.len()),
        region.to_string(),
    )
}

fn sorted_regions(regions: &BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = regions.iter().cloned().collect();
    out.sort_by_key(|r| region_rank(r));
    out
}

/// A maker as a person names it: the scanner's own spelling of the big
/// makers brought to one word, any other as written.
pub(crate) fn maker_of(raw: &str) -> Option<String> {
    let written = raw.trim();
    if written.is_empty() {
        return None;
    }
    let l = written.to_lowercase();
    let named = if l.contains("siemens") {
        "Siemens"
    } else if l.contains("philips") {
        "Philips"
    } else if l == "ge" || l.starts_with("ge ") || l.contains("general electric") {
        "GE"
    } else if l.contains("toshiba") || l.contains("canon") {
        "Canon"
    } else if l.contains("hitachi") || l.contains("fujifilm") {
        "Hitachi"
    } else {
        return Some(written.to_string());
    };
    Some(named.to_string())
}

/// A stack's decided axes, every value of each.
pub(crate) type Axes = BTreeMap<String, Vec<String>>;

fn has(a: &Axes, axis: &str, value: &str) -> bool {
    a.get(axis).is_some_and(|v| v.iter().any(|x| x == value))
}

/// The kinds of scan a visit is told by, in the order they are listed.
pub(crate) const KINDS: [&str; 13] = [
    "SyMRI",
    "DWI",
    "Perfusion",
    "fMRI",
    "Field map",
    "Scout",
    "FLAIR",
    "SWI",
    "T1w",
    "T2w",
    "PDw",
    "T2*w",
    "Other",
];

/// What kind of scan a stack is, in a person's words, each scan once: a
/// scout first (whatever else it is), then what made it (SyMRI), then its
/// datatype (diffusion, perfusion, functional, a field map), then FLAIR and
/// SWI, then its weighting.
pub(crate) fn kind_of(a: &Axes) -> &'static str {
    if has(a, "disposition", "scout")
        || has(a, "directory_type", "localizer")
        || has(a, "provenance", "Localizer")
    {
        "Scout"
    } else if has(a, "provenance", "SyMRI") {
        "SyMRI"
    } else if has(a, "directory_type", "dwi") || has(a, "base", "DWI") {
        "DWI"
    } else if has(a, "directory_type", "perf") || has(a, "base", "PWI") {
        "Perfusion"
    } else if has(a, "directory_type", "func") {
        "fMRI"
    } else if has(a, "directory_type", "fmap") {
        "Field map"
    } else if has(a, "modifier", "FLAIR") {
        "FLAIR"
    } else if has(a, "base", "SWI") || has(a, "provenance", "SWIRecon") {
        "SWI"
    } else if has(a, "base", "T1w") {
        "T1w"
    } else if has(a, "base", "T2w") {
        "T2w"
    } else if has(a, "base", "PDw") {
        "PDw"
    } else if has(a, "base", "T2*w") || has(a, "base", "T2starw") {
        "T2*w"
    } else {
        "Other"
    }
}

/// The family a scan is grouped in inside its datatype's folder, as the
/// desk's tree groups it: what made it (SyMRI, a mix sequence, STAGE, a
/// scanner's SWI), a derived image, a scan of the spine or the neck, or
/// none of these (`plain`).
pub(crate) fn family_of(a: &Axes) -> &'static str {
    let made = |p: &str| has(a, "provenance", p);
    if made("SyMRI") {
        "symri"
    } else if made("EPIMix") || made("NeuroMix") {
        "mix"
    } else if made("STAGE") {
        "stage"
    } else if made("SWIRecon") {
        "swi"
    } else if made("ProjectionDerived")
        || made("SubtractionDerived")
        || has(a, "disposition", "reformat")
    {
        "derived"
    } else if has(a, "body_part", "spine") || has(a, "body_part", "neck") {
        "body"
    } else {
        "plain"
    }
}

/// Whether a contrast agent was given: the label the pack stores for
/// `given`, or the id itself.
pub(crate) fn contrast_of(a: &Axes) -> bool {
    has(a, "post_contrast", "1") || has(a, "post_contrast", "given")
}

/// The axes of these stacks the viewer reads, by stack.
fn axes_of(store: &mut Store, stacks: &[i64]) -> Result<HashMap<i64, Axes>, StoreError> {
    let mut out: HashMap<i64, Axes> = HashMap::new();
    for chunk in stacks.chunks(IN_CHUNKS) {
        for r in store.query(
            &format!(
                "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({}) \
                 AND value IS NOT NULL AND axis IN ('body_part', 'provenance', 'post_contrast', \
                 'base', 'modifier', 'directory_type', 'disposition') ORDER BY id",
                store.qualified("classification_axis"),
                list(chunk)
            ),
            &[],
        )? {
            out.entry(r.int(0)?)
                .or_default()
                .entry(r.text(1)?.to_string())
                .or_default()
                .push(r.text(2)?.to_string());
        }
    }
    Ok(out)
}

/// The roles of the live picks naming each of these stacks: a pick no
/// person withdrew or overruled.
fn picked(store: &mut Store, stacks: &[i64]) -> Result<HashMap<i64, BTreeSet<String>>, StoreError> {
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    for chunk in stacks.chunks(IN_CHUNKS) {
        for r in store.query(
            &format!(
                "SELECT ps.stack_id, p.role FROM {} ps JOIN {} p ON p.id = ps.pick_id \
                 WHERE p.withdrawn_at IS NULL AND ps.stack_id IN ({})",
                store.qualified("pick_stack"),
                store.qualified("pick"),
                list(chunk)
            ),
            &[],
        )? {
            out.entry(r.int(0)?)
                .or_default()
                .insert(r.text(1)?.to_string());
        }
    }
    Ok(out)
}

/// A door's `filter`: the kinds asked for, every one of which must hold,
/// each with its values, one of which must.
#[derive(Debug, Default)]
struct Filters(BTreeMap<String, BTreeSet<String>>);

impl Filters {
    /// A comma list of words (`look`) and kinds with a value
    /// (`region:brain`); a word or kind the door does not know is refused.
    fn parse(text: Option<&String>, words: &[&str], valued: &[&str]) -> Result<Filters, Reply> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let knows = || {
            words
                .iter()
                .map(|w| w.to_string())
                .chain(valued.iter().map(|v| format!("{v}:<value>")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        for item in text
            .map(String::as_str)
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|i| !i.is_empty())
        {
            match item.split_once(':') {
                Some((kind, value)) if valued.contains(&kind) && !value.trim().is_empty() => {
                    out.entry(kind.to_string())
                        .or_default()
                        .insert(value.trim().to_lowercase());
                }
                None if words.contains(&item) => {
                    out.entry(item.to_string()).or_default();
                }
                _ => {
                    return Err(Reply::error(
                        400,
                        format!("filter: {item} is not one of {}", knows()),
                    ));
                }
            }
        }
        Ok(Filters(out))
    }

    fn wants(&self, kind: &str) -> bool {
        self.0.contains_key(kind)
    }

    /// Whether one of the asked values of `kind` is among `held`
    /// (lower-cased), or the kind is not asked for.
    fn any_of<'a>(&self, kind: &str, held: impl IntoIterator<Item = &'a String>) -> bool {
        match self.0.get(kind) {
            None => true,
            Some(asked) => held.into_iter().any(|h| asked.contains(&h.to_lowercase())),
        }
    }
}

/// The id type subjects are shown by, when it is not their code.
struct Shown {
    id: i64,
    name: String,
}

/// The `show` of a door: the code, or an id type the linkage store knows.
fn shown_by(registry: &mut Registry, show: Option<&String>) -> Result<Option<Shown>, Reply> {
    let show = show.map(|s| s.trim()).filter(|s| !s.is_empty());
    let Some(name) = show.filter(|s| *s != "code") else {
        return Ok(None);
    };
    let mut store = registry.open_linkage().map_err(failed)?;
    let types = linkage::id_types(&mut store).map_err(failed)?;
    match types.iter().find(|t| t.name == name) {
        Some(t) => Ok(Some(Shown {
            id: t.id,
            name: t.name.clone(),
        })),
        None => Err(Reply::error(
            400,
            format!(
                "show: {name} is not code or an id type ({})",
                types
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// Whether a caller may read identifiers: data:work at detail sensitive,
/// as every door that reads one asks (record 26).
fn reads_identifiers(access: &Access) -> bool {
    access.holds("data:work") && access.detail >= Detail::Sensitive
}

/// The labels of these subjects by an id type: each one's value of it,
/// opened and audited, as it is where this caller may see it and as its
/// shape otherwise; a subject that holds none, or one whose value the store
/// keeps nothing of (a personnummer), is not in the answer.
fn labels_by_type(
    registry: &mut Registry,
    caller: &Caller,
    shown: &Shown,
    subjects: &[i64],
) -> Result<HashMap<i64, String>, Reply> {
    if subjects.is_empty() || nils_registry::personnummer::is_type(&shown.name) {
        return Ok(HashMap::new());
    }
    let key = registry.pseudonym_key().map_err(failed)?;
    let keys = Subkeys::derive(&key);
    let mut store = registry.open_linkage().map_err(failed)?;
    let values = linkage::values_of_type(
        &mut store,
        &keys,
        subjects,
        shown.id,
        &caller.principal,
        WHY,
    )
    .map_err(failed)?;
    let clear = reads_identifiers(&caller.access)
        || (shown.name == SUBJECT_CODE_TYPE && caller.access.detail >= Detail::Quasi);
    Ok(values
        .into_iter()
        .map(|(s, v)| (s, if clear { v } else { shape(&v) }))
        .collect())
}

/// The subjects holding an identifier, of any type, equal to `text`: found
/// by its keyed lookup, nothing opened.
fn found_by_identifier(registry: &mut Registry, text: &str) -> Result<HashSet<i64>, Reply> {
    let key = registry.pseudonym_key().map_err(failed)?;
    let keys = Subkeys::derive(&key);
    let mut store = registry.open_linkage().map_err(failed)?;
    let types = linkage::id_types(&mut store).map_err(failed)?;
    let wanted: Vec<(i64, Vec<u8>)> = types
        .iter()
        .map(|t| (t.id, keys.lookup(&t.name, text)))
        .collect();
    let lookups: Vec<Vec<u8>> = wanted.iter().map(|(_, l)| l.clone()).collect();
    let found = linkage::identities_by_lookup(&mut store, &lookups).map_err(failed)?;
    Ok(found
        .into_iter()
        .filter(|i| {
            wanted
                .iter()
                .any(|(t, l)| *t == i.id_type_id && *l == i.lookup)
        })
        .map(|i| i.subject_id)
        .collect())
}

/// The id types the scope's subjects hold that keep a value, with how many
/// subjects hold each, most first; nothing is opened. Empty where the
/// linkage store cannot be read: a facet is no reason to fail the door.
fn id_type_facet(registry: &mut Registry, subjects: &[i64]) -> Vec<Value> {
    let Ok(mut store) = registry.open_linkage() else {
        return Vec::new();
    };
    let Ok(types) = linkage::id_types(&mut store) else {
        return Vec::new();
    };
    let mut held: HashMap<i64, BTreeSet<i64>> = HashMap::new();
    let identity = store.qualified("identity");
    for chunk in subjects.chunks(IN_CHUNKS) {
        let Ok(rows) = store.query(
            &format!(
                "SELECT DISTINCT id_type_id, subject_id FROM {identity} WHERE subject_id IN ({})",
                list(chunk)
            ),
            &[],
        ) else {
            return Vec::new();
        };
        for r in rows {
            if let (Ok(t), Ok(s)) = (r.int(0), r.int(1)) {
                held.entry(t).or_default().insert(s);
            }
        }
    }
    let mut out: Vec<(String, usize)> = types
        .iter()
        .filter(|t| !nils_registry::personnummer::is_type(&t.name))
        .filter_map(|t| held.get(&t.id).map(|s| (t.name.clone(), s.len())))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.into_iter()
        .map(|(name, n)| json!({"name": name, "subjects": n}))
        .collect()
}

/// A facet's counts as a list, most subjects first, then by `rank`.
fn facet<K: Ord>(counts: &BTreeMap<String, usize>, rank: impl Fn(&str) -> K) -> Vec<Value> {
    let mut out: Vec<(&String, &usize)> = counts.iter().collect();
    out.sort_by(|a, b| b.1.cmp(a.1).then_with(|| rank(a.0).cmp(&rank(b.0))));
    out.into_iter()
        .map(|(name, n)| json!({"name": name, "subjects": n}))
        .collect()
}

/// What the subjects door knows of one subject.
#[derive(Debug, Default)]
struct Facts {
    code: String,
    scans: i64,
    look: i64,
    visits: BTreeSet<VisitKey>,
    regions: BTreeSet<String>,
    makers: BTreeSet<String>,
    main: BTreeSet<String>,
}

/// `GET /api/{datasets|cohorts}/{name}/subjects`: the scope's subjects, a
/// page at a time, with what the Grid filters them by.
pub(crate) fn subjects(
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    query: &HashMap<String, String>,
) -> Result<Value, Reply> {
    let access = &caller.access;
    let quasi = access.detail >= Detail::Quasi;
    let text = query
        .get("q")
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty());
    let order = match query
        .get("order")
        .map(|o| o.trim())
        .filter(|o| !o.is_empty())
    {
        None => "look",
        Some(o @ ("look" | "code" | "visits" | "scans")) => o,
        Some(o) => {
            return Err(Reply::error(
                400,
                format!("order: {o} is not look, code, visits or scans"),
            ));
        }
    };
    let filters = Filters::parse(
        query.get("filter"),
        &["look", "visits2"],
        &["main", "region", "maker"],
    )?;
    let limit = match query.get("limit").filter(|l| !l.is_empty()) {
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=SUBJECTS_MOST).contains(n))
            .ok_or_else(|| Reply::error(400, format!("limit is 1 to {SUBJECTS_MOST}")))?,
        None => SUBJECTS_PAGE,
    };
    let after = match query.get("after").filter(|a| !a.is_empty()) {
        Some(a) => Some(
            a.parse::<i64>()
                .map_err(|_| Reply::error(400, "after is a subject's id, as `next` gave it"))?,
        ),
        None => None,
    };
    let shown = shown_by(registry, query.get("show"))?;

    let store = registry.store();
    let mut facts: BTreeMap<i64, Facts> = BTreeMap::new();
    // a cohort lists every current member, with nothing counted where no
    // scan of theirs is in the scope
    if let Some(members) = scope.members(store).map_err(failed)? {
        for chunk in members.chunks(IN_CHUNKS) {
            for r in store
                .query(
                    &format!(
                        "SELECT id, code FROM {} WHERE id IN ({})",
                        store.qualified("subject"),
                        list(chunk)
                    ),
                    &[],
                )
                .map_err(failed)?
            {
                facts.insert(
                    r.int(0).map_err(failed)?,
                    Facts {
                        code: r.text(1).map_err(failed)?.to_string(),
                        ..Facts::default()
                    },
                );
            }
        }
    }
    let holds = scope.holds(store, access, "st", "se");
    if let Some(holds) = &holds {
        let from = stacks_from(store);
        let q = |t: &str| store.qualified(t);
        let (subject, study, cached, axis, fp, pick, pick_stack) = (
            q("subject"),
            q("study"),
            q("session_cache_study"),
            q("classification_axis"),
            q("stack_fingerprint"),
            q("pick"),
            q("pick_stack"),
        );
        // the scans, by subject
        for r in store
            .query(
                &format!(
                    "SELECT se.subject_id, su.code, COUNT(*) FROM {from} \
                     JOIN {subject} su ON su.id = se.subject_id \
                     WHERE {holds} GROUP BY se.subject_id, su.code"
                ),
                &[],
            )
            .map_err(failed)?
        {
            let f = facts.entry(r.int(0).map_err(failed)?).or_default();
            f.code = r.text(1).map_err(failed)?.to_string();
            f.scans = r.int(2).map_err(failed)?;
        }
        // the visits: each study with a scan in the scope, by its session
        // or its day
        let w = window(store).map_err(failed)?;
        let day = study_day(store, "sy");
        for r in store
            .query(
                &format!(
                    "SELECT sy.subject_id, {day}, scs.session_id FROM {study} sy \
                     LEFT JOIN {cached} scs ON scs.study_id = sy.id AND scs.window_days = {w} \
                     WHERE sy.id IN (SELECT se.study_id FROM {from} WHERE {holds})"
                ),
                &[],
            )
            .map_err(failed)?
        {
            if let Some(f) = facts.get_mut(&r.int(0).map_err(failed)?) {
                f.visits.insert(visit_key(
                    r.opt_int(2).map_err(failed)?,
                    r.opt_text(1).map_err(failed)?,
                ));
            }
        }
        // the scans that need a look
        for (subject, _) in looked(store, holds, None).map_err(failed)?.values() {
            if let Some(f) = facts.get_mut(subject) {
                f.look += 1;
            }
        }
        // the body regions
        for r in store
            .query(
                &format!(
                    "SELECT DISTINCT se.subject_id, a.value FROM {from} \
                     JOIN {axis} a ON a.stack_id = st.id \
                     WHERE {holds} AND a.axis = 'body_part' AND a.value IS NOT NULL"
                ),
                &[],
            )
            .map_err(failed)?
        {
            if let Some(f) = facts.get_mut(&r.int(0).map_err(failed)?) {
                f.regions.extend(regions_of(r.text(1).map_err(failed)?));
            }
        }
        // the makers: the stack's own, else its study's
        for r in store
            .query(
                &format!(
                    "SELECT DISTINCT se.subject_id, COALESCE(f.manufacturer, sy.manufacturer) \
                     FROM {from} JOIN {study} sy ON sy.id = se.study_id \
                     LEFT JOIN {fp} f ON f.stack_id = st.id WHERE {holds}"
                ),
                &[],
            )
            .map_err(failed)?
        {
            if let (Some(f), Some(m)) = (
                facts.get_mut(&r.int(0).map_err(failed)?),
                r.opt_text(1).map_err(failed)?.and_then(maker_of),
            ) {
                f.makers.insert(m);
            }
        }
        // the roles of the live picks among the scope's scans
        for r in store
            .query(
                &format!(
                    "SELECT DISTINCT se.subject_id, p.role FROM {from} \
                     JOIN {pick_stack} ps ON ps.stack_id = st.id \
                     JOIN {pick} p ON p.id = ps.pick_id \
                     WHERE {holds} AND p.withdrawn_at IS NULL"
                ),
                &[],
            )
            .map_err(failed)?
        {
            if let Some(f) = facts.get_mut(&r.int(0).map_err(failed)?) {
                f.main.insert(r.text(1).map_err(failed)?.to_string());
            }
        }
    }

    // the whole scope's numbers and facets, before anything narrows it
    let mut makers: BTreeMap<String, usize> = BTreeMap::new();
    let mut regions: BTreeMap<String, usize> = BTreeMap::new();
    let mut roles: BTreeMap<String, usize> = BTreeMap::new();
    let (mut visits, mut scans, mut look) = (0usize, 0i64, 0i64);
    for f in facts.values() {
        visits += f.visits.len();
        scans += f.scans;
        look += f.look;
        for m in &f.makers {
            *makers.entry(m.clone()).or_default() += 1;
        }
        for r in &f.regions {
            *regions.entry(r.clone()).or_default() += 1;
        }
        for r in &f.main {
            *roles.entry(r.clone()).or_default() += 1;
        }
    }
    let everyone: Vec<i64> = facts.keys().copied().collect();
    let id_types = id_type_facet(registry, &everyone);

    // what `q` finds: the code as this caller is shown it, and an
    // identifier given whole to a caller who may read one
    let shown_code = |code: &str| {
        if quasi { code.to_string() } else { shape(code) }
    };
    let by_identifier = match &text {
        Some(t) if reads_identifiers(access) => found_by_identifier(registry, t)?,
        _ => HashSet::new(),
    };
    let needle = text.as_ref().map(|t| t.to_lowercase());
    let mut kept: Vec<(&i64, &Facts)> = facts
        .iter()
        .filter(|(id, f)| match &needle {
            None => true,
            Some(n) => shown_code(&f.code).to_lowercase().contains(n) || by_identifier.contains(id),
        })
        .filter(|(_, f)| !filters.wants("look") || f.look > 0)
        .filter(|(_, f)| !filters.wants("visits2") || f.visits.len() > 1)
        .filter(|(_, f)| filters.any_of("main", &f.main))
        .filter(|(_, f)| filters.any_of("region", &f.regions))
        .filter(|(_, f)| filters.any_of("maker", &f.makers))
        .collect();
    kept.sort_by(|(ia, a), (ib, b)| {
        let by = match order {
            "look" => b.look.cmp(&a.look),
            "visits" => b.visits.len().cmp(&a.visits.len()),
            "scans" => b.scans.cmp(&a.scans),
            _ => std::cmp::Ordering::Equal,
        };
        by.then_with(|| a.code.cmp(&b.code))
            .then_with(|| ia.cmp(ib))
    });
    let matched = kept.len();
    let start = match after {
        None => 0,
        Some(a) => match kept.iter().position(|(id, _)| **id == a) {
            Some(i) => i + 1,
            None => {
                return Err(Reply::error(
                    400,
                    format!("after names no subject of {} this list holds", scope.name()),
                ));
            }
        },
    };
    let page: Vec<(&i64, &Facts)> = kept.iter().skip(start).take(limit).copied().collect();
    let next = if start + page.len() < matched {
        page.last().map(|(id, _)| **id)
    } else {
        None
    };
    let ids: Vec<i64> = page.iter().map(|(id, _)| **id).collect();
    let labels = match &shown {
        Some(s) => Some(labels_by_type(registry, caller, s, &ids)?),
        None => None,
    };
    let subjects: Vec<Value> = page
        .iter()
        .map(|(id, f)| {
            let code = shown_code(&f.code);
            let label = match &labels {
                None => Value::from(code.clone()),
                Some(l) => l.get(id).map_or(Value::Null, |v| Value::from(v.clone())),
            };
            json!({
                "id": id,
                "code": code,
                "label": label,
                "visits": f.visits.len(),
                "scans": f.scans,
                "look": f.look,
                "regions": sorted_regions(&f.regions),
                "makers": f.makers.iter().collect::<Vec<_>>(),
                "main": f.main.iter().collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "scope": scope.as_json(),
        "detail": access.detail.name(),
        "show": shown.as_ref().map_or("code", |s| s.name.as_str()),
        "order": order,
        "totals": {"subjects": facts.len(), "visits": visits, "scans": scans, "look": look},
        "matched": matched,
        "count": subjects.len(),
        "subjects": subjects,
        "next": next,
        "facets": {
            "makers": facet(&makers, |m| m.to_string()),
            "regions": facet(&regions, region_rank),
            "roles": facet(&roles, |r| r.to_string()),
            "id_types": id_types,
        },
    }))
}

/// One visit of a subject's timeline as the visits door builds it.
#[derive(Debug, Default)]
struct VisitOf {
    first: Option<Day>,
    /// The studies of the visit, every dataset's.
    studies: BTreeSet<i64>,
    session: Option<i64>,
    /// The scope's scans of the visit, by study.
    stacks: Vec<(i64, i64)>,
}

/// `GET /api/{datasets|cohorts}/{name}/subjects/{subject}/visits`: one
/// subject's visits in the scope, with their scan kinds and main scans.
pub(crate) fn visits(
    registry: &mut Registry,
    caller: &Caller,
    pack: Option<&nils_pack::Pack>,
    scope: &Scope,
    subject: i64,
    query: &HashMap<String, String>,
) -> Result<Value, Reply> {
    let access = &caller.access;
    let quasi = access.detail >= Detail::Quasi;
    let naming = match query
        .get("name")
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
    {
        None => "date",
        Some(n @ ("date" | "number" | "days")) => n,
        Some(n) => {
            return Err(Reply::error(
                400,
                format!("name: {n} is not date, number or days"),
            ));
        }
    };
    let filters = Filters::parse(
        query.get("filter"),
        &["look", "contrast", "symri"],
        &["region"],
    )?;
    let shown = shown_by(registry, query.get("show"))?;
    let missing = || {
        Reply::error(
            404,
            format!("no subject {subject} in {} {}", scope.kind(), scope.name()),
        )
    };

    let store = registry.store();
    let holds = scope.holds(store, access, "st", "se");
    let from = stacks_from(store);
    // the subject's scans in the scope, by study
    let stacks: Vec<(i64, i64)> = match &holds {
        None => Vec::new(),
        Some(holds) => store
            .query(
                &format!(
                    "SELECT st.id, se.study_id FROM {from} \
                     WHERE {holds} AND se.subject_id = {subject} ORDER BY st.id"
                ),
                &[],
            )
            .map_err(failed)?
            .iter()
            .map(|r| Ok((r.int(0)?, r.int(1)?)))
            .collect::<Result<_, StoreError>>()
            .map_err(failed)?,
    };
    // a dataset's subject is one with a scan in it; a cohort's, a current
    // member, whether or not a scan of theirs is to be seen
    let present = match scope.members(store).map_err(failed)? {
        Some(members) => members.contains(&subject),
        None => !stacks.is_empty(),
    };
    if !present {
        return Err(missing());
    }
    let code = store
        .query(
            &format!(
                "SELECT code FROM {} WHERE id = {subject}",
                store.qualified("subject")
            ),
            &[],
        )
        .map_err(failed)?
        .first()
        .map(|r| r.text(0).map(str::to_string))
        .transpose()
        .map_err(failed)?
        .ok_or_else(missing)?;

    // the subject's whole timeline: every study of theirs, by its session
    // or its day, in date order
    let w = window(store).map_err(failed)?;
    let day = study_day(store, "sy");
    let cache_t = nils_registry::schema::table("session_cache");
    let first = store
        .dialect()
        .text_of_qualified(Some("sc"), cache_t.column("first").expect("first"));
    let mut timeline: BTreeMap<VisitKey, VisitOf> = BTreeMap::new();
    for r in store
        .query(
            &format!(
                "SELECT sy.id, {day}, scs.session_id, {first} FROM {} sy \
                 LEFT JOIN {} scs ON scs.study_id = sy.id AND scs.window_days = {w} \
                 LEFT JOIN {} sc ON sc.id = scs.session_id \
                 WHERE sy.subject_id = {subject}",
                store.qualified("study"),
                store.qualified("session_cache_study"),
                store.qualified("session_cache"),
            ),
            &[],
        )
        .map_err(failed)?
    {
        let study = r.int(0).map_err(failed)?;
        let day = r.opt_text(1).map_err(failed)?;
        let session = r.opt_int(2).map_err(failed)?;
        let opened = r.opt_text(3).map_err(failed)?.and_then(Day::parse);
        let key = visit_key(session, day);
        let v = timeline.entry(key).or_default();
        v.session = session;
        v.studies.insert(study);
        // a session opens on its first day; a day of its own on itself
        let on = opened.or_else(|| day.and_then(Day::parse));
        v.first = match (v.first, on) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    for (stack, study) in &stacks {
        if let Some(v) = timeline.values_mut().find(|v| v.studies.contains(study)) {
            v.stacks.push((*stack, *study));
        }
    }
    let mut ordered: Vec<(VisitKey, VisitOf)> = timeline.into_iter().collect();
    ordered.sort_by(|(ka, a), (kb, b)| match (a.first, b.first) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| ka.cmp(kb)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => ka.cmp(kb),
    });
    let zero = ordered.iter().find_map(|(_, v)| v.first);

    // what each of the scope's scans is
    let ids: Vec<i64> = stacks.iter().map(|(s, _)| *s).collect();
    let axes = axes_of(store, &ids).map_err(failed)?;
    let looks: HashSet<i64> = match &holds {
        Some(holds) => looked(store, holds, Some(subject))
            .map_err(failed)?
            .into_keys()
            .collect(),
        None => HashSet::new(),
    };
    let picks = picked(store, &ids).map_err(failed)?;
    let mains: Vec<i64> = {
        let mut m: Vec<i64> = picks.keys().copied().collect();
        m.sort_unstable();
        m
    };
    let mut names = nils_release::run::scan_names(store, pack, &mains)
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let mut described: HashMap<i64, String> = HashMap::new();
    for chunk in mains.chunks(IN_CHUNKS) {
        for r in store
            .query(
                &format!(
                    "SELECT stack_id, text_series_description FROM {} WHERE stack_id IN ({})",
                    store.qualified("stack_fingerprint"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            if let Some(t) = r.opt_text(1).map_err(failed)? {
                described.insert(r.int(0).map_err(failed)?, t.to_string());
            }
        }
    }
    let empty = Axes::new();
    let shaped = |v: String| if quasi { v } else { shape(&v) };

    let mut built = Vec::new();
    let (mut in_scope, mut scans, mut look) = (0usize, 0usize, 0usize);
    let mut dated: Vec<Day> = Vec::new();
    for (number, (_, v)) in ordered.iter().enumerate() {
        if v.stacks.is_empty() {
            continue;
        }
        in_scope += 1;
        scans += v.stacks.len();
        let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut regions: BTreeSet<String> = BTreeSet::new();
        let (mut contrast, mut symri, mut looking) = (false, 0usize, 0usize);
        let mut main: Vec<(String, i64)> = Vec::new();
        for (stack, _) in &v.stacks {
            let a = axes.get(stack).unwrap_or(&empty);
            *kinds.entry(kind_of(a)).or_default() += 1;
            for part in a.get("body_part").into_iter().flatten() {
                regions.extend(regions_of(part));
            }
            contrast |= contrast_of(a);
            if has(a, "provenance", "SyMRI") {
                symri += 1;
            }
            if looks.contains(stack) {
                looking += 1;
            }
            for role in picks.get(stack).into_iter().flatten() {
                main.push((role.clone(), *stack));
            }
        }
        look += looking;
        main.sort();
        let studies: Vec<i64> = v
            .studies
            .iter()
            .copied()
            .filter(|s| v.stacks.iter().any(|(_, st)| st == s))
            .collect();
        let days = match (zero, v.first) {
            (Some(z), Some(f)) => Some(z.days_to(f)),
            _ => None,
        };
        if let Some(f) = v.first {
            dated.push(f);
        }
        let label = match naming {
            "number" => format!("ses-{:02}", number + 1),
            "days" => match days {
                Some(d) => format!("ses-d{}", shaped(d.to_string())),
                None => "ses-none".to_string(),
            },
            _ => match v.first {
                Some(f) => format!("ses-{}", shaped(f.compact())),
                None => "ses-none".to_string(),
            },
        };
        let kinds: Vec<Value> = KINDS
            .iter()
            .filter_map(|k| kinds.get(k).map(|n| json!({"kind": k, "scans": n})))
            .collect();
        let main: Vec<Value> = main
            .into_iter()
            .map(|(role, stack)| {
                let name = names
                    .remove(&stack)
                    .map(|n| n.name)
                    .filter(|n| !n.trim().is_empty() && n != "unknown")
                    .or_else(|| described.get(&stack).cloned());
                json!({"role": role, "stack": stack, "name": name})
            })
            .collect();
        let regions = sorted_regions(&regions);
        built.push((
            looking,
            contrast,
            symri,
            regions.clone(),
            json!({
                "session": v.session,
                "studies": studies,
                "label": label,
                "first": v.first.map(|f| shaped(f.to_string())),
                "day": days.map(|d| shaped(d.to_string())),
                "number": number + 1,
                "scans": v.stacks.len(),
                "look": looking,
                "regions": regions,
                "kinds": kinds,
                "contrast": contrast,
                "symri": symri,
                "main": main,
            }),
        ));
    }
    let span = match (dated.iter().min(), dated.iter().max()) {
        (Some(a), Some(b)) => Some(shaped(a.days_to(*b).to_string())),
        _ => None,
    };
    let kept: Vec<Value> = built
        .into_iter()
        .filter(|(looking, ..)| !filters.wants("look") || *looking > 0)
        .filter(|(_, contrast, ..)| !filters.wants("contrast") || *contrast)
        .filter(|(_, _, symri, ..)| !filters.wants("symri") || *symri > 0)
        .filter(|(_, _, _, regions, _)| filters.any_of("region", regions))
        .map(|(.., v)| v)
        .collect();
    let label = match &shown {
        None => Value::from(shaped(code.clone())),
        Some(s) => labels_by_type(registry, caller, s, &[subject])?
            .remove(&subject)
            .map_or(Value::Null, Value::from),
    };
    Ok(json!({
        "scope": scope.as_json(),
        "detail": access.detail.name(),
        "name": naming,
        "show": shown.as_ref().map_or("code", |s| s.name.as_str()),
        "subject": {"id": subject, "code": shaped(code), "label": label},
        "totals": {"visits": in_scope, "scans": scans, "look": look, "span": span},
        "matched": kept.len(),
        "visits": kept,
    }))
}

/// The dataset viewer's facts on each scan of a page: `family`, the group
/// it is shown in inside its datatype's folder; `te`, `tr`, `ti` and `fa`,
/// the echo, repetition and inversion times (milliseconds) and the flip
/// angle (degrees) from its fingerprint, or null; and `main`, the roles of
/// the live picks naming it. None of it is quasi-identifying.
pub(crate) fn with_facts(registry: &mut Registry, doc: &mut Value) -> Result<(), Reply> {
    let stacks: Vec<i64> = doc["scans"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["stack"].as_i64()).collect())
        .unwrap_or_default();
    let store = registry.store();
    let mut physics: HashMap<i64, [Option<f64>; 4]> = HashMap::new();
    for chunk in stacks.chunks(IN_CHUNKS) {
        for r in store
            .query(
                &format!(
                    "SELECT stack_id, echo_time, repetition_time, inversion_time, flip_angle \
                     FROM {} WHERE stack_id IN ({})",
                    store.qualified("stack_fingerprint"),
                    list(chunk)
                ),
                &[],
            )
            .map_err(failed)?
        {
            physics.insert(
                r.int(0).map_err(failed)?,
                [
                    r.opt_double(1).map_err(failed)?,
                    r.opt_double(2).map_err(failed)?,
                    r.opt_double(3).map_err(failed)?,
                    r.opt_double(4).map_err(failed)?,
                ],
            );
        }
    }
    let picks = picked(store, &stacks).map_err(failed)?;
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            // the axes the names step put on the scan, each `a,b` split
            let axes: Axes = scan["axes"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| {
                            v.as_str().map(|v| {
                                (
                                    k.clone(),
                                    v.split(',')
                                        .map(str::trim)
                                        .filter(|x| !x.is_empty())
                                        .map(str::to_string)
                                        .collect(),
                                )
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            scan["family"] = json!(family_of(&axes));
            let [te, tr, ti, fa] = physics.get(&stack).copied().unwrap_or([None; 4]);
            scan["te"] = json!(te);
            scan["tr"] = json!(tr);
            scan["ti"] = json!(ti);
            scan["fa"] = json!(fa);
            scan["main"] = json!(
                picks
                    .get(&stack)
                    .map(|r| r.iter().collect::<Vec<_>>())
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes(pairs: &[(&str, &str)]) -> Axes {
        let mut a = Axes::new();
        for (k, v) in pairs {
            a.entry(k.to_string()).or_default().push(v.to_string());
        }
        a
    }

    #[test]
    fn a_maker_is_named_as_a_person_names_it() {
        assert_eq!(maker_of("SIEMENS").as_deref(), Some("Siemens"));
        assert_eq!(maker_of("Siemens Healthineers").as_deref(), Some("Siemens"));
        assert_eq!(maker_of("GE MEDICAL SYSTEMS").as_deref(), Some("GE"));
        assert_eq!(maker_of("ge").as_deref(), Some("GE"));
        assert_eq!(maker_of("General Electric").as_deref(), Some("GE"));
        assert_eq!(
            maker_of("Philips Medical Systems").as_deref(),
            Some("Philips")
        );
        assert_eq!(maker_of("TOSHIBA_MEC").as_deref(), Some("Canon"));
        assert_eq!(maker_of("Canon Medical Systems").as_deref(), Some("Canon"));
        assert_eq!(maker_of("FUJIFILM Healthcare").as_deref(), Some("Hitachi"));
        assert_eq!(maker_of("  Bruker ").as_deref(), Some("Bruker"));
        // a word that only starts like a maker is not that maker
        assert_eq!(maker_of("Gemini").as_deref(), Some("Gemini"));
        assert_eq!(maker_of("   "), None);
    }

    #[test]
    fn a_scan_is_told_by_one_kind() {
        assert_eq!(kind_of(&axes(&[("base", "T1w")])), "T1w");
        assert_eq!(
            kind_of(&axes(&[("base", "T2w"), ("modifier", "FLAIR")])),
            "FLAIR"
        );
        assert_eq!(
            kind_of(&axes(&[
                ("base", "T2w"),
                ("modifier", "FatSat"),
                ("modifier", "FLAIR")
            ])),
            "FLAIR"
        );
        assert_eq!(kind_of(&axes(&[("base", "T2*w")])), "T2*w");
        assert_eq!(kind_of(&axes(&[("base", "SWI")])), "SWI");
        assert_eq!(
            kind_of(&axes(&[("provenance", "SWIRecon"), ("base", "T2*w")])),
            "SWI"
        );
        assert_eq!(kind_of(&axes(&[("base", "DWI")])), "DWI");
        assert_eq!(
            kind_of(&axes(&[("directory_type", "dwi"), ("base", "T2w")])),
            "DWI"
        );
        assert_eq!(kind_of(&axes(&[("directory_type", "perf")])), "Perfusion");
        assert_eq!(kind_of(&axes(&[("directory_type", "func")])), "fMRI");
        assert_eq!(kind_of(&axes(&[("directory_type", "fmap")])), "Field map");
        // what made it before what it weighs, a scout before anything
        assert_eq!(
            kind_of(&axes(&[("provenance", "SyMRI"), ("base", "T1w")])),
            "SyMRI"
        );
        assert_eq!(
            kind_of(&axes(&[("disposition", "scout"), ("base", "T1w")])),
            "Scout"
        );
        assert_eq!(kind_of(&axes(&[("directory_type", "localizer")])), "Scout");
        assert_eq!(kind_of(&axes(&[])), "Other");
        assert_eq!(kind_of(&axes(&[("base", "MTw")])), "Other");
        // every kind a scan can be is one the visit lists
        for a in [
            axes(&[("base", "PDw")]),
            axes(&[("base", "T2w")]),
            axes(&[]),
        ] {
            assert!(KINDS.contains(&kind_of(&a)));
        }
    }

    #[test]
    fn a_scan_is_grouped_in_one_family_as_the_desk_groups_it() {
        assert_eq!(family_of(&axes(&[("provenance", "SyMRI")])), "symri");
        assert_eq!(family_of(&axes(&[("provenance", "EPIMix")])), "mix");
        assert_eq!(family_of(&axes(&[("provenance", "NeuroMix")])), "mix");
        assert_eq!(family_of(&axes(&[("provenance", "STAGE")])), "stage");
        assert_eq!(family_of(&axes(&[("provenance", "SWIRecon")])), "swi");
        assert_eq!(
            family_of(&axes(&[("provenance", "ProjectionDerived")])),
            "derived"
        );
        assert_eq!(
            family_of(&axes(&[("provenance", "SubtractionDerived")])),
            "derived"
        );
        assert_eq!(family_of(&axes(&[("disposition", "reformat")])), "derived");
        assert_eq!(family_of(&axes(&[("body_part", "spine")])), "body");
        assert_eq!(family_of(&axes(&[("body_part", "neck")])), "body");
        // what made it before where it is
        assert_eq!(
            family_of(&axes(&[("provenance", "SyMRI"), ("body_part", "spine")])),
            "symri"
        );
        assert_eq!(family_of(&axes(&[("body_part", "brain")])), "plain");
        assert_eq!(family_of(&axes(&[("provenance", "RawRecon")])), "plain");
    }

    #[test]
    fn regions_split_the_brain_and_the_neck_and_keep_their_order() {
        assert_eq!(regions_of("brain-neck"), vec!["brain", "neck"]);
        assert_eq!(regions_of("spine"), vec!["spine"]);
        assert!(regions_of(" ").is_empty());
        let set: BTreeSet<String> = ["other", "spine", "brain", "knee", "neck"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            sorted_regions(&set),
            vec!["brain", "neck", "spine", "other", "knee"]
        );
    }

    #[test]
    fn contrast_is_the_stored_label_or_the_id() {
        assert!(contrast_of(&axes(&[("post_contrast", "1")])));
        assert!(contrast_of(&axes(&[("post_contrast", "given")])));
        assert!(!contrast_of(&axes(&[("post_contrast", "0")])));
        assert!(!contrast_of(&axes(&[])));
    }

    #[test]
    fn a_visit_is_its_session_else_its_day_else_none() {
        assert_eq!(visit_key(Some(4), Some("2026-01-02")), VisitKey::Session(4));
        assert_eq!(
            visit_key(None, Some("2026-01-02")),
            VisitKey::Day("2026-01-02".to_string())
        );
        assert_eq!(visit_key(None, Some("not a day")), VisitKey::Undated);
        assert_eq!(visit_key(None, None), VisitKey::Undated);
    }

    #[test]
    fn a_filter_knows_its_words_and_kinds() {
        let Ok(f) = Filters::parse(
            Some(&"look, region:Brain,region:spine,maker:GE".to_string()),
            &["look"],
            &["region", "maker"],
        ) else {
            panic!("a filter of known words parses");
        };
        assert!(f.wants("look"));
        let held = vec!["Spine".to_string()];
        assert!(f.any_of("region", &held));
        assert!(!f.any_of("maker", &held));
        assert!(f.any_of("main", &held));
        for bad in ["visits3", "region", "region:", "look:1", "maker"] {
            assert!(
                Filters::parse(Some(&bad.to_string()), &["look"], &["region", "maker"]).is_err(),
                "{bad}"
            );
        }
    }
}
