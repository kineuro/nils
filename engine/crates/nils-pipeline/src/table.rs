// SPDX-License-Identifier: AGPL-3.0-only

//! A table output (record 49 A3): a file of numbers a pipeline wrote, read
//! into rows of declared, typed columns, one row per unit, so the runner can
//! load them for the ask. CSV and TSV are read by their header row, JSON as
//! one object (a unit's table) or a list of objects. A declared column is
//! found by the header its `from` names, or by the header whose folded form
//! is its name (`left hippocampus` is `left_hippocampus`); a column a file
//! does not carry is said, not guessed.

use serde_json::Value;

use crate::descriptor::{ColumnType, Table, fold};

/// The most rows one table file may hold.
pub const MAX_ROWS: usize = 100_000;

/// One row of a table, as the engine read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// For a run's table, the unit the row names, as the file wrote it.
    pub unit: Option<String>,
    /// The declared columns the row has a value for, by name: a number for
    /// a numeric column, text for a text one.
    pub values: Vec<(String, Value)>,
}

/// What a table file held.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Read {
    pub rows: Vec<Row>,
    /// Declared columns the file does not carry.
    pub missing: Vec<String>,
    /// Values that are not of their column's type, as `column: value`.
    pub refused: Vec<String>,
}

/// Read a table file by its declared format and columns.
pub fn read(spec: &Table, bytes: &[u8]) -> Result<Read, String> {
    match spec.format.as_str() {
        "csv" => delimited(spec, bytes, b','),
        "tsv" => delimited(spec, bytes, b'\t'),
        "json" => json(spec, bytes),
        other => Err(format!("{other} is not a table format")),
    }
}

/// Which of `headers` a declared name (or its `from`) is.
fn find(headers: &[String], name: &str, from: Option<&str>) -> Option<usize> {
    if let Some(f) = from {
        if let Some(i) = headers.iter().position(|h| h.trim() == f.trim()) {
            return Some(i);
        }
        let folded = fold(f);
        return headers.iter().position(|h| fold(h) == folded);
    }
    headers.iter().position(|h| fold(h) == name)
}

/// A value as its column's type, or why not. Empty text, and the words a
/// table writes for no value, are no value.
fn typed(ty: ColumnType, raw: &Value) -> Result<Option<Value>, String> {
    let text = match raw {
        Value::Null => return Ok(None),
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => return Err(other.to_string()),
    };
    if text.is_empty()
        || matches!(
            text.to_ascii_lowercase().as_str(),
            "nan" | "na" | "n/a" | "none" | "null"
        )
    {
        return Ok(None);
    }
    match ty {
        ColumnType::Text => Ok(Some(Value::String(text))),
        ColumnType::Number | ColumnType::Integer => {
            let n: f64 = text.parse().map_err(|_| text.clone())?;
            if !n.is_finite() || (ty == ColumnType::Integer && n.fract() != 0.0) {
                return Err(text);
            }
            Ok(serde_json::Number::from_f64(n).map(Value::Number))
        }
    }
}

fn delimited(spec: &Table, bytes: &[u8], delimiter: u8) -> Result<Read, String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(bytes);
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| format!("the table's header: {e}"))?
        .iter()
        .map(str::to_string)
        .collect();
    let mut out = Read::default();
    let mut at: Vec<(usize, &crate::descriptor::Column)> = Vec::new();
    for c in &spec.columns {
        match find(&headers, &c.name, c.from.as_deref()) {
            Some(i) => at.push((i, c)),
            None => out.missing.push(c.name.clone()),
        }
    }
    let unit_at = match &spec.unit_column {
        Some(u) => Some(
            find(&headers, &fold(u), Some(u))
                .ok_or_else(|| format!("the table has no column {u} to name its units"))?,
        ),
        None => None,
    };
    for record in reader.records() {
        let record = record.map_err(|e| format!("the table: {e}"))?;
        if record.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        if out.rows.len() >= MAX_ROWS {
            return Err(format!("the table holds more than {MAX_ROWS} rows"));
        }
        let mut row = Row {
            unit: unit_at.and_then(|i| record.get(i)).map(str::to_string),
            values: Vec::new(),
        };
        for (i, c) in &at {
            let raw = Value::String(record.get(*i).unwrap_or("").to_string());
            match typed(c.ty, &raw) {
                Ok(Some(v)) => row.values.push((c.name.clone(), v)),
                Ok(None) => {}
                Err(bad) => out.refused.push(format!("{}: {bad}", c.name)),
            }
        }
        out.rows.push(row);
    }
    Ok(out)
}

fn json(spec: &Table, bytes: &[u8]) -> Result<Read, String> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("the table is not JSON: {e}"))?;
    let objects: Vec<&serde_json::Map<String, Value>> = match &v {
        Value::Object(o) => vec![o],
        Value::Array(a) => a
            .iter()
            .map(|r| r.as_object().ok_or("a JSON table is a list of objects"))
            .collect::<Result<_, _>>()?,
        _ => return Err("a JSON table is an object or a list of objects".into()),
    };
    if objects.len() > MAX_ROWS {
        return Err(format!("the table holds more than {MAX_ROWS} rows"));
    }
    let mut out = Read::default();
    let mut seen: Vec<String> = Vec::new();
    for o in objects {
        let keys: Vec<String> = o.keys().cloned().collect();
        let mut row = Row {
            unit: None,
            values: Vec::new(),
        };
        if let Some(u) = &spec.unit_column {
            let i = find(&keys, &fold(u), Some(u))
                .ok_or_else(|| format!("a row of the table has no {u} to name its unit"))?;
            row.unit = match &o[&keys[i]] {
                Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            };
        }
        for c in &spec.columns {
            let Some(i) = find(&keys, &c.name, c.from.as_deref()) else {
                continue;
            };
            seen.push(c.name.clone());
            match typed(c.ty, &o[&keys[i]]) {
                Ok(Some(v)) => row.values.push((c.name.clone(), v)),
                Ok(None) => {}
                Err(bad) => out.refused.push(format!("{}: {bad}", c.name)),
            }
        }
        out.rows.push(row);
    }
    out.missing = spec
        .columns
        .iter()
        .filter(|c| !seen.contains(&c.name))
        .map(|c| c.name.clone())
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Column;

    fn col(name: &str, from: Option<&str>, ty: ColumnType) -> Column {
        Column {
            name: name.into(),
            from: from.map(str::to_string),
            ty,
            unit: None,
            description: None,
        }
    }

    #[test]
    fn synthseg_s_volumes_read_by_their_folded_headers() {
        let spec = Table {
            format: "csv".into(),
            columns: vec![
                col("total_intracranial", None, ColumnType::Number),
                col("left_hippocampus", None, ColumnType::Number),
                col("third_ventricle", Some("3rd ventricle"), ColumnType::Number),
                col("not_there", None, ColumnType::Number),
            ],
            unit_column: None,
        };
        let text = "subject,total intracranial,left hippocampus,3rd ventricle\n\
                    /input/sub-01_T1w.nii.gz,1523456.5,4012.25,1100\n";
        let r = read(&spec, text.as_bytes()).unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.missing, ["not_there"]);
        let v: std::collections::BTreeMap<_, _> = r.rows[0].values.iter().cloned().collect();
        assert_eq!(v["left_hippocampus"], 4012.25);
        assert_eq!(v["third_ventricle"], 1100.0);
    }

    #[test]
    fn a_run_s_table_names_its_units_and_refuses_what_is_not_its_type() {
        let spec = Table {
            format: "tsv".into(),
            columns: vec![
                col("snr", None, ColumnType::Number),
                col("holes", None, ColumnType::Integer),
                col("site", None, ColumnType::Text),
            ],
            unit_column: Some("bids_name".into()),
        };
        let text = "bids_name\tsnr\tholes\tsite\nsub-01_ses-a_T1w\t9.5\t12\tA\nsub-02_ses-b_T1w\tn/a\t1.5\tB\n";
        let r = read(&spec, text.as_bytes()).unwrap();
        assert_eq!(r.rows[0].unit.as_deref(), Some("sub-01_ses-a_T1w"));
        assert_eq!(
            r.rows[1].values,
            vec![("site".to_string(), Value::from("B"))]
        );
        assert_eq!(r.refused, ["holes: 1.5"]);
        let no_unit = Table {
            unit_column: Some("unit".into()),
            ..spec
        };
        assert!(read(&no_unit, text.as_bytes()).is_err());
    }

    #[test]
    fn a_json_table_is_an_object_or_a_list() {
        let spec = Table {
            format: "json".into(),
            columns: vec![
                col("snr_total", None, ColumnType::Number),
                col("cjv", None, ColumnType::Number),
            ],
            unit_column: None,
        };
        let r = read(&spec, br#"{"snr_total": 11.2, "cjv": "0.4", "efc": 0.5}"#).unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].values.len(), 2);
        assert!(r.missing.is_empty());
        let r = read(&spec, br#"[{"snr_total": 3}]"#).unwrap();
        assert_eq!(r.missing, ["cjv"]);
        assert!(read(&spec, b"[1]").is_err());
    }
}
