// SPDX-License-Identifier: AGPL-3.0-only
//! Rules releases (record 55 B5, `docs/specs/wave7a-ready-for-production.md`
//! §9.2): a first-party pack released on its own, so a rules fix reaches an
//! install without an engine release.
//!
//! A pack's own release is tagged `pack-<name>-v<version>` in the engine's
//! repository and carries `pack-<name>.tar.gz`, which holds `packs/<name>/`
//! the way the engine's `packs.tar.gz` holds every pack, beside a
//! `SHA256SUMS` naming it and a `pack-<name>.VERSION` holding the version.
//! On GitHub the releases are found in the repository's listing by their
//! tags. A channel of a deployment's own, a development channel among them,
//! lays each release out as GitHub does, under
//! `download/pack-<name>-v<version>/`, and names its newest in
//! `latest/download/pack-<name>.VERSION`; a channel without that file has no
//! rules releases, which is not an error.
//!
//! What a release works with is read from the `pack.yml` it carries: the
//! pack contract, which every engine checks, and from contract 10 the range
//! of engines it names. The newest release this engine reads is the one an
//! update takes; a newer one it would refuse waits and says why, as a desk
//! that needs a newer engine waits. The engine's own release stays the
//! floor ([`crate::packs::plan`]).
use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::packs::{Engine, Offer, Pack, Plan};
use crate::update;

/// The part's name where an update names its parts: `nils update --part
/// rules` takes every first-party pack's own newest release.
pub(crate) const PART: &str = "rules";

/// The tag a pack's own release is published under.
pub(crate) fn tag(pack: &str, version: &str) -> String {
    format!("pack-{pack}-v{version}")
}

/// The tarball a pack's own release carries, holding `packs/<pack>/`.
pub(crate) fn tarball(pack: &str) -> String {
    format!("pack-{pack}.tar.gz")
}

/// The file a release names its version in, and that a channel of a
/// deployment's own names its newest release in, under `latest/download/`.
pub(crate) fn pointer(pack: &str) -> String {
    format!("pack-{pack}.VERSION")
}

/// Where a pack's own releases come from: a channel asked for on the command
/// line, which covers every part as it does for the desk; else
/// `NILS_RULES_RELEASES`; else wherever the engine's releases come from.
pub(crate) fn base(channel: Option<&str>) -> String {
    channel
        .map(str::to_string)
        .or_else(|| std::env::var("NILS_RULES_RELEASES").ok())
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| update::engine_base(None))
}

/// Every release of one pack a base names, newest first: all of them on
/// GitHub, by their tags, and on a channel of a deployment's own the newest,
/// which is all it names; none where it names none.
pub(crate) fn versions(base: &str, pack: &str) -> Result<Vec<String>, String> {
    if let Some(listed) = update::tagged(base, &format!("pack-{pack}-v")) {
        return listed;
    }
    let url = format!("{base}/latest/download/{}", pointer(pack));
    let Some(bytes) = update::fetch_if_there(&url)? else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    if text.is_empty() || text.lines().count() > 1 {
        return Err(format!("{url} does not hold a version on one line"));
    }
    Ok(vec![text.trim_start_matches('v').to_string()])
}

/// One release of a pack, read from its tarball and checked against the
/// release's sums: the pack it carries and nothing else, at the version its
/// tag names.
pub(crate) fn offer(base: &str, pack: &str, version: &str) -> Result<Offer, String> {
    let tag = tag(pack, version);
    let bytes = update::fetch_checked_at(base, &tag, &tarball(pack)).map_err(|e| e.message)?;
    let offer = crate::packs::offers_in(&bytes, &tag)?
        .into_iter()
        .find(|o| o.pack.name == pack)
        .ok_or_else(|| format!("{tag} carries no {pack} pack"))?;
    if offer.pack.version.as_deref() != Some(version) {
        return Err(format!(
            "{tag} carries {}, not {pack} {version}",
            offer.pack.said()
        ));
    }
    Ok(offer)
}

/// What one pack's own releases offer an engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) pack: String,
    /// The newest release the base names.
    pub(crate) newest: Option<String>,
    /// Why the newest waits, where the engine would refuse it.
    pub(crate) held: Option<String>,
    /// The newest release the engine reads, which an update puts in place
    /// where it stands above what is there.
    pub(crate) takes: Option<Offer>,
    /// Why the releases could not be read.
    pub(crate) error: Option<String>,
    /// The version the install pins it to, where it does: only that one is
    /// taken, from its own release, or left to the engine's copy where that
    /// is the version.
    pub(crate) pinned: Option<String>,
}

impl Found {
    /// The same, without the files of the release it takes, for a look that
    /// only compares and is kept for a while.
    pub(crate) fn described(&self) -> Found {
        Found {
            takes: self.takes.as_ref().map(Offer::described),
            ..self.clone()
        }
    }
}

/// One pack's own releases against an engine, newest first: the newest it
/// reads is the one taken, and the walk goes no lower than `floor`, the
/// version the engine's own release carries, below which none would ever be
/// put in place. A release that cannot be read stops the walk rather than
/// being passed over, so a failed download never quietly takes an older one.
pub(crate) fn find(base: &str, pack: &str, engine: &Engine, floor: Option<&str>) -> Found {
    let mut found = Found {
        pack: pack.to_string(),
        ..Found::default()
    };
    let listed = match versions(base, pack) {
        Ok(listed) => listed,
        Err(e) => {
            found.error = Some(e);
            return found;
        }
    };
    found.newest = listed.first().cloned();
    for version in &listed {
        if floor.is_some_and(|f| update::newer(f, version)) {
            break;
        }
        match offer(base, pack, version) {
            Err(e) => {
                found.error = Some(format!("{pack} {version} could not be read: {e}"));
                break;
            }
            Ok(offer) => match engine.refusal(&offer.pack) {
                Some(why) => {
                    found.held.get_or_insert(why);
                }
                None => {
                    found.takes = Some(offer);
                    break;
                }
            },
        }
    }
    found
}

/// A pack pinned to one version (record 55 B5, 2026-10-10): its own
/// release of that version is the one offered, whatever is newer, and the
/// newest is still named so a check can say what the pin holds back. Where
/// the engine's copy is that version (`bundled`), nothing need be fetched.
pub(crate) fn find_pinned(
    base: &str,
    pack: &str,
    engine: &Engine,
    pin: &str,
    bundled: Option<&str>,
) -> Found {
    let mut found = Found {
        pack: pack.to_string(),
        pinned: Some(pin.to_string()),
        ..Found::default()
    };
    match versions(base, pack) {
        Ok(listed) => found.newest = listed.first().cloned(),
        Err(e) => found.error = Some(e),
    }
    if bundled == Some(pin) {
        return found;
    }
    match offer(base, pack, pin) {
        Ok(offer) => match engine.refusal(&offer.pack) {
            Some(why) => {
                found.error = Some(format!(
                    "it is pinned at {pin}, which this engine refuses: {why}"
                ));
            }
            None => {
                found.error = None;
                found.takes = Some(offer);
            }
        },
        Err(e) => {
            found.error = Some(format!(
                "it is pinned at {pin}, whose release could not be read: {e}"
            ));
        }
    }
    found
}

/// Each pack given that states a version, beside its own releases at
/// `base`: the packs the engine's release carries, or where that cannot be
/// read, the first-party packs in place ([`crate::packs::first_party_in`]),
/// each version the floor of its walk. A pack `pins` names is looked for at
/// its pinned version alone ([`find_pinned`]).
pub(crate) fn find_all(
    base: &str,
    packs: &[Pack],
    engine: &Engine,
    pins: &BTreeMap<String, String>,
) -> Vec<Found> {
    packs
        .iter()
        .filter(|p| p.version.is_some())
        .map(|p| match pins.get(&p.name) {
            Some(pin) => find_pinned(base, &p.name, engine, pin, p.version.as_deref()),
            None => find(base, &p.name, engine, p.version.as_deref()),
        })
        .collect()
}

/// The releases an update takes of what was found.
pub(crate) fn takes(found: &[Found]) -> Vec<Offer> {
    found.iter().filter_map(|f| f.takes.clone()).collect()
}

/// One pack beside its own releases, as `nils update --check` says it and
/// the install door hands it to the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) pack: String,
    /// The version in the directory the engine reads.
    pub(crate) installed: Option<String>,
    /// The newest release of its own.
    pub(crate) newest: Option<String>,
    /// Why a newer release waits.
    pub(crate) held: Option<String>,
    /// The release an update puts in place, where it would.
    pub(crate) takes: Option<String>,
    /// Whether a change made on this machine keeps the pack as it is.
    pub(crate) edited: bool,
    pub(crate) error: Option<String>,
    /// The version the install pins it to (record 55 B5).
    pub(crate) pinned: Option<String>,
}

impl Row {
    /// Whether an update would put its own release in place.
    pub(crate) fn behind(&self) -> bool {
        self.takes.is_some()
    }

    /// The line `nils update --check` says for it.
    pub(crate) fn line(&self) -> String {
        let at = self.installed.as_deref().unwrap_or("not installed");
        let name = format!("{PART} {} {at}", self.pack);
        if let Some(e) = &self.error {
            return format!("{name}: its own releases could not be read: {e}");
        }
        let newest = self.newest.as_deref().unwrap_or("a release");
        if let Some(pin) = &self.pinned {
            let lift = format!("`nils update --unpin {}` lifts it", self.pack);
            return match &self.takes {
                Some(t) => format!("{name}: pinned at {pin}; its release of {t} goes in ({lift})"),
                None => match &self.newest {
                    Some(n) if n != pin => {
                        format!("{name}: pinned at {pin}, kept (its newest release is {n}; {lift})")
                    }
                    _ => format!("{name}: pinned at {pin}, kept ({lift})"),
                },
            };
        }
        match (&self.takes, &self.held) {
            (Some(t), Some(why)) => {
                format!("{name}: {t} is out ({newest} is newer and waits: {why})")
            }
            (Some(t), None) if self.installed.as_deref() == Some(t.as_str()) => {
                format!("{name}: its own release of {t} is out, and differs from the one in place")
            }
            (Some(t), None) => format!("{name}: {t} is out"),
            (None, Some(why)) => format!("{name}: {newest} is out and waits: {why}"),
            (None, None) => match &self.newest {
                None => format!("{name}: no release of its own yet"),
                Some(n) if self.edited => {
                    format!("{name}: changed on this machine, kept (its newest release is {n})")
                }
                Some(n) if self.installed.as_deref() == Some(n.as_str()) => {
                    format!("{name}: the newest release")
                }
                Some(n) => format!("{name}: its newest release is {n}"),
            },
        }
    }

    /// What an update says of it where it does not take a release: one
    /// that waits, one that could not be read, or one kept as edited.
    pub(crate) fn update_line(&self) -> Option<String> {
        let said = self.line();
        let shown = self.error.is_some()
            || self.pinned.is_some()
            || (self.held.is_some() && self.takes.is_none())
            || (self.edited && self.newest.is_some());
        shown.then(|| said.trim_start_matches(&format!("{PART} ")).to_string())
    }

    /// The row as the install door hands it to the desk, in the shape of
    /// every other part's, with the pack it is.
    pub(crate) fn doc(&self) -> Value {
        json!({
            "part": PART,
            "pack": self.pack,
            "installed": self.installed,
            "newest": self.newest,
            "newer": self.takes,
            "held": if self.takes.is_some() { None } else { self.held.clone() },
            "takes": self.takes,
            "waits": self.held,
            "follows": Value::Null,
            "edited": self.edited,
            "error": self.error,
            "pinned": self.pinned,
            "command": format!("nils update --part {PART}"),
        })
    }
}

/// Write the pins asked for where the recorded engine reads its packs
/// (record 55 B5, 2026-10-10), and note them in the setup record. A pin
/// names a first-party pack and a version that is there to be put in place:
/// the one in the directory, the pack's own release of it, or the copy the
/// engine's release carries. Nothing is written where one is not.
pub(crate) fn set_pins(
    pin: &[String],
    unpin: &[String],
    channel: Option<&str>,
) -> Result<(), crate::Exit> {
    crate::setup::setup_recorded()?;
    let state = crate::setup::read_state().ok_or_else(|| {
        crate::fail("no setup is recorded on this machine, so there is nothing to pin")
    })?;
    let dir = crate::setup::engine_pack_dir(&state).ok_or_else(|| {
        crate::fail("this install's engine runs in a container, whose image carries its packs: a pin needs the packs a machine install keeps")
    })?;
    let first_party = |pack: &str| -> Result<(), crate::Exit> {
        if crate::setup::FIRST_PARTY_PACKS.contains(&pack) {
            Ok(())
        } else {
            Err(crate::usage(format!(
                "{pack}: a pin names a first-party pack, one of {}",
                crate::setup::FIRST_PARTY_PACKS.join(", ")
            )))
        }
    };
    let mut asked: Vec<(String, String)> = Vec::new();
    for p in pin {
        let Some((pack, version)) = p.split_once('@') else {
            return Err(crate::usage(format!(
                "--pin {p}: a pin is PACK@VERSION, for example mri@1.0.0"
            )));
        };
        let (pack, version) = (pack.trim(), version.trim().trim_start_matches('v'));
        first_party(pack)?;
        nils_pack::Version::parse(version, "--pin").map_err(|e| crate::usage(e.to_string()))?;
        asked.push((pack.to_string(), version.to_string()));
    }
    for pack in unpin {
        first_party(pack.trim())?;
    }
    // every pin checked before any is written
    for (pack, version) in &asked {
        let in_place = crate::packs::on_disk(&dir)
            .iter()
            .any(|p| &p.name == pack && p.version.as_deref() == Some(version.as_str()));
        if in_place || offer(&base(channel), pack, version).is_ok() {
            continue;
        }
        let carried = crate::setup::engine_version(&state)
            .and_then(|release| crate::packs::bundled(&update::engine_base(channel), &release).ok())
            .unwrap_or_default();
        if carried
            .iter()
            .any(|o| &o.pack.name == pack && o.pack.version.as_deref() == Some(version.as_str()))
        {
            continue;
        }
        return Err(crate::fail(format!(
            "{pack} {version} is neither in place nor a release this install can read, so nothing was pinned"
        )));
    }
    for pack in unpin {
        crate::packs::set_pin(&dir, pack.trim(), None).map_err(crate::fail)?;
        println!("{PART} {}: the pin is lifted", pack.trim());
    }
    for (pack, version) in &asked {
        crate::packs::set_pin(&dir, pack, Some(version)).map_err(crate::fail)?;
        println!(
            "{PART} {pack}: pinned at {version}; every update keeps that version until the pin is lifted"
        );
    }
    crate::setup::record_rules(&dir);
    Ok(())
}

/// The rows of what was found, as a plan would leave each pack: what an
/// update takes, and why a newer release waits where it is newer than what
/// is in place.
pub(crate) fn rows(plan: &Plan, found: &[Found]) -> Vec<Row> {
    found
        .iter()
        .map(|f| {
            let installed = plan
                .status
                .installed
                .iter()
                .find(|p| p.name == f.pack)
                .and_then(|p| p.version.clone());
            let takes = plan
                .take
                .iter()
                .find(|o| o.pack.name == f.pack && o.own_release())
                .and_then(|o| o.pack.version.clone());
            let newer = |v: &String| installed.as_deref().is_none_or(|i| update::newer(v, i));
            let held = f
                .held
                .clone()
                .filter(|_| f.newest.as_ref().is_some_and(newer));
            Row {
                pack: f.pack.clone(),
                installed,
                newest: f.newest.clone(),
                held,
                takes,
                edited: plan.status.edited.contains(&f.pack),
                error: f.error.clone(),
                pinned: f.pinned.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::{offers_in, plan};
    use crate::supervise::sha256_hex;
    use std::path::{Path, PathBuf};

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nils-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn tar_of(pack: &str, manifest: &str) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("packs/{pack}/pack.yml"),
            manifest.as_bytes(),
        )
        .unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// A pack's own release on a channel of a deployment's own: its files
    /// and sums under its tag, and the channel's pointer to its newest.
    fn publish(channel: &Path, pack: &str, version: &str, manifest: &str) {
        let at = channel.join("download").join(tag(pack, version));
        std::fs::create_dir_all(&at).unwrap();
        let tar = tar_of(pack, manifest);
        std::fs::write(at.join(tarball(pack)), &tar).unwrap();
        std::fs::write(at.join(pointer(pack)), format!("{version}\n")).unwrap();
        std::fs::write(
            at.join("SHA256SUMS"),
            format!(
                "{}  {}\n{}  {}\n",
                sha256_hex(&tar),
                tarball(pack),
                sha256_hex(format!("{version}\n").as_bytes()),
                pointer(pack)
            ),
        )
        .unwrap();
        let latest = channel.join("latest").join("download");
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::write(latest.join(pointer(pack)), format!("{version}\n")).unwrap();
    }

    #[test]
    fn a_packs_own_releases_are_named_by_their_tag() {
        assert_eq!(tag("mri", "1.0.2"), "pack-mri-v1.0.2");
        assert_eq!(tarball("mri"), "pack-mri.tar.gz");
        assert_eq!(pointer("mri"), "pack-mri.VERSION");
        assert_eq!(base(Some("file:///x/")), "file:///x");
    }

    #[test]
    fn a_channel_without_rules_releases_has_none_and_says_no_error() {
        let root = temp("none");
        let base = format!("file://{}", root.display());
        assert_eq!(versions(&base, "mri"), Ok(Vec::new()));
        let found = find(&base, "mri", &Engine::this(), Some("1.0.1"));
        assert_eq!(found.newest, None);
        assert_eq!(found.error, None);
        assert!(found.takes.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The newest release the engine reads is taken; a newer one it would
    /// refuse, by its contract or by the engines it names, waits and says
    /// why; and a release whose sums do not hold is never taken.
    #[test]
    fn the_newest_release_this_engine_reads_is_taken_and_a_newer_one_waits() {
        let root = temp("find");
        let base = format!("file://{}", root.display());
        let engine = Engine {
            version: "1.0.0-alpha.80".to_string(),
            contract: 10,
        };
        publish(
            &root,
            "mri",
            "1.0.2",
            "pack: mri\nversion: 1.0.2\ncontract: 8\n",
        );
        let found = find(&base, "mri", &engine, Some("1.0.1"));
        assert_eq!(found.newest.as_deref(), Some("1.0.2"));
        assert_eq!(found.error, None);
        let takes = found.takes.clone().expect("a release this engine reads");
        assert_eq!(takes.from, "pack-mri-v1.0.2");
        assert!(takes.own_release() && takes.files.contains_key("pack.yml"));

        // a release that needs a later contract waits
        publish(
            &root,
            "mri",
            "1.0.3",
            "pack: mri\nversion: 1.0.3\ncontract: 11\n",
        );
        let found = find(&base, "mri", &engine, Some("1.0.1"));
        assert!(
            found.takes.is_none(),
            "a channel of its own names only its newest"
        );
        assert_eq!(
            found.held.as_deref(),
            Some("mri 1.0.3 needs pack contract 11, and engine 1.0.0-alpha.80 implements 10")
        );
        // and so does one that names engines this one is not among
        publish(
            &root,
            "mri",
            "1.0.4",
            "pack: mri\nversion: 1.0.4\ncontract: 10\nengine: \">=1.0.0-alpha.90, <2.0.0\"\n",
        );
        let found = find(&base, "mri", &engine, Some("1.0.1"));
        assert!(
            found
                .held
                .as_deref()
                .is_some_and(|h| h.contains("works with engines >=1.0.0-alpha.90, <2.0.0")),
            "{found:?}"
        );
        // which the engine it names reads
        let later = Engine {
            version: "1.0.0-alpha.91".to_string(),
            contract: 10,
        };
        assert!(find(&base, "mri", &later, Some("1.0.1")).takes.is_some());

        // a tarball that is not what the sums say is never taken
        let at = root.join("download").join(tag("mri", "1.0.4"));
        std::fs::write(at.join(tarball("mri")), b"not what the sums say").unwrap();
        let found = find(&base, "mri", &later, Some("1.0.1"));
        assert!(found.takes.is_none());
        assert!(
            found
                .error
                .as_deref()
                .is_some_and(|e| e.contains("checksum")),
            "{found:?}"
        );
        // nor one whose tag and pack disagree
        publish(
            &root,
            "mri",
            "1.0.5",
            "pack: mri\nversion: 1.0.4\ncontract: 8\n",
        );
        let e = offer(&base, "mri", "1.0.5").unwrap_err();
        assert!(e.contains("carries mri 1.0.4, not mri 1.0.5"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The rows say what an update does: the release it takes, the newer one
    /// that waits, and nothing for a release below what is in place.
    #[test]
    fn a_row_says_what_an_update_would_take_and_what_waits() {
        let root = temp("rows");
        let dir = root.join("packs");
        std::fs::create_dir_all(dir.join("mri")).unwrap();
        std::fs::write(
            dir.join("mri/pack.yml"),
            "pack: mri\nversion: 1.0.1\ncontract: 8\n",
        )
        .unwrap();
        let carried = offers_in(
            &tar_of("mri", "pack: mri\nversion: 1.0.1\ncontract: 8\n"),
            "v1.0.0-alpha.80",
        )
        .unwrap();
        let own = offers_in(
            &tar_of("mri", "pack: mri\nversion: 1.0.2\ncontract: 8\n"),
            "pack-mri-v1.0.2",
        )
        .unwrap();
        let found = Found {
            pack: "mri".to_string(),
            newest: Some("1.0.3".to_string()),
            held: Some("mri 1.0.3 needs pack contract 11".to_string()),
            takes: Some(own[0].clone()),
            error: None,
            pinned: None,
        };
        let engine = Engine::this();
        let p = plan(
            &dir,
            "1.0.0-alpha.80",
            carried.clone(),
            takes(std::slice::from_ref(&found)),
            &engine,
        );
        let row = &rows(&p, std::slice::from_ref(&found))[0];
        assert!(row.behind());
        assert_eq!(
            row.line(),
            "rules mri 1.0.1: 1.0.2 is out (1.0.3 is newer and waits: mri 1.0.3 needs pack \
             contract 11)"
        );
        let doc = row.doc();
        assert_eq!(doc["part"], "rules");
        assert_eq!(doc["pack"], "mri");
        assert_eq!(doc["newer"], "1.0.2");
        assert_eq!(doc["held"], Value::Null);
        assert_eq!(doc["command"], "nils update --part rules");
        assert_eq!(
            row.update_line(),
            None,
            "a release taken is said by the packs' line"
        );

        // with nothing it reads, the newer one waits and nothing is taken
        let waits = Found {
            takes: None,
            ..found.clone()
        };
        let p = plan(&dir, "1.0.0-alpha.80", carried.clone(), Vec::new(), &engine);
        let row = &rows(&p, &[waits])[0];
        assert!(!row.behind());
        assert_eq!(
            row.line(),
            "rules mri 1.0.1: 1.0.3 is out and waits: mri 1.0.3 needs pack contract 11"
        );
        assert_eq!(
            row.update_line().as_deref(),
            Some("mri 1.0.1: 1.0.3 is out and waits: mri 1.0.3 needs pack contract 11")
        );
        assert_eq!(row.doc()["held"], "mri 1.0.3 needs pack contract 11");

        // a release no newer than the one in place is not offered
        let same = Found {
            newest: Some("1.0.1".to_string()),
            held: None,
            takes: Some(
                offers_in(
                    &tar_of("mri", "pack: mri\nversion: 1.0.1\ncontract: 8\n"),
                    "pack-mri-v1.0.1",
                )
                .unwrap()[0]
                    .clone(),
            ),
            ..found
        };
        let p = plan(
            &dir,
            "1.0.0-alpha.80",
            carried,
            takes(std::slice::from_ref(&same)),
            &engine,
        );
        let row = &rows(&p, &[same])[0];
        assert!(!row.behind(), "{row:?}");
        assert_eq!(row.line(), "rules mri 1.0.1: the newest release");
        let _ = std::fs::remove_dir_all(&root);
    }
}
