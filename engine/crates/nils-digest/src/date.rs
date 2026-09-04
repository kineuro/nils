// SPDX-License-Identifier: AGPL-3.0-only

//! Which day a study happened on
//! (`docs/specs/wave3-anonymize-and-bids.md`, §4).
//!
//! Without a date there is no session, no order and no clinical join, so a
//! study that carries none is not a study anyone can place. v0 fills a missing
//! `StudyDate` from three other elements and then from a date embedded in a
//! UID, taking the first source that answers.
//!
//! That is the wrong shape. **A source is not a rung, it is a vote with a
//! weight**, because a date three independent elements agree on is worth more
//! than one a single element asserts. This gathers candidates from every source
//! it knows, adds each source's weight to the candidate it names, and returns
//! the heaviest with its margin over the runner-up.
//!
//! Two rules are not weights:
//!
//! * **A placeholder is not a date.** `00000000`, `19000101` and their like are
//!   how a scanner or an anonymiser writes nothing.
//! * **Distrust the first of January.** Anonymisers rewrite creation and issue
//!   dates to `YYYY0101`. When the heaviest candidate is a first of January and
//!   any other candidate exists, the other one wins.

use std::collections::BTreeMap;

/// A date as the registry stores one: `YYYY-MM-DD`.
pub type Iso = String;

/// Where a candidate came from. The name is what `study.date_source` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    StudyDate,
    PpsStartDate,
    PpsEndDate,
    InstanceCreationDate,
    Private,
    SeriesDate,
    AcquisitionDate,
    ContentDate,
    IssueDate,
    PresentationCreationDate,
    /// `YYYYMMDD` inside a UID.
    Uid,
    /// Unix epoch seconds inside a UID, which is what some GE scanners leave.
    UidEpoch,
    /// A `YYYYMMDD` component of the file's path.
    Path,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::StudyDate => "study_date",
            Source::PpsStartDate => "pps_start_date",
            Source::PpsEndDate => "pps_end_date",
            Source::InstanceCreationDate => "instance_creation_date",
            Source::Private => "private",
            Source::SeriesDate => "series_date",
            Source::AcquisitionDate => "acquisition_date",
            Source::ContentDate => "content_date",
            Source::IssueDate => "issue_date",
            Source::PresentationCreationDate => "presentation_creation_date",
            Source::Uid => "uid",
            Source::UidEpoch => "uid_epoch",
            Source::Path => "path",
        }
    }

    /// What the source is worth. A date the scanner wrote where the date goes
    /// outweighs one recovered from a string that happens to contain digits.
    pub fn weight(self) -> u32 {
        match self {
            Source::StudyDate => 4,
            Source::PpsStartDate
            | Source::PpsEndDate
            | Source::InstanceCreationDate
            | Source::Private => 3,
            Source::SeriesDate
            | Source::AcquisitionDate
            | Source::ContentDate
            | Source::IssueDate
            | Source::PresentationCreationDate => 2,
            Source::Path => 2,
            Source::Uid | Source::UidEpoch => 1,
        }
    }
}

/// What a study's date was decided to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub date: Iso,
    /// The source that carried the most weight for the winning date.
    pub source: Source,
    /// The winning weight, and the runner-up's, so a report can say whether the
    /// answer was unanimous or a squeak.
    pub weight: u32,
    pub runner_up: u32,
}

/// The values a scanner or an anonymiser writes to mean nothing.
pub fn is_placeholder(raw: &str) -> bool {
    let v = raw.trim();
    matches!(
        v,
        "" | "00000000" | "0000" | "19000101" | "1900" | "XXXX" | "xxxx" | "NONE" | "UNKNOWN"
    )
}

/// The year range a recovered date must fall in to be believable, which is the
/// only thing that makes reading eight digits out of a UID reasonable.
#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub min_year: i32,
    pub max_year: i32,
}

impl Default for Range {
    fn default() -> Self {
        Range {
            min_year: 1980,
            max_year: 2100,
        }
    }
}

impl Range {
    fn holds(&self, y: i32) -> bool {
        y >= self.min_year && y <= self.max_year
    }
}

/// `YYYYMMDD`, or a non-conformant but unambiguous `YYYY-MM-DD`, as `Iso`.
/// A value that is not a real calendar day is not a date.
pub fn parse(raw: &str, range: Range) -> Option<Iso> {
    let v: String = raw.trim().chars().filter(|c| c.is_ascii_digit()).collect();
    if v.len() != 8 || is_placeholder(raw) {
        return None;
    }
    let y: i32 = v[0..4].parse().ok()?;
    let m: u32 = v[4..6].parse().ok()?;
    let d: u32 = v[6..8].parse().ok()?;
    if !range.holds(y) || !valid(y, m, d) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

fn valid(y: i32, m: u32, d: u32) -> bool {
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    d <= days_in(y, m)
}

fn days_in(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Every `YYYYMMDD` inside a longer string that is a real day in range. A UID
/// is mostly digits, so the range is what stops a random run being a date.
pub fn dates_in(text: &str, range: Range) -> Vec<Iso> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len().saturating_sub(7) {
        // The run must not be part of a longer number, or `1.2.20220115999`
        // yields three overlapping readings of the same digits.
        if i > 0 && b[i - 1].is_ascii_digit() {
            continue;
        }
        let end = i + 8;
        if !b[i..end].iter().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if end < b.len() && b[end].is_ascii_digit() {
            continue;
        }
        if let Some(d) = parse(&text[i..end], range) {
            out.push(d);
        }
    }
    out
}

/// Unix epoch seconds embedded in a UID, which some scanners leave in the SOP
/// UID and no `YYYYMMDD` reader would ever find. Ten digits, bounded so a
/// random run is not a timestamp.
pub fn epochs_in(text: &str, range: Range) -> Vec<Iso> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len().saturating_sub(9) {
        if i > 0 && b[i - 1].is_ascii_digit() {
            continue;
        }
        let end = i + 10;
        if !b[i..end].iter().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if end < b.len() && b[end].is_ascii_digit() {
            continue;
        }
        let Ok(secs) = text[i..end].parse::<i64>() else {
            continue;
        };
        if let Some(d) = from_epoch(secs, range) {
            out.push(d);
        }
    }
    out
}

/// A Unix timestamp as a day, by civil-from-days: no calendar crate for one
/// division.
fn from_epoch(secs: i64, range: Range) -> Option<Iso> {
    if secs <= 0 {
        return None;
    }
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (if m <= 2 { y + 1 } else { y }) as i32;
    if !range.holds(y) || !valid(y, m, d) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// One study's candidates, gathered before they are weighed.
#[derive(Debug, Default)]
pub struct Ballot {
    /// candidate date -> (total weight, the heaviest source that named it)
    votes: BTreeMap<Iso, (u32, Option<Source>)>,
}

impl Ballot {
    pub fn new() -> Ballot {
        Ballot::default()
    }

    /// A value one source read, parsed and weighed. A placeholder or an
    /// unparseable value is no vote at all.
    pub fn element(&mut self, source: Source, raw: Option<&str>, range: Range) {
        let Some(raw) = raw else { return };
        let Some(date) = parse(raw, range) else {
            return;
        };
        self.add(source, date);
    }

    /// A string that may *contain* a date: a UID, a path.
    pub fn inside(&mut self, source: Source, text: &str, range: Range) {
        let found = match source {
            Source::UidEpoch => epochs_in(text, range),
            _ => dates_in(text, range),
        };
        for d in found {
            self.add(source, d);
        }
    }

    /// Every source one file offers: the elements it carries, the UIDs it is
    /// named by, and the path it was found at.
    pub fn cast(&mut self, p: &crate::batch::ParsedFile, range: Range) {
        use nils_dicom::Level;
        let x = &p.extracted;
        let el = |level, column| {
            x.value(level, column)
                .map(|v| v.to_string())
                .filter(|v| !v.trim().is_empty())
        };
        for (source, level, column) in [
            (Source::StudyDate, Level::Study, "study_date"),
            (Source::PpsStartDate, Level::Study, "pps_start_date"),
            (Source::PpsEndDate, Level::Study, "pps_end_date"),
            (Source::IssueDate, Level::Study, "issue_date"),
            (Source::SeriesDate, Level::Series, "series_date"),
            (Source::AcquisitionDate, Level::Instance, "acquisition_date"),
            (Source::ContentDate, Level::Instance, "content_date"),
            (
                Source::InstanceCreationDate,
                Level::Instance,
                "instance_creation_date",
            ),
            (
                Source::PresentationCreationDate,
                Level::Instance,
                "presentation_creation_date",
            ),
        ] {
            self.element(source, el(level, column).as_deref(), range);
        }
        // A UID is mostly digits, so it votes lightly and only inside the
        // range; the epoch reading is what a YYYYMMDD reader never finds.
        for uid in [&x.study_uid, &x.series_uid, &x.sop_uid] {
            self.inside(Source::Uid, uid, range);
            self.inside(Source::UidEpoch, uid, range);
        }
        // A sorted archive puts the session date in a directory name, so the
        // directory votes and the file name does not.
        self.inside(Source::Path, &p.dir, range);
    }

    fn add(&mut self, source: Source, date: Iso) {
        let e = self.votes.entry(date).or_insert((0, None));
        e.0 += source.weight();
        // The source recorded is the heaviest that named this date, so
        // `date_source` says the best reason rather than the last one.
        if e.1.is_none_or(|s| source.weight() > s.weight()) {
            e.1 = Some(source);
        }
    }

    /// The heaviest candidate, with the first-of-January rule applied.
    pub fn verdict(&self) -> Option<Verdict> {
        if self.votes.is_empty() {
            return None;
        }
        let mut ranked: Vec<(&Iso, u32, Source)> = self
            .votes
            .iter()
            .map(|(d, (w, s))| (d, *w, s.expect("a vote names its source")))
            .collect();
        // Heaviest first; a tie goes to the earlier date so the answer does not
        // depend on the order the files arrived in.
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

        // Distrust the first of January: an anonymiser writes it, so when it
        // wins and anything else is on the ballot, the other one wins instead.
        let mut best = 0usize;
        if is_first_of_january(ranked[0].0)
            && ranked.len() > 1
            && let Some(i) = ranked.iter().position(|r| !is_first_of_january(r.0))
        {
            best = i;
        }
        let runner_up = ranked
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != best)
            .map(|(_, r)| r.1)
            .max()
            .unwrap_or(0);
        Some(Verdict {
            date: ranked[best].0.clone(),
            source: ranked[best].2,
            weight: ranked[best].1,
            runner_up,
        })
    }
}

fn is_first_of_january(d: &str) -> bool {
    d.ends_with("-01-01")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Range {
        Range::default()
    }

    #[test]
    fn a_value_is_a_date_only_when_it_is_a_real_day() {
        assert_eq!(parse("20220115", r()).unwrap(), "2022-01-15");
        // non-conformant but unambiguous, which real scanners write
        assert_eq!(parse("2022-01-15", r()).unwrap(), "2022-01-15");
        // the ways of writing nothing
        assert!(parse("00000000", r()).is_none());
        assert!(parse("19000101", r()).is_none());
        assert!(parse("", r()).is_none());
        // not a day
        assert!(parse("20221345", r()).is_none());
        assert!(parse("20220230", r()).is_none());
        // a real day, out of range
        assert!(parse("17000101", r()).is_none());
        // a leap day is a day
        assert_eq!(parse("20200229", r()).unwrap(), "2020-02-29");
        assert!(parse("20210229", r()).is_none());
    }

    #[test]
    fn a_uid_gives_up_a_date_but_not_a_random_run_of_digits() {
        let uid = "1.2.826.0.1.3680043.8.498.10.4.20220519.1";
        assert_eq!(dates_in(uid, r()), ["2022-05-19"]);
        // month 13 is not a month
        assert!(dates_in("1.2.826.20221345.1", r()).is_empty());
        // and a longer number is not eight digits
        assert!(dates_in("1.2.826.202205191234.1", r()).is_empty());
    }

    #[test]
    fn epoch_from_the_corpus_uid() {
        let r = Range::default();
        let uid = "1.2.826.0.1.3680043.8.498.10.ge.1572249167.1";
        assert_eq!(epochs_in(uid, r), ["2019-10-28"], "epochs_in");
        assert_eq!(dates_in(uid, r), Vec::<String>::new(), "dates_in");
    }

    #[test]
    fn a_uid_gives_up_a_timestamp_too() {
        // what a GE scanner leaves in a SOP UID
        let uid = "1.2.840.113619.2.55.3.12869.1572249167.550";
        assert_eq!(epochs_in(uid, r()), ["2019-10-28"]);
        // and a ten-digit run that is not a plausible time is not one
        assert!(epochs_in("1.2.840.9999999999.1", r()).is_empty());
    }

    #[test]
    fn three_sources_outweigh_one() {
        let mut b = Ballot::new();
        b.element(Source::SeriesDate, Some("20230301"), r());
        b.element(Source::AcquisitionDate, Some("20230301"), r());
        b.element(Source::ContentDate, Some("20230415"), r());
        let v = b.verdict().unwrap();
        assert_eq!(v.date, "2023-03-01");
        assert_eq!(v.weight, 4);
        assert_eq!(v.runner_up, 2);
    }

    #[test]
    fn the_heaviest_source_names_the_date() {
        let mut b = Ballot::new();
        b.element(Source::StudyDate, Some("20220115"), r());
        b.element(Source::ContentDate, Some("20220115"), r());
        let v = b.verdict().unwrap();
        assert_eq!(v.source, Source::StudyDate, "the best reason, not the last");
        assert_eq!(v.weight, 6);
    }

    #[test]
    fn a_first_of_january_loses_to_anything_else() {
        // An anonymiser rewrote the creation date, which outweighs the series
        // date; the rule is what stops it winning anyway.
        let mut b = Ballot::new();
        b.element(Source::InstanceCreationDate, Some("20220101"), r());
        b.element(Source::SeriesDate, Some("20220615"), r());
        let v = b.verdict().unwrap();
        assert_eq!(v.date, "2022-06-15");
        assert_eq!(v.source, Source::SeriesDate);
    }

    #[test]
    fn a_first_of_january_wins_when_it_is_all_there_is() {
        let mut b = Ballot::new();
        b.element(Source::StudyDate, Some("20220101"), r());
        assert_eq!(b.verdict().unwrap().date, "2022-01-01");
    }

    #[test]
    fn nothing_at_all_is_no_verdict() {
        let mut b = Ballot::new();
        b.element(Source::StudyDate, Some("00000000"), r());
        b.element(Source::SeriesDate, None, r());
        b.inside(Source::Uid, "1.2.826.0.1.3680043.8.498.7.1", r());
        assert!(b.verdict().is_none());
    }

    #[test]
    fn a_tie_does_not_depend_on_arrival_order() {
        let mut a = Ballot::new();
        a.element(Source::SeriesDate, Some("20220301"), r());
        a.element(Source::ContentDate, Some("20220115"), r());
        let mut b = Ballot::new();
        b.element(Source::ContentDate, Some("20220115"), r());
        b.element(Source::SeriesDate, Some("20220301"), r());
        assert_eq!(a.verdict().unwrap().date, b.verdict().unwrap().date);
    }
}
