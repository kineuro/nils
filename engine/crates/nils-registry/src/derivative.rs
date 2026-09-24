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

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};

/// The kinds a derivative may be. A mask is an uploaded segmentation, an
/// embedding one encoder's vectors for one stack, a pyramid the viewer's
/// tiles (wave 43 moves them here), an output anything else a pipeline
/// wrote. Record 43 adds two a run writes and a person does not: seeds,
/// the suggestions a run made for a person to curate, and model, the
/// artifact of a model a run fitted and the registry registered.
pub const KINDS: [&str; 6] = ["mask", "embedding", "pyramid", "output", "seeds", "model"];

/// The kinds only a pipeline run writes (record 43).
pub const RUN_KINDS: [&str; 2] = ["seeds", "model"];

/// Where the files of registered derivatives go under a working place.
pub const TREE: &str = "derivatives";

/// What a derivative belongs to, resolved against the registry: the scope
/// and its ids, the subject filled for every scope but a run's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Belongs {
    /// `stack`, `series`, `session`, `subject`, or `run` for a file that is
    /// a whole run's rather than any subject's (record 43: seeds, a model).
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
}

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
    if n.belongs.subject_id.is_none() && (n.belongs.scope != "run" || run_id.is_none()) {
        return Err(Error::Message(
            "a derivative belongs to a subject, or is a pipeline run's own file".into(),
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

const COLUMNS: [&str; 20] = [
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
    })
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

/// What a listing narrows to.
#[derive(Debug, Clone, Default)]
pub struct Filter<'a> {
    pub kind: Option<&'a str>,
    pub stack_id: Option<i64>,
    pub subject_id: Option<i64>,
    /// Record 43: only the files one pipeline run made.
    pub run_id: Option<i64>,
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
    sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", f.limit.max(1)));
    store.query(&sql, &params)?.iter().map(of).collect()
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
