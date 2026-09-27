// SPDX-License-Identifier: AGPL-3.0-only
//! Every part of an install beside its own newest release.
//!
//! The engine, the desk, the assistant and Kvasir each release on their own:
//! the desk may publish a release with no engine release beside it, and the
//! assistant and Kvasir carry tags of their own. So whether an update is
//! there is asked of each part against its own releases, and an update is
//! offered when any one part is behind, never only when the engine is.
//! `nils update --check` says it part by part, and the supervisor's install
//! door hands the same list to the desk's Parts page.
//!
//! One part's newer release may still need another's: a desk speaks the
//! engine's contracts, and a desk whose floor is above what the engine speaks
//! would refuse to start. A desk release names its contracts in a
//! `contracts.json` beside its binaries, and a desk that needs more than the
//! engine speaks is said to be held, and is not installed.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::setup::{PartState, State};
use crate::update;

/// The parts that have releases of their own, in the order an update takes
/// them. Postgres stays at its major version, and llama.cpp is the build the
/// engine's release pins, so neither is among them.
pub(crate) const OWN_RELEASES: [&str; 4] = ["engine", "desk", "assistant", "kvasir"];

/// The contract versions the engine this binary is speaks.
pub(crate) fn engine_contracts() -> Contracts {
    Contracts {
        openapi: crate::serve::OPENAPI_VERSION.trim().parse().unwrap_or(0),
        suite: crate::serve::SUITE_VERSION.trim().parse().unwrap_or(0),
    }
}

/// An engine's HTTP contract and suite contract, as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Contracts {
    pub(crate) openapi: u32,
    pub(crate) suite: u32,
}

/// What a desk release says it needs of the engine: the lowest HTTP contract
/// and suite contract it starts against. Read from the release's
/// `contracts.json`: `{openapi, openapi_floor, suite, suite_floor}`, each a
/// number or a string holding one.
pub(crate) fn floor_of(doc: &Value) -> Option<Contracts> {
    let num = |key: &str| match &doc[key] {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    Some(Contracts {
        openapi: num("openapi_floor")?,
        suite: num("suite_floor")?,
    })
}

/// Why a desk that needs `floor` cannot run beside an engine speaking
/// `engine`, or `None` where it can.
pub(crate) fn held_by(version: &str, floor: Contracts, engine: Contracts) -> Option<String> {
    let mut short = Vec::new();
    if floor.openapi > engine.openapi {
        short.push(format!(
            "HTTP contract {} (the engine speaks {})",
            floor.openapi, engine.openapi
        ));
    }
    if floor.suite > engine.suite {
        short.push(format!(
            "suite contract {} (the engine speaks {})",
            floor.suite, engine.suite
        ));
    }
    (!short.is_empty()).then(|| {
        format!(
            "desk {version} needs {}; it waits for an engine release that speaks it",
            short.join(" and ")
        )
    })
}

/// What a desk release needs of the engine, where it says: `Ok(None)` for a
/// release that publishes no `contracts.json`, which every desk before this
/// file was is, and whose floor every engine since speaks. With `checked`,
/// the file is read against the release's own sums, as an install reads it.
pub(crate) fn desk_floor(
    base: &str,
    version: &str,
    checked: bool,
) -> Result<Option<Contracts>, String> {
    let sums = crate::supervise::fetch(&format!("{base}/download/v{version}/SHA256SUMS"));
    let named = match &sums {
        Ok(bytes) => String::from_utf8_lossy(bytes)
            .lines()
            .any(|l| l.trim_end().ends_with("contracts.json")),
        Err(_) => false,
    };
    if !named {
        return Ok(None);
    }
    let bytes = if checked {
        update::fetch_checked(base, version, "contracts.json").map_err(|e| e.message)?
    } else {
        crate::supervise::fetch(&format!("{base}/download/v{version}/contracts.json"))?
    };
    let doc: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("desk {version}'s contracts.json is not JSON: {e}"))?;
    floor_of(&doc)
        .map(Some)
        .ok_or_else(|| format!("desk {version}'s contracts.json names no floor"))
}

/// One part beside its own newest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartRelease {
    pub(crate) part: String,
    /// The version installed, where it can be read.
    pub(crate) installed: Option<String>,
    /// The newest version the part's own releases name.
    pub(crate) newest: Option<String>,
    /// Why the newest could not be read.
    pub(crate) error: Option<String>,
    /// A ref a lab named for this part in place of its releases, which it
    /// follows instead; nothing is offered for such a part.
    pub(crate) follows: Option<String>,
    /// Why the newer version is not taken yet, where another part holds it.
    pub(crate) held: Option<String>,
}

impl PartRelease {
    /// The newer version this part may move to, where it is behind.
    pub(crate) fn newer(&self) -> Option<&str> {
        if self.follows.is_some() {
            return None;
        }
        match (self.newest.as_deref(), self.installed.as_deref()) {
            (Some(n), Some(have)) if update::newer(n, have) => Some(n),
            _ => None,
        }
    }

    /// Whether an update would move it now.
    pub(crate) fn behind(&self) -> bool {
        self.newer().is_some() && self.held.is_none()
    }

    /// The line `nils update --check` says for it.
    pub(crate) fn line(&self) -> String {
        let at = self.installed.as_deref().unwrap_or("a version not known");
        let name = &self.part;
        if let Some(r) = &self.follows {
            return format!("{name} {at}: follows {r}, which a lab named in place of its releases");
        }
        if let Some(e) = &self.error {
            return format!("{name} {at}: its newest release could not be read: {e}");
        }
        match (self.newer(), &self.held) {
            (Some(n), Some(why)) => format!("{name} {at}: {n} is out and waits: {why}"),
            (Some(n), None) => format!("{name} {at}: {n} is out"),
            (None, _) => match &self.newest {
                Some(n) if self.installed.as_deref() == Some(n.as_str()) => {
                    format!("{name} {at}: the newest release")
                }
                Some(n) => format!("{name} {at}: the newest release is {n}"),
                None => format!("{name} {at}"),
            },
        }
    }

    pub(crate) fn doc(&self) -> Value {
        json!({
            "part": self.part,
            "installed": self.installed,
            "newest": self.newest,
            "newer": self.newer(),
            "held": self.held,
            "follows": self.follows,
            "error": self.error,
            "command": format!("nils update --part {}", self.part),
        })
    }
}

/// The version of a part as installed: the record's, and for a part built
/// from source the version its own `package.json` names, since the record
/// holds only that it came from source.
pub(crate) fn installed_of(part: &PartState) -> Option<String> {
    if part.kind == "node" {
        let text = std::fs::read_to_string(Path::new(&part.path).join("package.json")).ok()?;
        let doc: Value = serde_json::from_str(&text).ok()?;
        return doc["version"]
            .as_str()
            .map(|v| v.trim_start_matches('v').to_string());
    }
    let v = part.version.trim().trim_start_matches('v');
    (!v.is_empty() && v != "from source").then(|| v.to_string())
}

/// What one lookup of a part's newest release came to: the version, or why
/// there is none.
pub(crate) type Newest = Result<String, String>;

/// Every part of the install that has releases of its own, beside its newest,
/// with `look` asking a part's releases for their newest version. A desk
/// newer than it is installed is read for the contracts it needs of the
/// engine, with `floor`, against `engine`.
pub(crate) fn part_releases(
    state: &State,
    look: &mut dyn FnMut(&str) -> Newest,
    floor: &mut dyn FnMut(&str) -> Result<Option<Contracts>, String>,
    engine: Contracts,
) -> Vec<PartRelease> {
    let mut out = Vec::new();
    for name in OWN_RELEASES {
        let Some(part) = state.parts.get(name) else {
            continue;
        };
        let follows = crate::setup::named_source_ref(name);
        let (newest, error) = if follows.is_some() {
            (None, None)
        } else {
            match look(name) {
                Ok(v) => (Some(v.trim_start_matches('v').to_string()), None),
                Err(e) => (None, Some(e)),
            }
        };
        let mut row = PartRelease {
            part: name.to_string(),
            installed: installed_of(part),
            newest,
            error,
            follows,
            held: None,
        };
        if name == "desk"
            && let Some(n) = row.newer().map(str::to_string)
        {
            match floor(&n) {
                Ok(Some(needs)) => row.held = held_by(&n, needs, engine),
                Ok(None) => {}
                Err(e) => {
                    row.held = Some(format!(
                        "what desk {n} needs of the engine could not be read: {e}"
                    ))
                }
            }
        }
        out.push(row);
    }
    // A desk the engine's own update will bring the contracts for is not
    // held: the engine goes first.
    if out.iter().any(|r| r.part == "engine" && r.behind()) {
        for r in out.iter_mut().filter(|r| r.part == "desk") {
            if r.held
                .as_deref()
                .is_some_and(|h| h.contains("waits for an engine release"))
            {
                r.held = None;
            }
        }
    }
    out
}

/// The newest release of one part, from where its releases come from.
pub(crate) fn newest_of(part: &str, channel: Option<&str>) -> Newest {
    match part {
        "engine" => update::newest_version(&update::engine_base(channel)).map_err(|e| e.message),
        "desk" => update::newest_version(&update::desk_base(channel)).map_err(|e| e.message),
        _ => match crate::setup::node_repo(part) {
            Some(repo) => newest_tag(repo),
            None => Err(format!("{part} has no releases of its own")),
        },
    }
}

/// The newest version tag of a git repository, asked of the repository
/// itself (`git ls-remote`), which has no API to be refused by: the assistant
/// and Kvasir publish tags and no release pages.
pub(crate) fn newest_tag(repo: &str) -> Newest {
    let out = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", repo])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{repo}: {}",
            said.lines().last().unwrap_or("git ls-remote failed").trim()
        ));
    }
    newest_listed(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("{repo} has no version tag"))
}

/// The highest version among the tags `git ls-remote --tags` lists.
pub(crate) fn newest_listed(listing: &str) -> Option<String> {
    listing
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/v"))
        .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
        .reduce(|best, v| if update::newer(&v, &best) { v } else { best })
}

/// The ref a part built from source is updated to: what a lab names, else
/// the newer of the tag this engine pins and the part's own newest tag, so
/// the part follows its own releases and never goes below what the engine
/// was released beside.
pub(crate) fn source_update_ref(pinned: &str, newest: Option<&str>) -> String {
    match newest {
        Some(n) if update::newer(n, pinned) => format!("v{}", n.trim_start_matches('v')),
        _ => pinned.to_string(),
    }
}

/// The install door's `release` object: the engine's release as before, for
/// a desk older than the list, and `parts`, each part beside its own newest.
/// `newer` is the engine's newer version, or else the first part that is
/// behind, named, so an older desk offers the update too.
pub(crate) fn release_doc(rows: &[PartRelease], engine_error: Option<&str>) -> Value {
    let engine = rows.iter().find(|r| r.part == "engine");
    let newer = engine
        .and_then(PartRelease::newer)
        .map(str::to_string)
        .or_else(|| {
            rows.iter()
                .find(|r| r.behind())
                .and_then(|r| r.newer().map(|n| format!("{} {n}", r.part)))
        });
    let behind: Vec<&str> = rows
        .iter()
        .filter(|r| r.behind())
        .map(|r| r.part.as_str())
        .collect();
    json!({
        "installed": engine.and_then(|r| r.installed.clone()),
        "newest": engine.and_then(|r| r.newest.clone()),
        "newer": newer,
        "error": engine.and_then(|r| r.error.clone()).or(engine_error.map(str::to_string)),
        "command": "nils update --all",
        "behind": behind,
        "parts": rows.iter().map(PartRelease::doc).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn part(version: &str, kind: &str) -> PartState {
        PartState {
            version: version.to_string(),
            path: "/nowhere".to_string(),
            kind: kind.to_string(),
        }
    }

    fn state(engine: &str, desk: &str) -> State {
        let mut parts = BTreeMap::new();
        parts.insert("engine".to_string(), part(engine, "binary"));
        parts.insert("desk".to_string(), part(desk, "binary"));
        parts.insert("postgres".to_string(), part("16", "podman"));
        toml::from_str::<State>("dir = \"/srv/nils\"\nmode = \"local\"\n")
            .map(|mut s| {
                s.parts = parts;
                s
            })
            .unwrap()
    }

    const SPEAKS: Contracts = Contracts {
        openapi: 7,
        suite: 3,
    };

    fn rows(state: &State, newest: &[(&str, &str)], floor: Option<Contracts>) -> Vec<PartRelease> {
        let newest: BTreeMap<String, String> = newest
            .iter()
            .map(|(p, v)| ((*p).to_string(), (*v).to_string()))
            .collect();
        part_releases(
            state,
            &mut |p| {
                newest
                    .get(p)
                    .cloned()
                    .ok_or_else(|| format!("{p}: not published"))
            },
            &mut |_| Ok(floor),
            SPEAKS,
        )
    }

    #[test]
    fn a_desk_released_alone_is_offered_while_the_engine_is_the_newest() {
        let s = state("1.0.0-alpha.49", "1.0.0-alpha.49");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.51")],
            None,
        );
        let engine = r.iter().find(|r| r.part == "engine").unwrap();
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(!engine.behind(), "{engine:?}");
        assert!(desk.behind(), "{desk:?}");
        assert_eq!(desk.newer(), Some("1.0.0-alpha.51"));
        assert_eq!(desk.line(), "desk 1.0.0-alpha.49: 1.0.0-alpha.51 is out");
        assert_eq!(engine.line(), "engine 1.0.0-alpha.49: the newest release");
        assert!(
            r.iter().all(|r| r.part != "postgres"),
            "postgres has no releases of its own"
        );

        let doc = release_doc(&r, None);
        assert_eq!(doc["behind"], json!(["desk"]));
        assert_eq!(doc["installed"], "1.0.0-alpha.49");
        // an older desk reads `newer` alone, and is offered the update too
        assert_eq!(doc["newer"], "desk 1.0.0-alpha.51");
        let desk_doc = doc["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "desk")
            .unwrap();
        assert_eq!(desk_doc["newer"], "1.0.0-alpha.51");
        assert_eq!(desk_doc["command"], "nils update --part desk");
    }

    #[test]
    fn parts_at_their_newest_offer_nothing() {
        let s = state("1.0.0-alpha.49", "1.0.0-alpha.51");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.51")],
            None,
        );
        assert!(r.iter().all(|r| !r.behind()), "{r:?}");
        let doc = release_doc(&r, None);
        assert_eq!(doc["newer"], Value::Null);
        assert_eq!(doc["behind"], json!([]));
    }

    #[test]
    fn an_engine_behind_is_offered_by_its_own_version() {
        let s = state("1.0.0-alpha.48", "1.0.0-alpha.51");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.51")],
            None,
        );
        let doc = release_doc(&r, None);
        assert_eq!(doc["newer"], "1.0.0-alpha.49");
        assert_eq!(doc["behind"], json!(["engine"]));
    }

    #[test]
    fn a_dev_build_is_ahead_of_the_release_it_came_from_and_behind_the_next_build() {
        // a development channel serves both parts as N.dev.M
        let s = state("1.0.0-alpha.49.dev.2", "1.0.0-alpha.49.dev.2");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.49")],
            None,
        );
        assert!(
            r.iter().all(|r| !r.behind()),
            "a dev build is not behind its release: {r:?}"
        );
        let r = rows(
            &s,
            &[
                ("engine", "1.0.0-alpha.49.dev.2"),
                ("desk", "1.0.0-alpha.49.dev.3"),
            ],
            None,
        );
        let behind: Vec<_> = r
            .iter()
            .filter(|r| r.behind())
            .map(|r| r.part.as_str())
            .collect();
        assert_eq!(behind, ["desk"]);
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.50"), ("desk", "1.0.0-alpha.50")],
            None,
        );
        assert_eq!(r.iter().filter(|r| r.behind()).count(), 2, "{r:?}");
    }

    #[test]
    fn a_desk_that_needs_more_than_the_engine_speaks_waits_and_says_why() {
        let s = state("1.0.0-alpha.49", "1.0.0-alpha.49");
        let needs = Contracts {
            openapi: 8,
            suite: 3,
        };
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.52")],
            Some(needs),
        );
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(!desk.behind(), "{desk:?}");
        assert_eq!(desk.newer(), Some("1.0.0-alpha.52"));
        let held = desk.held.as_deref().unwrap();
        assert!(
            held.contains("HTTP contract 8 (the engine speaks 7)"),
            "{held}"
        );
        assert!(desk.line().contains("waits"), "{}", desk.line());
        assert_eq!(release_doc(&r, None)["newer"], Value::Null);

        // where the engine is behind too, its update goes first and brings them
        let s = state("1.0.0-alpha.48", "1.0.0-alpha.49");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.52")],
            Some(needs),
        );
        assert!(r.iter().all(PartRelease::behind), "{r:?}");

        // a floor the engine speaks holds nothing
        let r = rows(
            &state("1.0.0-alpha.49", "1.0.0-alpha.49"),
            &[("engine", "1.0.0-alpha.49"), ("desk", "1.0.0-alpha.52")],
            Some(Contracts {
                openapi: 5,
                suite: 2,
            }),
        );
        assert!(r.iter().find(|r| r.part == "desk").unwrap().behind());
    }

    #[test]
    fn a_part_whose_releases_cannot_be_read_says_so_and_offers_nothing() {
        let s = state("1.0.0-alpha.49", "1.0.0-alpha.49");
        let r = rows(&s, &[("engine", "1.0.0-alpha.49")], None);
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(!desk.behind());
        assert!(desk.line().contains("could not be read"), "{}", desk.line());
    }

    #[test]
    fn a_contracts_file_names_its_floor_as_numbers_or_strings() {
        let doc = json!({"openapi": "7", "openapi_floor": "5", "suite": 3, "suite_floor": 2});
        assert_eq!(
            floor_of(&doc),
            Some(Contracts {
                openapi: 5,
                suite: 2
            })
        );
        assert_eq!(floor_of(&json!({"openapi": "7"})), None);
        assert_eq!(
            held_by(
                "1",
                Contracts {
                    openapi: 7,
                    suite: 3
                },
                SPEAKS
            ),
            None
        );
    }

    #[test]
    fn the_newest_tag_is_the_highest_version_listed() {
        let listing = "a\trefs/tags/v1.0.0-alpha.9\nb\trefs/tags/v1.0.0-alpha.10\n\
                       c\trefs/tags/vendor-x\nd\trefs/tags/v1.0.0-alpha.8\n";
        assert_eq!(newest_listed(listing).as_deref(), Some("1.0.0-alpha.10"));
        assert_eq!(newest_listed(""), None);
        assert_eq!(
            source_update_ref("v1.0.0-alpha.9", Some("1.0.0-alpha.10")),
            "v1.0.0-alpha.10"
        );
        assert_eq!(
            source_update_ref("v1.0.0-alpha.9", Some("1.0.0-alpha.8")),
            "v1.0.0-alpha.9"
        );
        assert_eq!(source_update_ref("v1.0.0-alpha.9", None), "v1.0.0-alpha.9");
    }

    #[test]
    fn a_part_from_source_is_at_the_version_its_package_names() {
        let dir = std::env::temp_dir().join(format!("nils-releases-node-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name": "x", "version": "1.0.0-alpha.27"}"#,
        )
        .unwrap();
        let p = PartState {
            version: "from source".to_string(),
            path: dir.display().to_string(),
            kind: "node".to_string(),
        };
        assert_eq!(installed_of(&p).as_deref(), Some("1.0.0-alpha.27"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(installed_of(&p), None);
        assert_eq!(installed_of(&part("from source", "binary")), None);
        assert_eq!(
            installed_of(&part("v1.0.0-alpha.3", "podman")).as_deref(),
            Some("1.0.0-alpha.3")
        );
    }
}
