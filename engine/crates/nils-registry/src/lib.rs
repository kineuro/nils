// SPDX-License-Identifier: AGPL-3.0-only

//! The registry of NILS.
//!
//! What lives here, from the Wave 1 specification
//! (`docs/specs/wave1-parse-and-digest.md`): the schema declared once and rendered
//! for each backend (§4), the dialect layer and the connection types for SQLite
//! and Postgres, migrations, the linkage store and the key store (§7).
//!
//! Slice 1 of the build is the skeleton: the two backends are named, and the test
//! suite proves that both answer (`tests/backends.rs`). The schema lands with
//! slice 3 (§14).

use std::fmt;
use std::str::FromStr;

/// The two database backends a registry can live in (§4.1): SQLite for a laptop
/// or a single host, Postgres for a shared server. `nils init --backend` takes
/// one of these by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// The bundled SQLite, one file on disk.
    Sqlite,
    /// A Postgres 16 or later server, reached through a DSN.
    Postgres,
}

impl Backend {
    /// The name used on the command line and in the registry's metadata.
    pub const fn name(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A backend name that is neither `sqlite` nor `postgres`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownBackend(pub String);

impl fmt::Display for UnknownBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown backend {:?}; expected sqlite or postgres",
            self.0
        )
    }
}

impl std::error::Error for UnknownBackend {}

impl FromStr for Backend {
    type Err = UnknownBackend;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sqlite" => Ok(Backend::Sqlite),
            "postgres" => Ok(Backend::Postgres),
            other => Err(UnknownBackend(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_round_trip() {
        for backend in [Backend::Sqlite, Backend::Postgres] {
            assert_eq!(backend.name().parse::<Backend>(), Ok(backend));
            assert_eq!(backend.to_string(), backend.name());
        }
    }

    #[test]
    fn unknown_backend_is_named_in_the_error() {
        let err = "duckdb".parse::<Backend>().unwrap_err();
        assert_eq!(err, UnknownBackend("duckdb".to_owned()));
        assert_eq!(
            err.to_string(),
            "unknown backend \"duckdb\"; expected sqlite or postgres"
        );
    }
}
