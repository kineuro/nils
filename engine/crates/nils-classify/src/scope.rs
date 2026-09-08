// SPDX-License-Identifier: AGPL-3.0-only

//! A classifier scope (Wave 4c §6.6): a batch, an origin or a pack version,
//! written `batch:<id>`, `origin:<name>` or `pack:<version>`. An origin
//! name is matched against the manufacturer, the model and the station of
//! the fingerprint, which are the three an overlay may be keyed on (C2).

use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Batch(i64),
    Origin(String),
    Pack(String),
}

impl Scope {
    pub fn parse(text: &str) -> Result<Scope, String> {
        let (kind, rest) = text
            .split_once(':')
            .ok_or_else(|| "a scope is batch:<id>, origin:<name> or pack:<version>".to_string())?;
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(format!("{kind}: names nothing"));
        }
        match kind {
            "batch" => rest
                .parse::<i64>()
                .map(Scope::Batch)
                .map_err(|_| format!("batch:{rest} is not a batch id")),
            "origin" => Ok(Scope::Origin(rest.to_string())),
            "pack" => Ok(Scope::Pack(rest.to_string())),
            other => Err(format!(
                "{other}: a scope is batch:<id>, origin:<name> or pack:<version>"
            )),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Scope::Batch(b) => format!("batch:{b}"),
            Scope::Origin(o) => format!("origin:{o}"),
            Scope::Pack(p) => format!("pack:{p}"),
        }
    }

    /// A subquery of the stack ids in scope, with its parameters, numbered
    /// from `first`. Written as `IN (...)` on a stack id column.
    pub fn stacks_sql(&self, store: &Store, first: usize) -> (String, Vec<Param>) {
        let d = store.dialect();
        match self {
            Scope::Batch(b) => (
                format!(
                    "(SELECT id FROM {} WHERE first_batch_id = {})",
                    store.qualified("stack"),
                    d.param(first, Type::Int)
                ),
                vec![Param::Int(*b)],
            ),
            Scope::Origin(o) => (
                format!(
                    "(SELECT stack_id FROM {} WHERE manufacturer = {} OR manufacturer_model_name = {} OR station_name = {})",
                    store.qualified("stack_fingerprint"),
                    d.param(first, Type::Text),
                    d.param(first + 1, Type::Text),
                    d.param(first + 2, Type::Text)
                ),
                vec![
                    Param::from(o.as_str()),
                    Param::from(o.as_str()),
                    Param::from(o.as_str()),
                ],
            ),
            Scope::Pack(v) => (
                format!(
                    "(SELECT stack_id FROM {} WHERE pack_version = {})",
                    store.qualified("classification"),
                    d.param(first, Type::Text)
                ),
                vec![Param::from(v.as_str())],
            ),
        }
    }

    /// The batches the scope touches: the one batch, or every batch a stack
    /// in scope first arrived in.
    pub fn batches_sql(&self, store: &Store, first: usize) -> (String, Vec<Param>) {
        match self {
            Scope::Batch(b) => (
                format!("({})", store.dialect().param(first, Type::Int)),
                vec![Param::Int(*b)],
            ),
            _ => {
                let (stacks, params) = self.stacks_sql(store, first);
                (
                    format!(
                        "(SELECT DISTINCT first_batch_id FROM {} WHERE id IN {stacks})",
                        store.qualified("stack")
                    ),
                    params,
                )
            }
        }
    }
}
