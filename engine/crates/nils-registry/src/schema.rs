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

pub(crate) const fn col(name: &'static str, ty: Type) -> Column {
    Column {
        name,
        ty,
        not_null: false,
        catalogue: false,
    }
}

pub(crate) const fn req(name: &'static str, ty: Type) -> Column {
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

/// A table a migration created and a later migration dropped, kept in the
/// shape the dropping migration expects, so that the upgrade path still runs
/// on a registry that has it. Never created for a fresh registry.
pub fn historical(name: &str) -> &'static Table {
    static HISTORICAL: OnceLock<Vec<Table>> = OnceLock::new();
    HISTORICAL
        .get_or_init(build_historical)
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no historical table named {name}"))
}

fn build_historical() -> Vec<Table> {
    vec![
        // Wave 3 §9's manifest, a row per file per version: created by
        // migration 11, rebuilt by 16 to know its stack, folded into
        // `release_stack` and dropped by 18 (Wave 4a §4).
        Table::new(
            "release_file",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                req("stack_id", Type::Int),
                col("instance_id", Type::Int),
                req("path", Type::Text),
                req("digest", Type::Text),
                req("bytes", Type::Int),
            ],
        )
        .index(&["release_id"])
        .index(&["instance_id"]),
    ]
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
        // Wave 4a §9.2: the audit log that is a table. Who did what, to
        // which scope, when, under which policy; never an identifier.
        Table::new(
            "audit",
            vec![
                col("id", Type::Id),
                req("at", Type::Timestamp),
                req("principal", Type::Text),
                req("action", Type::Text),
                req("scope", Type::Json),
                col("policy", Type::Json),
                col("job_id", Type::Int),
                col("epoch", Type::Int),
                col("details", Type::Json),
            ],
        )
        .index(&["principal"])
        .index(&["action"]),
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
                    // Wave 4a §7.1: the birth date and the sex are catalogue
                    // columns already, read from the files; the importer
                    // fills them where the files are silent, and a later
                    // file that disagrees is a review item rather than an
                    // overwrite (§13.3). What no file carries is when the
                    // subject died.
                    col("deceased_at", Type::Date),
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
        // Wave 4a §5.2: the private elements a pack asked the digest to read,
        // per series, as one JSON object keyed by each element's address
        // (`0019xx0C SIEMENS MR HEADER`). A series and not a column per
        // element, because which elements are read is pack data and changes
        // without the schema changing; a series and not an instance, because
        // an element worth reading is a parameter of the acquisition, and the
        // writer's rule for a field two files disagree on applies (the
        // smaller value in text order stays, and the address is listed under
        // `varied` so a reader knows the series was not of one mind).
        Table::new(
            "series_private",
            vec![
                col("id", Type::Id),
                req("series_id", Type::Int),
                req("elements", Type::Json),
                col("varied", Type::Text),
            ],
        )
        .unique(&["series_id"]),
        // Wave 4a §7.1: the clinical layer, in the one registry, holding what
        // v0 kept in a second database. The date is the join key: an import
        // matches on (subject, event_date), a session anchor is an event
        // date, and age, disease duration and the nearest observation are
        // date arithmetic. Nothing here is rewritten by a date policy. A
        // correction supersedes the old row and the old row stays
        // (`superseded_by`), which is the rule `decision` already lives by.
        //
        // A cohort is a membership fact in this wave (§13.7).
        Table::new(
            "cohort",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                req("owner", Type::Text),
                col("description", Type::Text),
                req("created_at", Type::Timestamp),
            ],
        )
        .unique(&["name"]),
        Table::new(
            "cohort_member",
            vec![
                col("id", Type::Id),
                req("cohort_id", Type::Int),
                req("subject_id", Type::Int),
                req("joined_at", Type::Timestamp),
                col("left_at", Type::Timestamp),
                col("notes", Type::Text),
            ],
        )
        .unique(&["cohort_id", "subject_id"])
        .index(&["subject_id"]),
        // The vocabulary: diseases and their types, and the kinds of
        // observation. Pack data, loaded by `nils clinical vocabulary load`,
        // because which scales a clinic records is knowledge about the
        // clinic and not about the engine.
        Table::new(
            "disease",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                col("code", Type::Text),
                col("description", Type::Text),
            ],
        )
        .unique(&["name"]),
        Table::new(
            "disease_type",
            vec![
                col("id", Type::Id),
                req("disease_id", Type::Int),
                req("name", Type::Text),
                col("description", Type::Text),
                col("sort_order", Type::Int),
            ],
        )
        .unique(&["disease_id", "name"]),
        Table::new(
            "observation_type",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                req("category", Type::Text),
                // numeric, text, boolean, json, or none for an observation
                // that is a date and nothing else (a diagnosis, an onset).
                col("value_type", Type::Text),
                col("unit", Type::Text),
                col("min_value", Type::Double),
                col("max_value", Type::Double),
                req("is_primary", Type::Int),
                // Wave 4a §7.4: a kind the pack marks sensitive is never
                // released, by name or by default.
                col("is_sensitive", Type::Int),
                col("description", Type::Text),
            ],
        )
        .unique(&["name"]),
        Table::new(
            "subject_disease",
            vec![
                col("id", Type::Id),
                req("subject_id", Type::Int),
                req("disease_id", Type::Int),
                col("onset_event_id", Type::Int),
                col("diagnosis_event_id", Type::Int),
                col("notes", Type::Text),
                col("family_history", Type::Text),
                req("created_at", Type::Timestamp),
                col("actor", Type::Text),
                col("superseded_by", Type::Int),
            ],
        )
        .index(&["subject_id"]),
        Table::new(
            "subject_disease_type",
            vec![
                col("id", Type::Id),
                req("subject_disease_id", Type::Int),
                req("disease_type_id", Type::Int),
                col("assigned_on", Type::Date),
                col("transition_event_id", Type::Int),
                col("notes", Type::Text),
                req("created_at", Type::Timestamp),
                col("actor", Type::Text),
                col("superseded_by", Type::Int),
            ],
        )
        .index(&["subject_disease_id"]),
        // One event-attribute-value shape for every clinical import, kept
        // from v0 because every import is one: what was observed, when, of
        // whom, and the value if the kind has one. `value` is the text as
        // it arrived; `number` is the value as a number when the kind is
        // numeric, so a window and a nearest-of are arithmetic and not a
        // parse.
        Table::new(
            "event",
            vec![
                col("id", Type::Id),
                req("subject_id", Type::Int),
                req("observation_type_id", Type::Int),
                req("event_date", Type::Date),
                col("event_time", Type::Time),
                col("value", Type::Text),
                col("number", Type::Double),
                col("unit", Type::Text),
                col("source", Type::Text),
                col("quality", Type::Text),
                col("notes", Type::Text),
                req("created_at", Type::Timestamp),
                col("actor", Type::Text),
                col("superseded_by", Type::Int),
            ],
        )
        .index(&["subject_id", "observation_type_id", "event_date"])
        .index(&["observation_type_id", "event_date"]),
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
                // One value per row (Wave 4a §6.1, fault 4): a single-valued
                // axis has one row, a multi-valued one has a row per value,
                // and an axis decided to nothing has one row with no value.
                // Wave 2 stored several values comma-joined, as v0 did, which
                // is why a role match was four LIKE patterns.
                col("value", Type::Text),
                req("confidence", Type::Double),
                req("tier", Type::Text),
            ],
        )
        .unique(&["stack_id", "axis", "value"])
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
                // Wave 4a §10.2: a staged decision is written but not in
                // force until committed; the epoch at staging is the drift
                // signature a commit checks.
                col("staged_at", Type::Timestamp),
                col("committed_at", Type::Timestamp),
                col("epoch_staged", Type::Int),
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
        // Wave 4a §4: a dataset is a name released into a root, and it is what
        // a version is a version of. Running the same name into the same root
        // makes the next version of the same tree (§8.6).
        Table::new(
            "dataset",
            vec![
                col("id", Type::Id),
                req("name", Type::Text),
                req("root", Type::Text),
                req("created_at", Type::Timestamp),
            ],
        )
        .unique(&["name", "root"]),
        Table::new(
            "release",
            vec![
                col("id", Type::Id),
                // The dataset. Running the same name again makes the next
                // version of the same tree (§8.6).
                req("name", Type::Text),
                // And the row that says so, since Wave 4a §4.
                col("dataset_id", Type::Int),
                // `YYYY.MM.DD.N`: it sorts by component, reads as the day it
                // was made, and N separates two runs on one day.
                req("version", Type::Text),
                // The version this one was worked out against. Null for the
                // first, which is why a first release writes everything.
                col("previous_id", Type::Int),
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
                // Which of the two layouts of §9 the tree is in, and for the
                // BIDS one the placements it chose (§9.3): a tree that does
                // not say where it put its localizers is a tree whose absence
                // of localizers means nothing.
                req("layout", Type::Text),
                req("placements", Type::Json),
                // The converter it found, recorded because a tree should say
                // which converter made it (§9.6).
                col("converter", Type::Text),
                req("pack", Type::Text),
                req("pack_version", Type::Text),
                req("actor", Type::Text),
                req("started_at", Type::Timestamp),
                col("finished_at", Type::Timestamp),
                // Wave 4a §13.1: a release is never removed; it may be
                // withdrawn, with a reason, by a person.
                col("withdrawn_at", Type::Timestamp),
                col("withdrawn_by", Type::Text),
                col("withdrawn_why", Type::Text),
                req("files", Type::Int),
                req("subjects", Type::Int),
                // What a re-run did, and mostly did not do (§8.6).
                req("unchanged", Type::Int),
                req("moved", Type::Int),
                req("rewritten", Type::Int),
                req("added", Type::Int),
                req("removed", Type::Int),
                col("error", Type::Text),
            ],
        )
        .index(&["name"]),
        // The **current state** of each stack of a dataset: one row per
        // stack, updated in place, whatever the number of versions (Wave 4a
        // §4). The history is `release_move`. "What is in the tree now" is a
        // lookup; "what did version 4 do" is a query on the log; "what was in
        // version 3" is a replay of the log backwards, exact and rare.
        //
        // Wave 3 wrote a row per stack per version and a row per file per
        // version, measured at 1.4 KB of memory and of disk per file, which is
        // tens of gigabytes at the archive's size for every version.
        //
        // The content digest covers everything that decides the file's
        // **bytes** and deliberately not where it goes: keeping the place out
        // of it is what lets a move be seen as a move rather than as a
        // rewrite, and a name is a rendering of the decided axes, none of which
        // touches a byte.
        Table::new(
            "release_stack",
            vec![
                col("id", Type::Id),
                req("dataset_id", Type::Int),
                req("stack_id", Type::Int),
                // The version that last changed this row.
                req("release_id", Type::Int),
                req("content", Type::Text),
                // Where it went. A directory in the descriptive layout, where
                // a stack owns one; a directory and a file stem in BIDS, where
                // stacks share `anat/` and are told apart by their names. The
                // two together are the stack's **place**, and a change to it
                // is a move (§8.6).
                req("dir", Type::Text),
                col("stem", Type::Text),
                // Which of §9.3's routes it took, so a tree can be asked what
                // it holds and what it left out.
                req("route", Type::Text),
                // What was written: how many files, how many bytes, and one
                // digest over the files' digests, which is what a handover
                // verifies at (Wave 4a §4.3). A converted stack's files are
                // its stem plus these extensions; a DICOM stack owns its
                // directory and its files are its instances.
                req("files", Type::Int),
                req("bytes", Type::Int),
                req("digest", Type::Text),
                col("extensions", Type::Text),
            ],
        )
        .unique(&["dataset_id", "stack_id"])
        .index(&["release_id"]),
        // Where this version means to put each stack, written as the plan is
        // made and read back joined to the state, so that the five outcomes of
        // §8.6 are a join in the database and not two maps in memory. Emptied
        // when the version closes.
        Table::new(
            "release_plan",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                req("stack_id", Type::Int),
                req("content", Type::Text),
                req("dir", Type::Text),
                col("stem", Type::Text),
                req("route", Type::Text),
                req("fallback_dir", Type::Text),
                col("fallback_stem", Type::Text),
                req("code", Type::Text),
                req("label", Type::Text),
                req("offset_days", Type::Int),
            ],
        )
        .unique(&["release_id", "stack_id"]),
        // Wave 3 §11: how a dataset physically left. The archive set is part
        // of the release record, so "what did we send them, and is it still
        // intact" is a query rather than a folder somebody remembers.
        //
        // The password is not here and never will be: it is a key, derived
        // from a named one in the store under a domain of its own.
        Table::new(
            "handover",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                // Where the archives were written, which is not where the tree
                // is: a handover leaves.
                req("root", Type::Text),
                req("strategy", Type::Text),
                req("chunk_bytes", Type::Int),
                req("level", Type::Int),
                // The key the password was derived from, by name. Knowing which
                // key opens an archive is not knowing the password.
                req("key_name", Type::Text),
                // The archiver that made it, so a set says what it needs to be
                // opened with.
                req("tool", Type::Text),
                col("par2_percent", Type::Int),
                req("actor", Type::Text),
                req("started_at", Type::Timestamp),
                col("finished_at", Type::Timestamp),
                req("archives", Type::Int),
                req("files", Type::Int),
                req("bytes", Type::Int),
                // The bytes the archives themselves take, which is what a
                // recipient has to receive.
                req("packed_bytes", Type::Int),
                col("error", Type::Text),
            ],
        )
        .index(&["release_id"]),
        Table::new(
            "handover_archive",
            vec![
                col("id", Type::Id),
                req("handover_id", Type::Int),
                req("ordinal", Type::Int),
                req("name", Type::Text),
                // Of the archive file, so a set can be checked without opening
                // any of it.
                req("digest", Type::Text),
                req("bytes", Type::Int),
                req("files", Type::Int),
                req("subjects", Type::Int),
                // Whether it was read back, and when. An archive nobody can
                // open is not an archive that was handed over.
                col("verified_at", Type::Timestamp),
                col("error", Type::Text),
            ],
        )
        .unique(&["handover_id", "ordinal"])
        .index(&["handover_id"]),
        // Which people are in which archive. One row per subject and archive,
        // not per file: a subject is the unit a handover packs, because a
        // recipient who has half a person has nothing.
        Table::new(
            "handover_subject",
            vec![
                col("id", Type::Id),
                req("archive_id", Type::Int),
                req("subject_id", Type::Int),
                req("code", Type::Text),
                req("files", Type::Int),
                req("bytes", Type::Int),
            ],
        )
        .index(&["archive_id"])
        .index(&["subject_id"]),
        // And what a version could place nowhere, with the reason. §9.3's
        // fourth route is never a silent drop.
        Table::new(
            "release_absent",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                req("stack_id", Type::Int),
                // A word for the tally: `no_suffix`, `no_task`, and the rest.
                req("kind", Type::Text),
                req("why", Type::Text),
            ],
        )
        .index(&["release_id"]),
        // And what became of each stack that was not left alone, so that "what
        // did version 4 do" is a query rather than a diff of two trees.
        Table::new(
            "release_move",
            vec![
                col("id", Type::Id),
                req("release_id", Type::Int),
                req("stack_id", Type::Int),
                // `moved`, `rewritten`, `added`, `removed`. Never `unchanged`:
                // a row per untouched stack is a row per stack.
                req("action", Type::Text),
                col("was", Type::Text),
                col("now", Type::Text),
            ],
        )
        .index(&["release_id"]),
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
                // Wave 4a §9.2 and §13.4: "the machine was right and I
                // checked" is an acknowledgement, not a decision. It has
                // its own home and its own count.
                col("accepted_by", Type::Text),
                col("accepted_at", Type::Timestamp),
                // Wave 4a §10.2: the run that asked, and for a grouped
                // question how many members it has and what groups them.
                col("job_id", Type::Int),
                col("members", Type::Int),
                col("group_key", Type::Text),
                // The decision that closed or staged it, so a commit or a
                // withdrawal finds its items by a number and not by
                // matching JSON text, which the two backends spell apart.
                col("decision_id", Type::Int),
            ],
        )
        .index(&["status", "kind"])
        .index(&["job_id"])
        .index(&["decision_id"]),
        // Wave 4a §10.2: one question about a rule is one item with n
        // members, not n items. Each member is a stack with the evidence
        // the question was raised on, and a member decided one at a time
        // says when.
        Table::new(
            "review_member",
            vec![
                col("id", Type::Id),
                req("item_id", Type::Int),
                req("stack_id", Type::Int),
                col("evidence", Type::Json),
                col("decided_at", Type::Timestamp),
            ],
        )
        .unique(&["item_id", "stack_id"])
        .index(&["stack_id"]),
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
