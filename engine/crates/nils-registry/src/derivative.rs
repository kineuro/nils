// SPDX-License-Identifier: AGPL-3.0-only

//! Derivatives (record 42 S4): files made from the archive that are not the
//! archive, a mask, an embedding, a pipeline's output, kept in a working
//! place (Wave 5 section 10.2) and named here by their digest.
//!
//! The registry holds the row: what the file is, what it belongs to, where
//! it lives, how many bytes it has and their sha256, and who made it. The
//! bytes are the place's. Nothing is deleted: a newer file supersedes an
//! older one by a link, and both stay.
//!
//! This module is the rows. Writing the bytes into the place and hashing
//! them is the binary's, which owns the doors and the command line.

use serde::Serialize;

use crate::dialect::Conflict;
use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};

/// The kinds a derivative may be. A mask is an uploaded segmentation, an
/// embedding one encoder's vectors for one stack, a pyramid the viewer's
/// tiles (wave 43 moves them here), an output anything else a pipeline
/// wrote. Record 43 adds two a run writes and a person does not: seeds,
/// the suggestions a run made for a person to curate, and model, the
/// artifact of a model a run fitted and the registry registered. Record 49
/// adds table: a file of numbers with declared columns, whose rows the
/// runner loads as measures the ask reads.
pub const KINDS: [&str; 7] = [
    "mask",
    "embedding",
    "pyramid",
    "output",
    "seeds",
    "model",
    "table",
];

/// The kinds only a pipeline run writes (record 43).
pub const RUN_KINDS: [&str; 2] = ["seeds", "model"];

/// Where the files of registered derivatives go under a working place.
pub const TREE: &str = "derivatives";

/// What a derivative belongs to, resolved against the registry: the scope
/// and its ids, the subject filled for every scope but a run's and a
/// model's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Belongs {
    /// `stack`, `series`, `session`, `subject`, `run` for a file that is a
    /// whole run's rather than any subject's (record 43: seeds, a model), or
    /// `model` for a registered model's artifact kept in a working place by
    /// `nils model keep` (record 50), which names the model through the
    /// row's `model_id` and neither a subject nor a run.
    pub scope: String,
    pub stack_id: Option<i64>,
    pub series_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
}

impl Belongs {
    /// A file that is a whole pipeline run's (record 43): it names the run,
    /// through the row's `run_id`, and no subject.
    pub fn run() -> Belongs {
        Belongs {
            scope: "run".into(),
            stack_id: None,
            series_id: None,
            subject_id: None,
            session_day: None,
        }
    }

    /// A registered model's artifact, kept where a run finds it (record
    /// 50): it names the model, through the row's `model_id`, and no
    /// subject and no run.
    pub fn model() -> Belongs {
        Belongs {
            scope: "model".into(),
            stack_id: None,
            series_id: None,
            subject_id: None,
            session_day: None,
        }
    }
}

/// One derivative as the registry holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Derivative {
    pub id: i64,
    pub kind: String,
    pub scope: String,
    pub stack_id: Option<i64>,
    pub series_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub session_day: Option<String>,
    pub place_id: i64,
    pub path: String,
    pub bytes: i64,
    pub sha256: String,
    pub media_type: String,
    pub registered_by: Option<String>,
    pub actor: serde_json::Value,
    pub model_id: Option<i64>,
    pub run_id: Option<i64>,
    pub preprocess_version: Option<String>,
    pub supersedes_id: Option<i64>,
    pub created_at: String,
    pub withdrawn_at: Option<String>,
    /// Job type 3 (record 55 H3): a mask's grid as its header says it,
    /// {dims, affine, datatype}, null until it is read.
    pub grid: serde_json::Value,
    /// The scan whose grid the mask is on.
    pub grid_stack_id: Option<i64>,
    /// `same`, `world` or `none`; null until the grid is read.
    pub grid_link: Option<String>,
    /// The part sets the mask is read as, null until its parts are made.
    pub part_sets: serde_json::Value,
    /// The status a check wrote: accepted, rejected, flagged, or null.
    #[serde(rename = "check")]
    pub check_status: Option<String>,
    pub check_reasons: serde_json::Value,
    pub check_decision_id: Option<i64>,
    pub checked_at: Option<String>,
}

/// The statuses a check writes onto a result and its parts (record 55 H3).
pub const CHECKS: [&str; 3] = ["accepted", "rejected", "flagged"];

/// The most parts one set of a mask holds (record 55 H3, section 3.5); a
/// set with more is read whole.
pub const PARTS_MAX: usize = 10_000;

/// How a mask's grid is tied to its scan's (record 55 H3, section 2).
pub const GRID_LINKS: [&str; 3] = ["same", "world", "none"];

/// A derivative to register, its file already in its place.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub kind: &'a str,
    pub belongs: &'a Belongs,
    pub place_id: i64,
    pub path: &'a str,
    pub bytes: i64,
    pub sha256: &'a str,
    pub media_type: &'a str,
    pub registered_by: &'a str,
    pub actor: Option<&'a serde_json::Value>,
    /// The registered model that made it (record 42 S2's table), when a
    /// model did; [`insert`] refuses an id the model table does not hold.
    pub model_id: Option<i64>,
    /// The pipeline run that wrote it, when one did (record 43).
    pub run_id: Option<i64>,
    /// For an embedding, the preprocessing it was made under (record 43
    /// S4); [`crate::embedding::register`] fills it with the encoder.
    pub preprocess_version: Option<&'a str>,
    pub supersedes_id: Option<i64>,
    pub created_at: &'a str,
}

/// Whether a kind is one [`KINDS`] names.
pub fn is_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Resolve what a derivative belongs to from the ids a caller named:
/// exactly one of a stack, a series, or a subject (with a day for one
/// occasion). A refusal is `Err(Ok(sentence))`; a store failure `Err(Err)`.
pub fn belongs(
    store: &mut Store,
    stack: Option<i64>,
    series: Option<i64>,
    subject: Option<i64>,
    day: Option<&str>,
) -> Result<Belongs, Result<String, Error>> {
    let named = [stack.is_some(), series.is_some(), subject.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if named != 1 {
        return Err(Ok(
            "a derivative belongs to exactly one of a stack, a series or a subject".into(),
        ));
    }
    if day.is_some() && subject.is_none() {
        return Err(Ok(
            "a day names one occasion of a subject; give it with the subject".into(),
        ));
    }
    let d = store.dialect();
    let one = |store: &mut Store, sql: String, id: i64| -> Result<Option<Row>, Error> {
        store.query_opt(&sql, &[Param::Int(id)])
    };
    if let Some(id) = stack {
        let sql = format!(
            "SELECT st.series_id, se.subject_id FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id = {}",
            store.qualified("stack"),
            store.qualified("series"),
            d.param(1, Type::Int)
        );
        let row = one(store, sql, id)
            .map_err(Err)?
            .ok_or_else(|| Ok(format!("no stack {id}")))?;
        return Ok(Belongs {
            scope: "stack".into(),
            stack_id: Some(id),
            series_id: row.opt_int(0).map_err(Err)?,
            subject_id: Some(row.int(1).map_err(Err)?),
            session_day: None,
        });
    }
    if let Some(id) = series {
        let sql = format!(
            "SELECT subject_id FROM {} WHERE id = {}",
            store.qualified("series"),
            d.param(1, Type::Int)
        );
        let row = one(store, sql, id)
            .map_err(Err)?
            .ok_or_else(|| Ok(format!("no series {id}")))?;
        return Ok(Belongs {
            scope: "series".into(),
            stack_id: None,
            series_id: Some(id),
            subject_id: Some(row.int(0).map_err(Err)?),
            session_day: None,
        });
    }
    let id = subject.expect("one of the three is named");
    let sql = format!(
        "SELECT id FROM {} WHERE id = {}",
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    one(store, sql, id)
        .map_err(Err)?
        .ok_or_else(|| Ok(format!("no subject {id}")))?;
    if let Some(day) = day
        && crate::day::Day::parse(day).is_none()
    {
        return Err(Ok(format!("{day} is not a day (YYYY-MM-DD)")));
    }
    Ok(Belongs {
        scope: if day.is_some() { "session" } else { "subject" }.into(),
        stack_id: None,
        series_id: None,
        subject_id: Some(id),
        session_day: day.map(str::to_string),
    })
}

/// Write the row. Answers its id. A model named is one the registry holds:
/// the column refers to the model table, which the store does not enforce.
pub fn insert(store: &mut Store, n: &New<'_>) -> Result<i64, Error> {
    insert_row(store, n, None)
}

/// [`insert`] for a file a pipeline run made (record 43 S2): the row names
/// the run, which the registry holds.
pub fn insert_of_run(store: &mut Store, n: &New<'_>, run_id: i64) -> Result<i64, Error> {
    if n.belongs.scope == "run" && n.belongs.subject_id.is_some() {
        return Err(Error::Message("a run's own file names no subject".into()));
    }
    if crate::pipeline::run(store, run_id)?.is_none() {
        return Err(Error::Message(format!(
            "no pipeline run {run_id} made this derivative"
        )));
    }
    insert_row(store, n, Some(run_id))
}

fn insert_row(store: &mut Store, n: &New<'_>, run_id: Option<i64>) -> Result<i64, Error> {
    if n.belongs.scope == "model" {
        // record 50: a registered model's artifact, and nothing else
        if n.kind != "model"
            || n.model_id.is_none()
            || n.belongs.subject_id.is_some()
            || n.belongs.stack_id.is_some()
            || n.belongs.series_id.is_some()
            || n.belongs.session_day.is_some()
            || run_id.or(n.run_id).is_some()
        {
            return Err(Error::Message(
                "a derivative of scope model is a registered model's artifact: kind model, the model named, and no subject and no run".into(),
            ));
        }
    } else if n.belongs.subject_id.is_none() && (n.belongs.scope != "run" || run_id.is_none()) {
        return Err(Error::Message(
            "a derivative belongs to a subject, or is a pipeline run's own file, or is a registered model's artifact".into(),
        ));
    }
    if let Some(model) = n.model_id
        && crate::model::get(store, model)?.is_none()
    {
        return Err(Error::Message(format!(
            "no registered model {model} made this derivative"
        )));
    }
    let rows = store.insert(
        &Insert::new(
            table("derivative"),
            &[
                "kind",
                "scope",
                "stack_id",
                "series_id",
                "subject_id",
                "session_day",
                "place_id",
                "path",
                "bytes",
                "sha256",
                "media_type",
                "registered_by",
                "actor",
                "model_id",
                "run_id",
                "preprocess_version",
                "supersedes_id",
                "created_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(n.kind),
            Param::from(n.belongs.scope.as_str()),
            n.belongs.stack_id.map_or(Param::Null, Param::Int),
            n.belongs.series_id.map_or(Param::Null, Param::Int),
            n.belongs.subject_id.map_or(Param::Null, Param::Int),
            n.belongs
                .session_day
                .as_deref()
                .map_or(Param::Null, Param::from),
            Param::Int(n.place_id),
            Param::from(n.path),
            Param::Int(n.bytes),
            Param::from(n.sha256),
            Param::from(n.media_type),
            Param::from(n.registered_by),
            n.actor.map_or(Param::Null, |a| Param::from(a.to_string())),
            n.model_id.map_or(Param::Null, Param::Int),
            run_id.or(n.run_id).map_or(Param::Null, Param::Int),
            n.preprocess_version.map_or(Param::Null, Param::from),
            n.supersedes_id.map_or(Param::Null, Param::Int),
            Param::from(n.created_at),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| Error::Message("the derivative was not written back".into()))?
        .int(0)
}

const COLUMNS: [&str; 28] = [
    "id",
    "kind",
    "scope",
    "stack_id",
    "series_id",
    "subject_id",
    "session_day",
    "place_id",
    "path",
    "bytes",
    "sha256",
    "media_type",
    "registered_by",
    "actor",
    "model_id",
    "run_id",
    "preprocess_version",
    "supersedes_id",
    "created_at",
    "withdrawn_at",
    "grid",
    "grid_stack_id",
    "grid_link",
    "part_sets",
    "check_status",
    "check_reasons",
    "check_decision_id",
    "checked_at",
];

pub(crate) fn select(store: &mut Store) -> String {
    let d = store.dialect();
    let t = table("derivative");
    let cols: Vec<String> = COLUMNS
        .iter()
        .map(|c| d.text_of(t.column(c).expect("a derivative column")))
        .collect();
    format!(
        "SELECT {} FROM {}",
        cols.join(", "),
        store.qualified("derivative")
    )
}

pub(crate) fn of(r: &Row) -> Result<Derivative, Error> {
    Ok(Derivative {
        id: r.int(0)?,
        kind: r.text(1)?.to_string(),
        scope: r.text(2)?.to_string(),
        stack_id: r.opt_int(3)?,
        series_id: r.opt_int(4)?,
        subject_id: r.opt_int(5)?,
        session_day: r.opt_text(6)?.map(str::to_string),
        place_id: r.int(7)?,
        path: r.text(8)?.to_string(),
        bytes: r.int(9)?,
        sha256: r.text(10)?.to_string(),
        media_type: r.text(11)?.to_string(),
        registered_by: r.opt_text(12)?.map(str::to_string),
        actor: r
            .opt_text(13)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(serde_json::Value::Null),
        model_id: r.opt_int(14)?,
        run_id: r.opt_int(15)?,
        preprocess_version: r.opt_text(16)?.map(str::to_string),
        supersedes_id: r.opt_int(17)?,
        created_at: r.text(18)?.to_string(),
        withdrawn_at: r.opt_text(19)?.map(str::to_string),
        grid: json_of(r.opt_text(20)?),
        grid_stack_id: r.opt_int(21)?,
        grid_link: r.opt_text(22)?.map(str::to_string),
        part_sets: json_of(r.opt_text(23)?),
        check_status: r.opt_text(24)?.map(str::to_string),
        check_reasons: json_of(r.opt_text(25)?),
        check_decision_id: r.opt_int(26)?,
        checked_at: r.opt_text(27)?.map(str::to_string),
    })
}

fn json_of(text: Option<&str>) -> serde_json::Value {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// One derivative by id.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Derivative>, Error> {
    let d = store.dialect();
    let sql = format!("{} WHERE id = {}", select(store), d.param(1, Type::Int));
    store
        .query_opt(&sql, &[Param::Int(id)])?
        .map(|r| of(&r))
        .transpose()
}

/// The live derivatives of one kind of `stacks` in one place, read a few
/// hundred stacks at a time (record 43: a run's derivative inputs).
pub fn of_stacks(
    store: &mut Store,
    kind: &str,
    stacks: &[i64],
    place_id: i64,
) -> Result<Vec<Derivative>, Error> {
    let d = store.dialect();
    let mut out = Vec::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "{} WHERE kind = {} AND place_id = {} AND withdrawn_at IS NULL AND stack_id IN ({list}) ORDER BY id",
            select(store),
            d.param(1, Type::Text),
            d.param(2, Type::Int)
        );
        for r in store.query(&sql, &[Param::from(kind), Param::Int(place_id)])? {
            out.push(of(&r)?);
        }
    }
    Ok(out)
}

/// The live derivatives of one kind in one place that belong to a session
/// or to the whole subject of one of `subjects`, read a few hundred
/// subjects at a time: what a pipeline run of the bids layout registers,
/// which names no stack, taken as a later run's derivative input.
pub fn of_subjects(
    store: &mut Store,
    kind: &str,
    subjects: &[i64],
    place_id: i64,
) -> Result<Vec<Derivative>, Error> {
    let d = store.dialect();
    let mut out = Vec::new();
    for chunk in subjects.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "{} WHERE kind = {} AND place_id = {} AND withdrawn_at IS NULL AND scope IN ('session', 'subject') AND subject_id IN ({list}) ORDER BY id",
            select(store),
            d.param(1, Type::Text),
            d.param(2, Type::Int)
        );
        for r in store.query(&sql, &[Param::from(kind), Param::Int(place_id)])? {
            out.push(of(&r)?);
        }
    }
    Ok(out)
}

/// The live derivatives of kind model that name `model_id` in one place,
/// newest first: the artifacts a run of that model may mount (record 43),
/// whether a run fitted it or `nils model keep` kept it (record 50).
pub fn of_model(store: &mut Store, model_id: i64, place_id: i64) -> Result<Vec<Derivative>, Error> {
    let d = store.dialect();
    let sql = format!(
        "{} WHERE kind = 'model' AND model_id = {} AND place_id = {} AND withdrawn_at IS NULL ORDER BY id DESC",
        select(store),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(model_id), Param::Int(place_id)])?
        .iter()
        .map(of)
        .collect()
}

/// What a listing narrows to.
#[derive(Debug, Clone, Default)]
pub struct Filter<'a> {
    pub kind: Option<&'a str>,
    pub stack_id: Option<i64>,
    pub subject_id: Option<i64>,
    /// Record 43: only the files one pipeline run made.
    pub run_id: Option<i64>,
    /// Record 55 H3: only the results a check left so (accepted, rejected,
    /// flagged), or that no check has decided (`unchecked`).
    pub check: Option<&'a str>,
    pub limit: usize,
}

/// The derivatives a filter names, newest first.
pub fn list(store: &mut Store, f: &Filter<'_>) -> Result<Vec<Derivative>, Error> {
    let d = store.dialect();
    let mut sql = format!("{} WHERE 1 = 1", select(store));
    let mut params: Vec<Param> = Vec::new();
    if let Some(k) = f.kind {
        params.push(Param::from(k));
        sql.push_str(&format!(
            " AND kind = {}",
            d.param(params.len(), Type::Text)
        ));
    }
    if let Some(s) = f.stack_id {
        params.push(Param::Int(s));
        sql.push_str(&format!(
            " AND stack_id = {}",
            d.param(params.len(), Type::Int)
        ));
    }
    if let Some(s) = f.subject_id {
        params.push(Param::Int(s));
        sql.push_str(&format!(
            " AND subject_id = {}",
            d.param(params.len(), Type::Int)
        ));
    }
    if let Some(r) = f.run_id {
        params.push(Param::Int(r));
        sql.push_str(&format!(
            " AND run_id = {}",
            d.param(params.len(), Type::Int)
        ));
    }
    match f.check {
        None => {}
        Some("unchecked") => sql.push_str(" AND check_status IS NULL"),
        Some(c) if CHECKS.contains(&c) => {
            params.push(Param::from(c));
            sql.push_str(&format!(
                " AND check_status = {}",
                d.param(params.len(), Type::Text)
            ));
        }
        Some(other) => {
            return Err(Error::Message(format!(
                "check is accepted, rejected, flagged or unchecked, not {other}"
            )));
        }
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", f.limit.max(1)));
    store.query(&sql, &params)?.iter().map(of).collect()
}

/// Write what a mask's header says of its grid, the scan whose grid it is
/// on and how the two are tied (record 55 H3, section 2).
pub fn set_grid(
    store: &mut Store,
    id: i64,
    grid: &serde_json::Value,
    grid_stack_id: Option<i64>,
    link: &str,
) -> Result<(), Error> {
    if !GRID_LINKS.contains(&link) {
        return Err(Error::Message(format!(
            "a grid link is same, world or none, not {link}"
        )));
    }
    store.update_by_id(
        table("derivative"),
        &[
            (
                "grid",
                if grid.is_null() {
                    Param::Null
                } else {
                    Param::from(grid.to_string())
                },
            ),
            (
                "grid_stack_id",
                grid_stack_id.map_or(Param::Null, Param::Int),
            ),
            ("grid_link", Param::from(link)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// One part of a mask (record 55 H3, section 3): a component of a lesion
/// label, or one label of a label map, in the mask's own grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Part {
    pub label: i64,
    pub name: Option<String>,
    pub voxels: i64,
    pub volume_mm3: f64,
    /// The part's voxel nearest its centre of mass, millimetres, LPS.
    pub anchor_mm: [f64; 3],
    /// The world minimum and maximum of the part's voxels' corners, LPS.
    pub box_mm: [[f64; 3]; 2],
    /// Its place in the order along the grid scan's slice direction,
    /// bottom to top, from 0.
    pub position: i64,
    pub method: String,
    #[serde(rename = "check")]
    pub check_status: Option<String>,
    pub check_reason: Option<String>,
    #[serde(skip)]
    pub check_decision_id: Option<i64>,
}

/// Write a mask's part sets and the parts of each, in one transaction. A
/// part made again the same keeps its check: the status is a person's and
/// stays. A part whose label now names other geometry (other voxels, anchor
/// or box), and a decided part a set no longer holds, lose their check and
/// its decision is withdrawn, never deleted, so a status never moves onto
/// another lesion (the review of E1, item 4). A part no check decided that
/// a set no longer holds is removed. Answers the decisions withdrawn.
pub fn set_parts(
    store: &mut Store,
    id: i64,
    part_sets: &serde_json::Value,
    parts: &[(String, Vec<Part>)],
) -> Result<Vec<i64>, Error> {
    store.begin()?;
    let written = (|| -> Result<Vec<i64>, Error> {
        store.update_by_id(
            table("derivative"),
            &[("part_sets", Param::from(part_sets.to_string()))],
            "id",
            id,
        )?;
        let d = store.dialect();
        let t = table("derivative_part");
        let mut withdrawn: Vec<i64> = Vec::new();
        for (set, list) in parts {
            let made: std::collections::BTreeMap<i64, &Part> =
                list.iter().map(|p| (p.label, p)).collect();
            // what each label holds now, as numbers, whatever text a
            // backend keeps the JSON in
            let sql = format!(
                "SELECT label, voxels, {}, {}, check_status, check_decision_id FROM {} \
                 WHERE derivative_id = {} AND part_set = {}",
                d.text_of(t.column("anchor_mm").expect("anchor_mm")),
                d.text_of(t.column("box_mm").expect("box_mm")),
                store.qualified("derivative_part"),
                d.param(1, Type::Int),
                d.param(2, Type::Text)
            );
            let mut gone: Vec<i64> = Vec::new();
            let mut moved: Vec<i64> = Vec::new();
            for r in store.query(&sql, &[Param::Int(id), Param::from(set.as_str())])? {
                let label = r.int(0)?;
                let decided = r.opt_text(4)?.is_some();
                let decision = r.opt_int(5)?;
                match made.get(&label) {
                    None => {
                        gone.push(label);
                        withdrawn.extend(decision);
                    }
                    Some(p) if decided => {
                        let anchor: Option<[f64; 3]> =
                            r.opt_text(2)?.and_then(|t| serde_json::from_str(t).ok());
                        let bx: Option<[[f64; 3]; 2]> =
                            r.opt_text(3)?.and_then(|t| serde_json::from_str(t).ok());
                        let same = r.int(1)? == p.voxels
                            && anchor.is_some_and(|a| close3(a, p.anchor_mm))
                            && bx.is_some_and(|b| {
                                close3(b[0], p.box_mm[0]) && close3(b[1], p.box_mm[1])
                            });
                        if !same {
                            moved.push(label);
                            withdrawn.extend(decision);
                        }
                    }
                    Some(_) => {}
                }
            }
            for chunk in gone.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                store.execute(
                    &format!(
                        "DELETE FROM {} WHERE derivative_id = {} AND part_set = {} AND label IN ({list})",
                        store.qualified("derivative_part"),
                        d.param(1, Type::Int),
                        d.param(2, Type::Text)
                    ),
                    &[Param::Int(id), Param::from(set.as_str())],
                )?;
            }
            for chunk in moved.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                store.execute(
                    &format!(
                        "UPDATE {} SET check_status = NULL, check_reason = NULL, check_decision_id = NULL \
                         WHERE derivative_id = {} AND part_set = {} AND label IN ({list})",
                        store.qualified("derivative_part"),
                        d.param(1, Type::Int),
                        d.param(2, Type::Text)
                    ),
                    &[Param::Int(id), Param::from(set.as_str())],
                )?;
            }
            let rows: Vec<Vec<Param>> = list
                .iter()
                .map(|p| {
                    vec![
                        Param::Int(id),
                        Param::from(set.as_str()),
                        Param::Int(p.label),
                        p.name.as_deref().map_or(Param::Null, Param::from),
                        Param::Int(p.voxels),
                        Param::Double(p.volume_mm3),
                        Param::from(serde_json::json!(p.anchor_mm).to_string()),
                        Param::from(serde_json::json!(p.box_mm).to_string()),
                        Param::Int(p.position),
                        Param::from(p.method.as_str()),
                    ]
                })
                .collect();
            for chunk in rows.chunks(500) {
                store.insert(
                    &Insert::new(
                        table("derivative_part"),
                        &[
                            "derivative_id",
                            "part_set",
                            "label",
                            "name",
                            "voxels",
                            "volume_mm3",
                            "anchor_mm",
                            "box_mm",
                            "position",
                            "method",
                        ],
                    )
                    .on_conflict(Conflict::Update {
                        target: &["derivative_id", "part_set", "label"],
                        set: &[
                            "name",
                            "voxels",
                            "volume_mm3",
                            "anchor_mm",
                            "box_mm",
                            "position",
                            "method",
                        ],
                    }),
                    chunk,
                )?;
            }
        }
        withdrawn.sort_unstable();
        withdrawn.dedup();
        if !withdrawn.is_empty() {
            let now = crate::time::now_iso();
            store.execute(
                &format!(
                    "UPDATE {} SET withdrawn_at = {} WHERE withdrawn_at IS NULL AND id IN ({})",
                    store.qualified("decision"),
                    d.param(1, Type::Timestamp),
                    withdrawn
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                &[Param::from(now.as_str())],
            )?;
        }
        Ok(withdrawn)
    })();
    match written {
        Ok(w) => {
            store.commit()?;
            Ok(w)
        }
        Err(e) => {
            let _ = store.rollback();
            Err(e)
        }
    }
}

/// Whether two points are one, to the thousandth of a millimetre a part
/// keeps.
fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.0015)
}

/// How a page of parts is ordered: along the slice direction, or largest
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartOrder {
    Position,
    Size,
}

/// One page of a set's parts, and how many the set holds.
pub fn parts(
    store: &mut Store,
    id: i64,
    set: &str,
    order: PartOrder,
    page: usize,
    per_page: usize,
) -> Result<(i64, Vec<Part>), Error> {
    let d = store.dialect();
    let t = table("derivative_part");
    let count = store
        .query_opt(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE derivative_id = {} AND part_set = {}",
                store.qualified("derivative_part"),
                d.param(1, Type::Int),
                d.param(2, Type::Text)
            ),
            &[Param::Int(id), Param::from(set)],
        )?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0);
    let by = match order {
        PartOrder::Position => "position, label",
        PartOrder::Size => "voxels DESC, position, label",
    };
    let cols = [
        "label",
        "name",
        "voxels",
        "volume_mm3",
        "anchor_mm",
        "box_mm",
        "position",
        "method",
        "check_status",
        "check_reason",
        "check_decision_id",
    ]
    .iter()
    .map(|c| d.text_of(t.column(c).expect("a part column")))
    .collect::<Vec<_>>()
    .join(", ");
    // a page past the set is empty, read from nothing, and its offset
    // never overflows (the review of E1, item 11)
    let per_page = per_page.clamp(1, PARTS_MAX);
    let offset = page.saturating_mul(per_page);
    if offset >= count.max(0) as usize {
        return Ok((count, Vec::new()));
    }
    let sql = format!(
        "SELECT {cols} FROM {} WHERE derivative_id = {} AND part_set = {} ORDER BY {by} LIMIT {per_page} OFFSET {offset}",
        store.qualified("derivative_part"),
        d.param(1, Type::Int),
        d.param(2, Type::Text),
    );
    let three = |text: Option<&str>| -> [f64; 3] {
        text.and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or([0.0; 3])
    };
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(id), Param::from(set)])? {
        out.push(Part {
            label: r.int(0)?,
            name: r.opt_text(1)?.map(str::to_string),
            voxels: r.int(2)?,
            volume_mm3: r.double(3)?,
            anchor_mm: three(r.opt_text(4)?),
            box_mm: r
                .opt_text(5)?
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or([[0.0; 3]; 2]),
            position: r.int(6)?,
            method: r.text(7)?.to_string(),
            check_status: r.opt_text(8)?.map(str::to_string),
            check_reason: r.opt_text(9)?.map(str::to_string),
            check_decision_id: r.opt_int(10)?,
        });
    }
    Ok((count, out))
}

/// Write the status a check left on a result (record 55 H3, section 6.4):
/// the verdict, the result-level reasons of the deciding answer, the
/// decision it is and when. None clears it, as a withdrawal does. Inside
/// the caller's transaction.
pub fn set_check(
    store: &mut Store,
    id: i64,
    status: Option<&str>,
    reasons: &serde_json::Value,
    decision: Option<i64>,
    at: Option<&str>,
) -> Result<(), Error> {
    if let Some(s) = status
        && !CHECKS.contains(&s)
    {
        return Err(Error::Message(format!(
            "a check is accepted, rejected or flagged, not {s}"
        )));
    }
    store.update_by_id(
        table("derivative"),
        &[
            ("check_status", status.map_or(Param::Null, Param::from)),
            (
                "check_reasons",
                if reasons.is_null() || status.is_none() {
                    Param::Null
                } else {
                    Param::from(reasons.to_string())
                },
            ),
            (
                "check_decision_id",
                decision.map_or(Param::Null, Param::Int),
            ),
            ("checked_at", at.map_or(Param::Null, Param::from)),
        ],
        "id",
        id,
    )?;
    Ok(())
}

/// Write the status a check left on one part of a result: the verdict, the
/// reason and the decision. None clears it. Answers whether the part is
/// there: a part a set no longer holds is not written.
pub fn set_part_check(
    store: &mut Store,
    id: i64,
    set: &str,
    label: i64,
    status: Option<&str>,
    reason: Option<&str>,
    decision: Option<i64>,
) -> Result<bool, Error> {
    if let Some(s) = status
        && !CHECKS.contains(&s)
    {
        return Err(Error::Message(format!(
            "a check is accepted, rejected or flagged, not {s}"
        )));
    }
    let d = store.dialect();
    let n = store.execute(
        &format!(
            "UPDATE {} SET check_status = {}, check_reason = {}, check_decision_id = {} \
             WHERE derivative_id = {} AND part_set = {} AND label = {}",
            store.qualified("derivative_part"),
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
            d.param(4, Type::Int),
            d.param(5, Type::Text),
            d.param(6, Type::Int),
        ),
        &[
            status.map_or(Param::Null, Param::from),
            reason.map_or(Param::Null, Param::from),
            decision.map_or(Param::Null, Param::Int),
            Param::Int(id),
            Param::from(set),
            Param::Int(label),
        ],
    )?;
    Ok(n > 0)
}

/// Clear every part status a result's parts hold, as a withdrawal of its
/// check does. Answers how many were cleared.
pub fn clear_part_checks(store: &mut Store, id: i64) -> Result<u64, Error> {
    let d = store.dialect();
    store.execute(
        &format!(
            "UPDATE {} SET check_status = NULL, check_reason = NULL, check_decision_id = NULL \
             WHERE derivative_id = {} AND check_status IS NOT NULL",
            store.qualified("derivative_part"),
            d.param(1, Type::Int),
        ),
        &[Param::Int(id)],
    )
}

/// The labels of a set that a result holds, of those asked.
pub fn labels_held(
    store: &mut Store,
    id: i64,
    set: &str,
    labels: &[i64],
) -> Result<std::collections::BTreeSet<i64>, Error> {
    let d = store.dialect();
    let mut out = std::collections::BTreeSet::new();
    for chunk in labels.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT label FROM {} WHERE derivative_id = {} AND part_set = {} AND label IN ({list})",
            store.qualified("derivative_part"),
            d.param(1, Type::Int),
            d.param(2, Type::Text),
        );
        for r in store.query(&sql, &[Param::Int(id), Param::from(set)])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

/// How many rows and bytes the registry names, for custody.
pub fn totals(store: &mut Store) -> Result<(i64, i64), Error> {
    let sql = format!(
        "SELECT COUNT(*), CAST(COALESCE(SUM(bytes), 0) AS BIGINT) FROM {}",
        store.qualified("derivative")
    );
    let r = store
        .query_opt(&sql, &[])?
        .ok_or_else(|| Error::Message("no count".into()))?;
    Ok((r.int(0)?, r.int(1)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::{self, Kind};

    fn store() -> Store {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate::migrate(&mut store, Kind::Registry).unwrap();
        store
    }

    #[test]
    fn a_derivative_belongs_to_exactly_one_thing_that_exists() {
        let mut store = store();
        let refused = |r: Result<Belongs, Result<String, Error>>| match r {
            Err(Ok(m)) => m,
            other => panic!("not a refusal: {other:?}"),
        };
        let m = refused(belongs(&mut store, None, None, None, None));
        assert!(m.contains("exactly one"), "{m}");
        let m = refused(belongs(&mut store, Some(1), Some(2), None, None));
        assert!(m.contains("exactly one"), "{m}");
        let m = refused(belongs(&mut store, Some(7), None, None, None));
        assert_eq!(m, "no stack 7");
        let m = refused(belongs(&mut store, None, Some(7), None, Some("2020-01-01")));
        assert!(m.contains("with the subject"), "{m}");
    }

    #[test]
    fn a_row_reads_back_as_it_was_written_and_lists_newest_first() {
        let mut store = store();
        let b = Belongs {
            scope: "session".into(),
            stack_id: None,
            series_id: None,
            subject_id: Some(3),
            session_day: Some("2021-02-03".into()),
        };
        let actor = serde_json::json!({"kind": "person"});
        let n = New {
            kind: "mask",
            belongs: &b,
            place_id: 1,
            path: "derivatives/mask/ab/abcd",
            bytes: 12,
            sha256: "abcd",
            media_type: "application/octet-stream",
            registered_by: "ana@node",
            actor: Some(&actor),
            model_id: None,
            run_id: None,
            preprocess_version: None,
            supersedes_id: None,
            created_at: "2026-09-24T00:00:00Z",
        };
        // a model named is one the model table holds
        let e = insert(
            &mut store,
            &New {
                model_id: Some(42),
                ..n.clone()
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no registered model 42"), "{e}");
        let first = insert(&mut store, &n).unwrap();
        let second = insert(
            &mut store,
            &New {
                supersedes_id: Some(first),
                ..n.clone()
            },
        )
        .unwrap();
        let got = get(&mut store, first).unwrap().unwrap();
        assert_eq!(got.kind, "mask");
        assert_eq!(got.session_day.as_deref(), Some("2021-02-03"));
        assert_eq!(got.subject_id, Some(3));
        assert_eq!(got.actor, actor);
        assert_eq!(got.model_id, None);
        let all = list(
            &mut store,
            &Filter {
                subject_id: Some(3),
                limit: 10,
                ..Filter::default()
            },
        )
        .unwrap();
        assert_eq!(
            all.iter().map(|d| d.id).collect::<Vec<_>>(),
            [second, first]
        );
        assert_eq!(all[0].supersedes_id, Some(first));
        assert_eq!(totals(&mut store).unwrap(), (2, 24));
        assert!(get(&mut store, 99).unwrap().is_none());
    }
}
