// SPDX-License-Identifier: AGPL-3.0-only

//! `nils ask gate` (Wave 4b §13): the fixtures, their canonicals and their
//! outcomes live in the repository, and this runs them on whichever
//! backend the registry is. Every row is rendered by the engine's own
//! renderer, so a canonical taken on SQLite is the one Postgres must
//! reproduce, and the two backends agreeing is the run agreeing with the
//! file. A fixture has one of three outcomes and there is no fourth: it
//! passes, it is deferred by a clause this wave staged, or the question is
//! out of the language. Anything else fails the gate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nils_ask::exec::Bounds;
use nils_ask::run::{self, Request};
use nils_ask::validate::{Class, Scope};
use nils_ask::{diagnose, parse};
use nils_catalog::{Caps, Catalog};
use nils_registry::home::Home;
use nils_registry::schema::{Type, table};
use nils_registry::session::Scheme;
use nils_registry::{Insert, Param, Registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Exit, actor, fail, open, usage};

/// The upload the key list fixture reads: six hundred positions over the
/// registry's own subjects, written by the gate so the fixture has
/// something to resolve.
const UPLOAD: &str = "gate-600";
const UPLOAD_ROWS: i64 = 600;

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    gate: Gate,
}

#[derive(Debug, Clone, Deserialize)]
struct Gate {
    plan: Plan,
    normalisation: String,
    fixtures: Vec<Fixture>,
    #[serde(default)]
    families: Vec<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
struct Plan {
    seed: u64,
    subjects: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    name: String,
    family: String,
    question: String,
    #[serde(default)]
    file: Option<String>,
    outcome: String,
    /// The staged clause a deferred fixture waits for.
    #[serde(default)]
    clause: Option<String>,
    /// The limit an out of language fixture names.
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

/// What a fixture left the last time the canonicals were written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Canonical {
    name: String,
    question: String,
    /// The content hash of the answer, which both backends must reproduce.
    hash: Option<String>,
    columns: Vec<String>,
    row_count: usize,
    /// Every row, rendered: the declared normalisation.
    rows: Vec<Vec<String>>,
    /// The funnel's last stage per named set, so a fixture says where its
    /// subjects went and not only how many came back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    funnel: BTreeMap<String, i64>,
}

/// One fixture's outcome in a run.
#[derive(Debug, Clone, Serialize)]
struct Ran {
    name: String,
    family: String,
    outcome: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    rows: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    differs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

fn scope() -> Scope {
    Scope {
        federated: false,
        classes: [Class::QuasiIdentifying, Class::Sensitive]
            .into_iter()
            .collect(),
    }
}

fn bounds() -> Bounds {
    let caps = Caps::default();
    Bounds {
        timeout_ms: caps.sync_timeout_ms,
        max_rows: caps.sync_max_rows,
        max_bytes: caps.sync_max_bytes,
    }
}

/// The gate directory: the one given, or the repository's beside the
/// binary's own source.
fn gate_dir(given: Option<PathBuf>) -> Result<PathBuf, Exit> {
    let dir = match given {
        Some(d) => d,
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gate"),
    };
    if !dir.join("gate.yml").exists() {
        return Err(usage(format!(
            "{}: no gate.yml; --gate DIR names the gate's own directory",
            dir.display()
        )));
    }
    Ok(dir)
}

/// The upload the key list fixture reads, written once and left alone.
fn ensure_upload(registry: &mut Registry) -> Result<(), Exit> {
    let store = registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE upload_id = {}",
        store.qualified("values_source"),
        d.param(1, Type::Text)
    );
    if store
        .query_opt(&sql, &[Param::from(UPLOAD)])
        .map_err(|e| fail(e.to_string()))?
        .is_some()
    {
        return Ok(());
    }
    let subjects: Vec<i64> = store
        .query(
            &format!("SELECT id FROM {} ORDER BY id", store.qualified("subject")),
            &[],
        )
        .map_err(|e| fail(e.to_string()))?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()
        .map_err(|e| fail(e.to_string()))?;
    if subjects.is_empty() {
        return Err(usage(
            "this registry holds no subjects; nils synth writes the gate's own",
        ));
    }
    let now = nils_registry::time::now_iso();
    let rows = store
        .insert(
            &Insert::new(
                table("values_source"),
                &[
                    "upload_id",
                    "namespace",
                    "digest",
                    "n",
                    "unresolved",
                    "principal",
                    "created_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(UPLOAD),
                Param::from("patient-id"),
                Param::from("the gate's own list"),
                Param::Int(UPLOAD_ROWS),
                Param::Int(0),
                Param::from("gate"),
                Param::from(now.as_str()),
            ]],
        )
        .map_err(|e| fail(e.to_string()))?;
    let source = rows[0].int(0).map_err(|e| fail(e.to_string()))?;
    let members: Vec<Vec<Param>> = (0..UPLOAD_ROWS)
        .map(|i| {
            vec![
                Param::Int(source),
                Param::Int(i),
                Param::Int(subjects[i as usize % subjects.len()]),
            ]
        })
        .collect();
    for chunk in members.chunks(500) {
        store
            .insert(
                &Insert::new(
                    table("values_member"),
                    &["source_id", "position", "subject_id"],
                ),
                chunk,
            )
            .map_err(|e| fail(e.to_string()))?;
    }
    Ok(())
}

/// Run one fixture and render what it left.
fn run_fixture(
    registry: &mut Registry,
    catalog: &Catalog,
    dir: &Path,
    f: &Fixture,
) -> Result<Canonical, Exit> {
    let file = f
        .file
        .as_ref()
        .ok_or_else(|| usage(format!("{}: a fixture that passes names its file", f.name)))?;
    let path = dir.join(file);
    let text =
        std::fs::read_to_string(&path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let ask = parse(&text).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    let scope = scope();
    let scheme = Scheme::default();
    let node = "gate".to_string();
    let out = run::run(
        registry,
        Request {
            ask: ask.clone(),
            names: catalog,
            scope: &scope,
            principal: &actor(),
            node: &node,
            pack_version: None,
            scheme: &scheme,
            bounds: bounds(),
            page_rows: Caps::default().page_rows as usize,
            name: None,
            keep: false,
            after: None,
            limit: None,
            may_project_raw: false,
            purpose: None,
            reader: None,
        },
    )
    .map_err(|e| fail(format!("{}: {e}", f.name)))?;
    if out.answer.truncated {
        return Err(fail(format!(
            "{}: the answer was cut by a cap, and a capped answer is no canonical",
            f.name
        )));
    }
    // the funnel's last stage per named set: where the subjects went
    let d = diagnose::diagnose(
        registry,
        ask,
        Vec::new(),
        catalog,
        &scope,
        &scheme,
        bounds(),
        false,
        None,
    )
    .map_err(|e| fail(format!("{}: {e}", f.name)))?;
    let mut funnel: BTreeMap<String, i64> = BTreeMap::new();
    for stage in &d.funnel {
        funnel.insert(stage.set.clone(), stage.subjects);
    }
    Ok(Canonical {
        name: f.name.clone(),
        question: f.question.clone(),
        hash: out.answer.content_hash.clone(),
        columns: out.answer.columns.clone(),
        row_count: out.answer.rows.len(),
        rows: out
            .answer
            .rows
            .iter()
            .map(|r| r.0.iter().map(nils_ask::exec::render).collect())
            .collect(),
        funnel,
    })
}

/// What differs between a canonical and a run, in words.
fn differences(want: &Canonical, got: &Canonical) -> Vec<String> {
    let mut out = Vec::new();
    if want.hash != got.hash {
        out.push(format!(
            "the content hash: {} became {}",
            want.hash.clone().unwrap_or_else(|| "none".into()),
            got.hash.clone().unwrap_or_else(|| "none".into())
        ));
    }
    if want.columns != got.columns {
        out.push(format!(
            "the columns: {} became {}",
            want.columns.join(", "),
            got.columns.join(", ")
        ));
    }
    if want.row_count != got.row_count {
        out.push(format!(
            "the row count: {} became {}",
            want.row_count, got.row_count
        ));
    }
    for (i, (a, b)) in want.rows.iter().zip(got.rows.iter()).enumerate() {
        if a != b {
            out.push(format!(
                "row {i}: {} became {}",
                a.join(" | "),
                b.join(" | ")
            ));
        }
        if out.len() > 6 {
            out.push("and more rows differ".into());
            break;
        }
    }
    for (set, subjects) in &want.funnel {
        if got.funnel.get(set) != Some(subjects) {
            out.push(format!(
                "the funnel at {set}: {subjects} subjects became {}",
                got.funnel
                    .get(set)
                    .map(i64::to_string)
                    .unwrap_or_else(|| "no stage".into())
            ));
        }
    }
    out
}

/// `nils ask gate`.
pub(crate) fn gate(
    home: &Home,
    pack_dir: PathBuf,
    pack: &str,
    given: Option<PathBuf>,
    write: bool,
    json_out: bool,
    ask_dsn: Option<String>,
) -> Result<(), Exit> {
    let dir = gate_dir(given)?;
    let text = std::fs::read_to_string(dir.join("gate.yml"))
        .map_err(|e| fail(format!("{}: {e}", dir.join("gate.yml").display())))?;
    let manifest: Manifest = serde_saphyr::from_str(&text)
        .map_err(|e| usage(format!("{}: {e}", dir.join("gate.yml").display())))?;
    let gate = manifest.gate;
    let mut registry = open(home)?;
    let pack = crate::ask_cli::load_pack(&pack_dir, pack)?;
    ensure_upload(&mut registry)?;
    let catalog = Catalog::build(&mut registry, &pack).map_err(|e| fail(e.to_string()))?;
    let backend = format!("{:?}", registry.config().backend).to_lowercase();

    let mut ran: Vec<Ran> = Vec::new();
    ran.push(write_refusal(&registry, ask_dsn.as_deref(), &backend)?);
    for f in &gate.fixtures {
        match f.outcome.as_str() {
            "passes" => {
                let got = run_fixture(&mut registry, &catalog, &dir, f)?;
                let canonical = dir.join(format!("expect/{}.json", f.name));
                if write {
                    std::fs::create_dir_all(dir.join("expect")).map_err(|e| fail(e.to_string()))?;
                    std::fs::write(
                        &canonical,
                        format!(
                            "{}\n",
                            serde_json::to_string_pretty(&got).unwrap_or_default()
                        ),
                    )
                    .map_err(|e| fail(format!("{}: {e}", canonical.display())))?;
                    ran.push(Ran {
                        name: f.name.clone(),
                        family: f.family.clone(),
                        outcome: "written".into(),
                        ok: true,
                        hash: got.hash.clone(),
                        rows: got.row_count,
                        differs: Vec::new(),
                        note: f.note.clone(),
                    });
                    continue;
                }
                let want: Canonical = match std::fs::read_to_string(&canonical) {
                    Ok(text) => serde_json::from_str(&text)
                        .map_err(|e| fail(format!("{}: {e}", canonical.display())))?,
                    Err(e) => {
                        return Err(usage(format!(
                            "{}: {e}; nils ask gate --write takes the canonicals",
                            canonical.display()
                        )));
                    }
                };
                let differs = differences(&want, &got);
                ran.push(Ran {
                    name: f.name.clone(),
                    family: f.family.clone(),
                    outcome: f.outcome.clone(),
                    ok: differs.is_empty(),
                    hash: got.hash.clone(),
                    rows: got.row_count,
                    differs,
                    note: f.note.clone(),
                });
            }
            "deferred" => ran.push(Ran {
                name: f.name.clone(),
                family: f.family.clone(),
                outcome: f.outcome.clone(),
                ok: f.clause.is_some(),
                hash: None,
                rows: 0,
                differs: if f.clause.is_some() {
                    Vec::new()
                } else {
                    vec!["a deferred fixture names the staged clause it needs".into()]
                },
                note: f.clause.clone().map(|c| format!("waits for {c}")),
            }),
            "out_of_language" => ran.push(Ran {
                name: f.name.clone(),
                family: f.family.clone(),
                outcome: f.outcome.clone(),
                ok: f.limit.is_some(),
                hash: None,
                rows: 0,
                differs: if f.limit.is_some() {
                    Vec::new()
                } else {
                    vec!["an out of language fixture names the limit".into()]
                },
                note: f.limit.clone(),
            }),
            other => {
                return Err(usage(format!(
                    "{}: {other} is not an outcome; those are passes, deferred and out_of_language",
                    f.name
                )));
            }
        }
    }

    let failed = ran.iter().filter(|r| !r.ok).count();
    let doc = json!({
        "gate": {
            "backend": backend,
            "plan": gate.plan,
            "normalisation": gate.normalisation,
            "fixtures": ran.len(),
            "failed": failed,
            "families": gate.families.len(),
        },
        "fixtures": ran,
    });
    if json_out {
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        println!(
            "nils ask gate   {backend}   seed {}   {} subjects   {} fixtures",
            gate.plan.seed,
            gate.plan.subjects,
            ran.len()
        );
        for r in &ran {
            let mark = if r.ok { "ok    " } else { "FAILED" };
            println!(
                "{mark}  {:<26}  {:<16}  {}",
                r.name,
                r.outcome,
                match (&r.hash, r.rows) {
                    (Some(h), n) => format!("{n} rows, hash {}", &h[..12.min(h.len())]),
                    (None, _) => r.note.clone().unwrap_or_default(),
                }
            );
            for d in &r.differs {
                println!("        {d}");
            }
        }
    }
    if failed > 0 {
        return Err(Exit {
            code: crate::FAILED,
            message: format!("{failed} of {} fixtures did not hold", ran.len()),
        });
    }
    Ok(())
}

/// Wave 4c §6.8, fixture 1: the ask reader cannot write, measured against
/// the live store rather than asserted. On SQLite the reader is the read
/// only connection. On Postgres it is the DSN of the SELECT only role, and
/// the fixture first tries to undo the session setting the fallback reader
/// relies on, because a setting is not a privilege; without a DSN the
/// fixture is deferred, since the role is a deployment's to create.
fn write_refusal(
    registry: &nils_registry::Registry,
    ask_dsn: Option<&str>,
    backend: &str,
) -> Result<Ran, Exit> {
    let name = "write-refusal".to_string();
    let family = "custody".to_string();
    if backend == "postgres" && ask_dsn.is_none() {
        return Ok(Ran {
            name,
            family,
            outcome: "deferred".into(),
            ok: true,
            hash: None,
            rows: 0,
            differs: Vec::new(),
            note: Some("no --ask-dsn: the SELECT only role is a deployment's to create".into()),
        });
    }
    let mut reader = registry
        .open_ask_reader(ask_dsn)
        .map_err(|e| fail(format!("the ask reader: {e}")))?;
    if backend == "postgres" {
        // A session setting is not a privilege: a real role survives this.
        let _ = reader.batch("SET default_transaction_read_only = off");
    }
    let t = reader.qualified("handle_read_audit");
    let statements = [
        (
            "INSERT",
            format!(
                "INSERT INTO {t} (principal, handle_id, read_at, columns, rows, epoch) VALUES ('gate', 0, '2026-01-01T00:00:00Z', '[]', 0, 0)"
            ),
        ),
        ("UPDATE", format!("UPDATE {t} SET rows = 0 WHERE id = 0")),
        ("DELETE", format!("DELETE FROM {t} WHERE id = 0")),
        (
            "CREATE",
            "CREATE TABLE gate_write_refusal (id INTEGER)".to_string(),
        ),
        ("COPY", format!("COPY {t} TO '/dev/null'")),
    ];
    let mut differs = Vec::new();
    for (verb, sql) in &statements {
        if reader.batch(sql).is_ok() {
            differs.push(format!("{verb} succeeded through the ask reader"));
        }
    }
    let ok = differs.is_empty();
    Ok(Ran {
        name,
        family,
        outcome: "passes".into(),
        ok,
        hash: None,
        rows: 0,
        differs,
        note: Some(if ok {
            "INSERT, UPDATE, DELETE, CREATE and COPY all refused".into()
        } else {
            "the ask reader can write".into()
        }),
    })
}
