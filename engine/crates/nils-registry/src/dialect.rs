// SPDX-License-Identifier: AGPL-3.0-only

//! The dialect layer (`docs/specs/wave1-parse-and-digest.md`, §4.1): the SQL
//! that differs between SQLite and Postgres, and nothing else. Type names,
//! identity columns, placeholders with their casts, `ON CONFLICT`, `RETURNING`,
//! the bulk path's temporary tables and `COPY`. Everything here is rendered from
//! the declaration in [`crate::schema`].

use std::fmt::Write as _;

use crate::Backend;
use crate::schema::{Column, Table, Type};

/// The two dialects, one per backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

/// What happens when an insert hits a unique key.
#[derive(Debug, Clone, Copy)]
pub enum Conflict<'a> {
    /// The statement fails.
    Fail,
    /// The row is dropped (`ON CONFLICT (target) DO NOTHING`).
    Nothing(&'a [&'a str]),
    /// The listed columns are overwritten from the new row.
    Update {
        target: &'a [&'a str],
        set: &'a [&'a str],
    },
}

/// A table name with its schema on Postgres, bare on SQLite.
pub fn qualified(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{s}.{table}"),
        None => table.to_string(),
    }
}

impl Dialect {
    pub fn of(backend: Backend) -> Dialect {
        match backend {
            Backend::Sqlite => Dialect::Sqlite,
            Backend::Postgres => Dialect::Postgres,
        }
    }

    /// The column type, from the table of §4.1.
    pub fn type_name(self, ty: Type) -> &'static str {
        match (self, ty) {
            (Dialect::Sqlite, Type::Id) => "INTEGER PRIMARY KEY",
            (Dialect::Sqlite, Type::Text) => "TEXT",
            (Dialect::Sqlite, Type::Int) => "INTEGER",
            (Dialect::Sqlite, Type::Double) => "REAL",
            (Dialect::Sqlite, Type::Bool) => "INTEGER",
            (Dialect::Sqlite, Type::Date | Type::Time | Type::Timestamp | Type::Json) => "TEXT",
            (Dialect::Sqlite, Type::Bytes) => "BLOB",
            (Dialect::Postgres, Type::Id) => "BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY",
            (Dialect::Postgres, Type::Text) => "TEXT",
            (Dialect::Postgres, Type::Int) => "BIGINT",
            (Dialect::Postgres, Type::Double) => "DOUBLE PRECISION",
            (Dialect::Postgres, Type::Bool) => "BOOLEAN",
            (Dialect::Postgres, Type::Date) => "DATE",
            (Dialect::Postgres, Type::Time) => "TIME",
            (Dialect::Postgres, Type::Timestamp) => "TIMESTAMPTZ",
            (Dialect::Postgres, Type::Json) => "JSONB",
            (Dialect::Postgres, Type::Bytes) => "BYTEA",
        }
    }

    /// The `n`th placeholder (1-based) for a value of type `ty`. Postgres
    /// receives dates, times, timestamps and JSON as text and casts them; the
    /// double cast keeps the parameter's inferred type `text`.
    pub fn param(self, n: usize, ty: Type) -> String {
        match self {
            Dialect::Sqlite => "?".to_string(),
            Dialect::Postgres => match ty {
                Type::Date => format!("${n}::text::date"),
                Type::Time => format!("${n}::text::time"),
                Type::Timestamp => format!("${n}::text::timestamptz"),
                Type::Json => format!("${n}::text::jsonb"),
                _ => format!("${n}"),
            },
        }
    }

    /// An expression that reads a column back as the text the engine wrote:
    /// the column itself on SQLite; on Postgres, dates and JSON cast to text,
    /// times with their six fraction digits, timestamps in UTC.
    pub fn text_of(self, column: &Column) -> String {
        self.text_of_qualified(None, column)
    }

    /// [`Dialect::text_of`] for a column of an aliased table in a join, so
    /// that a select across several tables reads a date, a time, a timestamp
    /// or a JSON column as text on either backend. Postgres hands those back
    /// in types the store does not read, and a select that forgets the cast
    /// fails only once a row of that shape exists, which is the worst time.
    pub fn text_of_qualified(self, alias: Option<&str>, column: &Column) -> String {
        let name = match alias {
            Some(a) => format!("{a}.{}", column.name),
            None => column.name.to_string(),
        };
        match (self, column.ty) {
            (Dialect::Postgres, Type::Date | Type::Json) => format!("{name}::text"),
            (Dialect::Postgres, Type::Time) => format!("to_char({name}, 'HH24:MI:SS.US')"),
            (Dialect::Postgres, Type::Timestamp) => {
                format!("to_char({name} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
            }
            _ => name,
        }
    }

    /// `CREATE TABLE` for a declared table.
    pub fn create_table(self, schema: Option<&str>, t: &Table) -> String {
        let mut sql = format!("CREATE TABLE {} (\n", qualified(schema, t.name));
        for (i, c) in t.columns.iter().enumerate() {
            if i > 0 {
                sql.push_str(",\n");
            }
            let _ = write!(sql, "  {} {}", c.name, self.type_name(c.ty));
            if c.not_null && c.ty != Type::Id {
                sql.push_str(" NOT NULL");
            }
        }
        if let Some(pk) = t.primary {
            let _ = write!(sql, ",\n  PRIMARY KEY ({pk})");
        }
        for key in &t.uniques {
            let _ = write!(
                sql,
                ",\n  CONSTRAINT uq_{}_{} UNIQUE ({})",
                t.name,
                key.join("_"),
                key.join(", ")
            );
        }
        sql.push_str("\n)");
        sql
    }

    /// `CREATE INDEX` for each declared index.
    pub fn create_indexes(self, schema: Option<&str>, t: &Table) -> Vec<String> {
        t.indexes
            .iter()
            .map(|key| {
                format!(
                    "CREATE INDEX IF NOT EXISTS ix_{}_{} ON {} ({})",
                    t.name,
                    key.join("_"),
                    qualified(schema, t.name),
                    key.join(", ")
                )
            })
            .collect()
    }

    /// The `ON CONFLICT` clause.
    fn conflict_clause(self, conflict: Conflict<'_>) -> String {
        match conflict {
            Conflict::Fail => String::new(),
            Conflict::Nothing(target) => {
                format!(" ON CONFLICT ({}) DO NOTHING", target.join(", "))
            }
            Conflict::Update { target, set } => {
                let sets: Vec<String> = set.iter().map(|c| format!("{c} = excluded.{c}")).collect();
                format!(
                    " ON CONFLICT ({}) DO UPDATE SET {}",
                    target.join(", "),
                    sets.join(", ")
                )
            }
        }
    }

    fn returning_clause(returning: &[&str]) -> String {
        if returning.is_empty() {
            String::new()
        } else {
            format!(" RETURNING {}", returning.join(", "))
        }
    }

    /// An `INSERT` of `rows` rows of the listed columns, the placeholders
    /// numbered across the rows.
    pub fn insert(
        self,
        schema: Option<&str>,
        t: &Table,
        columns: &[&Column],
        rows: usize,
        conflict: Conflict<'_>,
        returning: &[&str],
    ) -> String {
        let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
        let mut sql = format!(
            "INSERT INTO {} ({}) VALUES ",
            qualified(schema, t.name),
            names.join(", ")
        );
        let mut n = 0;
        for r in 0..rows {
            if r > 0 {
                sql.push_str(", ");
            }
            sql.push('(');
            for (i, c) in columns.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                n += 1;
                sql.push_str(&self.param(n, c.ty));
            }
            sql.push(')');
        }
        sql.push_str(&self.conflict_clause(conflict));
        sql.push_str(&Self::returning_clause(returning));
        sql
    }

    /// The name of the temporary table the bulk path fills.
    pub fn temp_name(t: &Table) -> String {
        format!("bulk_{}", t.name)
    }

    /// The temporary table of the Postgres bulk path: the listed columns,
    /// every one nullable. It is emptied by the store before each `COPY`, not
    /// at commit, so the path also works outside a transaction.
    pub fn create_temp(self, t: &Table, columns: &[&Column]) -> String {
        let cols: Vec<String> = columns
            .iter()
            .map(|c| format!("{} {}", c.name, self.type_name(c.ty)))
            .collect();
        format!(
            "CREATE TEMP TABLE IF NOT EXISTS {} ({})",
            Self::temp_name(t),
            cols.join(", ")
        )
    }

    /// `COPY` into the temporary table, text format.
    pub fn copy_temp(t: &Table, columns: &[&Column]) -> String {
        let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
        format!(
            "COPY {} ({}) FROM STDIN",
            Self::temp_name(t),
            names.join(", ")
        )
    }

    /// The merge from the temporary table into the real one.
    pub fn merge_temp(
        self,
        schema: Option<&str>,
        t: &Table,
        columns: &[&Column],
        conflict: Conflict<'_>,
        returning: &[&str],
    ) -> String {
        let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
        format!(
            "INSERT INTO {} ({}) SELECT {} FROM {}{}{}",
            qualified(schema, t.name),
            names.join(", "),
            names.join(", "),
            Self::temp_name(t),
            self.conflict_clause(conflict),
            Self::returning_clause(returning)
        )
    }

    /// `SELECT <columns as text> FROM t WHERE key IN (...)`: the placeholders
    /// are one per key on SQLite and one array on Postgres, of `bigint` for
    /// an integer key, `bytea` for a bytes key and `text` otherwise.
    pub fn select_by_keys(
        self,
        schema: Option<&str>,
        t: &Table,
        columns: &[&Column],
        key: &str,
        key_ty: Type,
        keys: usize,
    ) -> String {
        let exprs: Vec<String> = columns.iter().map(|c| self.text_of(c)).collect();
        let filter = match self {
            Dialect::Sqlite => {
                let marks = vec!["?"; keys];
                format!("{key} IN ({})", marks.join(", "))
            }
            Dialect::Postgres => match key_ty {
                Type::Int | Type::Id => format!("{key} = ANY($1::bigint[])"),
                Type::Bytes => format!("{key} = ANY($1::bytea[])"),
                _ => format!("{key} = ANY($1::text[])"),
            },
        };
        format!(
            "SELECT {} FROM {} WHERE {}",
            exprs.join(", "),
            qualified(schema, t.name),
            filter
        )
    }

    /// `UPDATE t SET <set> FROM (VALUES ...) AS v(key, val) WHERE t.<key> =
    /// v.key`, for `rows` integer pairs. `set` names the new value as `v.val`
    /// (`n_instances = n_instances + v.val`, `source_file_id = v.val`).
    pub fn update_from_values(
        self,
        schema: Option<&str>,
        t: &Table,
        set: &str,
        key: &str,
        rows: usize,
    ) -> String {
        let mut values = String::new();
        let mut n = 0;
        for r in 0..rows {
            if r > 0 {
                values.push_str(", ");
            }
            n += 1;
            let a = self.param(n, Type::Int);
            n += 1;
            let b = self.param(n, Type::Int);
            match self {
                Dialect::Sqlite => values.push_str(&format!("({a}, {b})")),
                Dialect::Postgres => values.push_str(&format!("({a}::bigint, {b}::bigint)")),
            }
        }
        let table = qualified(schema, t.name);
        match self {
            Dialect::Sqlite => format!(
                "UPDATE {table} SET {set} FROM (SELECT column1 AS key, column2 AS val FROM (VALUES {values})) AS v WHERE {table}.{key} = v.key"
            ),
            Dialect::Postgres => format!(
                "UPDATE {table} AS t SET {set} FROM (VALUES {values}) AS v(key, val) WHERE t.{key} = v.key"
            ),
        }
    }
}

/// One value in `COPY ... FROM STDIN` text format, appended to `out`: `\N` for
/// null; a backslash, tab, newline and carriage return escaped; bytes as the
/// hex form `\\x..` that BYTEA reads.
pub fn copy_text(out: &mut Vec<u8>, text: Option<&[u8]>) {
    match text {
        None => out.extend_from_slice(b"\\N"),
        Some(bytes) => {
            for &b in bytes {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\t' => out.extend_from_slice(b"\\t"),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    _ => out.push(b),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::table;

    #[test]
    fn sqlite_and_postgres_render_the_same_table_differently() {
        let t = table("subject");
        let sqlite = Dialect::Sqlite.create_table(None, t);
        let pg = Dialect::Postgres.create_table(Some("nils"), t);
        assert!(sqlite.starts_with("CREATE TABLE subject (\n  id INTEGER PRIMARY KEY,\n"));
        assert!(pg.starts_with(
            "CREATE TABLE nils.subject (\n  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,\n"
        ));
        assert!(
            sqlite.contains("  code TEXT NOT NULL,\n  code_digest BLOB,\n  birth_date TEXT,\n")
        );
        assert!(pg.contains("  code TEXT NOT NULL,\n  code_digest BYTEA,\n  birth_date DATE,\n"));
        assert!(sqlite.ends_with("CONSTRAINT uq_subject_code UNIQUE (code)\n)"));
        let detail = Dialect::Postgres.create_table(Some("nils"), table("series_mr"));
        assert!(detail.contains("  series_id BIGINT NOT NULL,\n"));
        assert!(detail.ends_with("  PRIMARY KEY (series_id)\n)"));
        assert_eq!(
            Dialect::Sqlite.create_indexes(None, table("source_file"))[0],
            "CREATE INDEX IF NOT EXISTS ix_source_file_source_id_dir ON source_file (source_id, dir)"
        );
    }

    #[test]
    fn inserts_number_their_placeholders_and_cast_on_postgres() {
        let t = table("study");
        let cols: Vec<&Column> = ["study_instance_uid", "subject_id", "study_date"]
            .iter()
            .map(|c| t.column(c).unwrap())
            .collect();
        let sqlite = Dialect::Sqlite.insert(
            None,
            t,
            &cols,
            2,
            Conflict::Nothing(&["study_instance_uid"]),
            &["id", "study_instance_uid"],
        );
        assert_eq!(
            sqlite,
            "INSERT INTO study (study_instance_uid, subject_id, study_date) VALUES (?, ?, ?), (?, ?, ?) ON CONFLICT (study_instance_uid) DO NOTHING RETURNING id, study_instance_uid"
        );
        let pg = Dialect::Postgres.insert(
            Some("nils"),
            t,
            &cols,
            2,
            Conflict::Update {
                target: &["study_instance_uid"],
                set: &["study_date"],
            },
            &[],
        );
        assert_eq!(
            pg,
            "INSERT INTO nils.study (study_instance_uid, subject_id, study_date) VALUES ($1, $2, $3::text::date), ($4, $5, $6::text::date) ON CONFLICT (study_instance_uid) DO UPDATE SET study_date = excluded.study_date"
        );
        assert_eq!(
            Dialect::Postgres.create_temp(t, &cols),
            "CREATE TEMP TABLE IF NOT EXISTS bulk_study (study_instance_uid TEXT, subject_id BIGINT, study_date DATE)"
        );
        assert_eq!(
            Dialect::copy_temp(t, &cols),
            "COPY bulk_study (study_instance_uid, subject_id, study_date) FROM STDIN"
        );
        assert_eq!(
            Dialect::Postgres.merge_temp(Some("nils"), t, &cols, Conflict::Fail, &["id"]),
            "INSERT INTO nils.study (study_instance_uid, subject_id, study_date) SELECT study_instance_uid, subject_id, study_date FROM bulk_study RETURNING id"
        );
        assert_eq!(
            Dialect::Postgres.select_by_keys(
                Some("nils"),
                t,
                &cols[2..],
                "study_instance_uid",
                Type::Text,
                3
            ),
            "SELECT study_date::text FROM nils.study WHERE study_instance_uid = ANY($1::text[])"
        );
        assert_eq!(
            Dialect::Sqlite.select_by_keys(
                None,
                t,
                &cols[2..],
                "study_instance_uid",
                Type::Text,
                3
            ),
            "SELECT study_date FROM study WHERE study_instance_uid IN (?, ?, ?)"
        );
        let identity = table("identity");
        let cols: Vec<&Column> = ["id", "subject_id"]
            .iter()
            .map(|c| identity.column(c).unwrap())
            .collect();
        assert_eq!(
            Dialect::Postgres.select_by_keys(
                Some("nils_linkage"),
                identity,
                &cols,
                "lookup",
                Type::Bytes,
                2
            ),
            "SELECT id, subject_id FROM nils_linkage.identity WHERE lookup = ANY($1::bytea[])"
        );
    }

    #[test]
    fn copy_text_escapes_what_the_format_reserves() {
        let mut out = Vec::new();
        copy_text(&mut out, None);
        out.push(b'\t');
        copy_text(&mut out, Some(b"a\\b\tc\nd\re"));
        assert_eq!(out, b"\\N\ta\\\\b\\tc\\nd\\re");
    }
}
