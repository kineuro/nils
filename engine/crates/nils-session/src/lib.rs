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

use std::collections::{BTreeMap, HashMap, HashSet};

use nils_registry::clinical;
use nils_registry::day::Day;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Anchor, Scheme, Session, Study};
use nils_registry::store::Error as StoreError;
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param, Registry, Store};
use serde::{Deserialize, Serialize};

pub use nils_registry::session::{Collision, Naming, Reason, Said, Unmatched};

/// The review item kind a moved or vanished span raises.
pub const MOVED_KIND: &str = "session.moved";

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
    pub relabelled: usize,
    pub moved: usize,
    pub vanished: usize,
    pub picks_rekeyed: usize,
    pub picks_withdrawn: usize,
    pub items: usize,
}

struct Point {
    subject_id: i64,
    code: String,
    study: Study,
}

/// Every dated study, whole timeline per subject, with the source's own
/// label when the scheme reads one. A study whose date the vote could not
/// settle is left out: it is not a point on a timeline.
fn points(store: &mut Store, scheme: &Scheme, only: Option<&str>) -> Result<Vec<Point>, Error> {
    let d = store.dialect();
    let (study_t, subject_t) = (store.qualified("study"), store.qualified("subject"));
    let st = table("study");
    let filled = d.text_of_qualified(Some("st"), st.column("date_filled").expect("date_filled"));
    let dated = d.text_of_qualified(Some("st"), st.column("study_date").expect("study_date"));
    let mut params: Vec<Param> = Vec::new();
    let mut where_subject = String::new();
    if let Some(code) = only {
        where_subject = format!(" AND su.code = {}", d.param(1, Type::Text));
        params.push(Param::from(code));
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
             ORDER BY su.id, 4, st.id"
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
/// the scheme gives, for one subject or for all. Runs in one transaction.
pub fn ensure(
    registry: &mut Registry,
    scheme: &Scheme,
    anchors: &Anchors,
    only: Option<&str>,
    force: bool,
) -> Result<Rebuilt, Error> {
    let epoch = registry.meta().epoch;
    let store = registry.store();
    let all = points(store, scheme, only)?;
    let mut by_subject: BTreeMap<i64, (String, Vec<Study>)> = BTreeMap::new();
    for p in all {
        let e = by_subject
            .entry(p.subject_id)
            .or_insert_with(|| (p.code.clone(), Vec::new()));
        e.1.push(p.study);
    }
    let digest = scheme.digest();
    let window = scheme.window_days;
    let now = now_iso();
    let mut out = Rebuilt::default();
    store.begin()?;
    let result = (|| -> Result<(), Error> {
        for (subject, (code, studies)) in &by_subject {
            out.subjects += 1;
            let timeline = timeline_digest(studies);
            let old = stored(store, *subject, window)?;
            let unchanged = !force && !old.is_empty() && old.iter().all(|s| s.digest == timeline);
            let anchor = anchors.of(scheme, code, studies);
            let resolved = session::sessions(studies, anchor, scheme);
            if unchanged {
                out.unchanged += 1;
                out.sessions += old.len();
                // the labels under this scheme may still be missing
                let ids: Vec<i64> = old.iter().map(|s| s.id).collect();
                out.relabelled += relabel(store, &ids, &old, &resolved, &digest)?;
                continue;
            }
            out.rebuilt += 1;
            let ids = reconcile(
                store, *subject, code, window, &old, &resolved, &timeline, epoch, &now, &digest,
                &mut out,
            )?;
            out.sessions += ids.len();
            let fresh = stored(store, *subject, window)?;
            out.relabelled += relabel(store, &ids, &fresh, &resolved, &digest)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            store.commit()?;
            Ok(out)
        }
        Err(e) => {
            store.rollback().ok();
            Err(e)
        }
    }
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

/// The labels under one scheme for the sessions named, replaced whole.
/// Returns how many sessions got a label row.
fn relabel(
    store: &mut Store,
    ids: &[i64],
    cached: &[Stored],
    resolved: &[Session],
    scheme_digest: &str,
) -> Result<usize, Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let d = store.dialect();
    // the resolver's session for each cached one, by its first study
    let mut rows: Vec<Vec<Param>> = Vec::new();
    for c in cached.iter().filter(|c| ids.contains(&c.id)) {
        let Some(s) = resolved
            .iter()
            .find(|s| c.studies.first().is_some_and(|f| s.studies.contains(f)))
        else {
            continue;
        };
        rows.push(vec![
            Param::Int(c.id),
            Param::from(scheme_digest),
            s.label.as_deref().map_or(Param::Null, Param::from),
            s.months.map_or(Param::Null, |m| Param::Int(i64::from(m))),
            s.nominal.map_or(Param::Null, |m| Param::Int(i64::from(m))),
            s.offset_months.map_or(Param::Null, Param::Double),
            Param::Int(i64::from(s.flagged)),
            s.reason.map_or(Param::Null, |r| Param::from(r.name())),
        ]);
    }
    for id in ids {
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
    let n = rows.len();
    if n > 0 {
        store.insert(
            &Insert::new(
                table("session_label"),
                &[
                    "session_id",
                    "scheme_digest",
                    "label",
                    "months",
                    "nominal",
                    "offset_months",
                    "flagged",
                    "reason",
                ],
            ),
            &rows,
        )?;
    }
    Ok(n)
}

fn iso(d: Day) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// Picks keyed on a day that moved follow it: those made under this scheme,
/// and those from before a pick named its scheme's digest.
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
         AND withdrawn_at IS NULL AND (scheme_digest = {} OR scheme_digest IS NULL)",
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
