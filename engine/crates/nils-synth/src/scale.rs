// SPDX-License-Identifier: AGPL-3.0-only

//! The scale registry: the shape of a registry a site of some years holds,
//! made up by design and deterministic from a seed, so that a question is
//! timed on the row counts it meets in production and its answer compared
//! between two builds of the compiler. Where the synthetic registry of
//! [`crate::build`] plants cases a fixture's rows are known by, this one
//! plants proportions: at the default size some 5,900 subjects, 34,000
//! studies on 30,000 subject-days, 360,000 series and 490,000 stacks, each
//! with its fingerprint and about ten axis rows over the fifteen axes of
//! the MR pack, a cohort holding three subjects in four (those with many
//! visits, so nearly every stack), four datasets of which two hold a few
//! hundred stacks, one of them copies of another's, some five hundred
//! cohorts of one subject, and two stacks in a thousand the pack ruled out.
//!
//! The spreads are the ones a plan depends on: one stack in four is 3D and
//! most of those are at 1.2 mm or finer; a scanner keeps its protocol's
//! resolution, so a subject scanned twice on one scanner has two visits of
//! one signature; an axis of several values holds none, one or two; one row
//! in a hundred holds a value's identity where the pack stores its label,
//! as a person's decision may; and a few axes are decided to nothing, a row
//! with no value. Every name is made up: the cohort is `alpha`, the
//! datasets are `site-a` to `site-d`.

use std::collections::HashMap;

use nils_registry::day::Day;
use nils_registry::place::{self, New, Role};
use nils_registry::schema::table;
use nils_registry::{Error, Insert, Param, Registry, Store};
use serde::{Deserialize, Serialize};

use crate::{Rng, day, iso, one_id, shift, text};

/// What to build: the seed and the number of subjects, which every other
/// count follows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScalePlan {
    pub seed: u64,
    pub subjects: usize,
}

impl Default for ScalePlan {
    fn default() -> ScalePlan {
        ScalePlan {
            seed: 7,
            subjects: 5_900,
        }
    }
}

/// What was built.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ScaleManifest {
    pub seed: u64,
    pub subjects: usize,
    pub cohort_members: usize,
    pub cohorts: usize,
    pub studies: usize,
    pub series: usize,
    pub stacks: usize,
    pub axis_rows: usize,
    pub excluded: usize,
    pub held: usize,
    pub datasets: Vec<(String, usize)>,
}

/// The cohort three subjects in four are members of.
pub const COHORT: &str = "alpha";
/// The datasets: the first holds the members' stacks, the second most of
/// the others', the third a few subjects' and the fourth copies of stacks
/// of the first.
pub const DATASETS: [&str; 4] = ["site-a", "site-b", "site-c", "site-d"];

/// One protocol: what a series of it is named, what the pack would say of
/// its stacks, and the geometry a scanner may run it at.
struct Proto {
    name: &'static str,
    weight: u64,
    /// The base as the pack stores it, and its identity where that differs.
    base: Option<(&'static str, &'static str)>,
    /// The technique, likewise.
    technique: Option<(&'static str, &'static str)>,
    modifiers: &'static [&'static str],
    /// The constructs of a series' stacks, one list per stack in turn.
    constructs: &'static [&'static [&'static str]],
    three_d: bool,
    in_plane: &'static [f64],
    /// Slice thickness; none for an isotropic 3D protocol.
    thickness: &'static [f64],
    orientation: &'static str,
    body_part: (&'static str, &'static str),
    provenance: &'static str,
    disposition: &'static str,
    directory: &'static str,
    role: Option<&'static str>,
    /// Most stacks a series of it holds.
    stacks: u64,
    /// In a hundred series, how many are after contrast.
    contrast: u64,
}

const BRAIN: (&str, &str) = ("brain", "head");
const SPINE: (&str, &str) = ("spine", "spine");
const NONE: &[&[&str]] = &[&[]];

#[rustfmt::skip]
const PROTOCOLS: &[Proto] = &[
    Proto { name: "t1_mprage_sag", weight: 9, base: Some(("T1w", "T1w")), technique: Some(("MPRAGE", "MPRAGE")), modifiers: &[], constructs: NONE, three_d: true, in_plane: &[1.0, 1.0, 0.9, 0.8, 0.5, 1.0], thickness: &[], orientation: "sagittal", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: Some("t1w"), stacks: 1, contrast: 0 },
    Proto { name: "t1_mprage_sag_mpr_tra", weight: 5, base: Some(("T1w", "T1w")), technique: Some(("MPRAGE", "MPRAGE")), modifiers: &[], constructs: &[&["MPR"]], three_d: true, in_plane: &[1.0, 0.9, 1.0, 0.5], thickness: &[1.0, 1.0, 3.0, 1.5], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "reformat", directory: "anat", role: None, stacks: 2, contrast: 0 },
    Proto { name: "t1_mprage_sag_ce", weight: 3, base: Some(("T1w", "T1w")), technique: Some(("MPRAGE", "MPRAGE")), modifiers: &[], constructs: NONE, three_d: true, in_plane: &[1.0, 0.9, 1.0], thickness: &[], orientation: "sagittal", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 100 },
    Proto { name: "t2_space_flair_sag", weight: 7, base: Some(("T2w", "T2w")), technique: Some(("SPACE", "3D-TSE")), modifiers: &["FLAIR"], constructs: NONE, three_d: true, in_plane: &[1.0, 1.0, 0.5, 1.1, 1.0], thickness: &[], orientation: "sagittal", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: Some("flair"), stacks: 1, contrast: 5 },
    Proto { name: "t2_space_flair_sag_fs", weight: 2, base: Some(("T2w", "T2w")), technique: Some(("SPACE", "3D-TSE")), modifiers: &["FLAIR", "FatSat"], constructs: NONE, three_d: true, in_plane: &[1.0, 0.9], thickness: &[], orientation: "sagittal", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: Some("flair"), stacks: 1, contrast: 0 },
    Proto { name: "t2_tirm_tra_dark-fluid", weight: 7, base: Some(("T2w", "T2w")), technique: Some(("TIRM", "IR-TSE")), modifiers: &["FLAIR"], constructs: NONE, three_d: false, in_plane: &[0.7, 0.9, 0.8, 0.45], thickness: &[4.0, 5.0, 3.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: Some("flair"), stacks: 1, contrast: 0 },
    Proto { name: "t2_tse_tra", weight: 10, base: Some(("T2w", "T2w")), technique: Some(("TSE", "TSE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.45, 0.7, 0.5, 0.9], thickness: &[4.0, 5.0, 3.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: Some("t2w"), stacks: 1, contrast: 0 },
    Proto { name: "pd_t2_tse_tra", weight: 4, base: Some(("PDw", "PDw")), technique: Some(("TSE", "TSE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.9, 0.7], thickness: &[5.0, 3.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 2, contrast: 0 },
    Proto { name: "ep2d_diff_3scan_trace", weight: 9, base: Some(("DWI", "DWI")), technique: Some(("DWI-EPI", "DWI-EPI")), modifiers: &[], constructs: &[&[], &["Trace"], &["ADC"], &["FA"]], three_d: false, in_plane: &[1.2, 1.8, 2.0, 1.4], thickness: &[4.0, 5.0], orientation: "axial", body_part: BRAIN, provenance: "DTIRecon", disposition: "scanner_derived", directory: "dwi", role: None, stacks: 4, contrast: 0 },
    Proto { name: "t2_swi3d_tra", weight: 5, base: Some(("SWI", "SWI")), technique: Some(("GRE", "GRE")), modifiers: &[], constructs: &[&[], &["MinIP", "SWI"], &["Phase"]], three_d: true, in_plane: &[0.9, 0.7, 1.0], thickness: &[1.5, 2.0, 1.2], orientation: "axial", body_part: BRAIN, provenance: "SWIRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 3, contrast: 0 },
    Proto { name: "t1_se_tra", weight: 8, base: Some(("T1w", "T1w")), technique: Some(("SE", "SE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.9, 0.7, 0.45], thickness: &[4.0, 5.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 45 },
    Proto { name: "localizer", weight: 9, base: None, technique: Some(("GRE", "GRE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[1.1, 2.0, 1.5], thickness: &[7.0, 8.0], orientation: "mixed", body_part: BRAIN, provenance: "Localizer", disposition: "scout", directory: "localizer", role: None, stacks: 3, contrast: 0 },
    Proto { name: "t2_space_dir_sag", weight: 1, base: Some(("T2w", "T2w")), technique: Some(("SPACE", "3D-TSE")), modifiers: &["DIR"], constructs: NONE, three_d: true, in_plane: &[1.0, 1.2, 1.3], thickness: &[], orientation: "sagittal", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 0 },
    Proto { name: "t2star_fl2d_tra", weight: 4, base: Some(("T2*w", "T2starw")), technique: Some(("FLASH", "SP-GRE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.9, 0.7], thickness: &[5.0, 4.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 0 },
    Proto { name: "ep2d_bold_rest", weight: 2, base: None, technique: Some(("BOLD", "BOLD-EPI")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[3.0, 2.5, 2.0], thickness: &[3.0, 2.5], orientation: "axial", body_part: BRAIN, provenance: "BOLDRecon", disposition: "acquisition", directory: "func", role: None, stacks: 1, contrast: 0 },
    Proto { name: "pcasl_3d", weight: 1, base: Some(("PWI", "PWI")), technique: Some(("ASL", "ASL")), modifiers: &[], constructs: &[&["CBF"], &["M0"]], three_d: true, in_plane: &[1.9, 3.0], thickness: &[4.0], orientation: "axial", body_part: BRAIN, provenance: "ASLRecon", disposition: "scanner_derived", directory: "perf", role: None, stacks: 2, contrast: 0 },
    Proto { name: "tof_fl3d_tra", weight: 3, base: None, technique: Some(("TOF", "TOF-MRA")), modifiers: &[], constructs: &[&[], &["MIP"]], three_d: true, in_plane: &[0.3, 0.5, 0.6], thickness: &[0.6, 0.7, 0.5], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 2, contrast: 0 },
    Proto { name: "t1_vibe_dixon_tra", weight: 2, base: Some(("T1w", "T1w")), technique: Some(("VIBE", "VI-GRE")), modifiers: &["Dixon"], constructs: &[&["Water"], &["Fat"], &["InPhase"], &["OutPhase"]], three_d: true, in_plane: &[1.2, 1.4, 1.0], thickness: &[1.5, 3.0, 1.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 4, contrast: 30 },
    Proto { name: "t1_tir_psir_tra", weight: 1, base: Some(("T1w", "T1w")), technique: Some(("TIRM", "IR-TSE")), modifiers: &["IR", "PSIR"], constructs: &[&["Magnitude"], &["Real"]], three_d: false, in_plane: &[0.8, 0.7], thickness: &[3.0, 4.0], orientation: "axial", body_part: BRAIN, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 2, contrast: 0 },
    Proto { name: "t2_tse_sag_spine", weight: 4, base: Some(("T2w", "T2w")), technique: Some(("TSE", "TSE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.8, 0.9, 0.7], thickness: &[3.0, 4.0], orientation: "sagittal", body_part: SPINE, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 0 },
    Proto { name: "t2_stir_sag_spine", weight: 2, base: Some(("T2w", "T2w")), technique: Some(("TIRM", "IR-TSE")), modifiers: &["STIR"], constructs: NONE, three_d: false, in_plane: &[0.9, 0.8], thickness: &[3.0], orientation: "sagittal", body_part: SPINE, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 0 },
    Proto { name: "t1_tse_sag_spine", weight: 2, base: Some(("T1w", "T1w")), technique: Some(("TSE", "TSE")), modifiers: &[], constructs: NONE, three_d: false, in_plane: &[0.8, 0.9], thickness: &[3.0, 4.0], orientation: "sagittal", body_part: SPINE, provenance: "RawRecon", disposition: "acquisition", directory: "anat", role: None, stacks: 1, contrast: 35 },
];

struct Station {
    name: String,
    model: &'static str,
    maker: &'static str,
    field: f64,
}

const MODELS: &[(&str, &str, f64)] = &[
    ("Model A15", "MAKER-A", 1.5),
    ("Model A30", "MAKER-A", 3.0),
    ("Model A30x", "MAKER-A", 2.89),
    ("Model B15", "MAKER-B", 1.5),
    ("Model B30", "MAKER-B", 3.0),
    ("Model C15", "MAKER-C", 1.5),
    ("Model C30", "MAKER-C", 3.0),
    ("Model A70", "MAKER-A", 7.0),
    ("Model B15e", "MAKER-B", 1.5),
    ("Model A30p", "MAKER-A", 3.0),
    ("Model C30w", "MAKER-C", 3.0),
    ("Model B30s", "MAKER-B", 3.0),
];

fn stations() -> Vec<Station> {
    (0..40)
        .map(|j| {
            let (model, maker, field) = MODELS[j % MODELS.len()];
            Station {
                name: format!("STN{:02}", j + 1),
                model,
                maker,
                field,
            }
        })
        .collect()
}

/// The inserts of the imaging tables, each with the columns written.
struct Tables<'a> {
    study: Insert<'a>,
    series: Insert<'a>,
    series_mr: Insert<'a>,
    stack: Insert<'a>,
    fingerprint: Insert<'a>,
    classification: Insert<'a>,
    axis: Insert<'a>,
    holder: Insert<'a>,
}

/// A stack to write once its series has an id.
struct StackPlan {
    series_uid: String,
    index: i64,
    proto: usize,
    station: usize,
    in_plane: f64,
    thickness: f64,
    contrast: bool,
    dice: u64,
}

/// A series to write once its study has an id.
struct SeriesPlan {
    uid: String,
    study_uid: String,
    proto: usize,
    station: usize,
    day: Day,
    stacks: i64,
}

struct StudyPlan {
    uid: String,
    subject: i64,
    day: Day,
    station: usize,
    /// The dataset the subject's stacks are read from, as an index.
    site: usize,
}

fn weighted(rng: &mut Rng) -> usize {
    let total: u64 = PROTOCOLS.iter().map(|p| p.weight).sum();
    let mut at = rng.below(total);
    for (i, p) in PROTOCOLS.iter().enumerate() {
        if at < p.weight {
            return i;
        }
        at -= p.weight;
    }
    0
}

fn axis(stack: i64, name: &str, value: Option<&str>, tier: &str) -> Vec<Param> {
    let confidence = match tier {
        "exclusive" => 0.95,
        "keywords" => 0.85,
        "combination" => 0.75,
        "decision" => 1.0,
        "default" => 0.7,
        _ => 0.9,
    };
    vec![
        Param::Int(stack),
        Param::from(name),
        value.map_or(Param::Null, Param::from),
        Param::Double(confidence),
        Param::from(tier),
    ]
}

/// Build the scale registry into an initialised, empty registry, in one
/// transaction. The sessions are the caller's to build, with the engine's
/// own cache.
pub fn build_scale(registry: &mut Registry, plan: &ScalePlan) -> Result<ScaleManifest, Error> {
    let mut manifest = ScaleManifest {
        seed: plan.seed,
        ..ScaleManifest::default()
    };
    let now = nils_registry::time::now_iso();
    let store = registry.store();
    store.begin()?;
    if let Err(e) = write(store, plan, &now, &mut manifest) {
        store.rollback().ok();
        return Err(e);
    }
    registry
        .next_epoch()
        .map_err(|e| Error::Message(e.to_string()))?;
    registry
        .set_meta("synthetic", "nils-synth")
        .map_err(|e| Error::Message(e.to_string()))?;
    registry.store().commit()?;
    Ok(manifest)
}

fn write(
    store: &mut Store,
    plan: &ScalePlan,
    now: &str,
    manifest: &mut ScaleManifest,
) -> Result<(), Error> {
    let existing = store.query(
        &format!("SELECT COUNT(*) FROM {}", store.qualified("subject")),
        &[],
    )?[0]
        .int(0)?;
    if existing > 0 {
        return Err(Error::Message(format!(
            "the registry already holds {existing} subjects; the scale registry is built into an empty one"
        )));
    }
    let mut rng = Rng::new(plan.seed);
    let stations = stations();

    // the datasets: a source place each, and the `source` row of its tree
    let mut sources: Vec<i64> = Vec::new();
    for name in DATASETS {
        let path = format!("/synthetic/scale/{name}");
        place::add(
            store,
            &New {
                name,
                role: Role::Source,
                path: &path,
                guarantees: serde_json::json!({}),
                probed: serde_json::Value::Null,
                handling: serde_json::Value::Null,
                dataset: serde_json::Value::Null,
            },
        )?;
        let tree = format!("{path}/derivatives/dcm-anon");
        sources.push(one_id(
            store,
            &Insert::new(
                table("source"),
                &["root", "root_canonical", "first_seen_at"],
            )
            .returning(&["id"]),
            vec![text(&tree), text(&tree), text(now)],
        )?);
    }
    let job = one_id(
        store,
        &Insert::new(
            table("job"),
            &["kind", "name", "state", "started_at", "finished_at"],
        )
        .returning(&["id"]),
        vec![
            text("synth"),
            text(&format!("the scale registry, seed {}", plan.seed)),
            text("done"),
            text(now),
            text(now),
        ],
    )?;
    let batch = one_id(
        store,
        &Insert::new(
            table("ingest_batch"),
            &[
                "source_id",
                "job_id",
                "name",
                "config",
                "started_at",
                "finished_at",
                "state",
            ],
        )
        .returning(&["id"]),
        vec![
            Param::Int(sources[0]),
            Param::Int(job),
            text("synthetic"),
            text(&serde_json::json!({"seed": plan.seed, "subjects": plan.subjects}).to_string()),
            text(now),
            text(now),
            text("done"),
        ],
    )?;
    let epoch = store
        .query_opt(
            &format!(
                "SELECT value FROM {} WHERE key = 'epoch'",
                store.qualified("registry_meta")
            ),
            &[],
        )?
        .and_then(|r| {
            r.opt_text(0)
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(0)
        + 1;

    // the subjects, and who is a member of the cohort
    let subject_insert = Insert::new(
        table("subject"),
        &["code", "birth_date", "sex", "first_batch_id", "created_at"],
    )
    .returning(&["id", "code"]);
    let mut members: Vec<bool> = Vec::with_capacity(plan.subjects);
    let mut rows: Vec<Vec<Param>> = Vec::with_capacity(plan.subjects);
    for i in 0..plan.subjects {
        members.push(rng.chance(75));
        let born = shift(day(1940, 1, 1), rng.range(0, 55 * 365));
        rows.push(vec![
            text(&format!("SC{:06}", i + 1)),
            if rng.chance(92) {
                text(&iso(born))
            } else {
                Param::Null
            },
            text(if rng.chance(64) { "F" } else { "M" }),
            Param::Int(batch),
            text(now),
        ]);
    }
    let mut subject_ids: HashMap<String, i64> = HashMap::with_capacity(plan.subjects);
    for r in store.insert(&subject_insert, &rows)? {
        subject_ids.insert(r.text(1)?.to_string(), r.int(0)?);
    }
    let subject_of = |i: usize| -> Result<i64, Error> {
        subject_ids
            .get(&format!("SC{:06}", i + 1))
            .copied()
            .ok_or_else(|| Error::Message(format!("subject {i} was not written")))
    };
    manifest.subjects = plan.subjects;

    // the cohorts: the one most are members of, and one of one subject for
    // every tenth subject, as the cohorts a site makes for single studies
    let cohort_insert = Insert::new(
        table("cohort"),
        &["name", "owner", "description", "created_at"],
    )
    .returning(&["id"]);
    let member_insert = Insert::new(
        table("cohort_member"),
        &["cohort_id", "subject_id", "joined_at", "source", "actor"],
    );
    let alpha = one_id(
        store,
        &cohort_insert,
        vec![
            text(COHORT),
            text("nils-synth"),
            text("three subjects in four, those with many visits"),
            text(now),
        ],
    )?;
    manifest.cohorts = 1;
    let mut member_rows: Vec<Vec<Param>> = Vec::new();
    for (i, member) in members.iter().enumerate() {
        let subject = subject_of(i)?;
        if *member {
            member_rows.push(vec![
                Param::Int(alpha),
                Param::Int(subject),
                text("2020-01-01T00:00:00Z"),
                text("import"),
                text("nils-synth"),
            ]);
            manifest.cohort_members += 1;
        }
        if i % 10 == 3 {
            let one = one_id(
                store,
                &cohort_insert,
                vec![
                    text(&format!("single-{:04}", i / 10 + 1)),
                    text("nils-synth"),
                    Param::Null,
                    text(now),
                ],
            )?;
            manifest.cohorts += 1;
            member_rows.push(vec![
                Param::Int(one),
                Param::Int(subject),
                text("2021-01-01T00:00:00Z"),
                text("manual"),
                text("nils-synth"),
            ]);
        }
    }
    store.insert(&member_insert, &member_rows)?;

    let tables = Tables {
        study: Insert::new(
            table("study"),
            &[
                "study_instance_uid",
                "subject_id",
                "study_date",
                "study_description",
                "manufacturer",
                "manufacturer_model_name",
                "station_name",
                "first_batch_id",
                "date_filled",
                "date_source",
            ],
        )
        .returning(&["id", "study_instance_uid"]),
        series: Insert::new(
            table("series"),
            &[
                "series_instance_uid",
                "study_id",
                "subject_id",
                "modality",
                "series_description",
                "protocol_name",
                "sequence_name",
                "series_date",
                "n_instances",
                "n_stacks",
                "first_batch_id",
            ],
        )
        .returning(&["id", "series_instance_uid"]),
        series_mr: Insert::new(
            table("series_mr"),
            &[
                "series_id",
                "mr_acquisition_type",
                "repetition_time",
                "echo_time",
                "flip_angle",
                "magnetic_field_strength",
            ],
        ),
        stack: Insert::new(
            table("stack"),
            &[
                "series_id",
                "stack_index",
                "stack_key",
                "modality",
                "orientation",
                "orientation_confidence",
                "n_instances",
                "first_batch_id",
                "echo_time",
                "repetition_time",
                "flip_angle",
            ],
        )
        .returning(&["id", "series_id", "stack_index"]),
        fingerprint: Insert::new(
            table("stack_fingerprint"),
            &[
                "stack_id",
                "series_id",
                "study_id",
                "subject_id",
                "modality",
                "text_series_description",
                "text_protocol_name",
                "text_all",
                "text_series_description_ci",
                "text_protocol_name_ci",
                "text_all_ci",
                "echo_time",
                "repetition_time",
                "flip_angle",
                "magnetic_field_strength",
                "slice_thickness",
                "spacing_between_slices",
                "mr_acquisition_type",
                "orientation",
                "orientation_confidence",
                "n_instances",
                "stack_index",
                "stacks_in_series",
                "rows",
                "columns",
                "pixel_spacing",
                "pixel_spacing_row",
                "pixel_spacing_col",
                "manufacturer",
                "manufacturer_model_name",
                "station_name",
                "field_strength_tesla",
                "field_strength_normalized",
                "field_strength_unit",
                "acquisition_type_filled",
                "acquisition_type_source",
                "image_role",
                "job_id",
                "epoch",
            ],
        ),
        classification: Insert::new(
            table("classification"),
            &[
                "stack_id",
                "pack",
                "pack_version",
                "contract",
                "job_id",
                "epoch",
                "review_items",
            ],
        ),
        axis: Insert::new(
            table("classification_axis"),
            &["stack_id", "axis", "value", "confidence", "tier"],
        ),
        holder: Insert::new(
            table("source_stack"),
            &["source_id", "stack_id", "first_seen_at"],
        ),
    };

    // the imaging, a hundred subjects at a time
    let mut others = 0usize;
    let mut held = [0usize; 4];
    let mut uid = 0u64;
    let mut written = 0usize;
    let mut i = 0usize;
    while i < plan.subjects {
        let upto = (i + 100).min(plan.subjects);
        let mut studies: Vec<StudyPlan> = Vec::new();
        let mut series: Vec<SeriesPlan> = Vec::new();
        let mut stacks: Vec<StackPlan> = Vec::new();
        for (s, member) in members.iter().enumerate().take(upto).skip(i) {
            let subject = subject_of(s)?;
            // a member comes back for years; the others were seen once
            let (days, site) = if *member {
                (2 + rng.below(5) + rng.below(5) + rng.below(2), 0usize)
            } else {
                others += 1;
                let site = match others % 25 {
                    3 => 2,
                    n if n % 8 == 1 => 0,
                    _ => 1,
                };
                (1 + u64::from(rng.chance(6)), site)
            };
            let home = rng.below(40).min(rng.below(40)) as usize;
            let mut at = shift(day(2008, 1, 1), rng.range(0, 8 * 365));
            for _ in 0..days {
                let station = if rng.chance(80) {
                    home
                } else {
                    rng.below(40) as usize
                };
                let sittings = if rng.chance(13) { 2 } else { 1 };
                for _ in 0..sittings {
                    uid += 1;
                    let study_uid = format!("2.25.{}.1.{uid}", plan.seed);
                    studies.push(StudyPlan {
                        uid: study_uid.clone(),
                        subject,
                        day: at,
                        station,
                        site,
                    });
                    for _ in 0..(6 + rng.below(10)) {
                        uid += 1;
                        let series_uid = format!("2.25.{}.2.{uid}", plan.seed);
                        let p = weighted(&mut rng);
                        let proto = &PROTOCOLS[p];
                        // a scanner keeps its protocol: its resolution
                        // follows the station, with a change now and then
                        let pick = |rng: &mut Rng, n: usize| -> usize {
                            if rng.chance(8) {
                                rng.below(n as u64) as usize
                            } else {
                                (station + p) % n
                            }
                        };
                        let in_plane = proto.in_plane[pick(&mut rng, proto.in_plane.len())];
                        let thickness = if proto.thickness.is_empty() {
                            in_plane
                        } else {
                            proto.thickness[pick(&mut rng, proto.thickness.len())]
                        };
                        let contrast = rng.below(100) < proto.contrast;
                        let n = if proto.stacks == 1 {
                            1 + i64::from(rng.chance(12))
                        } else {
                            1 + rng.below(proto.stacks) as i64
                        };
                        for index in 0..n {
                            stacks.push(StackPlan {
                                series_uid: series_uid.clone(),
                                index,
                                proto: p,
                                station,
                                in_plane,
                                thickness,
                                contrast,
                                dice: rng.next(),
                            });
                        }
                        series.push(SeriesPlan {
                            uid: series_uid,
                            study_uid: study_uid.clone(),
                            proto: p,
                            station,
                            day: at,
                            stacks: n,
                        });
                    }
                }
                at = shift(at, rng.range(90, 700));
            }
        }
        written += write_imaging(
            store,
            &tables,
            &Batch {
                studies: &studies,
                series: &series,
                stacks: &stacks,
                stations: &stations,
                sources: &sources,
                batch,
                job,
                epoch,
                now,
                first: written,
            },
            manifest,
            &mut held,
        )?;
        i = upto;
    }
    manifest.held = held.iter().sum();
    manifest.datasets = DATASETS
        .iter()
        .zip(held)
        .map(|(n, h)| (n.to_string(), h))
        .collect();
    Ok(())
}

struct Batch<'a> {
    studies: &'a [StudyPlan],
    series: &'a [SeriesPlan],
    stacks: &'a [StackPlan],
    stations: &'a [Station],
    sources: &'a [i64],
    batch: i64,
    job: i64,
    epoch: i64,
    now: &'a str,
    /// How many stacks were written before this batch.
    first: usize,
}

/// Write one batch of studies with their series and stacks, and return how
/// many stacks it wrote.
fn write_imaging(
    store: &mut Store,
    t: &Tables<'_>,
    b: &Batch<'_>,
    manifest: &mut ScaleManifest,
    held: &mut [usize; 4],
) -> Result<usize, Error> {
    let rows: Vec<Vec<Param>> = b
        .studies
        .iter()
        .map(|s| {
            let st = &b.stations[s.station];
            vec![
                text(&s.uid),
                Param::Int(s.subject),
                text(&iso(s.day)),
                text("MR examination"),
                text(st.maker),
                text(st.model),
                text(&st.name),
                Param::Int(b.batch),
                text(&iso(s.day)),
                text("study_date"),
            ]
        })
        .collect();
    let mut study_ids: HashMap<String, i64> = HashMap::with_capacity(rows.len());
    for r in store.insert(&t.study, &rows)? {
        study_ids.insert(r.text(1)?.to_string(), r.int(0)?);
    }
    manifest.studies += rows.len();
    let study_of: HashMap<&str, &StudyPlan> =
        b.studies.iter().map(|s| (s.uid.as_str(), s)).collect();
    let missing = |what: &str| Error::Message(format!("{what} was not written"));

    let mut rows: Vec<Vec<Param>> = Vec::with_capacity(b.series.len());
    for s in b.series {
        let study = study_of
            .get(s.study_uid.as_str())
            .ok_or(missing("a study"))?;
        let proto = &PROTOCOLS[s.proto];
        rows.push(vec![
            text(&s.uid),
            Param::Int(*study_ids.get(&s.study_uid).ok_or(missing("a study"))?),
            Param::Int(study.subject),
            text("MR"),
            text(proto.name),
            text(proto.name),
            text(proto.technique.map_or("", |t| t.0)),
            text(&iso(s.day)),
            Param::Int(s.stacks * 40),
            Param::Int(s.stacks),
            Param::Int(b.batch),
        ]);
    }
    let mut series_ids: HashMap<String, i64> = HashMap::with_capacity(rows.len());
    for r in store.insert(&t.series, &rows)? {
        series_ids.insert(r.text(1)?.to_string(), r.int(0)?);
    }
    manifest.series += rows.len();
    let series_of: HashMap<&str, &SeriesPlan> =
        b.series.iter().map(|s| (s.uid.as_str(), s)).collect();
    let rows: Vec<Vec<Param>> = b
        .series
        .iter()
        .map(|s| {
            let proto = &PROTOCOLS[s.proto];
            vec![
                Param::Int(series_ids[&s.uid]),
                text(if proto.three_d { "3D" } else { "2D" }),
                Param::Double(if proto.three_d { 2300.0 } else { 5200.0 }),
                Param::Double(if proto.three_d { 2.9 } else { 96.0 }),
                Param::Double(if proto.three_d { 9.0 } else { 150.0 }),
                Param::Double(b.stations[s.station].field),
            ]
        })
        .collect();
    store.insert(&t.series_mr, &rows)?;

    let rows: Vec<Vec<Param>> = b
        .stacks
        .iter()
        .map(|s| {
            let proto = &PROTOCOLS[s.proto];
            vec![
                Param::Int(series_ids[&s.series_uid]),
                Param::Int(s.index),
                text(&format!("scale-{}", s.index)),
                text("MR"),
                text(proto.orientation),
                Param::Double(1.0),
                Param::Int(40),
                Param::Int(b.batch),
                Param::Double(if proto.three_d { 2.9 } else { 96.0 }),
                Param::Double(if proto.three_d { 2300.0 } else { 5200.0 }),
                Param::Double(if proto.three_d { 9.0 } else { 150.0 }),
            ]
        })
        .collect();
    let mut stack_ids: HashMap<(i64, i64), i64> = HashMap::with_capacity(rows.len());
    for r in store.insert(&t.stack, &rows)? {
        stack_ids.insert((r.int(1)?, r.int(2)?), r.int(0)?);
    }
    manifest.stacks += rows.len();

    let mut fingerprints: Vec<Vec<Param>> = Vec::with_capacity(b.stacks.len());
    let mut classes: Vec<Vec<Param>> = Vec::with_capacity(b.stacks.len());
    let mut axes: Vec<Vec<Param>> = Vec::with_capacity(b.stacks.len() * 11);
    let mut holders: Vec<Vec<Param>> = Vec::with_capacity(b.stacks.len());
    for (n, s) in b.stacks.iter().enumerate() {
        let nth = b.first + n;
        let proto = &PROTOCOLS[s.proto];
        let series_plan = series_of
            .get(s.series_uid.as_str())
            .ok_or(missing("a series"))?;
        let study_plan = study_of
            .get(series_plan.study_uid.as_str())
            .ok_or(missing("a study"))?;
        let series = series_ids[&s.series_uid];
        let study = study_ids[&series_plan.study_uid];
        let stack = *stack_ids
            .get(&(series, s.index))
            .ok_or(missing("a stack"))?;
        let station = &b.stations[s.station];
        let mut dice = Rng::new(s.dice);

        // the fingerprint: the type the header states, absent on one stack
        // in twenty, filled from the geometry on most of those and on one
        // in four of the rest
        let stated = if proto.three_d { "3D" } else { "2D" };
        let read = (!dice.chance(5)).then_some(stated);
        let filled = match read {
            Some(_) => dice.chance(25).then_some(stated),
            None => dice.chance(80).then_some(stated),
        };
        let col = if dice.chance(3) {
            s.in_plane + 0.1
        } else {
            s.in_plane
        };
        let matrix = (240.0 / s.in_plane).round() as i64;
        let text_all = format!("{} {} {}", proto.name, proto.name, station.model);
        fingerprints.push(vec![
            Param::Int(stack),
            Param::Int(series),
            Param::Int(study),
            Param::Int(study_plan.subject),
            text("MR"),
            text(proto.name),
            text(proto.name),
            text(&text_all),
            text(&proto.name.to_lowercase()),
            text(&proto.name.to_lowercase()),
            text(&text_all.to_lowercase()),
            Param::Double(if proto.three_d { 2.9 } else { 96.0 }),
            Param::Double(if proto.three_d { 2300.0 } else { 5200.0 }),
            Param::Double(if proto.three_d { 9.0 } else { 150.0 }),
            Param::Double(station.field),
            if dice.chance(1) {
                Param::Null
            } else {
                Param::Double(s.thickness)
            },
            Param::Double(s.thickness),
            read.map_or(Param::Null, text),
            text(proto.orientation),
            Param::Double(1.0),
            Param::Int(40),
            Param::Int(s.index),
            Param::Int(series_plan.stacks),
            Param::Int(matrix),
            Param::Int(matrix),
            text(&format!("{}\\{col}", s.in_plane)),
            Param::Double(s.in_plane),
            Param::Double(col),
            text(station.maker),
            text(station.model),
            text(&station.name),
            Param::Double(station.field),
            Param::Double(station.field),
            text("T"),
            filled.map_or(Param::Null, text),
            filled.map_or(Param::Null, |_| text("geometry")),
            text(if proto.constructs.iter().any(|c| !c.is_empty()) {
                "derived"
            } else {
                "original"
            }),
            Param::Int(b.job),
            Param::Int(b.epoch),
        ]);
        classes.push(vec![
            Param::Int(stack),
            text("mri"),
            text("synthetic"),
            Param::Int(2),
            Param::Int(b.job),
            Param::Int(b.epoch),
            Param::Int(0),
        ]);

        // the axes: the stored name of each value, and on one row in a
        // hundred its identity, as a person's decision may hold it
        let form = |dice: &mut Rng, names: (&'static str, &'static str)| -> &'static str {
            if dice.chance(1) { names.1 } else { names.0 }
        };
        let before = axes.len();
        if let Some(names) = proto.base {
            axes.push(axis(
                stack,
                "base",
                Some(form(&mut dice, names)),
                "keywords",
            ));
            if dice.chance(88) {
                let basis = ["name", "physics", "name+physics"][dice.below(3) as usize];
                axes.push(axis(stack, "base_basis", Some(basis), "stated"));
            }
        }
        if let Some(names) = proto.technique
            && dice.chance(96)
        {
            axes.push(axis(
                stack,
                "technique",
                Some(form(&mut dice, names)),
                "exclusive",
            ));
        }
        if proto.modifiers.is_empty() {
            // decided to nothing: one row with no value
            if dice.below(300) == 0 {
                axes.push(axis(stack, "modifier", None, "decision"));
            }
        } else {
            for m in proto.modifiers {
                axes.push(axis(stack, "modifier", Some(m), "keywords"));
            }
            if dice.chance(4) {
                axes.push(axis(stack, "modifier", Some("MoCo"), "combination"));
            }
        }
        let constructs = proto.constructs[(s.index as usize) % proto.constructs.len()];
        for c in constructs {
            axes.push(axis(stack, "construct", Some(c), "keywords"));
        }
        if dice.chance(2) {
            axes.push(axis(stack, "construct", Some("ND"), "combination"));
        }
        axes.push(axis(stack, "provenance", Some(proto.provenance), "default"));
        let given = if s.contrast {
            ("1", "given")
        } else {
            ("0", "not_given")
        };
        axes.push(axis(
            stack,
            "post_contrast",
            Some(form(&mut dice, given)),
            "stated",
        ));
        axes.push(axis(stack, "body_part", Some(proto.body_part.0), "stated"));
        axes.push(axis(
            stack,
            "body_region",
            Some(proto.body_part.1),
            "stated",
        ));
        let excluded = nth % 500 == 137;
        if excluded {
            manifest.excluded += 1;
        }
        axes.push(axis(
            stack,
            "disposition",
            Some(if excluded {
                "excluded"
            } else {
                proto.disposition
            }),
            "stated",
        ));
        axes.push(axis(
            stack,
            "convertible",
            Some(if dice.chance(97) { "yes" } else { "no" }),
            "default",
        ));
        axes.push(axis(
            stack,
            "directory_type",
            Some(if excluded {
                "excluded"
            } else {
                proto.directory
            }),
            "stated",
        ));
        if let Some(role) = proto.role
            && s.index == 0
            && dice.chance(40)
        {
            axes.push(axis(stack, "role", Some(role), "stated"));
        }
        match dice.below(100) {
            0 => {
                axes.push(axis(stack, "quality", Some("Distorted"), "keywords"));
                axes.push(axis(stack, "quality", Some("InputUnavailable"), "keywords"));
            }
            1..=4 => axes.push(axis(stack, "quality", Some("Distorted"), "keywords")),
            _ => {}
        }
        if dice.chance(3) {
            axes.push(axis(
                stack,
                "contrast_mix",
                Some("intermediate_PD_T2"),
                "combination",
            ));
        }
        manifest.axis_rows += axes.len() - before;

        // which dataset's tree holds a file of the stack: its subject's,
        // none for one stack in ten thousand, and for one in eight hundred
        // of the first dataset's a copy in the fourth
        if nth % 10_000 != 4_321 {
            holders.push(vec![
                Param::Int(b.sources[study_plan.site]),
                Param::Int(stack),
                text(b.now),
            ]);
            held[study_plan.site] += 1;
            if study_plan.site == 0 && nth % 800 == 77 {
                holders.push(vec![
                    Param::Int(b.sources[3]),
                    Param::Int(stack),
                    text(b.now),
                ]);
                held[3] += 1;
            }
        }
    }
    store.insert(&t.fingerprint, &fingerprints)?;
    store.insert(&t.classification, &classes)?;
    store.insert(&t.axis, &axes)?;
    store.insert(&t.holder, &holders)?;
    Ok(b.stacks.len())
}
