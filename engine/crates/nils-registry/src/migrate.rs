// SPDX-License-Identifier: AGPL-3.0-only

//! Migrations (`docs/specs/wave1-parse-and-digest.md`, §4.1): numbered steps,
//! applied in order inside a transaction each, the version kept in the meta
//! table of each store. `nils init` runs them all; opening a store behind the
//! binary runs the missing ones; a store ahead of the binary is refused.

use std::fmt;

use crate::schema::{self, ID_TYPES, Table, linkage_tables, registry_tables};
use crate::store::{Error, Param, Store};

/// The version this binary writes.
pub const SCHEMA_VERSION: i64 = 10;

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
    Migration {
        version: 3,
        apply: create_classification,
    },
    Migration {
        version: 4,
        apply: evidence_says_which_pass,
    },
    Migration {
        version: 5,
        apply: study_carries_the_date_it_was_given,
    },
    Migration {
        version: 6,
        apply: create_session_scheme,
    },
    Migration {
        version: 7,
        apply: fingerprint_carries_what_it_worked_out,
    },
    Migration {
        version: 8,
        apply: study_says_whether_it_holds_a_primary,
    },
    Migration {
        version: 9,
        apply: diffusion_is_recorded_per_image,
    },
    Migration {
        version: 10,
        apply: a_decision_says_who_made_it,
    },
];

/// Wave 3 §10.1: a decision records whether a person, an agent or a model made
/// it, and the evidence a decision writes says so too.
///
/// A registry from before this has decisions with no kind. They are people's:
/// nothing else could have written one, because nothing else could reach the
/// verb. So the column is backfilled rather than left null, which is the one
/// case where a default is a fact and not a guess.
fn a_decision_says_who_made_it(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "decision", &["author_kind", "author_version"])?;
    add_columns(store, "classification_evidence", &["author", "author_kind"])?;
    let sql = format!(
        "UPDATE {} SET author_kind = 'person' WHERE author_kind IS NULL",
        store.qualified("decision")
    );
    store.execute(&sql, &[])?;
    Ok(())
}

/// Wave 3 §6: the seven diffusion values that vary from one image of a series
/// to the next move to the instance, and the fingerprint gains what it works
/// out from them.
///
/// A b value, a gradient orientation and a directionality are per image by
/// design: that is what a multi-shell, multi-direction acquisition is. Keeping
/// one per series records such a series as its smallest shell and its gradient
/// count as one. The columns on `series_mr` are left where they are in a
/// registry that already has them, unread, because a migration adds and does
/// not take away.
fn diffusion_is_recorded_per_image(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "instance",
        &[
            "diffusion_b_value",
            "diffusion_gradient_orientation",
            "diffusion_directionality",
            "dwi_siemens_b_value",
            "dwi_siemens_directionality",
            "dwi_ge_b_value",
            "dwi_philips_b_value",
        ],
    )?;
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "dwi_b_value",
            "dwi_b_values",
            "dwi_b_value_source",
            "dwi_pe_direction",
            "dwi_pe_direction_source",
            "dwi_directions",
            "dwi_directions_source",
        ],
    )
}

/// Wave 3 §6: a study says whether any of its stacks is one the scanner called
/// its output, which is half of the session rescue. The other half is the
/// scheme, and it is applied on read.
fn study_says_whether_it_holds_a_primary(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "study", &["has_original_primary"])
}

/// Wave 3 §6: the fields the fingerprint derives rather than reads, each beside
/// the measured column it came from. A registry from Wave 2 gains six columns;
/// one created now has them already.
fn fingerprint_carries_what_it_worked_out(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "field_strength_tesla",
            "field_strength_normalized",
            "field_strength_unit",
            "acquisition_type_filled",
            "acquisition_type_source",
            "image_role",
        ],
    )
}

/// Wave 3 §5: the registry keeps the schemes it derives sessions with, so a
/// labelling can be reproduced from the registry alone. A registry created at
/// version 1 gains the table; one created now already has it.
fn create_session_scheme(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["session_scheme"])
}

/// Wave 3 §4: a study whose `StudyDate` said nothing carries the day the vote
/// found, the source that carried the most weight for it, and how close the
/// vote was. Never over the measured column: a registry that has the table
/// from version 1 gains four, one created now has them already.
fn study_carries_the_date_it_was_given(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "study",
        &[
            "date_filled",
            "date_source",
            "date_weight",
            "date_runner_up",
        ],
    )
}

/// Wave 2 §7: a pass writes evidence like a rule does, and says which pass it
/// was and which named reference it voted against. A registry that has the
/// table from version 3 gains the two columns; one created now has them
/// already.
fn evidence_says_which_pass(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "classification_evidence", &["pass", "reference"])
}

/// Wave 2 §8: what a pack decided, what made it decide, and the decisions
/// that outrank it.
fn create_classification(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "classification",
            "classification_axis",
            "classification_evidence",
            "decision",
        ],
    )
}

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

/// Add declared columns a table has not got yet. A column is added, never
/// changed: what an older binary wrote stays readable.
fn add_columns(store: &mut Store, table: &str, names: &[&str]) -> Result<(), Error> {
    if !table_exists(store, table)? {
        return Ok(());
    }
    let dialect = store.dialect();
    let qualified = store.qualified(table);
    let declared = schema::table(table);
    for name in names {
        if column_exists(store, table, name)? {
            continue;
        }
        let column = declared
            .column(name)
            .unwrap_or_else(|| panic!("{table}.{name} is not a declared column"));
        store.batch(&format!(
            "ALTER TABLE {qualified} ADD COLUMN {name} {}",
            dialect.type_name(column.ty)
        ))?;
    }
    Ok(())
}

fn column_exists(store: &mut Store, table: &str, column: &str) -> Result<bool, Error> {
    Ok(match store {
        Store::Sqlite(_) => store
            .query(&format!("PRAGMA table_info({table})"), &[])?
            .iter()
            .any(|r| r.text(1).map(|n| n == column).unwrap_or(false)),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 AND column_name = $3",
                    &[Param::from(schema), Param::from(table), Param::from(column)],
                )?
                .is_some()
        }
    })
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

#[cfg(test)]
mod column_migration {
    use super::*;

    /// A registry written before Wave 2's passes opens under this binary with
    /// the two columns added rather than being rebuilt.
    #[test]
    fn a_column_a_later_wave_declared_is_added_to_an_existing_table() {
        let mut store = Store::sqlite_in_memory().unwrap();
        // Everything up to the version that created the table, and no further.
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 3) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        store
            .batch("ALTER TABLE classification_evidence DROP COLUMN pass")
            .unwrap();
        store
            .batch("ALTER TABLE classification_evidence DROP COLUMN reference")
            .unwrap();
        assert!(!column_exists(&mut store, "classification_evidence", "pass").unwrap());

        evidence_says_which_pass(&mut store, Kind::Registry).unwrap();
        assert!(column_exists(&mut store, "classification_evidence", "pass").unwrap());
        assert!(column_exists(&mut store, "classification_evidence", "reference").unwrap());
        // and again, because a migration that has run must be safe to run
        evidence_says_which_pass(&mut store, Kind::Registry).unwrap();
    }
}
