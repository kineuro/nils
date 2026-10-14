// SPDX-License-Identifier: AGPL-3.0-only

//! The post-contrast label over the registry (record 55 F2 and F5, spec
//! §12.3).
//!
//! The rule, its checks and the subject step are `nils_pack::contrast`'s, a
//! pure function; what is here is the registry: which stacks a subject's
//! sessions hold and what the rule reads of each, and the rows it writes.
//!
//! - **A session is a study** (one StudyInstanceUID), as the time-rule and
//!   time-label studies counted them; two studies of one visit are C1's and
//!   C2's concern, through the subject step.
//! - **The stacks** are a subject's MR stacks with their fingerprint (series
//!   number, the physics, the field strength), the axes the pack decided in
//!   force (base, modifier, technique, body part, directory type and
//!   disposition, read through the pack's vocabulary to its identities),
//!   their images' acquisition and content times aggregated per stack and
//!   acquisition date, and whether their series carries a contrast agent.
//! - **The T1 anchors** are the certified answers on the post-contrast axis
//!   (`given` post, `not_given` pre) of stacks of the T1 family: a person's
//!   decision in force (a settled read), or the image model's, staged at or
//!   above its card's threshold or committed since. A model stages only at
//!   or above its threshold and only when admitted or promoted
//!   (`nils_registry::proposals`), so a staged model answer is a certified
//!   one. An agent's decision is neither a read nor the image model, and is
//!   not an anchor. A stack of a sample sealed now is never an anchor
//!   (record 48), and its label says `sealed`.
//! - **What is written** is `contrast_label`, one row a stack, replaced
//!   whole for the subjects labelled.
//!
//! Labelling runs after every classification, after a run of a pipeline
//! that proposes the post-contrast axis, after a campaign closes, and on
//! `nils contrast label`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use nils_pack::Pack;
use nils_pack::contrast::{
    self, Answer, By, Config, Described, Label, Outside, Region, Scan, Study,
};
use nils_registry::dialect::Dialect;
use nils_registry::schema::table;
use nils_registry::store::{Insert, Param, Store};
use nils_registry::time::{days_from_civil, now_iso};
use serde_json::{Value, json};

use crate::job::Error;

/// How many subjects are read and written at once.
const CHUNK: usize = 200;

/// What a labelling did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labelled {
    pub subjects: usize,
    pub sessions: usize,
    pub stacks: usize,
    /// Sessions holding a certified T1 answer, and the stacks in them.
    pub anchored_sessions: usize,
    pub anchored_stacks: usize,
    pub by_state: BTreeMap<String, usize>,
    pub seconds: f64,
}

impl Labelled {
    pub fn to_json(&self) -> Value {
        json!({
            "rule": contrast::RULE,
            "subjects": self.subjects,
            "sessions": self.sessions,
            "stacks": self.stacks,
            "anchored_sessions": self.anchored_sessions,
            "anchored_stacks": self.anchored_stacks,
            "by_state": self.by_state,
            "seconds": self.seconds,
        })
    }
}

fn ids(v: &[i64]) -> String {
    v.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
}

/// A date (`YYYY-MM-DD`) as a day number.
fn day_of(text: &str) -> Option<i64> {
    let t = text.get(..10)?;
    let (y, m, d) = (t.get(..4)?, t.get(5..7)?, t.get(8..10)?);
    Some(days_from_civil(
        y.parse().ok()?,
        m.parse().ok()?,
        d.parse().ok()?,
    ))
}

/// A time (`HH:MM:SS` with a fraction or none) as seconds of the day.
fn seconds_of(text: &str) -> Option<f64> {
    let (h, rest) = text.split_once(':')?;
    let (m, s) = rest.split_once(':')?;
    Some(
        h.trim().parse::<f64>().ok()? * 3600.0
            + m.parse::<f64>().ok()? * 60.0
            + s.trim().parse::<f64>().ok()?,
    )
}

/// Every subject with an MR stack, in order.
fn all_subjects(store: &mut Store) -> Result<Vec<i64>, Error> {
    let sql = format!(
        "SELECT DISTINCT se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id \
         WHERE x.modality = 'MR' ORDER BY se.subject_id",
        store.qualified("stack"),
        store.qualified("series"),
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| r.int(0).map_err(Error::Store))
        .collect()
}

/// The subjects whose stacks are named.
pub fn subjects_of(store: &mut Store, stacks: &[i64]) -> Result<Vec<i64>, Error> {
    let mut out = BTreeSet::new();
    for chunk in stacks.chunks(1000) {
        let sql = format!(
            "SELECT DISTINCT se.subject_id FROM {} x JOIN {} se ON se.id = x.series_id WHERE x.id IN ({})",
            store.qualified("stack"),
            store.qualified("series"),
            ids(chunk)
        );
        for r in store.query(&sql, &[])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out.into_iter().collect())
}

/// Label every session of the subjects named, or of every subject with an
/// MR stack, and write their rows.
pub fn label(store: &mut Store, pack: &Pack, subjects: Option<&[i64]>) -> Result<Labelled, Error> {
    let started = Instant::now();
    let mut done = Labelled::default();
    // a pack without the post-contrast axis labels nothing
    if !pack.axes.iter().any(|a| a.name == "post_contrast") {
        return Ok(done);
    }
    let subjects = match subjects {
        Some(s) => s.to_vec(),
        None => all_subjects(store)?,
    };
    let cfg = Config::default();
    let vocab = Vocabulary::of(pack);
    for chunk in subjects.chunks(CHUNK) {
        let studies = read(store, &vocab, chunk)?;
        let mut rows: Vec<(i64, i64, Label)> = Vec::new();
        let mut by_subject: BTreeMap<i64, Vec<Study>> = BTreeMap::new();
        for (subject, study) in studies {
            by_subject.entry(subject).or_default().push(study);
        }
        for (subject, studies) in &by_subject {
            for passed in contrast::label(&cfg, studies) {
                done.sessions += 1;
                if passed.labels.iter().any(|l| l.anchored) {
                    done.anchored_sessions += 1;
                }
                for l in passed.labels {
                    rows.push((*subject, passed.study, l));
                }
            }
        }
        done.subjects += by_subject.len();
        write(store, chunk, &rows)?;
        for (_, _, l) in &rows {
            done.stacks += 1;
            if l.anchored {
                done.anchored_stacks += 1;
            }
            *done.by_state.entry(l.state.name().to_string()).or_default() += 1;
        }
    }
    done.seconds = started.elapsed().as_secs_f64();
    Ok(done)
}

/// The pack's vocabulary the rule reads: each stored value of the axes it
/// reads, to the value's identity, and which techniques are gradient echo.
struct Vocabulary {
    identity: HashMap<(String, String), String>,
    gradient_echo: BTreeSet<String>,
}

impl Vocabulary {
    fn of(pack: &Pack) -> Vocabulary {
        let mut identity = HashMap::new();
        let mut gradient_echo = BTreeSet::new();
        for a in &pack.axes {
            for v in &a.values {
                identity.insert((a.name.clone(), v.label.clone()), v.id.clone());
                identity.insert((a.name.clone(), v.id.clone()), v.id.clone());
                if a.name == "technique" && v.family.as_deref() == Some("GRE") {
                    gradient_echo.insert(v.id.clone());
                }
            }
        }
        Vocabulary {
            identity,
            gradient_echo,
        }
    }

    fn id(&self, axis: &str, stored: &str) -> String {
        self.identity
            .get(&(axis.to_string(), stored.to_string()))
            .cloned()
            .unwrap_or_else(|| stored.to_string())
    }
}

/// A stack's images of one acquisition date: the day (none where they
/// carry no date), the earliest and latest acquisition time, how many
/// images, how many with a time, how many with content time before
/// acquisition time, and how many with a derivation description.
type Images = (Option<i64>, Option<f64>, Option<f64>, i64, i64, i64, i64);

/// What a stack's row reads, before it is a [`Scan`].
#[derive(Default)]
struct Raw {
    stack: i64,
    series: i64,
    study: i64,
    series_number: Option<i64>,
    tr: Option<f64>,
    te: Option<f64>,
    ti: Option<f64>,
    field: Option<f64>,
    acquisition: Option<String>,
    image_type: Option<String>,
    earliest_date: Option<String>,
    earliest_time: Option<String>,
    agent: bool,
    axes: BTreeMap<String, Vec<String>>,
    images: Vec<Images>,
    sealed: bool,
    answer: Option<(u8, i64, Answer)>,
}

/// Read the sessions of a chunk of subjects, each with its subject.
fn read(
    store: &mut Store,
    vocab: &Vocabulary,
    subjects: &[i64],
) -> Result<Vec<(i64, Study)>, Error> {
    let d = store.dialect();
    let text = |alias: &str, t: &str, c: &str| {
        d.text_of_qualified(
            Some(alias),
            table(t).column(c).unwrap_or_else(|| panic!("{t}.{c}")),
        )
    };
    let subj = ids(subjects);
    // the studies, with their own date and time
    let sql = format!(
        "SELECT y.id, y.subject_id, {}, {} FROM {} y WHERE y.subject_id IN ({subj}) ORDER BY y.id",
        text("y", "study", "study_date"),
        text("y", "study", "study_time"),
        store.qualified("study"),
    );
    let mut studies: BTreeMap<i64, (i64, Option<i64>, Study)> = BTreeMap::new();
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        let day = r.opt_text(2)?.and_then(day_of);
        let at = day.map(|day| {
            day as f64 * 1440.0
                + r.opt_text(3)
                    .ok()
                    .flatten()
                    .and_then(seconds_of)
                    .unwrap_or(0.0)
                    / 60.0
        });
        studies.insert(
            id,
            (
                r.int(1)?,
                day,
                Study {
                    id,
                    at_minutes: at,
                    mr_series: 0,
                    mr_tagged: 0,
                    ct_tagged: 0,
                    scans: Vec::new(),
                },
            ),
        );
    }
    if studies.is_empty() {
        return Ok(Vec::new());
    }
    let study_ids: Vec<i64> = studies.keys().copied().collect();
    // the series: MR and CT, and which carry a contrast agent
    let sql = format!(
        "SELECT se.study_id, se.id, se.modality, se.contrast_bolus_agent FROM {} se WHERE se.study_id IN ({})",
        store.qualified("series"),
        ids(&study_ids)
    );
    let mut agent_series: BTreeSet<i64> = BTreeSet::new();
    for r in store.query(&sql, &[])? {
        let Some((_, _, s)) = studies.get_mut(&r.int(0)?) else {
            continue;
        };
        let tagged = r
            .opt_text(3)?
            .is_some_and(|t| t.chars().any(|c| c.is_ascii_alphanumeric()));
        match r.opt_text(2)? {
            Some("MR") => {
                s.mr_series += 1;
                if tagged {
                    s.mr_tagged += 1;
                    agent_series.insert(r.int(1)?);
                }
            }
            Some("CT") if tagged => s.ct_tagged += 1,
            _ => {}
        }
    }
    // the MR stacks, with their fingerprint
    let sql = format!(
        "SELECT x.id, x.series_id, se.study_id, f.series_number, f.repetition_time, f.echo_time, \
         f.inversion_time, f.field_strength_normalized, COALESCE(f.mr_acquisition_type, f.acquisition_type_filled), \
         x.image_type, {}, {} FROM {} x JOIN {} se ON se.id = x.series_id \
         LEFT JOIN {} f ON f.stack_id = x.id WHERE x.modality = 'MR' AND se.study_id IN ({}) ORDER BY x.id",
        text("f", "stack_fingerprint", "earliest_acquisition_date"),
        text("f", "stack_fingerprint", "earliest_acquisition_time"),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
        ids(&study_ids)
    );
    let mut raws: BTreeMap<i64, Raw> = BTreeMap::new();
    for r in store.query(&sql, &[])? {
        let series = r.int(1)?;
        raws.insert(
            r.int(0)?,
            Raw {
                stack: r.int(0)?,
                series,
                study: r.int(2)?,
                series_number: r.opt_int(3)?,
                tr: r.opt_double(4)?,
                te: r.opt_double(5)?,
                ti: r.opt_double(6)?,
                field: r.opt_double(7)?,
                acquisition: r.opt_text(8)?.map(str::to_string),
                image_type: r.opt_text(9)?.map(str::to_string),
                earliest_date: r.opt_text(10)?.map(str::to_string),
                earliest_time: r.opt_text(11)?.map(str::to_string),
                agent: agent_series.contains(&series),
                ..Raw::default()
            },
        );
    }
    let stack_ids: Vec<i64> = raws.keys().copied().collect();
    for chunk in stack_ids.chunks(2000) {
        let list = ids(chunk);
        // the axes in force
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) AND axis IN \
             ('base', 'modifier', 'technique', 'body_part', 'directory_type', 'disposition') AND value IS NOT NULL",
            store.qualified("classification_axis"),
        );
        for r in store.query(&sql, &[])? {
            if let Some(raw) = raws.get_mut(&r.int(0)?) {
                let axis = r.text(1)?.to_string();
                let value = vocab.id(&axis, r.text(2)?);
                raw.axes.entry(axis).or_default().push(value);
            }
        }
        // the images' times, per stack and acquisition date
        let content_early = match d {
            Dialect::Sqlite => {
                "CASE WHEN i.content_time IS NOT NULL AND i.acquisition_time IS NOT NULL AND \
                 julianday(COALESCE(i.content_date, i.acquisition_date, '2000-01-01') || ' ' || i.content_time) < \
                 julianday(COALESCE(i.acquisition_date, i.content_date, '2000-01-01') || ' ' || i.acquisition_time) \
                 - 60.0 / 86400.0 THEN 1 ELSE 0 END"
                    .to_string()
            }
            Dialect::Postgres => {
                "CASE WHEN i.content_time IS NOT NULL AND i.acquisition_time IS NOT NULL AND \
                 (COALESCE(i.content_date, i.acquisition_date, DATE '2000-01-01') + i.content_time) < \
                 (COALESCE(i.acquisition_date, i.content_date, DATE '2000-01-01') + i.acquisition_time) \
                 - INTERVAL '60 seconds' THEN 1 ELSE 0 END"
                    .to_string()
            }
        };
        let (min_t, max_t) = match d {
            Dialect::Sqlite => (
                "MIN(i.acquisition_time)".to_string(),
                "MAX(i.acquisition_time)".to_string(),
            ),
            Dialect::Postgres => (
                "to_char(MIN(i.acquisition_time), 'HH24:MI:SS.US')".to_string(),
                "to_char(MAX(i.acquisition_time), 'HH24:MI:SS.US')".to_string(),
            ),
        };
        let date = match d {
            Dialect::Sqlite => "i.acquisition_date".to_string(),
            Dialect::Postgres => "i.acquisition_date::text".to_string(),
        };
        let sql = format!(
            "SELECT i.stack_id, {date}, {min_t}, {max_t}, COUNT(*), COUNT(i.acquisition_time), \
             SUM({content_early}), COUNT(i.derivation_description) FROM {} i \
             WHERE i.stack_id IN ({list}) GROUP BY i.stack_id, i.acquisition_date",
            store.qualified("instance"),
        );
        for r in store.query(&sql, &[])? {
            if let Some(raw) = raws.get_mut(&r.int(0)?) {
                raw.images.push((
                    r.opt_text(1)?.and_then(day_of),
                    r.opt_text(2)?.and_then(seconds_of),
                    r.opt_text(3)?.and_then(seconds_of),
                    r.int(4)?,
                    r.int(5)?,
                    r.opt_int(6)?.unwrap_or(0),
                    r.int(7)?,
                ));
            }
        }
        // sealed now
        let sql = format!(
            "SELECT DISTINCT stack_id FROM {} WHERE stack_id IN ({list}) AND unsealed_at IS NULL",
            store.qualified("sealed_stack"),
        );
        for r in store.query(&sql, &[])? {
            if let Some(raw) = raws.get_mut(&r.int(0)?) {
                raw.sealed = true;
            }
        }
        // the post-contrast answers: in force, or a model's staged
        let refs = chunk
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let alive = "d.axis = 'post_contrast' AND d.withdrawn_at IS NULL \
                     AND (d.staged_at IS NULL OR d.committed_at IS NOT NULL OR d.author_kind = 'model')";
        let decision = store.qualified("decision");
        let sql = format!(
            "SELECT CAST(d.ref AS {int}), d.value, d.author_kind, d.id FROM {decision} d \
             WHERE {alive} AND d.scope = 'stack' AND d.ref IN ({refs}) \
             UNION ALL SELECT rm.stack_id, d.value, d.author_kind, d.id FROM {decision} d \
             JOIN {} rm ON CAST(rm.item_id AS TEXT) = d.ref \
             WHERE {alive} AND d.scope = 'group' AND rm.stack_id IN ({list})",
            store.qualified("review_member"),
            int = match d {
                Dialect::Sqlite => "INTEGER",
                Dialect::Postgres => "BIGINT",
            },
        );
        for r in store.query(&sql, &[])? {
            let kind = r.opt_text(2)?.unwrap_or("person");
            let by = match kind {
                "person" => By::Reader,
                "model" => By::Image,
                // an agent's call is neither a read nor the image model
                _ => continue,
            };
            let post = match r.opt_text(1)? {
                Some("given") => true,
                Some("not_given") => false,
                _ => continue,
            };
            let rank = nils_registry::review::rank(kind);
            let id = r.int(3)?;
            if let Some(raw) = raws.get_mut(&r.int(0)?) {
                let better = raw
                    .answer
                    .is_none_or(|(rk, i, _)| rank > rk || (rank == rk && id > i));
                if better {
                    raw.answer = Some((rank, id, Answer { post, by }));
                }
            }
        }
    }
    let cfg = Config::default();
    for raw in raws.into_values() {
        let Some((_, study_day, study)) = studies.get_mut(&raw.study) else {
            continue;
        };
        study.scans.push(scan_of(&cfg, vocab, raw, *study_day));
    }
    Ok(studies
        .into_values()
        .map(|(subject, _, s)| (subject, s))
        .collect())
}

fn scan_of(cfg: &Config, vocab: &Vocabulary, raw: Raw, study_day: Option<i64>) -> Scan {
    let one = |axis: &str| {
        raw.axes
            .get(axis)
            .and_then(|v| v.first())
            .map(String::as_str)
    };
    let none = Vec::new();
    let modifiers = raw.axes.get("modifier").unwrap_or(&none);
    let technique = one("technique");
    let described = Described {
        base: one("base"),
        modifiers,
        technique,
        gradient_echo: technique.is_none_or(|t| t == "Unknown" || vocab.gradient_echo.contains(t)),
        tr: raw.tr,
        te: raw.te,
        ti: raw.ti,
        field_tesla: raw.field,
        acquisition: raw.acquisition.as_deref(),
    };
    let kind = contrast::kind_of(cfg, &described);
    let derived_type = raw.image_type.as_deref().is_some_and(|t| {
        let t = t.to_ascii_uppercase();
        [
            "DERIVED",
            "SECONDARY",
            "MPR",
            "REFORMAT",
            "PROJECTION",
            "MIP",
        ]
        .iter()
        .any(|w| t.contains(w))
    });
    let derived_description = raw.images.iter().any(|x| x.6 > 0);
    let disposition = one("disposition");
    let directory = one("directory_type");
    let outside = if raw.sealed {
        Outside::Sealed
    } else if directory == Some("localizer") || disposition == Some("scout") {
        Outside::Localizer
    } else if directory == Some("excluded") || disposition == Some("excluded") {
        Outside::Excluded
    } else if derived_type || derived_description || disposition == Some("reformat") {
        Outside::Derived
    } else {
        Outside::No
    };
    // the clock: seconds on one line of days, an image's own date first,
    // else the study's
    let at = |day: Option<i64>, secs: Option<f64>| -> Option<f64> {
        let day = day.or(study_day)?;
        Some(day as f64 * 86_400.0 + secs?)
    };
    let mut first: Option<f64> = None;
    let mut last: Option<f64> = None;
    let (mut images, mut timed, mut early) = (0, 0, 0);
    let mut dates = BTreeSet::new();
    for (day, lo, hi, n, t, e, _) in &raw.images {
        if let Some(day) = day {
            dates.insert(*day);
        }
        if let Some(lo) = at(*day, *lo) {
            first = Some(first.map_or(lo, |f| f.min(lo)));
        }
        if let Some(hi) = at(*day, *hi) {
            last = Some(last.map_or(hi, |l| l.max(hi)));
        }
        images += n;
        timed += t;
        early += e;
    }
    let time = at(
        raw.earliest_date.as_deref().and_then(day_of),
        raw.earliest_time.as_deref().and_then(seconds_of),
    )
    .or(first);
    Scan {
        stack: raw.stack,
        series: raw.series,
        series_number: raw.series_number,
        kind,
        outside,
        region: Region::of_body_part(one("body_part")),
        time,
        first,
        last,
        images,
        timed,
        content_early: early,
        dates: dates.into_iter().collect(),
        tagged: raw.agent,
        answer: raw.answer.map(|(_, _, a)| a).filter(|_| kind.t1_family),
    }
}

/// Replace the rows of the subjects named with the labels given.
fn write(store: &mut Store, subjects: &[i64], rows: &[(i64, i64, Label)]) -> Result<(), Error> {
    let t = table("contrast_label");
    let columns = [
        "stack_id",
        "study_id",
        "subject_id",
        "contrast_state",
        "minutes_since_injection",
        "injection_ref",
        "minutes_basis",
        "contrast_source",
        "basis",
        "contrast_prior_72h",
        "asl_after_contrast",
        "reason",
        "checks",
        "weak_f",
        "family",
        "t1_family",
        "use_level",
        "anchored",
        "first_post_stack",
        "last_pre_stack",
        "rule",
        "labelled_at",
    ];
    let insert = Insert::new(t, &columns);
    let now = now_iso();
    let text = |s: Option<&str>| s.map_or(Param::Null, |s| Param::Text(s.to_string()));
    let params: Vec<Vec<Param>> = rows
        .iter()
        .map(|(subject, study, l)| {
            vec![
                Param::Int(l.stack),
                Param::Int(*study),
                Param::Int(*subject),
                Param::Text(l.state.name().into()),
                l.minutes_since_injection.map_or(Param::Null, Param::Double),
                text(l.injection_ref.map(|o| o.name())),
                text(l.minutes_basis.map(|b| b.name())),
                Param::Text(l.source.name().into()),
                Param::Text(l.basis.name().into()),
                Param::Text(l.prior_72h.name().into()),
                text(l.asl_after_contrast.map(|p| p.name())),
                text(l.reason.as_deref()),
                Param::Text(json!(l.flags.iter().collect::<Vec<_>>()).to_string()),
                Param::Int(i64::from(l.weak_f)),
                Param::Text(l.kind.family.name().into()),
                Param::Int(i64::from(l.kind.t1_family)),
                Param::Text(contrast::use_level(&l.kind).into()),
                Param::Int(i64::from(l.anchored)),
                l.first_post.map_or(Param::Null, Param::Int),
                l.last_pre.map_or(Param::Null, Param::Int),
                Param::Text(contrast::RULE.into()),
                Param::Text(now.clone()),
            ]
        })
        .collect();
    store.begin()?;
    let result = (|| {
        store.batch(&format!(
            "DELETE FROM {} WHERE subject_id IN ({})",
            store.qualified("contrast_label"),
            ids(subjects)
        ))?;
        for chunk in params.chunks(500) {
            store.insert(&insert, chunk)?;
        }
        Ok::<(), nils_registry::store::Error>(())
    })();
    match result {
        Ok(()) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times_read_as_the_registry_writes_them() {
        assert_eq!(day_of("1970-01-02"), Some(1));
        assert_eq!(day_of("2026-10-13"), Some(days_from_civil(2026, 10, 13)));
        assert_eq!(day_of("bad"), None);
        assert_eq!(seconds_of("09:15:30"), Some(33_330.0));
        assert_eq!(seconds_of("09:15:30.500000"), Some(33_330.5));
        assert_eq!(seconds_of("0915"), None);
    }
}
