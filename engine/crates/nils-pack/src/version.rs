// SPDX-License-Identifier: AGPL-3.0-only

//! A pack's version (`docs/specs/wave2-fingerprint-and-classify.md`, §5.2).
//!
//! Semantic in the usual sense (amended 2026-10-10): adding to the vocabulary
//! is a minor bump and removing or renaming any of it a major one, so a
//! federated question written for pack 2.1 is answered alike by every later
//! 2.x and never by pack 3's vocabulary (D26). The engine does not enforce that
//! promise, it records what version answered, which is what makes a breach
//! visible.

use crate::error::{Error, R};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(s: &str, at: &str) -> R<Version> {
        let mut it = s.split('.');
        let mut next = |what: &str| -> R<u32> {
            it.next().and_then(|p| p.parse().ok()).ok_or_else(|| {
                Error::at(
                    at,
                    format!("{s} is not a version: the {what} is missing or not a number"),
                )
            })
        };
        let major = next("major")?;
        let minor = next("minor")?;
        let patch = next("patch")?;
        if it.next().is_some() {
            return Err(Error::at(
                at,
                format!("{s} is not a version: it has more than three parts"),
            ));
        }
        Ok(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_numbers_and_no_more() {
        assert_eq!(
            Version::parse("2.1.0", "v").unwrap(),
            Version {
                major: 2,
                minor: 1,
                patch: 0
            }
        );
        for bad in ["2.1", "2", "2.1.0.1", "2.1.x", "", "v2.1.0"] {
            assert!(Version::parse(bad, "v").is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn versions_order_by_major_then_minor_then_patch() {
        let v = |s: &str| Version::parse(s, "v").unwrap();
        assert!(v("1.0.0") < v("1.0.1"));
        assert!(v("1.9.9") < v("2.0.0"));
        assert!(v("2.0.0") > v("1.99.99"));
    }
}
