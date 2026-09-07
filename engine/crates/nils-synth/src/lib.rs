// SPDX-License-Identifier: AGPL-3.0-only

//! The synthetic registry (`docs/specs/wave4b-the-ask.md`, §13.1): made up
//! by design, never a sample of an archive, and deterministic from a seed,
//! so that a fixture's expected rows are known before an ask runs and the
//! same registry can be built on either backend and compared.
//!
//! What it plants: subjects with birth dates, a course history per subject
//! with and without an intermediate course, a transition event known to the
//! year beside the dated course rows, an EDSS and an SDMT population,
//! sessions that split and merge under different windows, one subject-day in
//! seven carrying two studies, protocols at 0.5 mm that differ only at the
//! exact comparability level and protocols that differ at every level,
//! cohorts with closed intervals, and the yardstick's positive cases and its
//! named negatives, one defect each, so that a funnel can name the set where
//! each falls out. The manifest returned says what was planted.

use std::collections::BTreeMap;

use nils_registry::clinical::{self, Vocabulary};
use nils_registry::day::Day;
use nils_registry::schema::table;
use nils_registry::{Error, Insert, Param, Registry, Store};
use serde::{Deserialize, Serialize};

/// The vocabulary the registry is planted with: one disease with four
/// courses, two scales, a diagnosis, and a transition known to the year.
pub const VOCABULARY: &str = r#"vocabulary:
  diseases:
    - name: Multiple Sclerosis
      code: G35
      types:
        - {name: CIS}
        - {name: RRMS}
        - {name: SPMS}
        - {name: PPMS}
  observation_types:
    - {name: EDSS, category: Clinical Measurement, value_type: numeric, unit: points, min: 0, max: 10, primary: true, description: Expanded Disability Status Scale}
    - {name: SDMT, category: Clinical Measurement, value_type: numeric, unit: correct, min: 0, max: 110, description: Symbol Digit Modalities Test}
    - {name: Diagnosis, category: Assessment, description: The official diagnosis, on its date}
    - {name: SP Transition, category: Assessment, precision: year, description: The transition to secondary progression, known to the year}
"#;

/// What to build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    pub seed: u64,
    /// Subjects in all; the first 24 are the yardstick's planted cases.
    pub subjects: usize,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            seed: 1,
            subjects: 240,
        }
    }
}

/// What was planted, for the gate to compare against.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Manifest {
    pub seed: u64,
    pub counts: Counts,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Counts {
    pub subjects: usize,
    pub subjects_with_birth_date: usize,
    pub studies: usize,
    pub subject_days_with_two_studies: usize,
    pub series: usize,
    pub stacks: usize,
    pub dispositions: BTreeMap<String, usize>,
    pub cohorts: usize,
    pub memberships_open: usize,
    pub memberships_closed: usize,
    pub course_rows: usize,
    pub course_rows_at_year: usize,
    pub transitions: usize,
    pub events: usize,
}

/// One planted case of the yardstick: which subject, and the first named
/// set of the layered reading it falls out of (`answer` for a positive).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Case {
    pub code: String,
    pub falls_out_at: String,
    pub note: String,
}

// ---------------------------------------------------------------- the dice

/// SplitMix64: enough randomness for a fixture, and the same on every
/// platform, which a library generator does not promise across versions.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

// ---------------------------------------------------------------- the shapes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Course {
    Cis,
    Rrms,
    Spms,
    Ppms,
}

impl Course {
    fn name(self) -> &'static str {
        match self {
            Course::Cis => "CIS",
            Course::Rrms => "RRMS",
            Course::Spms => "SPMS",
            Course::Ppms => "PPMS",
        }
    }
}

/// One acquisition as the fingerprint and the axes would record it.
#[derive(Debug, Clone, Copy)]
struct Protocol {
    description: &'static str,
    base: &'static str,
    technique: &'static str,
    modifier: Option<&'static str>,
    construct: Option<&'static str>,
    disposition: &'static str,
    acquisition_type: &'static str,
    orientation: &'static str,
    repetition_time: f64,
    echo_time: f64,
    inversion_time: Option<f64>,
    flip_angle: f64,
    /// In-plane spacing and slice thickness, isotropic when equal.
    spacing: f64,
    thickness: f64,
    instances: i64,
}

const MPRAGE_05: Protocol = Protocol {
    description: "t1_mprage_sag_iso_0.5",
    base: "T1w",
    technique: "MPRAGE",
    modifier: None,
    construct: None,
    disposition: "acquisition",
    acquisition_type: "3D",
    orientation: "Sagittal",
    repetition_time: 2300.0,
    echo_time: 2.98,
    inversion_time: Some(1000.0),
    flip_angle: 9.0,
    spacing: 0.5,
    thickness: 0.5,
    instances: 352,
};

/// Differs from `MPRAGE_05` at the exact level only: the timing moved by a
/// hair, which a comparison at `strict` or `loose` rounds away.
const MPRAGE_05_NEAR: Protocol = Protocol {
    description: "t1_mprage_sag_iso_0.5_v2",
    inversion_time: Some(1010.0),
    echo_time: 3.02,
    ..MPRAGE_05
};

/// Differs from `MPRAGE_05` at every level: no inversion, a plain gradient
/// echo, however close the resolution sits.
const GRE_05: Protocol = Protocol {
    description: "t1_gre_3d_iso_0.5",
    technique: "GRE",
    repetition_time: 25.0,
    echo_time: 4.0,
    inversion_time: None,
    flip_angle: 15.0,
    ..MPRAGE_05
};

const MPRAGE_10: Protocol = Protocol {
    description: "t1_mprage_sag_iso_1.0",
    spacing: 1.0,
    thickness: 1.0,
    instances: 176,
    ..MPRAGE_05
};

const FLAIR_05: Protocol = Protocol {
    description: "t2_space_dark-fluid_sag_iso_0.5",
    base: "T2w",
    technique: "TSE",
    modifier: Some("FLAIR"),
    construct: None,
    disposition: "acquisition",
    acquisition_type: "3D",
    orientation: "Sagittal",
    repetition_time: 5000.0,
    echo_time: 386.0,
    inversion_time: Some(1800.0),
    flip_angle: 120.0,
    spacing: 0.5,
    thickness: 0.5,
    instances: 352,
};

const FLAIR_10: Protocol = Protocol {
    description: "t2_space_dark-fluid_sag_iso_1.0",
    spacing: 1.0,
    thickness: 1.0,
    instances: 176,
    ..FLAIR_05
};

const T2_TSE_2D: Protocol = Protocol {
    description: "t2_tse_tra_3mm",
    base: "T2w",
    technique: "TSE",
    modifier: None,
    construct: None,
    disposition: "acquisition",
    acquisition_type: "2D",
    orientation: "Axial",
    repetition_time: 6000.0,
    echo_time: 98.0,
    inversion_time: None,
    flip_angle: 150.0,
    spacing: 0.5,
    thickness: 3.0,
    instances: 44,
};

/// A scanner reformat of the MPRAGE: a stack a question about acquisitions
/// must not read by default.
const MPR_AX: Protocol = Protocol {
    description: "t1_mprage_MPR_tra",
    construct: Some("MPR"),
    disposition: "scanner_derived",
    orientation: "Axial",
    instances: 176,
    ..MPRAGE_05
};

const LOCALIZER: Protocol = Protocol {
    description: "localizer",
    base: "T1w",
    technique: "GRE",
    modifier: None,
    construct: None,
    disposition: "scout",
    acquisition_type: "2D",
    orientation: "Axial",
    repetition_time: 8.6,
    echo_time: 4.0,
    inversion_time: None,
    flip_angle: 20.0,
    spacing: 1.0,
    thickness: 5.0,
    instances: 9,
};

/// What a site scans at every session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kit {
    /// The same 0.5 mm pair every time.
    Standard,
    /// The MPRAGE alternates between two timings: comparable at `loose` and
    /// `strict`, not at `exact`.
    AlternateExact,
    /// The T1 alternates between an MPRAGE and a plain GRE: comparable at no
    /// level.
    AlternateLoose,
    /// 1.0 mm.
    Low,
    /// No FLAIR.
    NoFlair,
}

impl Kit {
    fn protocols(self, session: usize) -> Vec<Protocol> {
        let t1 = match self {
            Kit::Standard | Kit::NoFlair => MPRAGE_05,
            Kit::AlternateExact => {
                if session.is_multiple_of(2) {
                    MPRAGE_05
                } else {
                    MPRAGE_05_NEAR
                }
            }
            Kit::AlternateLoose => {
                if session.is_multiple_of(2) {
                    MPRAGE_05
                } else {
                    GRE_05
                }
            }
            Kit::Low => MPRAGE_10,
        };
        let mut out = vec![LOCALIZER, t1];
        match self {
            Kit::Low => out.push(FLAIR_10),
            Kit::NoFlair => {}
            _ => out.push(FLAIR_05),
        }
        out.push(T2_TSE_2D);
        if t1.technique == "MPRAGE" {
            out.push(MPR_AX);
        }
        out
    }
}

/// One subject, before it is written.
struct Person {
    code: String,
    sex: &'static str,
    birth: Option<Day>,
    /// The course rows in order: the course, the day it was assigned, and
    /// whether the day is known only to its year.
    courses: Vec<(Course, Day, bool)>,
    diagnosis: Day,
    kit: Kit,
    /// Each session: its day, whether a second study lands the same day,
    /// the EDSS offset in days (none: no EDSS), the SDMT offset.
    sessions: Vec<Session>,
    cohorts: Vec<(&'static str, bool)>,
    case: Option<Case>,
}

#[derive(Debug, Clone, Copy)]
struct Session {
    day: Day,
    twice: bool,
    edss: Option<i64>,
    sdmt: Option<i64>,
}

fn day(year: i32, month: u32, day: u32) -> Day {
    Day::new(year, month, day).expect("a real day")
}

fn shift(d: Day, days: i64) -> Day {
    Day::from_days(d.to_days() + days)
}

fn years_after(d: Day, years: i32, extra_days: i64) -> Day {
    let y = d.year() + years;
    let m = d.month();
    let dd = d.day().min(28);
    shift(day(y, m, dd), extra_days)
}

fn iso(d: Day) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

fn code_of(i: usize) -> String {
    format!("SYN{:04}", i + 1)
}

fn session_at(day: Day, edss: Option<i64>, sdmt: Option<i64>) -> Session {
    Session {
        day,
        twice: false,
        edss,
        sdmt,
    }
}

/// The twenty-four planted cases, then the background.
fn people(plan: &Plan, rng: &mut Rng) -> Vec<Person> {
    let mut out = Vec::with_capacity(plan.subjects);
    for i in 0..plan.subjects.max(24) {
        out.push(person(i, rng));
    }
    out
}

/// A converter: PPMS at the diagnosis, SPMS at the transition, born so that
/// the transition falls at the age asked for.
fn converter(i: usize, transition_age: i32, rng: &mut Rng) -> Person {
    let birth = day(
        1960 + (i as i32 % 8),
        1 + (i as u32 % 12),
        1 + (i as u32 % 27),
    );
    let diagnosis = years_after(birth, 33, i as i64 % 200);
    let transition = years_after(birth, transition_age, 40 + i as i64 % 100);
    let sex = if rng.chance(60) { "F" } else { "M" };
    Person {
        code: code_of(i),
        sex,
        birth: Some(birth),
        courses: vec![
            (Course::Ppms, diagnosis, false),
            (Course::Spms, transition, false),
        ],
        diagnosis,
        kit: Kit::Standard,
        sessions: Vec::new(),
        cohorts: vec![("ms-cohort-a", true)],
        case: None,
    }
}

fn transition_of(p: &Person) -> Day {
    p.courses
        .iter()
        .find(|(c, _, _)| *c == Course::Spms)
        .map(|(_, d, _)| *d)
        .unwrap_or(p.diagnosis)
}

/// Follow-ups after the transition, one a year, with both scores close.
fn good_followups(p: &mut Person, n: usize, rng: &mut Rng) {
    let t = transition_of(p);
    for k in 0..n {
        let d = years_after(t, k as i32 + 1, rng.range(-20, 20));
        p.sessions.push(session_at(
            d,
            Some(rng.range(-40, 40)),
            Some(rng.range(-90, 90)),
        ));
    }
}

fn person(i: usize, rng: &mut Rng) -> Person {
    let case = |code: &str, at: &str, note: &str| {
        Some(Case {
            code: code.to_string(),
            falls_out_at: at.to_string(),
            note: note.to_string(),
        })
    };
    match i {
        // twelve positives, two of them with a second study on one day and
        // one with a session before the transition that must not count
        0..=11 => {
            let mut p = converter(i, 41 + (i as i32 % 3), rng);
            good_followups(&mut p, 3 + (i % 3), rng);
            if i.is_multiple_of(6) {
                p.sessions[1].twice = true;
            }
            if i == 5 {
                let before = shift(transition_of(&p), -200);
                p.sessions.push(session_at(before, Some(10), Some(10)));
            }
            p.case = case(
                &p.code,
                "answer",
                "a converter with three or more comparable follow-ups",
            );
            p
        }
        12 => {
            let mut p = converter(i, 42, rng);
            let t = transition_of(&p);
            p.courses = vec![(Course::Rrms, p.diagnosis, false), (Course::Spms, t, false)];
            good_followups(&mut p, 4, rng);
            p.case = case(&p.code, "converted", "RRMS to SPMS, not PPMS to SPMS");
            p
        }
        13 => {
            let mut p = converter(i, 42, rng);
            let t = transition_of(&p);
            let middle = years_after(p.diagnosis, 4, 0);
            p.courses = vec![
                (Course::Ppms, p.diagnosis, false),
                (Course::Rrms, middle, false),
                (Course::Spms, t, false),
            ];
            good_followups(&mut p, 4, rng);
            p.case = case(
                &p.code,
                "converted",
                "PPMS, then RRMS, then SPMS: a converter only under adjacent: false",
            );
            p
        }
        14 => {
            let mut p = converter(i, 42, rng);
            good_followups(&mut p, 2, rng);
            p.case = case(&p.code, "good", "two good follow-ups, not three");
            p
        }
        15 => {
            let mut p = converter(i, 42, rng);
            good_followups(&mut p, 3, rng);
            p.sessions[2].sdmt = Some(250);
            p.case = case(
                &p.code,
                "good",
                "one SDMT 250 days away: two good follow-ups",
            );
            p
        }
        16 => {
            let mut p = converter(i, 42, rng);
            good_followups(&mut p, 3, rng);
            p.sessions[0].sdmt = None;
            p.case = case(&p.code, "good", "one follow-up with no SDMT at all");
            p
        }
        17 => {
            let mut p = converter(i, 52, rng);
            good_followups(&mut p, 4, rng);
            p.case = case(
                &p.code,
                "followups",
                "the transition at 52: no follow-up inside the age window",
            );
            p
        }
        18 => {
            let mut p = converter(i, 42, rng);
            p.kit = Kit::AlternateLoose;
            good_followups(&mut p, 4, rng);
            p.case = case(
                &p.code,
                "comparable",
                "an MPRAGE and a plain GRE alternating: comparable at no level",
            );
            p
        }
        19 => {
            let mut p = converter(i, 42, rng);
            p.kit = Kit::AlternateExact;
            good_followups(&mut p, 4, rng);
            p.case = case(
                &p.code,
                "answer",
                "two MPRAGE timings alternating: comparable at loose and strict, not at exact",
            );
            p
        }
        20 => {
            let mut p = converter(i, 42, rng);
            p.kit = Kit::Low;
            good_followups(&mut p, 4, rng);
            p.case = case(&p.code, "good", "1.0 mm: neither acquisition attaches");
            p
        }
        21 => {
            let mut p = converter(i, 42, rng);
            p.birth = None;
            good_followups(&mut p, 4, rng);
            p.case = case(&p.code, "converted", "no birth date");
            p
        }
        22 => {
            let mut p = converter(i, 42, rng);
            p.kit = Kit::NoFlair;
            good_followups(&mut p, 4, rng);
            p.case = case(&p.code, "good", "no FLAIR at any session");
            p
        }
        23 => {
            // the precision case: the transition is known to its year, and
            // three of the four follow-ups fall inside that year
            let mut p = converter(i, 42, rng);
            let t = transition_of(&p);
            let year_start = day(t.year(), 1, 1);
            p.courses = vec![
                (Course::Ppms, p.diagnosis, false),
                (Course::Spms, year_start, true),
            ];
            for (m, dd) in [(3u32, 10u32), (6, 15), (9, 20)] {
                p.sessions.push(session_at(
                    day(t.year(), m, dd),
                    Some(rng.range(-30, 30)),
                    Some(rng.range(-60, 60)),
                ));
            }
            p.sessions.push(session_at(
                day(t.year() + 1, 4, 5),
                Some(rng.range(-30, 30)),
                Some(rng.range(-60, 60)),
            ));
            p.case = case(
                &p.code,
                "precision",
                "the transition known to the year: four follow-ups under the forgiving reading, one under strict",
            );
            p
        }
        _ => background(i, rng),
    }
}

fn background(i: usize, rng: &mut Rng) -> Person {
    let birth = if rng.chance(97) {
        Some(day(
            1950 + rng.range(0, 40) as i32,
            1 + rng.below(12) as u32,
            1 + rng.below(28) as u32,
        ))
    } else {
        None
    };
    let anchor = birth.unwrap_or(day(1970, 6, 15));
    let diagnosis = years_after(anchor, rng.range(25, 45) as i32, rng.range(0, 300));
    let pattern = rng.below(100);
    let mut courses = Vec::new();
    let coarse = |rng: &mut Rng| rng.chance(20);
    match pattern {
        0..=29 => courses.push((Course::Rrms, diagnosis, false)),
        30..=54 => {
            courses.push((Course::Rrms, diagnosis, false));
            let t = years_after(diagnosis, rng.range(3, 8) as i32, rng.range(0, 200));
            let year = coarse(rng);
            courses.push((
                Course::Spms,
                if year { day(t.year(), 1, 1) } else { t },
                year,
            ));
        }
        55..=74 => courses.push((Course::Ppms, diagnosis, false)),
        75..=89 => {
            courses.push((Course::Ppms, diagnosis, false));
            let t = years_after(diagnosis, rng.range(3, 8) as i32, rng.range(0, 200));
            let year = coarse(rng);
            courses.push((
                Course::Spms,
                if year { day(t.year(), 1, 1) } else { t },
                year,
            ));
        }
        90..=94 => {
            courses.push((Course::Ppms, diagnosis, false));
            let m = years_after(diagnosis, rng.range(2, 4) as i32, 0);
            courses.push((Course::Rrms, m, false));
            let t = years_after(m, rng.range(2, 5) as i32, rng.range(0, 200));
            courses.push((Course::Spms, t, false));
        }
        _ => {
            courses.push((Course::Cis, diagnosis, false));
            courses.push((
                Course::Rrms,
                years_after(diagnosis, rng.range(1, 3) as i32, 0),
                false,
            ));
        }
    }
    let kit = match rng.below(10) {
        0..=5 => Kit::Standard,
        6 => Kit::AlternateExact,
        7 => Kit::AlternateLoose,
        8 => Kit::Low,
        _ => Kit::NoFlair,
    };
    let n = rng.range(1, 6) as usize;
    let mut sessions = Vec::with_capacity(n);
    let mut d = shift(diagnosis, rng.range(0, 400));
    for _ in 0..n {
        let edss = if rng.chance(90) {
            Some(rng.range(-240, 240))
        } else {
            None
        };
        let sdmt = if rng.chance(75) {
            Some(rng.range(-240, 240))
        } else {
            None
        };
        sessions.push(Session {
            day: d,
            twice: rng.chance(14),
            edss,
            sdmt,
        });
        d = shift(d, rng.range(180, 900));
    }
    let mut cohorts = Vec::new();
    if !i.is_multiple_of(3) {
        // one in twenty left the cohort: a closed interval, not a member
        cohorts.push(("ms-cohort-a", !rng.chance(5)));
    }
    if i.is_multiple_of(2) {
        cohorts.push(("ms-cohort-b", true));
    }
    Person {
        code: code_of(i),
        sex: if rng.chance(65) { "F" } else { "M" },
        birth,
        courses,
        diagnosis,
        kit,
        sessions,
        cohorts,
        case: None,
    }
}

// ---------------------------------------------------------------- the writer

fn one_id(store: &mut Store, insert: &Insert<'_>, row: Vec<Param>) -> Result<i64, Error> {
    let rows = store.insert(insert, &[row])?;
    rows.first()
        .ok_or_else(|| Error::Message("an insert returned no id".into()))?
        .int(0)
}

fn text(v: &str) -> Param {
    Param::from(v)
}

fn opt_f(v: Option<f64>) -> Param {
    v.map_or(Param::Null, Param::Double)
}

/// Build the registry the plan describes into an initialised, empty
/// registry, in one transaction, and return the manifest.
pub fn build(registry: &mut Registry, plan: &Plan) -> Result<Manifest, Error> {
    let mut rng = Rng::new(plan.seed);
    let folk = people(plan, &mut rng);
    let mut manifest = Manifest {
        seed: plan.seed,
        ..Manifest::default()
    };
    let now = nils_registry::time::now_iso();

    let store = registry.store();
    store.begin()?;
    let result = write(store, &folk, plan, &now, &mut manifest);
    match result {
        Ok(()) => {}
        Err(e) => {
            store.rollback().ok();
            return Err(e);
        }
    }
    registry
        .next_epoch()
        .map_err(|e| Error::Message(e.to_string()))?;
    registry.store().commit()?;
    Ok(manifest)
}

fn write(
    store: &mut Store,
    folk: &[Person],
    plan: &Plan,
    now: &str,
    manifest: &mut Manifest,
) -> Result<(), Error> {
    let existing = store.query(
        &format!("SELECT COUNT(*) FROM {}", store.qualified("subject")),
        &[],
    )?[0]
        .int(0)?;
    if existing > 0 {
        return Err(Error::Message(format!(
            "the registry already holds {existing} subjects; the synthetic registry is built into an empty one"
        )));
    }

    // the vocabulary, and the ids it made
    let vocabulary = Vocabulary::parse(VOCABULARY).map_err(Error::Message)?;
    clinical::load(store, &vocabulary)?;
    let kinds = clinical::observation_types(store)?;
    let kind = |name: &str| -> Result<i64, Error> {
        kinds
            .iter()
            .find(|k| k.name == name)
            .map(|k| k.id)
            .ok_or_else(|| Error::Message(format!("no kind {name}")))
    };
    let edss = kind("EDSS")?;
    let sdmt = kind("SDMT")?;
    let diagnosis_kind = kind("Diagnosis")?;
    let transition_kind = kind("SP Transition")?;
    let disease = store
        .query_opt(
            &format!(
                "SELECT id FROM {} WHERE name = 'Multiple Sclerosis'",
                store.qualified("disease")
            ),
            &[],
        )?
        .ok_or_else(|| Error::Message("no disease".into()))?
        .int(0)?;
    let mut course_ids: BTreeMap<&'static str, i64> = BTreeMap::new();
    for r in store.query(
        &format!(
            "SELECT id, name FROM {} WHERE disease_id = {disease}",
            store.qualified("disease_type")
        ),
        &[],
    )? {
        let name = r.text(1)?.to_string();
        for c in [Course::Cis, Course::Rrms, Course::Spms, Course::Ppms] {
            if c.name() == name {
                course_ids.insert(c.name(), r.int(0)?);
            }
        }
    }

    // the provenance rows every catalogue row must name
    let source = one_id(
        store,
        &Insert::new(
            table("source"),
            &["root", "root_canonical", "first_seen_at"],
        )
        .returning(&["id"]),
        vec![
            text("synthetic://nils-synth"),
            text("synthetic://nils-synth"),
            text(now),
        ],
    )?;
    let job = one_id(
        store,
        &Insert::new(
            table("job"),
            &["kind", "name", "state", "started_at", "finished_at"],
        )
        .returning(&["id"]),
        vec![
            text("synth"),
            text(&format!("nils synth --seed {}", plan.seed)),
            text("done"),
            text(now),
            text(now),
        ],
    )?;
    let batch = one_id(
        store,
        &Insert::new(
            table("ingest_batch"),
            &[
                "source_id",
                "job_id",
                "name",
                "config",
                "started_at",
                "finished_at",
                "state",
            ],
        )
        .returning(&["id"]),
        vec![
            Param::Int(source),
            Param::Int(job),
            text("synthetic"),
            text(&serde_json::json!({"seed": plan.seed, "subjects": plan.subjects}).to_string()),
            text(now),
            text(now),
            text("done"),
        ],
    )?;
    let epoch = store
        .query_opt(
            &format!(
                "SELECT value FROM {} WHERE key = 'epoch'",
                store.qualified("registry_meta")
            ),
            &[],
        )?
        .and_then(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(0)
        + 1;

    // cohorts
    let cohort_insert = Insert::new(
        table("cohort"),
        &["name", "owner", "description", "created_at"],
    )
    .returning(&["id"]);
    let mut cohorts: BTreeMap<&'static str, i64> = BTreeMap::new();
    for (name, description) in [
        (
            "ms-cohort-a",
            "the larger synthetic cohort; a twentieth of its members left",
        ),
        (
            "ms-cohort-b",
            "the smaller synthetic cohort; every other subject, so many are in both",
        ),
    ] {
        let id = one_id(
            store,
            &cohort_insert,
            vec![text(name), text("nils-synth"), text(description), text(now)],
        )?;
        cohorts.insert(name, id);
    }
    manifest.counts.cohorts = cohorts.len();

    let subject_insert = Insert::new(
        table("subject"),
        &["code", "birth_date", "sex", "first_batch_id", "created_at"],
    )
    .returning(&["id"]);
    let study_insert = Insert::new(
        table("study"),
        &[
            "study_instance_uid",
            "subject_id",
            "study_date",
            "study_description",
            "manufacturer",
            "manufacturer_model_name",
            "station_name",
            "first_batch_id",
            "date_filled",
            "date_source",
        ],
    )
    .returning(&["id"]);
    let series_insert = Insert::new(
        table("series"),
        &[
            "series_instance_uid",
            "study_id",
            "subject_id",
            "modality",
            "series_description",
            "protocol_name",
            "sequence_name",
            "series_date",
            "n_instances",
            "n_stacks",
            "first_batch_id",
        ],
    )
    .returning(&["id"]);
    let series_mr_insert = Insert::new(
        table("series_mr"),
        &[
            "series_id",
            "mr_acquisition_type",
            "repetition_time",
            "echo_time",
            "inversion_time",
            "flip_angle",
            "magnetic_field_strength",
        ],
    );
    let stack_insert = Insert::new(
        table("stack"),
        &[
            "series_id",
            "stack_index",
            "stack_key",
            "modality",
            "orientation",
            "orientation_confidence",
            "n_instances",
            "first_batch_id",
            "echo_time",
            "repetition_time",
            "inversion_time",
            "flip_angle",
        ],
    )
    .returning(&["id"]);
    let fingerprint_insert = Insert::new(
        table("stack_fingerprint"),
        &[
            "stack_id",
            "series_id",
            "study_id",
            "subject_id",
            "modality",
            "text_series_description",
            "text_protocol_name",
            "text_all",
            "text_series_description_ci",
            "text_protocol_name_ci",
            "text_all_ci",
            "echo_time",
            "repetition_time",
            "inversion_time",
            "flip_angle",
            "magnetic_field_strength",
            "slice_thickness",
            "spacing_between_slices",
            "mr_acquisition_type",
            "orientation",
            "orientation_confidence",
            "n_instances",
            "stack_index",
            "stacks_in_series",
            "rows",
            "columns",
            "pixel_spacing",
            "pixel_spacing_row",
            "pixel_spacing_col",
            "manufacturer",
            "manufacturer_model_name",
            "station_name",
            "field_strength_tesla",
            "field_strength_normalized",
            "field_strength_unit",
            "acquisition_type_filled",
            "acquisition_type_source",
            "image_role",
            "job_id",
            "epoch",
        ],
    );
    let classification_insert = Insert::new(
        table("classification"),
        &[
            "stack_id",
            "pack",
            "pack_version",
            "contract",
            "job_id",
            "epoch",
            "review_items",
        ],
    );
    let axis_insert = Insert::new(
        table("classification_axis"),
        &["stack_id", "axis", "value", "confidence", "tier"],
    );
    let member_insert = Insert::new(
        table("cohort_member"),
        &[
            "cohort_id",
            "subject_id",
            "joined_at",
            "left_at",
            "left_by",
            "source",
            "actor",
        ],
    );
    let subject_disease_insert = Insert::new(
        table("subject_disease"),
        &[
            "subject_id",
            "disease_id",
            "diagnosis_event_id",
            "created_at",
            "actor",
        ],
    )
    .returning(&["id"]);
    let course_insert = Insert::new(
        table("subject_disease_type"),
        &[
            "subject_disease_id",
            "disease_type_id",
            "assigned_on",
            "assigned_on_precision",
            "created_at",
            "actor",
        ],
    );
    let event_insert = Insert::new(
        table("event"),
        &[
            "subject_id",
            "observation_type_id",
            "event_date",
            "event_date_precision",
            "value",
            "number",
            "unit",
            "source",
            "created_at",
            "actor",
        ],
    )
    .returning(&["id"]);

    let mut uid = 0u64;
    let mut next_uid = |what: &str| {
        uid += 1;
        format!("2.25.{}.{what}.{uid}", plan.seed)
    };
    let mut rng = Rng::new(plan.seed ^ 0xA5A5_A5A5);

    for (i, p) in folk.iter().enumerate() {
        let subject = one_id(
            store,
            &subject_insert,
            vec![
                text(&p.code),
                p.birth.map_or(Param::Null, |d| text(&iso(d))),
                text(p.sex),
                Param::Int(batch),
                text(now),
            ],
        )?;
        manifest.counts.subjects += 1;
        if p.birth.is_some() {
            manifest.counts.subjects_with_birth_date += 1;
        }

        // the clinical layer: the diagnosis, the courses, the transition
        let diagnosis_event = one_id(
            store,
            &event_insert,
            vec![
                Param::Int(subject),
                Param::Int(diagnosis_kind),
                text(&iso(p.diagnosis)),
                text("day"),
                Param::Null,
                Param::Null,
                Param::Null,
                text("synthetic"),
                text(now),
                text("nils-synth"),
            ],
        )?;
        manifest.counts.events += 1;
        let subject_disease = one_id(
            store,
            &subject_disease_insert,
            vec![
                Param::Int(subject),
                Param::Int(disease),
                Param::Int(diagnosis_event),
                text(now),
                text("nils-synth"),
            ],
        )?;
        for (course, on, year) in &p.courses {
            let type_id = *course_ids
                .get(course.name())
                .ok_or_else(|| Error::Message(format!("no course {}", course.name())))?;
            store.insert(
                &course_insert,
                &[vec![
                    Param::Int(subject_disease),
                    Param::Int(type_id),
                    text(&iso(*on)),
                    text(if *year { "year" } else { "day" }),
                    text(now),
                    text("nils-synth"),
                ]],
            )?;
            manifest.counts.course_rows += 1;
            if *year {
                manifest.counts.course_rows_at_year += 1;
            }
            if *course == Course::Spms {
                // the transition as the clinic records it: a kind known to
                // its year, beside the dated course row
                store.insert(
                    &event_insert,
                    &[vec![
                        Param::Int(subject),
                        Param::Int(transition_kind),
                        text(&iso(day(on.year(), 1, 1))),
                        text("year"),
                        Param::Null,
                        Param::Null,
                        Param::Null,
                        text("synthetic"),
                        text(now),
                        text("nils-synth"),
                    ]],
                )?;
                manifest.counts.events += 1;
                manifest.counts.transitions += 1;
            }
        }

        // cohorts
        for (name, open) in &p.cohorts {
            let joined = shift(p.diagnosis, 30 + i as i64 % 300);
            let (left_at, left_by) = if *open {
                (Param::Null, Param::Null)
            } else {
                (
                    text(&format!("{}T00:00:00Z", iso(shift(joined, 400)))),
                    text("nils-synth"),
                )
            };
            store.insert(
                &member_insert,
                &[vec![
                    Param::Int(cohorts[name]),
                    Param::Int(subject),
                    text(&format!("{}T00:00:00Z", iso(joined))),
                    left_at,
                    left_by,
                    text("import"),
                    text("nils-synth"),
                ]],
            )?;
            if *open {
                manifest.counts.memberships_open += 1;
            } else {
                manifest.counts.memberships_closed += 1;
            }
        }

        // the imaging
        for (s, session) in p.sessions.iter().enumerate() {
            let studies = if session.twice { 2 } else { 1 };
            if session.twice {
                manifest.counts.subject_days_with_two_studies += 1;
            }
            for k in 0..studies {
                let study = one_id(
                    store,
                    &study_insert,
                    vec![
                        text(&next_uid("study")),
                        Param::Int(subject),
                        text(&iso(session.day)),
                        text(if k == 0 {
                            "MS follow-up"
                        } else {
                            "MS follow-up, second sitting"
                        }),
                        text("SYNTHETIC"),
                        text("Model S"),
                        text("SYN1"),
                        Param::Int(batch),
                        text(&iso(session.day)),
                        text("study_date"),
                    ],
                )?;
                manifest.counts.studies += 1;
                let protocols: Vec<Protocol> = if k == 0 {
                    p.kit.protocols(s)
                } else {
                    vec![LOCALIZER, T2_TSE_2D]
                };
                for (n, proto) in protocols.iter().enumerate() {
                    let series = one_id(
                        store,
                        &series_insert,
                        vec![
                            text(&next_uid("series")),
                            Param::Int(study),
                            Param::Int(subject),
                            text("MR"),
                            text(proto.description),
                            text(proto.description),
                            text(proto.technique),
                            text(&iso(session.day)),
                            Param::Int(proto.instances),
                            Param::Int(1),
                            Param::Int(batch),
                        ],
                    )?;
                    manifest.counts.series += 1;
                    store.insert(
                        &series_mr_insert,
                        &[vec![
                            Param::Int(series),
                            text(proto.acquisition_type),
                            Param::Double(proto.repetition_time),
                            Param::Double(proto.echo_time),
                            opt_f(proto.inversion_time),
                            Param::Double(proto.flip_angle),
                            Param::Double(3.0),
                        ]],
                    )?;
                    let stack = one_id(
                        store,
                        &stack_insert,
                        vec![
                            Param::Int(series),
                            Param::Int(0),
                            text(&format!("synth-{n}")),
                            text("MR"),
                            text(proto.orientation),
                            Param::Double(1.0),
                            Param::Int(proto.instances),
                            Param::Int(batch),
                            Param::Double(proto.echo_time),
                            Param::Double(proto.repetition_time),
                            opt_f(proto.inversion_time),
                            Param::Double(proto.flip_angle),
                        ],
                    )?;
                    manifest.counts.stacks += 1;
                    let text_all = format!(
                        "{} {} {}",
                        proto.description, proto.description, proto.technique
                    );
                    let spacing = format!("{}\\{}", proto.spacing, proto.spacing);
                    let matrix = (240.0 / proto.spacing).round() as i64;
                    store.insert(
                        &fingerprint_insert,
                        &[vec![
                            Param::Int(stack),
                            Param::Int(series),
                            Param::Int(study),
                            Param::Int(subject),
                            text("MR"),
                            text(proto.description),
                            text(proto.description),
                            text(&text_all),
                            text(&proto.description.to_lowercase()),
                            text(&proto.description.to_lowercase()),
                            text(&text_all.to_lowercase()),
                            Param::Double(proto.echo_time),
                            Param::Double(proto.repetition_time),
                            opt_f(proto.inversion_time),
                            Param::Double(proto.flip_angle),
                            Param::Double(3.0),
                            Param::Double(proto.thickness),
                            Param::Double(proto.thickness),
                            text(proto.acquisition_type),
                            text(proto.orientation),
                            Param::Double(1.0),
                            Param::Int(proto.instances),
                            Param::Int(0),
                            Param::Int(1),
                            Param::Int(matrix),
                            Param::Int(matrix),
                            text(&spacing),
                            Param::Double(proto.spacing),
                            Param::Double(proto.spacing),
                            text("SYNTHETIC"),
                            text("Model S"),
                            text("SYN1"),
                            Param::Double(3.0),
                            Param::Double(3.0),
                            text("T"),
                            text(proto.acquisition_type),
                            text("measured"),
                            text(if proto.construct.is_some() {
                                "derived"
                            } else {
                                "original"
                            }),
                            Param::Int(job),
                            Param::Int(epoch),
                        ]],
                    )?;
                    store.insert(
                        &classification_insert,
                        &[vec![
                            Param::Int(stack),
                            text("mri"),
                            text("synthetic"),
                            Param::Int(2),
                            Param::Int(job),
                            Param::Int(epoch),
                            Param::Int(0),
                        ]],
                    )?;
                    let mut axes: Vec<Vec<Param>> = vec![
                        axis_row(stack, "base", proto.base),
                        axis_row(stack, "technique", proto.technique),
                        axis_row(stack, "disposition", proto.disposition),
                        axis_row(stack, "body_part", "Brain"),
                    ];
                    if let Some(m) = proto.modifier {
                        axes.push(axis_row(stack, "modifier", m));
                    }
                    if let Some(c) = proto.construct {
                        axes.push(axis_row(stack, "construct", c));
                    }
                    store.insert(&axis_insert, &axes)?;
                    *manifest
                        .counts
                        .dispositions
                        .entry(proto.disposition.to_string())
                        .or_insert(0) += 1;
                }
            }
            // the scores near the session
            for (kind_id, offset, unit, lo, hi) in [
                (edss, session.edss, "points", 0i64, 17i64),
                (sdmt, session.sdmt, "correct", 20, 70),
            ] {
                let Some(offset) = offset else { continue };
                let value = rng.range(lo, hi);
                let number = if kind_id == edss {
                    value as f64 / 2.0
                } else {
                    value as f64
                };
                store.insert(
                    &event_insert,
                    &[vec![
                        Param::Int(subject),
                        Param::Int(kind_id),
                        text(&iso(shift(session.day, offset))),
                        text("day"),
                        text(&number.to_string()),
                        Param::Double(number),
                        text(unit),
                        text("synthetic"),
                        text(now),
                        text("nils-synth"),
                    ]],
                )?;
                manifest.counts.events += 1;
            }
        }
        if let Some(case) = &p.case {
            manifest.cases.push(case.clone());
        }
    }
    Ok(())
}

fn axis_row(stack: i64, axis: &str, value: &str) -> Vec<Param> {
    vec![
        Param::Int(stack),
        Param::from(axis),
        Param::from(value),
        Param::Double(0.9),
        Param::from("stated"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dice_are_the_same_on_every_run() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let x: Vec<u64> = (0..8).map(|_| a.next()).collect();
        let y: Vec<u64> = (0..8).map(|_| b.next()).collect();
        assert_eq!(x, y);
        assert_ne!(x, (0..8).map(|_| Rng::new(8).next()).collect::<Vec<_>>());
    }

    #[test]
    fn the_planted_cases_are_twenty_four_and_named() {
        let mut rng = Rng::new(1);
        let folk = people(&Plan::default(), &mut rng);
        let cases: Vec<&Case> = folk.iter().filter_map(|p| p.case.as_ref()).collect();
        assert_eq!(cases.len(), 24);
        assert_eq!(
            cases.iter().filter(|c| c.falls_out_at == "answer").count(),
            13
        );
        assert!(folk[23].courses.iter().any(|(_, _, year)| *year));
        assert!(
            folk.iter()
                .skip(24)
                .any(|p| p.sessions.iter().any(|s| s.twice))
        );
    }
}
