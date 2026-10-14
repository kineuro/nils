// SPDX-License-Identifier: AGPL-3.0-only

//! A pipeline's versions as a new run is offered them (Nima's ruling of
//! 2026-10-12: "what i like to have it when we have multiple version is to
//! be able to select the version ... sometimes old pipline is needed. if it
//! fixes any bug then we simply dont show previous version").
//!
//! Each version of a name stands one way:
//!
//! - `current`: the newest active version, which a name alone runs;
//! - `selectable`: an older active version no later one replaces, which a
//!   person may still choose;
//! - `replaced`: an active version a later active one replaces because it
//!   fixes it (`x-nils.replaces` of the later descriptor), with which one
//!   and why. A new run does not take it: the run door, the jobs door and
//!   the pre-flight refuse it naming the version that replaces it, and only
//!   `nils run <name>@<version> --replaced` at the keyboard runs it still.
//!   Its runs, derivatives and checks read and name it as before;
//! - `retired`: as `nils pipeline retire` left it, which replaces nothing.
//!
//! Nothing is written: a version's standing is read from the descriptors
//! the catalog keeps, so it holds for every version already there.

use std::collections::BTreeMap;

use nils_pipeline::descriptor;
use nils_registry::pipeline::{self as rows, Pipeline};
use nils_registry::store::Store;
use serde_json::{Value, json};

/// How one version stands for a new run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Standing {
    /// current, selectable, replaced or retired.
    pub(crate) state: &'static str,
    /// The version that replaces it, earliest first: its id and label.
    pub(crate) replaced_by: Option<(i64, String)>,
    /// Why, as the version that replaces it says.
    pub(crate) why: Option<String>,
    /// The name's current version, its id and label; none where every
    /// version is retired.
    pub(crate) current: Option<(i64, String)>,
}

impl Standing {
    pub(crate) fn replaced(&self) -> bool {
        self.state == "replaced"
    }

    /// The fields a catalog entry carries of it.
    pub(crate) fn fields(&self, doc: &mut Value) {
        doc["standing"] = json!(self.state);
        doc["replaced_by"] = json!(self.replaced_by.as_ref().map(|(_, l)| l));
        doc["replaced_by_id"] = json!(self.replaced_by.as_ref().map(|(id, _)| id));
        doc["replaced_why"] = json!(self.why);
        doc["current"] = json!(self.current.as_ref().map(|(_, l)| l));
    }

    /// The words that refuse a new run of it, where it is replaced.
    pub(crate) fn refusal(&self, label: &str) -> Option<String> {
        if !self.replaced() {
            return None;
        }
        let by = self
            .replaced_by
            .as_ref()
            .map_or("a newer version", |(_, l)| l.as_str());
        let why = self
            .why
            .as_deref()
            .map(|w| format!(": {}", w.trim_end_matches('.')))
            .unwrap_or_default();
        let take = match &self.current {
            Some((_, c)) => format!("; a new run takes {c}"),
            None => String::new(),
        };
        Some(format!("{label} is replaced by {by}{why}{take}"))
    }
}

/// The standing of each version of one name, by id. `versions` are that
/// name's, in any order.
pub(crate) fn of_versions(versions: &[Pipeline]) -> BTreeMap<i64, Standing> {
    let mut sorted: Vec<&Pipeline> = versions.iter().collect();
    sorted.sort_by_key(|p| p.version);
    let replaces: Vec<Option<descriptor::Replaces>> = sorted
        .iter()
        .map(|p| descriptor::replaces_of(&p.descriptor))
        .collect();
    // the earliest later active version that replaces each one
    let by: Vec<Option<usize>> = sorted
        .iter()
        .enumerate()
        .map(|(i, p)| {
            (i + 1..sorted.len()).find(|&j| {
                sorted[j].state == "active"
                    && sorted[j].version > p.version
                    && replaces[j].as_ref().is_some_and(|r| r.covers(p.version))
            })
        })
        .collect();
    let current = sorted
        .iter()
        .enumerate()
        .rev()
        .find(|(i, p)| p.state == "active" && by[*i].is_none())
        .map(|(_, p)| (p.id, p.label()));
    sorted
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let state = if p.state != "active" {
                "retired"
            } else if by[i].is_some() {
                "replaced"
            } else if current.as_ref().is_some_and(|(id, _)| *id == p.id) {
                "current"
            } else {
                "selectable"
            };
            let replacer = by[i].filter(|_| state == "replaced");
            (
                p.id,
                Standing {
                    state,
                    replaced_by: replacer.map(|j| (sorted[j].id, sorted[j].label())),
                    why: replacer.and_then(|j| replaces[j].as_ref().map(|r| r.why.clone())),
                    current: current.clone(),
                },
            )
        })
        .collect()
}

/// The standing of every version of the catalog, by id.
pub(crate) fn of_catalog(list: &[Pipeline]) -> BTreeMap<i64, Standing> {
    let mut by_name: BTreeMap<&str, Vec<Pipeline>> = BTreeMap::new();
    for p in list {
        by_name.entry(p.name.as_str()).or_default().push(p.clone());
    }
    by_name
        .values()
        .flat_map(|versions| of_versions(versions))
        .collect()
}

/// The standing of one version, read with the other versions of its name.
pub(crate) fn of_one(store: &mut Store, p: &Pipeline) -> Result<Standing, nils_registry::Error> {
    let versions = rows::versions(store, &p.name)?;
    Ok(of_versions(&versions)
        .remove(&p.id)
        .unwrap_or_else(|| Standing {
            state: if p.state == "active" {
                "current"
            } else {
                "retired"
            },
            replaced_by: None,
            why: None,
            current: None,
        }))
}

/// The refusal of a new run of `p`, where a later version replaces it.
pub(crate) fn refused(store: &mut Store, p: &Pipeline) -> Option<String> {
    of_one(store, p).ok()?.refusal(&p.label())
}

/// The catalog by name (the doors' and `nils pipeline list --json`'s
/// `names`): each name with its current version, the versions a new run
/// may take, newest first, and every version with its standing.
pub(crate) fn names(list: &[Pipeline], standing: &BTreeMap<i64, Standing>) -> Value {
    let mut by_name: BTreeMap<&str, Vec<&Pipeline>> = BTreeMap::new();
    for p in list {
        by_name.entry(p.name.as_str()).or_default().push(p);
    }
    json!(
        by_name
            .into_iter()
            .map(|(name, mut versions)| {
                versions.sort_by_key(|p| std::cmp::Reverse(p.version));
                let of = |p: &Pipeline| standing.get(&p.id);
                let current = versions
                    .iter()
                    .find(|p| of(p).is_some_and(|s| s.state == "current"));
                json!({
                    "name": name,
                    "current": current.map(|p| p.id),
                    "current_label": current.map(|p| p.label()),
                    "offered": versions
                        .iter()
                        .filter(|p| of(p).is_some_and(|s| matches!(s.state, "current" | "selectable")))
                        .map(|p| p.id)
                        .collect::<Vec<_>>(),
                    "versions": versions
                        .iter()
                        .map(|p| {
                            let s = of(p);
                            json!({
                                "id": p.id,
                                "version": p.version,
                                "label": p.label(),
                                "standing": s.map(|s| s.state),
                                "replaced_by": s.and_then(|s| s.replaced_by.as_ref().map(|(_, l)| l.clone())),
                                "replaced_why": s.and_then(|s| s.why.clone()),
                            })
                        })
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: i64, version: i64, state: &str, replaces: Value) -> Pipeline {
        Pipeline {
            id,
            name: "seg".into(),
            version,
            tool_version: "1".into(),
            descriptor: json!({"x-nils": {"replaces": replaces}}),
            descriptor_digest: format!("sha256:{id}"),
            image: "img".into(),
            image_digest: "sha256:x".into(),
            layout: "bids".into(),
            level: "session".into(),
            state: state.into(),
            added_by: "t".into(),
            added_at: "t".into(),
            origin: None,
        }
    }

    #[test]
    fn a_fix_replaces_the_versions_it_names_and_a_newer_one_without_replaces_leaves_them_offered() {
        // v1, v2 replaces every earlier version (v1), v3 a version a person
        // keeps beside v2 (n4's case): v1 replaced by v2, v2 selectable, v3
        // current
        let list = [
            version(10, 1, "active", Value::Null),
            version(11, 2, "active", json!({"why": "memory"})),
            version(12, 3, "active", Value::Null),
        ];
        let s = of_versions(&list);
        assert_eq!(s[&10].state, "replaced");
        assert_eq!(s[&10].replaced_by, Some((11, "seg@2".into())));
        assert_eq!(s[&10].why.as_deref(), Some("memory"));
        assert_eq!(s[&11].state, "selectable");
        assert_eq!(s[&12].state, "current");
        assert_eq!(
            s[&10].refusal("seg@1").unwrap(),
            "seg@1 is replaced by seg@2: memory; a new run takes seg@3"
        );
        assert_eq!(s[&11].refusal("seg@2"), None);
        // a version named in the list alone; the earliest replacer is named
        let list = [
            version(1, 1, "active", Value::Null),
            version(2, 2, "active", Value::Null),
            version(3, 3, "active", json!({"versions": [1], "why": "a"})),
            version(4, 4, "active", json!({"why": "b"})),
        ];
        let s = of_versions(&list);
        assert_eq!(
            (s[&1].state, s[&1].replaced_by.clone()),
            ("replaced", Some((3, "seg@3".into())))
        );
        assert_eq!(s[&2].state, "replaced");
        assert_eq!(s[&2].why.as_deref(), Some("b"));
        assert_eq!(s[&3].state, "replaced");
        assert_eq!(s[&4].state, "current");
        // a retired fix replaces nothing: the version it fixed is current again
        let list = [
            version(1, 1, "active", Value::Null),
            version(2, 2, "retired", json!({"why": "x"})),
        ];
        let s = of_versions(&list);
        assert_eq!((s[&1].state, s[&2].state), ("current", "retired"));
        let n = names(&list, &s);
        assert_eq!(n[0]["current"], 1);
        assert_eq!(n[0]["offered"], json!([1]));
        assert_eq!(n[0]["versions"][0]["standing"], "retired");
    }
}
