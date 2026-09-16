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
//!
//! Pack contract 5 (record 26, decision 12): an overlay amends the named
//! `buckets` as before, and beside them every axis value's word list, named
//! `lists.<axis>.<value>`. Words are all it amends. The flags, the physics,
//! the review thresholds and the order the values are tried in stay the
//! pack's, and an overlay that reaches for any of them is refused with why.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::error::{Error, R};
use crate::rules::Axis;
use crate::yaml::{self, File};

/// What an overlay does to one bucket or one list.
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
    /// The axis values' word lists it amends, keyed `axis.value` as written
    /// (the value's identity or its label).
    pub lists: BTreeMap<String, Edit>,
    /// What the site says its amendment does. An overlay carries its own
    /// cases for the same reason a pack does: it changes verdicts, and the
    /// pack author's cases are the author's claim about the pack, not a
    /// constraint on the site.
    pub cases: Vec<(std::path::PathBuf, crate::corpus::Case)>,
}

/// What an overlay says, and nothing else.
const SAYS: &[&str] = &[
    "overlay", "version", "pack", "scope", "buckets", "lists", "cases",
];

/// The keys of a pack, an axis or a value an overlay may not carry, at the
/// top or inside an edit: naming one of these is reaching for what stays
/// the pack's.
const STAYS: &[&str] = &[
    "flags",
    "physics",
    "thresholds",
    "review",
    "order",
    "tiers",
    "parsers",
    "normalize",
    "axes",
    "rules",
    "passes",
    "picks",
    "private",
    "dictionary",
    "bids",
    "levels",
    "mcp",
    "fields",
    "detection",
    "exclusive",
    "combination",
    "alternative_flags",
    "confidence",
    "requires",
    "default",
    "values",
    "search",
];

const STAYS_WHY: &str = "an overlay amends words, the buckets by name and the lists by axis.value, and nothing else; \
     the flags, the physics, the thresholds and the order the values are tried in stay the pack's";

impl Overlay {
    pub fn load(path: &Path) -> R<Overlay> {
        Overlay::of(File::read(path)?)
    }

    /// An overlay from its text (YAML, or the JSON a door received, which is
    /// YAML too). `name` is what a refusal blames.
    pub fn parse(name: &str, text: &str) -> R<Overlay> {
        Overlay::of(File::from_text(name, text)?)
    }

    /// The terms the overlay adds, over every bucket and every list: what
    /// `overlay_unused` counts against the citations of a batch (Wave 4c
    /// §6.6).
    pub fn added_terms(&self) -> Vec<String> {
        self.buckets
            .values()
            .chain(self.lists.values())
            .flat_map(|e| e.add.iter().cloned())
            .collect()
    }

    /// The edit this overlay makes to one axis value's list, named by the
    /// value's identity or its label.
    pub fn list_edit(&self, axis: &str, id: &str, label: &str) -> Option<&Edit> {
        self.lists
            .get(&format!("{axis}.{id}"))
            .or_else(|| self.lists.get(&format!("{axis}.{label}")))
    }

    fn of(f: File) -> R<Overlay> {
        let m = f.blame(yaml::obj(&f.value, "overlay"))?;

        // Reaching for anything but words is refused before anything else
        // is read: the refusal names the key and says what stays whose.
        for key in m.keys() {
            if STAYS.contains(&key.as_str()) {
                return Err(Error::at(key, STAYS_WHY).in_file(&f.path, Some(&f.source)));
            }
            if !SAYS.contains(&key.as_str()) {
                return Err(Error::at(
                    key,
                    format!(
                        "is not something an overlay says; it says {}",
                        SAYS.join(", ")
                    ),
                )
                .in_file(&f.path, Some(&f.source)));
            }
        }

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
        if let Some(b) = m.get("buckets") {
            for (name, edit) in f.blame(yaml::obj(b, "buckets"))? {
                buckets.insert(name.clone(), edit_of(&f, &format!("buckets.{name}"), edit)?);
            }
        }
        let mut lists = BTreeMap::new();
        if let Some(l) = m.get("lists") {
            for (name, edit) in f.blame(yaml::obj(l, "lists"))? {
                let at = format!("lists.{name}");
                if !name
                    .split_once('.')
                    .is_some_and(|(axis, value)| !axis.is_empty() && !value.is_empty())
                {
                    return Err(Error::at(
                        &at,
                        "a list is named axis.value: the axis, a dot, and one of its values",
                    )
                    .in_file(&f.path, Some(&f.source)));
                }
                lists.insert(name.clone(), edit_of(&f, &at, edit)?);
            }
        }
        if buckets.is_empty() && lists.is_empty() {
            return Err(Error::at(
                "buckets",
                "amends no bucket and no list, so it changes nothing; name a bucket or a list as axis.value",
            )
            .in_file(&f.path, Some(&f.source)));
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
            lists,
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

    /// Refuse a list naming an axis or a value the pack does not have, or a
    /// value no word of the pack's reaches. Amending the words of a value
    /// nothing tries would make the axis try it, and the order the values
    /// are tried in stays the pack's. `lists` is `axis.value`, by identity,
    /// of every value that takes a list ([`crate::rules::amendable`]).
    pub fn check_lists(&self, axes: &[Axis], lists: &[String]) -> R<()> {
        for name in self.lists.keys() {
            let at = format!("lists.{name}");
            let (axis, value) = name.split_once('.').unwrap_or((name, ""));
            let Some(a) = axes.iter().find(|a| a.name == axis) else {
                let names: Vec<&str> = axes.iter().map(|a| a.name.as_str()).collect();
                return Err(Error::at(
                    at,
                    format!(
                        "the pack has no axis named {axis}; it decides {}",
                        names.join(", ")
                    ),
                ));
            };
            let Some(v) = a.values.iter().find(|v| v.id == value || v.label == value) else {
                return Err(Error::at(at, format!("{axis} has no value named {value}")));
            };
            if !lists.iter().any(|l| *l == format!("{axis}.{}", v.id)) {
                return Err(Error::at(
                    at,
                    format!(
                        "no word of the pack's reaches {value} on {axis}: a route sets it or it is the default, \
                         and the order the values are tried in stays the pack's"
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// One edit: what it adds and what it removes, and nothing else.
fn edit_of(f: &File, at: &str, v: &Value) -> R<Edit> {
    let e = f.blame(yaml::obj(v, at))?;
    for key in e.keys() {
        if STAYS.contains(&key.as_str()) {
            return Err(
                Error::at(format!("{at}.{key}"), STAYS_WHY).in_file(&f.path, Some(&f.source))
            );
        }
        if key != "add" && key != "remove" {
            return Err(Error::at(
                format!("{at}.{key}"),
                "an edit adds words and removes words, and says nothing else",
            )
            .in_file(&f.path, Some(&f.source)));
        }
    }
    let mut out = Edit::default();
    if let Some(a) = e.get("add") {
        out.add = f.blame(yaml::texts(a, &format!("{at}.add")))?;
    }
    if let Some(r) = e.get("remove") {
        out.remove = f.blame(yaml::texts(r, &format!("{at}.remove")))?;
    }
    if out.add.is_empty() && out.remove.is_empty() {
        return Err(
            Error::at(at, "adds nothing and removes nothing").in_file(&f.path, Some(&f.source))
        );
    }
    Ok(out)
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

    #[test]
    fn a_list_is_read_beside_the_buckets_and_its_terms_are_counted() {
        let o = Overlay::parse(
            "overlay",
            "\
overlay: site
version: 1.0.0
pack: t
scope: {station: MR1}
buckets:
  agents: {add: [clariscan]}
lists:
  technique.TSE: {add: [zzturbo], remove: [turbo]}
cases:
  - name: c
    stack: {text_series_description: zzturbo}
    axes: {technique: TSE}
",
        )
        .unwrap();
        assert_eq!(o.added_terms(), s(&["clariscan", "zzturbo"]));
        assert_eq!(o.lists["technique.TSE"].remove, s(&["turbo"]));
        assert!(o.list_edit("technique", "TSE", "TSE").is_some());
        assert!(o.list_edit("technique", "SE", "SE").is_none());
    }

    #[test]
    fn a_list_alone_is_an_overlay_and_neither_is_not() {
        let o = Overlay::parse(
            "overlay",
            "overlay: s\nversion: 1.0.0\npack: t\nscope: {model: X}\nlists:\n  kind.a: {add: [x]}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: a}}\n",
        )
        .unwrap();
        assert!(o.buckets.is_empty());
        let e = Overlay::parse(
            "overlay",
            "overlay: s\nversion: 1.0.0\npack: t\nscope: {model: X}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: a}}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("changes nothing"), "{e}");
    }

    #[test]
    fn a_list_not_named_axis_dot_value_is_refused() {
        let e = Overlay::parse(
            "overlay",
            "overlay: s\nversion: 1.0.0\npack: t\nscope: {model: X}\nlists:\n  kind: {add: [x]}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: a}}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("lists.kind"), "{e}");
        assert!(e.contains("named axis.value"), "{e}");
    }

    #[test]
    fn reaching_for_a_flag_the_order_or_a_window_is_refused_with_why() {
        for (body, at) in [
            ("flags:\n  is_x: true\n", "flags"),
            ("order: [a, b]\n", "order"),
            ("physics:\n  - {value: a, when: {tr: 1}}\n", "physics"),
            ("review:\n  low_confidence: 0.1\n", "review"),
            (
                "lists:\n  kind.a: {exclusive: is_x}\n",
                "lists.kind.a.exclusive",
            ),
            (
                "lists:\n  kind.a: {add: [x], confidence: 0.5}\n",
                "lists.kind.a.confidence",
            ),
            (
                "buckets:\n  agents: {add: [x], order: [x]}\n",
                "buckets.agents.order",
            ),
        ] {
            let text = format!(
                "overlay: s\nversion: 1.0.0\npack: t\nscope: {{model: X}}\n{body}cases:\n  - {{name: c, stack: {{text_series_description: x}}, axes: {{kind: a}}}}\n"
            );
            let e = Overlay::parse("overlay", &text).err().unwrap().to_string();
            assert!(e.contains(at), "{at}: {e}");
            assert!(e.contains("stay the pack's"), "{at}: {e}");
        }
        let e = Overlay::parse(
            "overlay",
            "overlay: s\nversion: 1.0.0\npack: t\nscope: {model: X}\nlists:\n  kind.a: {add: [x], note: y}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: a}}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("says nothing else"), "{e}");
        let e = Overlay::parse(
            "overlay",
            "overlay: s\nversion: 1.0.0\npack: t\nscope: {model: X}\nnote: y\nlists:\n  kind.a: {add: [x]}\ncases:\n  - {name: c, stack: {text_series_description: x}, axes: {kind: a}}\n",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(e.contains("not something an overlay says"), "{e}");
    }
}
