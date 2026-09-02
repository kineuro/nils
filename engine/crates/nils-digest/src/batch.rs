// SPDX-License-Identifier: AGPL-3.0-only

//! What the parsers hand the writer (§9.1): items in batches, each parsed
//! file carrying the per-field hashes of its study and series rows, so the
//! writer can tell a disagreeing instance from a cached row without a read.
//!
//! A hash is of the field's canonical text, which a value from the reader and
//! a cell read back from either backend render the same way: dates and text
//! as written, a time without a zero fraction, JSON with sorted keys and
//! every number as a double, a double in Rust's shortest form.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use nils_dicom::catalogue::{VARIES_PER_INSTANCE, fields_of, stack_defining};
use nils_dicom::{Converter, Extracted, Level, Refusal, Value};
use nils_registry::store::Cell;

use crate::rule::Ident;
use crate::stack::Signature;
use crate::walk::SkipReason;

/// What an earlier run recorded for a path that is read again (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prior {
    /// The instance the earlier run filed the path under, if any.
    pub instance_id: Option<i64>,
    /// The size or the modification time differ from the record.
    pub changed: bool,
}

/// A file the reader accepted, with what the writer needs beside it.
pub struct ParsedFile {
    pub extracted: Extracted,
    /// The identifier the rule resolved (§7.3); the values it read are gone.
    pub ident: Ident,
    /// The stack the instance belongs to (§8), computed from the file alone.
    pub signature: Signature,
    /// Relative to the root, forward slashes.
    pub path: String,
    /// The directory part of `path`, empty at the root.
    pub dir: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub hashes: RowHashes,
    pub prior: Option<Prior>,
}

/// One thing the writer records.
pub enum Item {
    Parsed(Box<ParsedFile>),
    Refused {
        path: String,
        dir: String,
        size: u64,
        mtime_ns: i64,
        refusal: Refusal,
    },
    /// A file an earlier run recorded and this one found unchanged: only its
    /// row's batch and seen time move. The resume stage batches these itself;
    /// a parser never sees one.
    Unchanged {
        id: i64,
        quarantined: bool,
    },
    /// A symbolic link or a special file: a row, no read.
    Skipped {
        path: String,
        dir: String,
        size: u64,
        mtime_ns: i64,
        reason: SkipReason,
    },
    /// A directory that could not be listed; the error text, never the path.
    WalkError {
        error: String,
    },
}

/// What the resume stage hands a parser.
pub enum Task {
    Parse {
        path: PathBuf,
        rel: String,
        dir: String,
        size: u64,
        mtime_ns: i64,
        prior: Option<Prior>,
    },
    Skipped {
        rel: String,
        dir: String,
        size: u64,
        mtime_ns: i64,
        reason: SkipReason,
    },
    WalkError {
        error: String,
    },
}

/// A batch: what one parser (or, for unchanged files, the resume stage)
/// collected until the batch was full.
pub struct Batch {
    pub items: Vec<Item>,
    /// How many of the items are parsed files.
    pub parsed: usize,
}

/// Collects items into batches of `rows` parsed files (§9.1); a batch also
/// closes at `8 * rows` items of any kind, so a tree of unchanged files does
/// not grow one without bound.
pub struct Batcher {
    rows: usize,
    items: Vec<Item>,
    parsed: usize,
}

impl Batcher {
    pub fn new(rows: usize) -> Batcher {
        Batcher {
            rows: rows.max(1),
            items: Vec::new(),
            parsed: 0,
        }
    }

    /// Add an item; the batch comes back when it is full.
    pub fn push(&mut self, item: Item) -> Option<Batch> {
        if matches!(item, Item::Parsed(_)) {
            self.parsed += 1;
        }
        self.items.push(item);
        if self.parsed >= self.rows || self.items.len() >= 8 * self.rows {
            self.take()
        } else {
            None
        }
    }

    /// Whatever is collected, if anything.
    pub fn take(&mut self) -> Option<Batch> {
        if self.items.is_empty() {
            return None;
        }
        Some(Batch {
            items: std::mem::take(&mut self.items),
            parsed: std::mem::take(&mut self.parsed),
        })
    }
}

/// The detail level of a modality, if it has one.
pub fn detail_level(modality: &str) -> Option<Level> {
    Level::ALL
        .iter()
        .copied()
        .find(|l| l.modality() == Some(modality))
}

/// The per-field hashes of a file's study and series rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowHashes {
    pub study: Box<[u32]>,
    /// The series columns, then the detail table's, in catalogue order.
    pub series: Box<[u32]>,
}

impl RowHashes {
    pub fn of(x: &Extracted) -> RowHashes {
        let study: Vec<u32> = x.row(Level::Study).map(|(_, v)| hash_value(v)).collect();
        let mut series: Vec<u32> = x.row(Level::Series).map(|(_, v)| hash_value(v)).collect();
        if let Some(level) = detail_level(&x.modality) {
            series.extend(x.row(level).map(|(_, v)| hash_value(v)));
        }
        RowHashes {
            study: study.into_boxed_slice(),
            series: series.into_boxed_slice(),
        }
    }
}

/// The catalogue columns of a row the writer compares: names, converters, and
/// whether each takes part (the per-instance columns of the series row do
/// not, §9.1, nor the series columns a stack signature is made of, §8).
#[derive(Debug, Clone)]
pub struct Fields {
    pub names: Vec<&'static str>,
    pub levels: Vec<Level>,
    pub converters: Vec<Converter>,
    pub compared: Vec<bool>,
}

impl Fields {
    pub fn of(levels: &[Level]) -> Fields {
        let mut f = Fields {
            names: Vec::new(),
            levels: Vec::new(),
            converters: Vec::new(),
            compared: Vec::new(),
        };
        for &level in levels {
            for (_, field) in fields_of(level) {
                f.names.push(field.column);
                f.levels.push(level);
                f.converters.push(field.converter);
                let varies = level == Level::Series && VARIES_PER_INSTANCE.contains(&field.column);
                f.compared.push(!(varies || stack_defining(field)));
            }
        }
        f
    }

    pub fn study() -> Fields {
        Fields::of(&[Level::Study])
    }

    pub fn subject() -> Fields {
        Fields::of(&[Level::Subject])
    }

    /// The series row and, for a modality with a detail table, its detail row.
    pub fn series(modality: &str) -> Fields {
        match detail_level(modality) {
            Some(level) => Fields::of(&[Level::Series, level]),
            None => Fields::of(&[Level::Series]),
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// `table.column`, as a diagnostic names a field.
    pub fn label(&self, i: usize) -> String {
        format!("{}.{}", self.levels[i].name(), self.names[i])
    }

    /// The hashes of a row read back, aligned with the hashes of a parsed
    /// file; `cells` holds the catalogue columns in this order.
    pub fn hash_cells(&self, cells: &[Cell]) -> Box<[u32]> {
        debug_assert_eq!(cells.len(), self.len());
        self.converters
            .iter()
            .zip(cells)
            .map(|(&c, cell)| hash_cell(c, cell))
            .collect()
    }

    /// The compared fields where both sides hold a value and the values
    /// differ: `(index, name)`. A null is no value: it neither wins nor
    /// disagrees.
    pub fn differing<'a>(
        &'a self,
        mine: &'a [u32],
        theirs: &'a [u32],
    ) -> impl Iterator<Item = (usize, &'static str)> + 'a {
        let none = hash32(None);
        mine.iter()
            .zip(theirs)
            .enumerate()
            .filter(move |(i, (a, b))| self.compared[*i] && a != b && **a != none && **b != none)
            .map(move |(i, _)| (i, self.names[i]))
    }

    /// The compared fields the stored row holds no value for and the file
    /// does: the writer fills them.
    pub fn fillable(&self, mine: &[u32], theirs: &[u32]) -> Vec<usize> {
        let none = hash32(None);
        mine.iter()
            .zip(theirs)
            .enumerate()
            .filter(|(i, (a, b))| self.compared[*i] && **b == none && **a != none)
            .map(|(i, _)| i)
            .collect()
    }
}

/// A 32-bit hash of a field's canonical text; null hashes apart from any text.
pub fn hash32(text: Option<&str>) -> u32 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish() as u32
}

pub fn hash_value(v: Option<&Value>) -> u32 {
    hash32(v.map(canonical_value).as_deref())
}

pub fn hash_cell(converter: Converter, cell: &Cell) -> u32 {
    hash32(canonical_cell(converter, cell).as_deref())
}

/// The canonical text of a value from the reader.
pub fn canonical_value(v: &Value) -> Cow<'_, str> {
    match v {
        Value::Text(s) | Value::Date(s) => Cow::Borrowed(s),
        Value::Int(i) => Cow::Owned(i.to_string()),
        Value::Double(d) => Cow::Owned(canonical_double(*d)),
        Value::Time(s) => canonical_time(s),
        Value::Json(s) => canonical_json(s),
    }
}

/// The canonical text of a cell read back from a column of that converter;
/// none for null.
pub fn canonical_cell(converter: Converter, cell: &Cell) -> Option<Cow<'_, str>> {
    Some(match (converter, cell) {
        (_, Cell::Null) => return None,
        (Converter::Time, Cell::Text(s)) => canonical_time(s),
        (Converter::Json, Cell::Text(s)) => canonical_json(s),
        (Converter::Double, Cell::Int(i)) => Cow::Owned(canonical_double(*i as f64)),
        (Converter::Int, Cell::Double(d)) if d.fract() == 0.0 => {
            Cow::Owned((*d as i64).to_string())
        }
        (_, Cell::Text(s)) => Cow::Borrowed(s.as_str()),
        (_, Cell::Int(i)) => Cow::Owned(i.to_string()),
        (_, Cell::Double(d)) => Cow::Owned(canonical_double(*d)),
        (_, Cell::Bool(b)) => Cow::Owned(i64::from(*b).to_string()),
        (_, Cell::Bytes(b)) => Cow::Owned(String::from_utf8_lossy(b).into_owned()),
    })
}

fn canonical_double(d: f64) -> String {
    format!("{d:?}")
}

/// `HH:MM:SS.000000` is `HH:MM:SS`.
fn canonical_time(s: &str) -> Cow<'_, str> {
    match s.strip_suffix(".000000") {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Borrowed(s),
    }
}

/// JSON with its keys sorted, no spaces, every number a double.
fn canonical_json(s: &str) -> Cow<'_, str> {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => {
            let mut out = String::with_capacity(s.len());
            write_canonical(&v, &mut out);
            Cow::Owned(out)
        }
        Err(_) => Cow::Borrowed(s),
    }
}

fn write_canonical(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => {
            out.push_str(&canonical_double(n.as_f64().unwrap_or(f64::NAN)))
        }
        serde_json::Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = map.iter().collect();
            out.push('{');
            for (i, (k, v)) in sorted.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                write_canonical(v, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::QuarantineClass;

    #[test]
    fn canonical_texts_agree_between_values_and_cells() {
        let cases: Vec<(Converter, Value, Cell)> = vec![
            (
                Converter::Text,
                Value::Text("a b".into()),
                Cell::Text("a b".into()),
            ),
            (Converter::Int, Value::Int(-4), Cell::Int(-4)),
            (Converter::Int, Value::Int(4), Cell::Double(4.0)),
            (Converter::Double, Value::Double(2.0), Cell::Double(2.0)),
            (Converter::Double, Value::Double(2.0), Cell::Int(2)),
            (Converter::Double, Value::Double(0.1), Cell::Double(0.1)),
            (
                Converter::Date,
                Value::Date("2024-02-29".into()),
                Cell::Text("2024-02-29".into()),
            ),
            (
                Converter::Time,
                Value::Time("14:03:07".into()),
                Cell::Text("14:03:07.000000".into()),
            ),
            (
                Converter::Time,
                Value::Time("14:03:07.250000".into()),
                Cell::Text("14:03:07.250000".into()),
            ),
            (
                Converter::Json,
                Value::Json(r#"{"b":[1,2.50,"x"],"a":{"z":null,"y":true}}"#.into()),
                Cell::Text(r#"{"a": {"y": true, "z": null}, "b": [1, 2.5, "x"]}"#.into()),
            ),
        ];
        for (c, v, cell) in &cases {
            assert_eq!(
                canonical_value(v),
                canonical_cell(*c, cell).unwrap(),
                "{c:?} {v:?} {cell:?}"
            );
            assert_eq!(hash_value(Some(v)), hash_cell(*c, cell));
        }
        assert_eq!(canonical_cell(Converter::Text, &Cell::Null), None);
        assert_ne!(
            hash_value(None),
            hash_value(Some(&Value::Text(String::new())))
        );
        assert_ne!(
            hash_value(Some(&Value::Text("a".into()))),
            hash_value(Some(&Value::Text("b".into())))
        );
        assert_eq!(
            canonical_json(r#"{"n": 1e2}"#),
            canonical_json(r#"{"n": 100}"#)
        );
        assert_eq!(canonical_json("not json"), "not json");
    }

    #[test]
    fn fields_follow_the_catalogue_and_skip_the_per_instance_and_stack_columns() {
        let study = Fields::study();
        assert_eq!(study.len(), fields_of(Level::Study).count());
        assert!(study.compared.iter().all(|&c| c));
        let mr = Fields::series("MR");
        assert_eq!(
            mr.len(),
            fields_of(Level::Series).count() + fields_of(Level::SeriesMr).count()
        );
        let skipped: Vec<&str> = mr
            .names
            .iter()
            .zip(&mr.compared)
            .filter(|(_, c)| !**c)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(
            skipped,
            [
                "media_storage_sop_instance_uid",
                "image_type",
                "image_orientation_patient",
                "image_position_patient",
                "repetition_time",
                "echo_time",
                "inversion_time",
                "flip_angle",
                "echo_numbers",
                "echo_train_length",
                "receive_coil_name",
            ]
        );
        assert!(skipped.iter().all(|n| {
            VARIES_PER_INSTANCE.contains(n)
                || fields_of(Level::Series)
                    .chain(fields_of(Level::SeriesMr))
                    .any(|(_, f)| f.column == *n && stack_defining(f))
        }));
        assert_eq!(Fields::series("XX").len(), fields_of(Level::Series).count());
        assert_eq!(detail_level("PT"), Some(Level::SeriesPet));
        assert_eq!(detail_level("US"), None);

        let mine: Vec<u32> = (0..mr.len() as u32).collect();
        let mut theirs = mine.clone();
        let sop = mr
            .names
            .iter()
            .position(|n| *n == "media_storage_sop_instance_uid")
            .unwrap();
        let desc = mr
            .names
            .iter()
            .position(|n| *n == "series_description")
            .unwrap();
        theirs[sop] += 1;
        theirs[desc] += 1;
        let diff: Vec<_> = mr.differing(&mine, &theirs).collect();
        assert_eq!(diff, vec![(desc, "series_description")]);
    }

    #[test]
    fn batches_close_on_parsed_rows_or_on_items() {
        let mut b = Batcher::new(2);
        let refused = || Item::Refused {
            path: "x".into(),
            dir: String::new(),
            size: 0,
            mtime_ns: 0,
            refusal: Refusal::new(QuarantineClass::NotDicom, None),
        };
        for _ in 0..15 {
            assert!(b.push(refused()).is_none());
        }
        let full = b.push(refused()).unwrap();
        assert_eq!(full.items.len(), 16);
        assert_eq!(full.parsed, 0);
        assert!(b.take().is_none());
        assert!(b.push(Item::WalkError { error: "e".into() }).is_none());
        let rest = b.take().unwrap();
        assert_eq!(rest.items.len(), 1);
    }

    #[test]
    fn the_hashes_cover_the_study_and_series_rows() {
        use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10};
        let dir = TempDir::new("hashes");
        let path = dir.file(
            "a.dcm",
            &part10(
                &MetaFields::mr("1.2.3.4.5"),
                &minimal_mr("1.2.3", "1.2.3.4", "1.2.3.4.5"),
                true,
            ),
        );
        let x = nils_dicom::extract(&path).unwrap();
        let h = RowHashes::of(&x);
        assert_eq!(h.study.len(), fields_of(Level::Study).count());
        assert_eq!(h.series.len(), Fields::series("MR").len());
    }
}
