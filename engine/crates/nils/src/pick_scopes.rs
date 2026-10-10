// SPDX-License-Identifier: AGPL-3.0-only

//! Which scopes a pick run decides (record 55, decision 6; the main-scans
//! study, section 4). A run decides the pack's scope, made with version 1
//! of the rules, for its subjects; and every dataset or cohort with rules of
//! its own saved that holds one of them, made with its latest version, for
//! those of its subjects. A run with `--own-rules`, which saving rules
//! queues, decides the one scope alone.

use std::collections::BTreeSet;

use nils_classify::picking::RunScope;
use nils_registry::pick_rules::{self, Scope, ScopeKind};
use nils_registry::schema::Type;
use nils_registry::store::{Error as StoreError, Param, Store};

/// Why the scopes of a run were not found.
#[derive(Debug)]
pub(crate) enum Error {
    /// The ask was wrong in a way a person can mend.
    Usage(String),
    Store(StoreError),
}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Error::Store(e)
    }
}

/// The subjects of a scope: a cohort's open members, a dataset's subjects.
pub(crate) fn subjects_of(store: &mut Store, scope: &Scope) -> Result<BTreeSet<i64>, StoreError> {
    match scope.kind {
        ScopeKind::Cohort => nils_registry::cohort::open_members(store, scope.id),
        ScopeKind::Dataset => match nils_registry::place::show(store, scope.id)? {
            Some(place) => crate::chain::dataset_subjects(store, &place),
            None => Ok(BTreeSet::new()),
        },
    }
}

/// Whether a scope with rules saved is still in force: a cohort not
/// retired, a dataset whose place is a source in force.
fn in_force(store: &mut Store, scope: &Scope) -> Result<bool, StoreError> {
    Ok(match scope.kind {
        ScopeKind::Cohort => nils_registry::cohort::by_name(store, &scope.name)?
            .is_some_and(|c| c.id == scope.id && c.retired_at.is_none()),
        ScopeKind::Dataset => nils_registry::place::show(store, scope.id)?.is_some_and(|p| {
            p.role == nils_registry::place::Role::Source && p.retired_at.is_none()
        }),
    })
}

/// The scopes a `pick run` decides. `only` is the subjects it was asked
/// for (a cohort's, a dataset's, a sort's), `subject` the one subject it
/// was narrowed to; neither, every subject.
pub(crate) fn of_run(
    store: &mut Store,
    own_rules: bool,
    cohort: Option<&str>,
    dataset: Option<&str>,
    only: Option<&BTreeSet<i64>>,
    subject: Option<&str>,
) -> Result<Vec<RunScope>, Error> {
    if own_rules {
        let scope = match (cohort, dataset) {
            (Some(c), _) => Scope::cohort(store, c)?
                .ok_or_else(|| Error::Usage(format!("no cohort named {c}")))?,
            (None, Some(d)) => Scope::dataset(store, d)?
                .ok_or_else(|| Error::Usage(format!("no dataset named {d}")))?,
            (None, None) => {
                return Err(Error::Usage(
                    "--own-rules decides a cohort's or a dataset's picks; name one with --cohort or --dataset".into(),
                ));
            }
        };
        let saved = pick_rules::latest(store, &scope)?.ok_or_else(|| {
            Error::Usage(format!(
                "{} {} has no rules of its own saved, so it reads the pack's picks",
                scope.kind.name(),
                scope.name
            ))
        })?;
        return Ok(vec![RunScope {
            scope: Some(scope),
            saved: Some(saved),
            subjects: only.cloned(),
        }]);
    }
    let one: Option<i64> = match subject {
        Some(code) => store
            .query_opt(
                &format!(
                    "SELECT id FROM {} WHERE code = {}",
                    store.qualified("subject"),
                    store.dialect().param(1, Type::Text)
                ),
                &[Param::from(code)],
            )?
            .map(|r| r.int(0))
            .transpose()?,
        None => None,
    };
    let mut out = vec![RunScope::pack(only.cloned())];
    for (kind, id) in pick_rules::scopes(store)? {
        let Some(scope) = Scope::of(store, kind, id)? else {
            continue;
        };
        if !in_force(store, &scope)? {
            continue;
        }
        let members = subjects_of(store, &scope)?;
        let subjects: BTreeSet<i64> = match (only, subject.is_some()) {
            (Some(o), _) => members.intersection(o).copied().collect(),
            (None, true) => members.into_iter().filter(|s| Some(*s) == one).collect(),
            (None, false) => members,
        };
        if subjects.is_empty() {
            continue;
        }
        let Some(saved) = pick_rules::latest(store, &scope)? else {
            continue;
        };
        out.push(RunScope {
            scope: Some(scope),
            saved: Some(saved),
            subjects: Some(subjects),
        });
    }
    Ok(out)
}
