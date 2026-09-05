// SPDX-License-Identifier: AGPL-3.0-only

//! Sessions: the occasion a subject came in
//! (`docs/specs/wave3-anonymize-and-bids.md`, §5).
//!
//! A study is DICOM's unit and a row, because it is a fact in a file. A session
//! is not: no file records it. The PACS splits one visit into a brain study and
//! a spine study, a scan that stopped and started again is a new study, and a
//! visit too long for one day continues later in the week. So a session is
//! **derived**, from a scheme, on read, and never stored: changing the scheme
//! re-labels a cohort instead of migrating it.
//!
//! Carried from v0's `timeline/`, which had this right, with the one thing it
//! left undone. Its resolver says of itself that it "never keys on
//! `study_date`. Everything keys on `visit_key`, which today is
//! `study_date.isoformat()` and is the seam a future multi-day visit grouper
//! slots into." This is that grouper: §2 groups studies into visits before §3
//! labels them, so the brain-on-Monday and spine-on-Wednesday visit is one
//! session rather than two.
//!
//! **A label is not a key.** `M12` does not identify a session: two can share
//! it under the default policy, one can be `PRE06`, and one off the schedule
//! keeps its own real month. Anything that joins reads the date.
//!
//! Three things differ from v0, and each is argued where it happens:
//!
//! 1. A session is a group of studies, not a single one (`window_days`, §2).
//!    v0 keys on `visit_key` and sets it to the study date, which is this with
//!    the window fixed at zero.
//! 2. A contested label goes to the session closest to the nominal measured
//!    exactly, not to the one closest after rounding, which ties constantly and
//!    then falls back on whoever came first (`claim`).
//! 3. A pre-anchor session demotes to a `PRE` label, never to an `M` one
//!    (`demote`).
//!
//! Only the first changes anything under the default scheme; the other two are
//! reachable only under the opt-in collision policies.
//!
//! What arrives here has a date. A study whose date the vote of §4 could not
//! settle is not a point on a timeline, and the caller leaves it out rather
//! than asking this module to rank a null.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::day::Day;

/// Where month zero sits.
///
/// The resolver never turns a kind into a date: that needs event rows, and
/// making a pure function reach for them is what makes it impossible to test.
/// The caller resolves the kind and hands over the day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The subject's first session in scope, which is v0's default.
    FirstSession,
    /// A clinical event: onset, diagnosis, treatment start. The clinical layer
    /// arrives in Wave 4, so Wave 3 accepts the day and does not fetch it.
    Event,
    /// A day given per subject.
    Explicit,
    /// Month zero is wherever the source's own labels put it.
    ///
    /// The other kinds are resolved by the caller, because turning a diagnosis
    /// into a date needs event rows and reaching for those is what makes a
    /// resolver impossible to test. This one needs nothing the studies do not
    /// already carry, so it is resolved here.
    ///
    /// It is what an archive that is a fragment wants. If the copy we hold
    /// starts at the six-month visit, a `first_session` anchor calls that
    /// `M00`, which is true of our holdings and useless clinically; the folder
    /// the archive kept it in says `M06`, and that is the baseline.
    SourceLabel,
}

/// What a session is called.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Naming {
    /// The first study's day as `YYYYMMDD`, which is v0's `ses-` label.
    Date,
    /// Months from the anchor, snapped to a cadence when close enough.
    Months {
        /// Nominal visit months, ascending.
        cadence: Vec<i32>,
        /// How far from a nominal month a session may sit and still take its
        /// label. A fraction, because 1.5 must not behave like 1.
        tolerance: f64,
    },
    /// `01`, `02`, in date order. What to use when there is no anchor and no
    /// schedule, which v0 has no answer for.
    Ordinal,
}

/// What to do when two sessions of one subject want one label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    /// Everyone keeps it, and nothing is flagged.
    ///
    /// This is the default, and the argument is v0's: two sessions on one label
    /// is normal clinical reality, and demoting the second invents a timepoint
    /// nobody scanned. It is also the only policy under which a label is a
    /// function of its own session rather than of the subject's whole timeline
    /// (§5.1).
    Merge,
    /// One keeps it; the rest fall back to their own real month.
    DemoteThenDate,
    /// The same, then `a`, `b`, `c` when the real month is taken too.
    DemoteThenSuffix,
    /// Nobody keeps it.
    AlwaysDate,
}

/// What to do with a session that cannot be placed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unmatched {
    /// No label; the caller falls back to the date.
    KeepDate,
    /// The label `unknown`.
    LabelUnknown,
}

/// How a subject's studies become sessions, and what those are called.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scheme {
    /// A study joins the open session when it falls within this many days of
    /// that session's **first** study. Zero is the same calendar day, which is
    /// v0's event and its QC grouping.
    ///
    /// Measured from the first study rather than from the previous one, so a
    /// session's span is bounded: chaining lets a session drift a month at a
    /// time for ever, and a session nobody can put a length to is not one.
    #[serde(default)]
    pub window_days: i64,
    #[serde(default = "first_session")]
    pub anchor: Anchor,
    #[serde(default = "by_date")]
    pub naming: Naming,
    #[serde(default = "merge")]
    pub collision: Collision,
    #[serde(default = "keep_date")]
    pub unmatched: Unmatched,
    /// Where in a source path the archive's own label sits, when it has one.
    ///
    /// The resolver never reads this: it is given the label on each `Study`.
    /// It lives here because it is part of how a subject's studies become
    /// sessions, and because a scheme has to be enough, on its own, to
    /// reproduce a labelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<Said>,
}

/// Where the source's own label sits in a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Said {
    /// Counted from one, as the identity rule counts them.
    pub segment: usize,
    /// Taken from a group named `label`. Without a pattern the whole segment
    /// is the label, which is what an archive with `M06/` folders wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

fn first_session() -> Anchor {
    Anchor::FirstSession
}

fn by_date() -> Naming {
    Naming::Date
}

fn merge() -> Collision {
    Collision::Merge
}

fn keep_date() -> Unmatched {
    Unmatched::KeepDate
}

/// What is wrong with a scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeError(pub String);

impl fmt::Display for SchemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemeError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    session: Scheme,
}

impl Scheme {
    /// A scheme from its YAML, checked.
    ///
    /// The checks are the ones v0's model makes, for the same reason: a
    /// resolver has to stay total, so anything that could make it lie is
    /// refused here instead.
    pub fn parse(yaml: &str) -> Result<Scheme, SchemeError> {
        let file: File = serde_saphyr::from_str(yaml)
            .map_err(|e| SchemeError(format!("the session scheme does not parse: {e}")))?;
        file.session.check()?;
        Ok(file.session)
    }

    /// A scheme from the JSON it is stored as.
    pub fn from_json(text: &str) -> Result<Scheme, SchemeError> {
        let scheme: Scheme = serde_json::from_str(text)
            .map_err(|e| SchemeError(format!("the stored session scheme does not parse: {e}")))?;
        scheme.check()?;
        Ok(scheme)
    }

    pub fn check(&self) -> Result<(), SchemeError> {
        if self.window_days < 0 {
            return Err(SchemeError(
                "session.window_days is negative; a window is a number of days".into(),
            ));
        }
        if let Naming::Months { cadence, tolerance } = &self.naming {
            if cadence.iter().any(|m| *m < 0) {
                return Err(SchemeError(
                    "session.naming.months.cadence: a nominal month is not negative; a visit before the anchor is labelled PRE".into(),
                ));
            }
            if cadence.windows(2).any(|w| w[0] >= w[1]) {
                return Err(SchemeError(
                    "session.naming.months.cadence must be ascending and without duplicates".into(),
                ));
            }
            if tolerance.is_nan() || *tolerance < 0.0 {
                return Err(SchemeError(
                    "session.naming.months.tolerance is not a number of months".into(),
                ));
            }
        }
        if let Some(said) = &self.said {
            if said.segment == 0 {
                return Err(SchemeError(
                    "session.said.segment is counted from one".into(),
                ));
            }
            if let Some(pattern) = &said.pattern
                && !pattern.contains("(?<label>")
                && !pattern.contains("(?P<label>")
            {
                return Err(SchemeError(
                    "session.said.pattern must name a group `label`, so it is clear which part of the segment is the label".into(),
                ));
            }
        }
        Ok(())
    }
}

impl Default for Scheme {
    /// v0's behaviour: same-day sessions labelled by their date.
    fn default() -> Scheme {
        Scheme {
            window_days: 0,
            anchor: Anchor::FirstSession,
            naming: Naming::Date,
            collision: Collision::Merge,
            unmatched: Unmatched::KeepDate,
            said: None,
        }
    }
}

/// One study, as the scheme needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Study {
    pub id: i64,
    /// The day it happened, which may be the one the vote found (§4).
    pub day: Day,
    /// What the source archive called this visit, when it said. An archive
    /// whose paths already carry `M06` is telling you something the dates
    /// alone do not, so it is read as evidence rather than discarded.
    pub said: Option<String>,
    /// Whether this study holds a stack the scanner called its output
    /// (`study.has_original_primary`, §6). `None` when the study is not fully
    /// fingerprinted, which is not the same as no.
    pub has_primary: Option<bool>,
}

impl Study {
    /// A study the source said nothing about.
    pub fn new(id: i64, day: Day) -> Study {
        Study {
            id,
            day,
            said: None,
            has_primary: None,
        }
    }
}

/// A source label as a month offset: `M06` is six, `PRE06` is minus six.
///
/// A `ses-` prefix is tolerated because that is how the label appears in an
/// exported tree. Anything else is not a label this scheme wrote, and reading
/// a number out of it would be inventing evidence.
fn heard(said: &str) -> Option<i32> {
    let text = said.trim();
    let text = text.strip_prefix("ses-").unwrap_or(text);
    let (sign, digits) = match text.strip_prefix("PRE") {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('M')?),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(sign * digits.parse::<i32>().ok()?)
}

/// Why a session is worth a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nothing to measure from.
    NoAnchor,
    /// Before its anchor, which is a real thing under a diagnosis anchor.
    PreAnchor,
    /// It holds a label, but not the nominal one it wanted.
    Demoted,
    /// It could not be given a label of its own.
    Collision,
    /// The source called it something the dates do not support.
    SourceDisagrees,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::NoAnchor => "no_anchor",
            Reason::PreAnchor => "pre_anchor",
            Reason::Demoted => "demoted",
            Reason::Collision => "collision",
            Reason::SourceDisagrees => "source_disagrees",
        }
    }
}

/// One occasion the subject came in.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// The studies of it, in date order.
    pub studies: Vec<i64>,
    /// The day the session opened, which is the key everything joins on.
    pub first: Day,
    /// The day its last study happened, equal to `first` for a one-day visit.
    pub last: Day,
    /// What to call it, or none when the scheme could not name it.
    pub label: Option<String>,
    /// Months from the anchor, rounded, when there was one.
    pub months: Option<i32>,
    /// The cadence month it snapped to, when it did. A session labelled by its
    /// own real month has none, which is how a reader tells a scheduled visit
    /// from one that merely happened.
    pub nominal: Option<i32>,
    /// How far it actually is from the anchor, unrounded, so a reader can see
    /// that an `M06` sits at 7.4 months.
    pub offset_months: Option<f64>,
    pub flagged: bool,
    pub reason: Option<Reason>,
    /// Whether any study of this occasion holds a stack the scanner called its
    /// output (§6).
    ///
    /// `Some(false)` is the session rescue's condition: some exports tag every
    /// reconstruction `ORIGINAL\SECONDARY` and never write a primary at all,
    /// so the ordinary exclusion of secondary-without-primary throws the whole
    /// visit away and the subject becomes unusable. When this is false, a
    /// stack whose role is `original_secondary` is the best the session has and
    /// is treated as its primary.
    ///
    /// `None` means at least one study of the session has not been fully
    /// fingerprinted, so the question is not answered rather than answered no.
    /// A rescue on an unanswered question would be a rescue on a guess.
    ///
    /// This is why the rescue cannot be a stored field: which studies are one
    /// session is a scheme's answer, and the same study can be a session on its
    /// own under one scheme and part of a larger visit under another. v0 has
    /// the same grouping hard-wired to the calendar day and computes it over
    /// whatever stacks were in the batch.
    pub has_primary: Option<bool>,
}

/// Group a subject's studies into sessions and name them.
///
/// `studies` may arrive in any order and may repeat; the result is the same
/// either way, which is what makes a label reproducible. `anchor` is the day
/// the scheme's anchor kind resolved to, or none when it could not.
pub fn sessions(studies: &[Study], anchor: Option<Day>, scheme: &Scheme) -> Vec<Session> {
    let grouped = group(studies, scheme.window_days);
    let anchor = match scheme.anchor {
        Anchor::SourceLabel => from_labels(&grouped),
        _ => anchor,
    };
    let mut placed = place(&grouped, anchor, scheme);
    hear(&mut placed, &grouped, scheme);
    relax(&mut placed, scheme);
    settle(placed, scheme)
}

/// Month zero, read back out of the earliest visit the source named.
///
/// Only a visit the source was unambiguous about counts: one label, and one
/// this scheme could have written. When no visit qualifies there is no anchor,
/// and saying so is better than picking a visit at random.
fn from_labels(grouped: &[Visit]) -> Option<Day> {
    grouped.iter().find_map(|v| {
        let [said] = v.said.as_slice() else {
            return None;
        };
        Some(v.first.plus_months(-(heard(said)? as i64)))
    })
}

/// §2: studies into visits. Sorted first, so the grouping does not depend on
/// the order rows came back in.
fn group(studies: &[Study], window_days: i64) -> Vec<Visit> {
    let mut sorted: Vec<Study> = studies.to_vec();
    sorted.sort_by_key(|s| (s.day, s.id));
    sorted.dedup_by_key(|s| s.id);

    let mut out: Vec<Visit> = Vec::new();
    for s in sorted {
        match out.last_mut() {
            Some(v) if v.first.days_to(s.day) <= window_days => {
                v.last = s.day;
                v.studies.push(s.id);
                push_said(&mut v.said, s.said);
                v.has_primary = joined(v.has_primary, s.has_primary);
            }
            _ => {
                let mut said = Vec::new();
                push_said(&mut said, s.said);
                out.push(Visit {
                    first: s.day,
                    last: s.day,
                    studies: vec![s.id],
                    said,
                    has_primary: s.has_primary,
                });
            }
        }
    }
    out
}

/// One occasion, before it has a name.
struct Visit {
    first: Day,
    last: Day,
    studies: Vec<i64>,
    /// The distinct things the source called it, in the order first heard.
    /// More than one means the window pulled together studies the source kept
    /// apart, which is worth saying out loud.
    said: Vec<String>,
    /// Whether any study of the visit holds a primary. One study that says yes
    /// answers for the whole visit; otherwise one study that cannot say leaves
    /// the visit unable to.
    has_primary: Option<bool>,
}

/// Whether a visit holds a primary, given one more study.
///
/// One yes answers for the visit. Otherwise one study that cannot say leaves
/// the visit unable to, because a rescue turns on the answer being no and a
/// study that has not been fully fingerprinted has not said no.
fn joined(so_far: Option<bool>, next: Option<bool>) -> Option<bool> {
    match (so_far, next) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, _) | (_, None) => None,
        _ => Some(false),
    }
}

fn push_said(into: &mut Vec<String>, said: Option<String>) {
    if let Some(text) = said
        && !text.trim().is_empty()
        && !into.contains(&text)
    {
        into.push(text);
    }
}

/// A session with its provisional label, before collisions are settled.
struct Placed {
    session: Session,
    /// True when it precedes its anchor: it never snaps to a cadence point,
    /// because the cadence describes follow-up and `PRE06` must not be
    /// confusable with `M06`.
    pre_anchor: bool,
    /// True when the label came from the source rather than from the dates, in
    /// which case nothing later moves it.
    from_source: bool,
}

/// §3: a label for each, from its own offset alone.
fn place(grouped: &[Visit], anchor: Option<Day>, scheme: &Scheme) -> Vec<Placed> {
    grouped
        .iter()
        .enumerate()
        .map(|(i, visit)| {
            let (first, last) = (&visit.first, &visit.last);
            let base = Session {
                studies: visit.studies.clone(),
                first: *first,
                last: *last,
                label: None,
                months: None,
                nominal: None,
                offset_months: None,
                flagged: false,
                reason: None,
                has_primary: visit.has_primary,
            };
            match &scheme.naming {
                Naming::Date => Placed {
                    session: Session {
                        label: Some(first.compact()),
                        ..base
                    },
                    pre_anchor: false,
                    from_source: false,
                },
                Naming::Ordinal => Placed {
                    session: Session {
                        label: Some(format!("{:02}", i + 1)),
                        ..base
                    },
                    pre_anchor: false,
                    from_source: false,
                },
                Naming::Months { cadence, tolerance } => {
                    let Some(anchor) = anchor else {
                        return Placed {
                            session: Session {
                                label: match scheme.unmatched {
                                    Unmatched::LabelUnknown => Some("unknown".into()),
                                    Unmatched::KeepDate => None,
                                },
                                flagged: true,
                                reason: Some(Reason::NoAnchor),
                                ..base
                            },
                            pre_anchor: false,
                            from_source: false,
                        };
                    };
                    let exact = anchor.months_to(*first);
                    let months = exact.abs().round() as i32;
                    let pre = exact < 0.0 && months != 0;
                    if pre {
                        return Placed {
                            session: Session {
                                label: Some(format!("PRE{months:02}")),
                                months: Some(months),
                                offset_months: Some(exact),
                                flagged: true,
                                reason: Some(Reason::PreAnchor),
                                ..base
                            },
                            pre_anchor: true,
                            from_source: false,
                        };
                    }
                    // Month zero is month zero even under a cadence that does
                    // not list it.
                    let nominal = if months == 0 {
                        cadence.contains(&0).then_some(0)
                    } else {
                        nearest(exact, cadence, *tolerance)
                    };
                    Placed {
                        session: Session {
                            label: Some(format!("M{:02}", nominal.unwrap_or(months))),
                            months: Some(months),
                            nominal,
                            offset_months: Some(exact),
                            ..base
                        },
                        pre_anchor: false,
                        from_source: false,
                    }
                }
            }
        })
        .collect()
}

/// The cadence month `exact` snaps to, or none when it is off schedule.
///
/// Compared on the unrounded distance, so a fractional tolerance means what it
/// says. A tie goes to the earlier visit.
fn nearest(exact: f64, cadence: &[i32], tolerance: f64) -> Option<i32> {
    let best = cadence.iter().copied().min_by(|a, b| {
        let (da, db) = ((exact - *a as f64).abs(), (exact - *b as f64).abs());
        da.partial_cmp(&db).unwrap().then(a.cmp(b))
    })?;
    ((exact - best as f64).abs() <= tolerance).then_some(best)
}

/// §3b: what the source called the visit, where it said.
///
/// An archive whose paths already carry `M06` records intent that the dates
/// alone do not: somebody decided, at the time, which visit this was. So a
/// canonical source label that agrees with the computed gap wins, because it
/// is the same answer with a witness; one that disagrees leaves the computed
/// label standing and raises a flag, because a disagreement is a finding and
/// not a thing to resolve silently; and one with nothing to disagree with
/// (no anchor, so nothing was computed) is taken, because a label somebody
/// wrote beats no label at all.
///
/// A `first_session` anchor makes the earliest session we hold `M00` by
/// construction, so an archive that is a fragment disagrees with itself: the
/// source says `M06`, the dates say `M00`, and both are right about different
/// things. That is a finding, and it is reported as one. The archive that
/// wants the source's answer instead asks for `Anchor::SourceLabel`, which
/// reads month zero back out of the labels and then agrees with them.
fn hear(placed: &mut [Placed], grouped: &[Visit], scheme: &Scheme) {
    let (cadence, tolerance) = match &scheme.naming {
        Naming::Months { cadence, tolerance } => (cadence, *tolerance),
        // A date or an ordinal label has no month to compare against, and a
        // date label is already the strongest statement of when.
        _ => return,
    };
    for (p, visit) in placed.iter_mut().zip(grouped) {
        // Two different labels inside one visit means the window pulled
        // together studies the source kept apart. Say so, and believe neither.
        if visit.said.len() > 1 {
            p.session.flagged = true;
            p.session.reason = Some(Reason::SourceDisagrees);
            continue;
        }
        let Some(said) = visit.said.first() else {
            continue;
        };
        let Some(months) = heard(said) else {
            // Not a label this scheme writes, so it is not evidence about
            // months. Silence is right: an archive may name its folders
            // anything at all.
            continue;
        };
        match p.session.offset_months {
            Some(exact) if (exact - months as f64).abs() <= tolerance => {
                p.session.label = Some(label_for(months));
                // On the schedule if the source named a nominal visit, which
                // is what tells a reader this was a planned timepoint rather
                // than a scan that merely happened.
                p.session.nominal = cadence.contains(&months).then_some(months);
                p.from_source = true;
            }
            Some(_) => {
                p.session.flagged = true;
                p.session.reason = Some(Reason::SourceDisagrees);
            }
            None => {
                p.session.label = Some(label_for(months));
                p.session.months = Some(months.abs());
                p.session.nominal = cadence.contains(&months).then_some(months);
                p.from_source = true;
                // The flag and its reason stay: nothing checked this.
            }
        }
    }
}

fn label_for(months: i32) -> String {
    if months < 0 {
        format!("PRE{:02}", -months)
    } else {
        format!("M{months:02}")
    }
}

/// §3c: when two cadence points are both in reach, prefer the free one.
///
/// A schedule should fill rather than collide. If one session is plainly the
/// three-month visit and another sits between three and six, the second is the
/// six-month visit, and saying so is better than putting both on `M03` and
/// letting the collision policy sort it out.
///
/// The clearest claims are settled first, so the outcome does not depend on
/// the order visits happen to sit in. Under the default cadence this never
/// fires: nominals six months apart with a tolerance of one leave no session
/// within reach of two.
fn relax(placed: &mut [Placed], scheme: &Scheme) {
    let Naming::Months { cadence, tolerance } = &scheme.naming else {
        return;
    };
    let mut order: Vec<usize> = (0..placed.len()).collect();
    order.sort_by(|a, b| claim(&placed[*a]).partial_cmp(&claim(&placed[*b])).unwrap());

    let mut held: Vec<i32> = Vec::new();
    for i in order {
        let p = &placed[i];
        if p.from_source || p.pre_anchor {
            continue;
        }
        let (Some(nominal), Some(exact)) = (p.session.nominal, p.session.offset_months) else {
            // A session off the schedule keeps its own real month and is not
            // competing for a slot.
            continue;
        };
        if !held.contains(&nominal) {
            held.push(nominal);
            continue;
        }
        // Somebody nearer holds it. Take the nearest slot still in reach.
        let free = cadence
            .iter()
            .copied()
            .filter(|n| !held.contains(n) && (exact - *n as f64).abs() <= *tolerance)
            .min_by(|a, b| {
                let (da, db) = ((exact - *a as f64).abs(), (exact - *b as f64).abs());
                da.partial_cmp(&db).unwrap().then(a.cmp(b))
            });
        if let Some(n) = free {
            held.push(n);
            placed[i].session.nominal = Some(n);
            placed[i].session.label = Some(label_for(n));
        }
    }
}

/// §4: two sessions that want one label.
fn settle(placed: Vec<Placed>, scheme: &Scheme) -> Vec<Session> {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, p) in placed.iter().enumerate() {
        // A session with no anchor never takes part: it carries no month to be
        // demoted to, and under `LabelUnknown` sharing one label is the point.
        if p.session.reason == Some(Reason::NoAnchor) {
            continue;
        }
        if let Some(label) = &p.session.label {
            groups.entry(label.clone()).or_default().push(i);
        }
    }

    let mut out: Vec<Option<Session>> = placed.iter().map(|_| None).collect();
    let mut demoted: Vec<(String, usize)> = Vec::new();
    let mut taken: Vec<String> = Vec::new();

    for (label, members) in &groups {
        if members.len() == 1 || scheme.collision == Collision::Merge {
            // Merge takes the whole group: sharing the label IS the outcome,
            // and it is not an anomaly, so nothing is flagged.
            taken.push(label.clone());
            for &i in members {
                out[i] = Some(placed[i].session.clone());
            }
        } else if scheme.collision == Collision::AlwaysDate {
            for &i in members {
                out[i] = Some(collided(&placed[i].session));
            }
        } else {
            // Closest to the nominal keeps it, earliest first. A session
            // labelled by its own real month matches itself perfectly, so it
            // claims with distance zero and the date decides.
            let mut ranked = members.clone();
            ranked.sort_by(|a, b| claim(&placed[*a]).partial_cmp(&claim(&placed[*b])).unwrap());
            taken.push(label.clone());
            out[ranked[0]] = Some(placed[ranked[0]].session.clone());
            demoted.extend(ranked.into_iter().skip(1).map(|i| (label.clone(), i)));
        }
    }

    // Anything not in a group keeps what it has: a session with no anchor
    // never contests a label.
    for (i, p) in placed.iter().enumerate() {
        if out[i].is_none() && !demoted.iter().any(|(_, j)| *j == i) {
            out[i] = Some(p.session.clone());
        }
    }

    // Demotions come last, so one can never steal a label a winner already
    // holds, and in a fixed order, so which loser gets first refusal on a free
    // fallback does not depend on how the groups happened to be walked.
    demoted.sort_by(|(la, a), (lb, b)| {
        la.cmp(lb)
            .then_with(|| claim(&placed[*a]).partial_cmp(&claim(&placed[*b])).unwrap())
    });
    for (_, i) in demoted {
        out[i] = Some(demote(&placed[i], scheme, &mut taken));
    }

    out.into_iter()
        .map(|s| s.expect("every session settled"))
        .collect()
}

fn claim(p: &Placed) -> (f64, i64) {
    let distance = match (p.session.nominal, p.session.offset_months) {
        (Some(n), Some(exact)) => (exact - n as f64).abs(),
        _ => 0.0,
    };
    (distance, p.session.first.to_days())
}

fn demote(p: &Placed, scheme: &Scheme, taken: &mut Vec<String>) -> Session {
    let session = &p.session;
    if let Some(months) = session.months {
        // A pre-anchor session demotes inside the pre-anchor namespace. v0
        // demotes it to `M06`, which moves a scan six months BEFORE the anchor
        // onto the label of a visit six months after it; the demote policies
        // are opt-in and little used, which is why nobody hit it. Its own
        // label is the one it just lost, so it goes on to the suffix or comes
        // back unlabelled, which is the honest answer.
        let own = if p.pre_anchor {
            format!("PRE{months:02}")
        } else {
            format!("M{months:02}")
        };
        if !taken.contains(&own) {
            taken.push(own.clone());
            return Session {
                label: Some(own),
                nominal: None,
                flagged: true,
                reason: Some(Reason::Demoted),
                ..session.clone()
            };
        }
        if scheme.collision == Collision::DemoteThenSuffix {
            for suffix in suffixes() {
                let candidate = format!("{own}{suffix}");
                if !taken.contains(&candidate) {
                    taken.push(candidate.clone());
                    return Session {
                        label: Some(candidate),
                        nominal: None,
                        flagged: true,
                        reason: Some(Reason::Collision),
                        ..session.clone()
                    };
                }
            }
        }
    }
    collided(session)
}

fn collided(session: &Session) -> Session {
    Session {
        label: None,
        // The nominal is dropped: it is set only when the session holds it.
        nominal: None,
        flagged: true,
        reason: Some(Reason::Collision),
        ..session.clone()
    }
}

/// `b`, `c`, then `aa`, `ab`: the base label is implicitly `a`.
fn suffixes() -> impl Iterator<Item = String> {
    let single = ('b'..='z').map(|c| c.to_string());
    let double = ('a'..='z').flat_map(|a| ('a'..='z').map(move |b| format!("{a}{b}")));
    single.chain(double)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Day {
        Day::parse(s).unwrap()
    }

    fn studies(days: &[&str]) -> Vec<Study> {
        days.iter()
            .enumerate()
            .map(|(i, s)| Study::new(i as i64 + 1, d(s)))
            .collect()
    }

    /// The same, with what the source called each visit.
    fn told(pairs: &[(&str, &str)]) -> Vec<Study> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, (day, said))| Study {
                said: (!said.is_empty()).then(|| (*said).to_string()),
                ..Study::new(i as i64 + 1, d(day))
            })
            .collect()
    }

    fn months(cadence: &[i32], tolerance: f64) -> Scheme {
        Scheme {
            naming: Naming::Months {
                cadence: cadence.to_vec(),
                tolerance,
            },
            ..Scheme::default()
        }
    }

    #[test]
    fn a_window_makes_two_studies_one_visit() {
        let s = studies(&["20221001", "20221004"]);
        // v0's grouping: a session is a calendar day.
        assert_eq!(sessions(&s, None, &Scheme::default()).len(), 2);
        // and the seam it left, filled: a brain study and a spine study three
        // days apart are one visit to everybody involved.
        let wide = Scheme {
            window_days: 14,
            ..Scheme::default()
        };
        let one = sessions(&s, None, &wide);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].studies, [1, 2]);
        assert_eq!(one[0].first.to_string(), "2022-10-01");
        assert_eq!(one[0].last.to_string(), "2022-10-04");
    }

    #[test]
    fn the_window_is_measured_from_the_first_study_not_the_last() {
        // Four studies a fortnight apart in a chain. Compared each to the
        // previous one they are one session forty-two days long, which is not
        // a visit anybody had. Measured from the first, a session can never
        // outrun its own window.
        let s = studies(&["20220101", "20220115", "20220129", "20220212"]);
        let out = sessions(
            &s,
            None,
            &Scheme {
                window_days: 14,
                ..Scheme::default()
            },
        );
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| s.first.days_to(s.last) <= 14), "bounded");
        assert_eq!(out[0].studies, [1, 2]);
    }

    #[test]
    fn the_order_studies_arrive_in_changes_nothing() {
        let forward = studies(&["20220101", "20220703", "20221002", "20230101"]);
        let mut backward = forward.clone();
        backward.reverse();
        let scheme = months(&[0, 6, 12, 18, 24], 1.0);
        let a = sessions(&forward, Some(d("20220101")), &scheme);
        let b = sessions(&backward, Some(d("20220101")), &scheme);
        assert_eq!(a, b);
    }

    #[test]
    fn a_cadence_snaps_what_is_close_and_leaves_what_is_not() {
        let s = studies(&["20220101", "20220703", "20221002", "20230101"]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12, 18, 24], 1.0));
        let labels: Vec<&str> = out.iter().map(|s| s.label.as_deref().unwrap()).collect();
        assert_eq!(labels, ["M00", "M06", "M09", "M12"]);
        // the ninth month kept its own month, and says so by having no nominal
        assert_eq!(out[2].nominal, None, "off schedule");
        assert_eq!(out[1].nominal, Some(6), "on it");
    }

    #[test]
    fn a_tolerance_is_a_fraction() {
        // Seven months and a bit: out of reach at 1.0, within it at 1.5.
        let s = studies(&["20220810"]);
        let anchor = Some(d("20220101"));
        let tight = sessions(&s, anchor, &months(&[0, 6, 12], 1.0));
        assert_eq!(tight[0].label.as_deref(), Some("M07"));
        let loose = sessions(&s, anchor, &months(&[0, 6, 12], 1.5));
        assert_eq!(loose[0].label.as_deref(), Some("M06"));
        assert_eq!(loose[0].nominal, Some(6));
    }

    #[test]
    fn a_session_before_its_anchor_is_pre_and_never_snaps() {
        // Under a diagnosis anchor this is a tenth of the live archive.
        let s = studies(&["20211201", "20220601"]);
        let out = sessions(&s, Some(d("20220601")), &months(&[0, 6, 12], 1.0));
        assert_eq!(out[0].label.as_deref(), Some("PRE06"));
        assert_eq!(out[0].reason, Some(Reason::PreAnchor));
        assert!(out[0].flagged);
        assert_eq!(out[1].label.as_deref(), Some("M00"));
        // never M-06: a hyphen is BIDS's key-value separator
        assert!(!out[0].label.as_deref().unwrap().contains('-'));
    }

    #[test]
    fn two_sessions_may_share_a_label_and_nothing_is_wrong() {
        // A continuation scan a month later, both landing on M06.
        let s = studies(&["20220701", "20220715"]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12], 1.0));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label.as_deref(), Some("M06"));
        assert_eq!(out[1].label.as_deref(), Some("M06"));
        assert!(!out[0].flagged && !out[1].flagged, "sharing is the outcome");
    }

    #[test]
    fn a_cohort_that_needs_one_session_per_label_can_ask_for_it() {
        // Six months, and seven months at the edge of the tolerance: both want
        // M06, and only one may have it.
        let s = studies(&["20220701", "20220801"]);
        let scheme = Scheme {
            collision: Collision::DemoteThenDate,
            ..months(&[0, 6, 12], 1.0)
        };
        let out = sessions(&s, Some(d("20220101")), &scheme);
        assert_eq!(out[0].label.as_deref(), Some("M06"), "closest keeps it");
        assert_eq!(
            out[1].label.as_deref(),
            Some("M07"),
            "the other takes its own"
        );
        assert_eq!(out[1].reason, Some(Reason::Demoted));
        assert_eq!(out[1].nominal, None, "and is no longer on the schedule");
    }

    #[test]
    fn a_demotion_with_nowhere_to_go_says_so() {
        // Both round to month six, so the loser's own month is the label it
        // just lost and there is nothing to fall back to.
        let s = studies(&["20220701", "20220715"]);
        let anchor = Some(d("20220101"));
        let base = months(&[0, 6, 12], 1.0);

        let out = sessions(
            &s,
            anchor,
            &Scheme {
                collision: Collision::DemoteThenDate,
                ..base.clone()
            },
        );
        assert_eq!(out[0].label.as_deref(), Some("M06"));
        assert_eq!(out[1].label, None, "the caller falls back to the date");
        assert_eq!(out[1].reason, Some(Reason::Collision));

        let out = sessions(
            &s,
            anchor,
            &Scheme {
                collision: Collision::DemoteThenSuffix,
                ..base.clone()
            },
        );
        assert_eq!(
            out[1].label.as_deref(),
            Some("M06b"),
            "the base is implicitly a"
        );

        let out = sessions(
            &s,
            anchor,
            &Scheme {
                collision: Collision::AlwaysDate,
                ..base
            },
        );
        assert!(out.iter().all(|s| s.label.is_none()), "nobody keeps it");
    }

    #[test]
    fn a_pre_anchor_session_never_demotes_across_its_anchor() {
        // Two workup scans six months before diagnosis. v0 hands the loser
        // `M06`, which is the label of a visit six months AFTER the anchor.
        let s = studies(&["20211201", "20211215"]);
        let scheme = Scheme {
            collision: Collision::DemoteThenSuffix,
            ..months(&[0, 6, 12], 1.0)
        };
        let out = sessions(&s, Some(d("20220601")), &scheme);
        assert_eq!(out[0].label.as_deref(), Some("PRE06"));
        assert_eq!(out[1].label.as_deref(), Some("PRE06b"));
        assert!(
            out.iter()
                .all(|s| s.label.as_deref().unwrap().starts_with("PRE")),
            "a demotion stays on its own side of month zero"
        );
    }

    #[test]
    fn the_closest_session_keeps_the_label_not_the_earliest() {
        // Both sit one rounded month from six, so v0's integer distance ties
        // and hands the label to whichever came first. Measured exactly, the
        // July visit is 0.62 months out and the June one 0.70: July is the
        // six-month visit, and it keeps the label.
        let s = studies(&["20220610", "20220720"]);
        let scheme = Scheme {
            collision: Collision::DemoteThenDate,
            ..months(&[6], 1.5)
        };
        let out = sessions(&s, Some(d("20220101")), &scheme);
        assert_eq!(
            out[0].label.as_deref(),
            Some("M05"),
            "the earlier one gives way"
        );
        assert_eq!(out[1].label.as_deref(), Some("M06"));
    }

    #[test]
    fn with_no_anchor_a_scheme_says_so_rather_than_guessing() {
        let s = studies(&["20220101", "20220701"]);
        let out = sessions(&s, None, &months(&[0, 6, 12], 1.0));
        assert!(out.iter().all(|s| s.label.is_none()));
        assert!(out.iter().all(|s| s.reason == Some(Reason::NoAnchor)));
        // and a cohort that would rather see a label than a gap
        let named = Scheme {
            unmatched: Unmatched::LabelUnknown,
            ..months(&[0, 6, 12], 1.0)
        };
        let out = sessions(&s, None, &named);
        assert_eq!(out[0].label.as_deref(), Some("unknown"));
    }

    #[test]
    fn ordinals_need_no_anchor_at_all() {
        let s = studies(&["20220701", "20220101", "20230101"]);
        let out = sessions(
            &s,
            None,
            &Scheme {
                naming: Naming::Ordinal,
                ..Scheme::default()
            },
        );
        let labels: Vec<&str> = out.iter().map(|s| s.label.as_deref().unwrap()).collect();
        assert_eq!(
            labels,
            ["01", "02", "03"],
            "in date order, not arrival order"
        );
    }

    #[test]
    fn a_date_label_is_the_first_study_of_the_visit() {
        let s = studies(&["20221004", "20221001"]);
        let out = sessions(
            &s,
            None,
            &Scheme {
                window_days: 14,
                ..Scheme::default()
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label.as_deref(), Some("20221001"));
    }

    #[test]
    fn how_far_off_a_label_is_stays_readable() {
        // An M06 that is really 7.4 months out is not the same fact as one at
        // six, and a reader can tell.
        let s = studies(&["20220812"]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12], 2.0));
        assert_eq!(out[0].label.as_deref(), Some("M06"));
        let off = out[0].offset_months.unwrap();
        assert!((off - 7.4).abs() < 0.1, "{off}");
    }

    #[test]
    fn a_source_label_that_agrees_wins_because_it_has_a_witness() {
        let s = told(&[("20220101", "ses-M00"), ("20220703", "M06")]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12], 1.0));
        assert_eq!(out[1].label.as_deref(), Some("M06"));
        assert!(!out[1].flagged, "the dates say the same thing");
        assert_eq!(out[1].nominal, Some(6), "and it is a planned timepoint");
    }

    #[test]
    fn a_source_label_the_dates_refuse_is_a_finding_not_a_relabelling() {
        // The source says twelve months; the scan is six months out.
        let s = told(&[("20220101", "M00"), ("20220703", "M12")]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12], 1.0));
        assert_eq!(out[1].label.as_deref(), Some("M06"), "the dates stand");
        assert_eq!(out[1].reason, Some(Reason::SourceDisagrees));
    }

    #[test]
    fn an_archive_that_is_a_fragment_says_so() {
        // Only the six- and twelve-month visits were copied. Under a
        // first_session anchor the earliest is M00, which is true of what we
        // hold and wrong about the subject, so both readings are reported.
        let s = told(&[("20220701", "M06"), ("20230101", "M12")]);
        let out = sessions(&s, Some(d("20220701")), &months(&[0, 6, 12, 18], 1.0));
        assert_eq!(out[0].label.as_deref(), Some("M00"), "true of our holdings");
        assert_eq!(out[0].reason, Some(Reason::SourceDisagrees), "and reported");
        assert_eq!(out[1].label.as_deref(), Some("M06"));
        assert_eq!(out[1].reason, Some(Reason::SourceDisagrees));
    }

    #[test]
    fn a_fragment_can_take_month_zero_from_its_own_labels() {
        // The same archive, asked to believe its folders. Month zero moves six
        // months before the earliest scan we hold, and everything lines up.
        let s = told(&[("20220701", "M06"), ("20230101", "M12")]);
        let scheme = Scheme {
            anchor: Anchor::SourceLabel,
            ..months(&[0, 6, 12, 18], 1.0)
        };
        let out = sessions(&s, None, &scheme);
        assert_eq!(out[0].label.as_deref(), Some("M06"));
        assert_eq!(out[1].label.as_deref(), Some("M12"));
        assert!(out.iter().all(|s| !s.flagged), "nothing disagrees any more");
    }

    #[test]
    fn labels_nobody_wrote_leave_the_anchor_unfound() {
        let s = told(&[("20220701", "scan1"), ("20230101", "scan2")]);
        let scheme = Scheme {
            anchor: Anchor::SourceLabel,
            ..months(&[0, 6, 12], 1.0)
        };
        let out = sessions(&s, None, &scheme);
        assert!(out.iter().all(|s| s.reason == Some(Reason::NoAnchor)));
    }

    #[test]
    fn a_folder_name_that_is_not_a_label_is_not_evidence() {
        let s = told(&[("20220101", "MRI_BRAIN"), ("20220703", "visit 2")]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12], 1.0));
        assert_eq!(out[1].label.as_deref(), Some("M06"));
        assert!(
            out.iter().all(|s| !s.flagged),
            "an archive may name folders anything"
        );
    }

    #[test]
    fn two_labels_inside_one_visit_mean_the_window_is_too_wide() {
        let s = told(&[("20221001", "M06"), ("20221004", "M12")]);
        let scheme = Scheme {
            window_days: 14,
            ..months(&[0, 6, 12], 1.0)
        };
        let out = sessions(&s, Some(d("20220401")), &scheme);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].reason, Some(Reason::SourceDisagrees));
        assert!(out[0].flagged);
    }

    #[test]
    fn a_schedule_fills_rather_than_collides() {
        // Under a three-month cadence with a wide tolerance, the second visit
        // is within reach of both M03 and M06. The first is plainly M03, so
        // the second is the six-month visit.
        let s = studies(&["20220401", "20220520"]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 3, 6], 2.0));
        assert_eq!(out[0].label.as_deref(), Some("M03"));
        assert_eq!(out[1].label.as_deref(), Some("M06"));
        assert_eq!(out[1].nominal, Some(6));
        assert!(!out[1].flagged, "it was placed, not demoted");
    }

    #[test]
    fn the_default_cadence_never_relaxes() {
        // Six months apart with a tolerance of one leaves nothing in reach of
        // two nominals, so the pass is inert where it matters most.
        let s = studies(&["20220701", "20220715"]);
        let out = sessions(&s, Some(d("20220101")), &months(&[0, 6, 12, 18, 24], 1.0));
        assert!(out.iter().all(|s| s.label.as_deref() == Some("M06")));
    }

    #[test]
    fn a_label_is_read_the_way_it_is_written() {
        assert_eq!(heard("M06"), Some(6));
        assert_eq!(heard("ses-M06"), Some(6));
        assert_eq!(heard("M6"), Some(6));
        assert_eq!(heard("PRE06"), Some(-6));
        assert_eq!(heard("ses-PRE12"), Some(-12));
        assert_eq!(heard("M"), None);
        assert_eq!(heard("MRI"), None);
        assert_eq!(heard("baseline"), None);
        assert_eq!(heard("20220101"), None);
    }

    #[test]
    fn a_scheme_is_written_in_yaml_and_a_short_one_is_enough() {
        let scheme = Scheme::parse("session:\n  window_days: 30\n").unwrap();
        assert_eq!(scheme.window_days, 30);
        assert_eq!(scheme.naming, Naming::Date, "the rest is v0's behaviour");
        assert_eq!(scheme.collision, Collision::Merge);

        let full = Scheme::parse(
            r"
session:
  window_days: 30
  anchor: source_label
  naming:
    months:
      cadence: [0, 6, 12, 24]
      tolerance: 1.5
  collision: demote_then_suffix
  unmatched: label_unknown
  said:
    segment: 2
    pattern: '^(?<label>M[0-9]+)$'
",
        )
        .unwrap();
        assert_eq!(full.anchor, Anchor::SourceLabel);
        assert_eq!(
            full.naming,
            Naming::Months {
                cadence: vec![0, 6, 12, 24],
                tolerance: 1.5
            }
        );
        assert_eq!(full.said.as_ref().unwrap().segment, 2);
    }

    #[test]
    fn a_scheme_that_could_make_the_resolver_lie_is_refused() {
        let err = |yaml: &str| Scheme::parse(yaml).unwrap_err().to_string();
        assert!(err("session:\n  window_days: -1\n").contains("window"));
        assert!(
            err("session:\n  naming:\n    months:\n      cadence: [6, 0]\n      tolerance: 1\n")
                .contains("ascending")
        );
        assert!(
            err("session:\n  naming:\n    months:\n      cadence: [0, -6]\n      tolerance: 1\n")
                .contains("PRE")
        );
        assert!(err("session:\n  said:\n    segment: 0\n").contains("from one"));
        assert!(
            err("session:\n  said:\n    segment: 1\n    pattern: 'M(.+)'\n").contains("`label`")
        );
        assert!(err("session:\n  nonsense: 1\n").contains("does not parse"));
    }

    #[test]
    fn a_scheme_round_trips_through_the_json_it_is_stored_as() {
        let scheme = Scheme {
            window_days: 30,
            anchor: Anchor::SourceLabel,
            said: Some(Said {
                segment: 2,
                pattern: None,
            }),
            ..months(&[0, 6, 12], 1.5)
        };
        let text = serde_json::to_string(&scheme).unwrap();
        assert_eq!(Scheme::from_json(&text).unwrap(), scheme);
    }

    /// Studies with what each one holds, for the rescue.
    fn held(pairs: &[(&str, Option<bool>)]) -> Vec<Study> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, (day, has))| Study {
                has_primary: *has,
                ..Study::new(i as i64 + 1, d(day))
            })
            .collect()
    }

    #[test]
    fn a_visit_holds_a_primary_if_any_of_its_studies_does() {
        // The case the rescue exists for: an export that tags every
        // reconstruction ORIGINAL\SECONDARY and never writes a primary.
        let none = held(&[("20220901", Some(false))]);
        assert_eq!(
            sessions(&none, None, &Scheme::default())[0].has_primary,
            Some(false)
        );

        let some = held(&[("20220901", Some(true))]);
        assert_eq!(
            sessions(&some, None, &Scheme::default())[0].has_primary,
            Some(true)
        );
    }

    #[test]
    fn the_window_decides_whether_a_session_needs_rescuing() {
        // A brain study on the Monday with no primary anywhere in it, and a
        // spine study on the Wednesday that has one. Under v0's grouping they
        // are two occasions and the brain study is rescued; widen the window
        // and they are one visit that holds a primary, so nothing is.
        let s = held(&[("20221003", Some(false)), ("20221005", Some(true))]);

        let apart = sessions(&s, None, &Scheme::default());
        assert_eq!(apart.len(), 2);
        assert_eq!(apart[0].has_primary, Some(false), "rescue");

        let together = sessions(
            &s,
            None,
            &Scheme {
                window_days: 14,
                ..Scheme::default()
            },
        );
        assert_eq!(together.len(), 1);
        assert_eq!(together[0].has_primary, Some(true), "no rescue");
    }

    #[test]
    fn a_study_that_has_not_been_asked_leaves_the_question_open() {
        // Null is not no. A rescue turns on there being no primary anywhere,
        // and a study whose stacks are only partly fingerprinted has not said
        // there is none: it has said it does not know.
        let s = held(&[("20221003", Some(false)), ("20221005", None)]);
        let one = sessions(
            &s,
            None,
            &Scheme {
                window_days: 14,
                ..Scheme::default()
            },
        );
        assert_eq!(one[0].has_primary, None);

        // but one yes still answers, whatever else is unknown
        let s = held(&[("20221003", None), ("20221005", Some(true))]);
        let one = sessions(
            &s,
            None,
            &Scheme {
                window_days: 14,
                ..Scheme::default()
            },
        );
        assert_eq!(one[0].has_primary, Some(true));
    }
}
