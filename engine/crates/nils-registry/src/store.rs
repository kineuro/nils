// SPDX-License-Identifier: AGPL-3.0-only

//! One connection to one store, behind one type: a `rusqlite::Connection` or a
//! `postgres::Client`, with the parameter and row types that both understand
//! and the two insert paths of §9.2. The dialect renders the SQL; the store
//! runs it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Write as _};

use postgres::types::{IsNull, ToSql, Type as PgType, to_sql_checked};
use rusqlite::types::{ToSqlOutput, ValueRef};

use crate::Backend;
use crate::dialect::{Conflict, Dialect, copy_text};
use crate::schema::{Column, Table, Type};

/// A value bound to a placeholder.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Text(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

impl From<Option<&nils_dicom::Value>> for Param {
    fn from(v: Option<&nils_dicom::Value>) -> Param {
        use nils_dicom::Value;
        match v {
            None => Param::Null,
            Some(Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::Json(s)) => {
                Param::Text(s.clone())
            }
            Some(Value::Int(i)) => Param::Int(*i),
            Some(Value::Double(d)) => Param::Double(*d),
        }
    }
}

impl From<&str> for Param {
    fn from(s: &str) -> Param {
        Param::Text(s.to_string())
    }
}

impl From<String> for Param {
    fn from(s: String) -> Param {
        Param::Text(s)
    }
}

impl From<i64> for Param {
    fn from(i: i64) -> Param {
        Param::Int(i)
    }
}

impl From<Option<i64>> for Param {
    fn from(i: Option<i64>) -> Param {
        i.map(Param::Int).unwrap_or(Param::Null)
    }
}

impl From<Option<String>> for Param {
    fn from(s: Option<String>) -> Param {
        s.map(Param::Text).unwrap_or(Param::Null)
    }
}

impl From<Option<&str>> for Param {
    fn from(s: Option<&str>) -> Param {
        s.map(|s| Param::Text(s.to_string())).unwrap_or(Param::Null)
    }
}

impl From<Vec<u8>> for Param {
    fn from(b: Vec<u8>) -> Param {
        Param::Bytes(b)
    }
}

impl rusqlite::ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Param::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            Param::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            Param::Int(i) => ToSqlOutput::Borrowed(ValueRef::Integer(*i)),
            Param::Double(d) => ToSqlOutput::Borrowed(ValueRef::Real(*d)),
            Param::Bool(b) => ToSqlOutput::Borrowed(ValueRef::Integer(i64::from(*b))),
            Param::Bytes(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

/// SQL NULL for any Postgres column type.
#[derive(Debug)]
struct Null;

impl ToSql for Null {
    fn to_sql(
        &self,
        _ty: &PgType,
        _out: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        Ok(IsNull::Yes)
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }

    to_sql_checked!();
}

static NULL: Null = Null;

impl Param {
    fn pg(&self) -> &(dyn ToSql + Sync) {
        match self {
            Param::Null => &NULL,
            Param::Text(s) => s,
            Param::Int(i) => i,
            Param::Double(d) => d,
            Param::Bool(b) => b,
            Param::Bytes(b) => b,
        }
    }

    /// The value in `COPY` text format.
    fn copy(&self, out: &mut Vec<u8>) {
        match self {
            Param::Null => copy_text(out, None),
            Param::Text(s) => copy_text(out, Some(s.as_bytes())),
            Param::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            Param::Double(d) => out.extend_from_slice(format!("{d:?}").as_bytes()),
            Param::Bool(b) => out.push(if *b { b't' } else { b'f' }),
            Param::Bytes(b) => {
                out.extend_from_slice(b"\\\\x");
                out.extend_from_slice(hex::encode(b).as_bytes());
            }
        }
    }
}

/// One value read back.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Text(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// One row read back.
#[derive(Debug, Clone, PartialEq)]
pub struct Row(pub Vec<Cell>);

impl Row {
    pub fn get(&self, i: usize) -> &Cell {
        &self.0[i]
    }

    pub fn int(&self, i: usize) -> Result<i64, Error> {
        match &self.0[i] {
            Cell::Int(v) => Ok(*v),
            Cell::Bool(b) => Ok(i64::from(*b)),
            other => Err(Error::Message(format!(
                "column {i}: expected an integer, read {other:?}"
            ))),
        }
    }

    pub fn opt_int(&self, i: usize) -> Result<Option<i64>, Error> {
        match &self.0[i] {
            Cell::Null => Ok(None),
            _ => self.int(i).map(Some),
        }
    }

    pub fn double(&self, i: usize) -> Result<f64, Error> {
        match &self.0[i] {
            Cell::Double(v) => Ok(*v),
            Cell::Int(v) => Ok(*v as f64),
            other => Err(Error::Message(format!(
                "column {i}: expected a double, read {other:?}"
            ))),
        }
    }

    pub fn text(&self, i: usize) -> Result<&str, Error> {
        match &self.0[i] {
            Cell::Text(s) => Ok(s),
            other => Err(Error::Message(format!(
                "column {i}: expected text, read {other:?}"
            ))),
        }
    }

    pub fn opt_text(&self, i: usize) -> Result<Option<&str>, Error> {
        match &self.0[i] {
            Cell::Null => Ok(None),
            _ => self.text(i).map(Some),
        }
    }

    pub fn bytes(&self, i: usize) -> Result<&[u8], Error> {
        match &self.0[i] {
            Cell::Bytes(b) => Ok(b),
            other => Err(Error::Message(format!(
                "column {i}: expected bytes, read {other:?}"
            ))),
        }
    }

    pub fn opt_bytes(&self, i: usize) -> Result<Option<&[u8]>, Error> {
        match &self.0[i] {
            Cell::Null => Ok(None),
            _ => self.bytes(i).map(Some),
        }
    }
}

/// What a store can fail with.
#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Postgres(postgres::Error),
    Io(io::Error),
    /// A configuration or state problem, in words.
    Message(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "sqlite: {e}"),
            Error::Postgres(e) => match e.as_db_error() {
                Some(db) => write!(f, "postgres: {}", db.message()),
                None => write!(f, "postgres: {e}"),
            },
            Error::Io(e) => write!(f, "{e}"),
            Error::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        Error::Sqlite(e)
    }
}

impl From<postgres::Error> for Error {
    fn from(e: postgres::Error) -> Error {
        Error::Postgres(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// How the Postgres store inserts a batch (§9.2): `COPY` into a temporary
/// table and one merge, or multi-row `INSERT` statements. Slice 3 measures
/// both; `COPY` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkPath {
    Copy,
    Insert,
}

impl BulkPath {
    pub fn name(self) -> &'static str {
        match self {
            BulkPath::Copy => "copy",
            BulkPath::Insert => "insert",
        }
    }

    /// `NILS_PG_BULK=insert` selects the insert path; anything else is `copy`.
    pub fn from_env() -> BulkPath {
        match std::env::var("NILS_PG_BULK").as_deref() {
            Ok("insert") => BulkPath::Insert,
            _ => BulkPath::Copy,
        }
    }
}

/// Postgres allows 65,535 parameters per statement.
const PG_MAX_PARAMS: usize = 65_535;
/// And the writer keeps a statement's text within reason.
const PG_MAX_ROWS_PER_INSERT: usize = 1_000;
/// SQLite's default limit on placeholders is 32,766; keys are looked up in
/// chunks well under it.
pub const SQLITE_KEY_CHUNK: usize = 500;

/// An insert: which table, which columns, what a conflict does, what comes
/// back.
pub struct Insert<'a> {
    pub table: &'a Table,
    pub columns: Vec<&'a Column>,
    pub conflict: Conflict<'a>,
    pub returning: &'a [&'a str],
}

impl<'a> Insert<'a> {
    /// The listed columns of a table.
    pub fn new(table: &'a Table, columns: &[&str]) -> Insert<'a> {
        let columns = columns
            .iter()
            .map(|name| {
                table
                    .column(name)
                    .unwrap_or_else(|| panic!("{}.{name} is not a column", table.name))
            })
            .collect();
        Insert {
            table,
            columns,
            conflict: Conflict::Fail,
            returning: &[],
        }
    }

    /// Every column but the generated id.
    pub fn all(table: &'a Table) -> Insert<'a> {
        Insert {
            table,
            columns: table.data_columns().collect(),
            conflict: Conflict::Fail,
            returning: &[],
        }
    }

    pub fn on_conflict(mut self, conflict: Conflict<'a>) -> Insert<'a> {
        self.conflict = conflict;
        self
    }

    pub fn returning(mut self, columns: &'a [&'a str]) -> Insert<'a> {
        self.returning = columns;
        self
    }
}

/// One open connection.
pub enum Store {
    Sqlite(rusqlite::Connection),
    Postgres {
        client: Box<postgres::Client>,
        schema: String,
        statements: HashMap<String, postgres::Statement>,
        temps: HashSet<&'static str>,
        bulk: BulkPath,
    },
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Store::Sqlite(_) => f.write_str("Store::Sqlite"),
            Store::Postgres { schema, bulk, .. } => {
                write!(f, "Store::Postgres({schema}, {})", bulk.name())
            }
        }
    }
}

impl Store {
    /// Open a SQLite file: WAL, `synchronous=NORMAL`, a five-second wait on a
    /// lock, foreign keys left off (§9.2).
    pub fn open_sqlite(path: &std::path::Path) -> Result<Store, Error> {
        let conn = rusqlite::Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA temp_store = MEMORY;\n\
             PRAGMA cache_size = -65536;",
        )?;
        Ok(Store::Sqlite(conn))
    }

    /// An in-memory SQLite store, for tests.
    pub fn sqlite_in_memory() -> Result<Store, Error> {
        Ok(Store::Sqlite(rusqlite::Connection::open_in_memory()?))
    }

    /// Connect to Postgres and put `schema` first on the search path.
    pub fn connect_postgres(dsn: &str, schema: &str) -> Result<Store, Error> {
        let mut client = postgres::Client::connect(dsn, postgres::NoTls)?;
        client.batch_execute(&format!("SET search_path TO {schema}, public"))?;
        Ok(Store::Postgres {
            client: Box::new(client),
            schema: schema.to_string(),
            statements: HashMap::new(),
            temps: HashSet::new(),
            bulk: BulkPath::from_env(),
        })
    }

    pub fn backend(&self) -> Backend {
        match self {
            Store::Sqlite(_) => Backend::Sqlite,
            Store::Postgres { .. } => Backend::Postgres,
        }
    }

    pub fn dialect(&self) -> Dialect {
        Dialect::of(self.backend())
    }

    /// The schema the tables live in: none on SQLite.
    pub fn schema(&self) -> Option<&str> {
        match self {
            Store::Sqlite(_) => None,
            Store::Postgres { schema, .. } => Some(schema),
        }
    }

    /// The bulk path in use, on Postgres.
    pub fn bulk_path(&self) -> Option<BulkPath> {
        match self {
            Store::Sqlite(_) => None,
            Store::Postgres { bulk, .. } => Some(*bulk),
        }
    }

    pub fn set_bulk_path(&mut self, path: BulkPath) {
        if let Store::Postgres { bulk, .. } = self {
            *bulk = path;
        }
    }

    /// A table's name as this store addresses it.
    pub fn qualified(&self, table: &str) -> String {
        crate::dialect::qualified(self.schema(), table)
    }

    /// Run statements with no parameters and no result (DDL, pragmas).
    pub fn batch(&mut self, sql: &str) -> Result<(), Error> {
        match self {
            Store::Sqlite(c) => c.execute_batch(sql)?,
            Store::Postgres { client, .. } => client.batch_execute(sql)?,
        }
        Ok(())
    }

    pub fn begin(&mut self) -> Result<(), Error> {
        match self {
            Store::Sqlite(_) => self.batch("BEGIN IMMEDIATE"),
            Store::Postgres { .. } => self.batch("BEGIN"),
        }
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        self.batch("COMMIT")
    }

    pub fn rollback(&mut self) -> Result<(), Error> {
        self.batch("ROLLBACK")
    }

    /// Run one statement, return the rows it affected.
    pub fn execute(&mut self, sql: &str, params: &[Param]) -> Result<u64, Error> {
        match self {
            Store::Sqlite(c) => {
                let mut stmt = c.prepare_cached(sql)?;
                Ok(stmt.execute(rusqlite::params_from_iter(params.iter()))? as u64)
            }
            Store::Postgres {
                client, statements, ..
            } => {
                let stmt = prepared(client, statements, sql)?;
                let args: Vec<&(dyn ToSql + Sync)> = params.iter().map(Param::pg).collect();
                Ok(client.execute(&stmt, &args)?)
            }
        }
    }

    /// Run one query, return every row.
    pub fn query(&mut self, sql: &str, params: &[Param]) -> Result<Vec<Row>, Error> {
        match self {
            Store::Sqlite(c) => {
                let mut stmt = c.prepare_cached(sql)?;
                let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(sqlite_row(row)?);
                }
                Ok(out)
            }
            Store::Postgres {
                client, statements, ..
            } => {
                let stmt = prepared(client, statements, sql)?;
                let args: Vec<&(dyn ToSql + Sync)> = params.iter().map(Param::pg).collect();
                client.query(&stmt, &args)?.iter().map(pg_row).collect()
            }
        }
    }

    /// The first row, if any.
    pub fn query_opt(&mut self, sql: &str, params: &[Param]) -> Result<Option<Row>, Error> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Rows of `table` whose text `key` column is one of `keys`, the listed
    /// columns read back as text where the backend would otherwise reformat
    /// them.
    pub fn select_by_keys(
        &mut self,
        table: &Table,
        columns: &[&Column],
        key: &str,
        keys: &[String],
    ) -> Result<Vec<Row>, Error> {
        let mut out = Vec::new();
        match self {
            Store::Sqlite(_) => {
                for chunk in keys.chunks(SQLITE_KEY_CHUNK) {
                    let sql = self.dialect().select_by_keys(
                        None,
                        table,
                        columns,
                        key,
                        Type::Text,
                        chunk.len(),
                    );
                    let params: Vec<Param> =
                        chunk.iter().map(|k| Param::from(k.as_str())).collect();
                    out.extend(self.query(&sql, &params)?);
                }
            }
            Store::Postgres {
                client,
                statements,
                schema,
                ..
            } => {
                let sql = Dialect::Postgres.select_by_keys(
                    Some(schema),
                    table,
                    columns,
                    key,
                    Type::Text,
                    1,
                );
                let stmt = prepared(client, statements, &sql)?;
                let rows = client.query(&stmt, &[&keys])?;
                for row in &rows {
                    out.push(pg_row(row)?);
                }
            }
        }
        Ok(out)
    }

    /// Rows of `table` whose integer `key` column is one of `ids`, read back
    /// like [`Store::select_by_keys`].
    pub fn select_by_ids(
        &mut self,
        table: &Table,
        columns: &[&Column],
        key: &str,
        ids: &[i64],
    ) -> Result<Vec<Row>, Error> {
        let mut out = Vec::new();
        match self {
            Store::Sqlite(_) => {
                for chunk in ids.chunks(SQLITE_KEY_CHUNK) {
                    let sql = self.dialect().select_by_keys(
                        None,
                        table,
                        columns,
                        key,
                        Type::Int,
                        chunk.len(),
                    );
                    let params: Vec<Param> = chunk.iter().map(|&k| Param::Int(k)).collect();
                    out.extend(self.query(&sql, &params)?);
                }
            }
            Store::Postgres {
                client,
                statements,
                schema,
                ..
            } => {
                let sql = Dialect::Postgres.select_by_keys(
                    Some(schema),
                    table,
                    columns,
                    key,
                    Type::Int,
                    1,
                );
                let stmt = prepared(client, statements, &sql)?;
                let rows = client.query(&stmt, &[&ids])?;
                for row in &rows {
                    out.push(pg_row(row)?);
                }
            }
        }
        Ok(out)
    }

    /// Rows of `table` whose bytes `key` column is one of `keys`, read back
    /// like [`Store::select_by_keys`]. The linkage store's `identity.lookup`
    /// is the one such key.
    pub fn select_by_bytes(
        &mut self,
        table: &Table,
        columns: &[&Column],
        key: &str,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Row>, Error> {
        let mut out = Vec::new();
        match self {
            Store::Sqlite(_) => {
                for chunk in keys.chunks(SQLITE_KEY_CHUNK) {
                    let sql = self.dialect().select_by_keys(
                        None,
                        table,
                        columns,
                        key,
                        Type::Bytes,
                        chunk.len(),
                    );
                    let params: Vec<Param> =
                        chunk.iter().map(|k| Param::Bytes(k.clone())).collect();
                    out.extend(self.query(&sql, &params)?);
                }
            }
            Store::Postgres {
                client,
                statements,
                schema,
                ..
            } => {
                let sql = Dialect::Postgres.select_by_keys(
                    Some(schema),
                    table,
                    columns,
                    key,
                    Type::Bytes,
                    1,
                );
                let stmt = prepared(client, statements, &sql)?;
                let rows = client.query(&stmt, &[&keys])?;
                for row in &rows {
                    out.push(pg_row(row)?);
                }
            }
        }
        Ok(out)
    }

    /// `UPDATE table SET a = ?, b = ? WHERE key = id`, the placeholders cast
    /// by the columns' declared types. Returns the rows updated.
    pub fn update_by_id(
        &mut self,
        table: &Table,
        sets: &[(&str, Param)],
        key: &str,
        id: i64,
    ) -> Result<u64, Error> {
        let d = self.dialect();
        let mut sql = format!("UPDATE {} SET ", self.qualified(table.name));
        let mut params = Vec::with_capacity(sets.len() + 1);
        for (i, (name, value)) in sets.iter().enumerate() {
            let column = table
                .column(name)
                .unwrap_or_else(|| panic!("{}.{name} is not a column", table.name));
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&format!("{name} = {}", d.param(i + 1, column.ty)));
            params.push(value.clone());
        }
        sql.push_str(&format!(
            " WHERE {key} = {}",
            d.param(sets.len() + 1, Type::Int)
        ));
        params.push(Param::Int(id));
        self.execute(&sql, &params)
    }

    /// `UPDATE table SET <set> ... WHERE <key> = v.key` for integer pairs
    /// `(key, val)`, in chunks; `set` names the new value as `v.val`. Returns
    /// the rows updated.
    pub fn update_from_values(
        &mut self,
        table: &Table,
        set: &str,
        key: &str,
        pairs: &[(i64, i64)],
    ) -> Result<u64, Error> {
        let mut updated = 0;
        for chunk in pairs.chunks(SQLITE_KEY_CHUNK) {
            let sql = self.dialect().update_from_values(
                self.schema().map(str::to_string).as_deref(),
                table,
                set,
                key,
                chunk.len(),
            );
            let params: Vec<Param> = chunk
                .iter()
                .flat_map(|&(k, v)| [Param::Int(k), Param::Int(v)])
                .collect();
            updated += self.execute(&sql, &params)?;
        }
        Ok(updated)
    }

    /// Insert `rows` (one `Vec<Param>` per row, aligned with the insert's
    /// columns) and return what `RETURNING` produced, in no particular order.
    /// On SQLite a prepared statement runs per row; on Postgres the bulk path
    /// applies.
    pub fn insert(&mut self, spec: &Insert<'_>, rows: &[Vec<Param>]) -> Result<Vec<Row>, Error> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Store::Sqlite(c) => {
                let sql = Dialect::Sqlite.insert(
                    None,
                    spec.table,
                    &spec.columns,
                    1,
                    spec.conflict,
                    spec.returning,
                );
                let mut stmt = c.prepare_cached(&sql)?;
                let mut out = Vec::new();
                for row in rows {
                    debug_assert_eq!(row.len(), spec.columns.len());
                    if spec.returning.is_empty() {
                        stmt.execute(rusqlite::params_from_iter(row.iter()))?;
                    } else {
                        let mut got = stmt.query(rusqlite::params_from_iter(row.iter()))?;
                        while let Some(r) = got.next()? {
                            out.push(sqlite_row(r)?);
                        }
                    }
                }
                Ok(out)
            }
            Store::Postgres { bulk, .. } => match bulk {
                BulkPath::Insert => self.pg_insert(spec, rows),
                BulkPath::Copy => self.pg_copy(spec, rows),
            },
        }
    }

    fn pg_insert(&mut self, spec: &Insert<'_>, rows: &[Vec<Param>]) -> Result<Vec<Row>, Error> {
        let Store::Postgres {
            client,
            statements,
            schema,
            ..
        } = self
        else {
            unreachable!()
        };
        let per_stmt = (PG_MAX_PARAMS / spec.columns.len().max(1)).clamp(1, PG_MAX_ROWS_PER_INSERT);
        let mut out = Vec::new();
        for chunk in rows.chunks(per_stmt) {
            let sql = Dialect::Postgres.insert(
                Some(schema),
                spec.table,
                &spec.columns,
                chunk.len(),
                spec.conflict,
                spec.returning,
            );
            let stmt = prepared(client, statements, &sql)?;
            let args: Vec<&(dyn ToSql + Sync)> =
                chunk.iter().flat_map(|r| r.iter().map(Param::pg)).collect();
            for row in client.query(&stmt, &args)? {
                out.push(pg_row(&row)?);
            }
        }
        Ok(out)
    }

    fn pg_copy(&mut self, spec: &Insert<'_>, rows: &[Vec<Param>]) -> Result<Vec<Row>, Error> {
        let Store::Postgres {
            client,
            statements,
            schema,
            temps,
            ..
        } = self
        else {
            unreachable!()
        };
        if temps.insert(spec.table.name) {
            // every data column, so a later insert on the same table with
            // another column set still fits
            let all: Vec<&Column> = spec.table.data_columns().collect();
            client.batch_execute(&Dialect::Postgres.create_temp(spec.table, &all))?;
        } else {
            client.batch_execute(&format!("TRUNCATE {}", Dialect::temp_name(spec.table)))?;
        }
        let mut text = Vec::with_capacity(rows.len() * 64);
        for row in rows {
            debug_assert_eq!(row.len(), spec.columns.len());
            for (i, p) in row.iter().enumerate() {
                if i > 0 {
                    text.push(b'\t');
                }
                p.copy(&mut text);
            }
            text.push(b'\n');
        }
        let mut writer = client.copy_in(&Dialect::copy_temp(spec.table, &spec.columns))?;
        writer.write_all(&text)?;
        writer.finish()?;
        let merge = Dialect::Postgres.merge_temp(
            Some(schema),
            spec.table,
            &spec.columns,
            spec.conflict,
            spec.returning,
        );
        let stmt = prepared(client, statements, &merge)?;
        client.query(&stmt, &[])?.iter().map(pg_row).collect()
    }
}

fn prepared(
    client: &mut postgres::Client,
    statements: &mut HashMap<String, postgres::Statement>,
    sql: &str,
) -> Result<postgres::Statement, Error> {
    if let Some(s) = statements.get(sql) {
        return Ok(s.clone());
    }
    let stmt = client.prepare(sql)?;
    statements.insert(sql.to_string(), stmt.clone());
    Ok(stmt)
}

fn sqlite_row(row: &rusqlite::Row<'_>) -> Result<Row, Error> {
    let n = row.as_ref().column_count();
    let mut cells = Vec::with_capacity(n);
    for i in 0..n {
        cells.push(match row.get_ref(i)? {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(v) => Cell::Int(v),
            ValueRef::Real(v) => Cell::Double(v),
            ValueRef::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Cell::Bytes(b.to_vec()),
        });
    }
    Ok(Row(cells))
}

fn pg_row(row: &postgres::Row) -> Result<Row, Error> {
    let mut cells = Vec::with_capacity(row.len());
    for (i, col) in row.columns().iter().enumerate() {
        let ty = col.type_();
        let cell = match *ty {
            PgType::INT8 => row
                .try_get::<_, Option<i64>>(i)?
                .map_or(Cell::Null, Cell::Int),
            PgType::INT4 => row
                .try_get::<_, Option<i32>>(i)?
                .map_or(Cell::Null, |v| Cell::Int(v.into())),
            PgType::INT2 => row
                .try_get::<_, Option<i16>>(i)?
                .map_or(Cell::Null, |v| Cell::Int(v.into())),
            PgType::FLOAT8 => row
                .try_get::<_, Option<f64>>(i)?
                .map_or(Cell::Null, Cell::Double),
            PgType::FLOAT4 => row
                .try_get::<_, Option<f32>>(i)?
                .map_or(Cell::Null, |v| Cell::Double(v.into())),
            PgType::BOOL => row
                .try_get::<_, Option<bool>>(i)?
                .map_or(Cell::Null, Cell::Bool),
            PgType::BYTEA => row
                .try_get::<_, Option<Vec<u8>>>(i)?
                .map_or(Cell::Null, Cell::Bytes),
            PgType::TEXT | PgType::VARCHAR | PgType::BPCHAR | PgType::NAME | PgType::UNKNOWN => row
                .try_get::<_, Option<String>>(i)?
                .map_or(Cell::Null, Cell::Text),
            _ => {
                return Err(Error::Message(format!(
                    "column {} has type {ty}, which the store reads only as text",
                    col.name()
                )));
            }
        };
        cells.push(cell);
    }
    Ok(Row(cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_render_for_copy() {
        let mut out = Vec::new();
        for p in [
            Param::Null,
            Param::Text("a\tb".into()),
            Param::Int(-7),
            Param::Double(1.5),
            Param::Double(2.0),
            Param::Bool(true),
            Param::Bytes(vec![0x0a, 0xff]),
        ] {
            p.copy(&mut out);
            out.push(b'|');
        }
        assert_eq!(out, b"\\N|a\\tb|-7|1.5|2.0|t|\\\\x0aff|");
    }

    #[test]
    fn sqlite_round_trips_every_cell() {
        let mut s = Store::sqlite_in_memory().unwrap();
        s.batch("CREATE TABLE t (a INTEGER, b TEXT, c REAL, d BLOB, e INTEGER)")
            .unwrap();
        let n = s
            .execute(
                "INSERT INTO t VALUES (?, ?, ?, ?, ?)",
                &[
                    Param::Int(3),
                    Param::Text("x".into()),
                    Param::Double(0.25),
                    Param::Bytes(vec![1, 2]),
                    Param::Bool(true),
                ],
            )
            .unwrap();
        assert_eq!(n, 1);
        let rows = s.query("SELECT a, b, c, d, e, NULL FROM t", &[]).unwrap();
        assert_eq!(
            rows[0],
            Row(vec![
                Cell::Int(3),
                Cell::Text("x".into()),
                Cell::Double(0.25),
                Cell::Bytes(vec![1, 2]),
                Cell::Int(1),
                Cell::Null
            ])
        );
        assert_eq!(rows[0].int(4).unwrap(), 1);
        assert_eq!(rows[0].opt_text(5).unwrap(), None);
        assert!(rows[0].text(0).is_err());
    }
}
