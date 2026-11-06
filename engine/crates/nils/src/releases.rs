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
//! engine speaks is said to be held, and is not installed. The two carry
//! version numbers of their own that need not match: what a desk may run
//! beside is decided by those contracts, never by its number, so an install
//! takes the newest desk release whose floor its engine meets.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use crate::setup::{PartState, State};
use crate::update;

/// The parts that have releases of their own, in the order an update takes
/// them. Postgres stays at its major version, and llama.cpp is the build the
/// engine's release pins, so neither is among them. The rules are the
/// first-party packs' own releases (record 55 B5), read where the engine
/// reads its packs, so they are no part of the record's `parts` and their
/// rows are made apart ([`crate::rules`]).
pub(crate) const OWN_RELEASES: [&str; 5] = ["engine", "desk", "assistant", "kvasir", "rules"];

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

/// The contracts of `floor` that `engine` does not speak, named:
/// `HTTP contract 8`, and `suite contract 4` beside it where both are short.
pub(crate) fn short_of(floor: Contracts, engine: Contracts) -> Option<String> {
    let mut short = Vec::new();
    if floor.openapi > engine.openapi {
        short.push(format!("HTTP contract {}", floor.openapi));
    }
    if floor.suite > engine.suite {
        short.push(format!("suite contract {}", floor.suite));
    }
    (!short.is_empty()).then(|| short.join(" and "))
}

/// The contracts an engine release speaks, where it says: its
/// `contracts.json` (`{openapi, suite}`), which releases publish beside their
/// binaries from 1.0.0-beta.10 on. `Ok(None)` for a release that publishes
/// none, whose contracts are known only once it is installed.
pub(crate) fn engine_release_contracts(
    base: &str,
    version: &str,
) -> Result<Option<Contracts>, String> {
    let sums = crate::supervise::fetch(&format!("{base}/download/v{version}/SHA256SUMS"))?;
    let named = String::from_utf8_lossy(&sums)
        .lines()
        .any(|l| l.trim_end().ends_with("contracts.json"));
    if !named {
        return Ok(None);
    }
    let bytes = crate::supervise::fetch(&format!("{base}/download/v{version}/contracts.json"))?;
    let doc: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("engine {version}'s contracts.json is not JSON: {e}"))?;
    let num = |key: &str| match &doc[key] {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    match (num("openapi"), num("suite")) {
        (Some(openapi), Some(suite)) => Ok(Some(Contracts { openapi, suite })),
        _ => Err(format!(
            "engine {version}'s contracts.json names no contracts"
        )),
    }
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

/// The variable a lab pins the desk's version with, in place of the newest
/// desk release that speaks the engine, as `NILS_SETUP_KVASIR_REF` pins
/// Kvasir's tag.
pub(crate) const DESK_PIN: &str = "NILS_SETUP_DESK_VERSION";

/// The desk version a lab pinned, if one is.
pub(crate) fn desk_pin() -> Option<String> {
    std::env::var(DESK_PIN)
        .ok()
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
}

/// The newest of `versions` whose floor `engine` meets, read newest first;
/// `Ok(None)` where none does. A floor that cannot be read stops the walk
/// rather than being passed over, so a failed download never quietly picks
/// an older desk.
pub(crate) fn newest_that_speaks(
    versions: &[String],
    floor: &mut dyn FnMut(&str) -> Result<Option<Contracts>, String>,
    engine: Contracts,
) -> Result<Option<String>, String> {
    let mut sorted: Vec<&String> = versions.iter().collect();
    sorted.sort_by(|a, b| {
        if update::newer(a, b) {
            std::cmp::Ordering::Less
        } else if update::newer(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    for version in sorted {
        match floor(version) {
            Ok(None) => return Ok(Some(version.clone())),
            Ok(Some(needs)) if held_by(version, needs, engine).is_none() => {
                return Ok(Some(version.clone()));
            }
            Ok(Some(_)) => {}
            Err(why) => {
                return Err(format!(
                    "what desk {version} needs of the engine could not be read: {why}"
                ));
            }
        }
    }
    Ok(None)
}

/// The desk an install or an update takes beside this engine: the version a
/// lab pinned, else the newest desk release whose contract floor this engine
/// meets, whatever its number. With `checked`, each `contracts.json` is read
/// against its release's sums.
pub(crate) fn desk_to_take(channel: Option<&str>, checked: bool) -> Result<String, String> {
    desk_to_take_said(channel, checked).map(|(version, _)| version)
}

/// [`desk_to_take`], and beside it the newest desk release that stays out
/// and why, where the newest is not the one taken.
pub(crate) fn desk_to_take_said(
    channel: Option<&str>,
    checked: bool,
) -> Result<(String, Option<(String, String)>), String> {
    if let Some(pinned) = desk_pin() {
        return Ok((pinned, None));
    }
    let base = update::desk_base(channel);
    let versions = update::versions(&base).map_err(|e| e.message)?;
    let engine = engine_contracts();
    // why the newest waits, said where no release is taken
    let mut first_held: Option<(String, String)> = None;
    let mut floor = |v: &str| {
        let found = desk_floor(&base, v, checked);
        if first_held.is_none()
            && let Ok(Some(needs)) = &found
        {
            first_held = held_by(v, *needs, engine).map(|why| (v.to_string(), why));
        }
        found
    };
    match newest_that_speaks(&versions, &mut floor, engine)? {
        Some(version) => Ok((version, first_held)),
        None => Err(first_held.map(|(_, why)| why).unwrap_or_else(|| {
            format!(
                "no desk release speaks this engine's contracts (HTTP contract {}, suite \
                 contract {})",
                engine.openapi, engine.suite
            )
        })),
    }
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
    /// Where the newest is held, the newest release between it and the one
    /// installed that the engine does speak, which is taken instead.
    pub(crate) takes: Option<String>,
    /// What of the engine's contracts the version taken needs that the
    /// installed engine does not speak (`HTTP contract 8`): the engine's own
    /// update, which brings it, goes first.
    pub(crate) needs: Option<String>,
    /// A version tagged and newer than the newest release, whose release is
    /// not out yet: it is being built.
    pub(crate) coming: Option<String>,
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

    /// The version an update would move it to now: the newest, or where
    /// that is held, the newest release the engine speaks.
    pub(crate) fn to_take(&self) -> Option<&str> {
        let newer = self.newer()?;
        match &self.held {
            None => Some(newer),
            Some(_) => self.takes.as_deref(),
        }
    }

    /// Whether an update would move it now.
    pub(crate) fn behind(&self) -> bool {
        self.to_take().is_some()
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
        // what it needs of the engine, which goes first
        let after = self
            .needs
            .as_ref()
            .filter(|_| self.behind())
            .map(|n| format!(", after the engine: it needs {n}"))
            .unwrap_or_default();
        match (self.newer(), &self.held) {
            (Some(n), Some(why)) => match &self.takes {
                Some(t) => {
                    format!("{name} {at}: {t} is out{after} ({n} is newer and waits: {why})")
                }
                None => format!("{name} {at}: {n} is out and waits: {why}"),
            },
            (Some(n), None) => format!("{name} {at}: {n} is out{after}"),
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
            // what an update takes, so a desk's Parts page offers it
            "newer": self.to_take().or(self.newer()),
            "held": if self.takes.is_some() { None } else { self.held.clone() },
            "takes": self.takes,
            "waits": self.held,
            "follows": self.follows,
            "error": self.error,
            // added in place: what it needs of the engine's next release,
            // which goes first, and a tag whose release is being built
            "needs": self.needs,
            "after": self.needs.as_ref().map(|_| "engine"),
            "coming": self.coming,
            "command": if self.needs.is_some() && self.behind() {
                format!("nils update --part engine --part {}", self.part)
            } else {
                format!("nils update --part {}", self.part)
            },
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
/// engine, with `floor`, against `engine`; where the newest is held, the
/// desk's releases from `desk_list` are read for the newest one the engine
/// speaks, which an update takes instead.
#[cfg(test)]
pub(crate) fn part_releases(
    state: &State,
    look: &mut dyn FnMut(&str) -> Newest,
    floor: &mut dyn FnMut(&str) -> Result<Option<Contracts>, String>,
    desk_list: &mut dyn FnMut() -> Result<Vec<String>, String>,
    engine: Contracts,
) -> Vec<PartRelease> {
    part_releases_with(
        state,
        look,
        floor,
        desk_list,
        engine,
        &mut |_| Ok(None),
        true,
    )
}

/// [`part_releases`], with `offered` reading the contracts an engine
/// release speaks where it says ([`engine_release_contracts`]), and
/// `engine_goes` whether the engine's own update is in the same run: a desk
/// that needs the contract of an engine release then goes after it, and
/// where the engine is not in the run (`--part desk` alone) the desk waits
/// and says what takes both.
pub(crate) fn part_releases_with(
    state: &State,
    look: &mut dyn FnMut(&str) -> Newest,
    floor: &mut dyn FnMut(&str) -> Result<Option<Contracts>, String>,
    desk_list: &mut dyn FnMut() -> Result<Vec<String>, String>,
    engine: Contracts,
    offered: &mut dyn FnMut(&str) -> Result<Option<Contracts>, String>,
    engine_goes: bool,
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
            takes: None,
            needs: None,
            coming: None,
        };
        if name == "desk"
            && let Some(n) = row.newer().map(str::to_string)
        {
            match floor(&n) {
                Ok(Some(needs)) => {
                    row.held = held_by(&n, needs, engine);
                    if row.held.is_some()
                        && let Some(have) = row.installed.clone()
                        && let Ok(list) = desk_list()
                    {
                        let between: Vec<String> = list
                            .into_iter()
                            .filter(|v| update::newer(v, &have) && update::newer(&n, v))
                            .collect();
                        row.takes = newest_that_speaks(&between, &mut *floor, engine)
                            .ok()
                            .flatten();
                    }
                }
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
    // A desk the engine's own update brings the contracts for is not held:
    // the engine goes first, and the desk says what it needs of it. Where the
    // engine's release names its contracts and they fall short too, the desk
    // waits, said with both engines, and takes the newest the new engine
    // speaks, as the update will once that engine is in.
    let engine_row = out.iter().find(|r| r.part == "engine");
    let installed_engine = engine_row
        .and_then(|r| r.installed.clone())
        .unwrap_or_else(|| "installed".to_string());
    let next = engine_row
        .filter(|r| r.behind())
        .and_then(|r| r.to_take().map(str::to_string));
    if let Some(next) = next {
        let speaks = offered(&next).ok().flatten();
        for r in out.iter_mut().filter(|r| r.part == "desk") {
            let waits = r
                .held
                .as_deref()
                .is_some_and(|h| h.contains("waits for an engine release"));
            let Some(newest) = r.newer().map(str::to_string).filter(|_| waits) else {
                continue;
            };
            let Ok(Some(needs)) = floor(&newest) else {
                continue;
            };
            let short = short_of(needs, engine).unwrap_or_default();
            match speaks {
                Some(c) if held_by(&newest, needs, c).is_some() => {
                    r.held = Some(format!(
                        "desk {newest} needs {}, which neither engine {installed_engine} nor \
                         {next} speaks; it waits for an engine release that speaks it",
                        short_of(needs, c).unwrap_or(short)
                    ));
                    if !engine_goes {
                        // the engine is not in this run: the desk takes what
                        // the installed engine speaks, as it did
                        continue;
                    }
                    r.takes = None;
                    if let Some(have) = r.installed.clone()
                        && let Ok(list) = desk_list()
                    {
                        let between: Vec<String> = list
                            .into_iter()
                            .filter(|v| update::newer(v, &have) && update::newer(&newest, v))
                            .collect();
                        r.takes = newest_that_speaks(&between, &mut *floor, c).ok().flatten();
                    }
                    if let Some(t) = r.takes.clone()
                        && let Ok(Some(f)) = floor(&t)
                    {
                        r.needs = short_of(f, engine);
                    }
                }
                _ if engine_goes => {
                    r.held = None;
                    r.takes = None;
                    r.needs = Some(short);
                }
                _ => {
                    r.held = Some(format!(
                        "desk {newest} needs {short}, and engine {next} is out: \
                         nils update --part engine --part desk takes the engine first"
                    ));
                }
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
    newest_listed(&tags_of(repo)?).ok_or_else(|| format!("{repo} has no version tag"))
}

/// Every tag of a git repository as `git ls-remote --tags --refs` lists
/// them, asked of the repository itself: a git request, which GitHub does
/// not count against its API's hourly requests.
pub(crate) fn tags_of(repo: &str) -> Result<String, String> {
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
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The versions a listing's tags name under one prefix (`v` for the
/// engine's and the desk's own, `pack-mri-v` for a pack's), newest first.
pub(crate) fn tagged_versions(listing: &str, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = listing
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.strip_prefix("refs/tags/"))
        .filter_map(|tag| tag.strip_prefix(prefix))
        .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
        .collect();
    out.sort_by(|a, b| {
        if update::newer(a, b) {
            std::cmp::Ordering::Less
        } else if update::newer(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    out.dedup();
    out
}

/// What one watch of a repository's tags found for a part: the newest tag
/// newer than `kept` (the newest release the last look named, or none)
/// whose release is out, and the newest one whose release is not out yet,
/// which is being built. A tag is a release once `released` says its files
/// are there (its `SHA256SUMS`, a download and not a request to the API);
/// at most `MOST` tags are asked about, newest first.
pub(crate) fn watch_tags(
    kept: Option<&str>,
    listing: &str,
    prefix: &str,
    released: &mut dyn FnMut(&str) -> Result<bool, String>,
) -> Result<(Option<String>, Option<String>), String> {
    const MOST: usize = 3;
    let mut out = None;
    let mut coming = None;
    for version in tagged_versions(listing, prefix)
        .into_iter()
        .filter(|v| kept.is_none_or(|k| update::newer(v, k)))
        .take(MOST)
    {
        if released(&version)? {
            out = Some(version);
            break;
        }
        coming.get_or_insert(version);
    }
    Ok((out, coming))
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
                .and_then(|r| r.to_take().map(|n| format!("{} {n}", r.part)))
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

/// The order an update takes the parts in: the engine first, since the
/// parts after it are taken by the new engine with the versions its release
/// pins, and a new desk may need its contract; the desk after it; the
/// assistant and Kvasir, which need neither; then the engine's packs and the
/// rules, which are read against the engine in place.
pub(crate) const ORDER: [&str; 6] = ["engine", "desk", "assistant", "kvasir", "packs", "rules"];

/// Where a part comes in [`ORDER`]; a part it does not name comes last.
pub(crate) fn rank(part: &str) -> usize {
    ORDER.iter().position(|p| *p == part).unwrap_or(ORDER.len())
}

/// One part an update moves, in its place in the order, and why it is
/// there where that matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) part: String,
    /// A rules step's pack.
    pub(crate) pack: Option<String>,
    pub(crate) from: Option<String>,
    pub(crate) to: String,
    pub(crate) why: Option<String>,
}

/// A part with a newer release that an update leaves where it is, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stay {
    pub(crate) part: String,
    pub(crate) pack: Option<String>,
    pub(crate) version: String,
    pub(crate) why: String,
}

impl Step {
    fn name(&self) -> String {
        match (&self.pack, self.part.as_str()) {
            (Some(pack), _) => format!("{pack} rules"),
            (None, "kvasir") => "Kvasir".to_string(),
            (None, "packs") => "the engine's packs".to_string(),
            (None, part) => part.to_string(),
        }
    }

    /// The step as `nils update` says it.
    pub(crate) fn line(&self) -> String {
        if self.part == "packs" {
            return format!(
                "the engine's packs, to the ones engine {} was released with",
                self.to
            );
        }
        let moves = match &self.from {
            Some(from) if from != &self.to => format!("{from} to {}", self.to),
            Some(_) => format!("another build of {}", self.to),
            None => self.to.clone(),
        };
        match &self.why {
            Some(why) => format!("{} {moves}, {why}", self.name()),
            None => format!("{} {moves}", self.name()),
        }
    }

    pub(crate) fn doc(&self) -> Value {
        json!({
            "part": self.part,
            "pack": self.pack,
            "from": self.from,
            "to": self.to,
            "why": self.why,
        })
    }
}

impl Stay {
    pub(crate) fn line(&self) -> String {
        match &self.pack {
            Some(pack) => format!("{pack} rules {} stays out: {}", self.version, self.why),
            None => format!("{} {} stays out: {}", self.part, self.version, self.why),
        }
    }

    pub(crate) fn doc(&self) -> Value {
        json!({
            "part": self.part,
            "pack": self.pack,
            "version": self.version,
            "why": self.why,
        })
    }
}

/// The steps an update takes, in [`ORDER`], with why the engine goes first
/// and what waits: each part behind its own newest release, the engine's
/// packs where they are behind the release in place (`packs`, that
/// release's version, where the engine itself does not move), and each
/// rules row behind its own; and the newer releases that stay out, with why.
pub(crate) fn order(
    rows: &[PartRelease],
    rules: &[crate::rules::Row],
    packs: Option<&str>,
) -> (Vec<Step>, Vec<Stay>) {
    let mut steps: Vec<Step> = Vec::new();
    let mut stays: Vec<Stay> = Vec::new();
    for r in rows {
        if let Some(to) = r.to_take() {
            steps.push(Step {
                part: r.part.clone(),
                pack: None,
                from: r.installed.clone(),
                to: to.to_string(),
                why: None,
            });
        }
        if let (Some(newest), Some(why)) = (r.newer(), &r.held) {
            stays.push(Stay {
                part: r.part.clone(),
                pack: None,
                version: newest.to_string(),
                why: why.clone(),
            });
        }
    }
    let engine_moves = steps
        .iter()
        .find(|s| s.part == "engine")
        .map(|s| s.to.clone());
    if let Some(release) = packs.filter(|_| engine_moves.is_none()) {
        steps.push(Step {
            part: "packs".to_string(),
            pack: None,
            from: None,
            to: release.to_string(),
            why: None,
        });
    }
    for r in rules {
        if let Some(to) = &r.takes {
            steps.push(Step {
                part: crate::rules::PART.to_string(),
                pack: Some(r.pack.clone()),
                from: r.installed.clone(),
                to: to.clone(),
                why: engine_moves
                    .as_ref()
                    .map(|e| format!("after the engine: read against engine {e}")),
            });
        }
        if r.pinned.is_none()
            && let (Some(newest), Some(why)) = (&r.newest, &r.held)
            && r.takes.as_deref() != Some(newest.as_str())
        {
            stays.push(Stay {
                part: crate::rules::PART.to_string(),
                pack: Some(r.pack.clone()),
                version: newest.clone(),
                why: why.clone(),
            });
        }
    }
    steps.sort_by_key(|s| rank(&s.part));
    let desk_needs = rows
        .iter()
        .find(|r| r.part == "desk" && r.behind())
        .and_then(|r| Some((r.to_take()?.to_string(), r.needs.clone()?)));
    let more = steps.len() > 1;
    for s in &mut steps {
        s.why = match s.part.as_str() {
            "engine" => match &desk_needs {
                Some((desk, needs)) => Some(format!("first: desk {desk} needs its {needs}")),
                None if more => {
                    Some("first: the parts after it are taken by the new engine".to_string())
                }
                None => None,
            },
            "desk" => desk_needs
                .as_ref()
                .map(|(_, needs)| format!("after the engine: it needs {needs}")),
            _ => s.why.take(),
        };
    }
    (steps, stays)
}

/// The order as `nils update` says it before it starts: one numbered line
/// a step, and a line for each newer release that stays out.
pub(crate) fn order_lines(steps: &[Step], stays: &[Stay]) -> Vec<String> {
    let mut out = Vec::new();
    if !steps.is_empty() {
        out.push("in this order:".to_string());
        for (i, s) in steps.iter().enumerate() {
            out.push(format!("  {}. {}", i + 1, s.line()));
        }
    }
    for s in stays {
        out.push(format!("  {}", s.line()));
    }
    out
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
            &mut || Ok(Vec::new()),
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

    /// 2026-10-16: the Parts page named alpha.84 as the newest release a
    /// minute after alpha.85 was published, from a look kept for a quarter of
    /// an hour. A look is never what an offer is measured by alone: a part is
    /// offered only a release newer than the one installed, by the engine's own
    /// order, so a look older than what is installed (a stale answer, or an
    /// install past its releases, as a development build is) offers nothing,
    /// and never a downgrade.
    #[test]
    fn a_look_older_than_what_is_installed_offers_nothing_and_never_a_downgrade() {
        let s = state("1.0.0-alpha.85", "1.0.0-alpha.65");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.84"), ("desk", "1.0.0-alpha.64")],
            None,
        );
        assert!(
            r.iter()
                .all(|r| !r.behind() && r.newer().is_none() && r.to_take().is_none()),
            "{r:?}"
        );
        let engine = r.iter().find(|r| r.part == "engine").unwrap();
        assert_eq!(engine.newest.as_deref(), Some("1.0.0-alpha.84"));
        assert_eq!(
            engine.line(),
            "engine 1.0.0-alpha.85: the newest release is 1.0.0-alpha.84"
        );
        let doc = release_doc(&r, None);
        assert_eq!(doc["newer"], Value::Null, "{doc}");
        assert_eq!(doc["behind"], json!([]), "{doc}");
        for part in doc["parts"].as_array().unwrap() {
            assert_eq!(part["newer"], Value::Null, "{part}");
        }

        // the same look beside an install that is behind it offers that release, and only that
        let s = state("1.0.0-alpha.83", "1.0.0-alpha.65");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.84"), ("desk", "1.0.0-alpha.65")],
            None,
        );
        let doc = release_doc(&r, None);
        assert_eq!(doc["newer"], "1.0.0-alpha.84", "{doc}");
        assert_eq!(doc["behind"], json!(["engine"]), "{doc}");

        // and a release that is only spelled differently is not newer: the number is, not the text
        let s = state("1.0.0-alpha.9", "1.0.0-alpha.65");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.10"), ("desk", "1.0.0-alpha.65")],
            None,
        );
        assert_eq!(release_doc(&r, None)["newer"], "1.0.0-alpha.10");
        let s = state("1.0.0-alpha.10", "1.0.0-alpha.65");
        let r = rows(
            &s,
            &[("engine", "1.0.0-alpha.9"), ("desk", "1.0.0-alpha.65")],
            None,
        );
        assert_eq!(release_doc(&r, None)["newer"], Value::Null);
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
    fn a_desk_is_taken_by_the_contracts_it_needs_and_never_by_its_number() {
        let v = |list: &[&str]| list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let needs = |openapi| Contracts { openapi, suite: 3 };
        // desk 60 needs HTTP contract 8, desk 58 and every desk before speak 7
        let mut floor = |d: &str| {
            Ok(match d {
                "1.0.0-alpha.60" => Some(needs(8)),
                "1.0.0-alpha.58" => Some(needs(5)),
                _ => None,
            })
        };
        let listed = v(&["1.0.0-alpha.49", "1.0.0-alpha.60", "1.0.0-alpha.58"]);
        assert_eq!(
            newest_that_speaks(&listed, &mut floor, SPEAKS).unwrap(),
            Some("1.0.0-alpha.58".to_string()),
            "the newest the engine speaks, whatever the engine's own number"
        );
        assert_eq!(
            newest_that_speaks(&v(&["1.0.0-alpha.60"]), &mut floor, SPEAKS).unwrap(),
            None
        );
        // a floor that cannot be read stops the walk rather than picking older
        let mut unread = |d: &str| {
            if d == "1.0.0-alpha.60" {
                Err("404".to_string())
            } else {
                Ok(None)
            }
        };
        assert!(newest_that_speaks(&listed, &mut unread, SPEAKS).is_err());

        // an update passes the held newest over for the newest it speaks
        let s = state("1.0.0-alpha.53", "1.0.0-alpha.52");
        let r = part_releases(
            &s,
            &mut |p| {
                Ok(match p {
                    "engine" => "1.0.0-alpha.53",
                    _ => "1.0.0-alpha.60",
                }
                .to_string())
            },
            &mut floor,
            &mut || Ok(v(&["1.0.0-alpha.60", "1.0.0-alpha.58", "1.0.0-alpha.52"])),
            SPEAKS,
        );
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(desk.behind(), "{desk:?}");
        assert_eq!(desk.to_take(), Some("1.0.0-alpha.58"));
        assert!(
            desk.line()
                .starts_with("desk 1.0.0-alpha.52: 1.0.0-alpha.58 is out")
        );
        let doc = release_doc(&r, None);
        assert_eq!(doc["newer"], "desk 1.0.0-alpha.58");
        let d = &doc["parts"][1];
        assert_eq!(d["newer"], "1.0.0-alpha.58");
        assert_eq!(d["held"], Value::Null);
        assert!(d["waits"].as_str().unwrap().contains("HTTP contract 8"));
    }

    /// 2026-11-05: a release tagged and published went unnoticed on the
    /// Parts page for minutes. The watch reads each repository's tags (a git
    /// request, not the API) and asks a newer tag's release for its sums (a
    /// download): there, it is a release; not yet, it is being built.
    #[test]
    fn a_watch_finds_a_tag_released_and_one_still_being_built() {
        let listing = "a\trefs/tags/v1.0.0-beta.9\nb\trefs/tags/v1.0.0-beta.10\n\
                       c\trefs/tags/v1.0.0-beta.11\nd\trefs/tags/pack-mri-v1.4.1\n\
                       e\trefs/tags/pack-mri-v1.4.0\nf\trefs/tags/vendor-x\n";
        let asked = std::cell::RefCell::new(Vec::new());
        let mut out = |v: &str| {
            asked.borrow_mut().push(v.to_string());
            Ok(v != "1.0.0-beta.11")
        };
        let (found, coming) = watch_tags(Some("1.0.0-beta.9"), listing, "v", &mut out).unwrap();
        assert_eq!(found.as_deref(), Some("1.0.0-beta.10"));
        assert_eq!(coming.as_deref(), Some("1.0.0-beta.11"));
        // newest first, and nothing at or below what the look named
        assert_eq!(*asked.borrow(), ["1.0.0-beta.11", "1.0.0-beta.10"]);

        // nothing newer than the look: nothing asked
        let mut none =
            |_: &str| -> Result<bool, String> { panic!("asked a release that is not newer") };
        assert_eq!(
            watch_tags(Some("1.0.0-beta.11"), listing, "v", &mut none).unwrap(),
            (None, None)
        );
        // a pack's own tags, by their prefix
        let mut all = |_: &str| Ok(true);
        assert_eq!(
            watch_tags(Some("1.4.0"), listing, "pack-mri-v", &mut all).unwrap(),
            (Some("1.4.1".to_string()), None)
        );
        // a look that could not be made names no version: the newest released tag is the answer
        assert_eq!(
            watch_tags(None, listing, "v", &mut out).unwrap(),
            (
                Some("1.0.0-beta.10".to_string()),
                Some("1.0.0-beta.11".to_string())
            )
        );
        // a download that fails is said, not taken for a release that is not out
        let mut failing = |_: &str| Err("could not be asked".to_string());
        assert!(watch_tags(Some("1.0.0-beta.9"), listing, "v", &mut failing).is_err());
        assert_eq!(
            tagged_versions(listing, "v"),
            ["1.0.0-beta.11", "1.0.0-beta.10", "1.0.0-beta.9"]
        );
    }

    fn engine_first(
        offered: Option<Contracts>,
        engine_goes: bool,
        list: &[&str],
    ) -> Vec<PartRelease> {
        let s = state("1.0.0-beta.9", "1.0.0-beta.6");
        let listed: Vec<String> = list.iter().map(|v| (*v).to_string()).collect();
        part_releases_with(
            &s,
            &mut |p| {
                Ok(match p {
                    "engine" => "1.0.0-beta.10",
                    _ => "1.0.0-beta.8",
                }
                .to_string())
            },
            // desk beta.8 needs HTTP contract 9, beta.7 needs 8, older ones 7
            &mut |d| {
                Ok(Some(Contracts {
                    openapi: match d {
                        "1.0.0-beta.8" => 9,
                        "1.0.0-beta.7" => 8,
                        _ => 7,
                    },
                    suite: 3,
                }))
            },
            &mut || Ok(listed.clone()),
            SPEAKS,
            &mut |_| Ok(offered),
            engine_goes,
        )
    }

    /// Update everything takes the engine first where a new desk needs its
    /// contract, and says so; a desk that needs more than the new engine
    /// speaks too waits, said with both engines, and takes the newest the
    /// new engine speaks.
    #[test]
    fn a_desk_that_needs_the_new_engines_contract_goes_after_it_and_one_no_engine_speaks_waits() {
        // the engine's release does not say its contracts: the desk goes after it
        let r = engine_first(
            None,
            true,
            &["1.0.0-beta.8", "1.0.0-beta.7", "1.0.0-beta.6"],
        );
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(desk.behind(), "{desk:?}");
        assert_eq!(desk.to_take(), Some("1.0.0-beta.8"));
        assert_eq!(desk.needs.as_deref(), Some("HTTP contract 9"));
        assert_eq!(
            desk.line(),
            "desk 1.0.0-beta.6: 1.0.0-beta.8 is out, after the engine: it needs HTTP contract 9"
        );
        let doc = desk.doc();
        assert_eq!(doc["after"], "engine");
        assert_eq!(doc["command"], "nils update --part engine --part desk");
        let (steps, stays) = order(&r, &[], None);
        let said: Vec<String> = steps.iter().map(Step::line).collect();
        assert_eq!(
            said,
            [
                "engine 1.0.0-beta.9 to 1.0.0-beta.10, first: desk 1.0.0-beta.8 needs its HTTP contract 9",
                "desk 1.0.0-beta.6 to 1.0.0-beta.8, after the engine: it needs HTTP contract 9",
            ]
        );
        assert!(stays.is_empty(), "{stays:?}");

        // the engine's release speaks 8: beta.8 waits, said with both engines, and beta.7 is taken after the engine
        let eight = Contracts {
            openapi: 8,
            suite: 3,
        };
        let r = engine_first(
            Some(eight),
            true,
            &["1.0.0-beta.8", "1.0.0-beta.7", "1.0.0-beta.6"],
        );
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert_eq!(desk.to_take(), Some("1.0.0-beta.7"), "{desk:?}");
        assert_eq!(desk.needs.as_deref(), Some("HTTP contract 8"));
        let held = desk.held.as_deref().unwrap();
        assert!(
            held.contains(
                "needs HTTP contract 9, which neither engine 1.0.0-beta.9 nor 1.0.0-beta.10 speaks"
            ),
            "{held}"
        );
        let (steps, stays) = order(&r, &[], None);
        assert_eq!(steps[0].part, "engine");
        assert_eq!(
            steps[0].why.as_deref(),
            Some("first: desk 1.0.0-beta.7 needs its HTTP contract 8")
        );
        assert_eq!(steps[1].to, "1.0.0-beta.7");
        assert_eq!(stays.len(), 1);
        assert_eq!(stays[0].version, "1.0.0-beta.8");
        assert!(
            stays[0].line().starts_with(
                "desk 1.0.0-beta.8 stays out: desk 1.0.0-beta.8 needs HTTP contract 9"
            )
        );

        // nothing the new engine speaks between: the desk stays where it is, and says why
        let r = engine_first(Some(eight), true, &["1.0.0-beta.8", "1.0.0-beta.6"]);
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(!desk.behind(), "{desk:?}");
        let (steps, stays) = order(&r, &[], None);
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(steps[0].why, None, "the engine alone needs no why");
        assert_eq!(stays[0].part, "desk");

        // the desk alone, the engine left out of the run: it waits, and says what takes both
        let r = engine_first(None, false, &["1.0.0-beta.8", "1.0.0-beta.6"]);
        let desk = r.iter().find(|r| r.part == "desk").unwrap();
        assert!(!desk.behind(), "{desk:?}");
        let held = desk.held.as_deref().unwrap();
        assert!(
            held.contains("and engine 1.0.0-beta.10 is out: nils update --part engine --part desk takes the engine first"),
            "{held}"
        );
    }

    #[test]
    fn the_order_is_the_engine_the_desk_the_parts_from_source_then_the_packs_and_the_rules() {
        let row = |part: &str, installed: &str, newest: &str| PartRelease {
            part: part.to_string(),
            installed: Some(installed.to_string()),
            newest: Some(newest.to_string()),
            error: None,
            follows: None,
            held: None,
            takes: None,
            needs: None,
            coming: None,
        };
        let rows = [
            row("kvasir", "1.0.0-alpha.2", "1.0.0-alpha.3"),
            row("assistant", "1.0.0-alpha.32", "1.0.0-alpha.33"),
            row("desk", "1.0.0-beta.6", "1.0.0-beta.7"),
        ];
        let rules = [crate::rules::Row {
            pack: "mri".to_string(),
            installed: Some("1.4.0".to_string()),
            newest: Some("1.5.0".to_string()),
            held: Some("mri 1.5.0 needs pack contract 11".to_string()),
            takes: Some("1.4.1".to_string()),
            edited: false,
            error: None,
            pinned: None,
        }];
        let (steps, stays) = order(&rows, &rules, Some("1.0.0-beta.9"));
        let parts: Vec<&str> = steps.iter().map(|s| s.part.as_str()).collect();
        assert_eq!(parts, ["desk", "assistant", "kvasir", "packs", "rules"]);
        assert!(steps.iter().all(|s| s.why.is_none()), "{steps:?}");
        assert_eq!(
            steps[3].line(),
            "the engine's packs, to the ones engine 1.0.0-beta.9 was released with"
        );
        assert_eq!(steps[4].line(), "mri rules 1.4.0 to 1.4.1");
        assert_eq!(
            stays[0].line(),
            "mri rules 1.5.0 stays out: mri 1.5.0 needs pack contract 11"
        );
        let lines = order_lines(&steps, &stays);
        assert_eq!(lines[0], "in this order:");
        assert_eq!(lines[1], "  1. desk 1.0.0-beta.6 to 1.0.0-beta.7");
        assert_eq!(
            lines.last().unwrap(),
            "  mri rules 1.5.0 stays out: mri 1.5.0 needs pack contract 11"
        );

        // with the engine: first, and the rules are read against it
        let mut with = rows.to_vec();
        with.push(row("engine", "1.0.0-beta.9", "1.0.0-beta.10"));
        let (steps, _) = order(&with, &rules, None);
        assert_eq!(steps[0].part, "engine");
        assert_eq!(
            steps[0].why.as_deref(),
            Some("first: the parts after it are taken by the new engine")
        );
        assert_eq!(
            steps.last().unwrap().why.as_deref(),
            Some("after the engine: read against engine 1.0.0-beta.10")
        );
        assert!(
            steps.iter().all(|s| s.part != "packs"),
            "the packs go with the engine"
        );
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
