// SPDX-License-Identifier: AGPL-3.0-only

//! The catalog of NILS (`docs/specs/wave4b-the-ask.md`, §9): the engine's
//! only schema knowledge, in its own crate because the ask, the release, the
//! review and the federation need the same policy. It carries the grains
//! with their keys and days, the tree's edges with their standing
//! predicates, the fields per grain with their class and visibility, the
//! axes, the kinds with their precision, the diseases and courses, the
//! identifier namespaces, the cohorts, the schemes, the roles, the
//! comparability levels, the derived fields, the window presets, the
//! function table, the caps, the epoch and the ask-schema digest.
//!
//! It is built from a registry and a pack, curated from `catalog_curation`
//! (keyed by path, so a re-sync never overwrites a person's words), and
//! served per principal with the policy of §4.4 rule 15 applied inside it:
//! an identifying field has no record, a sensitive field or kind is absent
//! for a principal without the class, and nothing a caller says can widen
//! that.

use std::collections::{BTreeMap, BTreeSet};

use nils_ask::ast::Grain;
use nils_ask::validate::{
    Class, ColumnRef, DerivedInfo, FieldInfo, KindInfo, LevelSpec, Names, Scope,
};
use nils_dicom::catalogue::{self, Level as CatalogueLevel, Sensitivity};
use nils_pack::pack::{Pack, Visibility};
use nils_registry::clinical;
use nils_registry::schema::{Type, table};
use nils_registry::session::Scheme;
use nils_registry::time::now_iso;
use nils_registry::{Insert, Param, Registry, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub type Error = nils_registry::store::Error;

/// The byte budget of one page of a grain's field listing (§9), a
/// published constant set from a rendering of the real response.
pub const PAGE_BYTES: usize = 8192;

/// The caps of the bounded path (§11.5): the mechanism is ratified, the
/// numbers are provisional until the timing run of slice 9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    pub sync_timeout_ms: u64,
    pub sync_max_rows: u64,
    pub sync_max_bytes: u64,
    pub page_rows: u64,
    pub page_rows_max: u64,
    pub page_rows_mcp: u64,
    pub preview_rows: u64,
    pub options_values: u64,
    pub values_inline_rows: u64,
    pub diagnose_variants: u64,
    pub catalog_page_bytes: u64,
    pub count_on_options: bool,
    pub preview_on_options: bool,
    pub move_kinds: u64,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            sync_timeout_ms: 20_000,
            sync_max_rows: 5_000,
            sync_max_bytes: 4 * 1024 * 1024,
            page_rows: 200,
            page_rows_max: 1_000,
            page_rows_mcp: 50,
            preview_rows: 10,
            options_values: 50,
            values_inline_rows: 500,
            diagnose_variants: 24,
            catalog_page_bytes: PAGE_BYTES as u64,
            count_on_options: false,
            preview_on_options: false,
            move_kinds: 30,
        }
    }
}

/// The window presets, by convention (§5.1).
pub const PRESETS: &[(&str, i64)] = &[
    ("30 days", 30),
    ("3 months", 93),
    ("6 months", 186),
    ("1 year", 366),
    ("2 years", 732),
    ("5 years", 1830),
];

/// One field of one level, as the catalog serves it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub level: String,
    pub path: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub class: Class,
    pub dated: bool,
    /// Whether the value may leave the node in a federated answer.
    pub federated: bool,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caveats: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<String>,
    /// Whether a person curated this record.
    pub curated: bool,
    /// Where the record came from: `catalogue`, `registry`, `fingerprint`,
    /// `session`, `clinical`.
    pub provenance: &'static str,
    /// The table and column the field reads (§11).
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisRecord {
    pub name: String,
    pub multi: bool,
    pub values: Vec<AxisValueRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisValueRecord {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindRecord {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub value_type: Option<String>,
    pub unit: Option<String>,
    pub precision: String,
    pub sensitive: bool,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiseaseRecord {
    pub id: i64,
    pub name: String,
    pub courses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CohortRecord {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub members: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemeRecord {
    pub name: String,
    pub digest: String,
    pub window_days: i64,
    pub anchor: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelRecord {
    pub name: String,
    pub description: String,
    pub exact: Vec<String>,
    pub rounded: BTreeMap<String, f64>,
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedRecord {
    pub name: String,
    pub grain: Grain,
    pub params: Vec<(String, String)>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrainRecord {
    pub grain: Grain,
    pub key: String,
    pub day: Option<String>,
    pub carries: Vec<String>,
    pub reaches_release: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeRecord {
    pub from: Grain,
    pub to: Grain,
    pub cardinality: String,
    pub standing: Vec<String>,
}

/// What a person wrote about a path.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curation {
    pub description: Option<String>,
    pub caveats: Option<String>,
    pub ai_context: Option<String>,
    pub visibility: Option<String>,
    pub class: Option<String>,
}

/// The catalog, built once per epoch and pack version.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub epoch: i64,
    pub pack: String,
    pub pack_version: String,
    pub grains: Vec<GrainRecord>,
    pub edges: Vec<EdgeRecord>,
    /// Every field, by (level, path).
    pub fields: BTreeMap<(String, String), Field>,
    pub axes: Vec<AxisRecord>,
    pub kinds: Vec<KindRecord>,
    pub diseases: Vec<DiseaseRecord>,
    pub namespaces: Vec<String>,
    pub cohorts: Vec<CohortRecord>,
    pub schemes: Vec<SchemeRecord>,
    pub roles: Vec<String>,
    pub pick_models: Vec<String>,
    pub levels: Vec<LevelRecord>,
    pub derived: Vec<DerivedRecord>,
    pub selections: BTreeMap<String, u64>,
    pub handles: BTreeMap<String, Grain>,
    pub uploads: BTreeSet<String>,
    pub caps: Caps,
    pub schema_digest: String,
}

fn class_of(s: Sensitivity) -> Class {
    match s {
        Sensitivity::Identifying => Class::Identifying,
        Sensitivity::QuasiIdentifying => Class::QuasiIdentifying,
        Sensitivity::Clinical => Class::Clinical,
        Sensitivity::Technical => Class::Technical,
    }
}

fn type_name(t: Type) -> &'static str {
    match t {
        Type::Id | Type::Int => "integer",
        Type::Text => "text",
        Type::Double => "number",
        Type::Bool => "boolean",
        Type::Date => "date",
        Type::Time => "time",
        Type::Timestamp => "timestamp",
        Type::Json => "json",
        Type::Bytes => "bytes",
    }
}

/// The fixed fields a grain carries beyond the catalogue's columns: what
/// the registry, the fingerprint, the session cache and the clinical layer
/// add (§4.2).
fn fixed_fields() -> Vec<Field> {
    use Class::*;
    let f = |level: &str,
             path: &str,
             ty: &str,
             class: Class,
             dated: bool,
             federated: bool,
             provenance: &'static str,
             description: &str| {
        let (table, column): (&str, &str) = match (level, path) {
            ("cohort", _) => ("cohort", path),
            ("subject", _) => ("subject", path),
            ("session", _) => ("session_cache", path),
            ("study", _) => ("study", path),
            ("series", _) => ("series", path),
            ("stack", "id" | "stack_index" | "orientation" | "n_instances") => ("stack", path),
            ("stack", "day") => ("study", "day"),
            ("stack", _) => ("stack_fingerprint", path),
            ("instance", _) => ("instance", path),
            ("event", "kind") => ("observation_type", "name"),
            ("event", "date") => ("event", "event_date"),
            ("event", "precision") => ("event", "event_date_precision"),
            ("event", _) => ("event", path),
            _ => ("", path),
        };
        Field {
            level: level.into(),
            path: path.into(),
            type_: ty.into(),
            class,
            dated,
            federated,
            description: description.into(),
            caveats: None,
            ai_context: None,
            curated: false,
            provenance,
            table: table.into(),
            column: column.into(),
        }
    };
    vec![
        f(
            "cohort",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the cohort's key; its name is a label",
        ),
        f(
            "cohort",
            "name",
            "text",
            Technical,
            false,
            true,
            "registry",
            "the cohort's name",
        ),
        f(
            "cohort",
            "owner",
            "text",
            Technical,
            false,
            false,
            "registry",
            "who owns the cohort",
        ),
        f(
            "cohort",
            "description",
            "text",
            Technical,
            false,
            false,
            "registry",
            "what the cohort is",
        ),
        f(
            "subject",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the subject's key; the code is a label",
        ),
        f(
            "subject",
            "code",
            "text",
            QuasiIdentifying,
            false,
            false,
            "registry",
            "the pseudonym the registry knows the subject by",
        ),
        f(
            "subject",
            "deceased_at",
            "date",
            QuasiIdentifying,
            true,
            false,
            "clinical",
            "the date of death, when recorded",
        ),
        f(
            "session",
            "id",
            "integer",
            Technical,
            false,
            true,
            "session",
            "the session's surrogate key",
        ),
        f(
            "session",
            "first",
            "date",
            QuasiIdentifying,
            true,
            false,
            "session",
            "the day the session opened, its first study's day",
        ),
        f(
            "session",
            "last",
            "date",
            QuasiIdentifying,
            true,
            false,
            "session",
            "the day of the session's last study",
        ),
        f(
            "session",
            "n_studies",
            "integer",
            Technical,
            false,
            true,
            "session",
            "how many studies the session holds",
        ),
        f(
            "session",
            "label",
            "text",
            Technical,
            false,
            true,
            "session",
            "the label the document's scheme gives the session",
        ),
        f(
            "session",
            "months",
            "integer",
            Technical,
            false,
            true,
            "session",
            "the months from the scheme's anchor, when the scheme names by months",
        ),
        f(
            "session",
            "nominal",
            "integer",
            Technical,
            false,
            true,
            "session",
            "the cadence point the session was placed on",
        ),
        f(
            "session",
            "flagged",
            "boolean",
            Technical,
            false,
            true,
            "session",
            "whether the scheme flagged the session as worth a look",
        ),
        f(
            "session",
            "reason",
            "text",
            Technical,
            false,
            true,
            "session",
            "why it was flagged",
        ),
        f(
            "session",
            "has_primary",
            "boolean",
            Technical,
            false,
            true,
            "session",
            "whether any study of the session holds an original primary",
        ),
        f(
            "study",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the study's key",
        ),
        f(
            "study",
            "date_filled",
            "date",
            QuasiIdentifying,
            true,
            false,
            "registry",
            "the study's day as the vote settled it",
        ),
        f(
            "series",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the series' key",
        ),
        f(
            "series",
            "n_instances",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "how many instances the series holds",
        ),
        f(
            "series",
            "n_stacks",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "how many stacks the series split into",
        ),
        f(
            "stack",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the stack's key",
        ),
        f(
            "stack",
            "stack_index",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the stack's index in its series",
        ),
        f(
            "stack",
            "orientation",
            "text",
            Technical,
            false,
            true,
            "registry",
            "the orientation class: Axial, Sagittal, Coronal",
        ),
        f(
            "stack",
            "n_instances",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "how many instances the stack holds",
        ),
        f(
            "stack",
            "day",
            "date",
            QuasiIdentifying,
            true,
            false,
            "registry",
            "the stack's day: its study's date as filled, else as read",
        ),
        f(
            "stack",
            "magnetic_field_strength",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the field strength as read, in the units the file carried",
        ),
        f(
            "stack",
            "field_strength_tesla",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the field strength worked out in tesla",
        ),
        f(
            "stack",
            "repetition_time",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "TR in milliseconds",
        ),
        f(
            "stack",
            "echo_time",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "TE in milliseconds",
        ),
        f(
            "stack",
            "inversion_time",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "TI in milliseconds",
        ),
        f(
            "stack",
            "flip_angle",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the flip angle in degrees",
        ),
        f(
            "stack",
            "echo_train_length",
            "integer",
            Technical,
            false,
            true,
            "fingerprint",
            "the echo train length",
        ),
        f(
            "stack",
            "slice_thickness",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the slice thickness in millimetres",
        ),
        f(
            "stack",
            "spacing_between_slices",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the spacing between slices in millimetres",
        ),
        f(
            "stack",
            "pixel_spacing_row",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the row spacing in millimetres",
        ),
        f(
            "stack",
            "pixel_spacing_col",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the column spacing in millimetres",
        ),
        f(
            "stack",
            "rows",
            "integer",
            Technical,
            false,
            true,
            "fingerprint",
            "the image rows",
        ),
        f(
            "stack",
            "columns",
            "integer",
            Technical,
            false,
            true,
            "fingerprint",
            "the image columns",
        ),
        f(
            "stack",
            "fov_x",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the field of view along the columns, in millimetres",
        ),
        f(
            "stack",
            "fov_y",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the field of view along the rows, in millimetres",
        ),
        f(
            "stack",
            "number_of_averages",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the number of averages",
        ),
        f(
            "stack",
            "mr_acquisition_type",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "2D or 3D as read",
        ),
        f(
            "stack",
            "acquisition_type_filled",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "2D or 3D as worked out",
        ),
        f(
            "stack",
            "image_role",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "original, derived, or another role of the image type",
        ),
        f(
            "stack",
            "dwi_b_value",
            "number",
            Technical,
            false,
            true,
            "fingerprint",
            "the diffusion shell",
        ),
        f(
            "stack",
            "dwi_directions",
            "integer",
            Technical,
            false,
            true,
            "fingerprint",
            "the diffusion gradient count",
        ),
        f(
            "stack",
            "manufacturer",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "the scanner's maker",
        ),
        f(
            "stack",
            "manufacturer_model_name",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "the scanner's model",
        ),
        f(
            "stack",
            "station_name",
            "text",
            QuasiIdentifying,
            false,
            false,
            "fingerprint",
            "the scanner's station name",
        ),
        f(
            "stack",
            "text_series_description",
            "text",
            QuasiIdentifying,
            false,
            false,
            "fingerprint",
            "the series description, folded",
        ),
        f(
            "stack",
            "text_protocol_name",
            "text",
            QuasiIdentifying,
            false,
            false,
            "fingerprint",
            "the protocol name, folded",
        ),
        f(
            "stack",
            "text_sequence_name",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "the sequence name, folded",
        ),
        f(
            "stack",
            "text_body_part",
            "text",
            Technical,
            false,
            true,
            "fingerprint",
            "the body part examined, folded",
        ),
        f(
            "stack",
            "text_all",
            "text",
            QuasiIdentifying,
            false,
            false,
            "fingerprint",
            "every text field the pack reads, joined",
        ),
        f(
            "instance",
            "id",
            "integer",
            Technical,
            false,
            true,
            "registry",
            "the instance's key",
        ),
        f(
            "event",
            "id",
            "integer",
            Technical,
            false,
            true,
            "clinical",
            "the event's key",
        ),
        f(
            "event",
            "kind",
            "text",
            Clinical,
            false,
            true,
            "clinical",
            "the observation kind, by its name in the vocabulary",
        ),
        f(
            "event",
            "date",
            "date",
            QuasiIdentifying,
            true,
            false,
            "clinical",
            "the event's day, at its precision",
        ),
        f(
            "event",
            "precision",
            "text",
            Technical,
            false,
            true,
            "clinical",
            "day, month or year: how finely the date is known",
        ),
        f(
            "event",
            "value",
            "text",
            Clinical,
            false,
            true,
            "clinical",
            "the value as text",
        ),
        f(
            "event",
            "number",
            "number",
            Clinical,
            false,
            true,
            "clinical",
            "the value as a number, for a numeric kind",
        ),
        f(
            "event",
            "unit",
            "text",
            Clinical,
            false,
            true,
            "clinical",
            "the value's unit",
        ),
        f(
            "event",
            "source",
            "text",
            Technical,
            false,
            true,
            "clinical",
            "where the row came from",
        ),
        f(
            "event",
            "quality",
            "text",
            Technical,
            false,
            true,
            "clinical",
            "the row's quality note",
        ),
    ]
}

/// The derived fields (§4.3), with their parameters and defaults.
fn derived_fields() -> Vec<DerivedRecord> {
    let d = |name: &str, grain: Grain, params: &[(&str, &str)], description: &str| DerivedRecord {
        name: name.into(),
        grain,
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        description: description.into(),
    };
    vec![
        d(
            "acquisition_type",
            Grain::Stack,
            &[],
            "2D or 3D, as read or as worked out",
        ),
        d(
            "field_strength",
            Grain::Stack,
            &[],
            "the field strength in tesla",
        ),
        d("study_day", Grain::Stack, &[], "the stack's study day"),
        d(
            "voxel",
            Grain::Stack,
            &[("third", "slice_thickness")],
            "the largest voxel edge; third names which column stands for the third dimension: slice_thickness or spacing_between_slices",
        ),
        d("voxel_min", Grain::Stack, &[], "the smallest voxel edge"),
        d("voxel_max", Grain::Stack, &[], "the largest voxel edge"),
        d(
            "resolution",
            Grain::Stack,
            &[],
            "the three voxel edges as text, row by column by slice",
        ),
        d(
            "signature",
            Grain::Stack,
            &[("level", "loose")],
            "the acquisition's tuple at a comparability level of the pack (section 6)",
        ),
        d(
            "course",
            Grain::Subject,
            &[("disease", "")],
            "the subject's current course of a disease",
        ),
    ]
}

fn grains() -> Vec<GrainRecord> {
    let g = |grain: Grain, key: &str, day: Option<&str>, carries: &[&str]| GrainRecord {
        grain,
        key: key.into(),
        day: day.map(str::to_string),
        carries: carries.iter().map(|c| c.to_string()).collect(),
        reaches_release: nils_ask::validate::reaches_release(grain),
    };
    vec![
        g(Grain::Cohort, "cohort.id", None, &[]),
        g(Grain::Subject, "subject.id", None, &["cohort.id"]),
        g(
            Grain::Session,
            "session_cache.id",
            Some("first"),
            &["subject"],
        ),
        g(
            Grain::Stack,
            "stack.id",
            Some("COALESCE(date_filled, study_date)"),
            &["session", "study", "series", "subject"],
        ),
        g(
            Grain::Instance,
            "instance.id",
            Some("study day"),
            &["stack", "session", "subject"],
        ),
        g(
            Grain::Event,
            "event.id",
            Some("event_date, with its precision"),
            &["subject"],
        ),
        g(
            Grain::Group,
            "the by-tuple",
            None,
            &["every ancestor key named in by"],
        ),
    ]
}

fn edges() -> Vec<EdgeRecord> {
    let e = |from: Grain, to: Grain, cardinality: &str, standing: &[&str]| EdgeRecord {
        from,
        to,
        cardinality: cardinality.into(),
        standing: standing.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        e(
            Grain::Cohort,
            Grain::Subject,
            "many to many",
            &["cohort_member.left_at IS NULL (the open interval)"],
        ),
        e(Grain::Subject, Grain::Session, "one to many", &[]),
        e(
            Grain::Session,
            Grain::Stack,
            "one to many, under the document's scheme",
            &["disposition != excluded"],
        ),
        e(Grain::Stack, Grain::Instance, "one to many", &[]),
        e(
            Grain::Subject,
            Grain::Event,
            "one to many",
            &[
                "superseded_by IS NULL",
                "a sensitive kind is absent without the class",
            ],
        ),
    ]
}

impl Catalog {
    /// Build the catalog from a registry and its pack.
    pub fn build(registry: &mut Registry, pack: &Pack) -> Result<Catalog, Error> {
        let epoch = registry.meta().epoch;
        let store = registry.store();
        let mut fields: BTreeMap<(String, String), Field> = BTreeMap::new();

        // the catalogue's own columns, per level, with the pack's visibility
        for level in CatalogueLevel::ALL {
            let name = match level {
                CatalogueLevel::SeriesMr | CatalogueLevel::SeriesCt | CatalogueLevel::SeriesPet => {
                    "series"
                }
                other => other.name(),
            };
            let table = match level {
                CatalogueLevel::SeriesMr => "series_mr",
                CatalogueLevel::SeriesCt => "series_ct",
                CatalogueLevel::SeriesPet => "series_pet",
                other => other.name(),
            };
            for (_, f) in catalogue::fields_of(level) {
                let class = class_of(f.class);
                if class == Class::Identifying {
                    continue;
                }
                let key = (name.to_string(), f.column.to_string());
                fields.entry(key).or_insert(Field {
                    level: name.into(),
                    path: f.column.into(),
                    type_: type_name(Type::of(f.converter)).into(),
                    class,
                    dated: matches!(Type::of(f.converter), Type::Date),
                    federated: class == Class::Technical,
                    description: f.note.to_string(),
                    caveats: None,
                    ai_context: None,
                    curated: false,
                    provenance: "catalogue",
                    table: table.into(),
                    column: f.column.into(),
                });
            }
        }
        for f in fixed_fields() {
            fields.insert((f.level.clone(), f.path.clone()), f);
        }
        // the pack's visibility, by column, on every level that carries it
        for ((_, path), f) in fields.iter_mut() {
            if let Some(v) = pack.fields.get(path) {
                match v {
                    Visibility::Local => f.federated = false,
                    Visibility::Federated => f.federated = true,
                    Visibility::Sensitive => {
                        f.class = Class::Sensitive;
                        f.federated = false;
                    }
                }
            }
        }
        // curation, keyed by path
        for r in store.query(
            &format!(
                "SELECT path, description, caveats, ai_context, visibility, class FROM {} ORDER BY path",
                store.qualified("catalog_curation")
            ),
            &[],
        )? {
            let path = r.text(0)?.to_string();
            let Some((level, column)) = path.split_once('.') else {
                continue;
            };
            let Some(f) = fields.get_mut(&(level.to_string(), column.to_string())) else {
                continue;
            };
            if let Some(d) = r.opt_text(1)? {
                f.description = d.to_string();
            }
            f.caveats = r.opt_text(2)?.map(str::to_string);
            f.ai_context = r.opt_text(3)?.map(str::to_string);
            match r.opt_text(4)? {
                Some("local") => f.federated = false,
                Some("federated") => f.federated = true,
                Some("sensitive") => f.class = Class::Sensitive,
                _ => {}
            }
            match r.opt_text(5)? {
                Some("technical") => f.class = Class::Technical,
                Some("quasi_identifying") | Some("quasi-identifying") => f.class = Class::QuasiIdentifying,
                Some("clinical") => f.class = Class::Clinical,
                Some("sensitive") => f.class = Class::Sensitive,
                _ => {}
            }
            f.curated = true;
        }

        let axes = pack
            .axes
            .iter()
            .map(|a| AxisRecord {
                name: a.name.clone(),
                multi: a.multi,
                values: a
                    .values
                    .iter()
                    .map(|v| AxisValueRecord {
                        id: v.id.clone(),
                        label: v.label.clone(),
                    })
                    .collect(),
            })
            .collect();
        let kinds = clinical::observation_types(store)?
            .into_iter()
            .map(|k| KindRecord {
                id: k.id,
                name: k.name,
                category: k.category,
                value_type: k.value_type,
                unit: k.unit,
                precision: k.precision,
                sensitive: k.sensitive,
                primary: k.primary,
            })
            .collect();
        let mut diseases: Vec<DiseaseRecord> = Vec::new();
        for r in store.query(
            &format!(
                "SELECT id, name FROM {} ORDER BY name",
                store.qualified("disease")
            ),
            &[],
        )? {
            diseases.push(DiseaseRecord {
                id: r.int(0)?,
                name: r.text(1)?.to_string(),
                courses: Vec::new(),
            });
        }
        for r in store.query(
            &format!(
                "SELECT disease_id, name FROM {} ORDER BY disease_id, sort_order, name",
                store.qualified("disease_type")
            ),
            &[],
        )? {
            let id = r.int(0)?;
            if let Some(d) = diseases.iter_mut().find(|d| d.id == id) {
                d.courses.push(r.text(1)?.to_string());
            }
        }
        let namespaces = nils_registry::schema::ID_TYPES
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        let mut cohorts = Vec::new();
        for r in store.query(
            &format!(
                "SELECT c.id, c.name, c.owner, \
                 (SELECT COUNT(*) FROM {} m WHERE m.cohort_id = c.id AND m.left_at IS NULL) \
                 FROM {} c ORDER BY c.name",
                store.qualified("cohort_member"),
                store.qualified("cohort")
            ),
            &[],
        )? {
            cohorts.push(CohortRecord {
                id: r.int(0)?,
                name: r.text(1)?.to_string(),
                owner: r.text(2)?.to_string(),
                members: r.int(3)?,
            });
        }
        let mut schemes = Vec::new();
        let def = store.dialect().text_of(
            table("session_scheme")
                .column("definition")
                .expect("definition"),
        );
        for r in store.query(
            &format!(
                "SELECT name, digest, {def} FROM {} ORDER BY name",
                store.qualified("session_scheme")
            ),
            &[],
        )? {
            let scheme = Scheme::from_json(r.text(2)?).ok();
            schemes.push(SchemeRecord {
                name: r.text(0)?.to_string(),
                digest: r
                    .opt_text(1)?
                    .map(str::to_string)
                    .or_else(|| scheme.as_ref().map(Scheme::digest))
                    .unwrap_or_default(),
                window_days: scheme.as_ref().map_or(0, |s| s.window_days),
                anchor: scheme
                    .as_ref()
                    .map(|s| format!("{:?}", s.anchor).to_lowercase())
                    .unwrap_or_default(),
            });
        }
        let default = Scheme::default();
        schemes.insert(
            0,
            SchemeRecord {
                name: "default".into(),
                digest: default.digest(),
                window_days: default.window_days,
                anchor: "first_session".into(),
            },
        );
        let mut roles: Vec<String> = Vec::new();
        let mut pick_models = Vec::new();
        for m in &pack.picks {
            pick_models.push(m.name.clone());
            for r in &m.roles {
                if !roles.contains(r) {
                    roles.push(r.clone());
                }
            }
        }
        let levels = pack
            .levels
            .iter()
            .map(|l| LevelRecord {
                name: l.name.clone(),
                description: l.description.clone(),
                exact: l.exact.clone(),
                rounded: l.rounded.clone(),
                ignored: l.ignored.clone(),
            })
            .collect();
        let mut selections = BTreeMap::new();
        for r in store.query(
            &format!(
                "SELECT name, current_version FROM {}",
                store.qualified("selection")
            ),
            &[],
        )? {
            selections.insert(r.text(0)?.to_string(), r.int(1)? as u64);
        }
        let mut handles = BTreeMap::new();
        for r in store.query(
            &format!(
                "SELECT id, grain FROM {} WHERE withdrawn_at IS NULL",
                store.qualified("handle")
            ),
            &[],
        )? {
            let grain: Grain = serde_json::from_value(Value::String(r.text(1)?.to_string()))
                .unwrap_or(Grain::Subject);
            handles.insert(r.int(0)?.to_string(), grain);
        }
        let mut uploads = BTreeSet::new();
        for r in store.query(
            &format!("SELECT upload_id FROM {}", store.qualified("values_source")),
            &[],
        )? {
            uploads.insert(r.text(0)?.to_string());
        }
        Ok(Catalog {
            epoch,
            pack: pack.name.clone(),
            pack_version: pack.version.to_string(),
            grains: grains(),
            edges: edges(),
            fields,
            axes,
            kinds,
            diseases,
            namespaces,
            cohorts,
            schemes,
            roles,
            pick_models,
            levels,
            derived: derived_fields(),
            selections,
            handles,
            uploads,
            caps: Caps::default(),
            schema_digest: nils_ask::schema::digest(),
        })
    }

    /// Whether a principal may see a field at all (rule 15): never an
    /// identifier, and a sensitive one only with the class.
    pub fn visible(&self, f: &Field, scope: &Scope) -> bool {
        match f.class {
            Class::Identifying => false,
            Class::Sensitive => scope.classes.contains(&Class::Sensitive),
            _ => true,
        }
    }

    /// Whether a principal may project a field's raw value (rule 15): a
    /// quasi-identifying field needs the class; a technical or clinical one
    /// does not.
    pub fn may_project_raw(&self, f: &Field, scope: &Scope) -> bool {
        self.visible(f, scope)
            && match f.class {
                Class::QuasiIdentifying => scope.classes.contains(&Class::QuasiIdentifying),
                _ => true,
            }
    }

    /// The fields of one level a principal may see, sorted by path.
    pub fn fields_of(&self, level: &str, scope: &Scope) -> Vec<&Field> {
        self.fields
            .iter()
            .filter(|((l, _), f)| l == level && self.visible(f, scope))
            .map(|(_, f)| f)
            .collect()
    }

    /// The kinds a principal may see.
    pub fn kinds_for(&self, scope: &Scope) -> Vec<&KindRecord> {
        self.kinds
            .iter()
            .filter(|k| !k.sensitive || scope.classes.contains(&Class::Sensitive))
            .collect()
    }

    /// One page of a level's field listing, inside the byte budget, with
    /// the cursor of the next page when there is one.
    pub fn page(&self, level: &str, scope: &Scope, after: Option<&str>, budget: usize) -> Page {
        let all = self.fields_of(level, scope);
        let start = match after {
            Some(a) => all
                .iter()
                .position(|f| f.path.as_str() > a)
                .unwrap_or(all.len()),
            None => 0,
        };
        let mut out: Vec<Value> = Vec::new();
        let mut bytes = 64;
        let mut next = None;
        for f in &all[start..] {
            let v = serde_json::to_value(f).expect("a field serializes");
            let n = v.to_string().len() + 1;
            if !out.is_empty() && bytes + n > budget {
                next = Some(
                    out.last()
                        .and_then(|l| l["path"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                );
                break;
            }
            bytes += n;
            out.push(v);
        }
        Page {
            level: level.to_string(),
            fields: out,
            next,
            total: all.len(),
        }
    }

    /// The whole catalog for a principal, the field listing of every level
    /// paged by `page`.
    pub fn document(&self, scope: &Scope) -> Value {
        let levels: Vec<Value> = nils_ask::validate::levels()
            .iter()
            .map(|l| {
                let page = self.page(l, scope, None, PAGE_BYTES);
                json!({ "level": l, "fields": page.fields, "next": page.next, "total": page.total })
            })
            .collect();
        json!({
            "epoch": self.epoch,
            "pack": { "name": self.pack, "version": self.pack_version },
            "schema_digest": self.schema_digest,
            "caps": self.caps,
            "grains": self.grains,
            "edges": self.edges,
            "levels": levels,
            "axes": self.axes,
            "kinds": self.kinds_for(scope),
            "diseases": self.diseases,
            "namespaces": self.namespaces,
            "cohorts": self.cohorts,
            "schemes": self.schemes,
            "roles": self.roles,
            "pick_models": self.pick_models,
            "comparability_levels": self.levels,
            "derived": self.derived,
            "presets": PRESETS.iter().map(|(n, d)| json!({ "name": n, "days": d })).collect::<Vec<_>>(),
            "functions": functions(),
            "selections": self.selections,
        })
    }
}

/// One page of a level's fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub level: String,
    pub fields: Vec<Value>,
    pub next: Option<String>,
    pub total: usize,
}

/// The function table (§4.3), by family.
pub fn functions() -> Value {
    json!({
        "refs": ["field", "axis", "derived", "param"],
        "arithmetic": ["+", "-", "*", "/", "abs", "round", "coalesce", "case", "concat"],
        "comparison": ["=", "<>", ">", ">=", "<", "<=", "~=", "in", "not_in", "has", "not_null", "is_null", "contains", "starts_with", "and", "or", "not"],
        "time": ["days_between", "shift", "age_at", "bucket", "part"],
        "sequence": ["ordinal", "prev", "next", "change"],
        "aggregate": ["count", "distinct", "min", "max", "sum", "avg", "list", "share"],
    })
}

impl Names for Catalog {
    fn field(&self, level: &str, path: &str) -> Option<FieldInfo> {
        self.fields
            .get(&(level.to_string(), path.to_string()))
            .map(|f| FieldInfo {
                class: f.class,
                dated: f.dated,
                federated: f.federated,
            })
    }

    fn axis_values(&self, axis: &str) -> Option<Vec<String>> {
        self.axes
            .iter()
            .find(|a| a.name == axis)
            .map(|a| a.values.iter().map(|v| v.id.clone()).collect())
    }

    fn kind(&self, name: &str) -> Option<KindInfo> {
        self.kinds
            .iter()
            .find(|k| k.name.eq_ignore_ascii_case(name))
            .map(|k| KindInfo {
                precision: k.precision.clone(),
                sensitive: k.sensitive,
            })
    }

    fn level(&self, name: &str) -> bool {
        self.levels.iter().any(|l| l.name == name)
    }

    fn role(&self, name: &str) -> Option<Grain> {
        self.roles.iter().any(|r| r == name).then_some(Grain::Stack)
    }

    fn scheme(&self, name: &str) -> Option<String> {
        self.schemes
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.digest.clone())
    }

    fn cohort(&self, name: &str) -> bool {
        self.cohorts.iter().any(|c| c.name == name)
    }

    fn selection(&self, name: &str) -> Option<u64> {
        self.selections.get(name).copied()
    }

    fn handle(&self, id: &str) -> Option<Grain> {
        self.handles.get(id).copied()
    }

    fn upload(&self, id: &str) -> bool {
        self.uploads.contains(id)
    }

    fn derived(&self, name: &str) -> Option<DerivedInfo> {
        self.derived
            .iter()
            .find(|d| d.name == name)
            .map(|d| DerivedInfo {
                grain: d.grain,
                params: d.params.iter().map(|(k, _)| k.clone()).collect(),
            })
    }

    fn level_spec(&self, name: &str) -> Option<LevelSpec> {
        self.levels
            .iter()
            .find(|l| l.name == name)
            .map(|l| LevelSpec {
                exact: l.exact.clone(),
                rounded: l.rounded.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            })
    }

    fn column(&self, level: &str, path: &str) -> Option<ColumnRef> {
        let f = self.fields.get(&(level.to_string(), path.to_string()))?;
        if f.table.is_empty() {
            return None;
        }
        let ci = (f.table == "stack_fingerprint" && f.column.starts_with("text_"))
            .then(|| format!("{}_ci", f.column));
        Some(ColumnRef {
            table: f.table.clone(),
            column: f.column.clone(),
            ci,
        })
    }
}

/// Write what a person says about a path; a re-sync never overwrites it.
pub fn curate(store: &mut Store, path: &str, c: &Curation, actor: &str) -> Result<(), Error> {
    let now = now_iso();
    let d = store.dialect();
    let existing = store.query_opt(
        &format!(
            "SELECT id FROM {} WHERE path = {}",
            store.qualified("catalog_curation"),
            d.param(1, Type::Text)
        ),
        &[Param::from(path)],
    )?;
    let opt = |v: &Option<String>| v.as_deref().map_or(Param::Null, Param::from);
    match existing {
        Some(r) => {
            store.update_by_id(
                table("catalog_curation"),
                &[
                    ("description", opt(&c.description)),
                    ("caveats", opt(&c.caveats)),
                    ("ai_context", opt(&c.ai_context)),
                    ("visibility", opt(&c.visibility)),
                    ("class", opt(&c.class)),
                    ("updated_at", Param::from(now.as_str())),
                    ("actor", Param::from(actor)),
                ],
                "id",
                r.int(0)?,
            )?;
        }
        None => {
            store.insert(
                &Insert::new(
                    table("catalog_curation"),
                    &[
                        "path",
                        "description",
                        "caveats",
                        "ai_context",
                        "visibility",
                        "class",
                        "updated_at",
                        "actor",
                    ],
                ),
                &[vec![
                    Param::from(path),
                    opt(&c.description),
                    opt(&c.caveats),
                    opt(&c.ai_context),
                    opt(&c.visibility),
                    opt(&c.class),
                    Param::from(now.as_str()),
                    Param::from(actor),
                ]],
            )?;
        }
    }
    Ok(())
}
