// SPDX-License-Identifier: AGPL-3.0-only

//! The schema, declared once as data (`docs/specs/wave1-parse-and-digest.md`,
//! §4.1 and §4.2) and rendered per backend by [`crate::dialect`]. The catalogue
//! columns of `subject`, `study`, `series`, the three detail tables, `stack`
//! and `instance` come from `nils_dicom::catalogue::CATALOGUE`, so the
//! catalogue and the schema cannot drift apart; the fixed columns are here.
//!
//! The linkage store is a second declaration ([`linkage_tables`]) that lives in
//! its own file or schema (§7.2).

use std::sync::OnceLock;

use nils_dicom::catalogue::{CATALOGUE, fields_of};
use nils_dicom::{Converter, Level};

/// The logical types of §4.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// The generated primary key.
    Id,
    Text,
    Int,
    Double,
    Bool,
    /// `YYYY-MM-DD`.
    Date,
    /// `HH:MM:SS.ffffff`.
    Time,
    /// ISO 8601, UTC.
    Timestamp,
    Json,
    Bytes,
}

impl Type {
    /// The type a catalogue converter's values are stored as.
    pub fn of(converter: Converter) -> Type {
        match converter {
            Converter::Text => Type::Text,
            Converter::Int => Type::Int,
            Converter::Double => Type::Double,
            Converter::Date => Type::Date,
            Converter::Time => Type::Time,
            Converter::Json => Type::Json,
        }
    }
}

/// One column.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: &'static str,
    pub ty: Type,
    pub not_null: bool,
    /// A catalogue column, as opposed to a fixed one.
    pub catalogue: bool,
}

const fn col(name: &'static str, ty: Type) -> Column {
    Column {
        name,
        ty,
        not_null: false,
        catalogue: false,
    }
}

const fn req(name: &'static str, ty: Type) -> Column {
    Column {
        name,
        ty,
        not_null: true,
        catalogue: false,
    }
}

/// One table: its columns in order, its unique keys and its indexes.
#[derive(Debug, Clone)]
pub struct Table {
    pub name: &'static str,
    pub columns: Vec<Column>,
    /// Column sets with a unique index; the first is the `ON CONFLICT` target
    /// of the writer.
    pub uniques: Vec<Vec<&'static str>>,
    pub indexes: Vec<Vec<&'static str>>,
    /// A table whose primary key is a column of its own (`series_id` on the
    /// detail tables) instead of a generated id.
    pub primary: Option<&'static str>,
}

impl Table {
    fn new(name: &'static str, columns: Vec<Column>) -> Table {
        Table {
            name,
            columns,
            uniques: Vec::new(),
            indexes: Vec::new(),
            primary: None,
        }
    }

    fn unique(mut self, cols: &[&'static str]) -> Table {
        self.uniques.push(cols.to_vec());
        self
    }

    fn index(mut self, cols: &[&'static str]) -> Table {
        self.indexes.push(cols.to_vec());
        self
    }

    fn keyed_by(mut self, column: &'static str) -> Table {
        self.primary = Some(column);
        self
    }

    /// The column of that name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Every column but the generated id.
    pub fn data_columns(&self) -> impl Iterator<Item = &Column> {
        self.columns.iter().filter(|c| c.ty != Type::Id)
    }

    /// The names of the catalogue columns, in catalogue order.
    pub fn catalogue_columns(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.columns.iter().filter(|c| c.catalogue).map(|c| c.name)
    }
}

/// The catalogue columns of one level, in catalogue order.
fn catalogue_columns(level: Level) -> Vec<Column> {
    fields_of(level)
        .map(|(_, f)| Column {
            name: f.column,
            ty: Type::of(f.converter),
            not_null: false,
            catalogue: true,
        })
        .collect()
}

fn with_catalogue(mut fixed: Vec<Column>, level: Level, tail: Vec<Column>) -> Vec<Column> {
    fixed.extend(catalogue_columns(level));
    fixed.extend(tail);
    fixed
}

/// The registry's tables, in creation order.
pub fn registry_tables() -> &'static [Table] {
    static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
    TABLES.get_or_init(build_registry)
}

/// The linkage store's tables, in creation order (§7.2).
pub fn linkage_tables() -> &'static [Table] {
    static TABLES: OnceLock<Vec<Table>> = OnceLock::new();
    TABLES.get_or_init(build_linkage)
}

/// The table of that name, in either store.
pub fn table(name: &str) -> &'static Table {
    registry_tables()
        .iter()
        .chain(linkage_tables())
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no table named {name}"))
}

fn build_registry() -> Vec<Table> {
    debug_assert!(!CATALOGUE.is_empty());
    vec![
        Table::new(
            "registry_meta",
            vec![req("key", Type::Text), req("value", Type::Text)],
        )
        .keyed_by("key"),
        Table::new(
            "job",
            vec![
                col("id", Type::Id),
                req("kind", Type::Text),
                col("name", Type::Text),
                col("args", Type::Json),
                req("state", Type::Text),
                col("pid", Type::Int),
                col("host", Type::Text),
                req("started_at", Type::Timestamp),
                col("heartbeat_at", Type::Timestamp),
                col("finished_at", Type::Timestamp),
                col("progress", Type::Json),
                col("error", Type::Text),
            ],
        )
        .index(&["state"]),
        Table::new(
            "source",
            vec![
                col("id", Type::Id),
                req("root", Type::Text),
                req("root_canonical", Type::Text),
                req("first_seen_at", Type::Timestamp),
            ],
        )
        .unique(&["root_canonical"]),
        Table::new(
            "ingest_batch",
            vec![
                col("id", Type::Id),
                req("source_id", Type::Int),
                col("job_id", Type::Int),
                req("name", Type::Text),
                req("config", Type::Json),
                req("started_at", Type::Timestamp),
                col("finished_at", Type::Timestamp),
                req("state", Type::Text),
                col("counts", Type::Json),
                col("epoch_after", Type::Int),
            ],
        )
        .index(&["source_id"]),
        Table::new(
            "source_file",
            vec![
                col("id", Type::Id),
                req("source_id", Type::Int),
                req("batch_id", Type::Int),
                req("dir", Type::Text),
                req("path", Type::Text),
                req("size", Type::Int),
                req("mtime_ns", Type::Int),
                req("status", Type::Text),
                col("reason", Type::Text),
                col("detail", Type::Text),
                col("instance_id", Type::Int),
                req("seen_at", Type::Timestamp),
            ],
        )
        .unique(&["source_id", "path"])
        .index(&["source_id", "dir"])
        .index(&["batch_id", "status"])
        .index(&["instance_id"]),
        // `code_digest` and `first_batch_id` are null for a subject that
        // `nils linkage import` created: its code came from outside, not from
        // the scheme, and no batch made it (§7.4).
        Table::new(
            "subject",
            with_catalogue(
                vec![
                    col("id", Type::Id),
                    req("code", Type::Text),
                    col("code_digest", Type::Bytes),
                ],
                Level::Subject,
                vec![
                    col("first_batch_id", Type::Int),
                    req("created_at", Type::Timestamp),
                ],
            ),
        )
        .unique(&["code"]),
        Table::new(
            "study",
            with_catalogue(
                vec![
                    col("id", Type::Id),
                    req("study_instance_uid", Type::Text),
                    req("subject_id", Type::Int),
                ],
                Level::Study,
                vec![req("first_batch_id", Type::Int)],
            ),
        )
        .unique(&["study_instance_uid"])
        .index(&["subject_id"]),
        Table::new(
            "series",
            with_catalogue(
                vec![
                    col("id", Type::Id),
                    req("series_instance_uid", Type::Text),
                    req("study_id", Type::Int),
                    req("subject_id", Type::Int),
                ],
                Level::Series,
                vec![
                    req("n_instances", Type::Int),
                    req("n_stacks", Type::Int),
                    req("first_batch_id", Type::Int),
                ],
            ),
        )
        .unique(&["series_instance_uid"])
        .index(&["study_id"])
        .index(&["subject_id"]),
        detail("series_mr", Level::SeriesMr),
        detail("series_ct", Level::SeriesCt),
        detail("series_pet", Level::SeriesPet),
        Table::new(
            "stack",
            with_catalogue(
                vec![
                    col("id", Type::Id),
                    req("series_id", Type::Int),
                    req("stack_index", Type::Int),
                    req("stack_key", Type::Text),
                    req("modality", Type::Text),
                ],
                Level::Stack,
                vec![
                    col("orientation_confidence", Type::Double),
                    req("n_instances", Type::Int),
                    req("first_batch_id", Type::Int),
                ],
            ),
        )
        .unique(&["series_id", "stack_index"])
        .unique(&["series_id", "stack_key"]),
        Table::new(
            "instance",
            with_catalogue(
                vec![
                    col("id", Type::Id),
                    req("sop_instance_uid", Type::Text),
                    req("series_id", Type::Int),
                    col("stack_id", Type::Int),
                ],
                Level::Instance,
                vec![
                    col("source_file_id", Type::Int),
                    req("first_batch_id", Type::Int),
                ],
            ),
        )
        .unique(&["sop_instance_uid"])
        .index(&["series_id"])
        .index(&["stack_id"]),
        Table::new(
            "diagnostic",
            vec![
                col("id", Type::Id),
                req("batch_id", Type::Int),
                req("kind", Type::Text),
                req("scope", Type::Text),
                col("ref_id", Type::Int),
                req("count", Type::Int),
                col("sample", Type::Json),
                req("created_at", Type::Timestamp),
            ],
        )
        .index(&["batch_id", "kind"]),
        Table::new(
            "review_item",
            vec![
                col("id", Type::Id),
                req("kind", Type::Text),
                req("scope", Type::Text),
                col("ref", Type::Json),
                col("evidence", Type::Json),
                req("status", Type::Text),
                col("actor", Type::Text),
                req("created_at", Type::Timestamp),
                col("decided_at", Type::Timestamp),
                col("decision", Type::Json),
            ],
        )
        .index(&["status", "kind"]),
    ]
}

fn detail(name: &'static str, level: Level) -> Table {
    Table::new(
        name,
        with_catalogue(vec![req("series_id", Type::Int)], level, Vec::new()),
    )
    .keyed_by("series_id")
}

fn build_linkage() -> Vec<Table> {
    vec![
        Table::new(
            "linkage_meta",
            vec![req("key", Type::Text), req("value", Type::Text)],
        )
        .keyed_by("key"),
        Table::new(
            "id_type",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                col("description", Type::Text),
            ],
        )
        .unique(&["name"]),
        Table::new(
            "identity",
            vec![
                col("id", Type::Id),
                req("subject_id", Type::Int),
                req("id_type_id", Type::Int),
                req("lookup", Type::Bytes),
                req("ciphertext", Type::Bytes),
                req("source", Type::Text),
                col("first_batch_id", Type::Int),
                req("created_at", Type::Timestamp),
            ],
        )
        .unique(&["id_type_id", "lookup"])
        .index(&["subject_id"]),
        Table::new(
            "linkage",
            vec![
                col("id", Type::Id),
                req("subject_a", Type::Int),
                req("subject_b", Type::Int),
                req("kind", Type::Text),
                col("evidence", Type::Json),
                col("actor", Type::Text),
                req("created_at", Type::Timestamp),
                col("reversed_at", Type::Timestamp),
                col("reversed_by", Type::Text),
            ],
        )
        .index(&["subject_a"])
        .index(&["subject_b"]),
        Table::new(
            "date_shift",
            vec![req("subject_id", Type::Int), req("offset_days", Type::Int)],
        )
        .keyed_by("subject_id"),
        Table::new(
            "read_audit",
            vec![
                col("id", Type::Id),
                req("at", Type::Timestamp),
                req("actor", Type::Text),
                req("identity_id", Type::Int),
                col("why", Type::Text),
            ],
        )
        .index(&["identity_id"]),
    ]
}

/// The id types seeded at `nils init` (§7.2).
pub const ID_TYPES: [(&str, &str); 2] = [
    ("patient-id", "PatientID (0010,0020) as written, trimmed"),
    (
        "study-instance-uid",
        "StudyInstanceUID, the fallback when PatientID is absent",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_catalogue_level_has_its_table_and_columns() {
        for level in Level::ALL {
            let table = registry_tables()
                .iter()
                .find(|t| t.name == level.name())
                .unwrap_or_else(|| panic!("no table for {level}"));
            let expected: Vec<&str> = fields_of(level).map(|(_, f)| f.column).collect();
            let got: Vec<&str> = table.catalogue_columns().collect();
            assert_eq!(got, expected, "{level}");
        }
        assert_eq!(table("series_mr").primary, Some("series_id"));
        assert_eq!(table("instance").uniques[0], vec!["sop_instance_uid"]);
        assert_eq!(table("source_file").uniques[0], vec!["source_id", "path"]);
        assert_eq!(linkage_tables().len(), 6);
        assert_eq!(linkage_tables()[0].name, "linkage_meta");
    }

    #[test]
    fn names_are_distinct_and_every_table_has_a_key() {
        let mut names = HashSet::new();
        for t in registry_tables().iter().chain(linkage_tables()) {
            assert!(names.insert(t.name), "{} twice", t.name);
            let mut cols = HashSet::new();
            for c in &t.columns {
                assert!(cols.insert(c.name), "{}.{} twice", t.name, c.name);
            }
            let has_id = t.columns.iter().any(|c| c.ty == Type::Id);
            assert!(
                has_id != t.primary.is_some(),
                "{} needs exactly one primary key",
                t.name
            );
            for key in t.uniques.iter().chain(&t.indexes) {
                for c in key {
                    assert!(t.column(c).is_some(), "{}.{} indexed but absent", t.name, c);
                }
            }
        }
    }
}
