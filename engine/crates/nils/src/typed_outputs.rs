// SPDX-License-Identifier: AGPL-3.0-only
//! Typed outputs and typed links (Wave 7b spec section 8.2): a derivative
//! is typed by the output of the descriptor that made it, which says its
//! kind, the role and contrast it stands for and the grid or space it is
//! on; a `derivative:<kind>` input that asks for a role, a contrast or a
//! grid takes only a derivative whose output matches. The first link: the
//! registration family's aligned FLAIR (kind image, role flair, grid t1w),
//! which SAMSEG and segcsvd read in place of the header-only resampling,
//! and which tells the pack's `aligned(flair, t1w)` fact true.
use std::collections::{BTreeMap, BTreeSet};

use nils_pipeline::descriptor::{self, TypedInput};
use nils_registry::derivative::Derivative;
use nils_registry::pipeline as rows;
use nils_registry::store::Store;

/// What an output declares of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Typed {
    pub kind: String,
    pub role: Option<String>,
    pub contrast: Option<String>,
    pub grid: Option<String>,
    pub space: Option<String>,
}

/// The outputs of a run's pipeline, read once per run.
#[derive(Default)]
pub(crate) struct Outputs {
    by_run: BTreeMap<i64, Option<Vec<descriptor::Output>>>,
}

impl Outputs {
    fn of_run(&mut self, store: &mut Store, run_id: i64) -> Option<&Vec<descriptor::Output>> {
        self.by_run
            .entry(run_id)
            .or_insert_with(|| {
                let r = rows::run(store, run_id).ok().flatten()?;
                let p = rows::get(store, r.pipeline_id).ok().flatten()?;
                let d = descriptor::from_value(p.descriptor.clone()).ok()?;
                Some(d.outputs)
            })
            .as_ref()
    }

    /// The typed output that made this derivative: of its run's pipeline,
    /// the output of its kind whose path template its path matches. A
    /// registered path is the run's folder, then the unit's folder where
    /// the units ran apart, then the template's path, so the template is
    /// held to the path's last components, as many as it has.
    pub(crate) fn of_derivative(&mut self, store: &mut Store, row: &Derivative) -> Option<Typed> {
        let run_id = row.run_id?;
        let outputs = self.of_run(store, run_id)?;
        let parts: Vec<&str> = row.path.split('/').filter(|p| !p.is_empty()).collect();
        outputs
            .iter()
            .find(|o| {
                let n = o.template.split('/').count();
                o.kind == row.kind
                    && parts.len() >= n
                    && nils_pipeline::files::matches_template(
                        &o.template,
                        &parts[parts.len() - n..].join("/"),
                    )
            })
            .map(|o| Typed {
                kind: o.kind.clone(),
                role: o.role.clone(),
                contrast: o.contrast.clone(),
                grid: o.grid.clone(),
                space: o.space.clone(),
            })
    }

    /// Whether a typed input takes this derivative: an input that asks for
    /// no role, contrast or grid takes any of its kind; one that asks takes
    /// a derivative whose output says the same (the contrast read without
    /// case).
    pub(crate) fn takes(&mut self, store: &mut Store, t: &TypedInput, row: &Derivative) -> bool {
        if t.role.is_none() && t.contrast.is_none() && t.grid.is_none() {
            return true;
        }
        let Some(typed) = self.of_derivative(store, row) else {
            return false;
        };
        let same = |want: &Option<String>, have: &Option<String>, fold: bool| match want {
            None => true,
            Some(w) => have.as_ref().is_some_and(|h| {
                if fold {
                    h.eq_ignore_ascii_case(w)
                } else {
                    h == w
                }
            }),
        };
        same(&t.role, &typed.role, false)
            && same(&t.contrast, &typed.contrast, true)
            && same(&t.grid, &typed.grid, false)
    }
}

/// The sessions among `sessions` (subject, day) for which a registration
/// step wrote an image of role `moving` on the grid of `fixed`: a derivative
/// of kind image of the session, not withdrawn, not rejected by the pack's
/// checks, whose output says so. The pack fact `aligned(moving, fixed)`
/// reads true for them. The images of all the sessions' subjects are read a
/// few hundred subjects at a time, and one cache of outputs serves the whole
/// call. A store failure is the caller's to say, never an empty answer.
pub(crate) fn aligned_by_registration(
    store: &mut Store,
    outputs: &mut Outputs,
    sessions: &[(i64, String)],
    moving: &str,
    fixed: &str,
) -> Result<BTreeSet<(i64, String)>, String> {
    let wanted: BTreeSet<&(i64, String)> = sessions.iter().collect();
    let subjects: Vec<i64> = sessions
        .iter()
        .map(|(s, _)| *s)
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let mut out = BTreeSet::new();
    for chunk in subjects.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id FROM {} WHERE kind = 'image' AND subject_id IN ({list}) \
             AND session_day IS NOT NULL AND run_id IS NOT NULL AND withdrawn_at IS NULL \
             AND (check_status IS NULL OR check_status <> 'rejected') ORDER BY id DESC",
            store.qualified("derivative"),
        );
        let ids: Vec<i64> = store
            .query(&sql, &[])
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|r| r.int(0).ok())
            .collect();
        for row in nils_registry::derivative::of_ids(store, &ids).map_err(|e| e.to_string())? {
            let (Some(s), Some(day)) = (row.subject_id, row.session_day.clone()) else {
                continue;
            };
            let key = (s, day);
            if out.contains(&key) || !wanted.contains(&key) {
                continue;
            }
            if let Some(t) = outputs.of_derivative(store, &row)
                && t.role.as_deref() == Some(moving)
                && t.grid.as_deref() == Some(fixed)
            {
                out.insert(key);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::store::{Insert, Param};
    use nils_registry::{Backend, Home, InitOptions, Registry, Scheme};

    fn registry(dir: &std::path::Path, postgres: Option<(&str, &str)>) -> Registry {
        let home = Home::new(dir);
        home.keys(None).add("nils", b"a test passphrase").unwrap();
        home.init(&InitOptions {
            backend: if postgres.is_some() {
                Backend::Postgres
            } else {
                Backend::Sqlite
            },
            dsn: postgres.map(|(dsn, _)| dsn.to_string()),
            schema: postgres.map(|(_, schema)| schema.to_string()),
            scheme: Scheme::DEFAULT,
            key: "nils".into(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap()
    }

    /// The registration's aligned FLAIR says `aligned(flair, t1w)` for the
    /// sessions it was made for, on both backends: a rejected result, a
    /// withdrawn one, another role and another session do not.
    fn aligned_sessions_are_read(postgres: Option<&str>) {
        use nils_registry::derivative::{self, New};
        let dir = std::env::temp_dir().join(format!(
            "nils-typed-aligned-{}-{}",
            postgres.is_some(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let schema = "nils_typed_aligned";
        if let Some(dsn) = postgres {
            Store::connect_postgres(dsn, schema)
                .unwrap()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .unwrap();
        }
        let mut r = registry(&dir.join("reg"), postgres.map(|d| (d, schema)));
        let text = include_str!("../../../../pipelines/registration/ants/nils.job.yml");
        let added = crate::pipelines::add_descriptor(&mut r, text, None)
            .unwrap_or_else(|_| panic!("add the descriptor"));
        let pipeline = added["id"].as_i64().unwrap();
        let store = r.store();
        let table = nils_registry::schema::table;
        let subject = |code: &str, store: &mut Store| {
            store
                .insert(
                    &Insert::new(table("subject"), &["code", "created_at"]),
                    &[vec![Param::from(code), Param::from("2026-10-11T10:00:00Z")]],
                )
                .unwrap();
            let sql = format!(
                "SELECT id FROM {} WHERE code = {}",
                store.qualified("subject"),
                store.dialect().param(1, nils_registry::schema::Type::Text)
            );
            store
                .query_opt(&sql, &[Param::from(code)])
                .unwrap()
                .unwrap()
                .int(0)
                .unwrap()
        };
        let (s1, s2) = (subject("A1", store), subject("A2", store));
        let place = nils_registry::place::add(
            store,
            &nils_registry::place::New {
                name: "scratch",
                role: nils_registry::place::Role::Working,
                path: "/nowhere",
                guarantees: json!({}),
                probed: serde_json::Value::Null,
                handling: serde_json::Value::Null,
                dataset: serde_json::Value::Null,
            },
        )
        .unwrap();
        let run = nils_registry::pipeline::start(
            store,
            &nils_registry::pipeline::NewRun {
                pipeline_id: pipeline,
                job_id: None,
                handle_id: None,
                selection: None,
                params: &json!({}),
                runtime: "podman",
                runtime_version: "test",
                host: "lab",
                device: "cpu",
                model_ids: &[],
                label_set_id: None,
                place_id: Some(place),
                scratch_place_id: None,
                principal: "ops@lab",
                actor: None,
                started_at: "2026-10-11T10:00:00Z",
            },
        )
        .unwrap();
        let made = |store: &mut Store, subject: i64, day: &str, file: &str| {
            let belongs = derivative::belongs(store, None, None, Some(subject), Some(day))
                .unwrap_or_else(|_| panic!("subject {subject}"));
            let rel = format!("derivatives/ants/{run}/sub-x/ses-1/anat/{file}");
            derivative::insert_of_run(
                store,
                &New {
                    kind: "image",
                    belongs: &belongs,
                    place_id: place,
                    path: &rel,
                    bytes: 1,
                    sha256: &"0".repeat(64),
                    media_type: "application/x-nifti+gzip",
                    registered_by: "ops@lab",
                    actor: None,
                    model_id: None,
                    run_id: None,
                    preprocess_version: None,
                    supersedes_id: None,
                    created_at: "2026-10-11T10:00:00Z",
                },
                run,
            )
            .unwrap()
        };
        let flair = "sub-x_ses-1_space-T1w_desc-aligned_FLAIR.nii.gz";
        let t2w = "sub-x_ses-1_space-T1w_desc-aligned_T2w.nii.gz";
        // s1 on 2026-01-01: aligned FLAIR, taken; on 2026-02-01: only a T2w
        made(store, s1, "2026-01-01", flair);
        made(store, s1, "2026-02-01", t2w);
        // s2 on 2026-01-01: an aligned FLAIR a check rejected
        let rejected = made(store, s2, "2026-01-01", flair);
        derivative::set_check(
            store,
            rejected,
            Some("rejected"),
            &json!([]),
            None,
            Some("2026-10-11T10:00:00Z"),
        )
        .unwrap();
        let sessions: Vec<(i64, String)> = vec![
            (s1, "2026-01-01".into()),
            (s1, "2026-02-01".into()),
            (s2, "2026-01-01".into()),
            (s2, "2026-03-01".into()),
        ];
        let got =
            aligned_by_registration(store, &mut Outputs::default(), &sessions, "flair", "t1w")
                .unwrap();
        assert_eq!(
            got,
            [(s1, "2026-01-01".to_string())]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        drop(r);
        if let Some(dsn) = postgres {
            Store::connect_postgres(dsn, schema)
                .unwrap()
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
                ))
                .unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    use serde_json::json;

    #[test]
    fn the_registration_s_aligned_flair_tells_aligned() {
        aligned_sessions_are_read(None);
    }

    #[test]
    fn the_registration_s_aligned_flair_tells_aligned_on_postgres() {
        match std::env::var("NILS_TEST_POSTGRES_DSN") {
            Ok(dsn) if !dsn.is_empty() => aligned_sessions_are_read(Some(&dsn)),
            _ => eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped"),
        }
    }
}
