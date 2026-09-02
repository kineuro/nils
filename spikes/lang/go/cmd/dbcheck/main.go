// SPDX-License-Identifier: AGPL-3.0-only

// The language spike, criterion 3: one binary that embeds SQLite and DuckDB. Built by
// CI on the six release targets; running it proves both engines answer from inside
// the binary. Same semantics as rust/dbcheck.
package main

import (
	"database/sql"
	"fmt"
	"runtime"

	_ "github.com/duckdb/duckdb-go/v2"
	_ "modernc.org/sqlite"
)

func main() {
	sqlite, err := sql.Open("sqlite", ":memory:")
	if err != nil {
		panic(err)
	}
	defer sqlite.Close()
	if _, err := sqlite.Exec("create table t(id integer primary key, name text); insert into t(name) values ('a'), ('b'), ('c');"); err != nil {
		panic(err)
	}
	var n int64
	var sqliteVersion string
	if err := sqlite.QueryRow("select count(*), sqlite_version() from t").Scan(&n, &sqliteVersion); err != nil {
		panic(err)
	}

	duck, err := sql.Open("duckdb", "")
	if err != nil {
		panic(err)
	}
	defer duck.Close()
	if _, err := duck.Exec("create table t as select range as id, 'x' || range as name from range(1000);"); err != nil {
		panic(err)
	}
	var m int64
	var duckVersion string
	if err := duck.QueryRow("select count(*), version() from t where id % 7 = 0").Scan(&m, &duckVersion); err != nil {
		panic(err)
	}

	fmt.Printf("sqlite %s: %d rows; duckdb %s: %d rows; target %s/%s\n", sqliteVersion, n, duckVersion, m, runtime.GOOS, runtime.GOARCH)
	if n != 3 || m != 143 {
		panic("wrong counts")
	}
}
