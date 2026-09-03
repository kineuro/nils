// SPDX-License-Identifier: AGPL-3.0-only

//! Migrations (`docs/specs/wave1-parse-and-digest.md`, §4.1): numbered steps,
//! applied in order inside a transaction each, the version kept in the meta
//! table of each store. `nils init` runs them all; opening a store behind the
//! binary runs the missing ones; a store ahead of the binary is refused.

use std::fmt;

use crate::schema::{ID_TYPES, Table, linkage_tables, registry_tables};
use crate::store::{Error, Param, Store};

/// The version this binary writes.
pub const SCHEMA_VERSION: i64 = 2;

/// Which of the two stores a migration runs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Registry,
    Linkage,
}

impl Kind {
    /// The meta table of the store.
    pub fn meta_table(self) -> &'static str {
        match self {
            Kind::Registry => "registry_meta",
            Kind::Linkage => "linkage_meta",
        }
    }

    pub fn tables(self) -> &'static [Table] {
        match self {
            Kind::Registry => registry_tables(),
            Kind::Linkage => linkage_tables(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Registry => "registry",
            Kind::Linkage => "linkage store",
        }
    }
}

/// One migration.
pub struct Migration {
    pub version: i64,
    pub apply: fn(&mut Store, Kind) -> Result<(), Error>,
}

/// Every migration, in order. The first creates the schema as declared; the
/// rest add what a later wave declared, so that a registry written by an older
/// binary opens under a newer one without being rebuilt.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        apply: create_declared_tables,
    },
    Migration {
        version: 2,
        apply: create_stack_fingerprint,
    },
];

/// Wave 2 §4.2. A registry created at version 1 gains the table; one created
/// now already has it from [`create_declared_tables`], so this is a no-op
/// there and the two paths reach the same schema.
fn create_stack_fingerprint(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["stack_fingerprint"])
}

/// Create the named declared tables if they are not there yet.
fn add_tables(store: &mut Store, kind: Kind, names: &[&str]) -> Result<(), Error> {
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    for t in kind.tables().iter().filter(|t| names.contains(&t.name)) {
        if table_exists(store, t.name)? {
            continue;
        }
        store.batch(&dialect.create_table(schema.as_deref(), t))?;
        for ix in dialect.create_indexes(schema.as_deref(), t) {
            store.batch(&ix)?;
        }
    }
    Ok(())
}

fn table_exists(store: &mut Store, name: &str) -> Result<bool, Error> {
    Ok(match store {
        Store::Sqlite(_) => store
            .query_opt(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
                &[Param::from(name)],
            )?
            .is_some(),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2",
                    &[Param::from(schema), Param::from(name)],
                )?
                .is_some()
        }
    })
}

fn create_declared_tables(store: &mut Store, kind: Kind) -> Result<(), Error> {
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    for t in kind.tables() {
        store.batch(&dialect.create_table(schema.as_deref(), t))?;
        for ix in dialect.create_indexes(schema.as_deref(), t) {
            store.batch(&ix)?;
        }
    }
    if kind == Kind::Linkage {
        let table = store.qualified("id_type");
        let sql = format!(
            "INSERT INTO {table} (name, description) VALUES ({}, {})",
            dialect.param(1, crate::schema::Type::Text),
            dialect.param(2, crate::schema::Type::Text)
        );
        for (name, description) in ID_TYPES {
            store.execute(&sql, &[Param::from(name), Param::from(description)])?;
        }
    }
    Ok(())
}

/// The store's version: `None` when it has no meta table at all.
pub fn version_of(store: &mut Store, kind: Kind) -> Result<Option<i64>, Error> {
    let exists = match store {
        Store::Sqlite(_) => store
            .query_opt(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
                &[Param::from(kind.meta_table())],
            )?
            .is_some(),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2",
                    &[Param::from(schema), Param::from(kind.meta_table())],
                )?
                .is_some()
        }
    };
    if !exists {
        return Ok(None);
    }
    let table = store.qualified(kind.meta_table());
    let sql = format!(
        "SELECT value FROM {table} WHERE key = {}",
        store.dialect().param(1, crate::schema::Type::Text)
    );
    match store.query_opt(&sql, &[Param::from("schema_version")])? {
        Some(row) => {
            let v = row.text(0)?.parse::<i64>().map_err(|_| {
                Error::Message(format!("{}: schema_version is not a number", kind.name()))
            })?;
            Ok(Some(v))
        }
        None => Ok(Some(0)),
    }
}

/// Where a store stands against the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Nothing there yet.
    Empty,
    /// The binary's version.
    Current,
    /// Migrations are pending.
    Behind(i64),
    /// Written by a newer binary.
    Ahead(i64),
}

impl fmt::Display for Standing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Standing::Empty => f.write_str("empty"),
            Standing::Current => write!(f, "schema version {SCHEMA_VERSION}"),
            Standing::Behind(v) => write!(
                f,
                "schema version {v}, behind this binary's {SCHEMA_VERSION}"
            ),
            Standing::Ahead(v) => write!(
                f,
                "schema version {v}, ahead of this binary's {SCHEMA_VERSION}"
            ),
        }
    }
}

pub fn standing(store: &mut Store, kind: Kind) -> Result<Standing, Error> {
    Ok(match version_of(store, kind)? {
        None => Standing::Empty,
        Some(v) if v == SCHEMA_VERSION => Standing::Current,
        Some(v) if v < SCHEMA_VERSION => Standing::Behind(v),
        Some(v) => Standing::Ahead(v),
    })
}

/// Apply every migration after the store's version, each in its own
/// transaction, and record the version. Returns the versions applied.
pub fn migrate(store: &mut Store, kind: Kind) -> Result<Vec<i64>, Error> {
    let from = match standing(store, kind)? {
        Standing::Empty => 0,
        Standing::Current => return Ok(Vec::new()),
        Standing::Behind(v) => v,
        Standing::Ahead(v) => {
            return Err(Error::Message(format!(
                "the {} has schema version {v}, ahead of this binary's {SCHEMA_VERSION}; use a newer nils",
                kind.name()
            )));
        }
    };
    let mut applied = Vec::new();
    for m in MIGRATIONS.iter().filter(|m| m.version > from) {
        store.begin()?;
        let result = (m.apply)(store, kind).and_then(|()| set_version(store, kind, m.version));
        match result {
            Ok(()) => store.commit()?,
            Err(e) => {
                let _ = store.rollback();
                return Err(e);
            }
        }
        applied.push(m.version);
    }
    Ok(applied)
}

fn set_version(store: &mut Store, kind: Kind, version: i64) -> Result<(), Error> {
    let table = store.qualified(kind.meta_table());
    let d = store.dialect();
    let sql = format!(
        "INSERT INTO {table} (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        d.param(1, crate::schema::Type::Text),
        d.param(2, crate::schema::Type::Text)
    );
    store.execute(
        &sql,
        &[
            Param::from("schema_version"),
            Param::from(version.to_string()),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_from_one_without_gaps() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.version, i as i64 + 1);
        }
        assert_eq!(MIGRATIONS.last().unwrap().version, SCHEMA_VERSION);
    }

    #[test]
    fn an_empty_sqlite_store_is_created_and_then_current() {
        let mut store = Store::sqlite_in_memory().unwrap();
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Empty
        );
        // Every migration runs on a new store; the later ones are no-ops
        // there, since migration 1 creates every declared table.
        let applied: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        assert_eq!(migrate(&mut store, Kind::Registry).unwrap(), applied);
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Current
        );
        assert!(migrate(&mut store, Kind::Registry).unwrap().is_empty());
        let tables = store
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
                &[],
            )
            .unwrap();
        let names: Vec<&str> = tables.iter().map(|r| r.text(0).unwrap()).collect();
        for t in registry_tables() {
            assert!(names.contains(&t.name), "{} missing", t.name);
        }
        // a store from the future is refused
        store
            .execute(
                "UPDATE registry_meta SET value = '99' WHERE key = 'schema_version'",
                &[],
            )
            .unwrap();
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Ahead(99)
        );
        let err = migrate(&mut store, Kind::Registry).unwrap_err().to_string();
        assert!(err.contains("ahead of this binary"), "{err}");
    }

    #[test]
    fn the_linkage_store_is_seeded_with_its_id_types() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate(&mut store, Kind::Linkage).unwrap();
        let rows = store
            .query("SELECT name FROM id_type ORDER BY id", &[])
            .unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.text(0).unwrap()).collect();
        assert_eq!(names, vec!["patient-id", "study-instance-uid"]);
        assert_eq!(
            standing(&mut store, Kind::Linkage).unwrap(),
            Standing::Current
        );
    }
}
