// SPDX-License-Identifier: AGPL-3.0-only
//! The language spike, criterion 3: one binary that embeds SQLite and DuckDB. Built by
//! CI on the six release targets; `--check` proves both engines answer from inside
//! the binary. Same semantics as `go/cmd/dbcheck`.

fn main() {
    let sqlite = rusqlite::Connection::open_in_memory().expect("sqlite");
    sqlite
        .execute_batch("create table t(id integer primary key, name text); insert into t(name) values ('a'), ('b'), ('c');")
        .expect("sqlite ddl");
    let n: i64 = sqlite
        .query_row("select count(*) from t", [], |r| r.get(0))
        .expect("sqlite query");
    let sqlite_version: String = sqlite
        .query_row("select sqlite_version()", [], |r| r.get(0))
        .expect("sqlite version");

    let duck = duckdb::Connection::open_in_memory().expect("duckdb");
    duck.execute_batch(
        "create table t as select range as id, 'x' || range as name from range(1000);",
    )
    .expect("duckdb ddl");
    let m: i64 = duck
        .query_row("select count(*) from t where id % 7 = 0", [], |r| r.get(0))
        .expect("duckdb query");
    let duck_version: String = duck
        .query_row("select version()", [], |r| r.get(0))
        .expect("duckdb version");

    println!(
        "sqlite {sqlite_version}: {n} rows; duckdb {duck_version}: {m} rows; target {}",
        std::env::consts::ARCH
    );
    assert_eq!(n, 3);
    assert_eq!(m, 143);
}
