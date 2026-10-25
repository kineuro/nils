// SPDX-License-Identifier: AGPL-3.0-only

//! The registry of NILS.
//!
//! What lives here, from the Wave 1 specification
//! (`docs/specs/wave1-parse-and-digest.md`): the schema declared once and rendered
//! for each backend (§4, [`schema`] and [`dialect`]), the connection and bulk
//! paths of both backends ([`store`], §9.2), the migrations ([`migrate`]), the
//! registry home with its `nils.toml` ([`home`]), the pseudonym schemes and the
//! key store ([`pseudonym`], [`keys`], §7), and the clock ([`time`]).
//!
//! Slice 1 named the two backends and proved both answer (`tests/backends.rs`);
//! slice 3 landed the rest (§14).

use std::fmt;
use std::str::FromStr;

pub mod ab;
pub mod actor;
pub mod anchored;
pub mod asked;
pub mod audit;
pub mod audit_draw;
pub mod blind;
pub mod campaign;
pub mod check;
pub mod clinical;
pub mod cohort;
pub mod content;
pub mod dataset_subjects;
pub mod day;
pub mod derivative;
pub mod dialect;
pub mod embedding;
pub mod empty;
pub mod fold;
pub mod home;
pub mod idempotency;
pub mod identity_map;
pub mod import;
pub mod job;
pub mod keys;
pub mod labels;
pub mod linkage;
pub mod location;
pub mod lock;
pub mod measure;
pub mod merge;
pub mod migrate;
pub mod model;
pub mod names;
pub mod opinion;
pub mod overlay;
pub mod pair;
pub mod personnummer;
pub mod pick_rules;
pub mod pick_truth;
pub mod pipeline;
pub mod place;
pub mod principal;
pub mod proposals;
pub mod pseudonym;
pub mod review;
pub mod round;
pub mod rule_proposal;
pub mod schema;
pub mod session;
pub mod store;
pub mod suggestion;
pub mod time;
pub mod verify;
pub mod workflow;

/// A word with the article a sentence gives it, by its first letter: "an
/// agent", "an adjudicator", "an export place", "a mask", "a unit". The
/// engine's own words (author kinds, roles, kinds of places, derivatives,
/// jobs and rows) are all it is asked of.
pub fn a_or_an(word: impl AsRef<str>) -> String {
    let word = word.as_ref();
    let w = word.trim_start().to_ascii_lowercase();
    let vowel = match w.chars().next() {
        Some('a' | 'e' | 'i' | 'o') => true,
        // "an unchecked", "an update"; "a unit", "a user", "a usual"
        Some('u') => !["uni", "use", "usu", "uti", "ura"]
            .iter()
            .any(|p| w.starts_with(p)),
        _ => false,
    };
    format!("{} {word}", if vowel { "an" } else { "a" })
}

pub use home::{Home, HomeError, InitOptions, Meta, Registry};
pub use keys::{KeyError, KeyStore};
pub use linkage::Subkeys;
pub use migrate::SCHEMA_VERSION;
pub use pseudonym::Scheme;
pub use store::{BulkPath, Cancel, Error, Insert, Param, Row, Store};

/// The two database backends a registry can live in (§4.1): SQLite for a laptop
/// or a single host, Postgres for a shared server. `nils init --backend` takes
/// one of these by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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

    #[test]
    fn a_word_takes_the_article_a_sentence_gives_it() {
        for (word, said) in [
            ("agent", "an agent"),
            ("adjudicator", "an adjudicator"),
            ("export", "an export"),
            ("embedding", "an embedding"),
            ("instance", "an instance"),
            ("output", "an output"),
            ("unchecked", "an unchecked"),
            ("person", "a person"),
            ("rater", "a rater"),
            ("mask", "a mask"),
            ("unit", "a unit"),
            ("user", "a user"),
            ("", "a "),
        ] {
            assert_eq!(super::a_or_an(word), said);
        }
    }
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
