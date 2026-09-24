// SPDX-License-Identifier: AGPL-3.0-only

//! Measures (record 49 A3): the numbers a pipeline run's tables hold, one
//! row per unit and measure, and the declared metrics its results carried,
//! loaded so the ask reads them as fields of the unit's grain,
//! `measure.<pipeline>.<name>`. A unit's value is its newest run's of the
//! pipeline's current version; the rows of older runs stay, and every row
//! names the run and the table file it came from, so a number is traced to
//! its run.
//!
//! This module is the rows. Reading a table file is `nils-pipeline`'s and
//! loading a run's tables the binary's.

use serde::Serialize;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};

/// The scopes a measure belongs to: the unit's grain.
pub const SCOPES: [&str; 3] = ["subject", "session", "stack"];

/// Where a measure came from when no table holds it: a metric the run's
/// results declared for one of the descriptor's checks.
pub const METRICS: &str = "metrics";

/// One measure to write.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub run_id: i64,
    pub pipeline_id: i64,
    pub pipeline: &'a str,
    pub derivative_id: Option<i64>,
    pub source: &'a str,
    pub scope: &'a str,
    pub subject_id: Option<i64>,
    pub session_day: Option<&'a str>,
    pub stack_id: Option<i64>,
    pub unit_id: &'a str,
    pub name: &'a str,
    /// `number`, `integer` or `text`.
    pub ty: &'a str,
    pub number: Option<f64>,
    pub text: Option<&'a str>,
    pub unit: Option<&'a str>,
    pub created_at: &'a str,
}

/// One measure as the registry holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measure {
    pub id: i64,
    pub run_id: i64,
    pub pipeline: String,
    pub derivative_id: Option<i64>,
    pub source: String,
    pub scope: String,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
    pub stack_id: Option<i64>,
    pub unit_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub number: Option<f64>,
    pub text: Option<String>,
    pub unit: Option<String>,
}

const COLUMNS: [&str; 17] = [
    "run_id",
    "pipeline_id",
    "pipeline",
    "derivative_id",
    "source",
    "scope",
    "subject_id",
    "session_day",
    "stack_id",
    "unit_id",
    "name",
    "type",
    "number",
    "text",
    "unit",
    "created_at",
    "id",
];

/// Write measures, a few hundred to a statement. Answers how many.
pub fn insert(store: &mut Store, rows: &[New<'_>]) -> Result<usize, Error> {
    for r in rows {
        if !SCOPES.contains(&r.scope) {
            return Err(Error::Message(format!(
                "a measure belongs to a subject, a session or a stack, not {}",
                r.scope
            )));
        }
    }
    let opt_int = |v: Option<i64>| v.map_or(Param::Null, Param::Int);
    let opt_text = |v: Option<&str>| v.map_or(Param::Null, Param::from);
    for chunk in rows.chunks(200) {
        let params: Vec<Vec<Param>> = chunk
            .iter()
            .map(|r| {
                vec![
                    Param::Int(r.run_id),
                    Param::Int(r.pipeline_id),
                    Param::from(r.pipeline),
                    opt_int(r.derivative_id),
                    Param::from(r.source),
                    Param::from(r.scope),
                    opt_int(r.subject_id),
                    opt_text(r.session_day),
                    opt_int(r.stack_id),
                    Param::from(r.unit_id),
                    Param::from(r.name),
                    Param::from(r.ty),
                    r.number.map_or(Param::Null, Param::Double),
                    opt_text(r.text),
                    opt_text(r.unit),
                    Param::from(r.created_at),
                ]
            })
            .collect();
        store.insert(&Insert::new(table("measure"), &COLUMNS[..16]), &params)?;
    }
    Ok(rows.len())
}

fn select(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("measure");
    let cols: Vec<String> = [
        "id",
        "run_id",
        "pipeline",
        "derivative_id",
        "source",
        "scope",
        "subject_id",
        "session_day",
        "stack_id",
        "unit_id",
        "name",
        "type",
        "number",
        "text",
        "unit",
    ]
    .iter()
    .map(|c| d.text_of(t.column(c).expect("a measure column")))
    .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("measure")
    )
}

fn of(r: &Row) -> Result<Measure, Error> {
    Ok(Measure {
        id: r.int(0)?,
        run_id: r.int(1)?,
        pipeline: r.text(2)?.to_string(),
        derivative_id: r.opt_int(3)?,
        source: r.text(4)?.to_string(),
        scope: r.text(5)?.to_string(),
        subject_id: r.opt_int(6)?,
        session_day: r.opt_text(7)?.map(str::to_string),
        stack_id: r.opt_int(8)?,
        unit_id: r.text(9)?.to_string(),
        name: r.text(10)?.to_string(),
        ty: r.text(11)?.to_string(),
        number: r.opt_double(12)?,
        text: r.opt_text(13)?.map(str::to_string),
        unit: r.opt_text(14)?.map(str::to_string),
    })
}

/// A run's measures, by unit and name.
pub fn of_run(store: &mut Store, run_id: i64) -> Result<Vec<Measure>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE run_id = {} ORDER BY unit_id, name, id",
        select(store),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(run_id)])?
        .iter()
        .map(of)
        .collect()
}

/// One family of measures the ask reads as a field: a pipeline's measure
/// at one grain, its type and unit, and how many rows hold it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Family {
    pub pipeline: String,
    pub name: String,
    pub scope: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub unit: Option<String>,
    pub rows: i64,
}

/// Every family the registry holds, by pipeline, name and grain.
pub fn families(store: &mut Store) -> Result<Vec<Family>, Error> {
    let sql = format!(
        "SELECT pipeline, name, scope, MAX(type), MAX(unit), COUNT(*) FROM {} \
         GROUP BY pipeline, name, scope ORDER BY pipeline, name, scope",
        store.qualified("measure")
    );
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Family {
                pipeline: r.text(0)?.to_string(),
                name: r.text(1)?.to_string(),
                scope: r.text(2)?.to_string(),
                ty: r.text(3)?.to_string(),
                unit: r.opt_text(4)?.map(str::to_string),
                rows: r.int(5)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};

    #[test]
    fn a_run_s_measures_are_written_read_and_grouped_into_families() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        let row = |unit_id: &'static str, name: &'static str, number: Option<f64>| New {
            run_id: 7,
            pipeline_id: 2,
            pipeline: "synthseg",
            derivative_id: Some(11),
            source: "volumes",
            scope: "session",
            subject_id: Some(3),
            session_day: Some("2022-01-15"),
            stack_id: None,
            unit_id,
            name,
            ty: "number",
            number,
            text: None,
            unit: Some("mm3"),
            created_at: "2026-09-24T10:00:00Z",
        };
        let n = insert(
            &mut store,
            &[
                row("sub-P1_ses-1", "left_hippocampus", Some(4012.5)),
                row("sub-P2_ses-1", "left_hippocampus", Some(3900.0)),
                row("sub-P1_ses-1", "total_intracranial", Some(1.5e6)),
            ],
        )
        .unwrap();
        assert_eq!(n, 3);
        let all = of_run(&mut store, 7).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].unit_id, "sub-P1_ses-1");
        assert_eq!(all[0].number, Some(4012.5));
        assert_eq!(all[0].session_day.as_deref(), Some("2022-01-15"));
        let f = families(&mut store).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name, "left_hippocampus");
        assert_eq!(f[0].rows, 2);
        assert_eq!(f[0].unit.as_deref(), Some("mm3"));
        let mut bad = row("x", "y", None);
        bad.scope = "run";
        assert!(insert(&mut store, &[bad]).is_err());
    }
}
