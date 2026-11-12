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
//! at every detail, but where it is told by an id type's values. An
//! identifier of a type the person chooses to show subjects by is opened
//! for the page's subjects, each read audited; it is shown as it is to a
//! caller who may read identifiers (data:work at detail sensitive, record
//! 26) and as its shape to any other, but for the alias a merge files
//! (`subject-code`), which is a code and is shown as codes are.
//!
//! The order of what subjects are shown by (2026-11-12, `order=id`, and the
//! tie-break of every other order but `code`): by the code, or by the
//! shown type's values in natural order for a caller who is shown them as
//! they are. A caller who is shown shapes is answered in code order, since
//! an order by values they may not read would tell them how the values
//! compare. Telling that order opens the value of every subject of the
//! scope, not the page's alone; it is audited as one `linkage.sort` row a
//! request, with the type and the counts and no value, and the order, as
//! subject ids, is kept in memory for a short time so that a person paging
//! through a scope does not open it whole at every page ([`Orders`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

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
use crate::kept::{Ask, Keep, Reading};
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

/// Why the values of a whole scope were opened to tell an order, in the
/// audit.
const WHY_SORTED: &str = "the dataset viewer, sorting subjects by an ID type";

/// How long an order by an id type is kept in memory.
const ORDER_KEPT: Duration = Duration::from_secs(300);

/// How many such orders are kept at once.
const ORDERS_MOST: usize = 32;

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
#[derive(Clone)]
pub(crate) enum Scope {
    /// An active source place and its `source` rows; a dataset nothing has
    /// read has none and holds no scan.
    Dataset {
        place: Box<Place>,
        sources: Vec<i64>,
    },
    Cohort(Cohort),
    /// The whole registry (record 55 H3, Browse): every stack, the main
    /// scans each subject's own dataset's once read, else the pack's, as a
    /// reader about no scope reads them.
    Registry {
        picks: nils_registry::pick_truth::Of,
    },
}

impl Scope {
    /// The whole registry, for Browse.
    pub(crate) fn registry(registry: &mut Registry) -> Result<Scope, Reply> {
        let picks = nils_registry::pick_truth::Of::own(registry.store()).map_err(failed)?;
        Ok(Scope::Registry { picks })
    }

    /// The dataset a door names, by its name or its id.
    pub(crate) fn dataset(registry: &mut Registry, name: &str) -> Result<Scope, Reply> {
        let place = crate::scans::dataset_named(registry, name)?;
        let sources = crate::sources::source_ids(registry.store(), &place).map_err(failed)?;
        Ok(Scope::Dataset {
            place: Box::new(place),
            sources,
        })
    }

    /// Whose picks the viewer reads, as the one resolver says
    /// (`nils_registry::pick_truth`): this dataset's or cohort's own once
    /// read, else the pack's (record 55, decision 6; 2026-10-11).
    pub(crate) fn picks(&self) -> nils_registry::pick_truth::Of {
        use nils_registry::pick_truth::Of;
        match self {
            Scope::Dataset { place, sources } => {
                Of::dataset(place.id, &place.name, sources.clone())
            }
            Scope::Cohort(c) => Of::cohort(c.id, &c.name),
            Scope::Registry { picks } => picks.clone(),
        }
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
            Scope::Registry { .. } => "registry",
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Scope::Dataset { place, .. } => &place.name,
            Scope::Cohort(c) => &c.name,
            Scope::Registry { .. } => "the registry",
        }
    }

    pub(crate) fn id(&self) -> i64 {
        match self {
            Scope::Dataset { place, .. } => place.id,
            Scope::Cohort(c) => c.id,
            Scope::Registry { .. } => 0,
        }
    }

    pub(crate) fn as_json(&self) -> Value {
        match self {
            Scope::Registry { .. } => json!({"kind": "registry"}),
            _ => json!({"kind": self.kind(), "name": self.name(), "id": self.id()}),
        }
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
        self.holds_for(store, crate::sealed::reads(access), st, se)
    }

    /// [`Scope::holds`] by the one thing of a caller it reads: whether the
    /// stacks of sealed samples are among the scope's (`unsealed`).
    fn holds_for(&self, store: &Store, unsealed: bool, st: &str, se: &str) -> Option<String> {
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
            // every stack of the registry
            Scope::Registry { .. } => "1 = 1".to_string(),
        };
        if !unsealed {
            out.push_str(&format!(
                " AND NOT EXISTS (SELECT 1 FROM {} sst WHERE sst.stack_id = {st}.id \
                 AND sst.unsealed_at IS NULL)",
                store.qualified("sealed_stack")
            ));
        }
        Some(out)
    }

    /// The current members of a cohort, every one of them whether or not a
    /// scan of theirs is in the scope; none for a dataset, which holds
    /// stacks and has no members (Nima, 2026-10-12): its subjects come with
    /// the scans its tree holds a file of.
    fn members(&self, store: &mut Store) -> Result<Option<Vec<i64>>, StoreError> {
        match self {
            Scope::Dataset { .. } | Scope::Registry { .. } => Ok(None),
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
pub(crate) fn study_day(store: &Store, alias: &str) -> String {
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
pub(crate) fn window(store: &mut Store) -> Result<i64, StoreError> {
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
/// makers brought to one word, any other as written. The pack's crate holds
/// it since the damaged-header flag reads makers too (record 56 §3).
pub(crate) fn maker_of(raw: &str) -> Option<String> {
    nils_pack::words::maker_of(raw)
}

/// A stack's decided axes, every value of each.
pub(crate) type Axes = BTreeMap<String, Vec<String>>;

fn has(a: &Axes, axis: &str, value: &str) -> bool {
    a.get(axis).is_some_and(|v| v.iter().any(|x| x == value))
}

/// The kinds of scan a visit is told by, in the order they are listed.
pub(crate) const KINDS: [&str; 15] = [
    "SyMRI",
    "DWI",
    "Perfusion",
    "fMRI",
    "Field map",
    "Scout",
    "FLAIR",
    "SWI",
    "STAGE",
    "Mix",
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
///
/// `folder` is the folder of its own the pack gives the stack under `anat/`
/// ([`own_folder`]): a scan in SyMRI's is SyMRI's, as a release and the
/// Browse folders place it, so SyMRI's multi-dynamic multi-echo or QALAS
/// acquisition, a working scan the pack calls RawRecon, counts with SyMRI
/// and not as Other (2026-10-14).
///
/// `acquisition` is the acquisition family the pack's routes recognised
/// ([`acquisition_family`]). One rule for every family: an image of the
/// acquisition with no weighting (no base), an SWI's phase, a STAGE echo's
/// phase or a map a route made with none, counts under its family (SWI,
/// SyMRI, STAGE, Mix) and not as Other, as the family's box holds it
/// (2026-10-14, the review of 2026-10-15). One with a weighting is told by
/// it, as an SWI magnitude's T2*w; SyMRI's images are SyMRI's whatever they
/// are, as its folder holds them.
pub(crate) fn kind_of(a: &Axes, folder: Option<&str>, acquisition: Option<&str>) -> &'static str {
    if has(a, "disposition", "scout")
        || has(a, "directory_type", "localizer")
        || has(a, "provenance", "Localizer")
    {
        "Scout"
    } else if is_symri(a, folder, acquisition) {
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
    } else if let Some(family) = acquisition.filter(|_| !weighted(a)) {
        kind_of_family(family)
    } else {
        "Other"
    }
}

/// Whether a scan has a weighting: a base value.
fn weighted(a: &Axes) -> bool {
    a.get("base")
        .is_some_and(|v| v.iter().any(|x| !x.is_empty()))
}

/// The kind an acquisition family's image with no weighting counts as.
fn kind_of_family(family: &str) -> &'static str {
    match family {
        "symri" => "SyMRI",
        "swi" => "SWI",
        "stage" => "STAGE",
        "mix" => "Mix",
        _ => "Other",
    }
}

/// The family a scan is grouped in inside its datatype's folder, as the
/// desk's tree groups it: its acquisition's
/// ([`nils_classify::family::acquisition_family`]:
/// SyMRI, a mix sequence, STAGE, an SWI, whatever the image is, so a
/// projection or a reformat of one stays with it), a derived image, a scan
/// of the spine or the neck, or none of these (`plain`).
pub(crate) fn family_of(acquisition: Option<&'static str>, a: &Axes) -> &'static str {
    let made = |p: &str| has(a, "provenance", p);
    if let Some(family) = acquisition {
        family
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

/// What the viewer reads of a scan: its axes as the classification stands,
/// a person's decisions in force over them, and its acquisition family.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    pub(crate) axes: Axes,
    pub(crate) family: Option<&'static str>,
}

/// The axes the viewer reads.
const VIEWER_AXES: [&str; 8] = [
    "body_part",
    "provenance",
    "post_contrast",
    "base",
    "modifier",
    "directory_type",
    "disposition",
    "technique",
];

/// Each of these scans as a visit's kinds read it, in one pass (the review
/// of 2026-10-15): its axes ([`axes_of`]) with every decision in force put
/// over the rules' answer, the decisions read once for these stacks on
/// every axis; and its acquisition family, the one reading a release and
/// the scans doors share ([`nils_classify::family::acquisition_family`]),
/// from those same decisions.
pub(crate) fn scans_of(
    store: &mut Store,
    pack: Option<&nils_pack::Pack>,
    stacks: &[i64],
) -> Result<HashMap<i64, Scan>, Reply> {
    let mut axes = axes_of(store, stacks).map_err(failed)?;
    let decided = nils_classify::family::in_force(store, Some(stacks)).map_err(failed)?;
    let families =
        nils_classify::family::families_with(store, pack, Some(stacks), &decided, &|s| {
            axes.get(&s)
                .and_then(|a| a.get("provenance"))
                .and_then(|v| v.first().cloned())
        })
        .map_err(failed)?;
    let mut out = HashMap::new();
    for &stack in stacks {
        let mut a = axes.remove(&stack).unwrap_or_default();
        for (axis, value) in decided.get(&stack).into_iter().flatten() {
            if !VIEWER_AXES.contains(&axis.as_str()) {
                continue;
            }
            let values = nils_registry::review::value_set(value.as_deref());
            if values.is_empty() {
                a.remove(axis);
            } else {
                a.insert(axis.clone(), values);
            }
        }
        out.insert(
            stack,
            Scan {
                axes: a,
                family: families.get(&stack).copied(),
            },
        );
    }
    Ok(out)
}

/// Whether a scan is SyMRI's: of its acquisition family, made by it
/// (provenance SyMRI), or in its folder, as its acquisition is (record 55
/// C4). The kinds and `symri` read this one source.
fn is_symri(a: &Axes, folder: Option<&str>, acquisition: Option<&str>) -> bool {
    acquisition == Some("symri") || has(a, "provenance", "SyMRI") || folder == Some("SyMRI")
}

/// The folder of its own the pack gives a stack under `anat/` (record 55
/// C4, its BIDS mapping's `folders`): SyMRI's, for its maps and synthetic
/// images by their provenance and for its acquisition by its technique (MDME
/// or QALAS), as a release and the scans doors place it
/// (`nils_release::run::name_of`). None with no pack served.
pub(crate) fn own_folder<'p>(pack: Option<&'p nils_pack::Pack>, a: &Axes) -> Option<&'p str> {
    let pack = pack?;
    let id = |axis: &str| -> Option<String> {
        let stored = a.get(axis)?.first()?;
        pack.axes
            .iter()
            .find(|x| x.name == axis)?
            .id_of_stored(stored)
            .map(str::to_string)
    };
    pack.bids
        .folder_of(id("provenance").as_deref(), id("technique").as_deref())
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
                 'base', 'modifier', 'directory_type', 'disposition', 'technique') ORDER BY id",
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
/// person withdrew or overruled, of the scope the viewer was opened from
/// once it is read, else the pack's; opened from none, each subject's own
/// dataset's (record 55, decision 6; one place of truth, 2026-10-11).
fn picked(
    store: &mut Store,
    stacks: &[i64],
    scope: Option<&Scope>,
) -> Result<HashMap<i64, BTreeSet<String>>, StoreError> {
    let picks = match scope {
        Some(s) => s.picks(),
        None => nils_registry::pick_truth::Of::own(store)?,
    }
    .picks(store);
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    for chunk in stacks.chunks(IN_CHUNKS) {
        for r in store.query(
            &format!(
                "SELECT ps.stack_id, p.role FROM {} ps JOIN {picks} p ON p.id = ps.pick_id \
                 WHERE ps.stack_id IN ({})",
                store.qualified("pick_stack"),
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
pub(crate) fn reads_identifiers(access: &Access) -> bool {
    access.holds("data:work") && access.detail >= Detail::Sensitive
}

/// The labels of these subjects by an id type: each one's value of it,
/// opened and audited, as it is where this caller may see it and as its
/// shape otherwise; a subject that holds none, or one whose value the store
/// keeps nothing of (a personnummer, a reserve number), is not in the answer.
/// By `subject-code` the label is the subject's alias, the code a merge
/// filed, and never its own code, which a digest of a pseudonymised tree
/// files under the same type (2026-11-11).
fn labels_by_type(
    registry: &mut Registry,
    caller: &Caller,
    shown: &Shown,
    subjects: &[i64],
) -> Result<HashMap<i64, String>, Reply> {
    if subjects.is_empty() || !linkage::keeps(&shown.name) {
        return Ok(HashMap::new());
    }
    let key = registry.pseudonym_key().map_err(failed)?;
    let keys = Subkeys::derive(&key);
    let mut store = registry.open_linkage().map_err(failed)?;
    let values = linkage::values_of_type_filed_by(
        &mut store,
        &keys,
        subjects,
        shown.id,
        (shown.name == SUBJECT_CODE_TYPE).then_some(linkage::FILED_BY_MERGE),
        &caller.principal,
        WHY,
    )
    .map_err(failed)?;
    let clear = reads_shown(&caller.access, shown);
    Ok(values
        .into_iter()
        .map(|(s, v)| (s, if clear { v } else { shape(&v) }))
        .collect())
}

/// Whether a caller is shown the values of this type as they are: one who
/// may read identifiers, and at detail quasi the alias a merge files, which
/// is a code. Any other caller is shown shapes, and no order by the values.
fn reads_shown(access: &Access, shown: &Shown) -> bool {
    reads_identifiers(access) || (shown.name == SUBJECT_CODE_TYPE && access.detail >= Detail::Quasi)
}

/// An order by an id type, as told over the subjects of one scope.
struct KeptOrder {
    /// The identities of the type as they stood ([`linkage::stamp_of_type`]).
    stamp: [i64; 4],
    told: Instant,
    /// The subjects it was told over, with or without a value.
    over: HashSet<i64>,
    /// The place of each subject that holds a value; never a value.
    places: HashMap<i64, usize>,
}

/// The orders kept in memory, by the registry, the scope (its kind and id)
/// and the id type: subject ids and places only, so nothing here is an
/// identifier. One is used again while the type's identities stand as they
/// stood, it is no older than [`ORDER_KEPT`] and it was told over every
/// subject asked about; a scope a caller sees less of (a sealed sample) is
/// served by the order told over more. Only a caller who is shown the
/// values reaches it ([`reads_shown`]), so an entry is never an order a
/// caller could not have been told.
type Orders = HashMap<(String, &'static str, i64, i64), Arc<KeptOrder>>;

static ORDERS: LazyLock<Mutex<Orders>> = LazyLock::new(|| Mutex::new(Orders::new()));

/// The place of each of the scope's subjects in the order of the values
/// they are shown by, for a caller who is shown them as they are; none for
/// any other caller, for a type whose values are not kept and for a scope
/// with no subject, where the order is the code's.
///
/// Every request that is answered in this order writes one `linkage.sort`
/// row: who, the scope, the type, how many subjects were put in order and
/// how many values were opened for it, which is none where a kept order
/// served. The page's own labels are audited one by one as before
/// ([`labels_by_type`]).
fn order_by_shown(
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    shown: &Shown,
    everyone: &[i64],
) -> Result<Option<Arc<KeptOrder>>, Reply> {
    if everyone.is_empty() || !linkage::keeps(&shown.name) || !reads_shown(&caller.access, shown) {
        return Ok(None);
    }
    let mut store = registry.open_linkage().map_err(failed)?;
    let stamp = linkage::stamp_of_type(&mut store, shown.id).map_err(failed)?;
    let key = (
        registry.meta().registry_id.clone(),
        scope.kind(),
        scope.id(),
        shown.id,
    );
    let kept = ORDERS
        .lock()
        .ok()
        .and_then(|orders| orders.get(&key).cloned())
        .filter(|k| {
            k.stamp == stamp
                && k.told.elapsed() < ORDER_KEPT
                && everyone.iter().all(|s| k.over.contains(s))
        });
    let (order, opened) = match kept {
        Some(k) => (k, 0),
        None => {
            let key_bytes = registry.pseudonym_key().map_err(failed)?;
            let told = linkage::order_by_type(
                &mut store,
                &Subkeys::derive(&key_bytes),
                everyone,
                shown.id,
                (shown.name == SUBJECT_CODE_TYPE).then_some(linkage::FILED_BY_MERGE),
            )
            .map_err(failed)?;
            let order = Arc::new(KeptOrder {
                stamp,
                told: Instant::now(),
                over: everyone.iter().copied().collect(),
                places: told.places,
            });
            if let Ok(mut orders) = ORDERS.lock() {
                orders.retain(|_, k| k.told.elapsed() < ORDER_KEPT);
                if orders.len() >= ORDERS_MOST {
                    orders.clear();
                }
                orders.insert(key, order.clone());
            }
            (order, told.opened)
        }
    };
    drop(store);
    let mut about = scope.as_json();
    about["id_type"] = json!(shown.name);
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::LinkageSort,
            scope: about,
            policy: None,
            job_id: None,
            details: Some(json!({
                "why": WHY_SORTED,
                "subjects": everyone.len(),
                "opened": opened,
            })),
        },
    )
    .map_err(failed)?;
    Ok(Some(order))
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

/// The id types the scope's subjects hold a kept value of, with how many
/// subjects hold one of each, most first; nothing is opened. A type is
/// offered by its values, never by its name alone (2026-11-11): a row that
/// keeps nothing, a lookup, labels no subject, and `subject-code` counts the
/// aliases a merge filed, never a subject's own code. Empty where the
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
    let aliases = types
        .iter()
        .find(|t| t.name == SUBJECT_CODE_TYPE)
        .map(|t| {
            format!(
                " AND (id_type_id <> {} OR source = '{}')",
                t.id,
                linkage::FILED_BY_MERGE
            )
        })
        .unwrap_or_default();
    for chunk in subjects.chunks(IN_CHUNKS) {
        let Ok(rows) = store.query(
            &format!(
                "SELECT DISTINCT id_type_id, subject_id FROM {identity} WHERE subject_id IN ({}) AND {}{aliases}",
                list(chunk),
                linkage::KEEPS_A_VALUE
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
        .filter(|t| linkage::keeps(&t.name))
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

/// What the subjects door knows of one subject, as it is kept: counts and
/// the few words the Grid filters by.
#[derive(Debug, Default, Clone, PartialEq)]
struct Facts {
    code: String,
    scans: i64,
    look: i64,
    visits: usize,
    /// In the order they are listed ([`region_rank`]).
    regions: Vec<String>,
    makers: Vec<String>,
    main: Vec<String>,
}

/// One subject while its scope is read.
#[derive(Default)]
struct Found {
    code: String,
    scans: i64,
    look: i64,
    visits: BTreeSet<VisitKey>,
    regions: BTreeSet<String>,
    makers: BTreeSet<String>,
    main: BTreeSet<String>,
}

/// The facts of one scope as they are kept (2026-11-12): each subject's,
/// the scope's totals and its facets, the ID types among them. They are
/// read with the sealed rule of one kind of caller and with nothing else of
/// any caller ([`gather`]), so nothing here is shaped, labelled or ordered
/// for anyone: a request does that itself ([`told`]).
#[derive(Debug, Default)]
struct Held {
    facts: BTreeMap<i64, Facts>,
    visits: usize,
    scans: i64,
    look: i64,
    makers: Vec<Value>,
    regions: Vec<Value>,
    roles: Vec<Value>,
    id_types: Vec<Value>,
    /// What is left in a slot the keep let go of ([`let_go`]): it answers
    /// no request.
    gone: bool,
}

impl Held {
    fn gone() -> Held {
        Held {
            gone: true,
            ..Held::default()
        }
    }

    /// About how many bytes the facts hold in memory, for the tests: each
    /// subject's row in the map, its words and the lists that hold them.
    #[cfg(test)]
    fn weight(&self) -> usize {
        let words = |list: &Vec<String>| {
            list.capacity() * std::mem::size_of::<String>()
                + list.iter().map(String::capacity).sum::<usize>()
        };
        self.facts
            .values()
            .map(|f| {
                std::mem::size_of::<(i64, Facts)>()
                    + f.code.capacity()
                    + words(&f.regions)
                    + words(&f.makers)
                    + words(&f.main)
            })
            .sum()
    }
}

/// The number of the door's reading: what a scope's kept facts are and how
/// they are read ([`gather`]). Bump it by hand in the change that makes
/// them say something else, so that nothing kept before answers after. It
/// is not the engine's version.
const COUNTING: u32 = 1;

/// How long a scope's facts are kept while their stamp is the same: the
/// bound on what moves a fact without moving the stamp ([`stamp_of`]).
const KEEP: Duration = Duration::from_secs(600);

/// And while a job or a batch runs, whose facts move as it goes: a few
/// seconds, and never less than ten times what the reading took.
const KEEP_BUSY: Duration = Duration::from_secs(3);

/// How many scopes' facts are kept at once, each sealed variant of a scope
/// counted by itself: asked for one more, the keep lets go of the one
/// asked for longest ago. A scope of 10,000 subjects weighs about 4 MB
/// (the test `a_kept_scope_weighs_what_is_said`).
const HELD_MOST: usize = 16;

/// The `registry_meta` key the keep looks under once for each slot. The
/// door writes nothing there ([`Gathering::stored`]): the first request
/// after a start of the engine reads the scope in front, as every request
/// did before.
const NEVER_STORED: &str = "subjects_door:not_kept_between_starts";

/// The facts kept, by scope and sealed variant ([`crate::kept`]).
static KEPT: LazyLock<Keep<Held>> = LazyLock::new(Default::default);

/// Which slots the keep holds now, and when each was last asked for.
#[derive(Default)]
struct Turns {
    asked: u64,
    made: u64,
    /// By key ([`key_of`]): the slot's number and when it was last asked.
    of: HashMap<String, (u64, u64)>,
}

impl Turns {
    /// The slot's number for a key, a new one where the keep holds none,
    /// and the slots let go of to keep no more than `most`: the ones asked
    /// for longest ago, never the one asked for now.
    fn turn(&mut self, key: &str, most: usize) -> (u64, Vec<String>) {
        self.asked += 1;
        let number = match self.of.get_mut(key) {
            Some(slot) => {
                slot.1 = self.asked;
                slot.0
            }
            None => {
                self.made += 1;
                self.of.insert(key.to_string(), (self.made, self.asked));
                self.made
            }
        };
        let mut out = Vec::new();
        while self.of.len() > most.max(1) {
            let Some(oldest) = self
                .of
                .iter()
                .filter(|(k, _)| k.as_str() != key)
                .min_by_key(|(_, slot)| slot.1)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((number, _)) = self.of.remove(&oldest) {
                out.push(slot_name(&oldest, number));
            }
        }
        (number, out)
    }

    fn holds(&self, key: &str, number: u64) -> bool {
        self.of.get(key).is_some_and(|slot| slot.0 == number)
    }
}

static TURNS: LazyLock<Mutex<Turns>> = LazyLock::new(Default::default);

/// How many slots the door keeps: [`HELD_MOST`]. The tests of one process
/// share the keep, and a test that counts readings must not lose its slot
/// to another test's scopes, so there the door lets go of none by itself
/// and the tests let go by hand ([`Turns::turn`], [`let_go`]).
fn held_most() -> usize {
    if cfg!(test) { usize::MAX } else { HELD_MOST }
}

/// What names a scope's facts for one kind of caller: the registry, the
/// scope, and whether the stacks of sealed samples are among them. That is
/// everything of a caller that changes the facts.
fn key_of(registry: &Registry, scope: &Scope, unsealed: bool) -> String {
    format!(
        "{}:{}:{}:{}",
        registry.meta().registry_id,
        scope.kind(),
        scope.id(),
        if unsealed { "all" } else { "hidden" }
    )
}

/// A slot of the keep: a key and the number of its turn. A scope asked for
/// again after the keep let go of it has a new number, so a slot let go
/// of is never asked again.
fn slot_name(key: &str, number: u64) -> String {
    format!("{key}#{number}")
}

/// The slot a request asks, having let go of what the keep holds too much
/// of.
fn slot_of(key: &str) -> (String, u64) {
    let (number, out) = TURNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .turn(key, held_most());
    let_go(&out);
    (slot_name(key, number), number)
}

/// Empty these slots. The keep has no way to drop a slot, so what it holds
/// there is replaced by nothing ([`Held::gone`]), which also keeps a
/// reading under way out of it; the empty slot itself, a few hundred
/// bytes, stays.
fn let_go(slots: &[String]) {
    for slot in slots {
        KEPT.amend(slot, |_| Held::gone());
    }
}

/// One reading of a scope's facts. It carries the scope and the sealed
/// variant, and nothing else of the caller who started it.
struct Gathering {
    scope: Scope,
    unsealed: bool,
    key: String,
    number: u64,
}

impl Gathering {
    fn still_kept(&self) -> bool {
        TURNS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .holds(&self.key, self.number)
    }
}

impl Reading<Held> for Gathering {
    type Error = Reply;

    fn read(&mut self, registry: &mut Registry) -> Result<Held, Reply> {
        // a slot the keep let go of holds nothing again: not what a
        // reading started just before would put there
        if !self.still_kept() {
            return Ok(Held::gone());
        }
        let held = gather(registry, &self.scope, self.unsealed)?;
        if !self.still_kept() {
            return Ok(Held::gone());
        }
        Ok(held)
    }

    /// Never written to the registry: a subject's facts are not kept
    /// between starts of the engine.
    fn stored(&self, _stamp: &str, _as_of: &str, _value: &Held) -> Option<String> {
        None
    }
}

/// What a scope's facts are read under, read on every request from indexes
/// and small tables, and whether a job or a batch runs now:
///
/// - the reading's own number ([`COUNTING`]);
/// - what moves a dataset's summary ([`crate::dataset_summary::moved`]):
///   the batches, the newest finished job, the questions, the judgements,
///   the merges, the sealed stacks, the labels and the session cache;
/// - the scope: a dataset's `source` rows, a cohort's members, and whose
///   picks it reads ([`Scope::picks`]);
/// - the picks: the newest pick and the newest picked stack, how many
///   picks of a person were withdrawn, which writes no row and is told by
///   the audit, and the rules they were made by and the runs of those;
/// - the sealed stacks ever sealed and the newest, since one sealed and
///   one unsealed leave the number sealed now as it was;
/// - the questions waiting on a person, staged ones too, which is what
///   need-a-look counts ([`crate::certainty::Asks::sort`]);
/// - the window of the newest session of the cache;
/// - the identities and the ID types of the linkage store, for the ID type
///   facet.
///
/// While a job or a batch runs, the questions and the picks it writes are
/// not in it, as its questions and judgements are not in the summary's:
/// the facts are read again every few seconds instead ([`KEEP_BUSY`]).
/// A member of a waiting question removed by itself, and a value or a pick
/// rewritten in place by anything but the engine's own doors, move nothing
/// here; [`KEEP`] bounds those.
fn stamp_of(registry: &mut Registry, scope: &Scope) -> Result<(String, bool), Reply> {
    let store = registry.store();
    let (moved, busy) = crate::dataset_summary::moved(store).map_err(failed)?;
    let q = |t: &str| store.qualified(t);
    let (pick, pick_stack, rules, run) = (
        q("pick"),
        q("pick_stack"),
        q("pick_rules"),
        q("pick_rules_run"),
    );
    let (sealed, item, session, audit) = (
        q("sealed_stack"),
        q("review_item"),
        q("session_cache"),
        q("audit"),
    );
    let waiting = crate::certainty::WAITING;
    let withdrawn = nils_registry::audit::Action::PickWithdraw.name();
    let r = &store
        .query(
            &format!(
                "SELECT (SELECT MAX(id) FROM {pick}), (SELECT MAX(id) FROM {pick_stack}), \
                 (SELECT COUNT(*) FROM {audit} WHERE action = '{withdrawn}'), \
                 (SELECT MAX(id) FROM {rules}), (SELECT MAX(id) FROM {run}), \
                 (SELECT COUNT(*) FROM {run} WHERE state = 'done'), \
                 (SELECT COUNT(*) FROM {sealed}), (SELECT MAX(id) FROM {sealed}), \
                 (SELECT COUNT(*) FROM {item} WHERE status IN ({waiting})), \
                 (SELECT window_days FROM {session} ORDER BY id DESC LIMIT 1)"
            ),
            &[],
        )
        .map_err(failed)?[0];
    let n = |i: usize| -> Result<i64, Reply> { Ok(r.opt_int(i).map_err(failed)?.unwrap_or(0)) };
    let picks = format!("{}:{}:{}", n(0)?, n(1)?, n(2)?);
    let (rules, sealed) = (
        format!("{}:{}:{}", n(3)?, n(4)?, n(5)?),
        format!("{}:{}", n(6)?, n(7)?),
    );
    let (waiting, window) = (n(8)?, n(9)?);
    let moving = if busy {
        "-".to_string()
    } else {
        format!("{picks}|{waiting}")
    };
    let members = match scope {
        Scope::Cohort(c) => {
            let r = &store
                .query(
                    &format!(
                        "SELECT COUNT(*), MAX(id), COUNT(left_at), CAST(COALESCE(SUM(CASE \
                         WHEN left_at IS NULL THEN subject_id ELSE 0 END), 0) AS BIGINT) \
                         FROM {} WHERE cohort_id = {}",
                        store.qualified("cohort_member"),
                        c.id
                    ),
                    &[],
                )
                .map_err(failed)?[0];
            let n =
                |i: usize| -> Result<i64, Reply> { Ok(r.opt_int(i).map_err(failed)?.unwrap_or(0)) };
            format!("{}:{}:{}:{}", n(0)?, n(1)?, n(2)?, n(3)?)
        }
        Scope::Dataset { .. } | Scope::Registry { .. } => "-".to_string(),
    };
    let identities = identities_moved(registry);
    Ok((
        format!(
            "{COUNTING}|{moved}|{:?}|{members}|{rules}|{sealed}|{moving}|{window}|{identities}",
            scope.picks()
        ),
        busy,
    ))
}

/// What moves the ID type facet: how many identities the linkage store
/// holds, the newest and whose they are, and the ID types. A value
/// rewritten in place is not in it, since the facet counts subjects and
/// reads no value. `-` where the linkage store cannot be read, where the
/// facet is empty too.
fn identities_moved(registry: &mut Registry) -> String {
    let read = |store: &mut Store| -> Result<String, StoreError> {
        let (identity, id_type) = (store.qualified("identity"), store.qualified("id_type"));
        let r = &store.query(
            &format!(
                "SELECT COUNT(*), COALESCE(MAX(id), 0), \
                 CAST(COALESCE(SUM(subject_id), 0) AS BIGINT), \
                 (SELECT COUNT(*) FROM {id_type}), (SELECT COALESCE(MAX(id), 0) FROM {id_type}) \
                 FROM {identity}"
            ),
            &[],
        )?[0];
        Ok(format!(
            "{}:{}:{}:{}:{}",
            r.int(0)?,
            r.int(1)?,
            r.int(2)?,
            r.int(3)?,
            r.int(4)?
        ))
    };
    match registry.open_linkage() {
        Ok(mut store) => read(&mut store).unwrap_or_else(|_| "-".to_string()),
        Err(_) => "-".to_string(),
    }
}

/// Read a scope's facts from the registry: every subject's, the totals and
/// the facets. `unsealed` is whether the stacks of sealed samples are
/// among them, the one thing of a caller that changes what is read; no
/// caller is known here.
fn gather(registry: &mut Registry, scope: &Scope, unsealed: bool) -> Result<Held, Reply> {
    let store = registry.store();
    let mut facts: BTreeMap<i64, Found> = BTreeMap::new();
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
                    Found {
                        code: r.text(1).map_err(failed)?.to_string(),
                        ..Found::default()
                    },
                );
            }
        }
    }
    let holds = scope.holds_for(store, unsealed, "st", "se");
    if let Some(holds) = &holds {
        let from = stacks_from(store);
        let q = |t: &str| store.qualified(t);
        let (subject, study, cached, axis, fp, pick_stack) = (
            q("subject"),
            q("study"),
            q("session_cache_study"),
            q("classification_axis"),
            q("stack_fingerprint"),
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
        // the roles of the live picks among the scope's scans, the scope's
        // own where it has main-scan rules (record 55, decision 6)
        let pick = scope.picks().picks(store);
        for r in store
            .query(
                &format!(
                    "SELECT DISTINCT se.subject_id, p.role FROM {from} \
                     JOIN {pick_stack} ps ON ps.stack_id = st.id \
                     JOIN {pick} p ON p.id = ps.pick_id \
                     WHERE {holds}"
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
    Ok(Held {
        facts: facts
            .into_iter()
            .map(|(id, f)| {
                (
                    id,
                    Facts {
                        code: f.code,
                        scans: f.scans,
                        look: f.look,
                        visits: f.visits.len(),
                        regions: sorted_regions(&f.regions),
                        makers: f.makers.into_iter().collect(),
                        main: f.main.into_iter().collect(),
                    },
                )
            })
            .collect(),
        visits,
        scans,
        look,
        makers: facet(&makers, |m| m.to_string()),
        regions: facet(&regions, region_rank),
        roles: facet(&roles, |r| r.to_string()),
        id_types,
        gone: false,
    })
}

/// What a request asks the subjects door for, but `show`.
struct Asking<'q> {
    text: Option<String>,
    order: &'q str,
    filters: Filters,
    limit: usize,
    after: Option<i64>,
}

impl<'q> Asking<'q> {
    fn parse(query: &'q HashMap<String, String>) -> Result<Asking<'q>, Reply> {
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
            Some(o @ ("look" | "code" | "id" | "visits" | "scans")) => o,
            Some(o) => {
                return Err(Reply::error(
                    400,
                    format!("order: {o} is not look, code, id, visits or scans"),
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
        Ok(Asking {
            text,
            order,
            filters,
            limit,
            after,
        })
    }
}

/// `GET /api/{datasets|cohorts}/{name}/subjects`: the scope's subjects, a
/// page at a time, with what the Grid filters them by.
///
/// The scope's facts are kept (2026-11-12, [`crate::kept`]): a request is
/// answered from them at once and reads from the registry only what is its
/// own, the labels of its page, an order by a shown ID type and an
/// identifier searched for. Where what the facts were read under moved
/// ([`stamp_of`]) they are read again apart, and the answer says so
/// (`counting`) with when they were read (`counts_as_of`). Only a scope
/// with no facts kept makes a request wait for them, as every request did
/// before. On a registry of 900,000 scans the door took seconds for every
/// page, and a minute and more while another door counted a dataset.
pub(crate) fn subjects(
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    query: &HashMap<String, String>,
) -> Result<Value, Reply> {
    let asking = Asking::parse(query)?;
    let shown = shown_by(registry, query.get("show"))?;
    // everything of the caller that changes the facts; the rest of the
    // caller is applied to them by each request
    let unsealed = crate::sealed::reads(&caller.access);
    let (stamp, busy) = stamp_of(registry, scope)?;
    let key = key_of(registry, scope, unsealed);
    // a slot the keep let go of between its naming and its asking answers
    // nothing, and the scope is asked for again under a new one
    for _ in 0..3 {
        let (slot, number) = slot_of(&key);
        let answer = KEPT.answer(
            registry,
            &Ask {
                slot: &slot,
                stored: NEVER_STORED,
                stamp: &stamp,
                ttl: if busy { KEEP_BUSY } else { KEEP },
                paced: busy,
                wait: true,
                fresh: false,
            },
            |_| None,
            || Gathering {
                scope: scope.clone(),
                unsealed,
                key: key.clone(),
                number,
            },
        )?;
        let Some(held) = answer.value.filter(|h| !h.gone) else {
            continue;
        };
        let mut doc = told(registry, caller, scope, &held, &asking, shown.as_ref())?;
        doc["counting"] = json!(answer.counting);
        doc["counts_as_of"] = json!(answer.as_of);
        return Ok(doc);
    }
    in_front(registry, caller, scope, &asking, shown.as_ref())
}

/// The door's answer read from the registry now, with nothing kept: what
/// every request did before the facts were kept, and what the kept answer
/// is held against in the tests.
fn in_front(
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    asking: &Asking<'_>,
    shown: Option<&Shown>,
) -> Result<Value, Reply> {
    let as_of = nils_registry::time::now_iso();
    let held = gather(registry, scope, crate::sealed::reads(&caller.access))?;
    let mut doc = told(registry, caller, scope, &held, asking, shown)?;
    doc["counting"] = json!(false);
    doc["counts_as_of"] = json!(as_of);
    Ok(doc)
}

/// A request's own answer from a scope's facts: what `q` and `filter`
/// keep, in the order asked for, one page of it, each subject's code and
/// label as this caller is shown them. Everything of the caller but the
/// sealed rule is applied here and never kept: the detail level, the right
/// to read identifiers and the shape of a code.
fn told(
    registry: &mut Registry,
    caller: &Caller,
    scope: &Scope,
    held: &Held,
    asking: &Asking<'_>,
    shown: Option<&Shown>,
) -> Result<Value, Reply> {
    let access = &caller.access;
    let quasi = access.detail >= Detail::Quasi;
    let facts = &held.facts;
    let order = asking.order;
    let filters = &asking.filters;
    let everyone: Vec<i64> = facts.keys().copied().collect();

    // what `q` finds: the code as this caller is shown it, and an
    // identifier given whole to a caller who may read one
    let shown_code = |code: &str| {
        if quasi { code.to_string() } else { shape(code) }
    };
    let by_identifier = match &asking.text {
        Some(t) if reads_identifiers(access) => found_by_identifier(registry, t)?,
        _ => HashSet::new(),
    };
    let needle = asking.text.as_ref().map(|t| t.to_lowercase());
    let mut kept: Vec<(&i64, &Facts)> = facts
        .iter()
        .filter(|(id, f)| match &needle {
            None => true,
            Some(n) => shown_code(&f.code).to_lowercase().contains(n) || by_identifier.contains(id),
        })
        .filter(|(_, f)| !filters.wants("look") || f.look > 0)
        .filter(|(_, f)| !filters.wants("visits2") || f.visits > 1)
        .filter(|(_, f)| filters.any_of("main", &f.main))
        .filter(|(_, f)| filters.any_of("region", &f.regions))
        .filter(|(_, f)| filters.any_of("maker", &f.makers))
        .collect();
    // the order of what the subjects are shown by: `id` is that order, and
    // every other order but `code`, which is the code's as it always was,
    // breaks its ties by it. A subject with no value of the shown type
    // stands after those with one. Told over the whole scope, whatever `q`
    // and `filter` keep, so one kept order serves every narrowing of it.
    let shown_order = match shown {
        Some(s) if order != "code" => order_by_shown(registry, caller, scope, s, &everyone)?,
        _ => None,
    };
    let place = |id: &i64| {
        shown_order
            .as_ref()
            .and_then(|o| o.places.get(id).copied())
            .unwrap_or(usize::MAX)
    };
    kept.sort_by(|(ia, a), (ib, b)| {
        let by = match order {
            "look" => b.look.cmp(&a.look),
            "visits" => b.visits.cmp(&a.visits),
            "scans" => b.scans.cmp(&a.scans),
            _ => std::cmp::Ordering::Equal,
        };
        by.then_with(|| place(ia).cmp(&place(ib)))
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| ia.cmp(ib))
    });
    let matched = kept.len();
    let start = match asking.after {
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
    let page: Vec<(&i64, &Facts)> = kept
        .iter()
        .skip(start)
        .take(asking.limit)
        .copied()
        .collect();
    let next = if start + page.len() < matched {
        page.last().map(|(id, _)| **id)
    } else {
        None
    };
    let ids: Vec<i64> = page.iter().map(|(id, _)| **id).collect();
    let labels = match shown {
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
                "visits": f.visits,
                "scans": f.scans,
                "look": f.look,
                "regions": f.regions,
                "makers": f.makers,
                "main": f.main,
            })
        })
        .collect();
    Ok(json!({
        "scope": scope.as_json(),
        "detail": access.detail.name(),
        "show": shown.map_or("code", |s| s.name.as_str()),
        "order": order,
        "totals": {
            "subjects": facts.len(),
            "visits": held.visits,
            "scans": held.scans,
            "look": held.look,
        },
        "matched": matched,
        "count": subjects.len(),
        "subjects": subjects,
        "next": next,
        "facets": {
            "makers": held.makers,
            "regions": held.regions,
            "roles": held.roles,
            "id_types": held.id_types,
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
            match scope {
                Scope::Registry { .. } => format!("no subject {subject} in the registry"),
                _ => format!("no subject {subject} in {} {}", scope.kind(), scope.name()),
            },
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
    // each scan's axes, decisions applied, and its acquisition family
    let read = scans_of(store, pack, &ids)?;
    let looks: HashSet<i64> = match &holds {
        Some(holds) => looked(store, holds, Some(subject))
            .map_err(failed)?
            .into_keys()
            .collect(),
        None => HashSet::new(),
    };
    let picks = picked(store, &ids, Some(scope)).map_err(failed)?;
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
    let empty = Scan::default();
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
            let scan = read.get(stack).unwrap_or(&empty);
            let a = &scan.axes;
            // SyMRI's acquisition is SyMRI's here as in its folder (record
            // 55 C4, 2026-10-14), for the kinds and for `symri`; an image
            // of a family with no weighting is the family's, as its box
            // holds it
            let folder = own_folder(pack, a);
            let acquisition = scan.family;
            *kinds.entry(kind_of(a, folder, acquisition)).or_default() += 1;
            for part in a.get("body_part").into_iter().flatten() {
                regions.extend(regions_of(part));
            }
            contrast |= contrast_of(a);
            if is_symri(a, folder, acquisition) {
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

/// The dataset viewer's facts on each scan of a page: `acquisition_family`,
/// the family the pack's routes recognised in it (symri, swi, mix, stage
/// or null, 2026-10-14); `family`, the group it is shown in inside its
/// datatype's folder, that family first; `te`, `tr`, `ti` and `fa`,
/// the echo, repetition and inversion times (milliseconds) and the flip
/// angle (degrees) from its fingerprint, or null; and `main`, the roles of
/// the live picks naming it. None of it is quasi-identifying.
pub(crate) fn with_facts(
    registry: &mut Registry,
    pack: Option<&nils_pack::Pack>,
    doc: &mut Value,
    scope: &Scope,
) -> Result<(), Reply> {
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
    let picks = picked(store, &stacks, Some(scope)).map_err(failed)?;
    // the axes the names step put on each scan, each `a,b` split, and each
    // scan's family from the classification as it stands, as the visits'
    // kinds read it
    let axes: HashMap<i64, Axes> = doc["scans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scan| {
            let stack = scan["stack"].as_i64()?;
            let a: Axes = scan["axes"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| {
                            v.as_str()
                                .map(|v| (k.clone(), nils_registry::review::value_set(Some(v))))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some((stack, a))
        })
        .collect();
    let families = nils_classify::family::families_of(store, pack, Some(&stacks), &|s| {
        axes.get(&s)
            .and_then(|a| a.get("provenance"))
            .and_then(|v| v.first().cloned())
    })
    .map_err(failed)?;
    let empty = Axes::new();
    if let Some(scans) = doc["scans"].as_array_mut() {
        for scan in scans {
            let stack = scan["stack"].as_i64().unwrap_or_default();
            let acquisition = families.get(&stack).copied();
            scan["acquisition_family"] = json!(acquisition);
            scan["family"] = json!(family_of(acquisition, axes.get(&stack).unwrap_or(&empty)));
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

    /// The one family reading, from the routes a stack's evidence names
    /// and its provenance, with no decision in force.
    fn acquisition_family(routes: &[String], a: &Axes) -> Option<&'static str> {
        let said: Vec<nils_classify::family::Said> = routes
            .iter()
            .map(|r| nils_classify::family::Said {
                route: r.clone(),
                axes: BTreeMap::new(),
            })
            .collect();
        nils_classify::family::acquisition_family(
            None,
            &said,
            &nils_classify::family::InForce::new(),
            a.get("provenance")
                .and_then(|v| v.first())
                .map(String::as_str),
        )
    }

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
        assert_eq!(kind_of(&axes(&[("base", "T1w")]), None, None), "T1w");
        assert_eq!(
            kind_of(&axes(&[("base", "T2w"), ("modifier", "FLAIR")]), None, None),
            "FLAIR"
        );
        assert_eq!(
            kind_of(
                &axes(&[
                    ("base", "T2w"),
                    ("modifier", "FatSat"),
                    ("modifier", "FLAIR")
                ]),
                None,
                None
            ),
            "FLAIR"
        );
        assert_eq!(kind_of(&axes(&[("base", "T2*w")]), None, None), "T2*w");
        assert_eq!(kind_of(&axes(&[("base", "SWI")]), None, None), "SWI");
        assert_eq!(
            kind_of(
                &axes(&[("provenance", "SWIRecon"), ("base", "T2*w")]),
                None,
                None
            ),
            "SWI"
        );
        assert_eq!(kind_of(&axes(&[("base", "DWI")]), None, None), "DWI");
        assert_eq!(
            kind_of(
                &axes(&[("directory_type", "dwi"), ("base", "T2w")]),
                None,
                None
            ),
            "DWI"
        );
        assert_eq!(
            kind_of(&axes(&[("directory_type", "perf")]), None, None),
            "Perfusion"
        );
        assert_eq!(
            kind_of(&axes(&[("directory_type", "func")]), None, None),
            "fMRI"
        );
        assert_eq!(
            kind_of(&axes(&[("directory_type", "fmap")]), None, None),
            "Field map"
        );
        // what made it before what it weighs, a scout before anything
        assert_eq!(
            kind_of(
                &axes(&[("provenance", "SyMRI"), ("base", "T1w")]),
                None,
                None
            ),
            "SyMRI"
        );
        assert_eq!(
            kind_of(
                &axes(&[("disposition", "scout"), ("base", "T1w")]),
                None,
                None
            ),
            "Scout"
        );
        assert_eq!(
            kind_of(&axes(&[("directory_type", "localizer")]), None, None),
            "Scout"
        );
        assert_eq!(kind_of(&axes(&[]), None, None), "Other");
        assert_eq!(kind_of(&axes(&[("base", "MTw")]), None, None), "Other");
        // every kind a scan can be is one the visit lists
        for a in [
            axes(&[("base", "PDw")]),
            axes(&[("base", "T2w")]),
            axes(&[]),
        ] {
            assert!(KINDS.contains(&kind_of(&a, None, None)));
        }
    }

    /// The MRI pack as the repository holds it.
    fn mri() -> &'static nils_pack::Pack {
        static PACK: std::sync::OnceLock<nils_pack::Pack> = std::sync::OnceLock::new();
        PACK.get_or_init(|| {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
            nils_pack::load(&dir, None).expect("the MRI pack loads")
        })
    }

    #[test]
    fn symri_s_acquisition_counts_with_symri_as_its_folder_holds_it() {
        // 2026-10-14: the pack calls SyMRI's multi-dynamic multi-echo or
        // QALAS acquisition RawRecon and a working scan; it is in SyMRI's
        // folder (record 55 C4), and a visit counts it with SyMRI, not as
        // Other
        let kind = |a: &Axes| kind_of(a, own_folder(Some(mri()), a), None);
        for technique in ["MDME", "QALAS"] {
            let acquisition = axes(&[
                ("provenance", "RawRecon"),
                ("technique", technique),
                ("directory_type", "anat"),
                ("disposition", "working_scan"),
            ]);
            assert_eq!(own_folder(Some(mri()), &acquisition), Some("SyMRI"));
            assert_eq!(kind(&acquisition), "SyMRI", "{technique}");
            assert!(is_symri(
                &acquisition,
                own_folder(Some(mri()), &acquisition),
                None
            ));
        }
        // its maps and synthetic images, by their provenance as before
        let synthetic = axes(&[
            ("provenance", "SyMRI"),
            ("technique", "MDME"),
            ("base", "T1w"),
        ]);
        assert_eq!(kind(&synthetic), "SyMRI");
        // a scout is a scout first, a working scan no folder claims is Other
        let scout = axes(&[("technique", "MDME"), ("disposition", "scout")]);
        assert_eq!(kind(&scout), "Scout");
        let calibration = axes(&[
            ("provenance", "Calibration"),
            ("disposition", "working_scan"),
        ]);
        assert_eq!(own_folder(Some(mri()), &calibration), None);
        assert_eq!(kind(&calibration), "Other");
        // an SWI's magnitude keeps its weighting; no folder of its own
        let magnitude = axes(&[
            ("provenance", "RawRecon"),
            ("technique", "GRE-EPI"),
            ("base", "T2*w"),
        ]);
        assert_eq!(kind(&magnitude), "T2*w");
        // with no pack served nothing knows SyMRI's folder by its technique
        let mdme = axes(&[("provenance", "RawRecon"), ("technique", "MDME")]);
        assert_eq!(own_folder(None, &mdme), None);
        assert_eq!(kind_of(&mdme, None, None), "Other");
    }

    #[test]
    fn an_swi_s_own_image_with_no_weighting_counts_as_swi_as_its_box_holds_it() {
        // 2026-10-14: the SWI route calls an SWI's phase RawRecon and gives
        // it no base, so a visit counted it as Other while the SWI box held
        // it; by the acquisition family its route recognised it is the SWI's
        let phase = axes(&[
            ("provenance", "RawRecon"),
            ("technique", "GRE-EPI"),
            ("directory_type", "anat"),
        ]);
        assert_eq!(kind_of(&phase, None, Some("swi")), "SWI");
        // any of the SWI's images with no weighting, a map it made with none
        let r2star = axes(&[("provenance", "SWIRecon"), ("construct", "R2starmap")]);
        assert_eq!(kind_of(&r2star, None, Some("swi")), "SWI");
        // its magnitude keeps its weighting, its minIP and processed image
        // are SWI as before
        let magnitude = axes(&[("provenance", "RawRecon"), ("base", "T2*w")]);
        assert_eq!(kind_of(&magnitude, None, Some("swi")), "T2*w");
        let minip = axes(&[
            ("provenance", "ProjectionDerived"),
            ("base", "SWI"),
            ("disposition", "reformat"),
        ]);
        assert_eq!(kind_of(&minip, None, Some("swi")), "SWI");
        // what no route recognised is Other as before
        assert_eq!(kind_of(&phase, None, None), "Other");
        // the family the visits door reads it by, from the route its
        // evidence cites, or the provenance a route enters on
        let routes = vec!["swi".to_string(), "body_part".to_string()];
        assert_eq!(acquisition_family(&routes, &phase), Some("swi"));
        assert_eq!(
            kind_of(&phase, None, acquisition_family(&routes, &phase)),
            "SWI"
        );
    }

    #[test]
    fn an_image_with_no_weighting_counts_under_its_family_and_one_with_a_base_never_does() {
        // the review of 2026-10-15: one rule for every family, so the kinds
        // line and the boxes agree
        let phase = axes(&[("provenance", "RawRecon"), ("technique", "ME-GRE")]);
        assert_eq!(kind_of(&phase, None, Some("swi")), "SWI");
        assert_eq!(kind_of(&phase, None, Some("stage")), "STAGE");
        assert_eq!(kind_of(&phase, None, Some("mix")), "Mix");
        assert_eq!(kind_of(&phase, None, Some("symri")), "SyMRI");
        // a scan with any base is told by it, or is Other: never the family
        for base in ["MTw", "T1rho", "Unknown"] {
            let a = axes(&[("provenance", "RawRecon"), ("base", base)]);
            for family in ["swi", "stage", "mix"] {
                assert_eq!(kind_of(&a, None, Some(family)), "Other", "{base} {family}");
            }
        }
        let t1 = axes(&[("provenance", "RawRecon"), ("base", "T1w")]);
        assert_eq!(kind_of(&t1, None, Some("stage")), "T1w");
        // a family's kind is one of the kinds listed
        for family in ["swi", "stage", "mix", "symri"] {
            assert!(KINDS.contains(&kind_of(&phase, None, Some(family))));
        }
        // the family is the route the evidence gives first, not a fixed list
        let both = vec!["stage".to_string(), "swi".to_string()];
        assert_eq!(acquisition_family(&both, &phase), Some("stage"));
        let both = vec!["swi".to_string(), "stage".to_string()];
        assert_eq!(acquisition_family(&both, &phase), Some("swi"));
    }

    #[test]
    fn a_scan_is_grouped_in_one_family_as_the_desk_groups_it() {
        // by its axes alone, as before any route cited: what made it
        let family = |a: &Axes| family_of(acquisition_family(&[], a), a);
        assert_eq!(family(&axes(&[("provenance", "SyMRI")])), "symri");
        assert_eq!(family(&axes(&[("provenance", "EPIMix")])), "mix");
        assert_eq!(family(&axes(&[("provenance", "NeuroMix")])), "mix");
        assert_eq!(family(&axes(&[("provenance", "STAGE")])), "stage");
        assert_eq!(family(&axes(&[("provenance", "SWIRecon")])), "swi");
        assert_eq!(
            family(&axes(&[("provenance", "ProjectionDerived")])),
            "derived"
        );
        assert_eq!(
            family(&axes(&[("provenance", "SubtractionDerived")])),
            "derived"
        );
        assert_eq!(family(&axes(&[("disposition", "reformat")])), "derived");
        assert_eq!(family(&axes(&[("body_part", "spine")])), "body");
        assert_eq!(family(&axes(&[("body_part", "neck")])), "body");
        // what made it before where it is
        assert_eq!(
            family(&axes(&[("provenance", "SyMRI"), ("body_part", "spine")])),
            "symri"
        );
        assert_eq!(family(&axes(&[("body_part", "brain")])), "plain");
        assert_eq!(family(&axes(&[("provenance", "RawRecon")])), "plain");
        assert_eq!(acquisition_family(&[], &axes(&[("base", "T1w")])), None);
    }

    #[test]
    fn an_acquisition_s_own_images_join_the_family_of_the_route_that_decided_them() {
        // 2026-10-14: the routes call what an acquisition's scanner acquired
        // RawRecon and an SWI's minIP ProjectionDerived; the rule set their
        // evidence cites says which acquisition made them.
        let routes = |sets: &[&str]| sets.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let family = |sets: &[&str], a: &Axes| {
            let acquisition = acquisition_family(&routes(sets), a);
            (acquisition, family_of(acquisition, a))
        };
        // SyMRI's multi-dynamic multi-echo acquisition, a working scan
        let mdme = axes(&[
            ("provenance", "RawRecon"),
            ("technique", "MDME"),
            ("disposition", "working_scan"),
        ]);
        assert_eq!(
            family(&["symri", "disposition"], &mdme),
            (Some("symri"), "symri")
        );
        // an SWI's magnitude, its phase, its minIP and its processed image
        let magnitude = axes(&[("provenance", "RawRecon"), ("base", "T2*w")]);
        let phase = axes(&[("provenance", "RawRecon")]);
        let minip = axes(&[
            ("provenance", "ProjectionDerived"),
            ("construct", "MinIP"),
            ("base", "SWI"),
            ("disposition", "reformat"),
        ]);
        let swi = axes(&[("provenance", "SWIRecon"), ("construct", "SWI")]);
        for a in [&magnitude, &phase, &minip, &swi] {
            assert_eq!(
                family(&["swi", "body_part"], a),
                (Some("swi"), "swi"),
                "{a:?}"
            );
        }
        // a SyMRI or STAGE projection or reformat stays with its acquisition
        let reformat = axes(&[
            ("provenance", "ProjectionDerived"),
            ("disposition", "reformat"),
        ]);
        assert_eq!(family(&["symri"], &reformat), (Some("symri"), "symri"));
        assert_eq!(family(&["stage"], &reformat), (Some("stage"), "stage"));
        // the mix sequences are one route
        assert_eq!(family(&["epimix"], &magnitude), (Some("mix"), "mix"));
        // what no route decided is grouped as before
        assert_eq!(family(&["base", "construct"], &minip), (None, "derived"));
        assert_eq!(family(&[], &phase), (None, "plain"));
        // a route that entered and had nothing left to decide (a display
        // composite) is told by the provenance it entered on
        let composite = axes(&[
            ("provenance", "SyMRI"),
            ("disposition", "display_composite"),
        ]);
        assert_eq!(family(&["display"], &composite), (Some("symri"), "symri"));
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

    /// A registry in `dir`, on SQLite or, with a DSN, in a schema of its own.
    fn registry(dir: &std::path::Path, postgres: Option<(&str, &str)>) -> Registry {
        use nils_registry::{Backend, Home, InitOptions, Scheme};
        let home = Home::new(dir);
        home.keys(None).add("nils", b"a test passphrase").unwrap();
        home.init(&InitOptions {
            backend: if postgres.is_some() {
                Backend::Postgres
            } else {
                Backend::Sqlite
            },
            dsn: postgres.map(|(dsn, _)| dsn.to_string()),
            schema: postgres.map(|(_, schema)| schema.to_string()),
            scheme: Scheme::DEFAULT,
            key: "nils".into(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap()
    }

    /// A caller with these grants, a detail among them.
    fn caller(list: &str) -> Caller {
        let mut access = Access::default();
        for word in list.split(',') {
            match Detail::parse(word) {
                Some(d) => access.detail = d,
                None => access.add(&Access::of_list(word).unwrap()),
            }
        }
        Caller {
            principal: "reader@lab".to_string(),
            access,
            display: None,
            name: None,
            email: None,
            actor: nils_registry::actor::absent(),
            ceiling: None,
            idempotency_key: None,
        }
    }

    /// A registry of a test's own with one cohort, `viewed`, and nothing
    /// in it yet; the directory and, on Postgres, the schema are named
    /// after the test and dropped by [`Bed::done`].
    struct Bed {
        registry: Registry,
        dir: std::path::PathBuf,
        postgres: Option<(String, String)>,
        cohort: i64,
    }

    impl Bed {
        fn new(name: &str, postgres: Option<&str>) -> Bed {
            let dir = std::env::temp_dir().join(format!(
                "nils-viewer-{name}-{}-{}",
                postgres.is_some(),
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let schema = format!("nils_viewer_{name}");
            let mut bed = Bed {
                registry: registry(
                    &dir.join("reg"),
                    postgres.map(|d| {
                        Bed::drop_schemas(d, &schema);
                        (d, schema.as_str())
                    }),
                ),
                dir,
                postgres: postgres.map(|d| (d.to_string(), schema.clone())),
                cohort: 0,
            };
            bed.cohort = nils_registry::cohort::create(
                &mut bed.registry,
                "viewed",
                "owner@lab",
                None,
                "test",
            )
            .unwrap()
            .id;
            bed
        }

        fn drop_schemas(dsn: &str, schema: &str) {
            Store::connect_postgres(dsn, schema)
                .unwrap()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .unwrap();
        }

        fn keys(&self) -> Subkeys {
            Subkeys::derive(&self.registry.pseudonym_key().unwrap())
        }

        /// File these `(identifier, code)` pairs under an id type, which is
        /// added where the store has none of the name; a code that names no
        /// subject yet makes one, and every subject named joins the cohort.
        fn file(&mut self, id_type: &str, rows: &[(String, String)]) {
            let keys = self.keys();
            let mut store = self.registry.open_linkage().unwrap();
            if linkage::id_type_id(&mut store, id_type).unwrap().is_none() {
                linkage::add_id_type(&mut store, id_type, None).unwrap();
            }
            let rows: Vec<linkage::ImportRow> = rows
                .iter()
                .enumerate()
                .map(|(i, (identifier, code))| linkage::ImportRow {
                    line: i + 1,
                    identifier: identifier.clone(),
                    code: code.clone(),
                })
                .collect();
            if let Err(e) =
                linkage::import(self.registry.store(), &mut store, &keys, id_type, &rows)
            {
                panic!("{e}");
            }
            let codes: Vec<String> = rows.iter().map(|r| r.code.clone()).collect();
            let subjects: Vec<i64> = linkage::subjects_by_code(self.registry.store(), &codes)
                .unwrap()
                .iter()
                .map(|s| s.id)
                .collect();
            nils_registry::cohort::join(
                self.registry.store(),
                self.cohort,
                &subjects,
                "test",
                &nils_registry::cohort::Opened {
                    source: "manual",
                    ..Default::default()
                },
            )
            .unwrap();
        }

        /// The cohort's subjects door as this caller asks it.
        fn asked(&mut self, who: &Caller, query: &[(&str, &str)]) -> Result<Value, Reply> {
            let scope = Scope::cohort(&mut self.registry, "viewed")?;
            let query: HashMap<String, String> = query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            subjects(&mut self.registry, who, &scope, &query)
        }

        /// The door's answer, where it must answer.
        fn ask(&mut self, who: &Caller, query: &[(&str, &str)]) -> Value {
            match self.asked(who, query) {
                Ok(answer) => answer,
                Err(refused) => panic!("{query:?}: {} {}", refused.status, refused.body),
            }
        }

        /// The codes of an answer's subjects, in its order.
        fn codes(&mut self, answer: &Value) -> Vec<String> {
            let ids: Vec<i64> = answer["subjects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["id"].as_i64().unwrap())
                .collect();
            let by_id: HashMap<i64, String> = linkage::subjects_by_id(self.registry.store(), &ids)
                .unwrap()
                .into_iter()
                .map(|s| (s.id, s.code))
                .collect();
            ids.iter().map(|id| by_id[id].clone()).collect()
        }

        /// The `linkage.sort` rows of the audit, their details, oldest first.
        fn sorts(&mut self) -> Vec<Value> {
            let store = self.registry.store();
            let audit = store.qualified("audit");
            store
                .query(
                    &format!(
                        "SELECT principal, scope, details FROM {audit} \
                         WHERE action = 'linkage.sort' ORDER BY id"
                    ),
                    &[],
                )
                .unwrap()
                .iter()
                .map(|r| {
                    json!({
                        "principal": r.text(0).unwrap(),
                        "scope": serde_json::from_str::<Value>(r.text(1).unwrap()).unwrap(),
                        "details": serde_json::from_str::<Value>(r.text(2).unwrap()).unwrap(),
                    })
                })
                .collect()
        }

        /// How many values the linkage store's read audit says were opened.
        fn reads(&mut self) -> i64 {
            let mut store = self.registry.open_linkage().unwrap();
            let read_audit = store.qualified("read_audit");
            store
                .query(&format!("SELECT COUNT(*) FROM {read_audit}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap()
        }

        fn done(self) {
            let Bed {
                registry,
                dir,
                postgres,
                ..
            } = self;
            drop(registry);
            if let Some((dsn, schema)) = postgres {
                Bed::drop_schemas(&dsn, &schema);
            }
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(i, c)| (i.to_string(), c.to_string()))
            .collect()
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// The test's Postgres, where the environment names one.
    fn postgres_dsn(test: &str) -> Option<String> {
        let dsn = std::env::var("NILS_TEST_POSTGRES_DSN").ok();
        if dsn.is_none() {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set: {test} skipped");
        }
        dsn
    }

    /// 2026-11-12, `order=id`: subjects in the order of what they are shown
    /// by. Ten subjects of a cohort, eight with a value of a made-up type
    /// (one with two, two whose values differ in case and a leading zero
    /// alone) and two with none.
    fn subjects_are_ordered_by_what_they_are_shown_by(postgres: Option<&str>) {
        let mut bed = Bed::new("order", postgres);
        bed.file(
            "study-id",
            &pairs(&[
                ("S10", "c-01"),
                ("s2", "c-02"),
                ("S3", "c-03"),
                ("12", "c-04"),
                ("7", "c-05"),
                ("a12", "c-06"),
                ("A7", "c-07"),
                ("S02", "c-10"),
            ]),
        );
        // a second value of the third, filed later: the page shows the first
        bed.file("study-id", &pairs(&[("A1", "c-03")]));
        // two that hold no value of the type
        bed.file("site-label", &pairs(&[("zz", "c-08"), ("aa", "c-09")]));
        let by_code = strings(&[
            "c-01", "c-02", "c-03", "c-04", "c-05", "c-06", "c-07", "c-08", "c-09", "c-10",
        ]);
        // 7, 12, A7, a12, s2 and S02 (equal, so by code), S3, S10, then
        // those with none by code
        let by_id = strings(&[
            "c-05", "c-04", "c-07", "c-06", "c-02", "c-10", "c-03", "c-01", "c-08", "c-09",
        ]);
        let reads = caller("data:work,sensitive");
        let shapes = caller("data:work,quasi");

        // shown by the code, the id order is the code's, as `code` is
        for query in [
            vec![("order", "id")],
            vec![("order", "id"), ("show", "code")],
            vec![("order", "code")],
            vec![("order", "code"), ("show", "study-id")],
        ] {
            let d = bed.ask(&reads, &query);
            assert_eq!(bed.codes(&d), by_code, "{query:?}: {d}");
            assert_eq!(d["order"], query[0].1, "{d}");
        }
        assert_eq!(
            bed.sorts(),
            Vec::<Value>::new(),
            "nothing was sorted by a value yet"
        );

        // shown by the type, to a caller who may read its values: their
        // natural order
        let before = bed.reads();
        let d = bed.ask(&reads, &[("order", "id"), ("show", "study-id")]);
        assert_eq!(bed.codes(&d), by_id, "{d}");
        assert_eq!(d["order"], "id", "{d}");
        assert_eq!(d["show"], "study-id", "{d}");
        assert_eq!(d["subjects"][0]["label"], "7", "{d}");
        assert_eq!(d["subjects"][6]["label"], "S3", "{d}");
        assert_eq!(d["subjects"][8]["label"], Value::Null, "{d}");
        assert_eq!(d["next"], Value::Null, "{d}");
        // the audit of that one request: a row for each label of the page,
        // as before, and one row for the sort with no value in it
        assert_eq!(bed.reads() - before, 8);
        let sorts = bed.sorts();
        assert_eq!(sorts.len(), 1, "{sorts:?}");
        assert_eq!(
            sorts[0],
            json!({
                "principal": "reader@lab",
                "scope": {"kind": "cohort", "name": "viewed", "id": bed.cohort, "id_type": "study-id"},
                "details": {"why": WHY_SORTED, "subjects": 10, "opened": 8},
            })
        );

        // every other order keeps its meaning and breaks its ties in the
        // same order: no subject here has a scan, so every one is a tie.
        // The order is kept, so nothing more is opened for it.
        for order in [None, Some("look"), Some("visits"), Some("scans")] {
            let before = bed.reads();
            let mut query = vec![("show", "study-id"), ("limit", "3")];
            if let Some(o) = order {
                query.push(("order", o));
            }
            let d = bed.ask(&reads, &query);
            assert_eq!(bed.codes(&d), by_id[..3], "{order:?}: {d}");
            assert_eq!(d["order"], order.unwrap_or("look"), "{d}");
            assert_eq!(bed.reads() - before, 3, "the page's labels alone");
        }
        let sorts = bed.sorts();
        assert_eq!(sorts.len(), 5, "one row a request: {sorts:?}");
        assert!(
            sorts[1..].iter().all(|s| s["details"]["opened"] == 0),
            "{sorts:?}"
        );

        // a caller who is shown shapes is answered in code order, and
        // nothing is opened or kept for it
        let before = bed.reads();
        for query in [
            vec![("order", "id"), ("show", "study-id")],
            vec![("show", "study-id")],
        ] {
            let d = bed.ask(&shapes, &query);
            assert_eq!(bed.codes(&d), by_code, "{query:?}: {d}");
            assert_eq!(d["subjects"][0]["label"], "A99", "{d}");
            assert_eq!(d["subjects"][4]["label"], "9", "{d}");
            assert!(!d.to_string().contains("S10"), "{d}");
        }
        assert_eq!(bed.reads() - before, 16, "the two pages' labels alone");
        assert_eq!(bed.sorts().len(), 5);

        // a page at a time: every subject once, in the order, and what
        // carries the place is a subject's id, never a value
        let values = ["S10", "s2", "S3", "12", "a12", "A7", "S02", "A1"];
        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut query = vec![("order", "id"), ("show", "study-id"), ("limit", "3")];
            if let Some(a) = &after {
                query.push(("after", a));
            }
            let d = bed.ask(&reads, &query);
            seen.extend(bed.codes(&d));
            let Some(next) = d["next"].as_i64() else {
                assert_eq!(d["next"], Value::Null, "{d}");
                break;
            };
            assert_eq!(d["subjects"][2]["id"], next, "{d}");
            let next = next.to_string();
            assert!(!values.contains(&next.as_str()), "{next}");
            after = Some(next);
        }
        assert_eq!(seen, by_id);
        assert_eq!(bed.sorts().len(), 9, "four pages, four rows");

        // an identity filed moves the order, which is told again
        bed.file("study-id", &pairs(&[("S1", "c-09")]));
        let d = bed.ask(&reads, &[("order", "id"), ("show", "study-id")]);
        let moved = strings(&[
            "c-05", "c-04", "c-07", "c-06", "c-09", "c-02", "c-10", "c-03", "c-01", "c-08",
        ]);
        assert_eq!(bed.codes(&d), moved, "{d}");
        let sorts = bed.sorts();
        assert_eq!(sorts[9]["details"]["opened"], 9, "{sorts:?}");
        // and a search or a filter narrows the same order
        let d = bed.ask(
            &reads,
            &[("order", "id"), ("show", "study-id"), ("q", "c-0")],
        );
        assert_eq!(bed.codes(&d), [&moved[..6], &moved[7..]].concat(), "{d}");

        // an order the door does not know is refused, with the list
        for who in [&reads, &shapes] {
            let Err(refused) = bed.asked(who, &[("order", "nonsense"), ("show", "study-id")])
            else {
                panic!("an unknown order is refused");
            };
            assert_eq!(refused.status, 400);
            assert_eq!(
                refused.body["error"],
                "order: nonsense is not look, code, id, visits or scans"
            );
        }
        assert_eq!(bed.sorts().len(), 11);
        bed.done();
    }

    #[test]
    fn subjects_are_ordered_by_what_they_are_shown_by_on_sqlite() {
        subjects_are_ordered_by_what_they_are_shown_by(None);
    }

    #[test]
    fn subjects_are_ordered_by_what_they_are_shown_by_on_postgres() {
        if let Some(dsn) = postgres_dsn("the order by what subjects are shown by") {
            subjects_are_ordered_by_what_they_are_shown_by(Some(&dsn));
        }
    }

    /// The alias a merge files is a code: its order is told to a caller at
    /// detail quasi, who is shown it as it is, and not below.
    fn the_alias_orders_for_a_caller_who_reads_codes(postgres: Option<&str>) {
        let mut bed = Bed::new("alias", postgres);
        bed.file(
            "study-id",
            &pairs(&[
                ("x1", "c-01"),
                ("x2", "c-02"),
                ("x3", "c-03"),
                ("x4", "c-04"),
            ]),
        );
        let ids: HashMap<String, i64> = linkage::subjects_by_code(
            bed.registry.store(),
            &strings(&["c-01", "c-02", "c-03", "c-04"]),
        )
        .unwrap()
        .into_iter()
        .map(|s| (s.code, s.id))
        .collect();
        let keys = bed.keys();
        let mut store = bed.registry.open_linkage().unwrap();
        let alias = match linkage::id_type_id(&mut store, SUBJECT_CODE_TYPE).unwrap() {
            Some(id) => id,
            None => {
                linkage::add_id_type(&mut store, SUBJECT_CODE_TYPE, None)
                    .unwrap()
                    .id
            }
        };
        let filed = |code: &str, value: &str, source: &'static str| linkage::NewIdentity {
            subject_id: ids[code],
            id_type_id: alias,
            lookup: keys.lookup(SUBJECT_CODE_TYPE, value),
            ciphertext: keys.seal_kept(SUBJECT_CODE_TYPE, value),
            source,
            first_batch_id: None,
        };
        linkage::insert_identities(
            &mut store,
            &[
                filed("c-04", "old-9", linkage::FILED_BY_MERGE),
                filed("c-03", "old-10", linkage::FILED_BY_MERGE),
                // a code filed any other way is no alias
                filed("c-02", "old-1", "csv"),
            ],
        )
        .unwrap();
        drop(store);
        let query = [("order", "id"), ("show", SUBJECT_CODE_TYPE)];
        let d = bed.ask(&caller("data:see,quasi"), &query);
        assert_eq!(
            bed.codes(&d),
            strings(&["c-04", "c-03", "c-01", "c-02"]),
            "{d}"
        );
        assert_eq!(d["subjects"][0]["label"], "old-9", "{d}");
        let sorts = bed.sorts();
        assert_eq!(sorts.len(), 1, "{sorts:?}");
        assert_eq!(sorts[0]["details"]["opened"], 2, "{sorts:?}");
        let d = bed.ask(&caller("data:see,plain"), &query);
        assert_eq!(
            bed.codes(&d),
            strings(&["c-01", "c-02", "c-03", "c-04"]),
            "{d}"
        );
        assert!(!d.to_string().contains("old-"), "{d}");
        assert_eq!(bed.sorts().len(), 1);
        bed.done();
    }

    #[test]
    fn the_alias_orders_for_a_caller_who_reads_codes_on_sqlite() {
        the_alias_orders_for_a_caller_who_reads_codes(None);
    }

    #[test]
    fn the_alias_orders_for_a_caller_who_reads_codes_on_postgres() {
        if let Some(dsn) = postgres_dsn("the order by the alias") {
            the_alias_orders_for_a_caller_who_reads_codes(Some(&dsn));
        }
    }

    /// What an order over a few thousand subjects costs (2026-11-12): the
    /// first request opens a value a subject once and writes one audit row
    /// for the sort; every page after it opens the page's labels alone.
    /// The times are printed, not asserted.
    fn an_order_over_thousands_is_told_once(postgres: Option<&str>) {
        const SUBJECTS: usize = 4000;
        let mut bed = Bed::new("thousands", postgres);
        // values that run against the codes, so the order is not the code's
        let rows: Vec<(String, String)> = (0..SUBJECTS)
            .map(|i| (format!("P{}", SUBJECTS - i), format!("c-{i:05}")))
            .collect();
        bed.file("study-id", &rows);
        let reads = caller("data:work,sensitive");
        let query = [("order", "id"), ("show", "study-id")];
        let asked = |bed: &mut Bed, query: &[(&str, &str)]| {
            let (reads_before, at) = (bed.reads(), Instant::now());
            let d = bed.ask(&reads, query);
            (d, at.elapsed(), bed.reads() - reads_before)
        };
        let (plain, by_code, _) = asked(&mut bed, &[("order", "code"), ("show", "study-id")]);
        assert_eq!(plain["subjects"][0]["code"], "c-00000", "{plain}");
        let (first, cold, read_cold) = asked(&mut bed, &query);
        assert_eq!(first["subjects"][0]["label"], "P1", "{first}");
        assert_eq!(first["subjects"][59]["label"], "P60", "{first}");
        assert_eq!(first["count"], SUBJECTS_PAGE, "{first}");
        let after = first["next"].as_i64().unwrap().to_string();
        let (second, warm, read_warm) = asked(&mut bed, &[query[0], query[1], ("after", &after)]);
        assert_eq!(second["subjects"][0]["label"], "P61", "{second}");
        // the page's labels, one read audit row each, whatever the sort did
        assert_eq!((read_cold, read_warm), (60, 60));
        let sorts = bed.sorts();
        assert_eq!(sorts.len(), 2, "{sorts:?}");
        assert_eq!(sorts[0]["details"]["opened"], SUBJECTS, "{sorts:?}");
        assert_eq!(sorts[0]["details"]["subjects"], SUBJECTS, "{sorts:?}");
        assert_eq!(sorts[1]["details"]["opened"], 0, "{sorts:?}");
        eprintln!(
            "{SUBJECTS} subjects on {}: a page by code {by_code:?}, the first page by id \
             {cold:?} ({SUBJECTS} values opened, 1 sort row, {read_cold} read rows), the \
             next page {warm:?} (0 opened, 1 sort row, {read_warm} read rows)",
            if postgres.is_some() {
                "postgres"
            } else {
                "sqlite"
            },
        );
        bed.done();
    }

    #[test]
    fn an_order_over_thousands_is_told_once_on_sqlite() {
        an_order_over_thousands_is_told_once(None);
    }

    #[test]
    fn an_order_over_thousands_is_told_once_on_postgres() {
        if let Some(dsn) = postgres_dsn("the order over thousands") {
            an_order_over_thousands_is_told_once(Some(&dsn));
        }
    }

    // ---- the subjects door's kept facts (2026-11-12)

    /// When the made-up rows of [`Bed::fill`] were written.
    const THEN: &str = "2026-10-17T00:00:00Z";

    /// An answer but when its facts were read, which no two readings share.
    fn but_when(mut doc: Value) -> Value {
        doc.as_object_mut().unwrap().remove("counts_as_of");
        doc
    }

    /// Let go of a scope's slot as the keep does when it holds too many.
    fn evict(key: &str) {
        let slot = TURNS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .of
            .remove(key)
            .map(|(number, _)| slot_name(key, number));
        let_go(&slot.into_iter().collect::<Vec<_>>());
    }

    impl Bed {
        /// The newest row of a table.
        fn top(&mut self, table: &str) -> i64 {
            let store = self.registry.store();
            let table = store.qualified(table);
            store
                .query(&format!("SELECT COALESCE(MAX(id), 0) FROM {table}"), &[])
                .unwrap()[0]
                .int(0)
                .unwrap()
        }

        fn put(&mut self, table: &str, columns: &str, rows: &[String]) {
            let store = self.registry.store();
            let table = store.qualified(table);
            store
                .execute(
                    &format!("INSERT INTO {table} ({columns}) VALUES {}", rows.join(", ")),
                    &[],
                )
                .unwrap();
        }

        fn change(&mut self, table: &str, set_where: &str) {
            let store = self.registry.store();
            let table = store.qualified(table);
            store
                .execute(&format!("UPDATE {table} SET {set_where}"), &[])
                .unwrap();
        }

        /// Made-up rows for every read of the door. Nine subjects, `w001`
        /// to `w009`; the first eight have two studies of three scans each
        /// (scan `s` is of study `(s + 2) / 3`, study `j` of subject
        /// `(j + 1) / 2`), the ninth has none. A dataset, `ward`, holds the
        /// scans of the first six, another tree those of the sixth to the
        /// eighth. The cohort has every subject but the first, seven of
        /// them filed under the made-up type `ward-id`. There are makers
        /// by study and by scan, body regions, questions that wait and one
        /// decided, sessions for the first four subjects, three picks, and
        /// a sealed sample: one scan of the first subject and every scan of
        /// the fifth.
        fn fill(&mut self) {
            use nils_registry::place;
            assert_eq!(self.top("stack"), 0);
            let roots: Vec<String> = ["ward", "elsewhere"]
                .iter()
                .map(|name| {
                    let folder = self.dir.join(name);
                    std::fs::create_dir_all(&folder).unwrap();
                    std::fs::canonicalize(&folder)
                        .unwrap()
                        .display()
                        .to_string()
                })
                .collect();
            place::add(
                self.registry.store(),
                &place::New {
                    name: "ward",
                    role: place::Role::Source,
                    path: &roots[0],
                    guarantees: json!({}),
                    probed: json!({}),
                    handling: Value::Null,
                    dataset: json!({
                        "kind": "legacy",
                        "arrives": "deidentified",
                        "state": "anonymised",
                        "trees": {"originals": null, "anon": "."},
                        "patient_id": "id-type:patient-id",
                        "subjects": "map",
                    }),
                },
            )
            .unwrap();
            let rows: Vec<String> = roots
                .iter()
                .map(|r| format!("('{r}', '{r}', '{THEN}')"))
                .collect();
            self.put("source", "root, root_canonical, first_seen_at", &rows);
            let elsewhere = self.top("source");
            let ward = elsewhere - 1;
            self.put(
                "job",
                "kind, name, state, started_at, finished_at",
                &[format!("('digest', 'made up', 'done', '{THEN}', '{THEN}')")],
            );
            let job = self.top("job");
            self.put(
                "ingest_batch",
                "source_id, job_id, name, config, started_at, finished_at, state, kind",
                &[format!(
                    "({ward}, {job}, 'b', '{{}}', '{THEN}', '{THEN}', 'done', 'digest')"
                )],
            );
            let batch = self.top("ingest_batch");
            let rows: Vec<String> = (1..=9)
                .map(|i| format!("('w{i:03}', {batch}, '{THEN}')"))
                .collect();
            self.put("subject", "code, first_batch_id, created_at", &rows);
            // the third subject's two studies are of one day
            let rows: Vec<String> = (1..=16)
                .map(|j| {
                    let subject = (j + 1) / 2;
                    let day = if j % 2 == 0 && subject != 3 {
                        format!("2021-04-{subject:02}")
                    } else {
                        format!("2020-03-{subject:02}")
                    };
                    let maker = match subject % 3 {
                        0 => "'SIEMENS'",
                        1 => "'GE MEDICAL SYSTEMS'",
                        _ => "NULL",
                    };
                    format!("('1.2.3.{j}', {subject}, '{day}', {maker}, {batch})")
                })
                .collect();
            self.put(
                "study",
                "study_instance_uid, subject_id, study_date, manufacturer, first_batch_id",
                &rows,
            );
            let study_of = |s: i64| (s + 2) / 3;
            let subject_of = |s: i64| (study_of(s) + 1) / 2;
            let rows: Vec<String> = (1..=48)
                .map(|s| {
                    format!(
                        "('1.2.4.{s}', {}, {}, 20, 1, {batch})",
                        study_of(s),
                        subject_of(s)
                    )
                })
                .collect();
            self.put(
                "series",
                "series_instance_uid, study_id, subject_id, n_instances, n_stacks, first_batch_id",
                &rows,
            );
            let rows: Vec<String> = (1..=48)
                .map(|s| format!("({s}, 0, 'k', 'MR', 'ax', 20, {batch})"))
                .collect();
            self.put(
                "stack",
                "series_id, stack_index, stack_key, modality, orientation, n_instances, \
                 first_batch_id",
                &rows,
            );
            let rows: Vec<String> = (1..=48)
                .flat_map(|s| {
                    let mut held = Vec::new();
                    if subject_of(s) <= 6 {
                        held.push(format!("({ward}, {s}, '{THEN}')"));
                    }
                    if subject_of(s) >= 6 {
                        held.push(format!("({elsewhere}, {s}, '{THEN}')"));
                    }
                    held
                })
                .collect();
            self.put("source_stack", "source_id, stack_id, first_seen_at", &rows);
            // every other scan says its own maker or none
            let rows: Vec<String> = (1..=48)
                .filter(|s| s % 2 == 0)
                .map(|s| {
                    let maker = if s % 4 == 0 {
                        "'Philips Medical Systems'"
                    } else {
                        "NULL"
                    };
                    format!(
                        "({s}, {s}, {}, {}, 'MR', 'ax', 20, 0, 1, {maker}, 't1 sag', {job}, 1)",
                        study_of(s),
                        subject_of(s)
                    )
                })
                .collect();
            self.put(
                "stack_fingerprint",
                "stack_id, series_id, study_id, subject_id, modality, orientation, n_instances, \
                 stack_index, stacks_in_series, manufacturer, text_series_description, job_id, \
                 epoch",
                &rows,
            );
            let rows: Vec<String> = (1..=48)
                .map(|s| format!("({s}, 'mri', '1.0.0', 1, {job}, 1, 0)"))
                .collect();
            self.put(
                "classification",
                "stack_id, pack, pack_version, contract, job_id, epoch, review_items",
                &rows,
            );
            let rows: Vec<String> = (1..=48)
                .filter(|s| s % 7 != 0)
                .map(|s| {
                    let region = if s % 3 == 0 {
                        "spine"
                    } else if s % 5 == 0 {
                        "brain-neck"
                    } else {
                        "brain"
                    };
                    format!("({s}, 'body_part', '{region}', 0.9, 'rule')")
                })
                .collect();
            self.put(
                "classification_axis",
                "stack_id, axis, value, confidence, tier",
                &rows,
            );
            // the questions: about scans 4, 20 and 40, a staged one about
            // scan 10, a decided one about scan 11, and one about three
            let rows: Vec<String> = [(4, "open"), (20, "open"), (40, "open"), (10, "staged")]
                .iter()
                .map(|(s, status)| {
                    format!(
                        "('base:low_confidence', 'stack', '{{\"stack_id\": {s}}}', '{status}', \
                         '{THEN}')"
                    )
                })
                .collect();
            self.put("review_item", "kind, scope, ref, status, created_at", &rows);
            self.put(
                "review_item",
                "kind, scope, ref, status, created_at, decided_at",
                &[format!(
                    "('base:low_confidence', 'stack', '{{\"stack_id\": 11}}', 'decided', \
                     '{THEN}', '{THEN}')"
                )],
            );
            self.put(
                "review_item",
                "kind, scope, ref, status, created_at, members",
                &[format!(
                    "('base:missing', 'group', '{{}}', 'open', '{THEN}', 3)"
                )],
            );
            let group = self.top("review_item");
            let rows: Vec<String> = [5, 6, 22]
                .iter()
                .map(|s| format!("({group}, {s})"))
                .collect();
            self.put("review_member", "item_id, stack_id", &rows);
            // the sessions: one a study of the first two subjects, one of
            // both studies of the third, and the fourth's first study alone
            let sessions = [
                (1, "2020-03-01", 1),
                (1, "2021-04-01", 1),
                (2, "2020-03-02", 1),
                (2, "2021-04-02", 1),
                (3, "2020-03-03", 2),
                (4, "2020-03-04", 1),
            ];
            let rows: Vec<String> = sessions
                .iter()
                .map(|(subject, day, n)| {
                    format!("({subject}, 0, 'd', '{day}', '{day}', {n}, 1, '{THEN}')")
                })
                .collect();
            self.put(
                "session_cache",
                "subject_id, window_days, timeline_digest, first, last, n_studies, epoch, built_at",
                &rows,
            );
            let first = self.top("session_cache") - 5;
            let rows: Vec<String> = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (4, 6), (5, 7)]
                .iter()
                .map(|(session, study)| format!("({}, {study}, 0)", first + session))
                .collect();
            self.put(
                "session_cache_study",
                "session_id, study_id, window_days",
                &rows,
            );
            for (role, subject, day, scan) in [
                ("t1w", 1, "2020-03-01", 1),
                ("flair", 2, "2021-04-02", 10),
                ("t1w", 7, "2020-03-07", 37),
            ] {
                self.pick(role, subject, day, scan);
            }
            let rows: Vec<String> = [4, 25, 26, 27, 28, 29, 30]
                .iter()
                .map(|s| format!("('made-up', {s}, {}, 'someone', '{THEN}')", subject_of(*s)))
                .collect();
            self.put(
                "sealed_stack",
                "sample, stack_id, subject_id, sealed_by, sealed_at",
                &rows,
            );
            self.file(
                "ward-id",
                &pairs(&[
                    ("N-207", "w002"),
                    ("N-31", "w003"),
                    ("N-4", "w004"),
                    ("N-055", "w005"),
                    ("N-6", "w006"),
                    ("N-70", "w007"),
                    ("N-9", "w009"),
                ]),
            );
            self.join(8);
        }

        fn pick(&mut self, role: &str, subject: i64, day: &str, scan: i64) {
            self.put(
                "pick",
                "model, role, subject_id, session_day, scheme, reference, pack, pack_version, \
                 actor, author_kind, decided_at",
                &[format!(
                    "('main', '{role}', {subject}, '{day}', 'default', 'x', 'mri', '1.0.0', \
                     'engine', 'rules', '{THEN}')"
                )],
            );
            let pick = self.top("pick");
            self.put(
                "pick_stack",
                "pick_id, stack_id",
                &[format!("({pick}, {scan})")],
            );
        }

        fn join(&mut self, subject: i64) {
            nils_registry::cohort::join(
                self.registry.store(),
                self.cohort,
                &[subject],
                "test",
                &nils_registry::cohort::Opened {
                    source: "manual",
                    ..Default::default()
                },
            )
            .unwrap();
        }

        /// A scan more of a study, read into the dataset by a batch of its
        /// own.
        fn arrives(&mut self, study: i64) {
            let subject = (study + 1) / 2;
            let (ward, job) = (self.top("source") - 1, self.top("job"));
            self.put(
                "job",
                "kind, name, state, started_at, finished_at",
                &[format!("('digest', 'more', 'done', '{THEN}', '{THEN}')")],
            );
            self.put(
                "ingest_batch",
                "source_id, job_id, name, config, started_at, finished_at, state, kind",
                &[format!(
                    "({ward}, {}, 'more', '{{}}', '{THEN}', '{THEN}', 'done', 'digest')",
                    job + 1
                )],
            );
            let batch = self.top("ingest_batch");
            let scan = self.top("stack") + 1;
            self.put(
                "series",
                "series_instance_uid, study_id, subject_id, n_instances, n_stacks, first_batch_id",
                &[format!(
                    "('1.2.4.{scan}', {study}, {subject}, 20, 1, {batch})"
                )],
            );
            let series = self.top("series");
            self.put(
                "stack",
                "series_id, stack_index, stack_key, modality, orientation, n_instances, \
                 first_batch_id",
                &[format!("({series}, 0, 'k', 'MR', 'ax', 20, {batch})")],
            );
            self.put(
                "source_stack",
                "source_id, stack_id, first_seen_at",
                &[format!("({ward}, {scan}, '{THEN}')")],
            );
        }

        fn scope(&mut self, kind: &str) -> Scope {
            let scope = match kind {
                "dataset" => Scope::dataset(&mut self.registry, "ward"),
                "cohort" => Scope::cohort(&mut self.registry, "viewed"),
                _ => Scope::registry(&mut self.registry),
            };
            match scope {
                Ok(scope) => scope,
                Err(refused) => panic!("{kind}: {} {}", refused.status, refused.body),
            }
        }

        /// The door of a scope as this caller asks it, answered from what
        /// is kept.
        fn door(&mut self, kind: &str, who: &Caller, query: &[(&str, &str)]) -> Value {
            let scope = self.scope(kind);
            let query: HashMap<String, String> = query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            match subjects(&mut self.registry, who, &scope, &query) {
                Ok(answer) => answer,
                Err(refused) => panic!("{query:?}: {} {}", refused.status, refused.body),
            }
        }

        /// The same answer read from the registry now, with nothing kept.
        fn front(&mut self, kind: &str, who: &Caller, query: &[(&str, &str)]) -> Value {
            let scope = self.scope(kind);
            let query: HashMap<String, String> = query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let read = (|| {
                let asking = Asking::parse(&query)?;
                let shown = shown_by(&mut self.registry, query.get("show"))?;
                in_front(&mut self.registry, who, &scope, &asking, shown.as_ref())
            })();
            match read {
                Ok(answer) => answer,
                Err(refused) => panic!("{query:?}: {} {}", refused.status, refused.body),
            }
        }

        fn key(&mut self, kind: &str, unsealed: bool) -> String {
            let scope = self.scope(kind);
            key_of(&self.registry, &scope, unsealed)
        }

        /// The slot that keeps a scope's facts for one kind of caller.
        fn slot(&mut self, kind: &str, unsealed: bool) -> String {
            slot_of(&self.key(kind, unsealed)).0
        }

        fn stamp(&mut self, kind: &str) -> String {
            let scope = self.scope(kind);
            match stamp_of(&mut self.registry, &scope) {
                Ok((stamp, busy)) => {
                    assert!(!busy, "{stamp}");
                    stamp
                }
                Err(refused) => panic!("{} {}", refused.status, refused.body),
            }
        }
    }

    const SCOPES: [&str; 3] = ["dataset", "cohort", "registry"];

    /// What the tests ask the door: each order, by code and by a shown
    /// type, each filter, a search by code and one by an identifier, and a
    /// short page.
    const ASKED: [&[(&str, &str)]; 17] = [
        &[],
        &[("order", "code")],
        &[("order", "id")],
        &[("order", "visits")],
        &[("order", "scans")],
        &[("order", "look"), ("show", "ward-id")],
        &[("order", "id"), ("show", "ward-id")],
        &[("order", "code"), ("show", "ward-id")],
        &[("order", "scans"), ("show", "ward-id"), ("limit", "3")],
        &[("filter", "look")],
        &[("filter", "visits2,region:spine")],
        &[("filter", "maker:philips,maker:siemens"), ("order", "code")],
        &[("filter", "main:t1w")],
        &[("q", "w00")],
        &[("q", "N-207"), ("show", "ward-id")],
        &[("q", "nobody")],
        &[("limit", "2"), ("order", "visits")],
    ];

    /// The keep answers what a read in front answers: whole documents, for
    /// a dataset, a cohort and the registry, for callers at each detail and
    /// with each right, every page of a walk with `after` among them.
    fn the_keep_answers_what_a_read_in_front_answers(postgres: Option<&str>) {
        let mut bed = Bed::new("kept", postgres);
        bed.fill();
        let callers = [
            caller("data:work,sensitive"),
            caller("data:work,sensitive,sealed:see"),
            caller("data:see,quasi"),
            caller("data:see,plain"),
        ];
        for kind in SCOPES {
            for who in &callers {
                for query in ASKED {
                    let front = bed.front(kind, who, query);
                    assert_eq!(front["counting"], false, "{front}");
                    let kept = bed.door(kind, who, query);
                    assert_eq!(kept["counting"], false, "{kept}");
                    assert!(kept["counts_as_of"].is_string(), "{kept}");
                    assert_eq!(but_when(kept), but_when(front), "{kind} {query:?}");
                }
                // a walk to the end, two at a time
                let mut after: Option<String> = None;
                let mut walked = 0;
                loop {
                    let mut query = vec![("limit", "2"), ("order", "scans")];
                    if let Some(a) = &after {
                        query.push(("after", a));
                    }
                    let front = bed.front(kind, who, &query);
                    let kept = bed.door(kind, who, &query);
                    assert_eq!(but_when(kept.clone()), but_when(front), "{kind} {query:?}");
                    walked += kept["count"].as_u64().unwrap();
                    match kept["next"].as_i64() {
                        Some(next) => after = Some(next.to_string()),
                        None => break,
                    }
                }
                assert_eq!(walked, bed.door(kind, who, &[])["matched"], "{kind}");
            }
            // every caller was answered from two readings, one for each
            // sealed variant
            for unsealed in [false, true] {
                let slot = bed.slot(kind, unsealed);
                assert_eq!(KEPT.reads_of(&slot), 1, "{slot}");
            }
        }
        // the fixture is what the tests below say it is
        let reader = caller("data:work,sensitive,sealed:see");
        let d = bed.door("dataset", &reader, &[]);
        assert_eq!(
            d["totals"],
            json!({"subjects": 6, "visits": 11, "scans": 36, "look": 6}),
            "{d}"
        );
        let c = bed.door("cohort", &reader, &[]);
        assert_eq!(c["totals"]["subjects"], 8, "{c}");
        assert_eq!(c["totals"]["scans"], 42, "{c}");
        assert_eq!(c["facets"]["id_types"][0]["subjects"], 7, "{c}");
        let r = bed.door("registry", &reader, &[]);
        assert_eq!(r["totals"]["scans"], 48, "{r}");
        bed.done();
    }

    #[test]
    fn the_keep_answers_what_a_read_in_front_answers_on_sqlite() {
        the_keep_answers_what_a_read_in_front_answers(None);
    }

    #[test]
    fn the_keep_answers_what_a_read_in_front_answers_on_postgres() {
        if let Some(dsn) = postgres_dsn("the kept subjects door") {
            the_keep_answers_what_a_read_in_front_answers(Some(&dsn));
        }
    }

    /// The stacks of sealed samples are in the facts of a caller who may
    /// read them and of no one else: each kind of caller has a slot of its
    /// own, whoever asked first. Everything else of a caller is applied to
    /// the kept facts and one slot serves them all.
    #[test]
    fn a_caller_is_never_answered_facts_read_under_wider_access() {
        let mut bed = Bed::new("variants", None);
        bed.fill();
        let wide = caller("data:work,sensitive,sealed:see");
        let narrow = caller("data:work,sensitive");
        for wide_first in [true, false] {
            for kind in SCOPES {
                for unsealed in [false, true] {
                    evict(&bed.key(kind, unsealed));
                }
                let (all, hidden) = if wide_first {
                    let all = bed.door(kind, &wide, &[]);
                    (all, bed.door(kind, &narrow, &[]))
                } else {
                    let hidden = bed.door(kind, &narrow, &[]);
                    (bed.door(kind, &wide, &[]), hidden)
                };
                // seven scans are sealed: every scan of one subject, who
                // is a member of the cohort still, and one that asks a
                // question, of a subject the cohort does not have
                let n = |d: &Value, what: &str| d["totals"][what].as_i64().unwrap();
                let apart = i64::from(kind != "cohort");
                assert_eq!(n(&all, "scans") - n(&hidden, "scans"), 6 + apart, "{kind}");
                assert_eq!(n(&all, "look") - n(&hidden, "look"), apart, "{kind}");
                assert_eq!(n(&all, "subjects") - n(&hidden, "subjects"), apart);
                assert_eq!(
                    but_when(hidden.clone()),
                    but_when(bed.front(kind, &narrow, &[])),
                    "{kind}"
                );
                assert_eq!(
                    but_when(all.clone()),
                    but_when(bed.front(kind, &wide, &[])),
                    "{kind}"
                );
                // each from a slot of its own, read once, and again
                let (a, h) = (bed.slot(kind, true), bed.slot(kind, false));
                assert_ne!(a, h);
                assert_eq!(but_when(bed.door(kind, &narrow, &[])), but_when(hidden));
                assert_eq!(but_when(bed.door(kind, &wide, &[])), but_when(all));
                assert_eq!((KEPT.reads_of(&a), KEPT.reads_of(&h)), (1, 1), "{kind}");
            }
        }

        // the detail, the right to read identifiers and the shape of a
        // code are a request's own: the hidden slot serves all three
        let quasi = caller("data:see,quasi");
        let plain = caller("data:see,plain");
        let shown = [("show", "ward-id"), ("order", "id")];
        let told_narrow = bed.door("cohort", &narrow, &shown);
        let told_quasi = bed.door("cohort", &quasi, &shown);
        let told_plain = bed.door("cohort", &plain, &shown);
        let slot = bed.slot("cohort", false);
        assert_eq!(KEPT.reads_of(&slot), 1);
        for (who, told) in [(&quasi, &told_quasi), (&plain, &told_plain)] {
            assert_eq!(
                but_when(told.clone()),
                but_when(bed.front("cohort", who, &shown))
            );
            assert!(!told.to_string().contains("N-207"), "{told}");
        }
        assert!(told_narrow.to_string().contains("N-207"), "{told_narrow}");
        assert!(told_quasi.to_string().contains("w002"), "{told_quasi}");
        assert!(!told_plain.to_string().contains("w002"), "{told_plain}");
        assert_eq!(told_plain["detail"], "plain");
        // and an identifier finds a subject for the caller who reads one
        let found = bed.door("cohort", &narrow, &[("q", "N-207")]);
        assert_eq!(found["matched"], 1, "{found}");
        let found = bed.door("cohort", &quasi, &[("q", "N-207")]);
        assert_eq!(found["matched"], 0, "{found}");
        assert_eq!(KEPT.reads_of(&slot), 1);
        bed.done();
    }

    /// One write of a kind the facts depend on.
    struct Write {
        what: &'static str,
        /// The scopes whose stamp it must move.
        moves: &'static [&'static str],
        write: fn(&mut Bed),
    }

    const WRITES: [Write; 10] = [
        Write {
            what: "a new scan",
            moves: &SCOPES,
            write: |bed| bed.arrives(3),
        },
        Write {
            what: "a decision on a question that waited",
            moves: &SCOPES,
            write: |bed| {
                bed.change(
                    "review_item",
                    &format!("status = 'decided', decided_at = '{THEN}' WHERE id = 2"),
                )
            },
        },
        Write {
            what: "a pick",
            moves: &SCOPES,
            write: |bed| bed.pick("swi", 4, "2020-03-04", 21),
        },
        Write {
            what: "a pick withdrawn",
            moves: &SCOPES,
            write: |bed| {
                // as the engine withdraws a person's pick: no row is
                // written but the audit's
                bed.change("pick", &format!("withdrawn_at = '{THEN}' WHERE id = 2"));
                nils_registry::audit::record(
                    &mut bed.registry,
                    &nils_registry::audit::Entry {
                        principal: "someone@lab",
                        action: nils_registry::audit::Action::PickWithdraw,
                        scope: json!({"pick": 2, "restored": []}),
                        policy: None,
                        job_id: None,
                        details: None,
                    },
                )
                .unwrap();
            },
        },
        Write {
            what: "a cohort member added",
            moves: &["cohort"],
            write: |bed| bed.join(1),
        },
        Write {
            what: "a cohort member who left",
            moves: &["cohort"],
            write: |bed| {
                bed.change(
                    "cohort_member",
                    &format!("left_at = '{THEN}' WHERE subject_id = 9"),
                )
            },
        },
        Write {
            what: "a sample sealed",
            moves: &SCOPES,
            write: |bed| {
                bed.put(
                    "sealed_stack",
                    "sample, stack_id, subject_id, sealed_by, sealed_at",
                    &[format!("('more', 14, 3, 'someone', '{THEN}')")],
                )
            },
        },
        Write {
            what: "a sealed scan unsealed",
            moves: &SCOPES,
            write: |bed| {
                bed.change(
                    "sealed_stack",
                    &format!("unsealed_at = '{THEN}' WHERE stack_id = 4"),
                )
            },
        },
        Write {
            what: "an identifier filed",
            moves: &SCOPES,
            write: |bed| bed.file("ward-id", &pairs(&[("N-1", "w001")])),
        },
        Write {
            what: "a question staged, which waits as it did",
            moves: &[],
            write: |bed| bed.change("review_item", "status = 'staged' WHERE id = 3"),
        },
    ];

    /// The stamp moves on each kind of write the facts depend on. The
    /// answer after it is the kept one and says `counting`, the facts are
    /// read once apart, and the next answer is what a read in front says.
    /// Where a write leaves a scope's stamp, the kept facts are still what
    /// a read in front says.
    fn a_stale_answer_says_counting_and_the_next_says_the_new_facts(postgres: Option<&str>) {
        let mut bed = Bed::new("stale", postgres);
        bed.fill();
        let callers = [
            caller("data:work,sensitive"),
            caller("data:work,sensitive,sealed:see"),
        ];
        let query = [("show", "ward-id"), ("order", "look")];
        for kind in SCOPES {
            for who in &callers {
                bed.door(kind, who, &query);
            }
        }
        // asking moves nothing, an order by a shown type and its audit row
        // neither
        for kind in SCOPES {
            let before = bed.stamp(kind);
            bed.door(kind, &callers[0], &[("show", "ward-id"), ("order", "id")]);
            assert_eq!(bed.stamp(kind), before, "{kind}");
        }
        for w in &WRITES {
            let mut before = Vec::new();
            for kind in SCOPES {
                let stamp = bed.stamp(kind);
                for who in &callers {
                    let kept = bed.door(kind, who, &query);
                    assert_eq!(kept["counting"], false, "{}: {kept}", w.what);
                    before.push((kind, stamp.clone(), who, kept));
                }
            }
            (w.write)(&mut bed);
            let mut changed = 0;
            for (kind, stamp, who, was) in before {
                let unsealed = crate::sealed::reads(&who.access);
                let slot = bed.slot(kind, unsealed);
                let reads = KEPT.reads_of(&slot);
                let moved = bed.stamp(kind) != stamp;
                assert!(moved || !w.moves.contains(&kind), "{}: {kind}", w.what);
                let front = bed.front(kind, who, &query);
                let stale = bed.door(kind, who, &query);
                assert_eq!(stale["counting"], moved, "{}: {kind}", w.what);
                if moved {
                    // the facts kept, as they were and from when they were
                    for fact in ["totals", "facets", "matched", "counts_as_of"] {
                        assert_eq!(stale[fact], was[fact], "{}: {kind} {fact}", w.what);
                    }
                    // however many ask meanwhile, one reading
                    for _ in 0..3 {
                        bed.door(kind, who, &query);
                    }
                    KEPT.settled(&slot);
                    assert_eq!(KEPT.reads_of(&slot), reads + 1, "{}: {kind}", w.what);
                }
                let next = bed.door(kind, who, &query);
                assert_eq!(next["counting"], false, "{}: {kind}", w.what);
                assert_eq!(
                    but_when(next.clone()),
                    but_when(front),
                    "{}: {kind}",
                    w.what
                );
                if moved {
                    // read again, to the second
                    assert!(
                        next["counts_as_of"].as_str() >= was["counts_as_of"].as_str(),
                        "{}: {next}",
                        w.what
                    );
                } else {
                    assert_eq!(next, was, "{}: {kind}", w.what);
                    assert_eq!(KEPT.reads_of(&slot), reads, "{}: {kind}", w.what);
                }
                changed += usize::from(but_when(next) != but_when(was));
            }
            // each write but the last is one the door's answer shows
            assert_eq!(changed > 0, !w.moves.is_empty(), "{}", w.what);
        }
        bed.done();
    }

    #[test]
    fn a_stale_answer_says_counting_and_the_next_says_the_new_facts_on_sqlite() {
        a_stale_answer_says_counting_and_the_next_says_the_new_facts(None);
    }

    #[test]
    fn a_stale_answer_says_counting_and_the_next_says_the_new_facts_on_postgres() {
        if let Some(dsn) = postgres_dsn("the subjects door after a write") {
            a_stale_answer_says_counting_and_the_next_says_the_new_facts(Some(&dsn));
        }
    }

    /// A scope nothing is kept of is read once, whoever asks at the same
    /// moment: one request reads in front and the others wait for it.
    #[test]
    fn a_scope_is_read_once_for_all_who_ask_at_once() {
        let mut bed = Bed::new("once", None);
        bed.fill();
        let home = bed.registry.home().clone();
        let asking: Vec<_> = (0..4)
            .map(|_| {
                let home = home.clone();
                std::thread::spawn(move || {
                    let mut registry = home.open().unwrap();
                    let scope = match Scope::dataset(&mut registry, "ward") {
                        Ok(scope) => scope,
                        Err(refused) => panic!("{} {}", refused.status, refused.body),
                    };
                    let who = caller("data:work,sensitive");
                    match subjects(&mut registry, &who, &scope, &HashMap::new()) {
                        Ok(answer) => answer,
                        Err(refused) => panic!("{} {}", refused.status, refused.body),
                    }
                })
            })
            .collect();
        let answers: Vec<Value> = asking.into_iter().map(|a| a.join().unwrap()).collect();
        let slot = bed.slot("dataset", false);
        assert_eq!(KEPT.reads_of(&slot), 1);
        for answer in &answers {
            assert_eq!(answer, &answers[0]);
            assert_eq!(answer["counting"], false, "{answer}");
            assert_eq!(answer["totals"]["scans"], 29, "{answer}");
        }
        // and nothing was written for the next start to restore
        assert_eq!(
            nils_registry::home::get_meta(bed.registry.store(), NEVER_STORED).unwrap(),
            None
        );
        bed.done();
    }

    /// The keep holds so many scopes: one more and the one asked for
    /// longest ago goes, never the one asked for now.
    #[test]
    fn the_keep_lets_go_of_the_scope_asked_for_longest_ago() {
        let mut turns = Turns::default();
        let mut slots = Vec::new();
        for i in 0..HELD_MOST {
            let key = format!("scope-{i}");
            let (number, out) = turns.turn(&key, HELD_MOST);
            assert!(out.is_empty(), "{out:?}");
            slots.push(slot_name(&key, number));
        }
        // asked for again, the first is the newest and keeps its slot
        let (number, out) = turns.turn("scope-0", HELD_MOST);
        assert_eq!(slot_name("scope-0", number), slots[0]);
        assert!(out.is_empty(), "{out:?}");
        let (_, out) = turns.turn("one-more", HELD_MOST);
        assert_eq!(out, vec![slots[1].clone()]);
        assert!(turns.holds("scope-0", number));
        assert_eq!(turns.of.len(), HELD_MOST);
        // the one let go of has a new slot when it is asked for again, and
        // the next oldest goes for it
        let (again, out) = turns.turn("scope-1", HELD_MOST);
        assert_ne!(slot_name("scope-1", again), slots[1]);
        assert_eq!(out, vec![slots[2].clone()]);
        // a keep of one holds the one asked for
        let (number, out) = turns.turn("scope-9", 1);
        assert_eq!(out.len(), HELD_MOST - 1);
        assert!(turns.holds("scope-9", number));
        assert_eq!(turns.of.len(), 1);
    }

    /// A scope the keep let go of holds nothing: a reading of it under way
    /// keeps nothing, and the next request reads it again under a new slot.
    #[test]
    fn a_scope_let_go_of_is_read_again() {
        let mut bed = Bed::new("let-go", None);
        bed.fill();
        let who = caller("data:work,sensitive");
        let first = bed.door("dataset", &who, &[]);
        let key = bed.key("dataset", false);
        let (slot, number) = slot_of(&key);
        let mut under_way = Gathering {
            scope: bed.scope("dataset"),
            unsealed: false,
            key: key.clone(),
            number,
        };
        evict(&key);
        match under_way.read(&mut bed.registry) {
            Ok(held) => assert!(held.gone && held.facts.is_empty()),
            Err(refused) => panic!("{} {}", refused.status, refused.body),
        }
        let again = bed.door("dataset", &who, &[]);
        assert_eq!(again["counting"], false, "{again}");
        assert_eq!(but_when(again), but_when(first));
        let new = bed.slot("dataset", false);
        assert_ne!(new, slot);
        assert_eq!((KEPT.reads_of(&slot), KEPT.reads_of(&new)), (1, 1));
        bed.done();
    }

    /// What one kept scope of 10,000 subjects weighs, each with a code of
    /// twelve letters, three regions, two makers and two roles: about
    /// 3.5 MB, so sixteen of them under 60 MB.
    #[test]
    fn a_kept_scope_weighs_what_is_said() {
        let regions: BTreeSet<String> = strings(&["brain", "neck", "spine"]).into_iter().collect();
        let facts: BTreeMap<i64, Facts> = (1..=10_000i64)
            .map(|i| {
                (
                    i,
                    Facts {
                        code: format!("{:012x}", i * 7919),
                        scans: 90,
                        look: 3,
                        visits: 5,
                        regions: sorted_regions(&regions),
                        makers: strings(&["Philips", "Siemens"]),
                        main: strings(&["flair", "t1w"]),
                    },
                )
            })
            .collect();
        let held = Held {
            facts,
            ..Held::default()
        };
        let weight = held.weight();
        assert!((3_000_000..4_000_000).contains(&weight), "{weight}");
    }
}
