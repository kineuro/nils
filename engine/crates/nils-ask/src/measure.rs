// SPDX-License-Identifier: AGPL-3.0-only

//! The post pass (§4.5, §11.1): `out.measures` computed in Rust over the
//! answer's rows, never in SQL. `share {of, over}` adds a column, the value
//! divided by the distinct subjects of the named set; `stddev`, `median`
//! and `percentile {of, p}` are scalars over the answer. A truncated answer
//! has no measures: a measure over the row cap is a job's.

use std::collections::BTreeMap;
use std::fmt;

use nils_registry::store::Cell;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::Measure;
use crate::exec::Answer;

#[derive(Debug)]
pub enum MeasureError {
    Truncated,
    NoColumn(String),
    Unknown(String),
    NoDenominator(String),
    Message(String),
}

impl fmt::Display for MeasureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeasureError::Truncated => {
                f.write_str("a truncated answer has no measures; run the ask as a job")
            }
            MeasureError::NoColumn(c) => write!(f, "no column named {c} to measure"),
            MeasureError::Unknown(m) => write!(
                f,
                "{m} is not a measure; those are share, stddev, median, percentile"
            ),
            MeasureError::NoDenominator(s) => write!(f, "no count for the set {s} a share is over"),
            MeasureError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for MeasureError {}

/// What the post pass added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Measured {
    /// Columns appended to every row, in order.
    pub columns: Vec<String>,
    /// Scalars over the answer, by `<measure>.<column>[.<p>]`.
    pub scalars: BTreeMap<String, f64>,
}

fn numeric(c: &Cell) -> Option<f64> {
    match c {
        Cell::Int(i) => Some(*i as f64),
        Cell::Double(d) => Some(*d),
        _ => None,
    }
}

fn column_index(answer: &Answer, of: &str) -> Result<usize, MeasureError> {
    answer
        .columns
        .iter()
        .position(|c| c == of)
        .ok_or_else(|| MeasureError::NoColumn(of.to_string()))
}

fn values_of(answer: &Answer, i: usize) -> Vec<f64> {
    let mut v: Vec<f64> = answer
        .rows
        .iter()
        .filter_map(|r| r.0.get(i).and_then(numeric))
        .collect();
    v.sort_by(f64::total_cmp);
    v
}

fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

/// The nearest rank percentile, p in 0..=100.
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

fn stddev(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    Some((v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt())
}

/// Round to nine decimals, the renderer's rule, so the two backends agree.
pub fn nine(x: f64) -> f64 {
    (x * 1e9).round() / 1e9
}

/// Whether a measure is a scalar kind (`share` is the column kind).
pub fn is_scalar(kind: &str) -> bool {
    matches!(kind, "stddev" | "median" | "percentile")
}

/// The name a measure's result carries.
pub fn name_of(kind: &str, of: &str, p: Option<f64>) -> String {
    match (kind, p) {
        ("percentile", Some(p)) => format!("percentile.{of}.{p}"),
        _ => format!("{kind}.{of}"),
    }
}

/// One scalar over a list of values.
pub fn scalar(kind: &str, values: &mut [f64], p: Option<f64>) -> Result<Option<f64>, MeasureError> {
    values.sort_by(f64::total_cmp);
    let v = match kind {
        "stddev" => stddev(values),
        "median" => median(values),
        "percentile" => {
            let p = p.ok_or_else(|| MeasureError::Message("percentile names {of, p}".into()))?;
            if !(0.0..=100.0).contains(&p) {
                return Err(MeasureError::Message("p is between 0 and 100".into()));
            }
            percentile(values, p)
        }
        other => return Err(MeasureError::Unknown(other.to_string())),
    };
    Ok(v.map(nine))
}

/// A cell as a number, when it is one.
pub fn number(c: &Cell) -> Option<f64> {
    numeric(c)
}

/// Apply the measures to an answer; `over` holds the distinct subject
/// count of every set a share is over.
pub fn apply(
    answer: &mut Answer,
    measures: &[Measure],
    over: &BTreeMap<String, f64>,
) -> Result<Measured, MeasureError> {
    if measures.is_empty() {
        return Ok(Measured::default());
    }
    if answer.truncated {
        return Err(MeasureError::Truncated);
    }
    let mut out = Measured::default();
    for m in measures {
        for (kind, spec) in &m.0 {
            let of = spec
                .get("of")
                .and_then(Value::as_str)
                .ok_or_else(|| MeasureError::Message(format!("{kind} names {{of}}")))?;
            let i = column_index(answer, of)?;
            match kind.as_str() {
                "share" => {
                    let set = spec
                        .get("over")
                        .and_then(Value::as_str)
                        .ok_or_else(|| MeasureError::Message("share names {of, over}".into()))?;
                    let denominator = *over
                        .get(set)
                        .ok_or_else(|| MeasureError::NoDenominator(set.to_string()))?;
                    let name = format!("share.{of}");
                    for r in answer.rows.iter_mut() {
                        let v = r.0.get(i).and_then(numeric);
                        r.0.push(match v {
                            Some(x) if denominator > 0.0 => Cell::Double(nine(x / denominator)),
                            _ => Cell::Null,
                        });
                    }
                    answer.columns.push(name.clone());
                    out.columns.push(name);
                }
                "stddev" => {
                    let v = values_of(answer, i);
                    if let Some(s) = stddev(&v) {
                        out.scalars.insert(format!("stddev.{of}"), nine(s));
                    }
                }
                "median" => {
                    let v = values_of(answer, i);
                    if let Some(s) = median(&v) {
                        out.scalars.insert(format!("median.{of}"), nine(s));
                    }
                }
                "percentile" => {
                    let p = spec
                        .get("p")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| MeasureError::Message("percentile names {of, p}".into()))?;
                    if !(0.0..=100.0).contains(&p) {
                        return Err(MeasureError::Message("p is between 0 and 100".into()));
                    }
                    let v = values_of(answer, i);
                    if let Some(s) = percentile(&v, p) {
                        out.scalars.insert(format!("percentile.{of}.{p}"), nine(s));
                    }
                }
                other => return Err(MeasureError::Unknown(other.to_string())),
            }
        }
    }
    Ok(out)
}
