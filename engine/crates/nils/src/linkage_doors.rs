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

use std::collections::HashMap;
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
        ["api", "linkage", "held", "code"] if post => held_code(registry, body),
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
        return Ok(Reply::ok(report.as_json()));
    }
    // the map is read by the job: sensitive, as record 25 set for jobs
    caller.allowed(
        "POST /api/linkage/imports",
        Need::One("data:work"),
        Detail::Sensitive,
    )?;
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
        "SELECT shape, id_type, COUNT(*), CAST(MIN(first_seen) AS TEXT), MIN(batch_id) FROM {} \
         WHERE place_id = {} AND state = 'held' AND released_at IS NULL \
         GROUP BY shape, id_type ORDER BY shape, id_type",
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
        }));
    }
    Ok(Reply::ok(serde_json::Value::from(out)))
}

/// `POST /api/linkage/held/code {place}`: code the held files anyway on
/// the next run.
fn held_code(registry: &mut Registry, body: &str) -> Result<Reply, Reply> {
    let doc = json_body(body)?;
    let name = doc["place"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Reply::error(400, "place: the dataset's name"))?;
    let p = place_named(registry, name)?;
    if !held_table(registry)? {
        return Ok(Reply::ok(
            serde_json::json!({ "place": p.name, "files": 0 }),
        ));
    }
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET code_anyway = 1 WHERE place_id = {} AND state = 'held' \
         AND released_at IS NULL AND code_anyway = 0",
        store.qualified(HELD_TABLE),
        d.param(1, Type::Int)
    );
    let n = store.execute(&sql, &[Param::Int(p.id)])?;
    Ok(Reply::ok(
        serde_json::json!({ "place": p.name, "files": n }),
    ))
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
            serde_json::json!({
                "shape": g.shape,
                "id_type": g.id_type,
                "files": files,
                "identifiers": g.values.iter().map(|v| serde_json::json!({ "value": v.value, "files": v.files })).collect::<Vec<_>>(),
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
                scheme: nils_registry::pseudonym::Scheme::DEFAULT,
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
                dataset: serde_json::Value::Null,
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
        assert_eq!(names, ["patient-id", "study-instance-uid", "subject-code"]);
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
                {"shape": "999999999999", "id_type": "patient-id", "files": 1, "first_seen": "2026-09-14T00:00:00Z", "batch": 4},
                {"shape": "A9", "id_type": "patient-id", "files": 3, "first_seen": "2026-09-11T00:00:00Z", "batch": 3},
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
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body, serde_json::json!({"place": "ward-b", "files": 4}));
        let r = call(
            &home,
            &mut registry,
            &work,
            "POST",
            "/api/linkage/held/code",
            r#"{"place": "ward-b"}"#,
        );
        assert_eq!(r.body["files"], 0);
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
        assert_eq!(
            r.body,
            serde_json::json!([
                {"shape": "999999999999", "id_type": "patient-id", "files": 1, "identifiers": [{"value": "199001011234", "files": 1}]},
                {"shape": "A9", "id_type": "patient-id", "files": 3, "identifiers": [{"value": "P1", "files": 2}, {"value": "P2", "files": 1}]},
            ])
        );
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
}
