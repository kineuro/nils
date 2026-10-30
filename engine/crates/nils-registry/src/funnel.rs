// SPDX-License-Identifier: AGPL-3.0-only

//! The yield funnel and the error-budget line (record 58 P4, Wave 7b spec
//! section 15): for a selection and a pipeline, the subjects, sessions and
//! stacks left after each query filter of the selection and after each hard
//! need of the pipeline's pack, by acquisition year, sequence (base) and 2D
//! or 3D, and a last line that multiplies the certified bound of each axis
//! the selection filtered on by the stacks that filter read.
//!
//! This module counts. The engine works out which stacks are left after
//! each row (it freezes the selection's ask with its filters added one at
//! a time, and judges the units a run would have with the pack) and hands
//! the rows here as lists of stack ids; what comes back is counts only,
//! never a subject code, a path or a name.
//!
//! What a row counts, for its stacks:
//!
//! - **subjects**: the distinct subjects of its stacks (the series' subject);
//! - **sessions**: the distinct sessions, a session as the session cache
//!   holds it under the default scheme (`session_cache_study`), and a study
//!   the cache does not hold yet as its own session;
//! - **stacks**: the stacks.
//!
//! Each row is broken down three ways, each a list of `{value, subjects,
//! sessions, stacks}`: by the year of the study's date (`study_date`, else
//! the day the date vote filled), by the stack's `base` label, and by its
//! `MRAcquisitionType` (`2D`, `3D`). A stack that has none of one is counted
//! under `unknown`, so each breakdown's stacks add up to the row's.
//!
//! The error-budget line reads the newest recorded certificate, or the one
//! asked for: an axis's bound is the certificate's `result.axes.<axis>.bound`
//! where it measured that axis, else its stack bound (`result.bound`, or a
//! sealed sample's `result.risk`). Each filter row that reads an axis adds
//! the bound times the stacks that entered the filter (every stack whose
//! label the filter read may be misplaced by it: kept wrongly or dropped
//! wrongly), and the line rounds the sum up: "at most N stacks misplaced by
//! label error".

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::session::Scheme;
use crate::store::{Error as StoreError, Param, Store};

/// A stack's place in the counts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackOf {
    pub subject: i64,
    /// The session's key: `c<id>` for a cached session, `s<study>` for a
    /// study the cache does not hold yet.
    pub session: String,
    pub year: Option<i32>,
    pub base: Option<String>,
    /// `2D` or `3D` where the header says.
    pub dimension: Option<String>,
}

/// The value a stack is counted under where it has none.
pub const UNKNOWN: &str = "unknown";

/// What each of many stacks is counted by, read a few queries at a time.
/// A stack the registry does not hold is left out of the map.
pub fn stacks_of(store: &mut Store, stacks: &[i64]) -> Result<BTreeMap<i64, StackOf>, StoreError> {
    let mut out: BTreeMap<i64, StackOf> = BTreeMap::new();
    let d = store.dialect();
    let study = table("study");
    let date = d.text_of_qualified(Some("sd"), study.column("study_date").expect("study_date"));
    let filled = d.text_of_qualified(
        Some("sd"),
        study.column("date_filled").expect("date_filled"),
    );
    let window = Scheme::default().window_days;
    for chunk in stacks.chunks(500) {
        let ids = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, se.subject_id, se.study_id, {date}, {filled} FROM {stack} st \
             JOIN {series} se ON se.id = st.series_id JOIN {study} sd ON sd.id = se.study_id \
             WHERE st.id IN ({ids})",
            stack = store.qualified("stack"),
            series = store.qualified("series"),
            study = store.qualified("study"),
        );
        let mut studies: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for r in store.query(&sql, &[])? {
            let id = r.int(0)?;
            let study_id = r.int(2)?;
            let day = r.opt_text(3)?.or(r.opt_text(4)?).map(str::to_string);
            let year = day
                .as_deref()
                .and_then(|t| t.get(..4))
                .and_then(|y| y.parse::<i32>().ok());
            studies.entry(study_id).or_default().push(id);
            out.insert(
                id,
                StackOf {
                    subject: r.int(1)?,
                    session: format!("s{study_id}"),
                    year,
                    ..StackOf::default()
                },
            );
        }
        if !studies.is_empty() {
            let list = studies
                .keys()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT study_id, session_id FROM {} WHERE window_days = {} AND study_id IN ({list})",
                store.qualified("session_cache_study"),
                d.param(1, Type::Int)
            );
            for r in store.query(&sql, &[Param::Int(window)])? {
                let (study_id, session) = (r.int(0)?, r.int(1)?);
                for s in studies.get(&study_id).into_iter().flatten() {
                    if let Some(o) = out.get_mut(s) {
                        o.session = format!("c{session}");
                    }
                }
            }
        }
        let sql = format!(
            "SELECT stack_id, value FROM {} WHERE stack_id IN ({ids}) AND axis = 'base' \
             AND value IS NOT NULL ORDER BY stack_id, value",
            store.qualified("classification_axis")
        );
        for r in store.query(&sql, &[])? {
            if let Some(o) = out.get_mut(&r.int(0)?)
                && o.base.is_none()
            {
                o.base = Some(r.text(1)?.to_string());
            }
        }
        let sql = format!(
            "SELECT stack_id, mr_acquisition_type FROM {} WHERE stack_id IN ({ids})",
            store.qualified("stack_fingerprint")
        );
        for r in store.query(&sql, &[])? {
            if let Some(o) = out.get_mut(&r.int(0)?) {
                o.dimension = r
                    .opt_text(1)?
                    .map(str::trim)
                    .filter(|t| matches!(*t, "2D" | "3D"))
                    .map(str::to_string);
            }
        }
    }
    Ok(out)
}

/// Subjects, sessions and stacks of some stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub subjects: usize,
    pub sessions: usize,
    pub stacks: usize,
}

impl Counts {
    pub fn of<'a>(stacks: impl Iterator<Item = &'a StackOf>) -> Counts {
        let mut subjects = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        let mut n = 0;
        for s in stacks {
            subjects.insert(s.subject);
            sessions.insert((s.subject, s.session.clone()));
            n += 1;
        }
        Counts {
            subjects: subjects.len(),
            sessions: sessions.len(),
            stacks: n,
        }
    }

    pub fn doc(&self) -> Value {
        json!({"subjects": self.subjects, "sessions": self.sessions, "stacks": self.stacks})
    }
}

/// A row's counts and its three breakdowns.
pub fn row_counts(of: &BTreeMap<i64, StackOf>, stacks: &BTreeSet<i64>) -> Value {
    let held: Vec<&StackOf> = stacks.iter().filter_map(|s| of.get(s)).collect();
    let by = |key: &dyn Fn(&StackOf) -> String| -> Value {
        let mut groups: BTreeMap<String, Vec<&StackOf>> = BTreeMap::new();
        for s in &held {
            groups.entry(key(s)).or_default().push(s);
        }
        Value::Array(
            groups
                .into_iter()
                .map(|(value, stacks)| {
                    let mut c = Counts::of(stacks.into_iter()).doc();
                    c["value"] = json!(value);
                    c
                })
                .collect(),
        )
    };
    let mut doc = Counts::of(held.iter().copied()).doc();
    doc["by_year"] = by(&|s| s.year.map_or_else(|| UNKNOWN.to_string(), |y| y.to_string()));
    doc["by_base"] = by(&|s| s.base.clone().unwrap_or_else(|| UNKNOWN.to_string()));
    doc["by_dimension"] = by(&|s| s.dimension.clone().unwrap_or_else(|| UNKNOWN.to_string()));
    doc
}

/// The bounds the error-budget line multiplies by: one certificate's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bounds {
    pub certificate: Option<i64>,
    /// The stack bound: the share of stacks the certificate bounds wrong.
    pub stack: Option<f64>,
    /// The bound of each axis the certificate measured on its own.
    pub axes: BTreeMap<String, f64>,
    pub words: Option<String>,
}

impl Bounds {
    /// The bounds of a certificate's result.
    pub fn of(c: &crate::labels::Certificate) -> Bounds {
        let r = &c.result;
        let share = |v: &Value| v.as_f64().filter(|b| (0.0..=1.0).contains(b));
        Bounds {
            certificate: Some(c.id),
            stack: share(&r["bound"]).or_else(|| share(&r["risk"])),
            axes: r["axes"]
                .as_object()
                .map(|a| {
                    a.iter()
                        .filter_map(|(k, v)| share(&v["bound"]).map(|b| (k.clone(), b)))
                        .collect()
                })
                .unwrap_or_default(),
            words: Some(c.words()),
        }
    }

    /// The certificate asked for, else the newest recorded; none where the
    /// registry records no certificate.
    pub fn read(store: &mut Store, certificate: Option<i64>) -> Result<Bounds, crate::labels::Error> {
        let c = match certificate {
            Some(id) => Some(crate::labels::certificate(store, id)?.ok_or_else(|| {
                crate::labels::Error::NotFound(format!("no certificate {id}"))
            })?),
            None => crate::labels::certificates(store)?.into_iter().next(),
        };
        Ok(c.as_ref().map(Bounds::of).unwrap_or_default())
    }
}

/// One filter row the line multiplies: the axes its filter read and the
/// stacks that entered it.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub row: usize,
    pub axes: Vec<String>,
    pub entered: usize,
}

/// The error-budget line: for each filter that read an axis, the axis's
/// bound (the stack bound where the certificate has none of the axis) times
/// the stacks that entered the filter, summed and rounded up. A filter that
/// reads several axes adds a term for each. Without a certificate, or one
/// with no bound, the line says so and carries no number.
pub fn budget(bounds: &Bounds, reads: &[Read]) -> Value {
    let mut terms = Vec::new();
    let mut total = 0.0f64;
    let mut unbounded = Vec::new();
    for r in reads {
        for axis in &r.axes {
            let (bound, from) = match bounds.axes.get(axis) {
                Some(b) => (Some(*b), "axis"),
                None => (bounds.stack, "stack"),
            };
            let product = bound.map(|b| b * r.entered as f64);
            if let Some(p) = product {
                total += p;
            } else {
                unbounded.push(axis.clone());
            }
            terms.push(json!({
                "row": r.row, "axis": axis, "bound": bound, "bound_from": from,
                "stacks": r.entered, "product": product,
            }));
        }
    }
    let at_most = (bounds.stack.is_some() || !bounds.axes.is_empty()) && unbounded.is_empty();
    let n = at_most.then(|| (total - 1e-9).max(0.0).ceil() as u64);
    let words = match (n, bounds.certificate, terms.is_empty()) {
        (_, _, true) => "the selection filters on no axis, so label error moves no stack".to_string(),
        (Some(n), _, false) => format!(
            "at most {n} {} misplaced by label error",
            if n == 1 { "stack" } else { "stacks" }
        ),
        (None, None, false) => {
            "no certificate is recorded, so no bound: nils labels certify records one".to_string()
        }
        (None, Some(id), false) => format!(
            "certificate {id} states no bound for {}, so no number",
            unbounded.join(", ")
        ),
    };
    json!({
        "certificate": bounds.certificate,
        "certificate_words": bounds.words,
        "stack_bound": bounds.stack,
        "axis_bounds": bounds.axes,
        "terms": terms,
        "sum": (!terms.is_empty() && n.is_some()).then_some(total),
        "at_most": if terms.is_empty() { Some(0) } else { n },
        "words": words,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(subject: i64, session: &str) -> StackOf {
        StackOf {
            subject,
            session: session.into(),
            ..StackOf::default()
        }
    }

    #[test]
    fn counts_take_distinct_subjects_and_sessions() {
        let c = Counts::of([s(1, "c1"), s(1, "c1"), s(1, "c2"), s(2, "s9")].iter());
        assert_eq!(
            c,
            Counts {
                subjects: 2,
                sessions: 3,
                stacks: 4
            }
        );
    }

    #[test]
    fn the_line_multiplies_each_axis_bound_by_the_stacks_it_read() {
        let b = Bounds {
            certificate: Some(2),
            stack: Some(0.0024),
            axes: [("base".to_string(), 0.01)].into_iter().collect(),
            words: None,
        };
        let reads = [
            Read {
                row: 1,
                axes: vec!["base".into()],
                entered: 1000,
            },
            Read {
                row: 2,
                axes: vec!["post_contrast".into()],
                entered: 500,
            },
        ];
        let v = budget(&b, &reads);
        // 0.01 * 1000 + 0.0024 * 500 = 11.2, rounded up
        assert_eq!(v["at_most"], 12, "{v}");
        assert_eq!(v["terms"][1]["bound_from"], "stack", "{v}");
        assert_eq!(v["words"], "at most 12 stacks misplaced by label error");
        let none = budget(&Bounds::default(), &reads);
        assert!(none["at_most"].is_null(), "{none}");
        assert_eq!(budget(&b, &[])["at_most"], 0);
    }
}
