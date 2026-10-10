// SPDX-License-Identifier: AGPL-3.0-only

//! The linkage doors (record 26, decisions 4, 5, 6 and 15): the identifier
//! types, the map in any shape with its dry run, the held files of a
//! dataset by shape, and the merge. What reads an identifier (the map
//! applied, the held reveal, the merge) needs `data:work` and detail
//! sensitive at the door, as record 25 set for jobs; the dry run's report
//! and the held list hold no identifier and need `data:work` and
//! `data:see`. The grants are in the door table of `serve`, like every
//! other door's; the apply checks its detail here, since the same door
//! answers a dry run at plain.
//!
//! Wave 7a, the pseudonymise step of a dataset (2026-10-09): the held
//! identifiers one row each, by shape and never by value, with their files
//! and how each stands (no code yet, a code a map gave, a code to be
//! generated, a subject waiting for a value), so a page lists them and
//! fills each with its code as a map is rehearsed; the codes a rehearsal
//! for a dataset gives its held identifiers; chosen identifiers coded
//! anyway, with or without the run queued; and the reveal naming each
//! identifier by the same row. A row's id stands for its identifier: the
//! first held row of the dataset that carries it, a technical key.

use std::collections::{BTreeSet, HashMap};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use nils_registry::identity_map::{self, Column, Derive, Map, Role, Row};
use nils_registry::linkage::{self, Subkeys};
use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};
use nils_registry::{Home, Registry, migrate, place};

use crate::grants::{Detail, Need};
use crate::serve::{Caller, Reply, json_body, queued_by};

/// The doors this module answers, for the capabilities.
pub(crate) const DOORS: &[&str] = &[
    "GET /api/linkage/types",
    "POST /api/linkage/types",
    "POST /api/linkage/imports",
    "GET /api/linkage/held",
    "GET /api/linkage/held/ids",
    "POST /api/linkage/held/code",
    "POST /api/linkage/held/reveal",
    "POST /api/linkage/merge",
];

/// The most rows one call to the imports door takes; a larger map goes in
/// as a file under a registered location through `POST /api/jobs`.
pub(crate) const MAX_ROWS: usize = 100_000;

/// The table the pseudonymiser records every file in, declared by the
/// dataset slice; the held doors answer nothing until it is there.
const HELD_TABLE: &str = "pseudonym_file";

/// Where the imports door leaves the map for its job: under the registry
/// home, the directory 700 and the file 600, removed by the job.
const IMPORTS_DIR: &str = "imports";

/// The doors under `/api/linkage`, or nothing when the path is not one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn route(
    home: &Home,
    registry: &mut Registry,
    caller: &Caller,
    method: &str,
    segs: &[&str],
    query: &HashMap<String, String>,
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let get = method == "GET";
    let post = method == "POST";
    Some(match segs {
        ["api", "linkage", "types"] if get => types(registry),
        ["api", "linkage", "types"] if post => add_type(registry, body),
        ["api", "linkage", "imports"] if post => imports(home, registry, caller, body),
        ["api", "linkage", "held"] if get => held(registry, query.get("place").map(String::as_str)),
        ["api", "linkage", "held", "ids"] if get => {
            held_ids(registry, caller, query.get("place").map(String::as_str))
        }
        ["api", "linkage", "held", "code"] if post => held_code(registry, caller, body),
        ["api", "linkage", "held", "reveal"] if post => held_reveal(registry, caller, body),
        ["api", "linkage", "merge"] if post => merge(registry, caller, body),
        _ => return None,
    })
}

fn open_linkage(registry: &Registry) -> Result<Store, Reply> {
    registry
        .open_linkage()
        .map_err(|e| Reply::error(500, e.to_string()))
}

fn keys_of(registry: &Registry) -> Result<(Vec<u8>, Subkeys), Reply> {
    let key = registry
        .pseudonym_key()
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let keys = Subkeys::derive(&key);
    Ok((key, keys))
}

/// `GET /api/linkage/types`: every type with its counts.
fn types(registry: &mut Registry) -> Result<Reply, Reply> {
    let mut store = open_linkage(registry)?;
    let types = linkage::id_type_counts(&mut store)?;
    Ok(Reply::ok(serde_json::Value::from(
        types.iter().map(|t| t.as_json()).collect::<Vec<_>>(),
    )))
}

/// `POST /api/linkage/types {name, description}`: made once, found after.
fn add_type(registry: &mut Registry, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let name = doc["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| {
            Reply::error(
                400,
                "name: lower case letters, digits and hyphens, like study-id",
            )
        })?;
    let description = doc["description"].as_str().map(str::trim);
    let mut store = open_linkage(registry)?;
    let (t, made) = linkage::ensure_id_type(&mut store, name, description)
        .map_err(|e| Reply::error(400, e.to_string()))?;
    let counts = linkage::id_type_counts(&mut store)?;
    let mut answer = counts
        .into_iter()
        .find(|c| c.id == t.id)
        .map(|c| c.as_json())
        .unwrap_or_else(
            || serde_json::json!({ "id": t.id, "name": t.name, "description": t.description }),
        );
    answer["created"] = serde_json::Value::Bool(made);
    Ok(if made {
        Reply::created(answer)
    } else {
        Reply::ok(answer)
    })
}

/// The columns a body names: `[{header, role, id_type?}]`.
fn columns_of(doc: &serde_json::Value) -> Result<Vec<Column>, Reply> {
    let given = doc["columns"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            Reply::error(
                400,
                "columns: [{header, role: identifier|canonical|code|ignore, id_type?}]",
            )
        })?;
    let mut columns = Vec::with_capacity(given.len());
    let mut headers: Vec<&str> = Vec::with_capacity(given.len());
    for (i, c) in given.iter().enumerate() {
        let header = c["header"]
            .as_str()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .ok_or_else(|| Reply::error(400, format!("columns[{i}].header: the column's name")))?;
        if headers.contains(&header) {
            return Err(Reply::error(
                400,
                format!("columns[{i}].header: {header} names two columns"),
            ));
        }
        headers.push(header);
        let role = c["role"].as_str().unwrap_or("").trim();
        let text = match c["id_type"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(t) => format!("{role}:{t}"),
            None => role.to_string(),
        };
        let role =
            Role::parse(&text).map_err(|e| Reply::error(400, format!("columns[{i}]: {e}")))?;
        columns.push(Column {
            header: header.to_string(),
            role,
        });
    }
    Ok(columns)
}

/// The rows a body carries: arrays of cells, a cell a string, a number or
/// nothing. The cells never reach the reply.
fn rows_of(doc: &serde_json::Value, width: usize) -> Result<Vec<Row>, Reply> {
    let given = doc["rows"]
        .as_array()
        .ok_or_else(|| Reply::error(400, "rows: [[cell, ...], ...] in the columns' order"))?;
    if given.len() > MAX_ROWS {
        return Err(Reply::error(
            413,
            format!(
                "{} rows; the door takes {MAX_ROWS} in one call, a larger map goes in as a file under a registered location through POST /api/jobs",
                given.len()
            ),
        ));
    }
    let mut rows = Vec::with_capacity(given.len());
    for (i, r) in given.iter().enumerate() {
        let cells = r
            .as_array()
            .ok_or_else(|| Reply::error(400, format!("rows[{i}]: an array of cells")))?;
        if cells.len() > width {
            return Err(Reply::error(
                400,
                format!("rows[{i}]: {} cells for {width} columns", cells.len()),
            ));
        }
        let cells: Vec<String> = cells
            .iter()
            .map(|c| match c {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            })
            .collect();
        rows.push(Row { line: i + 2, cells });
    }
    Ok(rows)
}

/// A place by the name a body or a query gives, active.
fn place_named(registry: &mut Registry, name: &str) -> Result<place::Place, Reply> {
    place::by_name(registry.store(), name)?
        .filter(|p| p.retired_at.is_none())
        .ok_or_else(|| Reply::error(404, format!("no place named {name}")))
}

/// `POST /api/linkage/imports`: a dry run answers the report here; an
/// import is a `linkage import` job over a file this door writes.
fn imports(
    home: &Home,
    registry: &mut Registry,
    caller: &Caller,
    body: &str,
) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let columns = columns_of(&doc)?;
    let rows = rows_of(&doc, columns.len())?;
    let dry_run = doc["dry_run"].as_bool().unwrap_or(false);
    let make_types = doc["make_types"].as_bool().unwrap_or(false);
    let place = match doc["place"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(name) => Some(place_named(registry, name)?),
        None => None,
    };
    if dry_run {
        let (key, keys) = keys_of(registry)?;
        let derive = Derive {
            scheme: registry.meta().pseudonym_scheme,
            key: &key,
            display_length: registry.meta().display_length,
        };
        let mut linkage = open_linkage(registry)?;
        let report = identity_map::import(
            registry.store(),
            &mut linkage,
            &keys,
            Some(&derive),
            &Map {
                columns: &columns,
                rows: &rows,
                dry_run: true,
                make_types,
                place_id: place.as_ref().map(|p| p.id),
                actor: &caller.principal,
                job_id: None,
            },
        )
        .map_err(|e| Reply::error(400, e.to_string()))?;
        let mut answer = report.as_json();
        // Wave 7a: rehearsed for a dataset, the codes its held identifiers
        // would get, each by the row that stands for it
        if let Some(p) = &place {
            answer["held_ids"] = rehearsed_ids(registry, caller, p, &report.held_codes)?;
        }
        return Ok(Reply::ok(answer));
    }
    // the map is read by the job: sensitive, as record 25 set for jobs
    caller.allowed(
        "POST /api/linkage/imports",
        Need::One("data:work"),
        Detail::Sensitive,
    )?;
    // Wave 7a (Nima, 2026-10-08): a personnummer is never saved anywhere
    // in the registry, a job's map file included. A map with a personnummer
    // column is imported here and now, from the request alone, and nothing
    // of it touches the disk but what the import files: the keyed lookup.
    let numbers = columns.iter().any(|c| {
        c.role
            .id_type()
            .is_some_and(nils_registry::personnummer::is_type)
    });
    if numbers {
        let (key, keys) = keys_of(registry)?;
        let derive = Derive {
            scheme: registry.meta().pseudonym_scheme,
            key: &key,
            display_length: registry.meta().display_length,
        };
        let mut linkage = open_linkage(registry)?;
        let report = identity_map::import(
            registry.store(),
            &mut linkage,
            &keys,
            Some(&derive),
            &Map {
                columns: &columns,
                rows: &rows,
                dry_run: false,
                make_types,
                place_id: place.as_ref().map(|p| p.id),
                actor: &caller.principal,
                job_id: None,
            },
        )
        .map_err(|e| Reply::error(400, e.to_string()))?;
        if report.written() {
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal: &caller.principal,
                    action: nils_registry::audit::Action::LinkageImport,
                    scope: serde_json::json!({
                        "rows": report.rows,
                        "place": place.as_ref().map(|p| p.id),
                        "held_released": report.held_released,
                    }),
                    policy: None,
                    job_id: None,
                    details: Some(
                        serde_json::json!({ "inline": "a column of IDs that are the same everywhere" }),
                    ),
                },
            )?;
        }
        registry
            .refresh_meta()
            .map_err(|e| Reply::error(500, e.to_string()))?;
        let mut answer = report.as_json();
        answer["job"] = serde_json::Value::Null;
        answer["state"] = serde_json::json!(if report.conflicts.is_empty() {
            "done"
        } else {
            "refused"
        });
        let status = if report.conflicts.is_empty() {
            200
        } else {
            422
        };
        let mut reply = Reply::ok(answer);
        reply.status = status;
        return Ok(reply);
    }
    let path = write_map(home, &columns, &rows).map_err(|e| Reply::error(500, e))?;
    let mut command = vec![
        "linkage".to_string(),
        "import".to_string(),
        path.display().to_string(),
    ];
    for c in &columns {
        let role = match c.role.id_type() {
            Some(t) => format!("{}:{t}", c.role.name()),
            None => c.role.name().to_string(),
        };
        command.extend(["--column".to_string(), format!("{}={role}", c.header)]);
    }
    if make_types {
        command.push("--make-types".to_string());
    }
    if let Some(p) = &place {
        command.extend(["--place".to_string(), p.name.clone()]);
    }
    command.push("--consume".to_string());
    let queued = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(
            place
                .as_ref()
                .map(|p| p.name.as_str())
                .unwrap_or("identifier map"),
        ),
        Some(&caller.principal),
        queued_by(caller),
    );
    match queued {
        Ok(id) => Ok(Reply::accepted(
            serde_json::json!({ "job": id, "state": "queued", "rows": rows.len() }),
        )),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(Reply::error(500, e.to_string()))
        }
    }
}

static WRITTEN: AtomicUsize = AtomicUsize::new(0);

/// The map as a CSV the job reads and removes: under the registry home,
/// the directory 700, the file 600 and new.
fn write_map(home: &Home, columns: &[Column], rows: &[Row]) -> Result<PathBuf, String> {
    let dir = home.dir().join(IMPORTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let n = WRITTEN.fetch_add(1, Ordering::SeqCst);
    let path = dir.join(format!(
        "map-{}-{}-{n}.csv",
        nils_registry::time::now_secs(),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = csv::Writer::from_writer(file);
    let written = (|| -> Result<(), csv::Error> {
        writer.write_record(columns.iter().map(|c| c.header.as_str()))?;
        for r in rows {
            let mut cells: Vec<&str> = r.cells.iter().map(String::as_str).collect();
            cells.resize(columns.len(), "");
            writer.write_record(&cells)?;
        }
        writer.flush()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{}: {e}", path.display()));
    }
    let mut file = writer
        .into_inner()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.flush()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Whether the pseudonymiser's table is there to read.
fn held_table(registry: &mut Registry) -> Result<bool, Reply> {
    Ok(migrate::table_exists(registry.store(), HELD_TABLE)?)
}

/// `GET /api/linkage/held?place=`: the held files of a dataset by shape.
fn held(registry: &mut Registry, place: Option<&str>) -> Result<Reply, Reply> {
    let name = place
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Reply::error(400, "place: the dataset's name"))?;
    let p = place_named(registry, name)?;
    if !held_table(registry)? {
        return Ok(Reply::ok(serde_json::json!([])));
    }
    let store = registry.store();
    let d = store.dialect();
    // the stamp as text, whatever type the column has
    let sql = format!(
        "SELECT shape, id_type, COUNT(*), CAST(MIN(first_seen) AS TEXT), MIN(batch_id), wants_type FROM {} \
         WHERE place_id = {} AND state = 'held' AND released_at IS NULL \
         GROUP BY shape, id_type, wants_type ORDER BY shape, id_type, wants_type",
        store.qualified(HELD_TABLE),
        d.param(1, Type::Int)
    );
    let rows = store.query(&sql, &[Param::Int(p.id)])?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(serde_json::json!({
            "shape": r.opt_text(0)?,
            "id_type": r.opt_text(1)?,
            "files": r.int(2)?,
            "first_seen": r.opt_text(3)?,
            "batch": r.opt_int(4)?,
            // Wave 7a §5.4: the id type the files wait for their subject to
            // have a value of; null for files whose subject is unknown
            "waits_for": r.opt_text(5)?,
        }));
    }
    Ok(Reply::ok(serde_json::Value::from(out)))
}

/// What `ids` is, said when a body gives it otherwise.
const IDS_ARE: &str =
    "ids: the held identifiers, each by the id the held ids door gives it; leave it out for all";

fn no_held_id(place: &str, id: i64) -> Reply {
    Reply::error(404, format!("{place} holds no identifier {id}"))
}

/// One held identifier of a dataset (Wave 7a): the row that stands for it
/// (the first not yet released, else the first), the first row that
/// carries it, its files, its keyed lookup, its shape and type, and how it
/// stands. Never a value.
pub(crate) struct HeldId {
    pub(crate) id: i64,
    pub(crate) first: i64,
    pub(crate) files: i64,
    pub(crate) lookup: Option<Vec<u8>>,
    pub(crate) shape: Option<String>,
    pub(crate) id_type: Option<String>,
    pub(crate) first_seen: Option<String>,
    pub(crate) batch: Option<i64>,
    /// A map named it: its files go at the next run, under the map's code.
    pub(crate) released: bool,
    /// A person asked for it to be coded anyway at the next run.
    pub(crate) generated: bool,
    /// Its subject is known and has no value of this type yet.
    pub(crate) subject: Option<i64>,
    pub(crate) waits_for: Option<String>,
}

impl HeldId {
    /// How it stands, in one word: waits (for a value), mapped, generated,
    /// or held (no code yet).
    fn state(&self) -> &'static str {
        if self.waits_for.is_some() {
            "waits"
        } else if self.released {
            "mapped"
        } else if self.generated {
            "generated"
        } else {
            "held"
        }
    }
}

/// The held identifiers of a dataset, ordered by the first row that
/// carries each: the rows grouped by the keyed lookup they carry, in the
/// store, so a dataset of millions of held files answers one row an
/// identifier; a row that carries no lookup stands alone.
pub(crate) fn held_identifiers(
    store: &mut Store,
    place_id: i64,
) -> Result<Vec<HeldId>, nils_registry::store::Error> {
    let d = store.dialect();
    let table = store.qualified(HELD_TABLE);
    let place = d.param(1, Type::Int);
    let grouped = format!(
        "SELECT MIN(id), MIN(CASE WHEN released_at IS NULL THEN id END), COUNT(*), COUNT(released_at), \
         lookup, MIN(shape), MIN(id_type), CAST(MIN(first_seen) AS TEXT), MIN(batch_id), MAX(code_anyway), \
         MIN(subject_id), MIN(wants_type) FROM {table} \
         WHERE place_id = {place} AND state = 'held' AND lookup IS NOT NULL GROUP BY lookup"
    );
    let alone = format!(
        "SELECT id, CASE WHEN released_at IS NULL THEN id END, 1, CASE WHEN released_at IS NULL THEN 0 ELSE 1 END, \
         lookup, shape, id_type, CAST(first_seen AS TEXT), batch_id, code_anyway, subject_id, wants_type FROM {table} \
         WHERE place_id = {place} AND state = 'held' AND lookup IS NULL"
    );
    let mut out = Vec::new();
    for sql in [grouped, alone] {
        for r in store.query(&sql, &[Param::Int(place_id)])? {
            let first = r.int(0)?;
            let files = r.int(2)?;
            out.push(HeldId {
                id: r.opt_int(1)?.unwrap_or(first),
                first,
                files,
                released: r.int(3)? == files,
                lookup: r.opt_bytes(4)?.map(<[u8]>::to_vec),
                shape: r.opt_text(5)?.map(str::to_string),
                id_type: r.opt_text(6)?.map(str::to_string),
                first_seen: r.opt_text(7)?.map(str::to_string),
                batch: r.opt_int(8)?,
                generated: r.int(9)? != 0,
                subject: r.opt_int(10)?,
                waits_for: r.opt_text(11)?.map(str::to_string),
            });
        }
    }
    out.sort_by_key(|h| h.first);
    Ok(out)
}

/// The codes of subjects by id, a merged subject answered by the one it
/// went into.
fn codes_of(
    registry: &mut Registry,
    ids: &[i64],
) -> Result<HashMap<i64, (i64, String)>, nils_registry::store::Error> {
    let found = linkage::subjects_by_id(registry.store(), ids)?;
    let into: Vec<i64> = found.iter().filter_map(|s| s.merged_into).collect();
    let canon = linkage::subjects_by_id(registry.store(), &into)?;
    Ok(found
        .into_iter()
        .map(|s| {
            let kept = s
                .merged_into
                .and_then(|m| canon.iter().find(|c| c.id == m))
                .unwrap_or(&s);
            (s.id, (kept.id, kept.code.clone()))
        })
        .collect())
}

/// The datasets each subject is in already (Wave 7a): the source places
/// whose roots hold a study of the subject, counted under the read that
/// brought the study first as the sources door counts it, and the
/// identified datasets whose pseudonymised files carry it. One subject
/// wherever the data came from.
fn datasets_holding(
    store: &mut Store,
    subjects: &[i64],
) -> Result<HashMap<i64, BTreeSet<String>>, nils_registry::store::Error> {
    let mut out: HashMap<i64, BTreeSet<String>> = HashMap::new();
    if subjects.is_empty() {
        return Ok(out);
    }
    let ids = subjects
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let places: Vec<place::Place> = place::list(store)?
        .into_iter()
        .filter(|p| p.role == place::Role::Source && p.retired_at.is_none())
        .collect();
    let mut by_source: HashMap<i64, Vec<String>> = HashMap::new();
    for p in &places {
        for s in crate::sources::source_ids(store, p)? {
            by_source.entry(s).or_default().push(p.name.clone());
        }
    }
    let sql = format!(
        "SELECT DISTINCT s.subject_id, b.source_id FROM {} s JOIN {} b ON b.id = s.first_batch_id \
         WHERE s.subject_id IN ({ids})",
        store.qualified("study"),
        store.qualified("ingest_batch"),
    );
    for r in store.query(&sql, &[])? {
        let names = by_source.get(&r.int(1)?).cloned().unwrap_or_default();
        out.entry(r.int(0)?).or_default().extend(names);
    }
    let sql = format!(
        "SELECT DISTINCT subject_id, place_id FROM {} WHERE subject_id IN ({ids}) \
         AND state IN ('written', 'unchanged')",
        store.qualified(HELD_TABLE),
    );
    for r in store.query(&sql, &[])? {
        let place = r.int(1)?;
        if let Some(p) = places.iter().find(|p| p.id == place) {
            out.entry(r.int(0)?).or_default().insert(p.name.clone());
        }
    }
    Ok(out)
}

/// A subject's code as a caller may read it: below detail quasi, its shape,
/// as the scans door answers it (record 55 K7).
fn code_shown(caller: &Caller, code: &str) -> serde_json::Value {
    if caller.access.detail >= Detail::Quasi {
        serde_json::Value::from(code)
    } else {
        serde_json::Value::from(crate::scans::shape(code))
    }
}

/// `GET /api/linkage/held/ids?place=` (Wave 7a, the pseudonymise step): the
/// held identifiers of a dataset one row each, by the row that stands for
/// it, with its shape, its type, its files, when it was first seen and how
/// it stands: `held` with no code yet, `mapped` with the code a map gave it
/// and its files waiting for the next run, `generated` to be coded anyway at
/// the next run, or `waits` with its subject's code and the type it waits
/// for a value of. A code is answered with the datasets its subject is in
/// already (`also_in`); below detail quasi as its shape. Beside them, the
/// subjects the dataset's pseudonymised files carry and how many of those
/// were coded without a map. Never a value, so `data:see` at plain.
fn held_ids(registry: &mut Registry, caller: &Caller, place: Option<&str>) -> Result<Reply, Reply> {
    let name = place
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Reply::error(400, "place: the dataset's name"))?;
    let p = place_named(registry, name)?;
    let mut doc = serde_json::json!({
        "place": p.name,
        "files": 0,
        "identifiers": 0,
        "ids": [],
        "subjects": {"coded": 0, "generated": 0},
    });
    if !held_table(registry)? {
        return Ok(Reply::ok(doc));
    }
    let held = held_identifiers(registry.store(), p.id)?;
    // the codes: a mapped identifier's through the identity the map filed,
    // a waiting one's through its subject
    let mapped: Vec<Vec<u8>> = held
        .iter()
        .filter(|h| h.state() == "mapped")
        .filter_map(|h| h.lookup.clone())
        .collect();
    let mut linkage = open_linkage(registry)?;
    let identities: HashMap<Vec<u8>, i64> = linkage::identities_by_lookup(&mut linkage, &mapped)?
        .into_iter()
        .map(|i| (i.lookup, i.subject_id))
        .collect();
    let subject_of = |h: &HeldId| -> Option<i64> {
        match h.state() {
            "waits" => h.subject,
            "mapped" => h.lookup.as_ref().and_then(|l| identities.get(l).copied()),
            _ => None,
        }
    };
    let subjects: Vec<i64> = held
        .iter()
        .filter_map(subject_of)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let codes = codes_of(registry, &subjects)?;
    let kept: Vec<i64> = codes
        .values()
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let holding = datasets_holding(registry.store(), &kept)?;
    let files: i64 = held.iter().map(|h| h.files).sum();
    let ids: Vec<serde_json::Value> = held
        .iter()
        .map(|h| {
            let code = subject_of(h).and_then(|s| codes.get(&s));
            let also: Vec<&String> = code
                .and_then(|(s, _)| holding.get(s))
                .map(|names| names.iter().filter(|n| **n != p.name).collect())
                .unwrap_or_default();
            serde_json::json!({
                "id": h.id,
                "shape": h.shape,
                "id_type": h.id_type,
                "files": h.files,
                "first_seen": h.first_seen,
                "batch": h.batch,
                "state": h.state(),
                "code": code.map(|(_, c)| code_shown(caller, c)),
                "also_in": also,
                "waits_for": h.waits_for,
            })
        })
        .collect();
    // the subjects its pseudonymised files carry, and how many of them were
    // coded without a map
    let store = registry.store();
    let sql = format!(
        "SELECT COUNT(DISTINCT f.subject_id), COUNT(DISTINCT CASE WHEN s.provisional = 1 THEN f.subject_id END) \
         FROM {} f JOIN {} s ON s.id = f.subject_id \
         WHERE f.place_id = {} AND f.state IN ('written', 'unchanged')",
        store.qualified(HELD_TABLE),
        store.qualified("subject"),
        store.dialect().param(1, Type::Int)
    );
    let row = &store.query(&sql, &[Param::Int(p.id)])?[0];
    doc["files"] = serde_json::json!(files);
    doc["identifiers"] = serde_json::json!(held.len());
    doc["ids"] = serde_json::Value::from(ids);
    doc["subjects"] = serde_json::json!({"coded": row.int(0)?, "generated": row.int(1)?});
    Ok(Reply::ok(doc))
}

/// The codes a rehearsed map gives a dataset's held identifiers (Wave 7a):
/// each identifier it names whose files still wait, by the row that stands
/// for it, with the code and the datasets that code's subject is in
/// already; below detail quasi the code as its shape.
fn rehearsed_ids(
    registry: &mut Registry,
    caller: &Caller,
    p: &place::Place,
    codes: &[identity_map::HeldCode],
) -> Result<serde_json::Value, Reply> {
    if codes.is_empty() || !held_table(registry)? {
        return Ok(serde_json::json!([]));
    }
    let by_lookup: HashMap<&[u8], &str> = codes
        .iter()
        .map(|c| (c.lookup.as_slice(), c.code.as_str()))
        .collect();
    let held = held_identifiers(registry.store(), p.id)?;
    // the identifiers whose files still wait: one a map released before is
    // mapped already, whatever another dataset still holds of it
    let named: Vec<(&HeldId, &str)> = held
        .iter()
        .filter(|h| !h.released)
        .filter_map(|h| {
            h.lookup
                .as_deref()
                .and_then(|l| by_lookup.get(l))
                .map(|c| (h, *c))
        })
        .collect();
    let distinct: Vec<String> = named
        .iter()
        .map(|(_, c)| c.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // a code already a subject's, by the subject it stands for now
    let known: HashMap<String, i64> = linkage::subjects_by_code(registry.store(), &distinct)?
        .into_iter()
        .map(|s| (s.code, s.merged_into.unwrap_or(s.id)))
        .collect();
    let ids: Vec<i64> = known.values().copied().collect();
    let holding = datasets_holding(registry.store(), &ids)?;
    Ok(serde_json::Value::from(
        named
            .iter()
            .map(|(h, code)| {
                let also: Vec<&String> = known
                    .get(*code)
                    .and_then(|s| holding.get(s))
                    .map(|names| names.iter().filter(|n| **n != p.name).collect())
                    .unwrap_or_default();
                serde_json::json!({ "id": h.id, "code": code_shown(caller, code), "also_in": also })
            })
            .collect::<Vec<_>>(),
    ))
}

/// The keyed lookups the rows named in `ids` carry, each read by its row in
/// the store: a row that is no held row of the dataset is refused by its
/// number, and a row with no lookup has nothing to code by.
fn chosen_lookups(store: &mut Store, p: &place::Place, ids: &[i64]) -> Result<Vec<Vec<u8>>, Reply> {
    let mut found: HashMap<i64, Option<Vec<u8>>> = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(nils_registry::store::SQLITE_KEY_CHUNK) {
        // the rows are numbers the body gave as integers, set in the
        // statement as they are
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, lookup FROM {} WHERE place_id = {} AND state = 'held' AND id IN ({list})",
            store.qualified(HELD_TABLE),
            store.dialect().param(1, Type::Int)
        );
        for r in store.query(&sql, &[Param::Int(p.id)])? {
            found.insert(r.int(0)?, r.opt_bytes(1)?.map(<[u8]>::to_vec));
        }
    }
    let mut lookups: BTreeSet<Vec<u8>> = BTreeSet::new();
    for id in ids {
        match found.get(id) {
            None => return Err(no_held_id(&p.name, *id)),
            Some(lookup) => lookups.extend(lookup.clone()),
        }
    }
    Ok(lookups.into_iter().collect())
}

/// `POST /api/linkage/held/code {place, ids?, run?}`: code the held files
/// anyway, and queue the run that does (Wave 7a §5.4): the pseudonymise of
/// the held files of an identified dataset, the digest of a dataset read in
/// place. The answer names the job, `{place, files, job, state}`; with
/// nothing new to code, no job is queued and `job` is null, so a second
/// press queues no second run. A file held for want of its subject's id
/// type value is not coded anyway: its subject is known, and it waits for
/// the value, which a map gives.
///
/// Wave 7a, the pseudonymise step: `ids` names the identifiers to code by
/// the rows the held ids door gives them, every one of the dataset where it
/// is left out; and `run: false` marks them and queues nothing, for the
/// next run of the dataset's own to code them (`state` marked).
fn held_code(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let name = doc["place"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Reply::error(400, "place: the dataset's name"))?;
    let chosen: Option<Vec<i64>> = match &doc["ids"] {
        serde_json::Value::Null => None,
        serde_json::Value::Array(list) if !list.is_empty() => Some(
            list.iter()
                .map(serde_json::Value::as_i64)
                .collect::<Option<Vec<i64>>>()
                .ok_or_else(|| Reply::error(400, IDS_ARE))?,
        ),
        _ => return Err(Reply::error(400, IDS_ARE)),
    };
    let run = match &doc["run"] {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => *b,
        _ => return Err(Reply::error(400, "run: true or false")),
    };
    let p = place_named(registry, name)?;
    if let Some(why) = crate::dataset::undeclared_refusal(&p, registry.meta().pseudonym_scheme) {
        return Err(Reply::error(409, why));
    }
    // The job a run queues is held to what its verb needs at the jobs door,
    // checked before anything is marked: the pseudonymiser reads the
    // identifiers it replaces, so queueing it needs detail sensitive (the
    // review of Wave 7a's merge, 2026-10-10); `run: false` only marks.
    let verb = if p.dataset["arrives"].as_str() == Some("identified") {
        "pseudonymize"
    } else {
        "digest"
    };
    if run && let Some((grant, detail)) = crate::serve::verb_needs(&[verb.to_string()]) {
        caller.allowed(
            &format!("POST /api/linkage/held/code with run, which queues {verb}"),
            Need::One(grant),
            detail,
        )?;
    }
    let nothing = || {
        Ok(Reply::ok(serde_json::json!({
            "place": p.name, "files": 0, "job": null, "state": "nothing new to code",
        })))
    };
    if !held_table(registry)? {
        return match chosen {
            Some(ids) => Err(no_held_id(&p.name, ids[0])),
            None => nothing(),
        };
    }
    // the identifiers chosen, by the lookup each row carries
    let lookups = match &chosen {
        None => None,
        Some(ids) => Some(chosen_lookups(registry.store(), &p, ids)?),
    };
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET code_anyway = 1 WHERE place_id = {} AND state = 'held' \
         AND released_at IS NULL AND code_anyway = 0 AND wants_type IS NULL",
        store.qualified(HELD_TABLE),
        d.param(1, Type::Int)
    );
    let n = match lookups {
        None => store.execute(&sql, &[Param::Int(p.id)])?,
        Some(lookups) => {
            // a chunk of lookups a statement, so the dataset's held rows are
            // read once a chunk, never once an identifier
            let mut n = 0;
            for chunk in lookups.chunks(nils_registry::store::SQLITE_KEY_CHUNK) {
                let marks: Vec<String> = (0..chunk.len())
                    .map(|i| d.param(i + 2, Type::Bytes))
                    .collect();
                let mut params = vec![Param::Int(p.id)];
                params.extend(chunk.iter().map(|l| Param::Bytes(l.clone())));
                n += store.execute(
                    &format!("{sql} AND lookup IN ({})", marks.join(", ")),
                    &params,
                )?;
            }
            n
        }
    };
    if n == 0 {
        return nothing();
    }
    if !run {
        return Ok(Reply::ok(serde_json::json!({
            "place": p.name, "files": n, "job": null, "state": "marked",
        })));
    }
    let at = format!("@{}", p.name);
    let command: Vec<String> = if verb == "pseudonymize" {
        vec![verb.into(), at, "--held".into()]
    } else {
        vec![verb.into(), at]
    };
    let job = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some(&p.name),
        Some(&caller.principal),
        crate::serve::queued_by(caller),
    )
    .map_err(crate::serve::job_err)?;
    Ok(Reply::accepted(serde_json::json!({
        "place": p.name,
        "files": n,
        "job": job,
        "state": "queued",
        "command": command,
    })))
}

/// One held identifier revealed: the value, the first held row that
/// carries it (for the read audit) and how many files do.
struct Revealed {
    value: String,
    row: i64,
    files: i64,
}

/// The held identifiers of one shape and type.
struct Group {
    shape: String,
    id_type: String,
    values: Vec<Revealed>,
}

/// `POST /api/linkage/held/reveal {place}`: the identifiers themselves,
/// grouped by shape, one read audit row per identifier.
fn held_reveal(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let name = doc["place"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Reply::error(400, "place: the dataset's name"))?;
    let p = place_named(registry, name)?;
    if !held_table(registry)? {
        return Ok(Reply::ok(serde_json::json!([])));
    }
    let (_, keys) = keys_of(registry)?;
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id, shape, id_type, sealed FROM {} \
         WHERE place_id = {} AND state = 'held' AND released_at IS NULL ORDER BY shape, id_type, id",
        store.qualified(HELD_TABLE),
        d.param(1, Type::Int)
    );
    let rows = store.query(&sql, &[Param::Int(p.id)])?;
    // by shape and type: the distinct identifiers, and the files each holds
    let mut groups: Vec<Group> = Vec::new();
    for r in &rows {
        let id = r.int(0)?;
        let shape = r.opt_text(1)?.unwrap_or("").to_string();
        let id_type = r.opt_text(2)?.unwrap_or("").to_string();
        let Some(sealed) = r.opt_bytes(3)? else {
            continue;
        };
        let value = keys.open(sealed)?;
        let group = match groups
            .iter_mut()
            .find(|g| g.shape == shape && g.id_type == id_type)
        {
            Some(g) => g,
            None => {
                groups.push(Group {
                    shape: shape.clone(),
                    id_type: id_type.clone(),
                    values: Vec::new(),
                });
                groups.last_mut().expect("just pushed")
            }
        };
        match group.values.iter_mut().find(|v| v.value == value) {
            Some(entry) => entry.files += 1,
            None => group.values.push(Revealed {
                value,
                row: id,
                files: 1,
            }),
        }
    }
    // one read audit row per identifier revealed: the held row's id stands
    // where an identity's would, since a held identifier has no identity
    let revealed: usize = groups.iter().map(|g| g.values.len()).sum();
    if revealed > 0 {
        let mut linkage = open_linkage(registry)?;
        let now = nils_registry::time::now_iso();
        let audit: Vec<Vec<Param>> = groups
            .iter()
            .flat_map(|g| g.values.iter())
            .map(|v| {
                vec![
                    Param::from(now.as_str()),
                    Param::from(caller.principal.as_str()),
                    Param::Int(v.row),
                    Param::from("held reveal"),
                ]
            })
            .collect();
        linkage.begin()?;
        let written = linkage.insert(
            &nils_registry::Insert::all(nils_registry::schema::table("read_audit")),
            &audit,
        );
        match written {
            Ok(_) => linkage.commit()?,
            Err(e) => {
                let _ = linkage.rollback();
                return Err(e.into());
            }
        }
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: &caller.principal,
                action: nils_registry::audit::Action::LinkageReveal,
                scope: serde_json::json!({ "place": p.id, "held": true, "identifiers": revealed }),
                policy: None,
                job_id: None,
                details: Some(serde_json::json!({ "why": "held reveal" })),
            },
        )?;
    }
    let out: Vec<serde_json::Value> = groups
        .into_iter()
        .map(|g| {
            let files: i64 = g.values.iter().map(|v| v.files).sum();
            // Wave 7a: each identifier by the row the held ids door gives it,
            // so a page shows the value in that row's place
            serde_json::json!({
                "shape": g.shape,
                "id_type": g.id_type,
                "files": files,
                "identifiers": g.values.iter().map(|v| serde_json::json!({ "id": v.row, "value": v.value, "files": v.files })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Reply::ok(serde_json::Value::from(out)))
}

/// `POST /api/linkage/merge {canonical, alias, why}`: a `linkage merge`
/// job, refused here when the codes are not two subjects.
fn merge(registry: &mut Registry, caller: &Caller, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let code = |key: &str| -> Result<String, Reply> {
        doc[key]
            .as_str()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .ok_or_else(|| Reply::error(400, format!("{key}: a subject's code")))
    };
    let canonical = code("canonical")?;
    let alias = code("alias")?;
    let why = doc["why"]
        .as_str()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .ok_or_else(|| Reply::error(400, "why: what shows they are one person"))?;
    if canonical == alias {
        return Err(Reply::error(
            400,
            "canonical and alias name the same subject",
        ));
    }
    let found = linkage::subjects_by_code(registry.store(), &[canonical.clone(), alias.clone()])?;
    for c in [&canonical, &alias] {
        match found.iter().find(|s| s.code == *c) {
            None => return Err(Reply::gated(404, format!("no subject with code {c}"))),
            Some(s) if s.merged_into.is_some() => {
                return Err(Reply::gated(409, format!("subject {c} was merged already")));
            }
            Some(_) => {}
        }
    }
    let command = vec![
        "linkage".to_string(),
        "merge".to_string(),
        canonical,
        alias,
        "--why".to_string(),
        why.to_string(),
    ];
    let id = nils_registry::job::enqueue_with(
        registry.store(),
        &command,
        Some("merge"),
        Some(&caller.principal),
        queued_by(caller),
    )
    .map_err(|e| Reply::error(500, e.to_string()))?;
    Ok(Reply::accepted(
        serde_json::json!({ "job": id, "state": "queued" }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grants::Access;
    use nils_dicom::synth::TempDir;

    /// The door table: what each door needs.
    #[test]
    fn the_linkage_doors_need_their_grants_and_detail() {
        let need = |method: &str, path: &str| {
            let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
            let (need, detail) = crate::serve::door(method, &segs);
            (need.words(), detail)
        };
        assert_eq!(
            need("GET", "/api/linkage/types"),
            ("the data:see grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("GET", "/api/linkage/held"),
            ("the data:see grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("GET", "/api/linkage/held/ids"),
            ("the data:see grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("POST", "/api/linkage/types"),
            ("the data:work grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("POST", "/api/linkage/imports"),
            ("the data:work grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("POST", "/api/linkage/held/code"),
            ("the data:work grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("POST", "/api/linkage/held/reveal"),
            ("the data:work grant".to_string(), Detail::Sensitive)
        );
        assert_eq!(
            need("POST", "/api/linkage/merge"),
            ("the data:work grant".to_string(), Detail::Sensitive)
        );
        // every door here is in the capabilities and the policy
        let policy = crate::serve::policy();
        for d in DOORS {
            assert!(
                policy.iter().any(|r| r["door"] == *d),
                "{d} has no policy row"
            );
        }
        // the jobs door queues linkage import and linkage merge, with the
        // sensitive detail either way
        let verb = |words: &[&str]| {
            crate::serve::verb_needs(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            verb(&["linkage", "import", "x.csv"]),
            Some(("data:work", Detail::Sensitive))
        );
        assert_eq!(
            verb(&["linkage", "merge", "a", "b"]),
            Some(("data:work", Detail::Sensitive))
        );
    }

    /// A caller holding the listed grants, at the detail named beside
    /// them (plain when none is).
    fn caller(list: &str) -> Caller {
        let mut access = Access::default();
        for word in list.split(',') {
            match Detail::parse(word) {
                Some(d) => access.detail = d,
                None => access.add(&Access::of_list(word).unwrap()),
            }
        }
        Caller {
            principal: "anna@lab".to_string(),
            access,
            display: None,
            email: None,
            actor: nils_registry::actor::absent(),
            ceiling: None,
            idempotency_key: None,
        }
    }

    fn registry(dir: &TempDir) -> (Home, Registry) {
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let registry = home
            .init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap();
        (home, registry)
    }

    fn call(
        home: &Home,
        registry: &mut Registry,
        caller: &Caller,
        method: &str,
        path: &str,
        body: &str,
    ) -> Reply {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
        let query: HashMap<String, String> = query
            .split('&')
            .filter(|q| !q.is_empty())
            .filter_map(|q| q.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        match route(home, registry, caller, method, &segs, &query, body).expect("a linkage door") {
            Ok(r) | Err(r) => r,
        }
    }

    fn a_place(registry: &mut Registry, dir: &TempDir, name: &str) -> i64 {
        let path = dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        place::add(
            registry.store(),
            &place::New {
                name,
                role: place::Role::Source,
                path: &path.display().to_string(),
                guarantees: serde_json::json!({}),
                probed: serde_json::json!({}),
                handling: serde_json::Value::Null,
                // read in place, a whole declaration: the folder is the
                // pseudonymised tree, PatientID holds the patient id, and
                // subjects are found through a map
                dataset: serde_json::json!({
                    "kind": "legacy",
                    "arrives": "deidentified",
                    "state": "anonymised",
                    "trees": {"originals": null, "anon": "."},
                    "patient_id": "id-type:patient-id",
                    "subjects": "map",
                }),
            },
        )
        .unwrap()
    }

    #[test]
    fn types_are_listed_and_made_once() {
        let dir = TempDir::new("linkage-doors");
        let (home, mut registry) = registry(&dir);
        let anna = caller("data:work");
        let r = call(&home, &mut registry, &anna, "GET", "/api/linkage/types", "");
        assert_eq!(r.status, 200);
        let names: Vec<&str> = r
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "patient-id",
                "study-instance-uid",
                "subject-code",
                "personnummer"
            ]
        );
        assert_eq!(r.body[0]["identifiers"], 0);
        let r = call(
            &home,
            &mut registry,
            &anna,
            "POST",
            "/api/linkage/types",
            r#"{"name": "study-id", "description": "a study's own"}"#,
        );
        assert_eq!(r.status, 201, "{}", r.body);
        assert_eq!(r.body["name"], "study-id");
        assert_eq!(r.body["created"], true);
        let again = call(
            &home,
            &mut registry,
            &anna,
            "POST",
            "/api/linkage/types",
            r#"{"name": "study-id"}"#,
        );
        assert_eq!(again.status, 200);
        assert_eq!(again.body["id"], r.body["id"]);
        assert_eq!(again.body["created"], false);
        let bad = call(
            &home,
            &mut registry,
            &anna,
            "POST",
            "/api/linkage/types",
            r#"{"name": "Study Id"}"#,
        );
        assert_eq!(bad.status, 400);
    }

    #[test]
    fn an_import_dry_runs_here_and_is_queued_from_a_file_of_its_own() {
        let dir = TempDir::new("linkage-doors");
        let (home, mut registry) = registry(&dir);
        let place_id = a_place(&mut registry, &dir, "ward-a");
        let keys = Subkeys::derive(b"nils-fixture-key");
        let lookup = keys.lookup("patient-id", "P1");
        registry
            .store()
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, lookup, sealed, id_type, first_seen, batch_id, code_anyway) VALUES (?, 'a', 0, 0, 'held', 'A9', ?, ?, 'patient-id', '2026-09-15T00:00:00Z', 3, 0)",
                &[Param::Int(place_id), Param::Bytes(lookup.clone()), Param::Bytes(keys.seal("P1"))],
            )
            .unwrap();
        let body = r#"{"place": "ward-a", "dry_run": true, "make_types": true,
            "columns": [{"header": "pid", "role": "identifier", "id_type": "patient-id"},
                        {"header": "person", "role": "canonical", "id_type": "registry-id"},
                        {"header": "note", "role": "ignore"}],
            "rows": [["P1", "PID-0001", "x"], ["P2", "PID-0001", null], ["P3", 7, ""]]}"#;
        // a dry run at plain detail answers the report, and writes nothing
        let plain = caller("data:work");
        let r = call(
            &home,
            &mut registry,
            &plain,
            "POST",
            "/api/linkage/imports",
            body,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["dry_run"], true);
        assert_eq!(r.body["written"], false);
        assert_eq!(r.body["subjects"]["new"], 2);
        assert_eq!(r.body["identifiers"]["new"], 5);
        assert_eq!(r.body["identifiers"]["types_new"], 1);
        assert_eq!(r.body["held_released"], 1);
        assert_eq!(r.body["conflicts"].as_array().unwrap().len(), 0);
        assert!(!r.body.to_string().contains("PID-0001"));
        let n = registry
            .store()
            .query("SELECT COUNT(*) FROM subject", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(n, 0);
        assert!(!home.dir().join(IMPORTS_DIR).exists());
        // the apply needs detail sensitive
        let apply = body.replace("\"dry_run\": true", "\"dry_run\": false");
        let r = call(
            &home,
            &mut registry,
            &plain,
            "POST",
            "/api/linkage/imports",
            &apply,
        );
        assert_eq!(r.status, 403, "{}", r.body);
        assert!(
            r.body["error"]
                .as_str()
                .unwrap()
                .contains("detail sensitive")
        );
        let sensitive = caller("data:work,sensitive");
        let r = call(
            &home,
            &mut registry,
            &sensitive,
            "POST",
            "/api/linkage/imports",
            &apply,
        );
        assert_eq!(r.status, 202, "{}", r.body);
        assert_eq!(r.body["state"], "queued");
        assert_eq!(r.body["rows"], 3);
        let job = nils_registry::job::show(registry.store(), r.body["job"].as_i64().unwrap())
            .unwrap()
            .unwrap();
        let argv = job.argv().unwrap();
        assert_eq!(argv[0..2], ["linkage", "import"]);
        assert!(argv.contains(&"--column".to_string()));
        assert!(argv.contains(&"pid=identifier:patient-id".to_string()));
        assert!(argv.contains(&"person=canonical:registry-id".to_string()));
        assert!(argv.contains(&"note=ignore".to_string()));
        assert!(argv.contains(&"--make-types".to_string()));
        assert!(argv.contains(&"--consume".to_string()));
        assert_eq!(job.args["detail"], "sensitive");
        assert_eq!(job.principal(), Some("anna@lab"));
        let file = PathBuf::from(&argv[2]);
        assert!(file.starts_with(home.dir().join(IMPORTS_DIR)));
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "pid,person,note\nP1,PID-0001,x\nP2,PID-0001,\nP3,7,\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(home.dir().join(IMPORTS_DIR))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        // the bound, and a bad column
        let mut big = String::from(
            r#"{"columns": [{"header": "pid", "role": "identifier", "id_type": "patient-id"}], "rows": ["#,
        );
        big.push_str(&vec!["[\"x\"]"; MAX_ROWS + 1].join(","));
        big.push_str("]}");
        let r = call(
            &home,
            &mut registry,
            &sensitive,
            "POST",
            "/api/linkage/imports",
            &big,
        );
        assert_eq!(r.status, 413);
        let r = call(
            &home,
            &mut registry,
            &sensitive,
            "POST",
            "/api/linkage/imports",
            r#"{"columns": [{"header": "pid", "role": "identifier"}], "rows": []}"#,
        );
        assert_eq!(r.status, 400);
        assert!(
            r.body["error"]
                .as_str()
                .unwrap()
                .contains("identifier names its type")
        );
        let r = call(
            &home,
            &mut registry,
            &sensitive,
            "POST",
            "/api/linkage/imports",
            r#"{"place": "nowhere", "columns": [{"header": "pid", "role": "identifier", "id_type": "patient-id"}], "rows": []}"#,
        );
        assert_eq!(r.status, 404);
    }

    #[test]
    fn a_file_waiting_for_an_id_type_value_is_never_coded_anyway() {
        // Wave 7a: its subject is known; it waits for the value, which a
        // map gives, and the door names the type it waits for
        let dir = TempDir::new("linkage-doors-wanting");
        let (home, mut registry) = registry(&dir);
        let work = caller("data:work,sensitive");
        let see = caller("data:see");
        let place_id = a_place(&mut registry, &dir, "ward-w");
        // the table is the pseudonymiser's: one held file makes it
        registry
            .store()
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, id_type, first_seen, code_anyway, subject_id, wants_type) VALUES (?, 'w1', 0, 0, 'held', '999999999999', 'personnummer', '2026-10-08T00:00:00Z', 0, 7, 'site-id')",
                &[Param::Int(place_id)],
            )
            .unwrap();
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            r#"{"place": "ward-w"}"#,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["files"], 0, "{}", r.body);
        assert_eq!(r.body["job"], serde_json::Value::Null, "{}", r.body);
        let flagged = registry
            .store()
            .query(
                "SELECT COUNT(*) FROM pseudonym_file WHERE code_anyway = 1",
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(flagged, 0);
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=ward-w",
            "",
        );
        assert_eq!(r.body[0]["waits_for"], "site-id", "{}", r.body);
    }

    #[test]
    fn the_held_doors_count_code_and_reveal_by_shape() {
        let dir = TempDir::new("linkage-doors");
        let (home, mut registry) = registry(&dir);
        let see = caller("data:see");
        let work = caller("data:work,sensitive");
        a_place(&mut registry, &dir, "ward-a");
        // before the pseudonymiser's table exists: nothing held
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=ward-a",
            "",
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.body, serde_json::json!([]));
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=nowhere",
            "",
        );
        assert_eq!(r.status, 404);
        let r = call(&home, &mut registry, &see, "GET", "/api/linkage/held", "");
        assert_eq!(r.status, 400);
        let place_id = a_place(&mut registry, &dir, "ward-b");
        let keys = Subkeys::derive(b"nils-fixture-key");
        let mut n = 0;
        let mut held = |registry: &mut Registry,
                        place: i64,
                        value: &str,
                        shape: &str,
                        state: &str,
                        batch: i64| {
            n += 1;
            registry
                .store()
                .execute(
                    "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, lookup, sealed, id_type, first_seen, batch_id, code_anyway) VALUES (?, ?, 0, 0, ?, ?, ?, ?, 'patient-id', ?, ?, 0)",
                    &[
                        Param::Int(place),
                        Param::from(format!("f{n}")),
                        Param::from(state),
                        Param::from(shape),
                        Param::Bytes(keys.lookup("patient-id", value)),
                        Param::Bytes(keys.seal(value)),
                        Param::from(format!("2026-09-1{n}T00:00:00Z")),
                        Param::Int(batch),
                    ],
                )
                .unwrap();
        };
        held(&mut registry, place_id, "P1", "A9", "held", 3);
        held(&mut registry, place_id, "P1", "A9", "held", 4);
        held(&mut registry, place_id, "P2", "A9", "held", 4);
        held(
            &mut registry,
            place_id,
            "199001011234",
            "999999999999",
            "held",
            4,
        );
        held(&mut registry, place_id, "P3", "A9", "written", 4);
        held(&mut registry, place_id + 1, "P4", "A9", "held", 4);
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=ward-b",
            "",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(
            r.body,
            serde_json::json!([
                {"shape": "999999999999", "id_type": "patient-id", "files": 1, "first_seen": "2026-09-14T00:00:00Z", "batch": 4, "waits_for": null},
                {"shape": "A9", "id_type": "patient-id", "files": 3, "first_seen": "2026-09-11T00:00:00Z", "batch": 3, "waits_for": null},
            ])
        );
        assert!(!r.body.to_string().contains("P1"));
        // code them anyway: the held rows of that dataset, once
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            r#"{"place": "ward-b"}"#,
        );
        // Wave 7a §5.4: and the run that codes them is queued and named
        assert_eq!(r.status, 202, "{}", r.body);
        assert_eq!(r.body["place"], "ward-b", "{}", r.body);
        assert_eq!(r.body["files"], 4, "{}", r.body);
        assert_eq!(r.body["state"], "queued", "{}", r.body);
        assert_eq!(
            r.body["command"],
            serde_json::json!(["digest", "@ward-b"]),
            "{}",
            r.body
        );
        let job = r.body["job"].as_i64().unwrap();
        let queued = nils_registry::job::show(registry.store(), job)
            .unwrap()
            .unwrap();
        assert_eq!(queued.state, nils_registry::job::State::Queued);
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            r#"{"place": "ward-b"}"#,
        );
        assert_eq!(r.body["files"], 0);
        assert_eq!(r.body["job"], serde_json::Value::Null, "{}", r.body);
        let flagged = registry
            .store()
            .query(
                "SELECT COUNT(*) FROM pseudonym_file WHERE code_anyway = 1",
                &[],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(flagged, 4);
        // reveal: the identifiers by shape, each audited once
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/reveal",
            r#"{"place": "ward-b"}"#,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        // Wave 7a: each identifier by the row the held ids door names it by
        assert_eq!(
            r.body,
            serde_json::json!([
                {"shape": "999999999999", "id_type": "patient-id", "files": 1, "identifiers": [{"id": 4, "value": "199001011234", "files": 1}]},
                {"shape": "A9", "id_type": "patient-id", "files": 3, "identifiers": [{"id": 1, "value": "P1", "files": 2}, {"id": 3, "value": "P2", "files": 1}]},
            ])
        );
        let listed = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held/ids?place=ward-b",
            "",
        );
        let rows: Vec<i64> = listed.body["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["id"].as_i64().unwrap())
            .collect();
        assert_eq!(rows, [1, 3, 4], "{}", listed.body);
        let mut linkage = registry.open_linkage().unwrap();
        let audit = linkage
            .query(
                "SELECT actor, identity_id, why FROM read_audit ORDER BY id",
                &[],
            )
            .unwrap();
        assert_eq!(audit.len(), 3);
        assert_eq!(audit[0].text(0).unwrap(), "anna@lab");
        assert_eq!(audit[0].text(2).unwrap(), "held reveal");
        let ids: Vec<i64> = audit.iter().map(|r| r.int(1).unwrap()).collect();
        assert_eq!(ids, [4, 1, 3]);
        let rows = nils_registry::audit::list(
            registry.store(),
            &nils_registry::audit::Filter {
                action: Some("linkage.reveal".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope["identifiers"], 3);
        assert_eq!(rows[0].scope["held"], true);
    }

    /// Wave 7a, the pseudonymise step: a dataset's held identifiers one row
    /// each, by shape and never by value; a rehearsed map fills them with
    /// the codes it gives, naming the datasets a code's subject is in
    /// already; one of them coded anyway on its own and without a run; and
    /// a code is a shape below detail quasi.
    #[test]
    fn the_held_ids_are_one_row_each_and_fill_with_codes() {
        let dir = TempDir::new("linkage-doors-ids");
        let (home, mut registry) = registry(&dir);
        let plain = caller("data:see,data:work");
        let quasi = caller("data:see,data:work,quasi");
        let ward_a = a_place(&mut registry, &dir, "ward-a");
        let ward_b = a_place(&mut registry, &dir, "ward-b");
        // before the pseudonymiser's table exists: nothing held, nothing coded
        let r = call(
            &home,
            &mut registry,
            &plain,
            "GET",
            "/api/linkage/held/ids?place=ward-b",
            "",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["ids"], serde_json::json!([]));
        assert_eq!(r.body["subjects"]["coded"], 0);
        let keys = Subkeys::derive(b"nils-fixture-key");
        registry
            .store()
            .execute(
                "INSERT INTO subject (id, code, created_at, provisional) VALUES (1, 'sub-one', 't', 0), (2, 'sub-two', 't', 1), (7, 'sub-seven', 't', 0)",
                &[],
            )
            .unwrap();
        let mut n = 0;
        let mut file = |registry: &mut Registry,
                        place: i64,
                        value: Option<&str>,
                        state: &str,
                        subject: Option<i64>,
                        wants: Option<&str>| {
            n += 1;
            registry
                .store()
                .execute(
                    "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, lookup, sealed, id_type, first_seen, batch_id, code_anyway, subject_id, wants_type) VALUES (?, ?, 0, 0, ?, ?, ?, ?, 'patient-id', ?, 5, 0, ?, ?)",
                    &[
                        Param::Int(place),
                        Param::from(format!("f{n}")),
                        Param::from(state),
                        value.map_or(Param::Null, |v| Param::from(nils_dicom::diagnostic::shape(v))),
                        value.map_or(Param::Null, |v| Param::Bytes(keys.lookup("patient-id", v))),
                        value.map_or(Param::Null, |v| Param::Bytes(keys.seal(v))),
                        Param::from(format!("2026-10-0{}T00:00:00Z", n.min(9))),
                        subject.map_or(Param::Null, Param::Int),
                        wants.map_or(Param::Null, Param::from),
                    ],
                )
                .unwrap();
        };
        // ward-b holds four identifiers and a file waiting for a value;
        // ward-a holds sub-one already, and ward-b's copies carry two subjects
        for (value, state) in [
            ("AB123", "held"),
            ("AB123", "held"),
            ("AB456", "held"),
            ("CD789", "held"),
            ("EF012", "held"),
        ] {
            file(&mut registry, ward_b, Some(value), state, None, None);
        }
        file(
            &mut registry,
            ward_b,
            None,
            "held",
            Some(7),
            Some("site-id"),
        );
        file(&mut registry, ward_b, None, "written", Some(1), None);
        file(&mut registry, ward_b, None, "written", Some(2), None);
        file(&mut registry, ward_a, None, "written", Some(1), None);

        // a map names CD789 as sub-one, filed: its files are released
        let Ok((key, subkeys)) = keys_of(&registry) else {
            panic!("the registry's key")
        };
        let derive = Derive {
            scheme: registry.meta().pseudonym_scheme,
            key: &key,
            display_length: registry.meta().display_length,
        };
        let mut linkage = registry.open_linkage().unwrap();
        let columns = [
            Column {
                header: "id".into(),
                role: Role::Identifier("patient-id".into()),
            },
            Column {
                header: "code".into(),
                role: Role::Code,
            },
        ];
        let filed = identity_map::import(
            registry.store(),
            &mut linkage,
            &subkeys,
            Some(&derive),
            &Map {
                columns: &columns,
                rows: &[Row {
                    line: 2,
                    cells: vec!["CD789".into(), "sub-one".into()],
                }],
                dry_run: false,
                make_types: false,
                place_id: Some(ward_b),
                actor: "anna@lab",
                job_id: None,
            },
        )
        .unwrap();
        assert_eq!(filed.held_released, 1);
        drop(linkage);

        // EF012 coded anyway on its own, marked and no run queued
        let ids = call(
            &home,
            &mut registry,
            &plain,
            "GET",
            "/api/linkage/held/ids?place=ward-b",
            "",
        );
        let row_of = |doc: &serde_json::Value, shape: &str, files: i64| -> i64 {
            doc["ids"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["shape"] == shape && h["files"] == files)
                .unwrap()["id"]
                .as_i64()
                .unwrap()
        };
        let ef012 = ids.body["ids"][3]["id"].as_i64().unwrap();
        let r = call(
            &home,
            &mut registry,
            &plain,
            "POST",
            "/api/linkage/held/code",
            &format!(r#"{{"place": "ward-b", "ids": [{ef012}], "run": false}}"#),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["files"], 1, "{}", r.body);
        assert_eq!(r.body["state"], "marked", "{}", r.body);
        assert_eq!(r.body["job"], serde_json::Value::Null);
        assert!(
            nils_registry::job::list(registry.store(), true, 10)
                .unwrap()
                .is_empty()
        );
        for (body, status) in [
            (r#"{"place": "ward-b", "ids": []}"#, 400),
            (r#"{"place": "ward-b", "ids": ["x"]}"#, 400),
            (r#"{"place": "ward-b", "ids": [999]}"#, 404),
            (r#"{"place": "ward-b", "run": "no"}"#, 400),
        ] {
            let r = call(
                &home,
                &mut registry,
                &plain,
                "POST",
                "/api/linkage/held/code",
                body,
            );
            assert_eq!(r.status, status, "{body}: {}", r.body);
        }

        // one row each, by shape, with how it stands and never a value
        let r = call(
            &home,
            &mut registry,
            &quasi,
            "GET",
            "/api/linkage/held/ids?place=ward-b",
            "",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        for value in ["AB123", "AB456", "CD789", "EF012"] {
            assert!(!r.body.to_string().contains(value), "{}", r.body);
        }
        assert_eq!(r.body["place"], "ward-b");
        assert_eq!(r.body["identifiers"], 5, "{}", r.body);
        assert_eq!(r.body["files"], 6, "{}", r.body);
        let states: Vec<(&str, i64, &str)> = r.body["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["shape"].as_str().unwrap_or(""),
                    h["files"].as_i64().unwrap(),
                    h["state"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            states,
            [
                ("AA999", 2, "held"),
                ("AA999", 1, "held"),
                ("AA999", 1, "mapped"),
                ("AA999", 1, "generated"),
                ("", 1, "waits"),
            ]
        );
        let mapped = &r.body["ids"][2];
        assert_eq!(mapped["code"], "sub-one", "{}", r.body);
        assert_eq!(mapped["also_in"], serde_json::json!(["ward-a"]));
        let waits = &r.body["ids"][4];
        assert_eq!(waits["code"], "sub-seven");
        assert_eq!(waits["waits_for"], "site-id");
        assert_eq!(r.body["ids"][0]["code"], serde_json::Value::Null);
        assert_eq!(r.body["ids"][0]["first_seen"], "2026-10-01T00:00:00Z");
        assert_eq!(r.body["ids"][0]["batch"], 5);
        assert_eq!(
            r.body["subjects"],
            serde_json::json!({"coded": 2, "generated": 1})
        );
        // below detail quasi a code is its shape
        let r = call(
            &home,
            &mut registry,
            &plain,
            "GET",
            "/api/linkage/held/ids?place=ward-b",
            "",
        );
        assert_eq!(r.body["ids"][2]["code"], "aaa-aaa", "{}", r.body);
        assert!(!r.body.to_string().contains("sub-one"), "{}", r.body);

        // a map rehearsed for ward-b fills the identifiers it names, each by
        // its row, a known subject's with the datasets it is in already
        let ab123 = row_of(&r.body, "AA999", 2);
        let ab456 = r.body["ids"][1]["id"].as_i64().unwrap();
        let body = r#"{"place": "ward-b", "dry_run": true,
            "columns": [{"header": "ID", "role": "identifier", "id_type": "patient-id"}, {"header": "subject code", "role": "code"}],
            "rows": [["AB123", "sub-one"], ["AB456", "sub-new"], ["ZZ000", "sub-other"]]}"#;
        let r = call(
            &home,
            &mut registry,
            &quasi,
            "POST",
            "/api/linkage/imports",
            body,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["held_released"], 3, "{}", r.body);
        assert_eq!(
            r.body["held_ids"],
            serde_json::json!([
                {"id": ab123, "code": "sub-one", "also_in": ["ward-a"]},
                {"id": ab456, "code": "sub-new", "also_in": []},
            ]),
            "{}",
            r.body
        );
        assert!(!r.body.to_string().contains("AB123"), "{}", r.body);
        // at plain, the codes as their shapes
        let r = call(
            &home,
            &mut registry,
            &plain,
            "POST",
            "/api/linkage/imports",
            body,
        );
        assert_eq!(r.body["held_ids"][0]["code"], "aaa-aaa", "{}", r.body);
        // rehearsed for no dataset, no rows to fill
        let r = call(
            &home,
            &mut registry,
            &plain,
            "POST",
            "/api/linkage/imports",
            &body.replace(r#""place": "ward-b", "#, ""),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.get("held_ids").is_none(), "{}", r.body);
    }

    /// Wave 7a, the pseudonymise step at scale: a dataset of a few thousand
    /// held files over hundreds of identifiers, each identifier's rows far
    /// apart and another dataset holding some of the same, is one row an
    /// identifier in the order its first file was held, each with its own
    /// count and state; identifiers chosen by their rows are coded anyway
    /// and no others; and a rehearsed map fills exactly the identifiers it
    /// names that still wait.
    #[test]
    fn many_held_files_group_into_their_identifiers() {
        const IDS: i64 = 500;
        const EACH: i64 = 6;
        let dir = TempDir::new("linkage-doors-many");
        let (home, mut registry) = registry(&dir);
        let work = caller("data:see,data:work,quasi");
        let ward = a_place(&mut registry, &dir, "ward-big");
        let other = a_place(&mut registry, &dir, "ward-other");
        let keys = Subkeys::derive(b"nils-fixture-key");
        let value = |i: i64| format!("ID{i:05}");
        // how each identifier stands: every file released, the first
        // released and the rest held, coded anyway, or held
        let all_released = |i: i64| i % 7 == 0;
        let first_released = |i: i64| i % 11 == 0 && !all_released(i);
        let anyway = |i: i64| i % 5 == 0 && !all_released(i) && !first_released(i);
        registry.store().begin().unwrap();
        // round by round, so an identifier's files lie IDS rows apart
        for round in 0..EACH {
            for i in 0..IDS {
                let released = all_released(i) || (first_released(i) && round == 0);
                registry
                    .store()
                    .execute(
                        "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, lookup, id_type, first_seen, batch_id, released_at, code_anyway) VALUES (?, ?, 0, 0, 'held', 'AA99999', ?, 'patient-id', '2026-10-09T00:00:00Z', 7, ?, ?)",
                        &[
                            Param::Int(ward),
                            Param::from(format!("r{round}/f{i}")),
                            Param::Bytes(keys.lookup("patient-id", &value(i))),
                            if released {
                                Param::from("2026-10-09T01:00:00Z")
                            } else {
                                Param::Null
                            },
                            Param::Int(i64::from(anyway(i))),
                        ],
                    )
                    .unwrap();
            }
        }
        // the other dataset holds one file of every tenth identifier
        for i in (0..IDS).step_by(10) {
            registry
                .store()
                .execute(
                    "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, shape, lookup, id_type, first_seen, code_anyway) VALUES (?, ?, 0, 0, 'held', 'AA99999', ?, 'patient-id', '2026-10-09T00:00:00Z', 0)",
                    &[
                        Param::Int(other),
                        Param::from(format!("o/f{i}")),
                        Param::Bytes(keys.lookup("patient-id", &value(i))),
                    ],
                )
                .unwrap();
        }
        registry.store().commit().unwrap();
        // the rows are 1..=3000 for ward-big: identifier i's file of round r
        // is row r * IDS + i + 1
        let row = |round: i64, i: i64| round * IDS + i + 1;

        let r = call(
            &home,
            &mut registry,
            &work,
            "GET",
            "/api/linkage/held/ids?place=ward-big",
            "",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["identifiers"], IDS);
        assert_eq!(r.body["files"], IDS * EACH);
        let ids = r.body["ids"].as_array().unwrap();
        assert_eq!(ids.len() as i64, IDS);
        for (i, h) in (0..IDS).zip(ids) {
            assert_eq!(h["files"], EACH, "identifier {i}: {h}");
            // the row that stands for it: the first not yet released
            let stands = if first_released(i) {
                row(1, i)
            } else {
                row(0, i)
            };
            assert_eq!(h["id"], stands, "identifier {i}: {h}");
            let state = if all_released(i) {
                "mapped"
            } else if anyway(i) {
                "generated"
            } else {
                "held"
            };
            assert_eq!(h["state"], state, "identifier {i}: {h}");
        }

        // fifty identifiers that wait with no code, chosen by their rows:
        // their files and no others are coded anyway, and nothing is queued
        let held: Vec<i64> = (0..IDS)
            .filter(|&i| !all_released(i) && !first_released(i) && !anyway(i))
            .take(50)
            .collect();
        let chosen: Vec<i64> = held.iter().map(|&i| row(0, i)).collect();
        let flagged = |registry: &mut Registry| {
            registry
                .store()
                .query(
                    "SELECT COUNT(*) FROM pseudonym_file WHERE code_anyway = 1",
                    &[],
                )
                .unwrap()[0]
                .int(0)
                .unwrap()
        };
        let before = flagged(&mut registry);
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            &serde_json::json!({"place": "ward-big", "ids": chosen, "run": false}).to_string(),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["files"], 50 * EACH, "{}", r.body);
        assert_eq!(flagged(&mut registry), before + 50 * EACH);
        assert!(
            nils_registry::job::list(registry.store(), true, 10)
                .unwrap()
                .is_empty()
        );
        let r = call(
            &home,
            &mut registry,
            &work,
            "GET",
            "/api/linkage/held/ids?place=ward-big",
            "",
        );
        let generated: BTreeSet<i64> = r.body["ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["state"] == "generated")
            .map(|h| h["id"].as_i64().unwrap())
            .collect();
        assert!(chosen.iter().all(|c| generated.contains(c)));
        // a row of another dataset is no identifier of this one
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            &format!(
                r#"{{"place": "ward-big", "ids": [{}], "run": false}}"#,
                IDS * EACH + 1
            ),
        );
        assert_eq!(r.status, 404, "{}", r.body);

        // a map naming the first two hundred: the identifiers among them
        // whose files still wait fill, each by its own row and code
        let rows: Vec<serde_json::Value> = (0..200)
            .map(|i| serde_json::json!([value(i), format!("code-{i:04}")]))
            .collect();
        let body = serde_json::json!({
            "place": "ward-big",
            "dry_run": true,
            "columns": [{"header": "id", "role": "identifier", "id_type": "patient-id"}, {"header": "code", "role": "code"}],
            "rows": rows,
        });
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/imports",
            &body.to_string(),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let filled: Vec<(i64, String)> = r.body["held_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["id"].as_i64().unwrap(),
                    h["code"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let expected: Vec<(i64, String)> = (0..200)
            .filter(|&i| !all_released(i))
            .map(|i| {
                let stands = if first_released(i) {
                    row(1, i)
                } else {
                    row(0, i)
                };
                (stands, format!("code-{i:04}"))
            })
            .collect();
        assert_eq!(filled, expected);
        assert!(!r.body.to_string().contains("ID00001"), "{}", r.body);
    }

    #[test]
    fn a_merge_is_queued_when_both_codes_are_subjects() {
        let dir = TempDir::new("linkage-doors");
        let (home, mut registry) = registry(&dir);
        let work = caller("data:work,sensitive");
        registry
            .store()
            .execute(
                "INSERT INTO subject (id, code, created_at, merged_into) VALUES (1, 'canon', 't', NULL), (2, 'alias', 't', NULL), (3, 'gone', 't', 1)",
                &[],
            )
            .unwrap();
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/merge",
            r#"{"canonical": "canon", "alias": "alias", "why": "the clinic renamed"}"#,
        );
        assert_eq!(r.status, 202, "{}", r.body);
        let job = nils_registry::job::show(registry.store(), r.body["job"].as_i64().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            job.argv().unwrap(),
            [
                "linkage",
                "merge",
                "canon",
                "alias",
                "--why",
                "the clinic renamed"
            ]
        );
        assert_eq!(job.args["detail"], "sensitive");
        for (body, status) in [
            (
                r#"{"canonical": "canon", "alias": "nope", "why": "x"}"#,
                404,
            ),
            (
                r#"{"canonical": "canon", "alias": "gone", "why": "x"}"#,
                409,
            ),
            (
                r#"{"canonical": "canon", "alias": "canon", "why": "x"}"#,
                400,
            ),
            (r#"{"canonical": "canon", "alias": "alias"}"#, 400),
        ] {
            let r = call(
                &home,
                &mut registry,
                &work,
                "POST",
                "/api/linkage/merge",
                body,
            );
            assert_eq!(r.status, status, "{body}: {}", r.body);
        }
    }

    /// One synthetic MR file about a person, for a dataset read in place.
    fn dicom_of(patient: &str, sop: &str) -> Vec<u8> {
        use dicom_core::VR;
        use dicom_dictionary_std::tags;
        use nils_dicom::synth::{self, MetaFields};
        let study = &sop[..1];
        let mut elems = synth::minimal_mr(study, &format!("{study}.1"), sop);
        elems.push(synth::text(tags::PATIENT_ID, VR::LO, patient));
        synth::part10(&MetaFields::mr(sop), &elems, true)
    }

    #[test]
    fn the_held_doors_answer_for_the_files_a_digest_held() {
        // Lab 26b, finding 3: a dataset read in place holds its files at the
        // digest, where an identified one holds them at the pseudonymiser.
        // The doors and the sources count answer for both, so a person sees
        // what waits for a map, can reveal it, and can have it coded anyway.
        let dir = TempDir::new("linkage-doors-digest");
        let (home, mut registry) = registry(&dir);
        let see = caller("data:see");
        let work = caller("data:work,sensitive");
        let tree = dir.path().join("south");
        for (path, patient, sop) in [
            ("a/IM_0001", "P1", "A.1.1"),
            ("a/IM_0002", "P1", "A.1.2"),
            ("b/IM_0001", "P2", "B.1.1"),
        ] {
            let file = tree.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, dicom_of(patient, sop)).unwrap();
        }
        place::add(
            registry.store(),
            &place::New {
                name: "south",
                role: place::Role::Source,
                path: &tree.display().to_string(),
                guarantees: serde_json::json!({}),
                probed: serde_json::json!({}),
                handling: serde_json::json!({}),
                dataset: serde_json::json!({
                    "kind": "legacy",
                    "arrives": "deidentified",
                    "state": "anonymised",
                    "trees": {"originals": null, "anon": "."},
                    "patient_id": "id-type:patient-id",
                    "subjects": "map",
                }),
            },
        )
        .unwrap();
        let mut settings = nils_digest::Settings::new(&tree);
        settings.unmapped = nils_digest::Unmapped::Hold;
        nils_digest::digest(&settings, &mut registry).unwrap();

        // what waits, by shape, with no value on the list
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=south",
            "",
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body.as_array().unwrap().len(), 1, "{}", r.body);
        assert_eq!(r.body[0]["files"], 3);
        assert_eq!(r.body[0]["shape"], "A9");
        assert_eq!(r.body[0]["id_type"], "patient-id");
        assert!(!r.body.to_string().contains("P1"));
        // and the dataset's own card says the same, so it is not empty while
        // its files sit quarantined
        let sources = crate::sources::document(&mut registry, 5).unwrap();
        assert_eq!(
            sources["sources"][0]["held"],
            serde_json::json!({"files": 3, "identifiers": 2})
        );

        // the identifiers themselves, at sensitive detail
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/reveal",
            r#"{"place": "south"}"#,
        );
        assert_eq!(r.status, 200, "{}", r.body);
        // sorted here rather than by the door: the identifiers of a shape
        // come back in the order their rows were written, and the digest's
        // workers hold the files of a batch in whatever order they read them
        let mut values: Vec<&str> = r.body[0]["identifiers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["value"].as_str().unwrap())
            .collect();
        values.sort_unstable();
        assert_eq!(values, ["P1", "P2"]);

        // coded anyway: the next digest of the dataset codes them, marks the
        // subjects provisional, and the door empties
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            r#"{"place": "south"}"#,
        );
        assert_eq!(r.body["place"], "south", "{}", r.body);
        assert_eq!(r.body["files"], 3, "{}", r.body);
        assert!(r.body["job"].is_i64(), "{}", r.body);
        nils_digest::digest(&settings, &mut registry).unwrap();
        let r = call(
            &home,
            &mut registry,
            &see,
            "GET",
            "/api/linkage/held?place=south",
            "",
        );
        assert_eq!(r.body, serde_json::json!([]));
        let sources = crate::sources::document(&mut registry, 5).unwrap();
        assert_eq!(
            sources["sources"][0]["held"],
            serde_json::json!({"files": 0, "identifiers": 0})
        );
        let provisional = registry
            .store()
            .query("SELECT COUNT(*) FROM subject WHERE provisional = 1", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(provisional, 2);
    }
}
