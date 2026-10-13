// SPDX-License-Identifier: AGPL-3.0-only

//! The post-contrast label (record 55 F2 and F5, `docs/specs/wave7a-ready-for-production.md`
//! §12.3): the time rule as a session pass, its checks against the eight
//! failure modes, and the subject-level step after it.
//!
//! **The rule** (Nima's, 2026-10-05): in a session whose T1-family stacks
//! carry certified answers, every stack acquired after the first
//! post-contrast T1 is `post`, and every stack acquired before the last
//! pre-contrast T1 is `pre`. A certified answer is the image model's at or
//! above its card's threshold, or a person's settled read. A session with no
//! T1 answer, or with only a post T1 and a stack before it, is
//! `undetermined`, never `pre`.
//!
//! **Its checks** run as code, each with a reason code, and a check that
//! fires makes a labelled stack `undetermined`. They are the time-label
//! study's (`studies/2026-10-08-postcontrast-time-label`, plan section 4),
//! as registered there:
//!
//! | code | failure mode | here |
//! |---|---|---|
//! | `C1_prior_72h` | an earlier injection, the same day or days before | the subject step: a study of the subject 0 to 72 hours earlier with an MR contrast event makes this study's `pre` labels `undetermined`, and flags every stack |
//! | `C2_exam` | two exams in one session | head and spine stacks in one session: the rule runs per exam, and a label the exam's own bracket does not give is `undetermined`; another MR study of the subject within 24 hours is a flag (`C2_second_study`) |
//! | `C3_bolus_before_L` | bolus and test-bolus runs | a DSC, DCE or contrast MRA series before the last pre T1 contradicts the bracket: every label of the session is `undetermined`. Otherwise stacks after the bolus that the rule left unknown are `post` (`basis` bolus), and the bolus start is the injection time |
//! | `C4_split` | a split dose or a preload | a flag: two bolus series five minutes or more apart, or a bolus after the first post T1, and the stacks between the two events |
//! | `C5_midnight`, `C5_no_time`, `C5_content`, `C5_straddle`, `C5_sn_guard` | clock errors | more than one acquisition date in the session; a stack with images that carry no acquisition time; content time before acquisition time by more than a minute, on the stack or on its bracket stack; a stack whose images reach across its bracket stack's first image; the clock and the series number disagreeing |
//! | `C6_derived` | derived series carrying a later time | a derived or reformatted stack: the registry holds no Source or Referenced Image Sequence, so its source cannot be resolved and it is `undetermined` |
//! | `C7_no_t1`, `C7_post_only_before_F` | sessions without a usable T1 | `undetermined`, never `pre` |
//! | `C8_weak_F` | a mislabelled post T1 | a flag on a `post` label whose first post T1 rests on one model answer that nothing else in the session supports (no second post answer, no reader, no contrast tag on it or after it, no bolus before it) |
//!
//! **By definition:** DSC, DCE and contrast MR angiography are `by_definition`
//! (F5). PSIR, vessel-wall T1 and PD stacks count as the T1 family for the
//! image model, so their certified answers are anchors and they keep them.
//!
//! **The minutes since injection** are counted from the bolus start where a
//! bolus series comes before the first post T1, or the session has none,
//! and otherwise from the first post T1's first image (Nima, 2026-10-11, the
//! time-label plan's T1; spec §20 question 6), with which one recorded.
//!
//! The 72-hour flag needs the subject's earlier sessions, which a session
//! pass cannot see, so [`subject`] runs after [`session`] over every session
//! of one subject. A pure function of what it is handed: the registry's side
//! is `nils_classify::contrast`.

use std::collections::{BTreeMap, BTreeSet};

/// The rule's version, written beside every label it makes.
pub const RULE: &str = "time-rule-1";

/// Every number the rule and its checks use, as the time-label study
/// registered them (plan sections 4 and 5) and as the time-rule study's
/// trust guard has them (`2026-10-05-postcontrast-time-rule`, 1.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// C1: how far back an earlier injection counts, in minutes (72 h).
    pub lookback_minutes: f64,
    /// C1 and C2: another study within this many minutes is the same visit (24 h).
    pub same_visit_minutes: f64,
    /// C4: two bolus series at least this many seconds apart are two events.
    pub split_gap_seconds: f64,
    /// C5: content time before acquisition time by more than this many seconds.
    pub content_slack_seconds: f64,
    /// The trust guard: a session spans at least this many seconds,
    pub trust_min_span_seconds: f64,
    /// and at most this many,
    pub trust_max_span_seconds: f64,
    /// with at least this share of its series at distinct times,
    pub trust_distinct_share: f64,
    /// at least this many series,
    pub trust_min_series: usize,
    /// and its times and series numbers agreeing at least this well
    /// (Spearman), where three or more series carry both.
    pub trust_min_spearman: f64,
    /// The physics guard of the T1 family (model r2, part 6): a short
    /// inversion time below this at 1.5 T and below,
    pub ti_max_low: f64,
    /// at 3 T,
    pub ti_max_3t: f64,
    /// and at 7 T, in milliseconds, is no T1 contrast;
    pub ti_max_7t: f64,
    /// nor an echo time above this outside the gradient-echo family,
    pub te_max_spin_echo: f64,
    /// nor one above this in a gradient echo with no inversion,
    pub te_max_gradient_echo: f64,
    /// nor a 2D acquisition with no inversion and a repetition time above this.
    pub tr_max_2d: f64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            lookback_minutes: 4320.0,
            same_visit_minutes: 1440.0,
            split_gap_seconds: 300.0,
            content_slack_seconds: 60.0,
            trust_min_span_seconds: 120.0,
            trust_max_span_seconds: 4.0 * 3600.0,
            trust_distinct_share: 0.5,
            trust_min_series: 2,
            trust_min_spearman: 0.8,
            ti_max_low: 250.0,
            ti_max_3t: 350.0,
            ti_max_7t: 450.0,
            te_max_spin_echo: 40.0,
            te_max_gradient_echo: 15.0,
            tr_max_2d: 1500.0,
        }
    }
}

/// What a stack is, for the rule (the time-rule study's families, 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    T1,
    Flair,
    T2,
    Dwi,
    SwiT2star,
    Pd,
    CeMra,
    DscDce,
    Other,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::T1 => "T1-type",
            Family::Flair => "FLAIR",
            Family::T2 => "T2-type",
            Family::Dwi => "DWI",
            Family::SwiT2star => "SWI/T2*",
            Family::Pd => "PD",
            Family::CeMra => "CE-MRA",
            Family::DscDce => "DSC/DCE",
            Family::Other => "other",
        }
    }

    /// DSC, DCE and contrast MRA: a bolus, and contrast by definition.
    pub fn bolus(self) -> bool {
        matches!(self, Family::CeMra | Family::DscDce)
    }
}

/// What the pack decided of a stack and the physics it was acquired with,
/// as [`kind_of`] reads them: axis values as the MRI pack spells them.
#[derive(Debug, Clone, Default)]
pub struct Described<'a> {
    pub base: Option<&'a str>,
    pub modifiers: &'a [String],
    pub technique: Option<&'a str>,
    /// Whether the technique is of the pack's gradient-echo family, or unknown.
    pub gradient_echo: bool,
    /// Repetition, echo and inversion time, in milliseconds.
    pub tr: Option<f64>,
    pub te: Option<f64>,
    pub ti: Option<f64>,
    pub field_tesla: Option<f64>,
    /// `2D` or `3D`.
    pub acquisition: Option<&'a str>,
}

/// A stack's family and the kinds the label reports apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub family: Family,
    /// Whether the image model answers it: T1-type, PSIR, vessel-wall T1, PD.
    pub t1_family: bool,
    pub asl: bool,
    pub quantitative_map: bool,
    pub tof: bool,
}

/// The family of a stack from its axes and physics (the time-rule study's
/// table of 1.1, with the physics guard of model r2's part 6), and whether
/// the image model counts it as the T1 family (F5: PSIR, vessel-wall T1 and
/// PD as well).
pub fn kind_of(cfg: &Config, d: &Described<'_>) -> Kind {
    let tech = d.technique.unwrap_or("");
    let has = |m: &str| d.modifiers.iter().any(|x| x == m);
    let angio_flow = matches!(tech, "TOF-MRA" | "PC-MRA");
    let base = d.base.unwrap_or("");
    let family = if matches!(tech, "Perfusion-EPI" | "DCE") {
        Family::DscDce
    } else if tech == "CE-MRA" {
        Family::CeMra
    } else if base == "T1w" && !angio_flow && !not_t1(cfg, d) {
        Family::T1
    } else if has("FLAIR") && !angio_flow {
        Family::Flair
    } else if base == "T2w" && !angio_flow {
        Family::T2
    } else if base == "DWI" {
        Family::Dwi
    } else if matches!(base, "T2*w" | "T2starw" | "SWI") {
        Family::SwiT2star
    } else if base == "PDw" {
        Family::Pd
    } else {
        Family::Other
    };
    let t1_family = family == Family::T1
        || family == Family::Pd
        || (!family.bolus()
            && !angio_flow
            && (has("PSIR") || (base == "T1w" && has("BlackBlood"))));
    Kind {
        family,
        t1_family,
        asl: tech == "ASL",
        quantitative_map: matches!(tech, "MDME" | "QALAS"),
        tof: tech == "TOF-MRA",
    }
}

/// Model r2's physics guard (part 6): a short-TI inversion, a long echo or
/// a long-TR 2D acquisition with no inversion is no T1 contrast.
fn not_t1(cfg: &Config, d: &Described<'_>) -> bool {
    let ti = d.ti.unwrap_or(0.0);
    let inverted = ti > 0.0;
    let field = d.field_tesla.unwrap_or(1.5);
    let ti_max = if field >= 5.0 {
        cfg.ti_max_7t
    } else if field >= 2.5 {
        cfg.ti_max_3t
    } else {
        cfg.ti_max_low
    };
    let te = d.te.unwrap_or(0.0);
    let tr = d.tr.unwrap_or(0.0);
    (inverted && ti < ti_max)
        || (!d.gradient_echo && te > cfg.te_max_spin_echo)
        || (d.gradient_echo && !inverted && te > cfg.te_max_gradient_echo)
        || (d.acquisition == Some("2D") && tr > cfg.tr_max_2d && !inverted)
}

/// Where a stack is, for C2: an exam of the head or of the spine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Region {
    Head,
    Spine,
    None,
}

impl Region {
    /// From the body part as the MRI pack spells it.
    pub fn of_body_part(value: Option<&str>) -> Region {
        match value {
            Some("brain" | "brain-neck" | "neck") => Region::Head,
            Some("spine") => Region::Spine,
            _ => Region::None,
        }
    }
}

/// Who gave a T1-family stack's certified answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The image model, at or above its card's threshold.
    Image,
    /// A person's settled read.
    Reader,
}

/// A certified answer on a T1-family stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    /// `given` (true) or `not given`.
    pub post: bool,
    pub by: By,
}

/// Why a stack is outside the rule's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outside {
    /// Acquired, and read by the rule.
    No,
    /// A localizer or a scout.
    Localizer,
    /// Derived or reformatted (C6).
    Derived,
    /// The pack excluded it.
    Excluded,
    /// Of a sample sealed now: never an anchor, never shown to a reader
    /// who may not read it.
    Sealed,
}

/// One stack of a session, as the rule reads it. Times are seconds on one
/// clock (days and seconds since an epoch, as the caller makes them).
#[derive(Debug, Clone)]
pub struct Scan {
    pub stack: i64,
    pub series: i64,
    pub series_number: Option<i64>,
    pub kind: Kind,
    pub outside: Outside,
    pub region: Region,
    /// The stack's earliest acquisition, the time it stands at on the clock.
    pub time: Option<f64>,
    /// Its images' earliest and latest acquisition times (the same clock).
    pub first: Option<f64>,
    pub last: Option<f64>,
    /// Its images, and how many of them carry an acquisition time.
    pub images: i64,
    pub timed: i64,
    /// How many of its images have a content time more than
    /// [`Config::content_slack_seconds`] before their acquisition time.
    pub content_early: i64,
    /// The distinct acquisition dates its images carry, as day numbers.
    pub dates: Vec<i64>,
    /// Whether its series carries a contrast agent.
    pub tagged: bool,
    /// Its certified answer, for a stack of the T1 family.
    pub answer: Option<Answer>,
}

/// One session (a study) and what the subject step needs of it.
#[derive(Debug, Clone)]
pub struct Study {
    pub id: i64,
    /// The study's own date and time in minutes on one clock; none where it
    /// has no date.
    pub at_minutes: Option<f64>,
    /// MR series, and MR and CT series that carry a contrast agent.
    pub mr_series: i64,
    pub mr_tagged: i64,
    pub ct_tagged: i64,
    pub scans: Vec<Scan>,
}

/// The post-contrast state (F5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Pre,
    Post,
    ByDefinition,
    Undetermined,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Pre => "pre",
            State::Post => "post",
            State::ByDefinition => "by_definition",
            State::Undetermined => "undetermined",
        }
    }
}

/// Where a label came from (F5's `contrast_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The session's clock against its T1 anchors (and the bolus, C3).
    T1AnchorTimeRule,
    /// The header: a DSC, DCE or contrast MRA series is contrast by definition.
    Header,
    /// The image model's own certified answer on the stack.
    Image,
    /// A person's settled read of the stack.
    Reader,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::T1AnchorTimeRule => "t1_anchor_time_rule",
            Source::Header => "header",
            Source::Image => "image",
            Source::Reader => "reader",
        }
    }
}

/// What a label rests on, beside its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// A T1-family stack's own certified answer.
    OwnAnswer,
    /// Placed by the session's first post or last pre T1.
    Bracket,
    /// Placed after the bolus (C3).
    Bolus,
    /// Its family (by definition).
    Family,
    /// Nothing placed it.
    None,
}

impl Basis {
    pub fn name(self) -> &'static str {
        match self {
            Basis::OwnAnswer => "own_answer",
            Basis::Bracket => "bracket",
            Basis::Bolus => "bolus",
            Basis::Family => "family",
            Basis::None => "none",
        }
    }
}

/// The 72-hour flag (F5): `yes`, `no`, or `unknown` where a date is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prior {
    Yes,
    No,
    Unknown,
}

impl Prior {
    pub fn name(self) -> &'static str {
        match self {
            Prior::Yes => "yes",
            Prior::No => "no",
            Prior::Unknown => "unknown",
        }
    }
}

/// Where the minutes are counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    BolusStart,
    FirstPostT1,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::BolusStart => "bolus_start",
            Origin::FirstPostT1 => "first_post_t1",
        }
    }
}

/// What the minutes were read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinutesBasis {
    /// First-image acquisition times.
    Instance,
    /// The stacks' own times, where an image time is missing.
    StackTime,
}

impl MinutesBasis {
    pub fn name(self) -> &'static str {
        match self {
            MinutesBasis::Instance => "instance",
            MinutesBasis::StackTime => "stack_time",
        }
    }
}

/// F5's error level by use, which the label is held to where it is used.
pub fn use_level(kind: &Kind) -> &'static str {
    if kind.family.bolus() {
        "by definition"
    } else if kind.t1_family {
        "T1 family"
    } else if kind.family == Family::Flair || kind.asl || kind.quantitative_map {
        "1 %"
    } else if kind.tof {
        "2 %"
    } else {
        "5 %"
    }
}

/// One stack's label (F5), with what made it.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub stack: i64,
    pub state: State,
    /// For `undetermined`: the check or the rule's reason; none otherwise.
    pub reason: Option<String>,
    pub basis: Basis,
    pub source: Source,
    pub minutes_since_injection: Option<f64>,
    pub injection_ref: Option<Origin>,
    pub minutes_basis: Option<MinutesBasis>,
    pub prior_72h: Prior,
    /// For an ASL stack: `yes` when acquired after the injection (invalid
    /// for perfusion), `no` before it, `unknown` otherwise.
    pub asl_after_contrast: Option<Prior>,
    pub weak_f: bool,
    /// Every check that fired on the stack, the flag-only ones included.
    pub flags: BTreeSet<&'static str>,
    pub kind: Kind,
    /// The session's first post and last pre T1, where it has them.
    pub first_post: Option<i64>,
    pub last_pre: Option<i64>,
    /// Whether the session has a T1 anchor: a certified answer on a stack of
    /// the T1 family.
    pub anchored: bool,
}

/// What the subject step reads of a session after its pass.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// A contrast event in the session: a certified post answer on a T1,
    /// a bolus series, or an MR contrast tag.
    pub contrast: bool,
}

/// A session's pass: its labels, and the facts the subject step reads.
#[derive(Debug, Clone)]
pub struct Passed {
    pub study: i64,
    pub labels: Vec<Label>,
    pub facts: Facts,
}

/// The rule's own reason for a stack it could not place, before any check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    NoTrustedTime,
    NoT1,
    Conflict,
    SeriesNumber,
    Window,
    PostOnlyBeforeF,
    PreOnlyAfterL,
}

impl Why {
    fn code(self) -> &'static str {
        match self {
            Why::NoTrustedTime => "R_no_trusted_time",
            Why::NoT1 => "C7_no_t1",
            Why::Conflict => "R_conflict",
            Why::SeriesNumber => "C5_sn_guard",
            Why::Window => "R_window",
            Why::PostOnlyBeforeF => "C7_post_only_before_F",
            Why::PreOnlyAfterL => "R_pre_only_after_L",
        }
    }
}

/// Where the rule put a stack before the checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placed {
    Pre,
    Post,
    Unknown(Why),
    Own,
}

/// A session's bracket: its first post and last pre T1 answers.
#[derive(Debug, Clone, Default)]
struct Bracket {
    first_post: Option<usize>,
    last_pre: Option<usize>,
    any: bool,
    conflict: bool,
}

fn earlier(a: (f64, Option<i64>), b: (f64, Option<i64>)) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1.unwrap_or(i64::MAX) < b.1.unwrap_or(i64::MAX))
}

/// The bracket over the answers of the scans `which` names (tr_rule.py's
/// `bracket`, unchanged): F the earliest post answer, L the latest pre, and
/// a conflict where the latest pre is not earlier than the earliest post.
fn bracket(scans: &[Scan], answers: &[usize], which: &dyn Fn(usize) -> bool) -> Bracket {
    let mut b = Bracket::default();
    for &i in answers.iter().filter(|&&i| which(i)) {
        let s = &scans[i];
        let (Some(a), Some(t)) = (s.answer, s.time) else {
            continue;
        };
        b.any = true;
        let key = (t, s.series_number);
        if a.post {
            let better = b.first_post.is_none_or(|f| {
                let f = &scans[f];
                earlier(key, (f.time.unwrap_or(f64::MAX), f.series_number))
            });
            if better {
                b.first_post = Some(i);
            }
        } else {
            // the latest pre: the last in time and series number
            let better = b.last_pre.is_none_or(|l| {
                let l = &scans[l];
                !earlier(key, (l.time.unwrap_or(f64::MIN), l.series_number))
            });
            if better {
                b.last_pre = Some(i);
            }
        }
    }
    if let (Some(f), Some(l)) = (b.first_post, b.last_pre) {
        b.conflict = scans[l].time >= scans[f].time;
    }
    b
}

/// tr_rule.py's `place`: a stack after F (and of a higher series number) is
/// post, before L (and lower) pre, else unknown with why.
fn place(s: &Scan, trusted: bool, b: &Bracket, scans: &[Scan]) -> Placed {
    if !trusted || s.time.is_none() {
        return Placed::Unknown(Why::NoTrustedTime);
    }
    if !b.any {
        return Placed::Unknown(Why::NoT1);
    }
    if b.conflict {
        return Placed::Unknown(Why::Conflict);
    }
    let t = s.time.unwrap_or_default();
    let f = b.first_post.map(|i| &scans[i]);
    let l = b.last_pre.map(|i| &scans[i]);
    let after = f.is_some_and(|f| f.time.is_some_and(|ft| t > ft));
    let before = l.is_some_and(|l| l.time.is_some_and(|lt| t < lt));
    let sn_ok_post = match (s.series_number, f.and_then(|f| f.series_number)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    };
    let sn_ok_pre = match (s.series_number, l.and_then(|l| l.series_number)) {
        (Some(a), Some(b)) => a < b,
        _ => true,
    };
    if after && sn_ok_post {
        Placed::Post
    } else if before && sn_ok_pre {
        Placed::Pre
    } else if after || before {
        Placed::Unknown(Why::SeriesNumber)
    } else if f.is_some() && l.is_some() {
        Placed::Unknown(Why::Window)
    } else if f.is_some() {
        Placed::Unknown(Why::PostOnlyBeforeF)
    } else {
        Placed::Unknown(Why::PreOnlyAfterL)
    }
}

/// Spearman's rank correlation, ties at their mean rank; none where either
/// side has a single value.
fn spearman(xs: &[f64], ys: &[f64]) -> Option<f64> {
    fn ranks(v: &[f64]) -> Vec<f64> {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|&a, &b| v[a].total_cmp(&v[b]));
        let mut r = vec![0.0; v.len()];
        let mut i = 0;
        while i < idx.len() {
            let mut j = i;
            while j + 1 < idx.len() && v[idx[j + 1]] == v[idx[i]] {
                j += 1;
            }
            let mean = (i + j) as f64 / 2.0 + 1.0;
            for &k in &idx[i..=j] {
                r[k] = mean;
            }
            i = j + 1;
        }
        r
    }
    let distinct = |v: &[f64]| v.iter().any(|x| *x != v[0]);
    if xs.len() < 3 || !distinct(xs) || !distinct(ys) {
        return None;
    }
    let (rx, ry) = (ranks(xs), ranks(ys));
    let n = rx.len() as f64;
    let (mx, my) = (rx.iter().sum::<f64>() / n, ry.iter().sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in rx.iter().zip(&ry) {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
        syy += (b - my) * (b - my);
    }
    Some(sxy / (sxx * syy).sqrt())
}

/// The trust guard of the time-rule study (1.2), over a session's acquired,
/// non-localizer series: every one has a time, none is exactly midnight,
/// the session spans 2 minutes to 4 hours, at least half the series are at
/// distinct times, it has two series or more, and time and series number
/// agree where three or more series carry both.
fn trusted(cfg: &Config, scans: &[Scan]) -> bool {
    let mut series: BTreeMap<i64, (Option<f64>, Option<i64>)> = BTreeMap::new();
    for s in scans.iter().filter(|s| s.outside == Outside::No) {
        let e = series.entry(s.series).or_insert((None, None));
        e.0 = match (e.0, s.time) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        e.1 = e.1.or(s.series_number);
    }
    let k = series.len();
    if k < cfg.trust_min_series {
        return false;
    }
    let times: Vec<f64> = series.values().filter_map(|(t, _)| *t).collect();
    if times.len() != k {
        return false;
    }
    if times.iter().any(|t| t.rem_euclid(86_400.0) == 0.0) {
        return false;
    }
    let lo = times.iter().copied().fold(f64::MAX, f64::min);
    let hi = times.iter().copied().fold(f64::MIN, f64::max);
    let span = hi - lo;
    if span < cfg.trust_min_span_seconds || span > cfg.trust_max_span_seconds {
        return false;
    }
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup();
    if (sorted.len() as f64) / (k as f64) < cfg.trust_distinct_share {
        return false;
    }
    let (xs, ys): (Vec<f64>, Vec<f64>) = series
        .values()
        .filter_map(|(t, n)| Some((t.as_ref().copied()?, *n.as_ref()? as f64)))
        .unzip();
    spearman(&xs, &ys).is_none_or(|rho| rho >= cfg.trust_min_spearman)
}

/// The session pass (record 55 F2): the rule and the checks that read one
/// session, C2 to C8. C1 and the 72-hour flag are [`subject`]'s.
pub fn session(cfg: &Config, study: &Study) -> Passed {
    let scans = &study.scans;
    let n = scans.len();
    let in_frame = |s: &Scan| s.outside == Outside::No;
    let trusted = trusted(cfg, scans);
    // the certified answers that count: on the T1 family, never sealed
    let answers: Vec<usize> = (0..n)
        .filter(|&i| {
            let s = &scans[i];
            s.kind.t1_family && s.answer.is_some() && s.outside != Outside::Sealed
        })
        .collect();
    let anchored = !answers.is_empty();
    // the bracket rests only on answers of a session whose clock is trusted
    let b = if trusted {
        bracket(scans, &answers, &|i| in_frame(&scans[i]))
    } else {
        Bracket {
            any: anchored,
            ..Bracket::default()
        }
    };
    let own: BTreeSet<usize> = answers.iter().copied().collect();
    let mut placed: Vec<Placed> = (0..n)
        .map(|i| {
            if own.contains(&i) {
                Placed::Own
            } else {
                place(&scans[i], trusted, &b, scans)
            }
        })
        .collect();
    let base = placed.clone();
    let mut reason: Vec<Option<&'static str>> = vec![None; n];
    let mut flags: Vec<BTreeSet<&'static str>> = vec![BTreeSet::new(); n];
    let labelled = |p: Placed| matches!(p, Placed::Pre | Placed::Post);
    let fire = |code: &'static str,
                mask: &dyn Fn(usize, Placed) -> bool,
                placed: &mut [Placed],
                reason: &mut [Option<&'static str>],
                flags: &mut [BTreeSet<&'static str>]| {
        for i in 0..n {
            if !in_frame(&scans[i]) || !mask(i, placed[i]) {
                continue;
            }
            flags[i].insert(code);
            if labelled(placed[i]) {
                reason[i].get_or_insert(code);
                placed[i] = Placed::Unknown(Why::NoTrustedTime);
                // the placeholder is never read: the reason decides
            }
        }
    };

    let f_scan = b.first_post.map(|i| &scans[i]);
    let l_scan = b.last_pre.map(|i| &scans[i]);
    let f_time = f_scan.and_then(|s| s.time);
    let l_time = l_scan.and_then(|s| s.time);

    // C3 first: it changes the bracket. The bolus is the session's first
    // DSC, DCE or contrast MRA series of trusted time.
    let bolus: Vec<&Scan> = scans
        .iter()
        .filter(|s| in_frame(s) && s.kind.family.bolus() && s.time.is_some())
        .collect();
    let i_time = trusted
        .then(|| bolus.iter().filter_map(|s| s.time).reduce(f64::min))
        .flatten();
    let i_sn = bolus.iter().filter_map(|s| s.series_number).min();
    let i_first = bolus.iter().filter_map(|s| s.first).reduce(f64::min);
    let c3_conflict = matches!((i_time, l_time), (Some(it), Some(lt)) if it < lt);
    if c3_conflict {
        fire(
            "C3_bolus_before_L",
            &|_, _| true,
            &mut placed,
            &mut reason,
            &mut flags,
        );
    }
    let mut added = vec![false; n];
    if let (Some(it), false) = (i_time, c3_conflict) {
        for i in 0..n {
            let s = &scans[i];
            let after = s.time.is_some_and(|t| t > it)
                && match (s.series_number, i_sn) {
                    (Some(a), Some(b)) => a > b,
                    _ => true,
                };
            let open = matches!(
                placed[i],
                Placed::Unknown(
                    Why::Window | Why::PreOnlyAfterL | Why::NoT1 | Why::PostOnlyBeforeF
                )
            );
            if in_frame(s) && after && open && !s.kind.family.bolus() {
                placed[i] = Placed::Post;
                added[i] = true;
                flags[i].insert("C3_bolus_post_added");
            }
        }
    }

    // C2: head and spine in one session, the rule run per exam
    let regions: BTreeSet<Region> = scans
        .iter()
        .filter(|s| in_frame(s) && s.region != Region::None)
        .map(|s| s.region)
        .collect();
    if regions.len() > 1 {
        let mut by_exam: Vec<Option<Placed>> = vec![None; n];
        for r in &regions {
            let br = bracket(scans, &answers, &|i| {
                in_frame(&scans[i]) && scans[i].region == *r
            });
            for i in 0..n {
                if scans[i].region == *r {
                    by_exam[i] = Some(place(&scans[i], trusted, &br, scans));
                }
            }
        }
        for f in flags.iter_mut() {
            f.insert("C2_multi_region_session");
        }
        let c2: Vec<bool> = (0..n)
            .map(|i| {
                labelled(base[i]) && labelled(placed[i]) && by_exam[i].is_some_and(|p| p != base[i])
            })
            .collect();
        fire(
            "C2_exam",
            &|i, _| c2[i],
            &mut placed,
            &mut reason,
            &mut flags,
        );
    }

    // C4: a split dose or a preload, a flag
    let mut bolus_series: BTreeMap<i64, f64> = BTreeMap::new();
    for s in &bolus {
        if let Some(t) = s.time {
            let e = bolus_series.entry(s.series).or_insert(t);
            *e = e.min(t);
        }
    }
    let ev: Vec<f64> = bolus_series.values().copied().collect();
    let two_events = ev.len() >= 2 && {
        let lo = ev.iter().copied().fold(f64::MAX, f64::min);
        let hi = ev.iter().copied().fold(f64::MIN, f64::max);
        hi - lo >= cfg.split_gap_seconds
    };
    let after_f = matches!((i_time, f_time), (Some(it), Some(ft)) if it > ft);
    if trusted && (two_events || after_f) {
        let lo = [i_time, f_time].into_iter().flatten().reduce(f64::min);
        let hi = [i_time, f_time].into_iter().flatten().reduce(f64::max);
        for i in 0..n {
            flags[i].insert("C4_split_session");
            if let (Some(t), Some(lo), Some(hi)) = (scans[i].time, lo, hi)
                && t > lo
                && t < hi
            {
                flags[i].insert("C4_split");
            }
        }
    }

    // C5: the clock
    let dates: BTreeSet<i64> = scans
        .iter()
        .filter(|s| in_frame(s))
        .flat_map(|s| s.dates.iter().copied())
        .collect();
    if dates.len() > 1 {
        fire(
            "C5_midnight",
            &|_, _| true,
            &mut placed,
            &mut reason,
            &mut flags,
        );
    }
    fire(
        "C5_no_time",
        &|i, _| scans[i].timed < scans[i].images,
        &mut placed,
        &mut reason,
        &mut flags,
    );
    let f_early = f_scan.is_some_and(|s| s.content_early > 0);
    let l_early = l_scan.is_some_and(|s| s.content_early > 0);
    fire(
        "C5_content",
        &|i, p| {
            scans[i].content_early > 0
                || (p == Placed::Post && f_early)
                || (p == Placed::Pre && l_early)
        },
        &mut placed,
        &mut reason,
        &mut flags,
    );
    let f_first = f_scan.and_then(|s| s.first);
    let l_first = l_scan.and_then(|s| s.first);
    fire(
        "C5_straddle",
        &|i, p| {
            let s = &scans[i];
            (p == Placed::Pre && matches!((l_first, s.last), (Some(lf), Some(last)) if last >= lf))
                || (p == Placed::Post
                    && !added[i]
                    && matches!((f_first, s.first), (Some(ff), Some(first)) if first <= ff))
        },
        &mut placed,
        &mut reason,
        &mut flags,
    );
    for i in 0..n {
        if placed[i] == Placed::Unknown(Why::SeriesNumber) {
            flags[i].insert("C5_sn_guard");
        }
    }
    // C6: a derived stack, whose source the registry cannot name

    // C7 and C8
    // C8: F rests on one model answer that nothing else supports
    let weak_f = match b.first_post {
        Some(fi) => {
            let f = &scans[fi];
            let one_model = f.answer.is_some_and(|a| a.by == By::Image);
            let second_post = answers
                .iter()
                .any(|&i| i != fi && scans[i].answer.is_some_and(|a| a.post));
            let tag_on_or_after = scans
                .iter()
                .any(|s| s.tagged && s.time.is_some_and(|t| f.time.is_some_and(|ft| t >= ft)));
            let bolus_before = matches!((i_time, f.time), (Some(it), Some(ft)) if it <= ft);
            one_model && !second_post && !tag_on_or_after && !bolus_before
        }
        None => false,
    };

    // the labels (tr_label.py)
    let mut labels = Vec::with_capacity(n);
    let use_bolus = i_time.is_some() && !c3_conflict && (f_time.is_none() || i_time <= f_time);
    let use_f = !c3_conflict && !use_bolus && f_time.is_some();
    let (o_first, o_time, origin) = if use_bolus {
        (i_first, i_time, Some(Origin::BolusStart))
    } else if use_f {
        (
            f_scan.and_then(|s| s.first),
            f_time,
            Some(Origin::FirstPostT1),
        )
    } else {
        (None, None, None)
    };
    let mut contrast = study.mr_tagged > 0 || scans.iter().any(|s| s.tagged);
    for i in 0..n {
        let s = &scans[i];
        if s.kind.family.bolus() {
            contrast = true;
        }
        if s.kind.t1_family && s.outside != Outside::Sealed && s.answer.is_some_and(|a| a.post) {
            contrast = true;
        }
        let mut l = Label {
            stack: s.stack,
            state: State::Undetermined,
            reason: None,
            basis: Basis::None,
            source: Source::T1AnchorTimeRule,
            minutes_since_injection: None,
            injection_ref: None,
            minutes_basis: None,
            prior_72h: Prior::No,
            asl_after_contrast: None,
            weak_f: false,
            flags: std::mem::take(&mut flags[i]),
            kind: s.kind,
            first_post: f_scan.map(|s| s.stack),
            last_pre: l_scan.map(|s| s.stack),
            anchored,
        };
        match s.outside {
            Outside::No => {}
            Outside::Sealed => {
                l.reason = Some("sealed".into());
                labels.push(l);
                continue;
            }
            Outside::Localizer => {
                l.reason = Some("R_localizer".into());
                labels.push(l);
                continue;
            }
            Outside::Excluded => {
                l.reason = Some("R_excluded".into());
                labels.push(l);
                continue;
            }
            Outside::Derived => {
                l.flags.insert("C6_derived");
                if !s.kind.family.bolus() {
                    l.reason = Some("C6_derived".into());
                    labels.push(l);
                    continue;
                }
            }
        }
        if s.kind.family.bolus() {
            // DSC, DCE and contrast MRA: by definition, whatever the bracket says
            l.state = State::ByDefinition;
            l.basis = Basis::Family;
            l.source = Source::Header;
        } else {
            match placed[i] {
                Placed::Own => {
                    let a = s.answer.expect("an own answer");
                    if b.conflict {
                        l.reason = Some(Why::Conflict.code().into());
                    } else if c3_conflict {
                        l.reason = Some("C3_bolus_before_L".into());
                    } else {
                        l.state = if a.post { State::Post } else { State::Pre };
                        l.basis = Basis::OwnAnswer;
                        l.source = match a.by {
                            By::Image => Source::Image,
                            By::Reader => Source::Reader,
                        };
                    }
                }
                Placed::Pre => {
                    l.state = State::Pre;
                    l.basis = Basis::Bracket;
                }
                Placed::Post => {
                    l.state = State::Post;
                    l.basis = if added[i] {
                        Basis::Bolus
                    } else {
                        Basis::Bracket
                    };
                    l.weak_f = weak_f && !added[i];
                    if l.weak_f {
                        l.flags.insert("C8_weak_F");
                    }
                }
                Placed::Unknown(why) => {
                    l.reason = Some(reason[i].unwrap_or(why.code()).into());
                }
            }
        }
        if matches!(l.state, State::Post | State::ByDefinition)
            && let Some(origin) = origin
        {
            let by_image = s.first.zip(o_first).map(|(a, o)| (a - o) / 60.0);
            let by_stack = s.time.zip(o_time).map(|(a, o)| (a - o) / 60.0);
            let (m, basis) = match (by_image, by_stack) {
                (Some(m), _) => (Some(m), Some(MinutesBasis::Instance)),
                (None, Some(m)) => (Some(m), Some(MinutesBasis::StackTime)),
                _ => (None, None),
            };
            l.minutes_since_injection = m.map(|m| (m * 10.0).round() / 10.0);
            l.minutes_basis = basis;
            l.injection_ref = m.map(|_| origin);
        }
        labels.push(l);
    }
    Passed {
        study: study.id,
        labels,
        facts: Facts { contrast },
    }
}

/// What the subject step found of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    prior: Prior,
    within_day: bool,
    ct_72h: bool,
    other_mr_study_day: bool,
}

/// The subject-level step (record 55 F5): over every session of one
/// subject, after each session's pass, the 72-hour flag and C1. A session
/// with an MR contrast event 0 to 72 hours before it (the same day
/// included) makes its `pre` labels `undetermined` (`C1_prior_72h`), its
/// own pre T1 answers too, and flags every stack `yes`; where a date is
/// missing the flag is `unknown`. CT sessions are counted as a flag and
/// change nothing (the time-label plan's T6: MR only until CT is
/// pseudonymised). Another MR study of the subject within 24 hours either
/// way is C2's flag.
pub fn subject(cfg: &Config, studies: &[Study], passed: &mut [Passed]) {
    let at: BTreeMap<i64, Option<f64>> = studies.iter().map(|s| (s.id, s.at_minutes)).collect();
    let mr: BTreeMap<i64, bool> = studies
        .iter()
        .map(|s| (s.id, s.mr_series > 0 || !s.scans.is_empty()))
        .collect();
    let ct: BTreeMap<i64, bool> = studies.iter().map(|s| (s.id, s.ct_tagged > 0)).collect();
    let contrast: BTreeMap<i64, bool> =
        passed.iter().map(|p| (p.study, p.facts.contrast)).collect();
    let ids: Vec<i64> = studies.iter().map(|s| s.id).collect();
    let mut looks: BTreeMap<i64, Look> = BTreeMap::new();
    for &i in &ids {
        let mut look = Look {
            prior: Prior::No,
            within_day: false,
            ct_72h: false,
            other_mr_study_day: false,
        };
        let mut unknown = at[&i].is_none();
        for &j in ids.iter().filter(|&&j| j != i) {
            let (Some(ti), Some(tj)) = (at[&i], at[&j]) else {
                unknown = true;
                continue;
            };
            let d = ti - tj;
            if (0.0..=cfg.lookback_minutes).contains(&d) {
                if contrast.get(&j).copied().unwrap_or(false) && mr[&j] {
                    look.prior = Prior::Yes;
                    if d < cfg.same_visit_minutes {
                        look.within_day = true;
                    }
                }
                if ct[&j] {
                    look.ct_72h = true;
                }
            }
            if mr[&j] && d.abs() < cfg.same_visit_minutes {
                look.other_mr_study_day = true;
            }
        }
        if look.prior != Prior::Yes && unknown {
            look.prior = Prior::Unknown;
        }
        looks.insert(i, look);
    }
    for p in passed.iter_mut() {
        let look = looks.get(&p.study).copied().unwrap_or(Look {
            prior: Prior::Unknown,
            within_day: false,
            ct_72h: false,
            other_mr_study_day: false,
        });
        for l in &mut p.labels {
            l.prior_72h = look.prior;
            if look.prior == Prior::Yes {
                l.flags.insert("C1_prior_72h_flag");
                l.flags.insert(if look.within_day {
                    "C1_source_within_24h"
                } else {
                    "C1_source_24_to_72h"
                });
                // C1: before F is undetermined, never pre; an own pre T1
                // answer as well
                if l.state == State::Pre {
                    l.state = State::Undetermined;
                    l.reason = Some("C1_prior_72h".into());
                    l.basis = Basis::None;
                    l.source = Source::T1AnchorTimeRule;
                    l.flags.insert("C1_prior_72h");
                }
            }
            if look.prior == Prior::Unknown {
                l.flags.insert("C1_dates_unknown");
            }
            if look.ct_72h {
                l.flags.insert("C1_ct_72h");
            }
            if look.other_mr_study_day {
                l.flags.insert("C2_second_study");
            }
            if l.kind.asl {
                l.asl_after_contrast = Some(match l.state {
                    State::Post => Prior::Yes,
                    State::Pre => Prior::No,
                    _ => Prior::Unknown,
                });
            }
        }
    }
}

/// The label of every stack of one subject's sessions: each session's pass,
/// then the subject step.
pub fn label(cfg: &Config, studies: &[Study]) -> Vec<Passed> {
    let mut passed: Vec<Passed> = studies.iter().map(|s| session(cfg, s)).collect();
    subject(cfg, studies, &mut passed);
    passed
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: Kind = Kind {
        family: Family::T1,
        t1_family: true,
        asl: false,
        quantitative_map: false,
        tof: false,
    };
    const FLAIR: Kind = Kind {
        family: Family::Flair,
        t1_family: false,
        asl: false,
        quantitative_map: false,
        tof: false,
    };
    const DSC: Kind = Kind {
        family: Family::DscDce,
        t1_family: false,
        asl: false,
        quantitative_map: false,
        tof: false,
    };
    const ASL: Kind = Kind {
        family: Family::Other,
        t1_family: false,
        asl: true,
        quantitative_map: false,
        tof: false,
    };

    /// A stack at `minute` of the day 20000, series number `sn`.
    fn scan(stack: i64, sn: i64, minute: f64, kind: Kind) -> Scan {
        let t = 20_000.0 * 86_400.0 + 8.0 * 3600.0 + minute * 60.0;
        Scan {
            stack,
            series: stack,
            series_number: Some(sn),
            kind,
            outside: Outside::No,
            region: Region::Head,
            time: Some(t),
            first: Some(t),
            last: Some(t + 120.0),
            images: 10,
            timed: 10,
            content_early: 0,
            dates: vec![20_000],
            tagged: false,
            answer: None,
        }
    }

    fn answered(mut s: Scan, post: bool, by: By) -> Scan {
        s.answer = Some(Answer { post, by });
        s
    }

    fn study(id: i64, day: f64, scans: Vec<Scan>) -> Study {
        Study {
            id,
            at_minutes: Some(day * 1440.0 + 8.0 * 60.0),
            mr_series: scans.len() as i64,
            mr_tagged: 0,
            ct_tagged: 0,
            scans,
        }
    }

    /// pre T1 at 0, FLAIR at 10, post T1 at 20, FLAIR at 30, T2 at 40.
    fn bracketed() -> Vec<Scan> {
        vec![
            answered(scan(1, 10, 0.0, T1), false, By::Image),
            scan(2, 20, 10.0, FLAIR),
            answered(scan(3, 30, 20.0, T1), true, By::Image),
            scan(4, 40, 30.0, FLAIR),
            scan(
                5,
                50,
                40.0,
                Kind {
                    family: Family::T2,
                    ..FLAIR
                },
            ),
        ]
    }

    fn by_stack(p: &[Passed]) -> BTreeMap<i64, Label> {
        p.iter()
            .flat_map(|p| p.labels.iter().cloned())
            .map(|l| (l.stack, l))
            .collect()
    }

    fn run(studies: Vec<Study>) -> BTreeMap<i64, Label> {
        by_stack(&label(&Config::default(), &studies))
    }

    #[test]
    fn the_rule_places_a_session_by_its_clock() {
        let l = run(vec![study(1, 20_000.0, bracketed())]);
        // the window between L and F: neither pre nor post
        assert_eq!(l[&2].state, State::Undetermined);
        assert_eq!(l[&2].reason.as_deref(), Some("R_window"));
        assert_eq!(l[&4].state, State::Post);
        assert_eq!(l[&4].source, Source::T1AnchorTimeRule);
        assert_eq!(l[&4].basis, Basis::Bracket);
        assert_eq!(l[&4].minutes_since_injection, Some(10.0));
        assert_eq!(l[&4].injection_ref, Some(Origin::FirstPostT1));
        assert_eq!(l[&5].minutes_since_injection, Some(20.0));
        // a T1's own answer stands, as the image's
        assert_eq!(l[&1].state, State::Pre);
        assert_eq!(l[&1].source, Source::Image);
        assert_eq!(l[&3].state, State::Post);
        assert_eq!(l[&3].minutes_since_injection, Some(0.0));
        for x in l.values() {
            assert!(x.anchored);
            assert_eq!(x.prior_72h, Prior::No);
        }
    }

    #[test]
    fn a_stack_before_the_last_pre_t1_is_pre() {
        let mut s = bracketed();
        s.insert(0, scan(9, 5, -10.0, FLAIR));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&9].state, State::Pre);
        assert_eq!(l[&9].minutes_since_injection, None);
    }

    #[test]
    fn no_t1_or_only_a_post_t1_is_never_pre() {
        // C7: no T1 answer at all
        let l = run(vec![study(
            1,
            20_000.0,
            vec![scan(1, 1, 0.0, FLAIR), scan(2, 2, 10.0, T1)],
        )]);
        assert_eq!(l[&1].reason.as_deref(), Some("C7_no_t1"));
        assert!(!l[&1].anchored);
        // C7: a post-only session, and a stack before F
        let l = run(vec![study(
            1,
            20_000.0,
            vec![
                scan(1, 1, 0.0, FLAIR),
                answered(scan(2, 2, 10.0, T1), true, By::Reader),
                scan(3, 3, 20.0, FLAIR),
            ],
        )]);
        assert_eq!(l[&1].state, State::Undetermined);
        assert_eq!(l[&1].reason.as_deref(), Some("C7_post_only_before_F"));
        assert_eq!(l[&3].state, State::Post);
        assert_eq!(l[&2].source, Source::Reader);
    }

    #[test]
    fn c1_an_earlier_injection_within_72_hours() {
        let earlier = study(
            1,
            19_999.0,
            vec![
                scan(11, 1, 0.0, FLAIR),
                answered(scan(12, 2, 10.0, T1), true, By::Image),
            ],
        );
        let mut s = bracketed();
        s.insert(0, scan(9, 5, -10.0, FLAIR));
        let l = run(vec![earlier, study(2, 20_000.0, s)]);
        assert_eq!(l[&9].state, State::Undetermined);
        assert_eq!(l[&9].reason.as_deref(), Some("C1_prior_72h"));
        // the own pre T1 answer as well
        assert_eq!(l[&1].reason.as_deref(), Some("C1_prior_72h"));
        // post stays post, with the flag
        assert_eq!(l[&4].state, State::Post);
        assert_eq!(l[&4].prior_72h, Prior::Yes);
        assert!(
            l[&4].flags.contains("C1_source_within_24h")
                || l[&4].flags.contains("C1_source_24_to_72h")
        );
        // the earlier study had nothing before it
        assert_eq!(l[&11].prior_72h, Prior::No);
    }

    #[test]
    fn c1_four_days_back_does_not_count_and_a_missing_date_is_unknown() {
        let earlier = study(
            1,
            19_996.0,
            vec![answered(scan(12, 2, 10.0, T1), true, By::Image)],
        );
        let l = run(vec![earlier.clone(), study(2, 20_000.0, bracketed())]);
        assert_eq!(l[&4].prior_72h, Prior::No);
        let mut undated = study(2, 20_000.0, bracketed());
        undated.at_minutes = None;
        let l = run(vec![earlier, undated]);
        assert_eq!(l[&4].prior_72h, Prior::Unknown);
        assert!(l[&4].flags.contains("C1_dates_unknown"));
    }

    #[test]
    fn c2_two_exams_in_one_session() {
        // a head exam bracketed pre, then a spine exam whose own bracket is post only
        let spine = |stack, sn, m, k| {
            let mut s = scan(stack, sn, m, k);
            s.region = Region::Spine;
            s
        };
        let scans = vec![
            answered(scan(1, 1, 0.0, T1), false, By::Image),
            scan(2, 2, 10.0, FLAIR),
            answered(scan(3, 3, 20.0, T1), true, By::Image),
            spine(4, 4, 30.0, FLAIR),
            answered(spine(5, 5, 40.0, T1), true, By::Image),
            spine(6, 6, 50.0, FLAIR),
        ];
        let l = run(vec![study(1, 20_000.0, scans)]);
        // stack 4 is post by the session's F, and before F in the spine exam
        assert_eq!(l[&4].state, State::Undetermined);
        assert_eq!(l[&4].reason.as_deref(), Some("C2_exam"));
        assert_eq!(l[&6].state, State::Post);
    }

    #[test]
    fn c3_a_bolus_before_the_last_pre_contradicts_the_bracket() {
        let mut s = bracketed();
        s.insert(0, scan(8, 5, -10.0, DSC));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C3_bolus_before_L"));
        assert_eq!(l[&3].reason.as_deref(), Some("C3_bolus_before_L"));
        assert_eq!(l[&8].state, State::ByDefinition);
        assert_eq!(l[&8].source, Source::Header);
    }

    #[test]
    fn c3_a_bolus_in_the_window_labels_what_follows_and_starts_the_clock() {
        let mut s = bracketed();
        // a DSC at minute 12, between L (0) and F (20), and a FLAIR after it
        s.push(scan(8, 22, 12.0, DSC));
        s.push(scan(7, 25, 15.0, FLAIR));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&7].state, State::Post);
        assert_eq!(l[&7].basis, Basis::Bolus);
        assert_eq!(l[&7].injection_ref, Some(Origin::BolusStart));
        assert_eq!(l[&7].minutes_since_injection, Some(3.0));
        assert_eq!(l[&4].minutes_since_injection, Some(18.0));
        assert_eq!(l[&8].minutes_since_injection, Some(0.0));
        // the window before the bolus stays open
        assert_eq!(l[&2].reason.as_deref(), Some("R_window"));
    }

    #[test]
    fn c4_a_split_dose_is_a_flag() {
        let mut s = bracketed();
        s.push(scan(8, 60, 50.0, DSC));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert!(l[&4].flags.contains("C4_split"));
        assert_eq!(l[&4].state, State::Post);
        assert!(l[&5].flags.contains("C4_split_session"));
    }

    #[test]
    fn c5_clock_errors() {
        // a session over midnight
        let mut s = bracketed();
        s[4].dates = vec![20_000, 20_001];
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C5_midnight"));
        // images with no time
        let mut s = bracketed();
        s[3].timed = 7;
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C5_no_time"));
        // content before acquisition, on the bracket stack F
        let mut s = bracketed();
        s[2].content_early = 3;
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C5_content"));
        // a stack reaching across F's first image
        let mut s = bracketed();
        s[3].first = s[2].first;
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C5_straddle"));
        // the series number against the clock
        let mut s = bracketed();
        s[3].series_number = Some(25);
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("C5_sn_guard"));
    }

    #[test]
    fn c5_an_untrusted_clock_labels_nothing_but_own_answers() {
        // a session that spans more than four hours
        let mut s = bracketed();
        s[4].time = s[4].time.map(|t| t + 5.0 * 3600.0);
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("R_no_trusted_time"));
        assert_eq!(l[&3].state, State::Post);
        // a session whose clock runs against its series numbers
        let mut s = bracketed();
        for (x, sn) in s.iter_mut().zip([50, 40, 30, 20, 10]) {
            x.series_number = Some(sn);
        }
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&4].reason.as_deref(), Some("R_no_trusted_time"));
    }

    #[test]
    fn c6_a_derived_series_has_no_source_to_take_a_time_from() {
        let mut s = bracketed();
        let mut d = scan(9, 60, 50.0, FLAIR);
        d.outside = Outside::Derived;
        s.push(d);
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&9].state, State::Undetermined);
        assert_eq!(l[&9].reason.as_deref(), Some("C6_derived"));
        // and it does not break the clock's trust
        assert_eq!(l[&4].state, State::Post);
    }

    #[test]
    fn c8_a_lone_model_post_is_flagged_weak() {
        let l = run(vec![study(1, 20_000.0, bracketed())]);
        assert!(l[&4].weak_f);
        assert!(l[&4].flags.contains("C8_weak_F"));
        // a second post answer supports it
        let mut s = bracketed();
        s.push(answered(scan(6, 60, 50.0, T1), true, By::Image));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert!(!l[&4].weak_f);
        // a reader's F is not a model's alone
        let mut s = bracketed();
        s[2].answer = Some(Answer {
            post: true,
            by: By::Reader,
        });
        let l = run(vec![study(1, 20_000.0, s)]);
        assert!(!l[&4].weak_f);
    }

    #[test]
    fn conflicting_answers_label_nothing() {
        let mut s = bracketed();
        // a pre answer after the post one
        s.push(answered(scan(6, 60, 50.0, T1), false, By::Image));
        let l = run(vec![study(1, 20_000.0, s)]);
        for id in [1, 3, 4, 6] {
            assert_eq!(l[&id].reason.as_deref(), Some("R_conflict"), "{id}");
        }
    }

    #[test]
    fn asl_after_contrast_and_by_definition() {
        let mut s = bracketed();
        s.push(scan(6, 60, 50.0, ASL));
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&6].state, State::Post);
        assert_eq!(l[&6].asl_after_contrast, Some(Prior::Yes));
        assert_eq!(use_level(&l[&6].kind), "1 %");
        assert_eq!(use_level(&l[&4].kind), "1 %");
        assert_eq!(use_level(&l[&5].kind), "5 %");
        assert_eq!(use_level(&l[&1].kind), "T1 family");
    }

    #[test]
    fn sealed_answers_are_no_anchors() {
        let mut s = bracketed();
        s[2].outside = Outside::Sealed;
        let l = run(vec![study(1, 20_000.0, s)]);
        assert_eq!(l[&3].reason.as_deref(), Some("sealed"));
        // without F the FLAIR after it is not post
        assert_eq!(l[&4].reason.as_deref(), Some("R_pre_only_after_L"));
    }

    #[test]
    fn families_follow_the_pack_and_the_physics_guard() {
        let cfg = Config::default();
        let mods = |m: &[&str]| m.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let none: Vec<String> = Vec::new();
        let mprage = Described {
            base: Some("T1w"),
            modifiers: &none,
            technique: Some("MPRAGE"),
            gradient_echo: true,
            tr: Some(2300.0),
            te: Some(3.0),
            ti: Some(900.0),
            field_tesla: Some(3.0),
            acquisition: Some("3D"),
        };
        assert_eq!(kind_of(&cfg, &mprage).family, Family::T1);
        // a short-TI inversion is no T1 contrast
        let stir = Described {
            ti: Some(150.0),
            ..mprage.clone()
        };
        assert_ne!(kind_of(&cfg, &stir).family, Family::T1);
        let flair_mods = mods(&["FLAIR"]);
        let flair = Described {
            base: Some("T2w"),
            modifiers: &flair_mods,
            technique: Some("TSE"),
            gradient_echo: false,
            tr: Some(9000.0),
            te: Some(90.0),
            ti: Some(2500.0),
            ..mprage.clone()
        };
        assert_eq!(kind_of(&cfg, &flair).family, Family::Flair);
        let pd = Described {
            base: Some("PDw"),
            ..flair.clone()
        };
        let k = kind_of(
            &cfg,
            &Described {
                modifiers: &none,
                ..pd
            },
        );
        assert_eq!(k.family, Family::Pd);
        assert!(k.t1_family);
        let psir_mods = mods(&["PSIR"]);
        let psir = Described {
            modifiers: &psir_mods,
            ti: Some(400.0),
            ..mprage.clone()
        };
        assert!(kind_of(&cfg, &psir).t1_family);
        let dsc = Described {
            technique: Some("Perfusion-EPI"),
            ..mprage.clone()
        };
        assert_eq!(kind_of(&cfg, &dsc).family, Family::DscDce);
        let tof = Described {
            technique: Some("TOF-MRA"),
            ..mprage
        };
        let k = kind_of(&cfg, &tof);
        assert!(k.tof && !k.t1_family);
        assert_eq!(use_level(&k), "2 %");
    }

    #[test]
    fn spearman_ties_and_order() {
        assert_eq!(spearman(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]), Some(1.0));
        assert_eq!(spearman(&[1.0, 2.0, 3.0], &[3.0, 2.0, 1.0]), Some(-1.0));
        assert_eq!(spearman(&[1.0, 2.0], &[1.0, 2.0]), None);
        assert_eq!(spearman(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]), None);
    }
}
