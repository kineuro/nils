// SPDX-License-Identifier: AGPL-3.0-only

//! The sessions of NILS (`docs/specs/wave4b-the-ask.md`, §7): one resolver
//! over each subject's whole timeline, never the selection you look through,
//! cached so that a question, a release, a picker and the command line read
//! the same rows.
//!
//! Identity is a function of the scheme's window alone: grouping runs first
//! and everything else in a scheme (anchor, naming, collision, unmatched, the
//! source's own label) only names what grouping made. So `session_cache`
//! holds identity keyed by (subject, window) with a surrogate id, and
//! `session_label` holds the names a scheme gives those sessions, keyed by
//! the scheme's digest. A label tweak never rebuilds identity.
//!
//! The rebuild unit is the subject, through a digest of its timeline: an
//! epoch bump rebuilds only the subjects whose studies or dates changed. A
//! rebuild diffs old spans against new. A session whose first day moved keeps
//! its surrogate, its stored picks are re-keyed to the new day, and a review
//! item says so; a session that is gone withdraws its picks and says so too.
//! Nothing drops silently.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use nils_registry::clinical;
use nils_registry::day::Day;
use nils_registry::dialect::Conflict;
use nils_registry::lock;
use nils_registry::review;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Anchor, Scheme, Session, Study};
use nils_registry::store::Error as StoreError;
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param, Registry, Row, Store};
use serde::{Deserialize, Serialize};

pub use nils_registry::session::{Collision, Naming, Reason, Said, Unmatched};

/// The review item kind a moved or vanished span raises.
pub const MOVED_KIND: &str = "session.moved";

/// The columns of a label row, and those an insert that meets the row of
/// its key writes over.
const LABEL_COLUMNS: [&str; 8] = [
    "session_id",
    "scheme_digest",
    "label",
    "months",
    "nominal",
    "offset_months",
    "flagged",
    "reason",
];
const LABEL_SET: [&str; 6] = [
    "label",
    "months",
    "nominal",
    "offset_months",
    "flagged",
    "reason",
];

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    Message(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

// ---------------------------------------------------------------- anchors

/// Month zero per subject, by code, for the anchors that come from outside
/// the timeline: an explicit CSV, or the earliest event of a kind.
#[derive(Debug, Clone, Default)]
pub struct Anchors {
    pub explicit: BTreeMap<String, Day>,
    pub events: BTreeMap<String, Day>,
}

impl Anchors {
    /// The anchors a scheme needs, from the registry: none for a first
    /// session or a source label, the earliest event of the kind for an
    /// event anchor, and the explicit map handed in.
    pub fn resolve(
        registry: &mut Registry,
        scheme: &Scheme,
        explicit: BTreeMap<String, Day>,
    ) -> Result<Anchors, Error> {
        let mut out = Anchors {
            explicit,
            events: BTreeMap::new(),
        };
        if scheme.anchor == Anchor::Event {
            let Some(name) = &scheme.event else {
                return Err(Error::Message(
                    "anchor `event` needs session.event, the kind month zero is".into(),
                ));
            };
            let store = registry.store();
            let kind = clinical::kind_named(store, name)?.ok_or_else(|| {
                Error::Message(format!(
                    "session.event names {name}, which is not an observation kind the registry holds; load the vocabulary, or name one of its kinds"
                ))
            })?;
            out.events = clinical::anchor_events(store, kind.id)?
                .into_iter()
                .collect();
        }
        Ok(out)
    }

    /// `code,date` rows, a header allowed.
    pub fn parse_csv(text: &str) -> Result<BTreeMap<String, Day>, Error> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());
        let mut out = BTreeMap::new();
        for (i, record) in reader.records().enumerate() {
            let record =
                record.map_err(|e| Error::Message(format!("anchors line {}: {e}", i + 1)))?;
            let (Some(code), Some(date)) = (record.get(0), record.get(1)) else {
                return Err(Error::Message(format!(
                    "anchors line {}: a row is `code,date`",
                    i + 1
                )));
            };
            let code = code.trim();
            if i == 0 && code.eq_ignore_ascii_case("code") {
                continue;
            }
            let Some(day) = Day::parse(date) else {
                return Err(Error::Message(format!(
                    "anchors line {}: {date} is not a date",
                    i + 1
                )));
            };
            out.insert(code.to_string(), day);
        }
        Ok(out)
    }

    fn of(&self, scheme: &Scheme, code: &str, studies: &[Study]) -> Option<Day> {
        match scheme.anchor {
            Anchor::FirstSession => studies.iter().map(|s| s.day).min(),
            Anchor::Explicit => self.explicit.get(code).copied(),
            Anchor::Event => self.events.get(code).copied(),
            // resolved from the labels inside the resolver
            Anchor::SourceLabel => None,
        }
    }
}

// ---------------------------------------------------------------- the cache

/// One cached session, with the label the scheme asked for gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub id: i64,
    pub subject_id: i64,
    pub code: String,
    pub window_days: i64,
    pub first: Day,
    pub last: Day,
    pub studies: Vec<i64>,
    pub label: Option<String>,
    pub months: Option<i32>,
    pub nominal: Option<i32>,
    pub offset_months: Option<f64>,
    pub flagged: bool,
    pub reason: Option<String>,
    /// Whether any study of the session holds an original primary: true if
    /// any does, null if any has not said, else false.
    pub has_primary: Option<bool>,
}

impl Cached {
    /// The name a release directory or a pick key uses: the label, else the
    /// day the session opened, which is what `keep_date` means.
    pub fn name(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.first.compact())
    }
}

/// A study's session under a scheme, for a reader that starts from studies.
#[derive(Debug, Clone, PartialEq)]
pub struct Labelled {
    pub session_id: i64,
    pub subject_id: i64,
    pub first: Day,
    pub label: Option<String>,
}

impl Labelled {
    pub fn name(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.first.compact())
    }
}

/// What a rebuild did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rebuilt {
    pub subjects: usize,
    pub unchanged: usize,
    pub rebuilt: usize,
    pub sessions: usize,
    /// Sessions whose label row under the scheme the run wrote: made,
    /// changed or taken away. None where the rows already said what the
    /// scheme says, so a read of a cache nothing changed counts none.
    pub relabelled: usize,
    pub moved: usize,
    pub vanished: usize,
    pub picks_rekeyed: usize,
    pub picks_withdrawn: usize,
    pub items: usize,
    /// Study rows that hold no series, so no session was made from them,
    /// and each of which is an open question (§7, record 35 finding 7).
    #[serde(default)]
    pub empty_studies: usize,
    /// Subjects that own series and no study, so they are on no timeline.
    #[serde(default)]
    pub without_a_study: usize,
    /// The epoch the build moved the registry to, where it wrote a session
    /// or a label: an answer kept before it was made from other sessions
    /// and is not this epoch's answer. None where it wrote neither.
    #[serde(skip)]
    pub epoch: Option<i64>,
}

struct Point {
    subject_id: i64,
    code: String,
    study: Study,
}

/// Every dated study that holds something, whole timeline per subject, with
/// the source's own label when the scheme reads one. A study whose date the
/// vote could not settle is left out: it is not a point on a timeline.
///
/// A study row with no series is left out too. A session is the occasion a
/// subject came in, and a study that holds no series is not one: the row
/// survives where the series that declared it were filed under another
/// study, and a session made from it carries no series, no stacks and the
/// same labels as a real one. The row itself stays, because it is a fact in
/// a file; `ensure` says it is there and empty. The branch that reads the
/// source's own label always asked for a series, by joining through one, so
/// the two ways of reading a timeline used to disagree about which studies
/// are on it.
/// Whose timelines a run of [`ensure`] reads: every subject's, one
/// subject's by code, or some subjects' by id.
#[derive(Clone, Copy)]
enum Whose<'a> {
    All,
    Code(&'a str),
    Ids(&'a [i64]),
}

/// How many subject ids one query of [`points`] names.
const IDS_CHUNK: usize = 500;

fn points(store: &mut Store, scheme: &Scheme, whose: Whose<'_>) -> Result<Vec<Point>, Error> {
    if let Whose::Ids(ids) = whose
        && ids.len() > IDS_CHUNK
    {
        let mut out = Vec::new();
        for chunk in ids.chunks(IDS_CHUNK) {
            out.extend(points(store, scheme, Whose::Ids(chunk))?);
        }
        return Ok(out);
    }
    let d = store.dialect();
    let (study_t, subject_t) = (store.qualified("study"), store.qualified("subject"));
    let st = table("study");
    let filled = d.text_of_qualified(Some("st"), st.column("date_filled").expect("date_filled"));
    let dated = d.text_of_qualified(Some("st"), st.column("study_date").expect("study_date"));
    let mut params: Vec<Param> = Vec::new();
    let mut where_subject = String::new();
    match whose {
        Whose::All => {}
        Whose::Code(code) => {
            where_subject = format!(" AND su.code = {}", d.param(1, Type::Text));
            params.push(Param::from(code));
        }
        Whose::Ids([]) => return Ok(Vec::new()),
        Whose::Ids(ids) => {
            let list = ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            where_subject = format!(" AND su.id IN ({list})");
        }
    }
    let said = match &scheme.said {
        Some(spec) => Some((
            spec.segment,
            match &spec.pattern {
                Some(p) => Some(
                    regex::Regex::new(p)
                        .map_err(|e| Error::Message(format!("session.said.pattern: {e}")))?,
                ),
                None => None,
            },
        )),
        None => None,
    };
    let sql = if said.is_some() {
        format!(
            "SELECT su.id, su.code, st.id, COALESCE({filled}, {dated}), MAX(st.has_original_primary), MIN(sf.path) \
             FROM {study_t} st JOIN {subject_t} su ON su.id = st.subject_id \
             JOIN {series} se ON se.study_id = st.id \
             JOIN {instance} i ON i.series_id = se.id \
             JOIN {source_file} sf ON sf.instance_id = i.id \
             WHERE COALESCE({filled}, {dated}) IS NOT NULL{where_subject} \
             GROUP BY su.id, su.code, st.id, COALESCE({filled}, {dated}) \
             ORDER BY su.id, 4, st.id",
            series = store.qualified("series"),
            instance = store.qualified("instance"),
            source_file = store.qualified("source_file"),
        )
    } else {
        format!(
            "SELECT su.id, su.code, st.id, COALESCE({filled}, {dated}), st.has_original_primary, CAST(NULL AS TEXT) \
             FROM {study_t} st JOIN {subject_t} su ON su.id = st.subject_id \
             WHERE COALESCE({filled}, {dated}) IS NOT NULL{where_subject} \
             AND EXISTS (SELECT 1 FROM {series} se WHERE se.study_id = st.id) \
             ORDER BY su.id, 4, st.id",
            series = store.qualified("series"),
        )
    };
    let mut out = Vec::new();
    for r in store.query(&sql, &params)? {
        let Some(day) = Day::parse(r.text(3)?) else {
            continue;
        };
        let path = r.opt_text(5)?.unwrap_or("");
        out.push(Point {
            subject_id: r.int(0)?,
            code: r.text(1)?.to_string(),
            study: Study {
                id: r.int(2)?,
                day,
                said: said
                    .as_ref()
                    .and_then(|(segment, pattern)| label_in(path, *segment, pattern.as_ref())),
                // Null is not no: a study whose stacks are not all
                // fingerprinted has not said it holds no primary.
                has_primary: r.opt_int(4)?.map(|v| v != 0),
            },
        });
    }
    Ok(out)
}

/// The source's own label, out of one segment of a path. The filename is
/// dropped first, as the identity rule drops it: a segment number counts
/// directories, so that adding a file does not shift it.
pub fn label_in(path: &str, segment: usize, pattern: Option<&regex::Regex>) -> Option<String> {
    let dirs: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    let text = dirs.get(segment.checked_sub(1)?)?;
    match pattern {
        None => Some((*text).to_string()),
        Some(re) => Some(re.captures(text)?.name("label")?.as_str().to_string()),
    }
}

/// The digest of a subject's timeline: every dated study, in order.
fn timeline_digest(studies: &[Study]) -> String {
    use blake2::digest::consts::U16;
    use blake2::{Blake2b, Digest};
    let mut hasher = Blake2b::<U16>::new();
    let mut sorted: Vec<&Study> = studies.iter().collect();
    sorted.sort_by_key(|s| (s.day, s.id));
    for s in sorted {
        hasher.update(format!("{}\t{}\n", s.id, s.day.compact()).as_bytes());
    }
    hex::encode(hasher.finalize())
}

struct Stored {
    id: i64,
    first: Day,
    digest: String,
    studies: Vec<i64>,
}

fn stored(store: &mut Store, subject: i64, window: i64) -> Result<Vec<Stored>, Error> {
    let d = store.dialect();
    let t = table("session_cache");
    let first = d.text_of_qualified(Some("c"), t.column("first").expect("first"));
    let sql = format!(
        "SELECT c.id, {first}, c.timeline_digest FROM {} c WHERE c.subject_id = {} AND c.window_days = {} ORDER BY c.id",
        store.qualified("session_cache"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(subject), Param::Int(window)])? {
        out.push(Stored {
            id: r.int(0)?,
            first: Day::parse(r.text(1)?).ok_or_else(|| Error::Message("a cached day".into()))?,
            digest: r.text(2)?.to_string(),
            studies: Vec::new(),
        });
    }
    if out.is_empty() {
        return Ok(out);
    }
    let members = format!(
        "SELECT cs.session_id, cs.study_id FROM {} cs JOIN {} c ON c.id = cs.session_id \
         WHERE c.subject_id = {} AND cs.window_days = {} ORDER BY cs.study_id",
        store.qualified("session_cache_study"),
        store.qualified("session_cache"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    for r in store.query(&members, &[Param::Int(subject), Param::Int(window)])? {
        let sid = r.int(0)?;
        if let Some(s) = out.iter_mut().find(|s| s.id == sid) {
            s.studies.push(r.int(1)?);
        }
    }
    Ok(out)
}

/// Build or refresh the cache under a scheme: identity for every subject
/// whose timeline changed (or every subject, when forced), and the labels
/// the scheme gives, for one subject or for all. Runs in one transaction,
/// one build at a time, and writes only what changed: a run over timelines
/// and labels that did not change writes nothing.
pub fn ensure(
    registry: &mut Registry,
    scheme: &Scheme,
    anchors: &Anchors,
    only: Option<&str>,
    force: bool,
) -> Result<Rebuilt, Error> {
    let whose = match only {
        Some(code) => Whose::Code(code),
        None => Whose::All,
    };
    ensure_of(registry, scheme, Some(anchors), whose, force)
}

/// [`ensure`] for some subjects by id: the cache brought up to date for
/// their timelines alone, which is what a reader of a dataset's or a
/// cohort's visits needs before it reads them (2026-10-12: a dataset read
/// and sorted with nothing since that built the cache had no visits on its
/// Main scans map). A subject whose timeline and labels are unchanged costs
/// a few reads and no write, so the doors that read visits can call it on
/// every read, several at once.
pub fn ensure_for(
    registry: &mut Registry,
    scheme: &Scheme,
    anchors: &Anchors,
    subjects: &BTreeSet<i64>,
) -> Result<Rebuilt, Error> {
    let ids: Vec<i64> = subjects.iter().copied().collect();
    ensure_of(registry, scheme, Some(anchors), Whose::Ids(&ids), false)
}

/// What [`stale`] found: the subjects whose cache does not say what their
/// timeline says, among how many timelines, and the sessions of the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stale {
    pub subjects: BTreeSet<i64>,
    pub timelines: usize,
    pub sessions: usize,
}

/// Every stored session of a window, by subject, with its studies in the
/// order [`stored`] reads them: two statements for the whole registry.
fn stored_all(store: &mut Store, window: i64) -> Result<HashMap<i64, Vec<Stored>>, Error> {
    let d = store.dialect();
    let t = table("session_cache");
    let first = d.text_of_qualified(Some("c"), t.column("first").expect("first"));
    let sql = format!(
        "SELECT c.id, c.subject_id, {first}, c.timeline_digest FROM {} c \
         WHERE c.window_days = {} ORDER BY c.id",
        store.qualified("session_cache"),
        d.param(1, Type::Int)
    );
    let mut out: HashMap<i64, Vec<Stored>> = HashMap::new();
    // where a session sits, to hand it its studies
    let mut at: HashMap<i64, (i64, usize)> = HashMap::new();
    for r in store.query(&sql, &[Param::Int(window)])? {
        let (id, subject) = (r.int(0)?, r.int(1)?);
        let of = out.entry(subject).or_default();
        at.insert(id, (subject, of.len()));
        of.push(Stored {
            id,
            first: Day::parse(r.text(2)?).ok_or_else(|| Error::Message("a cached day".into()))?,
            digest: r.text(3)?.to_string(),
            studies: Vec::new(),
        });
    }
    if out.is_empty() {
        return Ok(out);
    }
    let members = format!(
        "SELECT cs.session_id, cs.study_id FROM {} cs WHERE cs.window_days = {} \
         ORDER BY cs.study_id",
        store.qualified("session_cache_study"),
        d.param(1, Type::Int)
    );
    for r in store.query(&members, &[Param::Int(window)])? {
        if let Some((subject, i)) = at.get(&r.int(0)?)
            && let Some(s) = out.get_mut(subject).and_then(|of| of.get_mut(*i))
        {
            s.studies.push(r.int(1)?);
        }
    }
    Ok(out)
}

/// Every label row under a scheme of a window's sessions, by session: one
/// statement for the whole registry, where [`labels_of`] reads a subject's.
fn labels_all(
    store: &mut Store,
    window: i64,
    scheme_digest: &str,
) -> Result<HashMap<i64, Label>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT l.session_id, l.label, l.months, l.nominal, l.offset_months, l.flagged, l.reason \
         FROM {label} l JOIN {cache} c ON c.id = l.session_id \
         WHERE c.window_days = {} AND l.scheme_digest = {}",
        d.param(1, Type::Int),
        d.param(2, Type::Text),
        label = store.qualified("session_label"),
        cache = store.qualified("session_cache"),
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[Param::Int(window), Param::from(scheme_digest)])? {
        out.insert(r.int(0)?, Label::read(&r)?);
    }
    Ok(out)
}

/// The subjects a build under this scheme would write something for, found
/// by reading alone: no transaction, no lock, no write. A subject is stale
/// when it has a timeline and no session, when a session of its was built
/// from another timeline (a study came, went, moved to or from another
/// subject, or changed its day), and, with anchors, when a label row under
/// the scheme is missing or says something else than the resolver.
///
/// It reads what [`ensure`] reads and decides as it decides, in a handful
/// of statements for the whole registry where a build asks three of every
/// subject, so a reader can afford it on every request and call a build
/// only for what it names. Without anchors the labels are not looked at.
pub fn stale(
    store: &mut Store,
    scheme: &Scheme,
    anchors: Option<&Anchors>,
) -> Result<Stale, Error> {
    let window = scheme.window_days;
    let digest = scheme.digest();
    let mut by_subject: BTreeMap<i64, (String, Vec<Study>)> = BTreeMap::new();
    for p in points(store, scheme, Whose::All)? {
        let e = by_subject
            .entry(p.subject_id)
            .or_insert_with(|| (p.code.clone(), Vec::new()));
        e.1.push(p.study);
    }
    let mut cached = stored_all(store, window)?;
    let mut labels = match anchors {
        Some(_) => labels_all(store, window, &digest)?,
        None => HashMap::new(),
    };
    let mut out = Stale {
        timelines: by_subject.len(),
        ..Stale::default()
    };
    for (subject, (code, studies)) in &by_subject {
        let old = cached.remove(subject).unwrap_or_default();
        let timeline = timeline_digest(studies);
        if old.is_empty() || old.iter().any(|s| s.digest != timeline) {
            out.subjects.insert(*subject);
            continue;
        }
        if let Some(anchors) = anchors {
            let resolved = session::sessions(studies, anchors.of(scheme, code, studies), scheme);
            let ids: Vec<i64> = old.iter().map(|s| s.id).collect();
            let mut held: HashMap<i64, Label> = ids
                .iter()
                .filter_map(|id| labels.remove(id).map(|l| (*id, l)))
                .collect();
            let (rows, gone) = label_changes(&ids, &old, &resolved, &mut held, &digest);
            if !rows.is_empty() || !gone.is_empty() {
                out.subjects.insert(*subject);
                continue;
            }
        }
        out.sessions += old.len();
    }
    Ok(out)
}

/// The `registry_meta` row a [`refresh`] keeps per scheme: what the
/// registry looked like when the cache was last found up to date.
fn fresh_key(scheme: &Scheme) -> String {
    format!("sessions_fresh.{}.{}", scheme.window_days, scheme.digest())
}

/// What moves when a timeline may have: the epoch, which every digest
/// batch, merge and repair moves with its rows, and the studies themselves
/// (how many, how many dated, the newest, whose they are), since a date the
/// vote fills when a run ends puts a study on a timeline and moves no
/// epoch. One read of a key and one pass over the studies.
fn timelines_mark(store: &mut Store) -> Result<(i64, String), Error> {
    let epoch = nils_registry::home::get_meta(store, "epoch")?
        .and_then(|e| e.parse::<i64>().ok())
        .unwrap_or(0);
    let sql = format!(
        "SELECT COUNT(*), COUNT(COALESCE(st.date_filled, st.study_date)), COALESCE(MAX(st.id), 0), \
         CAST(COALESCE(SUM(st.subject_id), 0) AS BIGINT) FROM {} st",
        store.qualified("study")
    );
    let rows = store.query(&sql, &[])?;
    let r = rows
        .first()
        .ok_or_else(|| Error::Message("no count of the studies".into()))?;
    Ok((
        epoch,
        format!("{}.{}.{}.{}", r.int(0)?, r.int(1)?, r.int(2)?, r.int(3)?),
    ))
}

/// What moves when a window's sessions do: how many there are and the
/// newest. A merge takes rows away and a build makes new ones.
fn cache_mark(store: &mut Store, window: i64) -> Result<String, Error> {
    let sql = format!(
        "SELECT COUNT(*), COALESCE(MAX(c.id), 0) FROM {} c WHERE c.window_days = {}",
        store.qualified("session_cache"),
        store.dialect().param(1, Type::Int)
    );
    let rows = store.query(&sql, &[Param::Int(window)])?;
    let r = rows
        .first()
        .ok_or_else(|| Error::Message("no count of the sessions".into()))?;
    Ok(format!("{}.{}", r.int(0)?, r.int(1)?))
}

/// The cache brought up to date for a reader that reads it on every
/// request and has only the scheme (2026-11-27: a question read the cache
/// as it stood, and a subject whose rows a merge had dropped had no session
/// in any answer until some other reader built them).
///
/// Two steps, the first one cheap enough for every request. It holds the
/// registry as it is (the epoch, a count of the studies, a count of the
/// window's sessions: [`timelines_mark`], [`cache_mark`]) against what it
/// was when the cache was last found up to date under this scheme, kept in
/// `registry_meta`. Where the two are the same nothing that makes or
/// unmakes a session has happened, and the cache is read as it is. Where
/// they differ, or nothing is kept, [`stale`] reads every timeline and the
/// whole cache and finds the subjects a build would write for, a build is
/// run for them alone, and the mark is kept again.
///
/// What the first step does not see is a change that moves neither the
/// epoch nor those counts: a study's date changed to another date, or a
/// label row taken away, by something that is not the engine. Both are
/// found by the second step the next time the registry moves, and by a
/// rebuild (`ensure`) at once. A subject with studies and no session is
/// always seen, since its sessions are missing from the count.
///
/// The anchors are the ones the registry gives. A scheme whose month zero
/// comes from outside it (an explicit anchor, whose file only a rebuild is
/// handed) or cannot be resolved has its identity brought up to date and
/// its labels left as the build that knew the anchors wrote them.
pub fn refresh(registry: &mut Registry, scheme: &Scheme) -> Result<Rebuilt, Error> {
    let key = fresh_key(scheme);
    // read before the timelines are, so that a write that lands while they
    // are read leaves a mark that differs from the one kept here
    let (epoch, studies) = timelines_mark(registry.store())?;
    let kept = nils_registry::home::get_meta(registry.store(), &key)?;
    if let Some(kept) = &kept {
        let now = format!(
            "{epoch}.{studies}.{}",
            cache_mark(registry.store(), scheme.window_days)?
        );
        let mut parts = kept.split(' ');
        if parts.next() == Some(now.as_str()) {
            let mut count = || parts.next().and_then(|n| n.parse::<usize>().ok());
            let (subjects, sessions) = (count().unwrap_or(0), count().unwrap_or(0));
            return Ok(Rebuilt {
                subjects,
                unchanged: subjects,
                sessions,
                ..Rebuilt::default()
            });
        }
    }
    let anchors = anchors_known(registry, scheme)?;
    let found = stale(registry.store(), scheme, anchors.as_ref())?;
    let fresh = found.timelines - found.subjects.len();
    let done = if found.subjects.is_empty() {
        Rebuilt {
            subjects: fresh,
            unchanged: fresh,
            sessions: found.sessions,
            ..Rebuilt::default()
        }
    } else {
        // the epoch a build writes on its rows is the registry's, not the
        // one this connection read when it opened
        registry
            .refresh_meta()
            .map_err(|e| Error::Message(e.to_string()))?;
        let ids: Vec<i64> = found.subjects.iter().copied().collect();
        let mut done = ensure_of(registry, scheme, anchors.as_ref(), Whose::Ids(&ids), false)?;
        done.subjects += fresh;
        done.unchanged += fresh;
        done.sessions += found.sessions;
        done
    };
    // A build that wrote moved the epoch itself. Where the epoch it wrote
    // is the next after the one read above, nothing else moved it, and the
    // mark is kept under it; where another writer came between, the mark
    // keeps the epoch read above, differs at the next read, and the whole
    // pass runs once more.
    let epoch = match done.epoch {
        Some(moved) if moved == epoch + 1 => moved,
        _ => epoch,
    };
    let mark = format!(
        "{epoch}.{studies}.{} {} {}",
        cache_mark(registry.store(), scheme.window_days)?,
        done.subjects,
        done.sessions
    );
    if kept.as_deref() != Some(mark.as_str()) {
        // a mark that cannot be kept costs the next reader the second step
        // again, and nothing else
        let _ = nils_registry::home::set_meta(registry.store(), &key, &mark);
    }
    Ok(done)
}

/// The anchors a caller that has only the scheme can know: the registry's
/// own, and none for an explicit anchor or one that does not resolve.
fn anchors_known(registry: &mut Registry, scheme: &Scheme) -> Result<Option<Anchors>, Error> {
    if scheme.anchor == Anchor::Explicit {
        return Ok(None);
    }
    match Anchors::resolve(registry, scheme, BTreeMap::new()) {
        Ok(a) => Ok(Some(a)),
        Err(Error::Message(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The cache built for some subjects by id, by a writer that has only the
/// scheme: what a merge's caller runs once the merge has committed, since
/// the merge drops both subjects' sessions and the subject that is left
/// has none until they are built. Anchors as [`refresh`] knows them.
pub fn refresh_for(
    registry: &mut Registry,
    scheme: &Scheme,
    subjects: &BTreeSet<i64>,
) -> Result<Rebuilt, Error> {
    let anchors = anchors_known(registry, scheme)?;
    let ids: Vec<i64> = subjects.iter().copied().collect();
    ensure_of(registry, scheme, anchors.as_ref(), Whose::Ids(&ids), false)
}

/// A subject with a timeline and no session in a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sessionless {
    pub subject_id: i64,
    pub code: String,
    /// The dated studies that hold series: what a build makes sessions of.
    pub studies: i64,
}

/// The subjects that hold a study a session is made of and have no session
/// in a scheme's window: no question at session grain finds them. A cache
/// is in that state between a write to the timelines and the next build,
/// so some are no fault; ones that stay are.
pub fn without_sessions(store: &mut Store, scheme: &Scheme) -> Result<Vec<Sessionless>, Error> {
    let d = store.dialect();
    let st = table("study");
    let filled = d.text_of_qualified(Some("st"), st.column("date_filled").expect("date_filled"));
    let dated = d.text_of_qualified(Some("st"), st.column("study_date").expect("study_date"));
    let sql = format!(
        "SELECT su.id, su.code, COUNT(*) FROM {study} st JOIN {subject} su ON su.id = st.subject_id \
         WHERE COALESCE({filled}, {dated}) IS NOT NULL \
         AND EXISTS (SELECT 1 FROM {series} se WHERE se.study_id = st.id) \
         AND NOT EXISTS (SELECT 1 FROM {cache} c WHERE c.subject_id = su.id AND c.window_days = {}) \
         GROUP BY su.id, su.code ORDER BY su.code",
        d.param(1, Type::Int),
        study = store.qualified("study"),
        subject = store.qualified("subject"),
        series = store.qualified("series"),
        cache = store.qualified("session_cache"),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(scheme.window_days)])? {
        out.push(Sessionless {
            subject_id: r.int(0)?,
            code: r.text(1)?.to_string(),
            studies: r.int(2)?,
        });
    }
    Ok(out)
}

/// The build itself. Without anchors (`None`) it brings identity up to date
/// and leaves every label row as it is: what a reader that cannot know a
/// scheme's month zero does, since labels written from no anchor would take
/// away the ones a build that knew it wrote.
fn ensure_of(
    registry: &mut Registry,
    scheme: &Scheme,
    anchors: Option<&Anchors>,
    whose: Whose<'_>,
    force: bool,
) -> Result<Rebuilt, Error> {
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let digest = scheme.digest();
    let window = scheme.window_days;
    let now = now_iso();
    let mut out = Rebuilt::default();
    store.begin()?;
    let result = (|| -> Result<(), Error> {
        // One build at a time (2026-10-14). A reader brings the cache up to
        // date before it reads, and a page sends several reads at once; on
        // Postgres, under read committed, two builds side by side each
        // wrote the rows the other was writing, and the second failed on a
        // unique key. The second now waits here until the first commits,
        // then reads the timelines and the cache as the first left them,
        // and finds nothing to do. SQLite's writers take turns already. A
        // merge of two subjects takes the same lock.
        lock::take_for_transaction(store, lock::SESSION_CACHE)?;
        let mut by_subject: BTreeMap<i64, (String, Vec<Study>)> = BTreeMap::new();
        for p in points(store, scheme, whose)? {
            let e = by_subject
                .entry(p.subject_id)
                .or_insert_with(|| (p.code.clone(), Vec::new()));
            e.1.push(p.study);
        }
        for (subject, (code, studies)) in &by_subject {
            out.subjects += 1;
            let timeline = timeline_digest(studies);
            let old = stored(store, *subject, window)?;
            let unchanged = !force && !old.is_empty() && old.iter().all(|s| s.digest == timeline);
            let anchor = anchors.and_then(|a| a.of(scheme, code, studies));
            let resolved = session::sessions(studies, anchor, scheme);
            if unchanged {
                out.unchanged += 1;
                out.sessions += old.len();
                // the labels under this scheme may still be missing, or say
                // what another month zero made of them
                if anchors.is_some() {
                    let ids: Vec<i64> = old.iter().map(|s| s.id).collect();
                    out.relabelled +=
                        relabel(store, *subject, window, &ids, &old, &resolved, &digest)?;
                }
                continue;
            }
            out.rebuilt += 1;
            let ids = reconcile(
                store, *subject, code, window, &old, &resolved, &timeline, epoch, &now, &digest,
                &mut out,
            )?;
            out.sessions += ids.len();
            if anchors.is_some() {
                let fresh = stored(store, *subject, window)?;
                out.relabelled +=
                    relabel(store, *subject, window, &ids, &fresh, &resolved, &digest)?;
            }
        }
        // What the timeline could not take, said rather than dropped. Over
        // the whole registry, whichever subject was asked for: an empty row
        // and a subject off every timeline are facts about the registry, and
        // a run that named them only while rebuilding their own subject
        // would never name the subject that has no session to rebuild.
        let (empty, orphans) = structural(store, &now)?;
        out.empty_studies = empty;
        out.without_a_study = orphans;
        out.items += empty + orphans;
        // An answer kept at this epoch was made from the sessions as they
        // stood. Where the build made, moved or took away a session, or
        // wrote a label, that answer is no longer the one a run would
        // give, so the epoch moves with the rows, in their transaction; a
        // build that found everything as it should be moves nothing.
        if out.rebuilt > 0 || out.relabelled > 0 {
            out.epoch = Some(nils_registry::home::next_epoch_in(store)?);
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            store.commit()?;
            if out.epoch.is_some() {
                registry
                    .refresh_meta()
                    .map_err(|e| Error::Message(e.to_string()))?;
            }
            Ok(out)
        }
        Err(e) => {
            store.rollback().ok();
            Err(e)
        }
    }
}

/// The two shapes identity can end in that no session can hold, said as
/// review items: a study row with no series, which used to become a session
/// with nothing in it, and a subject that owns series and no study, which
/// gets no session at all and appears in no ask. Both are re-derived from
/// the tags by every digest, so neither is repaired here: the rows are what
/// the files said, and a run that quietly rewrote them would be deciding
/// which patient identifier of a study is the right one. What a run does
/// decide is that neither becomes a session.
///
/// One open item per study and per subject, brought up to date by a later
/// run and closed as soon as the row is whole again. Answers how many of
/// each there are.
fn structural(store: &mut Store, now: &str) -> Result<(usize, usize), Error> {
    let d = store.dialect();
    let st = table("study");
    let filled = d.text_of_qualified(Some("st"), st.column("date_filled").expect("date_filled"));
    let dated = d.text_of_qualified(Some("st"), st.column("study_date").expect("study_date"));
    let sql = format!(
        "SELECT st.id, su.id, su.code, COALESCE({filled}, {dated}) \
         FROM {study} st JOIN {subject} su ON su.id = st.subject_id \
         WHERE su.merged_into IS NULL \
         AND NOT EXISTS (SELECT 1 FROM {series} se WHERE se.study_id = st.id) \
         ORDER BY st.id",
        study = store.qualified("study"),
        subject = store.qualified("subject"),
        series = store.qualified("series"),
    );
    let rows = store.query(&sql, &[])?;
    let mut empty = Vec::new();
    for r in &rows {
        empty.push((
            r.int(0)?,
            r.int(1)?,
            r.text(2)?.to_string(),
            r.opt_text(3)?.map(str::to_string),
        ));
    }
    let sql = format!(
        "SELECT su.id, su.code, COUNT(se.id), CAST(COALESCE(SUM(se.n_instances), 0) AS BIGINT) \
         FROM {subject} su JOIN {series} se ON se.subject_id = su.id \
         WHERE su.merged_into IS NULL \
         AND NOT EXISTS (SELECT 1 FROM {study} st WHERE st.subject_id = su.id) \
         GROUP BY su.id, su.code ORDER BY su.id",
        study = store.qualified("study"),
        subject = store.qualified("subject"),
        series = store.qualified("series"),
    );
    let rows = store.query(&sql, &[])?;
    let mut orphans = Vec::new();
    for r in &rows {
        orphans.push((r.int(0)?, r.text(1)?.to_string(), r.int(2)?, r.int(3)?));
    }

    let mut keys = Vec::with_capacity(empty.len());
    for (study, subject, code, day) in &empty {
        review::raise_empty_study(store, *study, *subject, code, day.as_deref(), now)?;
        keys.push(review::empty_study_key(*study));
    }
    let said = serde_json::json!({"by": "session rebuild", "why": "the study holds series again"});
    review::close_resolved(store, review::EMPTY_STUDY_KIND, &keys, "session", &said)?;

    let mut keys = Vec::with_capacity(orphans.len());
    for (subject, code, series, instances) in &orphans {
        review::raise_no_study(store, *subject, code, *series, *instances, now)?;
        keys.push(review::no_study_key(*subject));
    }
    let said = serde_json::json!({"by": "session rebuild", "why": "the subject has a study"});
    review::close_resolved(store, review::NO_STUDY_KIND, &keys, "session", &said)?;
    Ok((empty.len(), orphans.len()))
}

/// Identity: match the resolver's sessions against the stored ones by the
/// studies they share, keep the surrogate where a session survives, re-key
/// or withdraw the picks of a span that moved or vanished, and say so.
#[allow(clippy::too_many_arguments)]
fn reconcile(
    store: &mut Store,
    subject: i64,
    code: &str,
    window: i64,
    old: &[Stored],
    resolved: &[Session],
    timeline: &str,
    epoch: i64,
    now: &str,
    scheme_digest: &str,
    out: &mut Rebuilt,
) -> Result<Vec<i64>, Error> {
    let cache_t = table("session_cache");
    let study_t = table("session_cache_study");
    let mut used: HashSet<i64> = HashSet::new();
    let mut ids = Vec::with_capacity(resolved.len());
    let insert = Insert::new(
        cache_t,
        &[
            "subject_id",
            "window_days",
            "timeline_digest",
            "first",
            "last",
            "n_studies",
            "epoch",
            "built_at",
        ],
    )
    .returning(&["id"]);
    let member = Insert::new(study_t, &["session_id", "study_id", "window_days"]);
    for s in resolved {
        // the stored session holding this one's first study, else the one
        // sharing the most studies
        let first_study = s.studies.first().copied();
        let mut best: Option<(&Stored, usize)> = None;
        for o in old.iter().filter(|o| !used.contains(&o.id)) {
            let shared = o.studies.iter().filter(|x| s.studies.contains(x)).count();
            if shared == 0 {
                continue;
            }
            let holds_first = first_study.is_some_and(|f| o.studies.contains(&f));
            let better = match best {
                None => true,
                Some((b, n)) => {
                    let b_first = first_study.is_some_and(|f| b.studies.contains(&f));
                    (holds_first && !b_first) || (holds_first == b_first && shared > n)
                }
            };
            if better {
                best = Some((o, shared));
            }
        }
        let id = match best {
            Some((o, _)) => {
                used.insert(o.id);
                if o.first != s.first {
                    out.moved += 1;
                    let rekeyed =
                        rekey_picks(store, subject, o.first, s.first, scheme_digest, now)?;
                    out.picks_rekeyed += rekeyed;
                    raise(
                        store,
                        subject,
                        code,
                        window,
                        Some(o.first),
                        Some(s.first),
                        &s.studies,
                        rekeyed,
                        0,
                        now,
                    )?;
                    out.items += 1;
                }
                store.update_by_id(
                    cache_t,
                    &[
                        ("timeline_digest", Param::from(timeline)),
                        ("first", Param::from(iso(s.first))),
                        ("last", Param::from(iso(s.last))),
                        ("n_studies", Param::Int(s.studies.len() as i64)),
                        ("epoch", Param::Int(epoch)),
                        ("built_at", Param::from(now)),
                    ],
                    "id",
                    o.id,
                )?;
                let d = store.dialect();
                store.execute(
                    &format!(
                        "DELETE FROM {} WHERE session_id = {}",
                        store.qualified("session_cache_study"),
                        d.param(1, Type::Int)
                    ),
                    &[Param::Int(o.id)],
                )?;
                o.id
            }
            None => {
                let rows = store.insert(
                    &insert,
                    &[vec![
                        Param::Int(subject),
                        Param::Int(window),
                        Param::from(timeline),
                        Param::from(iso(s.first)),
                        Param::from(iso(s.last)),
                        Param::Int(s.studies.len() as i64),
                        Param::Int(epoch),
                        Param::from(now),
                    ]],
                )?;
                rows.first()
                    .ok_or_else(|| Error::Message("no id for a session".into()))?
                    .int(0)?
            }
        };
        let rows: Vec<Vec<Param>> = s
            .studies
            .iter()
            .map(|st| vec![Param::Int(id), Param::Int(*st), Param::Int(window)])
            .collect();
        if !rows.is_empty() {
            store.insert(&member, &rows)?;
        }
        ids.push(id);
    }
    // sessions that are gone
    for o in old.iter().filter(|o| !used.contains(&o.id)) {
        out.vanished += 1;
        let withdrawn = withdraw_picks(store, subject, o.first, scheme_digest, now)?;
        out.picks_withdrawn += withdrawn;
        raise(
            store,
            subject,
            code,
            window,
            Some(o.first),
            None,
            &o.studies,
            0,
            withdrawn,
            now,
        )?;
        out.items += 1;
        let d = store.dialect();
        for t in ["session_label", "session_cache_study"] {
            store.execute(
                &format!(
                    "DELETE FROM {} WHERE session_id = {}",
                    store.qualified(t),
                    d.param(1, Type::Int)
                ),
                &[Param::Int(o.id)],
            )?;
        }
        store.execute(
            &format!(
                "DELETE FROM {} WHERE id = {}",
                store.qualified("session_cache"),
                d.param(1, Type::Int)
            ),
            &[Param::Int(o.id)],
        )?;
    }
    Ok(ids)
}

/// What a scheme calls one session, as its `session_label` row says it.
#[derive(Debug, Clone)]
struct Label {
    label: Option<String>,
    months: Option<i64>,
    nominal: Option<i64>,
    offset_months: Option<f64>,
    flagged: i64,
    reason: Option<String>,
}

impl Label {
    fn of(s: &Session) -> Label {
        Label {
            label: s.label.clone(),
            months: s.months.map(i64::from),
            nominal: s.nominal.map(i64::from),
            offset_months: s.offset_months,
            flagged: i64::from(s.flagged),
            reason: s.reason.map(|r| r.name().to_string()),
        }
    }

    /// A label row as [`labels_of`] and [`labels_all`] select it: the
    /// session first, then the six columns.
    fn read(r: &Row) -> Result<Label, Error> {
        Ok(Label {
            label: r.opt_text(1)?.map(str::to_string),
            months: r.opt_int(2)?,
            nominal: r.opt_int(3)?,
            offset_months: r.opt_double(4)?,
            flagged: r.int(5)?,
            reason: r.opt_text(6)?.map(str::to_string),
        })
    }

    /// Whether a stored row says this already. A double reads back as it
    /// was written on either backend; zero and minus zero are one value.
    fn said_by(&self, row: &Label) -> bool {
        let offset = match (self.offset_months, row.offset_months) {
            (Some(a), Some(b)) => a == b || (a.is_nan() && b.is_nan()),
            (a, b) => a.is_none() && b.is_none(),
        };
        offset
            && self.label == row.label
            && self.months == row.months
            && self.nominal == row.nominal
            && self.flagged == row.flagged
            && self.reason == row.reason
    }

    /// The row an insert writes, in the order of [`LABEL_COLUMNS`].
    fn row(&self, session_id: i64, scheme_digest: &str) -> Vec<Param> {
        vec![
            Param::Int(session_id),
            Param::from(scheme_digest),
            self.label.as_deref().map_or(Param::Null, Param::from),
            self.months.map_or(Param::Null, Param::Int),
            self.nominal.map_or(Param::Null, Param::Int),
            self.offset_months.map_or(Param::Null, Param::Double),
            Param::Int(self.flagged),
            self.reason.as_deref().map_or(Param::Null, Param::from),
        ]
    }
}

/// The label rows under a scheme of one subject's sessions in a window, by
/// session. One statement whatever the subject, so a connection that reads
/// many subjects prepares it once.
fn labels_of(
    store: &mut Store,
    subject: i64,
    window: i64,
    scheme_digest: &str,
) -> Result<HashMap<i64, Label>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT l.session_id, l.label, l.months, l.nominal, l.offset_months, l.flagged, l.reason \
         FROM {label} l JOIN {cache} c ON c.id = l.session_id \
         WHERE c.subject_id = {} AND c.window_days = {} AND l.scheme_digest = {}",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Text),
        label = store.qualified("session_label"),
        cache = store.qualified("session_cache"),
    );
    let mut out = HashMap::new();
    let params = [
        Param::Int(subject),
        Param::Int(window),
        Param::from(scheme_digest),
    ];
    for r in store.query(&sql, &params)? {
        out.insert(r.int(0)?, Label::read(&r)?);
    }
    Ok(out)
}

/// What the label rows of a subject's sessions named have to become to say
/// what the resolver says: the rows to write, and the sessions whose row is
/// to go. `held` is the rows as they stand, by session. Both empty where
/// the labels stand, which is how a reader finds them up to date without
/// writing ([`stale`]) and how [`relabel`] writes only what changed.
fn label_changes(
    ids: &[i64],
    cached: &[Stored],
    resolved: &[Session],
    held: &mut HashMap<i64, Label>,
    scheme_digest: &str,
) -> (Vec<Vec<Param>>, Vec<i64>) {
    // the resolver's session for each cached one, by its first study
    let mut rows: Vec<Vec<Param>> = Vec::new();
    for c in cached.iter().filter(|c| ids.contains(&c.id)) {
        let Some(s) = resolved
            .iter()
            .find(|s| c.studies.first().is_some_and(|f| s.studies.contains(f)))
        else {
            continue;
        };
        let label = Label::of(s);
        if held.remove(&c.id).is_some_and(|row| label.said_by(&row)) {
            continue;
        }
        rows.push(label.row(c.id, scheme_digest));
    }
    // a row still held is of a session named that the resolver gives no
    // label
    let mut gone: Vec<i64> = held.keys().copied().filter(|id| ids.contains(id)).collect();
    gone.sort_unstable();
    (rows, gone)
}

/// The labels under one scheme of a subject's sessions named, brought to
/// what the resolver says: a missing row is written, a row that says
/// something else is written over, the row of a session the resolver gives
/// no label is taken away, and a row that says it already is left alone
/// (2026-10-14: every read deleted every row and wrote it again, and two
/// reads at once collided on the rows' unique key). Answers how many
/// sessions' rows it wrote, none where the labels stood.
fn relabel(
    store: &mut Store,
    subject: i64,
    window: i64,
    ids: &[i64],
    cached: &[Stored],
    resolved: &[Session],
    scheme_digest: &str,
) -> Result<usize, Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut held = labels_of(store, subject, window, scheme_digest)?;
    let (rows, stale) = label_changes(ids, cached, resolved, &mut held, scheme_digest);
    let d = store.dialect();
    for id in &stale {
        store.execute(
            &format!(
                "DELETE FROM {} WHERE session_id = {} AND scheme_digest = {}",
                store.qualified("session_label"),
                d.param(1, Type::Int),
                d.param(2, Type::Text)
            ),
            &[Param::Int(*id), Param::from(scheme_digest)],
        )?;
    }
    if !rows.is_empty() {
        // an insert that writes over the row its key finds: the lock keeps
        // two builds apart, and this is what a row another writer made
        // meanwhile meets
        store.insert(
            &Insert::new(table("session_label"), &LABEL_COLUMNS).on_conflict(Conflict::Update {
                target: &["session_id", "scheme_digest"],
                set: &LABEL_SET,
            }),
            &rows,
        )?;
    }
    Ok(rows.len() + stale.len())
}

fn iso(d: Day) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Picks keyed on a day that moved follow it: those made under this scheme,
/// and those from before a pick named its scheme's digest. A run's pick a
/// person's overruled follows too (record 42 S3), so that withdrawing the
/// person's pick lets it apply on the day the occasion now opens.
fn rekey_picks(
    store: &mut Store,
    subject: i64,
    from: Day,
    to: Day,
    scheme_digest: &str,
    _now: &str,
) -> Result<usize, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET session_day = {} WHERE subject_id = {} AND session_day = {} \
         AND (withdrawn_at IS NULL OR overruled_by IS NOT NULL) \
         AND (scheme_digest = {} OR scheme_digest IS NULL)",
        store.qualified("pick"),
        d.param(1, Type::Date),
        d.param(2, Type::Int),
        d.param(3, Type::Date),
        d.param(4, Type::Text)
    );
    Ok(store.execute(
        &sql,
        &[
            Param::from(iso(to)),
            Param::Int(subject),
            Param::from(iso(from)),
            Param::from(scheme_digest),
        ],
    )? as usize)
}

/// Picks keyed on a day that no longer opens a session are withdrawn, never
/// deleted: what was decided stays readable, and stops applying.
fn withdraw_picks(
    store: &mut Store,
    subject: i64,
    day: Day,
    scheme_digest: &str,
    now: &str,
) -> Result<usize, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET withdrawn_at = {} WHERE subject_id = {} AND session_day = {} \
         AND withdrawn_at IS NULL AND (scheme_digest = {} OR scheme_digest IS NULL)",
        store.qualified("pick"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Date),
        d.param(4, Type::Text)
    );
    Ok(store.execute(
        &sql,
        &[
            Param::from(now),
            Param::Int(subject),
            Param::from(iso(day)),
            Param::from(scheme_digest),
        ],
    )? as usize)
}

#[allow(clippy::too_many_arguments)]
fn raise(
    store: &mut Store,
    subject: i64,
    code: &str,
    window: i64,
    from: Option<Day>,
    to: Option<Day>,
    studies: &[i64],
    rekeyed: usize,
    withdrawn: usize,
    now: &str,
) -> Result<(), Error> {
    let reference = serde_json::json!({ "subject_id": subject, "code": code });
    let evidence = serde_json::json!({
        "window_days": window,
        "from_first": from.map(iso),
        "to_first": to.map(iso),
        "studies": studies,
        "picks_rekeyed": rekeyed,
        "picks_withdrawn": withdrawn,
    });
    store.insert(
        &Insert::new(
            table("review_item"),
            &["kind", "scope", "ref", "evidence", "status", "created_at"],
        ),
        &[vec![
            Param::from(MOVED_KIND),
            Param::from("subject"),
            Param::from(reference.to_string()),
            Param::from(evidence.to_string()),
            Param::from("open"),
            Param::from(now),
        ]],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- readers

/// Every cached session under a scheme, for one subject or all, ordered by
/// subject code, then day. A session with no label row under this scheme is
/// returned with no label; `ensure` is what writes them.
pub fn sessions_of(
    store: &mut Store,
    scheme: &Scheme,
    only: Option<&str>,
) -> Result<Vec<Cached>, Error> {
    let d = store.dialect();
    let t = table("session_cache");
    let first = d.text_of_qualified(Some("c"), t.column("first").expect("first"));
    let last = d.text_of_qualified(Some("c"), t.column("last").expect("last"));
    let mut params = vec![Param::from(scheme.digest()), Param::Int(scheme.window_days)];
    let mut where_subject = String::new();
    if let Some(code) = only {
        where_subject = format!(" AND su.code = {}", d.param(3, Type::Text));
        params.push(Param::from(code));
    }
    let sql = format!(
        "SELECT c.id, c.subject_id, su.code, {first}, {last}, \
                l.label, l.months, l.nominal, l.offset_months, l.flagged, l.reason \
         FROM {cache} c JOIN {subject} su ON su.id = c.subject_id \
         LEFT JOIN {label} l ON l.session_id = c.id AND l.scheme_digest = {p1} \
         WHERE c.window_days = {p2}{where_subject} \
         ORDER BY su.code, {first}, c.id",
        cache = store.qualified("session_cache"),
        subject = store.qualified("subject"),
        label = store.qualified("session_label"),
        p1 = d.param(1, Type::Text),
        p2 = d.param(2, Type::Int),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &params)? {
        out.push(Cached {
            id: r.int(0)?,
            subject_id: r.int(1)?,
            code: r.text(2)?.to_string(),
            window_days: scheme.window_days,
            first: Day::parse(r.text(3)?).ok_or_else(|| Error::Message("a cached day".into()))?,
            last: Day::parse(r.text(4)?).ok_or_else(|| Error::Message("a cached day".into()))?,
            studies: Vec::new(),
            label: r.opt_text(5)?.map(str::to_string),
            months: r.opt_int(6)?.map(|m| m as i32),
            nominal: r.opt_int(7)?.map(|m| m as i32),
            offset_months: r.opt_double(8)?,
            flagged: r.opt_int(9)?.unwrap_or(0) != 0,
            reason: r.opt_text(10)?.map(str::to_string),
            has_primary: None,
        });
    }
    if out.is_empty() {
        return Ok(out);
    }
    let members = format!(
        "SELECT cs.session_id, cs.study_id, st.has_original_primary FROM {} cs \
         JOIN {} st ON st.id = cs.study_id WHERE cs.window_days = {} ORDER BY cs.study_id",
        store.qualified("session_cache_study"),
        store.qualified("study"),
        d.param(1, Type::Int)
    );
    let mut by_session: HashMap<i64, (Vec<i64>, Option<bool>, bool)> = HashMap::new();
    for r in store.query(&members, &[Param::Int(scheme.window_days)])? {
        let e = by_session
            .entry(r.int(0)?)
            .or_insert((Vec::new(), None, true));
        e.0.push(r.int(1)?);
        // null is not no: a study whose stacks are not all fingerprinted has
        // not said it holds no primary
        let says = r.opt_int(2)?.map(|v| v != 0);
        e.1 = match (e.2, e.1, says) {
            (true, _, first) => first,
            (false, Some(true), _) | (false, _, Some(true)) => Some(true),
            (false, None, _) | (false, _, None) => None,
            _ => Some(false),
        };
        e.2 = false;
    }
    for c in &mut out {
        if let Some((studies, has_primary, _)) = by_session.remove(&c.id) {
            c.studies = studies;
            c.has_primary = has_primary;
        }
    }
    Ok(out)
}

/// Each study's session under a scheme, by study id, for a reader that
/// starts from studies (the release, the picker).
pub fn labels_by_study(
    store: &mut Store,
    scheme: &Scheme,
) -> Result<HashMap<i64, Labelled>, Error> {
    let d = store.dialect();
    let t = table("session_cache");
    let first = d.text_of_qualified(Some("c"), t.column("first").expect("first"));
    let sql = format!(
        "SELECT cs.study_id, c.id, c.subject_id, {first}, l.label \
         FROM {members} cs JOIN {cache} c ON c.id = cs.session_id \
         LEFT JOIN {label} l ON l.session_id = c.id AND l.scheme_digest = {p1} \
         WHERE cs.window_days = {p2}",
        members = store.qualified("session_cache_study"),
        cache = store.qualified("session_cache"),
        label = store.qualified("session_label"),
        p1 = d.param(1, Type::Text),
        p2 = d.param(2, Type::Int),
    );
    let mut out = HashMap::new();
    for r in store.query(
        &sql,
        &[Param::from(scheme.digest()), Param::Int(scheme.window_days)],
    )? {
        out.insert(
            r.int(0)?,
            Labelled {
                session_id: r.int(1)?,
                subject_id: r.int(2)?,
                first: Day::parse(r.text(3)?)
                    .ok_or_else(|| Error::Message("a cached day".into()))?,
                label: r.opt_text(4)?.map(str::to_string),
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeline_digest_ignores_order_and_follows_dates() {
        let a = Study::new(1, Day::new(2020, 1, 1).unwrap());
        let b = Study::new(2, Day::new(2020, 6, 1).unwrap());
        let ab = timeline_digest(&[a.clone(), b.clone()]);
        let ba = timeline_digest(&[b.clone(), a.clone()]);
        assert_eq!(ab, ba);
        let moved = Study::new(2, Day::new(2020, 7, 1).unwrap());
        assert_ne!(ab, timeline_digest(&[a, moved]));
    }

    #[test]
    fn a_source_label_comes_out_of_one_directory() {
        assert_eq!(label_in("/a/b/V02/f.dcm", 3, None).as_deref(), Some("V02"));
        assert_eq!(label_in("/a/b/V02/f.dcm", 4, None), None);
        let re = regex::Regex::new(r"^V(?P<label>\d+)$").unwrap();
        assert_eq!(
            label_in("/a/b/V02/f.dcm", 3, Some(&re)).as_deref(),
            Some("02")
        );
    }

    #[test]
    fn anchors_parse_a_csv_with_or_without_a_header() {
        let m = Anchors::parse_csv("code,date\nS1,2020-01-02\nS2,20210304\n").unwrap();
        assert_eq!(m["S1"], Day::new(2020, 1, 2).unwrap());
        assert_eq!(m["S2"], Day::new(2021, 3, 4).unwrap());
        assert!(Anchors::parse_csv("S1,not a date\n").is_err());
    }
}
