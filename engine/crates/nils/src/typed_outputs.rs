// SPDX-License-Identifier: AGPL-3.0-only
//! Typed outputs and typed links (Wave 7b spec section 8.2): a derivative
//! is typed by the output of the descriptor that made it, which says its
//! kind, the role and contrast it stands for and the grid or space it is
//! on; a `derivative:<kind>` input that asks for a role, a contrast or a
//! grid takes only a derivative whose output matches. The first link: the
//! registration family's aligned FLAIR (kind image, role flair, grid t1w),
//! which SAMSEG and segcsvd read in place of the header-only resampling,
//! and which tells the pack's `aligned(flair, t1w)` fact true.
use std::collections::BTreeMap;

use nils_pipeline::descriptor::{self, TypedInput};
use nils_registry::derivative::Derivative;
use nils_registry::pipeline as rows;
use nils_registry::store::{Param, Store};

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
    /// the output of its kind whose path template its path matches.
    pub(crate) fn of_derivative(&mut self, store: &mut Store, row: &Derivative) -> Option<Typed> {
        let run_id = row.run_id?;
        let marker = format!("/{run_id}/");
        let rel = row
            .path
            .split_once(&marker)
            .map(|(_, rel)| rel.to_string())
            .unwrap_or_else(|| row.path.clone());
        let outputs = self.of_run(store, run_id)?;
        outputs
            .iter()
            .find(|o| {
                o.kind == row.kind && nils_pipeline::files::matches_template(&o.template, &rel)
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

/// Whether a registration step wrote an image of role `moving` on the grid
/// of `fixed` for this session: a derivative of kind image of the session,
/// not withdrawn, whose output says so. The pack fact `aligned(moving,
/// fixed)` reads true where it did.
pub(crate) fn aligned_by_registration(
    store: &mut Store,
    subject_id: i64,
    day: &str,
    moving: &str,
    fixed: &str,
) -> bool {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE kind = 'image' AND subject_id = {} AND session_day = {} \
         AND run_id IS NOT NULL AND withdrawn_at IS NULL ORDER BY id DESC",
        store.qualified("derivative"),
        d.param(1, nils_registry::schema::Type::Int),
        d.param(2, nils_registry::schema::Type::Text)
    );
    let ids: Vec<i64> = store
        .query(&sql, &[Param::Int(subject_id), Param::from(day)])
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.int(0).ok())
        .collect();
    let mut outputs = Outputs::default();
    for id in ids {
        let Ok(Some(row)) = nils_registry::derivative::get(store, id) else {
            continue;
        };
        if let Some(t) = outputs.of_derivative(store, &row)
            && t.role.as_deref() == Some(moving)
            && t.grid.as_deref() == Some(fixed)
        {
            return true;
        }
    }
    false
}
