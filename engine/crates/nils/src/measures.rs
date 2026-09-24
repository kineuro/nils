// SPDX-License-Identifier: AGPL-3.0-only

//! A run's numbers (record 49 A3): the rows of the tables it wrote, loaded
//! as measures the ask reads, and the declared checks held against each
//! unit's metrics and measures, a breach being a `pipeline:qc` item that
//! names the metric and its value.
//!
//! The runner (`pipelines.rs`) registers a table's file as a derivative of
//! kind `table` like any other output; this module reads what the file
//! holds, ties each row to its unit, and says which checks a unit breaks.

use std::collections::BTreeMap;

use nils_pipeline::descriptor::{Check, Output};
use nils_registry::derivative::Belongs;
use nils_registry::store::Store;
use serde_json::{Value, json};

/// One measure waiting to be written, owned.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub unit_id: String,
    pub belongs: Belongs,
    pub derivative_id: Option<i64>,
    pub source: String,
    pub name: String,
    pub ty: String,
    pub number: Option<f64>,
    pub text: Option<String>,
    pub unit: Option<String>,
}

/// What reading a run's tables and checks came to, for its summary.
#[derive(Debug, Default)]
pub(crate) struct Notes {
    /// Table files read, and their rows.
    pub files: usize,
    pub rows: usize,
    /// Declared columns a file did not carry, as `output: column`.
    pub missing: Vec<String>,
    /// Values not of their column's type, as `output: column: value`.
    pub refused_values: Vec<String>,
    /// Rows of a run's table that named no unit of the run.
    pub strays: usize,
    /// Units a check could not be held against: the metric was not there.
    pub unchecked: usize,
    pub breaches: usize,
}

impl Notes {
    pub(crate) fn summary(&self, measures: usize, declared: usize) -> Value {
        json!({
            "tables": {
                "files": self.files, "rows": self.rows, "missing_columns": self.missing,
                "refused_values": self.refused_values, "rows_naming_no_unit": self.strays,
            },
            "measures": measures,
            "checks": {"declared": declared, "breaches": self.breaches, "unchecked": self.unchecked},
        })
    }
}

/// The scope a unit's measure belongs to, from what its files belong to.
fn scope_of(b: &Belongs) -> Option<&'static str> {
    match b.scope.as_str() {
        "stack" => Some("stack"),
        "session" => Some("session"),
        "subject" => Some("subject"),
        _ => None,
    }
}

/// A unit's own table: its rows as that unit's measures. A unit's table
/// holds one row, since a unit is one row of the ask; more is refused.
pub(crate) fn unit_table(
    o: &Output,
    bytes: &[u8],
    unit_id: &str,
    belongs: &Belongs,
    derivative_id: i64,
    notes: &mut Notes,
    into: &mut Vec<Pending>,
) -> Result<(), String> {
    let spec = o.table.as_ref().ok_or("not a table output")?;
    let read = nils_pipeline::table::read(spec, bytes)?;
    notes.files += 1;
    note(o, &read, notes);
    if read.rows.len() > 1 {
        return Err(format!(
            "a unit's table holds one row, and this one holds {}",
            read.rows.len()
        ));
    }
    for row in &read.rows {
        notes.rows += 1;
        push(o, row, unit_id, belongs, derivative_id, into);
    }
    Ok(())
}

/// A run's own table: each row's unit read from its unit column, and the
/// row taken as that unit's measures. A row that names no unit of the run
/// is counted and left.
pub(crate) fn run_table(
    o: &Output,
    bytes: &[u8],
    units: &[(String, Option<Belongs>)],
    derivative_id: i64,
    notes: &mut Notes,
    into: &mut Vec<Pending>,
) -> Result<(), String> {
    let spec = o.table.as_ref().ok_or("not a table output")?;
    let read = nils_pipeline::table::read(spec, bytes)?;
    notes.files += 1;
    note(o, &read, notes);
    for row in &read.rows {
        let named = row.unit.as_deref().unwrap_or_default();
        match unit_named(named, units) {
            Some((id, Some(b))) => {
                notes.rows += 1;
                push(o, row, id, b, derivative_id, into);
            }
            _ => notes.strays += 1,
        }
    }
    Ok(())
}

fn note(o: &Output, read: &nils_pipeline::table::Read, notes: &mut Notes) {
    notes
        .missing
        .extend(read.missing.iter().map(|c| format!("{}: {c}", o.id)));
    notes
        .refused_values
        .extend(read.refused.iter().map(|v| format!("{}: {v}", o.id)));
}

fn push(
    o: &Output,
    row: &nils_pipeline::table::Row,
    unit_id: &str,
    belongs: &Belongs,
    derivative_id: i64,
    into: &mut Vec<Pending>,
) {
    let Some(spec) = &o.table else { return };
    for (name, v) in &row.values {
        let Some(c) = spec.columns.iter().find(|c| &c.name == name) else {
            continue;
        };
        into.push(Pending {
            unit_id: unit_id.to_string(),
            belongs: belongs.clone(),
            derivative_id: Some(derivative_id),
            source: o.id.clone(),
            name: name.clone(),
            ty: c.ty.name().to_string(),
            number: v.as_f64(),
            text: v.as_str().map(str::to_string),
            unit: c.unit.clone(),
        });
    }
}

/// The unit a run's table row names: the unit itself (`sub-01_ses-a`, or
/// `sub-01/ses-a`), or a name that begins with the unit and an `_`
/// (`sub-01_ses-a_T1w`), the longest such unit where two would do.
pub(crate) fn unit_named<'a>(
    written: &str,
    units: &'a [(String, Option<Belongs>)],
) -> Option<(&'a str, &'a Option<Belongs>)> {
    let w = nils_pipeline::results::normalise(written);
    units
        .iter()
        .filter(|(id, _)| w == *id || w.starts_with(&format!("{id}_")))
        .max_by_key(|(id, _)| id.len())
        .map(|(id, b)| (id.as_str(), b))
}

/// The checks one unit breaks: each check's metric read from the unit's
/// results metrics, else from its measures. Answers the breaches, the
/// metrics taken from the results (which are kept as measures), and how
/// many checks found no value to hold.
pub(crate) fn hold(
    checks: &[Check],
    metrics: &Value,
    measured: &BTreeMap<String, f64>,
) -> (Vec<Value>, Vec<(String, f64)>, usize) {
    let mut breaches = Vec::new();
    let mut taken: Vec<(String, f64)> = Vec::new();
    let mut unchecked = 0;
    for c in checks {
        let from_results = metrics[&c.metric].as_f64().filter(|v| v.is_finite());
        let value = from_results.or_else(|| measured.get(&c.metric).copied());
        let Some(v) = value else {
            unchecked += 1;
            continue;
        };
        if let Some(r) = from_results
            && !measured.contains_key(&c.metric)
            && !taken.iter().any(|(m, _)| m == &c.metric)
        {
            taken.push((c.metric.clone(), r));
        }
        if !c.holds(v) {
            breaches.push(json!({
                "metric": c.metric, "value": v, "check": c.text(),
                "op": c.op, "threshold": c.value, "description": c.description,
            }));
        }
    }
    (breaches, taken, unchecked)
}

/// The sentence a breach item says: `snr_total is 5.2, and the check is
/// snr_total >= 8`, one clause a breach.
pub(crate) fn breach_words(breaches: &[Value]) -> String {
    breaches
        .iter()
        .map(|b| {
            format!(
                "{} is {}, and the check is {}",
                b["metric"].as_str().unwrap_or_default(),
                nils_pipeline::descriptor::number_text(b["value"].as_f64().unwrap_or(f64::NAN)),
                b["check"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Write what was read, naming the run and the pipeline. Answers how many.
pub(crate) fn write(
    store: &mut Store,
    run_id: i64,
    pipeline_id: i64,
    pipeline: &str,
    pending: &[Pending],
    now: &str,
) -> Result<usize, String> {
    let rows: Vec<nils_registry::measure::New<'_>> = pending
        .iter()
        .filter_map(|p| {
            Some(nils_registry::measure::New {
                run_id,
                pipeline_id,
                pipeline,
                derivative_id: p.derivative_id,
                source: &p.source,
                scope: scope_of(&p.belongs)?,
                subject_id: p.belongs.subject_id,
                session_day: p.belongs.session_day.as_deref(),
                stack_id: p.belongs.stack_id,
                unit_id: &p.unit_id,
                name: &p.name,
                ty: &p.ty,
                number: p.number,
                text: p.text.as_deref(),
                unit: p.unit.as_deref(),
                created_at: now,
            })
        })
        .collect();
    nils_registry::measure::insert(store, &rows).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Option<Belongs> {
        Some(Belongs::run())
    }

    #[test]
    fn a_run_s_row_finds_its_unit_by_name_or_by_prefix() {
        let units = vec![
            ("sub-01".to_string(), b()),
            ("sub-01_ses-a".to_string(), b()),
            ("sub-10_ses-a".to_string(), b()),
        ];
        assert_eq!(
            unit_named("sub-01_ses-a_T1w", &units).unwrap().0,
            "sub-01_ses-a"
        );
        assert_eq!(
            unit_named("sub-01/ses-a", &units).unwrap().0,
            "sub-01_ses-a"
        );
        assert_eq!(unit_named("sub-01_T1w", &units).unwrap().0, "sub-01");
        assert!(unit_named("sub-1_ses-a", &units).is_none());
        assert!(unit_named("", &units).is_none());
    }

    #[test]
    fn a_check_holds_against_the_results_first_and_the_tables_next() {
        let checks = vec![
            Check {
                metric: "snr".into(),
                op: ">=".into(),
                value: 8.0,
                description: None,
            },
            Check {
                metric: "holes".into(),
                op: "<=".into(),
                value: 200.0,
                description: None,
            },
            Check {
                metric: "absent".into(),
                op: ">".into(),
                value: 0.0,
                description: None,
            },
        ];
        let measured = BTreeMap::from([("holes".to_string(), 250.0), ("snr".to_string(), 99.0)]);
        let (breaches, taken, unchecked) = hold(&checks, &json!({"snr": 5.5}), &measured);
        assert_eq!(unchecked, 1);
        assert_eq!(breaches.len(), 2, "{breaches:?}");
        assert_eq!(
            breaches[0]["value"], 5.5,
            "the results' metric is the unit's own"
        );
        assert!(taken.is_empty(), "a measured metric is not kept twice");
        assert_eq!(
            breach_words(&breaches),
            "snr is 5.5, and the check is snr >= 8; holes is 250, and the check is holes <= 200"
        );
        let (none, taken, _) = hold(&checks[..1], &json!({"snr": 9}), &BTreeMap::new());
        assert!(none.is_empty());
        assert_eq!(taken, [("snr".to_string(), 9.0)]);
    }
}
