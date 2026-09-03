// SPDX-License-Identifier: AGPL-3.0-only

//! Overlays (`docs/specs/wave2-fingerprint-and-classify.md`, §5.3, C2).
//!
//! A site's own words for the same thing, scoped to an origin and never to a
//! selection, applied when the pack is loaded and never edited into it. The
//! merge rule is v0's, exactly, including the two things about it that are
//! easy to get wrong: duplicates are dropped case-insensitively but the
//! **original spelling is kept**, and the order is the order they were
//! written, because v0's contrast vocabulary contains `" -k"` and `" -gd"`
//! whose leading space is load-bearing.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Error, R};
use crate::yaml::{self, File};

/// What an overlay does to one bucket.
#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// An origin-scoped amendment to a pack.
pub struct Overlay {
    /// `name@version` of the overlay itself, recorded on every row it judged.
    pub id: String,
    /// The pack it amends.
    pub pack: String,
    /// What it is scoped to: manufacturer, model, station, or an ingest
    /// batch. Provenance, never a selection.
    pub scope: BTreeMap<String, String>,
    pub buckets: BTreeMap<String, Edit>,
    /// What the site says its amendment does. An overlay carries its own
    /// cases for the same reason a pack does: it changes verdicts, and the
    /// pack author's cases are the author's claim about the pack, not a
    /// constraint on the site.
    pub cases: Vec<(std::path::PathBuf, crate::corpus::Case)>,
}

impl Overlay {
    pub fn load(path: &Path) -> R<Overlay> {
        let f = File::read(path)?;
        let m = f.blame(yaml::obj(&f.value, "overlay"))?;
        let name = f.blame(yaml::text(yaml::get(m, "overlay", "overlay")?, "overlay"))?;
        let version = f.blame(yaml::text(yaml::get(m, "version", "overlay")?, "version"))?;
        let pack = f.blame(yaml::text(yaml::get(m, "pack", "overlay")?, "pack"))?;

        let mut scope = BTreeMap::new();
        if let Some(s) = m.get("scope") {
            for (k, v) in f.blame(yaml::obj(s, "scope"))? {
                if !SCOPES.contains(&k.as_str()) {
                    return Err(Error::at(
                        format!("scope.{k}"),
                        format!(
                            "an overlay is scoped by origin ({}), never by a selection",
                            SCOPES.join(", ")
                        ),
                    )
                    .in_file(&f.path, Some(&f.source)));
                }
                scope.insert(k.clone(), f.blame(yaml::text(v, &format!("scope.{k}")))?);
            }
        }
        if scope.is_empty() {
            return Err(Error::at(
                "scope",
                "an overlay with no scope amends everything, which is what a pack version is for",
            )
            .in_file(&f.path, Some(&f.source)));
        }

        let mut buckets = BTreeMap::new();
        for (name, edit) in f.blame(yaml::obj(yaml::get(m, "buckets", "overlay")?, "buckets"))? {
            let at = format!("buckets.{name}");
            let e = f.blame(yaml::obj(edit, &at))?;
            let mut out = Edit::default();
            if let Some(a) = e.get("add") {
                out.add = f.blame(yaml::texts(a, &format!("{at}.add")))?;
            }
            if let Some(r) = e.get("remove") {
                out.remove = f.blame(yaml::texts(r, &format!("{at}.remove")))?;
            }
            if out.add.is_empty() && out.remove.is_empty() {
                return Err(Error::at(&at, "adds nothing and removes nothing")
                    .in_file(&f.path, Some(&f.source)));
            }
            buckets.insert(name.clone(), out);
        }

        let cases = match m.get("cases") {
            Some(v) => crate::corpus::cases_of(&f, v)?,
            None => Vec::new(),
        };
        if cases.is_empty() {
            return Err(Error::at(
                "cases",
                "an overlay changes verdicts, so it ships the cases that show what it changed, as a pack does",
            )
            .in_file(&f.path, Some(&f.source)));
        }

        Ok(Overlay {
            id: format!("{name}@{version}"),
            pack,
            scope,
            buckets,
            cases,
        })
    }

    /// Refuse an overlay for another pack, or one naming a bucket the pack
    /// does not open. A pack decides what is editable, not the engine.
    pub fn check_against(&self, pack: &str, buckets: &BTreeMap<String, Vec<String>>) -> R<()> {
        if self.pack != pack {
            return Err(Error::at(
                "pack",
                format!("the overlay amends {}, and this pack is {pack}", self.pack),
            ));
        }
        for name in self.buckets.keys() {
            if !buckets.contains_key(name) {
                let open: Vec<&str> = buckets.keys().map(String::as_str).collect();
                return Err(Error::at(
                    format!("buckets.{name}"),
                    format!(
                        "the pack does not open {name} for editing; it opens {}",
                        if open.is_empty() {
                            "nothing".to_string()
                        } else {
                            open.join(", ")
                        }
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// What an overlay may be keyed on: an origin, never a selection (C2).
pub const SCOPES: &[&str] = &["manufacturer", "model", "station", "batch"];

/// v0's rule: the defaults then the additions, de-duplicated
/// case-insensitively with the first spelling kept, then the removals taken
/// out (also case-insensitively).
pub fn merge(defaults: &[String], edit: &Edit) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for v in defaults.iter().chain(edit.add.iter()) {
        let folded = v.to_lowercase();
        if seen.contains(&folded) {
            continue;
        }
        seen.push(folded);
        out.push(v.clone());
    }
    let drop: Vec<String> = edit.remove.iter().map(|r| r.to_lowercase()).collect();
    out.retain(|v| !drop.contains(&v.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn additions_come_after_and_order_is_kept() {
        let out = merge(
            &s(&["gd", "dotarem"]),
            &Edit {
                add: s(&["clariscan"]),
                remove: Vec::new(),
            },
        );
        assert_eq!(out, s(&["gd", "dotarem", "clariscan"]));
    }

    #[test]
    fn a_duplicate_is_dropped_by_fold_and_the_first_spelling_is_kept() {
        let out = merge(
            &s(&["Gd"]),
            &Edit {
                add: s(&["gd", "GD", "Dotarem"]),
                remove: Vec::new(),
            },
        );
        assert_eq!(out, s(&["Gd", "Dotarem"]));
    }

    #[test]
    fn a_leading_space_is_load_bearing_and_survives() {
        let out = merge(
            &s(&[" -k", " -gd"]),
            &Edit {
                add: s(&[" -c"]),
                remove: Vec::new(),
            },
        );
        assert_eq!(out, s(&[" -k", " -gd", " -c"]));
        // and it is not the same term as the one without the space
        let out = merge(
            &s(&["-gd"]),
            &Edit {
                add: s(&[" -gd"]),
                remove: Vec::new(),
            },
        );
        assert_eq!(out, s(&["-gd", " -gd"]));
    }

    #[test]
    fn a_removal_takes_a_default_out_whatever_its_case() {
        let out = merge(
            &s(&["Gd", "dotarem"]),
            &Edit {
                add: Vec::new(),
                remove: s(&["GD"]),
            },
        );
        assert_eq!(out, s(&["dotarem"]));
    }

    #[test]
    fn a_removal_beats_an_addition_of_the_same_term() {
        let out = merge(
            &s(&["gd"]),
            &Edit {
                add: s(&["clariscan"]),
                remove: s(&["clariscan"]),
            },
        );
        assert_eq!(out, s(&["gd"]));
    }
}
