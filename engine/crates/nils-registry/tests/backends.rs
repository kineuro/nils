// SPDX-License-Identifier: AGPL-3.0-only

//! Both backends answer. Slice 1 proves the plumbing before the schema exists:
//! the bundled SQLite opens in memory, and a Postgres 16 server answers on the
//! DSN that CI provides in `NILS_TEST_POSTGRES_DSN` (`.github/workflows/ci.yml`).
//! Without that variable the Postgres test says so and passes, so `cargo test`
//! on a laptop without a server stays green.

use std::env;

/// The DSN of the Postgres server the tests may use, if one was given.
fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres test is skipped");
            None
        }
    }
}

#[test]
fn sqlite_answers() {
    let conn = rusqlite::Connection::open_in_memory().expect("open SQLite in memory");
    let one: i64 = conn
        .query_row("SELECT 1", [], |row| row.get(0))
        .expect("SELECT 1 on SQLite");
    assert_eq!(one, 1);
}

#[test]
fn postgres_answers_and_is_16_or_later() {
    let Some(dsn) = postgres_dsn() else { return };
    let mut client = postgres::Client::connect(&dsn, postgres::NoTls).expect("connect to Postgres");
    let one: i32 = client
        .query_one("SELECT 1", &[])
        .expect("SELECT 1 on Postgres")
        .get(0);
    assert_eq!(one, 1);

    // The spec asks for Postgres 16; the number is major * 10000 + minor.
    let version: String = client
        .query_one("SHOW server_version_num", &[])
        .expect("SHOW server_version_num")
        .get(0);
    let version: u32 = version.parse().expect("server_version_num is a number");
    assert!(
        version >= 160_000,
        "Postgres 16 or later is required, found {version}"
    );
}
