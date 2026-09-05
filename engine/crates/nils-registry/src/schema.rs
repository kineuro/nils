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
        // Wave 3 §5. A scheme says HOW a subject's studies become sessions; it
        // never stores the sessions. Labels are derived on read, so
        // re-labelling a cohort is an edit to one row rather than a migration
        // over every study. `definition` is the scheme itself as JSON, because
        // it is configuration to be read whole, not something anything joins
        // on; `check()` is what stands between it and the resolver.
        Table::new(
            "session_scheme",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                req("definition", Type::Json),
                req("created_at", Type::Timestamp),
                col("note", Type::Text),
            ],
        )
        .unique(&["name"]),
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
        // `reparse_from` is set on a batch that ends `failed`: the `seen_at`
        // of the files its last transaction recorded, which the runs after it
        // read again, since a crash between the registry's commit and the
        // linkage store's loses the identity rows of that transaction (§9.3).
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
                col("reparse_from", Type::Timestamp),
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
                vec![
                    req("first_batch_id", Type::Int),
                    // The day the study happened on, when `study_date` did not
                    // say (Wave 3 §4). Never written over the measured column:
                    // `date_source` names which vote won and `date_weight` and
                    // `date_runner_up` say how close it was.
                    col("date_filled", Type::Date),
                    col("date_source", Type::Text),
                    col("date_weight", Type::Int),
                    col("date_runner_up", Type::Int),
                    // Wave 3 §6: whether any stack of this study is what the
                    // scanner called its output. A fact about the study, not
                    // about a session: the session rescue is this composed
                    // with a scheme, and it is composed on read because the
                    // scheme can change.
                    col("has_original_primary", Type::Int),
                ],
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
                    req("orientation", Type::Text),
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
        // The fingerprint of Wave 2 (`docs/specs/wave2-fingerprint-and-classify.md`,
        // §4.2): the join a classifier would otherwise do per stack, materialized
        // and typed. It holds what is true of the file; what is true of MRI is in
        // a pack, so there are no flags here and the text is folded but not
        // rewritten.
        Table::new(
            "stack_fingerprint",
            vec![
                col("id", Type::Id),
                req("stack_id", Type::Int),
                req("series_id", Type::Int),
                req("study_id", Type::Int),
                req("subject_id", Type::Int),
                req("modality", Type::Text),
                // folded text: NFKC, whitespace collapsed, case kept, because a
                // pack's first normalizer step is a case-sensitive removal
                col("text_series_description", Type::Text),
                col("text_protocol_name", Type::Text),
                col("text_sequence_name", Type::Text),
                col("text_body_part", Type::Text),
                col("text_series_comments", Type::Text),
                col("text_image_comments", Type::Text),
                col("text_all", Type::Text),
                col("text_contrast", Type::Text),
                // the multi-valued fields as read; a parser tokenizes them
                col("image_type", Type::Text),
                col("scanning_sequence", Type::Text),
                col("sequence_variant", Type::Text),
                col("scan_options", Type::Text),
                col("image_orientation_patient", Type::Text),
                // physics
                col("echo_time", Type::Double),
                col("repetition_time", Type::Double),
                col("inversion_time", Type::Double),
                col("flip_angle", Type::Double),
                col("echo_train_length", Type::Int),
                col("echo_numbers", Type::Text),
                // The shell, not the raw element: the values are per image
                // now (§6), and a rule that compares this compares a number.
                col("diffusion_b_value", Type::Double),
                col("magnetic_field_strength", Type::Double),
                col("slice_thickness", Type::Double),
                col("spacing_between_slices", Type::Double),
                col("number_of_averages", Type::Double),
                col("pixel_bandwidth", Type::Text),
                // shape
                col("mr_acquisition_type", Type::Text),
                req("orientation", Type::Text),
                col("orientation_confidence", Type::Double),
                req("n_instances", Type::Int),
                req("stack_index", Type::Int),
                col("signature", Type::Text),
                req("stacks_in_series", Type::Int),
                // Why this stack's series split, when it did: v0's stack key
                // (`sort/stack_key.py`), which its own classifier reads for
                // three flags and never receives. Null for a single-stack
                // series.
                col("split_reason", Type::Text),
                col("rows", Type::Int),
                col("columns", Type::Int),
                col("pixel_spacing", Type::Text),
                col("fov_x", Type::Double),
                col("fov_y", Type::Double),
                col("aspect_ratio", Type::Double),
                // provenance
                col("manufacturer", Type::Text),
                col("manufacturer_model_name", Type::Text),
                col("station_name", Type::Text),
                col("implementation_class_uid", Type::Text),
                col("implementation_version_name", Type::Text),
                // Wave 3 §6: worked out, not read, and beside the measured
                // column rather than over it. v0 writes each of these back
                // into the column it was inferred from, so a guess one run
                // made is a measurement the next run reads.
                col("field_strength_tesla", Type::Double),
                col("field_strength_normalized", Type::Double),
                col("field_strength_unit", Type::Text),
                col("acquisition_type_filled", Type::Text),
                col("acquisition_type_source", Type::Text),
                col("image_role", Type::Text),
                // Wave 3 §6, from the per-image diffusion values: the shell,
                // every shell, the anatomical phase-encoding direction, the
                // gradient count, and which kind of evidence answered each.
                col("dwi_b_value", Type::Double),
                col("dwi_b_values", Type::Text),
                col("dwi_b_value_source", Type::Text),
                col("dwi_pe_direction", Type::Text),
                col("dwi_pe_direction_source", Type::Text),
                col("dwi_directions", Type::Int),
                col("dwi_directions_source", Type::Text),
                // what made it
                req("job_id", Type::Int),
                req("epoch", Type::Int),
            ],
        )
        .unique(&["stack_id"])
        .index(&["series_id"])
        .index(&["modality"]),
        // What a pack decided, and what made it decide
        // (`docs/specs/wave2-fingerprint-and-classify.md`, §8).
        //
        // The axes are the pack's and not the engine's, so they are rows and
        // not columns: the registry stores what a pack says without knowing
        // what any of it means, which is what lets a modality be added
        // without touching this file (§13, slice 8).
        Table::new(
            "classification",
            vec![
                col("id", Type::Id),
                req("stack_id", Type::Int),
                // Which pack judged it, and under which overlay. This is the
                // column that turns a re-classification from a blind
                // overwrite into a diff (§5.2).
                req("pack", Type::Text),
                req("pack_version", Type::Text),
                req("contract", Type::Int),
                col("overlay", Type::Text),
                req("job_id", Type::Int),
                req("epoch", Type::Int),
                // How many review items this stack's verdict raised.
                req("review_items", Type::Int),
            ],
        )
        .unique(&["stack_id"])
        .index(&["pack", "pack_version"]),
        Table::new(
            "classification_axis",
            vec![
                col("id", Type::Id),
                req("stack_id", Type::Int),
                req("axis", Type::Text),
                // What a row stores: one value, or several comma-joined for a
                // multi-valued axis, exactly as v0 wrote them.
                col("value", Type::Text),
                req("confidence", Type::Double),
                req("tier", Type::Text),
            ],
        )
        .unique(&["stack_id", "axis"])
        .index(&["axis", "value"]),
        Table::new(
            "classification_evidence",
            vec![
                col("id", Type::Id),
                req("stack_id", Type::Int),
                req("axis", Type::Text),
                req("value", Type::Text),
                req("tier", Type::Text),
                req("confidence", Type::Double),
                req("rule_set", Type::Text),
                req("rule", Type::Text),
                req("source", Type::Text),
                col("matched", Type::Text),
                // A pass wrote this, and against which named reference. Null
                // when a rule did, which is most of the time.
                col("pass", Type::Text),
                col("reference", Type::Text),
                // Or a person, an agent or a model did (§10.1), and which one.
                // Null when a rule or a pass did. A value a model produced may
                // not sit where a rule's answer belongs and look the same.
                col("author", Type::Text),
                col("author_kind", Type::Text),
            ],
        )
        .index(&["stack_id"]),
        // A person's or an agent's verdict, which outranks a rule and
        // survives a re-classification (C15, D7).
        Table::new(
            "decision",
            vec![
                col("id", Type::Id),
                // What it applies to: stack, series, subject or origin.
                req("scope", Type::Text),
                req("ref", Type::Text),
                req("axis", Type::Text),
                col("value", Type::Text),
                req("actor", Type::Text),
                // Wave 3 §10.1: whether a person, an agent or a model made
                // it, and for a model which version. In the live v0 archive
                // 4,692 body parts are an image model's predictions committed
                // through its QC into the classifier's own column with nothing
                // to mark them; they are discoverable only because v0's
                // keyword classifier happens to disagree.
                req("author_kind", Type::Text),
                col("author_version", Type::Text),
                col("why", Type::Text),
                req("decided_at", Type::Timestamp),
                // A decision a later person withdrew stays, and stops
                // applying: nothing about a human's judgement is deleted.
                col("withdrawn_at", Type::Timestamp),
            ],
        )
        .index(&["scope", "ref", "axis"]),
        // Wave 3 §10: which stack stands for a session's role. One row per
        // role and occasion, and the stacks it names in `pick_stack`.
        //
        // Not an axis, because it is not a property of a stack: the same
        // stack is the session's main T1w or not depending on what else the
        // session holds. And not derived on read either, because it is a
        // decision with evidence and a person may overrule it.
        Table::new(
            "pick",
            vec![
                col("id", Type::Id),
                req("model", Type::Text),
                req("role", Type::Text),
                req("subject_id", Type::Int),
                // The occasion, as the day it opened. A session has no id
                // because it is derived from a scheme (§5), so a pick names
                // the scheme it was made under and the day it names.
                req("session_day", Type::Date),
                req("scheme", Type::Text),
                col("score", Type::Double),
                // How far ahead of the next candidate, as a fraction. Zero is
                // a tie, and a tie is reported rather than settled by row
                // order.
                col("margin", Type::Double),
                col("runner_up_score", Type::Double),
                // `too_close`, `rare`, `nothing_eligible`, comma-joined.
                col("borders", Type::Text),
                // The component scores, and what each read to get there.
                col("parts", Type::Json),
                // Every candidate and its score: what the alternatives were.
                col("considered", Type::Json),
                // The population the cohort-relative components were scored
                // against. v0 computes the same numbers and records none of
                // them, so its picks cannot be reproduced from what is stored.
                req("reference", Type::Text),
                req("pack", Type::Text),
                req("pack_version", Type::Text),
                // Who made it (§10.1). An automatic pick is an agent's.
                req("actor", Type::Text),
                req("author_kind", Type::Text),
                col("author_version", Type::Text),
                col("job_id", Type::Int),
                req("decided_at", Type::Timestamp),
                // A pick a person overruled stays and stops applying.
                col("withdrawn_at", Type::Timestamp),
            ],
        )
        .index(&["role", "subject_id", "session_day"]),
        Table::new(
            "pick_stack",
            vec![
                col("id", Type::Id),
                req("pick_id", Type::Int),
                req("stack_id", Type::Int),
            ],
        )
        .index(&["pick_id"])
        .index(&["stack_id"]),
        // Wave 3 §8.5: what a release did, as rows.
        //
        // Not a workbook beside the originals under a password kept in a
        // database, which is v0's audit, and deliberately without an old-value
        // column anywhere: an audit that records what was removed is a copy of
        // the identifiers, in the registry, in clear. What a release removed is
        // recoverable from the originals by someone entitled to read them.
        Table::new(
            "release",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                req("root", Type::Text),
                // Every policy, written down, because "de-identified" is not a
                // property a file can carry without saying under what rule.
                req("policy", Type::Json),
                // What the release selected, as it was asked for.
                req("selection", Type::Json),
                // The categories it applied, by name. v0's table is a menu and
                // nothing in its output says which pick was made.
                req("categories", Type::Text),
                req("session_scheme", Type::Text),
                req("pack", Type::Text),
                req("pack_version", Type::Text),
                req("actor", Type::Text),
                req("started_at", Type::Timestamp),
                col("finished_at", Type::Timestamp),
                req("files", Type::Int),
                req("subjects", Type::Int),
                col("error", Type::Text),
            ],
        )
        .index(&["name"]),
        Table::new(
            "release_file",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                req("instance_id", Type::Int),
                // Where it landed, under the release's root.
                req("path", Type::Text),
                // What was written, so a handover can be verified without
                // reading the file back (§11).
                req("digest", Type::Text),
                req("bytes", Type::Int),
            ],
        )
        .index(&["release_id"])
        .index(&["instance_id"]),
        // §8.5: what a release changed, by tag and action and count. No old
        // value: an audit that records what was removed is a copy of the
        // identifiers, in the registry, in clear.
        Table::new(
            "release_change",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                // `(0010,0010)` for a standard element, `(0019,xx0C) CREATOR`
                // for a private one, `overlay` and `curve` for a whole group.
                req("tag", Type::Text),
                // `removed`, `replaced`, `shifted`, `remapped`, `kept`.
                req("action", Type::Text),
                req("count", Type::Int),
            ],
        )
        .index(&["release_id"]),
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
