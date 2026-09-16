// SPDX-License-Identifier: AGPL-3.0-only

//! What becomes of a dataset's originals (record 26 §1): the two acts
//! beside `kept`, as one door and one job kind.
//!
//! A dataset declares `originals_kept`, and until this slice that was a
//! word on a row that nothing acted on. Here are the acts. **Vault** moves
//! the originals into a place the engine never reads for data: a rename
//! where the destination is on the same filesystem, and otherwise a copy
//! whose digest is checked against the source's before the source is let
//! go, so no file is ever removed whose copy did not verify. **Purge**
//! deletes them, and is refused unless every original is verified and no
//! file of the dataset waits for a map: what a purge destroys is the only
//! identified copy of those people's scans, and a file that is held has no
//! copy at all.
//!
//! **An original is verified only when it is still the file that was
//! copied** (lab 26c): its size and its modification time are what the row
//! recorded, *and* the copy is what was recorded, there at the recorded
//! size with the digest hashed again now. Where the two disagree the purge
//! refuses, and the refusal names the cure. A file whose bytes changed
//! after its copy was written reads as verified under the copy's half
//! alone, and a purge would then delete the only version of the data
//! nobody has read; the asymmetry decides it, since a false refusal costs
//! a run of the pseudonymiser and a false acceptance destroys data that
//! exists nowhere else. A modification time that moved innocently, after a
//! restore or a copy between disks, is refused too, and that is the price.
//!
//! **A purge proves each original by its content, in one read of the file
//! taken at the moment it removes it** (lab 26d). Two things were wrong
//! with judging it any other way. The walk lists a directory and then
//! reaches its files one by one, so the size and the modification time it
//! captured may be many files old by the time a file's turn comes, and an
//! rsync, a re-export or a corrected study copied over the old one while
//! the purge runs was destroyed on that stale reading. And the copy was
//! proved honestly, by a digest hashed on the spot, while the original was
//! proved by two numbers anything may set, so an original changed in place
//! whose modification time was then put back passed as verified. So the
//! pseudonymiser records the digest of the original it read beside the
//! digest of the copy it wrote, and a purge, immediately before it removes
//! a file, opens that file, measures it, hashes every byte of it and judges
//! *that*: one read serves both. A row with no such digest, written before
//! this, cannot be proved at all, and the refusal says so and names the run
//! that mends it. A purge therefore reads every original once, whole,
//! because it proves what it destroys, and that is the point of it.
//!
//! The survey and the door stay cheap, on the rows and on each original's
//! size and modification time, and say in [`FORECAST`] that they are a
//! forecast, since what the purge proves it proves file by file as it goes.
//!
//! The counts a person reads keep the problems apart, because their cures
//! differ: a file whose **copy** is missing or is no longer what was
//! recorded, a file whose **original moved on** since it was copied, and a
//! file the reader **refused**, which has no copy at all and never will.
//!
//! Both acts are jobs of kind `originals`, resumable because a file that
//! has moved is simply not there the next time and cancellable at a
//! heartbeat like any other job, and both are audited (`originals.vault`,
//! `originals.purge`) with their counts and the reason the person gave,
//! which moves the epoch.
//!
//! Neither act touches the pseudonymised tree, the registry's rows or the
//! linkage store: a person keeps their code and their history whatever
//! becomes of the originals. What an act changes is the dataset's
//! `originals_kept`, set when the job succeeds and `kept` until then, and
//! `originals_vault`, the place a vault put them in.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use blake2::{Blake2s256, Digest};
use nils_digest::Cancel;
use nils_digest::resume::relative;
use nils_registry::place::{self, Place, Role};
use nils_registry::schema::{Type, table};
use nils_registry::store::{Error as StoreError, Param, Store};
use nils_registry::{Registry, audit, job};
use serde_json::{Value, json};

use crate::dataset::Refused;

/// The job kind both acts claim.
pub(crate) const KIND: &str = "originals";

/// The role a vault writes to: `backup` is protected storage elsewhere
/// from the registry, and the one role the engine never reads for data,
/// which is what the originals need once they leave the dataset.
pub(crate) const VAULT_ROLE: Role = Role::Backup;

/// Where under that place a dataset's originals go, so that the engine's
/// archives and a dataset's originals never share a folder.
pub(crate) const VAULT_UNDER: &str = "originals";

/// Files between heartbeats, which is also how often a cancel is seen.
const BEAT_EVERY: u64 = 128;

/// What the survey and the door are, said in the answer itself (lab 26d).
/// A person who reads `ready` must not then meet a refusal with nothing to
/// explain it: the counts are cheap and the proof is not, and this sentence
/// is the difference between the two, wherever the survey is answered.
pub(crate) const FORECAST: &str = "these counts are a forecast, read from the rows and from each original's size and modification time; a purge reads every original again as it reaches it and proves it by its content, so a file that changes in between is left as it is and the job names it";

/// What a copy and a digest read through.
const BUF: usize = 256 * 1024;

/// The two acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    Vault,
    Purge,
}

impl Act {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Act::Vault => "vault",
            Act::Purge => "purge",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Act> {
        match text {
            "vault" => Some(Act::Vault),
            "purge" => Some(Act::Purge),
            _ => None,
        }
    }

    /// What the dataset carries once the act has run.
    fn kept(self) -> &'static str {
        match self {
            Act::Vault => "vaulted",
            Act::Purge => "purged",
        }
    }

    fn action(self) -> audit::Action {
        match self {
            Act::Vault => audit::Action::OriginalsVault,
            Act::Purge => audit::Action::OriginalsPurge,
        }
    }
}

fn bad(message: impl Into<String>) -> Refused {
    Refused {
        status: 400,
        message: message.into(),
    }
}

fn conflict(message: impl Into<String>) -> Refused {
    Refused {
        status: 409,
        message: message.into(),
    }
}

fn store_failed(e: StoreError) -> Refused {
    Refused {
        status: 500,
        message: e.to_string(),
    }
}

/// `n things`, singular where there is one.
fn many(n: u64, word: &str) -> String {
    format!(
        "{} {word}{}",
        nils_digest::report::thousands(n),
        if n == 1 { "" } else { "s" }
    )
}

/// The verb that agrees with a count, so a refusal reads as a sentence
/// whether it is about one file or a thousand.
fn are(n: u64) -> &'static str {
    if n == 1 { "is" } else { "are" }
}

/// The object pronoun that agrees with a count.
fn them(n: u64) -> &'static str {
    if n == 1 { "it" } else { "them" }
}

/// The subject pronoun with its verb, so a sentence about one file and a
/// sentence about many read alike.
fn they_have(n: u64) -> &'static str {
    if n == 1 { "it has" } else { "they have" }
}

/// The dataset a door names by the place's id: a source place in force.
pub(crate) fn dataset_at(store: &mut Store, id: i64) -> Result<Place, Refused> {
    match place::show(store, id).map_err(store_failed)? {
        Some(p) if p.role == Role::Source && p.retired_at.is_none() => Ok(p),
        Some(p) if p.role != Role::Source => Err(bad(format!(
            "the originals are a dataset's; {} is a {} place",
            p.name,
            p.role.name()
        ))),
        Some(p) => Err(conflict(format!("the dataset {} is retired", p.name))),
        None => Err(Refused {
            status: 404,
            message: format!("no place {id}"),
        }),
    }
}

// ------------------------------------------------------------- the survey

/// What a dataset's originals hold, and whether a purge may run over them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Survey {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) verified: u64,
    /// Every original that is not verified, whatever is wrong with it: the
    /// three counts below say which, since their cures differ.
    pub(crate) unverified: u64,
    /// The copy is missing, or is no longer what the pseudonymiser
    /// recorded.
    pub(crate) copy_unverified: u64,
    /// The original moved on: its size or its modification time is not
    /// what the row recorded, so the copy was made from other bytes.
    pub(crate) changed: u64,
    /// The row says nothing about what the original hashed to when it was
    /// copied (lab 26d), so nothing proves this file is that one and a
    /// purge will not destroy it on a row's word alone.
    pub(crate) unproved: u64,
    /// The reader refused the original, so it has no copy at all.
    pub(crate) no_copy: u64,
    pub(crate) held: u64,
    /// The refusal's words, when a purge may not run.
    pub(crate) why: Option<String>,
}

impl Survey {
    pub(crate) fn ready(&self) -> bool {
        self.why.is_none()
    }

    pub(crate) fn as_json(&self) -> Value {
        json!({
            "files": self.files,
            "bytes": self.bytes,
            "verified": self.verified,
            "unverified": self.unverified,
            "copy_unverified": self.copy_unverified,
            "changed": self.changed,
            "unproved": self.unproved,
            "no_copy": self.no_copy,
            "held": self.held,
            "ready": self.ready(),
            "why": self.why,
            "forecast": FORECAST,
        })
    }
}

/// What the act would do, without doing it, as a forecast ([`FORECAST`]).
/// Every copy is read and hashed, which is what says a copy still stands,
/// and every original is read as the walk reads it, by its size and its
/// modification time, which is what says cheaply that it has not moved on.
/// What this cannot say is whether an original whose numbers stand is still
/// the file that was copied: that is the purge's own reading, file by file,
/// of every byte (lab 26d). So `ready` means a purge may begin, not that
/// every file will prove itself when it is reached, and the answer says so.
pub(crate) fn survey(registry: &mut Registry, place: &Place) -> Result<Survey, StoreError> {
    let mut survey = Survey {
        held: held_files(registry, place)?,
        ..Survey::default()
    };
    let Some(originals) = place.tree_path("originals") else {
        survey.why = Some(read_in_place(place));
        return Ok(survey);
    };
    let anon = written_tree(place);
    let recorded = nils_registry::migrate::table_exists(registry.store(), "pseudonym_file")?;
    let place_id = place.id;
    let mut failed: Option<StoreError> = None;
    each_directory(&originals, |dir, files| {
        let rows = if recorded {
            match recorded_in(registry.store(), place_id, dir) {
                Ok(rows) => rows,
                Err(e) => {
                    failed = Some(e);
                    return Flow::Stop;
                }
            }
        } else {
            HashMap::new()
        };
        for (_, rel, size, mtime) in files {
            survey.files += 1;
            survey.bytes += size;
            match standing(anon.as_deref(), rows.get(rel.as_str()), *size, *mtime) {
                Standing::Verified => survey.verified += 1,
                Standing::Changed => {
                    survey.unverified += 1;
                    survey.changed += 1;
                }
                Standing::Unproved => {
                    survey.unverified += 1;
                    survey.unproved += 1;
                }
                Standing::NoCopy => {
                    survey.unverified += 1;
                    survey.no_copy += 1;
                }
                Standing::CopyUnverified => {
                    survey.unverified += 1;
                    survey.copy_unverified += 1;
                }
            }
        }
        Flow::Go
    });
    if let Some(e) = failed {
        return Err(e);
    }
    survey.why = purge_refusal(place, &survey);
    Ok(survey)
}

/// The pseudonymised tree a purge verifies against: none where the dataset
/// reads its own folder, which is a tree with nothing to verify against.
fn written_tree(place: &Place) -> Option<PathBuf> {
    let doc = place::dataset_of(&place.dataset, None).ok()?;
    match doc["trees"]["anon"].as_str() {
        Some(".") | None => None,
        Some(_) => place.tree_path("anon"),
    }
}

/// What the dataset says became of its originals already, and the place a
/// vault put them in.
fn already(place: &Place) -> (String, Option<String>) {
    let doc = place::dataset_of(&place.dataset, None).unwrap_or_else(|_| json!({}));
    (
        doc["originals_kept"].as_str().unwrap_or("kept").to_string(),
        doc["originals_vault"].as_str().map(str::to_string),
    )
}

fn read_in_place(place: &Place) -> String {
    format!(
        "the dataset {} is read in place: its folder is its pseudonymised tree and it has no originals, so there is nothing to vault or purge",
        place.name
    )
}

/// What refuses either act, whatever it is: a dataset with no originals,
/// and one whose originals an act has already taken away.
fn nothing_to_act_on(place: &Place, holds_a_file: bool) -> Option<String> {
    if place.tree_path("originals").is_none() {
        return Some(read_in_place(place));
    }
    if holds_a_file {
        return None;
    }
    let (kept, vault) = already(place);
    match kept.as_str() {
        "vaulted" => Some(format!(
            "the originals of the dataset {} are vaulted into {} and its originals tree holds no file; there is nothing to vault or purge",
            place.name,
            vault.as_deref().unwrap_or("another place")
        )),
        "purged" => Some(format!(
            "the originals of the dataset {} are purged and its originals tree holds no file; there is nothing to vault or purge",
            place.name
        )),
        _ => None,
    }
}

/// What refuses a purge, in the order a person should hear it.
fn purge_refusal(place: &Place, survey: &Survey) -> Option<String> {
    if let Some(why) = nothing_to_act_on(place, survey.files > 0) {
        return Some(why);
    }
    if written_tree(place).is_none() {
        return Some(format!(
            "the pseudonymised tree of the dataset {} is the folder itself, so there is nothing to verify the originals against; a purge is refused",
            place.name
        ));
    }
    // one sentence per problem, in the order a person should hear them:
    // what waits for a map, what has no copy at all, what moved on since it
    // was copied, and what was copied and no longer verifies. Each names
    // the cure that works for it, and each is counted on its own, because
    // a person told only that something is unverified cannot tell which
    // they have (lab 26c).
    let mut why: Vec<String> = Vec::new();
    if survey.held > 0 {
        why.push(held_words(place, survey.held));
    }
    if survey.no_copy > 0 {
        why.push(no_copy_words(place, survey.no_copy));
    }
    if survey.changed > 0 {
        why.push(changed_words(place, survey.changed));
    }
    if survey.unproved > 0 {
        why.push(unproved_words(place, survey.unproved));
    }
    if survey.copy_unverified > 0 {
        why.push(copy_words(place, survey.copy_unverified));
    }
    (!why.is_empty()).then(|| why.join(" "))
}

fn held_words(place: &Place, held: u64) -> String {
    format!(
        "{} of the dataset {} {} held for want of a map, and the originals of a held file are the only copy of that person's scans, so a purge is refused. File a map that names the identifiers, or code them anyway, then look once more.",
        many(held, "file"),
        place.name,
        are(held)
    )
}

/// The words a purge is refused with when the reader refused an original
/// (lab 26c, finding 6): a file that is not DICOM is never copied, so a
/// purge would lose it and no run of the pseudonymiser can help. Nothing
/// offers to delete it: the engine deletes no file it never read.
fn no_copy_words(place: &Place, n: u64) -> String {
    format!(
        "{} of the dataset {} {} not DICOM and the reader refused {}, so {} no copy in the pseudonymised tree and a purge would lose {}; a purge is refused. Move {} out of the originals tree, to where such files are kept, then look once more.",
        many(n, "file"),
        place.name,
        are(n),
        them(n),
        they_have(n),
        them(n),
        them(n)
    )
}

/// The words a purge is refused with when an original moved on since its
/// copy was written (lab 26c, finding 1): the tree holds the older bytes,
/// so removing the original would destroy what is newer. The cure is a run
/// of the pseudonymiser, which writes a changed original again.
fn changed_words(place: &Place, n: u64) -> String {
    format!(
        "{} of the dataset {} changed after being copied, at the size or the modification time the pseudonymiser recorded, so the pseudonymised tree holds the older bytes and a purge would destroy what is newer; a purge is refused. Pseudonymise the dataset again with nils pseudonymize @{}, which writes a changed original again, then look once more.",
        many(n, "file"),
        place.name,
        place.name
    )
}

/// The words a purge is refused with when the row says nothing about what
/// the original hashed to (lab 26d, finding 2): a row the pseudonymiser
/// wrote before it recorded that digest. Such a file cannot be proved by
/// anything now, and a purge destroys nothing it cannot prove; the run the
/// refusal names reads the original again and records what it is, after
/// which the file can be proved and the purge may run.
fn unproved_words(place: &Place, n: u64) -> String {
    format!(
        "{} of the dataset {} {} copied before the pseudonymiser recorded what the original hashed to, so nothing proves {} still the {} that {} copied and a purge is refused: a purge proves every file by its content before it destroys it. Pseudonymise the dataset again with nils pseudonymize @{}, which reads each original and records its digest, then look once more.",
        many(n, "file"),
        place.name,
        if n == 1 { "was" } else { "were" },
        if n == 1 { "it is" } else { "they are" },
        if n == 1 { "file" } else { "files" },
        if n == 1 { "was" } else { "were" },
        place.name
    )
}

/// The words a purge is refused with when a copy is missing or is no
/// longer what was recorded. The pseudonymiser writes such a copy again
/// (lab 26c, finding 3), so the advice is true for this case too.
fn copy_words(place: &Place, n: u64) -> String {
    format!(
        "{} of the dataset {} {} not verified in its pseudonymised tree, so a purge is refused: every original must have its copy there at the size and the digest that were recorded. Pseudonymise the dataset again with nils pseudonymize @{}, which writes the copy again where it is missing or is no longer what was recorded, then look once more.",
        many(n, "file"),
        place.name,
        are(n),
        place.name
    )
}

/// The files of a dataset that wait for a map: the rows the pseudonymiser
/// holds and no map has released, and the rows a digest of a dataset read
/// in place quarantined under `identity.unmapped`.
fn held_files(registry: &mut Registry, place: &Place) -> Result<u64, StoreError> {
    let store = registry.store();
    let rows_there = nils_registry::migrate::table_exists(store, "pseudonym_file")?;
    let pseudonym = store.qualified("pseudonym_file");
    let released = store.dialect().text_of(
        table("pseudonym_file")
            .column("released_at")
            .expect("pseudonym_file.released_at"),
    );
    let mut held = 0i64;
    if rows_there {
        let sql = format!(
            "SELECT COUNT(*) FROM {pseudonym} WHERE place_id = {} AND state = 'held' AND {released} IS NULL",
            store.dialect().param(1, Type::Int)
        );
        held += store.query(&sql, &[Param::Int(place.id)])?[0].int(0)?;
    }
    let sources = source_ids(store, place)?;
    if !sources.is_empty() {
        let ids = sources
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let source_file = store.qualified("source_file");
        let d = store.dialect();
        // lab 26c, finding 3: a digest of a dataset read in place records
        // the file it holds in both tables, so a quarantined row counts
        // only where no held row counts that file already. One file waits
        // once, whichever verb held it, and the count a person reads in the
        // refusal is the count the sources door and the held door answer.
        let once = if rows_there {
            format!(
                " AND NOT EXISTS (SELECT 1 FROM {pseudonym} WHERE {pseudonym}.place_id = {} AND {pseudonym}.path = {source_file}.path AND {pseudonym}.state = 'held' AND {released} IS NULL)",
                d.param(2, Type::Int)
            )
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT COUNT(*) FROM {source_file} WHERE source_id IN ({ids}) AND status = 'quarantined' AND reason = {}{once}",
            d.param(1, Type::Text)
        );
        let mut params = vec![Param::from(nils_registry::review::UNMAPPED_KIND)];
        if rows_there {
            params.push(Param::Int(place.id));
        }
        held += store.query(&sql, &params)?[0].int(0)?;
    }
    Ok(held.max(0) as u64)
}

/// The source rows whose tree lies under the place: what a digest of this
/// dataset filed its files against.
fn source_ids(store: &mut Store, place: &Place) -> Result<Vec<i64>, StoreError> {
    let sql = format!(
        "SELECT id, root_canonical FROM {}",
        store.qualified("source")
    );
    let mut ids = Vec::new();
    for r in &store.query(&sql, &[])? {
        if place.holds_path(Path::new(r.text(1)?)) {
            ids.push(r.int(0)?);
        }
    }
    Ok(ids)
}

/// One `pseudonym_file` row, as the check reads it.
#[derive(Debug, Clone)]
struct Recorded {
    state: String,
    /// The size and the modification time of the original as the run that
    /// wrote the copy read them: what the original must still be for that
    /// copy to be a copy of it (lab 26c).
    size: i64,
    mtime: i64,
    out_path: Option<String>,
    out_size: Option<i64>,
    digest: Option<String>,
    /// What the original hashed to when the run that wrote the copy read
    /// it (lab 26d): what a purge proves the file against before it
    /// removes it. None on a row written before it was recorded, which is
    /// a file no purge may destroy until a run records it.
    original_digest: Option<String>,
}

/// What the pseudonymiser recorded for the files of one directory of the
/// originals, by their path relative to the originals tree, read one
/// directory at a time on the index the resume stage reads.
fn recorded_in(
    store: &mut Store,
    place_id: i64,
    dir: &str,
) -> Result<HashMap<String, Recorded>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT path, state, size, mtime, out_path, out_size, digest, original_digest FROM {} WHERE place_id = {} AND dir = {}",
        store.qualified("pseudonym_file"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    let rows = store.query(&sql, &[Param::Int(place_id), Param::from(dir)])?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in &rows {
        out.insert(
            r.text(0)?.to_string(),
            Recorded {
                state: r.text(1)?.to_string(),
                size: r.int(2)?,
                mtime: r.int(3)?,
                out_path: r.opt_text(4)?.map(str::to_string),
                out_size: r.opt_int(5)?,
                digest: r.opt_text(6)?.map(str::to_string),
                original_digest: r.opt_text(7)?.map(str::to_string),
            },
        );
    }
    Ok(out)
}

/// What one original stands as when the row the pseudonymiser left, the
/// copy in the pseudonymised tree and the original itself are read
/// together (lab 26c). The three ways of not being verified are kept apart
/// because their cures are different.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Still the file that was copied, and the copy is what was recorded:
    /// a purge may remove it.
    Verified,
    /// The reader refused it, so it has no copy and no run will make one.
    NoCopy,
    /// It moved on since it was copied: the copy is of other bytes.
    Changed,
    /// Nothing says what the original hashed to when it was copied, so it
    /// cannot be proved to be that file (lab 26d).
    Unproved,
    /// The copy is missing, or is no longer what was recorded.
    CopyUnverified,
}

impl Standing {
    /// Why a purge left the file, as the job's first reason names it.
    fn why(self) -> &'static str {
        match self {
            Standing::Verified => "is verified",
            Standing::NoCopy => {
                "was refused by the reader and has no copy in the pseudonymised tree"
            }
            Standing::Changed => "changed after its copy was written",
            Standing::Unproved => {
                "was copied before the pseudonymiser recorded what the original hashed to, so nothing proves it is that file"
            }
            Standing::CopyUnverified => "has no verified copy in the pseudonymised tree",
        }
    }
}

/// Where one original stands in the forecast: the row says a copy was
/// written; the original has not moved on, at the size and the modification
/// time the row recorded; the row says what the original hashed to, which a
/// purge will prove it against; the copy is there at the recorded size; and
/// its digest now is the digest that was recorded. A row alone proves
/// nothing, which is why the copy is read here and the original is read by
/// the purge (lab 26d).
fn standing(anon: Option<&Path>, recorded: Option<&Recorded>, size: u64, mtime: i64) -> Standing {
    // no tree to verify against, or no row at all: never copied
    let (Some(anon), Some(r)) = (anon, recorded) else {
        return Standing::CopyUnverified;
    };
    if r.state == "refused" {
        return Standing::NoCopy;
    }
    if !matches!(r.state.as_str(), "written" | "unchanged") {
        return Standing::CopyUnverified;
    }
    // the original first: a file whose bytes changed after its copy was
    // written is the one case where a purge destroys what exists nowhere
    // else, and the copy's own half cannot see it at all
    if r.size != size as i64 || r.mtime != mtime {
        return Standing::Changed;
    }
    // lab 26d: a row from before the original's digest was recorded is
    // counted here rather than found at the last moment by the purge, so
    // that a person is told what to do about it before they ask for one
    if r.original_digest.is_none() {
        return Standing::Unproved;
    }
    if copy_stands(anon, r) {
        Standing::Verified
    } else {
        Standing::CopyUnverified
    }
}

/// One original read again, whole, now: the size and the modification time
/// the file system answers for the very handle that is hashed, and the
/// digest of every byte of it. This is the one read a purge takes of a file
/// before it removes it, and it serves both halves of the judgment (lab
/// 26d): what the file is now, and whether it is what was copied.
///
/// The file is measured before the hash and again after it, because a write
/// landing while it is being read would otherwise be hashed into a file
/// that never existed on disk. Where the two readings differ the file is
/// moving under the purge and the purge leaves it, saying so.
fn read_now(path: &Path) -> Result<(u64, i64, String), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("could not be read again: {e}"))?;
    let before = file
        .metadata()
        .map_err(|e| format!("could not be read again: {e}"))?;
    let mut hasher = Blake2s256::new();
    let mut buf = vec![0u8; BUF];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) => return Err(format!("could not be read again: {e}")),
        }
    }
    let after = file
        .metadata()
        .map_err(|e| format!("could not be read again: {e}"))?;
    let (was, is) = (
        nils_digest::walk::mtime_ns_of(&before),
        nils_digest::walk::mtime_ns_of(&after),
    );
    if before.len() != after.len() || was != is {
        return Err("was written to while the purge was reading it".to_string());
    }
    Ok((after.len(), is, hex::encode(hasher.finalize())))
}

/// Where one original stands when it is read again at the moment it would
/// be removed (lab 26d): judged on what the file is then, never on what the
/// walk's listing said, since a directory is listed once and its files are
/// reached one by one. The size and the modification time are asked first,
/// because they name the commoner case in words a person can act on, but
/// what proves the file is the digest of its content against the digest the
/// run that copied it recorded.
fn standing_now(
    anon: &Path,
    recorded: Option<&Recorded>,
    size: u64,
    mtime: i64,
    digest: &str,
) -> Standing {
    let Some(r) = recorded else {
        return Standing::CopyUnverified;
    };
    if r.state == "refused" {
        return Standing::NoCopy;
    }
    if !matches!(r.state.as_str(), "written" | "unchanged") {
        return Standing::CopyUnverified;
    }
    if r.size != size as i64 || r.mtime != mtime {
        return Standing::Changed;
    }
    match r.original_digest.as_deref() {
        None => Standing::Unproved,
        Some(recorded) if recorded != digest => Standing::Changed,
        _ if copy_stands(anon, r) => Standing::Verified,
        _ => Standing::CopyUnverified,
    }
}

/// Whether the copy of an original stands in the pseudonymised tree as it
/// was written: it is there, it is the size that was recorded, and its
/// digest now is the digest that was recorded, hashed again here.
fn copy_stands(anon: &Path, recorded: &Recorded) -> bool {
    let (Some(out), Some(size), Some(digest)) = (
        recorded.out_path.as_deref(),
        recorded.out_size,
        recorded.digest.as_deref(),
    ) else {
        return false;
    };
    let path = anon.join(out);
    match std::fs::metadata(&path) {
        Ok(m) if m.len() as i64 == size => {}
        _ => return false,
    }
    digest_of(&path).is_some_and(|d| d == digest)
}

/// The digest of a file as the pseudonymiser hashes what it writes.
fn digest_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Blake2s256::new();
    let mut buf = vec![0u8; BUF];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return None,
        }
    }
    Some(hex::encode(hasher.finalize()))
}

// -------------------------------------------------------------- the walk

enum Flow {
    Go,
    Stop,
}

/// Every directory under `root` with the files in it: the path, the path
/// relative to `root` as `pseudonym_file` records it, the size and the
/// modification time as they were **when the directory was listed**. Links
/// and special files are left out, as the pseudonymiser leaves them out.
/// One directory is held at a time, never the tree.
///
/// Those two numbers are a listing, not a judgment (lab 26d, finding 1):
/// by the time a file's turn comes they may be many files old, and a purge
/// reads the file again rather than believe them. The survey counts by them
/// and says it is a forecast; a vault takes a file's size from them for its
/// tally, which no file is destroyed on.
fn each_directory(root: &Path, mut f: impl FnMut(&str, &[(PathBuf, String, u64, i64)]) -> Flow) {
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files = Vec::new();
        let mut here = String::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                queue.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let (size, mtime) = match entry.metadata() {
                Ok(m) => (m.len(), nils_digest::walk::mtime_ns_of(&m)),
                Err(_) => (0, 0),
            };
            let (rel, in_dir) = relative(root, &path);
            here = in_dir;
            files.push((path, rel, size, mtime));
        }
        if files.is_empty() {
            continue;
        }
        if matches!(f(&here, &files), Flow::Stop) {
            break;
        }
    }
}

/// Whether a tree holds any file at all, which is all an act needs to know
/// before it reads a row.
fn holds_a_file(root: &Path) -> bool {
    let mut found = false;
    each_directory(root, |_, _| {
        found = true;
        Flow::Stop
    });
    found
}

/// The directories an act emptied, removed from the bottom up; the tree's
/// own root stays, so the dataset keeps the layout it was declared with.
fn prune_empty(root: &Path) {
    fn walk(dir: &Path, root: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|k| k.is_dir()) {
                walk(&entry.path(), root);
            }
        }
        if dir != root && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
            let _ = std::fs::remove_dir(dir);
        }
    }
    walk(root, root);
}

// ------------------------------------------------------------- the moving

/// How one file moves: a rename where the destination is on the same
/// filesystem, and a copy otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum How {
    Rename,
    Copy,
}

/// Whether the two paths are on one filesystem, so that a move is a
/// rename. A device the system will not say is read as another one, which
/// makes the answer the careful one.
pub(crate) fn how(from: &Path, to: &Path) -> How {
    match (device_of(from), device_of(to)) {
        (Some(a), Some(b)) if a == b => How::Rename,
        _ => How::Copy,
    }
}

#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

/// One file moved into the vault. Answers whether the move verified a
/// copy by its digest, which a rename never does because it copies
/// nothing; a file that could not be moved answers why, and stays.
pub(crate) fn move_file(from: &Path, to: &Path, how: How) -> Result<bool, String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot make {}: {e}", parent.display()))?;
    }
    if to.exists() {
        // an earlier run copied the file and did not get to let the source
        // go: the same file under the same name is that copy, and the
        // source may go now; another file is not written over
        let (Some(source), Some(there)) = (digest_of(from), digest_of(to)) else {
            return Err(format!(
                "{} is already there and could not be read to compare",
                to.display()
            ));
        };
        if source != there {
            return Err(format!(
                "{} is already there and is another file",
                to.display()
            ));
        }
        std::fs::remove_file(from).map_err(|e| format!("cannot remove {}: {e}", from.display()))?;
        return Ok(true);
    }
    if how == How::Rename {
        // a tree may span filesystems whatever its root says, so a rename
        // that cannot be one becomes the copy
        if std::fs::rename(from, to).is_ok() {
            return Ok(false);
        }
    }
    copy_verify_remove(from, to)
}

/// The copy that verifies before it removes (record 26 §1): the source is
/// read and hashed as it is written to a `.part` beside the destination,
/// the part is renamed into place, the copy is read back and hashed, and
/// only a copy whose digest is the source's lets the source go.
pub(crate) fn copy_verify_remove(from: &Path, to: &Path) -> Result<bool, String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot make {}: {e}", parent.display()))?;
    }
    let part = PathBuf::from(format!("{}.part", to.display()));
    let source = copy_into(from, &part).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("cannot copy {} to {}: {e}", from.display(), to.display())
    })?;
    std::fs::rename(&part, to).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("cannot put {} in place: {e}", to.display())
    })?;
    let copied = digest_of(to).ok_or_else(|| {
        format!(
            "the copy of {} could not be read back to verify it",
            from.display()
        )
    })?;
    if copied != source {
        let _ = std::fs::remove_file(to);
        return Err(format!(
            "the copy of {} did not verify, so the original stays",
            from.display()
        ));
    }
    std::fs::remove_file(from).map_err(|e| format!("cannot remove {}: {e}", from.display()))?;
    Ok(true)
}

/// Copy one file, hashing the source as it goes; answers the source's
/// digest.
fn copy_into(from: &Path, to: &Path) -> std::io::Result<String> {
    let mut source = std::fs::File::open(from)?;
    let mut target = std::io::BufWriter::with_capacity(BUF, std::fs::File::create(to)?);
    let mut hasher = Blake2s256::new();
    let mut buf = vec![0u8; BUF];
    loop {
        let n = source.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        target.write_all(&buf[..n])?;
    }
    target.flush()?;
    target.into_inner()?.sync_all()?;
    Ok(hex::encode(hasher.finalize()))
}

// --------------------------------------------------------------- the acts

/// Where a vault puts a dataset's originals.
#[derive(Debug, Clone)]
pub(crate) struct Destination {
    pub(crate) place: String,
    pub(crate) path: PathBuf,
}

/// What a door can refuse without reading every file: a dataset with no
/// originals, one an act has already taken the originals from, a purge of
/// a tree there is nothing to verify against or of a dataset holding files
/// for want of a map, and a vault whose destination is not a place the
/// engine keeps what it never reads. What only a walk can tell, that every
/// original is verified, the job asks again when it runs.
pub(crate) fn check(
    registry: &mut Registry,
    place: &Place,
    act: Act,
    into: Option<&str>,
) -> Result<Option<Destination>, Refused> {
    let originals = place.tree_path("originals");
    if let Some(why) = nothing_to_act_on(place, originals.as_deref().is_some_and(holds_a_file)) {
        return Err(conflict(why));
    }
    if act == Act::Purge {
        if into.is_some() {
            return Err(bad(
                "into names the place a vault moves the originals to; a purge moves them nowhere",
            ));
        }
        if written_tree(place).is_none() {
            return Err(conflict(format!(
                "the pseudonymised tree of the dataset {} is the folder itself, so there is nothing to verify the originals against; a purge is refused",
                place.name
            )));
        }
        // lab 26c: the forecast here as well, so that a person who asks for
        // a purge meets the refusal in words at the door, in the sentence
        // GET answered with, rather than as a job that failed. What it can
        // see it refuses here: a file waiting for a map, one the reader
        // refused, one whose copy no longer stands, one whose numbers moved
        // and one no row can prove. What only the file itself can say, the
        // purge asks of each file as it reaches it (lab 26d).
        let surveyed = survey(registry, place).map_err(store_failed)?;
        if let Some(why) = surveyed.why {
            return Err(conflict(why));
        }
        return Ok(None);
    }
    let name = into.map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| {
        bad(format!(
            "into: the name of the {} place a vault moves the originals to, which is the role the engine never reads for data",
            VAULT_ROLE.name()
        ))
    })?;
    let target = place::by_name(registry.store(), name)
        .map_err(store_failed)?
        .ok_or_else(|| {
            bad(format!(
                "no place is named {name}; nils place list shows the places"
            ))
        })?;
    if target.retired_at.is_some() {
        return Err(conflict(format!(
            "the originals go to a place in force; {name} is retired"
        )));
    }
    if target.id == place.id {
        return Err(conflict(format!(
            "the originals of the dataset {} cannot be vaulted into the dataset itself",
            place.name
        )));
    }
    if target.role != VAULT_ROLE {
        return Err(conflict(format!(
            "the originals go to a {} place, which the engine never reads for data; {name} is a {} place",
            VAULT_ROLE.name(),
            target.role.name()
        )));
    }
    let path = Path::new(&target.path).join(VAULT_UNDER).join(&place.name);
    crate::places::require(registry.store(), VAULT_ROLE, &path).map_err(|r| conflict(r.message))?;
    Ok(Some(Destination {
        place: target.name,
        path,
    }))
}

/// What the act did, as the job's result records it.
#[derive(Debug, Clone)]
pub(crate) struct Done {
    pub(crate) did: &'static str,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) into: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) verified: u64,
    pub(crate) seconds: f64,
    pub(crate) cancelled: bool,
}

impl Done {
    pub(crate) fn as_json(&self) -> Value {
        let mut doc = json!({
            "did": self.did,
            "files": self.files,
            "bytes": self.bytes,
            "verified": self.verified,
            "seconds": (self.seconds * 1000.0).round() / 1000.0,
        });
        if let Some(into) = &self.into {
            doc["into"] = json!(into);
            doc["path"] = json!(self.path);
        }
        if self.cancelled {
            doc["cancelled"] = json!(true);
        }
        doc
    }
}

/// What a person reads before an act: what the door answers, in words.
pub(crate) fn as_text(place: &Place, survey: &Survey) -> String {
    use nils_digest::report::{human_bytes, thousands};
    let (kept, vault) = already(place);
    let mut out = format!("originals of {}   {kept}", place.name);
    if let Some(v) = vault {
        out.push_str(&format!(" into {v}"));
    }
    out.push('\n');
    out.push_str(&format!(
        "  files            {}   {}\n",
        thousands(survey.files),
        human_bytes(survey.bytes)
    ));
    out.push_str(&format!(
        "  verified         {} of {} in the pseudonymised tree\n",
        thousands(survey.verified),
        thousands(survey.files)
    ));
    out.push_str(&format!(
        "  copy unverified  {} whose copy is missing or is not what was recorded\n",
        thousands(survey.copy_unverified)
    ));
    out.push_str(&format!(
        "  changed          {} that changed after being copied\n",
        thousands(survey.changed)
    ));
    out.push_str(&format!(
        "  unproved         {} copied before the original's own digest was recorded\n",
        thousands(survey.unproved)
    ));
    out.push_str(&format!(
        "  no copy          {} the reader refused, which are never copied\n",
        thousands(survey.no_copy)
    ));
    out.push_str(&format!(
        "  held             {} for want of a map\n",
        thousands(survey.held)
    ));
    match &survey.why {
        None => out.push_str("  purge            may run\n"),
        Some(why) => out.push_str(&format!("  purge            is refused: {why}\n")),
    }
    // lab 26d: a person told `may run` is told in the same breath what this
    // reading is and what the purge will do that it has not done
    out.push_str(&format!("\n{FORECAST}\n"));
    out
}

/// What a person reads after one.
pub(crate) fn done_text(place: &Place, done: &Done) -> String {
    use nils_digest::report::{human_bytes, human_secs};
    let went = match (&done.into, &done.path) {
        (Some(into), Some(path)) => format!(" into {into} at {path}"),
        _ => String::new(),
    };
    format!(
        "{} {} of the dataset {} ({}){went} in {}{}\n",
        if done.did == "vault" {
            "vaulted"
        } else {
            "purged"
        },
        many(done.files, "file"),
        place.name,
        human_bytes(done.bytes),
        human_secs(done.seconds),
        if done.cancelled {
            ", stopped before the end"
        } else {
            ""
        }
    )
}

/// What one pass counted.
#[derive(Debug, Default)]
struct Tally {
    files: u64,
    bytes: u64,
    verified: u64,
    /// Files an act left as they are, and the first reason it did.
    left: u64,
    first: Option<String>,
    cancelled: bool,
}

/// Act on a dataset's originals. The refusals are checked first and
/// nothing is claimed until they pass, so a refusal is words rather than a
/// failed job; a purge verifies every original again here, however its
/// rows read, since what it destroys is the only identified copy.
pub(crate) fn run(
    registry: &mut Registry,
    place: &Place,
    act: Act,
    into: Option<&str>,
    why: &str,
    principal: &str,
    cancel: &Cancel,
) -> Result<Done, Refused> {
    let why = why.trim();
    if why.is_empty() {
        return Err(bad("why: a sentence saying why, which the audit row keeps"));
    }
    // a purge is verified inside `check`, which walks and hashes: the
    // refusal is words and no job is claimed for it
    let destination = check(registry, place, act, into)?;
    let originals = place
        .tree_path("originals")
        .ok_or_else(|| conflict(read_in_place(place)))?;
    let job_id = claim(registry, place, act, destination.as_ref(), why)?;
    let started = Instant::now();
    let outcome = match act {
        Act::Vault => {
            let to = &destination.as_ref().expect("a vault names a place").path;
            vault(registry, &originals, to, job_id, cancel)
        }
        Act::Purge => purge(registry, place, &originals, job_id, cancel),
    };
    let tally = match outcome {
        Ok(t) => t,
        Err(e) => {
            let _ = job::finish(
                registry.store(),
                job_id,
                job::State::Failed,
                Some(&e.message),
            );
            return Err(e);
        }
    };
    if !tally.cancelled {
        prune_empty(&originals);
    }
    let done = Done {
        did: act.name(),
        files: tally.files,
        bytes: tally.bytes,
        into: destination.as_ref().map(|d| d.place.clone()),
        path: destination.as_ref().map(|d| d.path.display().to_string()),
        verified: tally.verified,
        seconds: started.elapsed().as_secs_f64(),
        cancelled: tally.cancelled,
    };
    // a file left behind is a refusal, not a run that ended well: the
    // dataset keeps the state it had and the job says why
    if tally.left > 0 {
        let message = match act {
            Act::Vault => format!(
                "{} of the dataset {} could not be moved, the first because {}; no file is removed whose copy did not verify",
                many(tally.left, "file"),
                place.name,
                tally.first.as_deref().unwrap_or("of an error")
            ),
            Act::Purge => format!(
                "{} under the originals of the dataset {} could not be proved when the purge read {} again and {}, the first because {}; a purge reads every original as it reaches it and removes nothing that does not prove itself at that moment. Pseudonymise the dataset again with nils pseudonymize @{}, then look once more.",
                many(tally.left, "file"),
                place.name,
                them(tally.left),
                if tally.left == 1 {
                    "was left as it is"
                } else {
                    "were left as they are"
                },
                tally
                    .first
                    .as_deref()
                    .unwrap_or("its copy could not be verified"),
                place.name
            ),
        };
        let _ = job::set_result(registry.store(), job_id, &done.as_json());
        let _ = job::finish(registry.store(), job_id, job::State::Failed, Some(&message));
        return Err(conflict(message));
    }
    // record 26 §1: the dataset records what became of its originals only
    // when the act ran to the end; the state is `kept` until then
    if !tally.cancelled {
        // the dataset keeps the name of the place its originals went to, so
        // that a page can say where they are; the path and the hour are the
        // audit row's and the job's result
        let vault = destination.as_ref().map(|d| d.place.as_str());
        place::set_originals(registry.store(), place.id, act.kept(), vault)
            .map_err(|e| store_failed(StoreError::Message(e.to_string())))?;
        audit::record(
            registry,
            &audit::Entry {
                principal,
                action: act.action(),
                scope: json!({
                    "place": place.id,
                    "dataset": place.name,
                    "into": done.into,
                    "path": done.path,
                }),
                policy: None,
                job_id: Some(job_id),
                details: Some(json!({
                    "files": done.files,
                    "bytes": done.bytes,
                    "verified": done.verified,
                    "why": why,
                })),
            },
        )
        .map_err(store_failed)?;
    }
    job::set_result(registry.store(), job_id, &done.as_json())
        .map_err(|e| conflict(e.to_string()))?;
    let (state, error) = if tally.cancelled {
        (
            job::State::Cancelled,
            Some(
                "stopped: what was moved stays moved and what was deleted stays deleted; run it again to go on",
            ),
        )
    } else {
        (job::State::Done, None)
    };
    job::finish(registry.store(), job_id, state, error).map_err(|e| conflict(e.to_string()))?;
    Ok(done)
}

fn claim(
    registry: &mut Registry,
    place: &Place,
    act: Act,
    destination: Option<&Destination>,
    why: &str,
) -> Result<i64, Refused> {
    let args = json!({
        "dataset": place.name,
        "place_id": place.id,
        "do": act.name(),
        "into": destination.map(|d| d.place.clone()),
        "path": destination.map(|d| d.path.display().to_string()),
        "why": why,
        "version": env!("CARGO_PKG_VERSION"),
    });
    match job::claim(
        registry.store(),
        &job::Claim {
            kind: KIND,
            name: &place.name,
            args,
        },
    ) {
        Ok(id) => Ok(id),
        Err(e @ job::Error::Busy { .. }) => Err(conflict(e.to_string())),
        Err(e) => Err(store_failed(StoreError::Message(e.to_string()))),
    }
}

/// The vault: every original moved into the destination under the path it
/// had, by a rename where the filesystem allows one and by a verified copy
/// otherwise.
fn vault(
    registry: &mut Registry,
    originals: &Path,
    to: &Path,
    job_id: i64,
    cancel: &Cancel,
) -> Result<Tally, Refused> {
    std::fs::create_dir_all(to)
        .map_err(|e| conflict(format!("cannot make {}: {e}", to.display())))?;
    let moving = how(originals, to);
    let mut tally = Tally::default();
    let mut since = 0u64;
    each_directory(originals, |_, files| {
        for (path, rel, size, _) in files {
            // a stop is seen before the next file is touched, however few
            // files there are between heartbeats
            if cancel.stop() {
                tally.cancelled = true;
                return Flow::Stop;
            }
            match move_file(path, &to.join(rel), moving) {
                Ok(checked) => {
                    tally.files += 1;
                    tally.bytes += size;
                    if checked {
                        tally.verified += 1;
                    }
                }
                Err(why) => {
                    tally.left += 1;
                    tally.first.get_or_insert(why);
                }
            }
            since += 1;
            if since >= BEAT_EVERY {
                since = 0;
                if beaten(registry, job_id, "vault", &tally) {
                    tally.cancelled = true;
                    return Flow::Stop;
                }
            }
        }
        Flow::Go
    });
    Ok(tally)
}

/// The seam the tests write through (lab 26d, finding 1): called for each
/// file a purge is about to judge, after that file's directory has been
/// listed and its rows read, and before the file is read again. That is the
/// window an rsync, a re-export or a corrected study writes in, and it is a
/// property of one file, not of a corpus: a test with two files stands in
/// it exactly, where the lab needed 19,200 files and two runs to catch the
/// same thing by weight of numbers. It is compiled into the test build
/// alone, so a real run pays nothing for it.
#[cfg(test)]
type UnderTheWalk = Option<Box<dyn FnMut(&Path) + Send>>;

#[cfg(test)]
static UNDER_THE_WALK: std::sync::Mutex<UnderTheWalk> = std::sync::Mutex::new(None);

#[cfg(test)]
fn under_the_walk(path: &Path) {
    let mut seam = UNDER_THE_WALK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(write) = seam.as_mut() {
        write(path);
    }
}

#[cfg(not(test))]
#[inline]
fn under_the_walk(_path: &Path) {}

/// The purge: the originals deleted, each proved once more immediately
/// before it goes, by a reading of the file itself and not of the listing
/// (lab 26d, finding 1) and by the digest of its content and not by two
/// numbers anything may set (finding 2). One read of the original serves
/// both, and the copy is hashed beside it, so a file that appeared or
/// changed under the originals while the run went on is left rather than
/// destroyed, and the job names the first that was.
fn purge(
    registry: &mut Registry,
    place: &Place,
    originals: &Path,
    job_id: i64,
    cancel: &Cancel,
) -> Result<Tally, Refused> {
    let anon = written_tree(place).ok_or_else(|| {
        conflict(format!(
            "the pseudonymised tree of the dataset {} is the folder itself, so there is nothing to verify the originals against; a purge is refused",
            place.name
        ))
    })?;
    let place_id = place.id;
    let mut tally = Tally::default();
    let mut since = 0u64;
    let mut failed: Option<StoreError> = None;
    each_directory(originals, |dir, files| {
        let rows = match recorded_in(registry.store(), place_id, dir) {
            Ok(rows) => rows,
            Err(e) => {
                failed = Some(e);
                return Flow::Stop;
            }
        };
        for (path, rel, _, _) in files {
            if cancel.stop() {
                tally.cancelled = true;
                return Flow::Stop;
            }
            under_the_walk(path);
            // lab 26c and lab 26d: the whole judgment again, on this file,
            // now. The survey hashed every copy a moment ago and the walk
            // listed this directory some files ago; neither says what the
            // file is at the moment it would be destroyed. So the original
            // is read here, once, for its size, its modification time and
            // the digest of every byte of it, and judged on that reading
            // against the row. A file that appeared under the originals
            // while the run went on has no row and no copy at all.
            match read_now(path) {
                Ok((size, mtime, digest)) => {
                    match standing_now(&anon, rows.get(rel.as_str()), size, mtime, &digest) {
                        Standing::Verified => match std::fs::remove_file(path) {
                            Ok(()) => {
                                tally.files += 1;
                                tally.bytes += size;
                                tally.verified += 1;
                            }
                            Err(e) => {
                                tally.left += 1;
                                tally.first.get_or_insert(format!(
                                    "{} could not be removed: {e}",
                                    path.display()
                                ));
                            }
                        },
                        left => {
                            tally.left += 1;
                            tally.first.get_or_insert(format!("{rel} {}", left.why()));
                        }
                    }
                }
                Err(why) => {
                    tally.left += 1;
                    tally.first.get_or_insert(format!("{rel} {why}"));
                }
            }
            since += 1;
            if since >= BEAT_EVERY {
                since = 0;
                if beaten(registry, job_id, "purge", &tally) {
                    tally.cancelled = true;
                    return Flow::Stop;
                }
            }
        }
        Flow::Go
    });
    match failed {
        Some(e) => Err(store_failed(e)),
        None => Ok(tally),
    }
}

/// The heartbeat with the progress on it; true when someone asked the job
/// to stop.
fn beaten(registry: &mut Registry, job_id: i64, did: &str, tally: &Tally) -> bool {
    let progress = json!({
        "do": did,
        "files": tally.files,
        "bytes": tally.bytes,
        "verified": tally.verified,
        "left": tally.left,
    });
    matches!(
        job::beat(registry.store(), job_id, Some(&progress)),
        Ok(job::Asked::Cancel)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grants::Detail;
    use nils_dicom::synth::TempDir;
    use nils_registry::home::{Home, InitOptions};
    use nils_registry::job::State;

    /// Two originals, and the copies the pseudonymiser would have written.
    const ORIGINALS: [(&str, &[u8]); 2] = [
        ("a/1.dcm", b"the first original"),
        ("a/2.dcm", b"the second original"),
    ];

    const WHO: &str = "anna@lab";

    fn lab(dir: &TempDir) -> Registry {
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        home.init(&InitOptions {
            backend: nils_registry::Backend::Sqlite,
            dsn: None,
            schema: None,
            scheme: nils_registry::pseudonym::Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
            session_scheme: None,
        })
        .unwrap()
    }

    /// One row as the pseudonymiser leaves it.
    #[allow(clippy::too_many_arguments)]
    fn record(
        registry: &mut Registry,
        place_id: i64,
        rel: &str,
        size: u64,
        mtime: i64,
        out: &str,
        digest: &str,
        original_digest: Option<&str>,
        state: &str,
    ) {
        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        registry
            .store()
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, dir, size, mtime, state, out_path, out_size, digest, original_digest, first_seen, code_anyway) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '2026-09-16T00:00:00Z', 0)",
                &[
                    Param::Int(place_id),
                    Param::from(rel),
                    Param::from(dir),
                    Param::Int(size as i64),
                    Param::Int(mtime),
                    Param::from(state),
                    Param::from(out),
                    Param::Int(size as i64),
                    Param::from(digest),
                    match original_digest {
                        Some(d) => Param::from(d),
                        None => Param::Null,
                    },
                ],
            )
            .unwrap();
    }

    /// The modification time of a file as the walk reads it: what the row
    /// must still say for the original to be the file that was copied.
    fn mtime_of(path: &Path) -> i64 {
        nils_digest::walk::mtime_ns_of(&std::fs::metadata(path).unwrap())
    }

    /// One file's bytes changed in place, keeping its length, with the
    /// modification time moved on as a disk moves it (lab 26c).
    fn change_in_place(path: &Path, bytes: &[u8]) {
        let was = std::fs::metadata(path).unwrap();
        let moved = was.modified().unwrap() + std::time::Duration::from_secs(1);
        assert_eq!(was.len() as usize, bytes.len(), "the same length");
        std::fs::write(path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(moved))
            .unwrap();
    }

    /// A dataset with its originals, its pseudonymised tree and the rows
    /// that say where each original's copy is.
    fn dataset(registry: &mut Registry, dir: &TempDir, name: &str) -> Place {
        let root = dir.path().join(name);
        let id = place::add(
            registry.store(),
            &place::New {
                name,
                role: Role::Source,
                path: &root.display().to_string(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: json!({
                    "arrives": "identified",
                    "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
                }),
            },
        )
        .unwrap();
        for (i, (rel, bytes)) in ORIGINALS.iter().enumerate() {
            let original = dir.file(&format!("{name}/derivatives/dcm-original/{rel}"), bytes);
            let out = format!("x/001/{:05}.dcm", i + 1);
            let copy = dir.file(&format!("{name}/derivatives/dcm-anon/{out}"), bytes);
            let digest = digest_of(&copy).unwrap();
            // the copy holds the original's own bytes here, and the two
            // digests are recorded apart all the same: one proves the copy
            // and the other proves the original (lab 26d)
            let original_digest = digest_of(&original).unwrap();
            record(
                registry,
                id,
                rel,
                bytes.len() as u64,
                mtime_of(&original),
                &out,
                &digest,
                Some(&original_digest),
                "written",
            );
        }
        place::show(registry.store(), id).unwrap().unwrap()
    }

    /// The seam set for one test and cleared when it ends, so that a panic
    /// never leaves one standing for another test of this binary.
    struct Seam;

    impl Drop for Seam {
        fn drop(&mut self) {
            *UNDER_THE_WALK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
    }

    /// Write `bytes` into `target` the first time a purge reaches a file
    /// under `root`: the moment that file's directory has been listed and
    /// nothing under it has been read again yet. That is where an rsync
    /// refreshing files in place lands, and the walk's reading of the size
    /// and the modification time is stale from then on.
    fn write_under_the_walk(root: PathBuf, target: PathBuf, bytes: &'static [u8]) -> Seam {
        let mut written = false;
        *UNDER_THE_WALK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(move |reached| {
            if written || !reached.starts_with(&root) {
                return;
            }
            written = true;
            std::fs::write(&target, bytes).expect("write under the walk");
        }));
        Seam
    }

    /// One file's modification time put back to what it was, as a tool that
    /// restores content and timestamps out of step leaves it.
    fn put_back(path: &Path, mtime: i64) {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(mtime as u64);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    }

    /// A backup place to vault into, on the same filesystem as the dataset.
    fn backup(registry: &mut Registry, dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        place::add(
            registry.store(),
            &place::New {
                name,
                role: Role::Backup,
                path: &path.display().to_string(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: Value::Null,
            },
        )
        .unwrap();
        path
    }

    fn acts(registry: &mut Registry, action: &str) -> Vec<nils_registry::audit::Row> {
        nils_registry::audit::list(
            registry.store(),
            &nils_registry::audit::Filter {
                action: Some(action.to_string()),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn the_job(registry: &mut Registry) -> nils_registry::job::Job {
        job::list(registry.store(), true, 10)
            .unwrap()
            .into_iter()
            .find(|j| j.kind == KIND)
            .expect("an originals job")
    }

    fn rows_kept(registry: &mut Registry) -> i64 {
        registry
            .store()
            .query("SELECT COUNT(*) FROM pseudonym_file", &[])
            .unwrap()[0]
            .int(0)
            .unwrap()
    }

    #[cfg(unix)]
    fn inode(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).unwrap().ino()
    }

    #[test]
    fn a_vault_on_one_filesystem_renames_and_the_dataset_records_the_place() {
        let dir = TempDir::new("originals-vault");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let archive = backup(&mut registry, &dir, "archive");
        let originals = ds.tree_path("originals").unwrap();
        #[cfg(unix)]
        let was = inode(&originals.join("a/1.dcm"));

        let done = run(
            &mut registry,
            &ds,
            Act::Vault,
            Some("archive"),
            "the study is over and the originals leave the working disk",
            WHO,
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!((done.did, done.files, done.bytes), ("vault", 2, 37));
        // a rename copies nothing, so it verifies nothing
        assert_eq!(done.verified, 0);
        assert_eq!(done.into.as_deref(), Some("archive"));

        // the originals left the dataset and are under the place, whole
        assert!(!originals.join("a/1.dcm").exists());
        assert!(originals.is_dir(), "the tree itself stays");
        assert!(!originals.join("a").exists(), "the emptied folders go");
        let moved = archive.join("originals/north/a/1.dcm");
        assert_eq!(std::fs::read(&moved).unwrap(), ORIGINALS[0].1);
        #[cfg(unix)]
        assert_eq!(inode(&moved), was, "the same file, renamed and not copied");

        // the pseudonymised tree and the registry's rows are untouched
        let anon = ds.tree_path("anon").unwrap();
        assert!(anon.join("x/001/00001.dcm").is_file());
        assert_eq!(rows_kept(&mut registry), 2);

        // the dataset says what became of them, and where they went
        let after = place::show(registry.store(), ds.id).unwrap().unwrap();
        assert_eq!(after.dataset["originals_kept"], "vaulted");
        assert_eq!(after.dataset["originals_vault"], "archive");

        let rows = acts(&mut registry, "originals.vault");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope["dataset"], "north");
        assert_eq!(rows[0].scope["into"], "archive");
        let details = rows[0].details.clone().unwrap();
        assert_eq!(details["files"], 2);
        assert!(
            details["why"]
                .as_str()
                .is_some_and(|w| w.starts_with("the study is over")),
            "{details}"
        );
        assert!(rows[0].epoch.is_some(), "the act moves the epoch");

        let job = the_job(&mut registry);
        assert_eq!(job.state, State::Done);
        assert_eq!(job.result.clone().unwrap()["did"], "vault");
        assert_eq!(job.result.unwrap()["into"], "archive");

        // asked again, there is nothing left to act on, and it says so
        let vaulted = place::show(registry.store(), ds.id).unwrap().unwrap();
        let again = run(
            &mut registry,
            &vaulted,
            Act::Vault,
            Some("archive"),
            "again",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(
            again.message.contains("are vaulted into archive"),
            "{}",
            again.message
        );
    }

    #[test]
    fn a_vault_across_filesystems_copies_verifies_and_only_then_removes() {
        let dir = TempDir::new("originals-copy");
        let from = dir.file("from/1.dcm", b"a file that crosses a filesystem");
        let to = dir.path().join("to/1.dcm");
        assert!(copy_verify_remove(&from, &to).unwrap(), "the copy verified");
        assert!(!from.exists(), "and only then was the source let go");
        assert_eq!(
            std::fs::read(&to).unwrap(),
            b"a file that crosses a filesystem"
        );
        assert!(
            !dir.path().join("to/1.dcm.part").exists(),
            "the part is renamed into place"
        );

        // a run that stopped between the copy and the removal: the file at
        // the destination is that copy, so the source may go now
        let again = dir.file("from/1.dcm", b"a file that crosses a filesystem");
        assert!(move_file(&again, &to, How::Copy).unwrap());
        assert!(!again.exists());

        // another file of that name is never written over, and the original
        // stays where it is
        let other = dir.file("from/2.dcm", b"another file");
        let taken = dir.path().join("to/2.dcm");
        std::fs::write(&taken, b"something else").unwrap();
        let why = move_file(&other, &taken, How::Copy).unwrap_err();
        assert!(why.contains("is another file"), "{why}");
        assert!(other.exists(), "the original stays");
        assert_eq!(std::fs::read(&taken).unwrap(), b"something else");

        // one filesystem is a rename; a destination the system will not
        // answer for is treated as another filesystem, which is the
        // careful answer
        assert_eq!(how(dir.path(), dir.path()), How::Rename);
        assert_eq!(how(dir.path(), Path::new("/nowhere-at-all")), How::Copy);
    }

    #[test]
    fn a_purge_runs_once_every_original_is_verified() {
        let dir = TempDir::new("originals-purge");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(
            (
                surveyed.files,
                surveyed.verified,
                surveyed.unverified,
                surveyed.held
            ),
            (2, 2, 0, 0)
        );
        assert!(surveyed.ready() && surveyed.why.is_none());
        assert_eq!(surveyed.as_json()["ready"], true);

        let done = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed and the tree stands",
            WHO,
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!((done.did, done.files, done.verified), ("purge", 2, 2));
        assert!(done.into.is_none());

        let originals = ds.tree_path("originals").unwrap();
        assert!(originals.is_dir() && !originals.join("a").exists());
        // the pseudonymised tree, the rows and the linkage store are untouched
        let anon = ds.tree_path("anon").unwrap();
        assert!(anon.join("x/001/00001.dcm").is_file());
        assert!(anon.join("x/001/00002.dcm").is_file());
        assert_eq!(rows_kept(&mut registry), 2);

        let after = place::show(registry.store(), ds.id).unwrap().unwrap();
        assert_eq!(after.dataset["originals_kept"], "purged");
        assert_eq!(after.dataset["originals_vault"], Value::Null);

        let rows = acts(&mut registry, "originals.purge");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].details.clone().unwrap()["files"], 2);
        assert!(rows[0].epoch.is_some());
        let job = the_job(&mut registry);
        assert_eq!(job.state, State::Done);
        assert_eq!(job.result.unwrap()["did"], "purge");
    }

    #[test]
    fn a_purge_is_refused_while_a_file_of_the_dataset_is_held() {
        let dir = TempDir::new("originals-held");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        // the pseudonymiser holds one file for want of a map
        registry
            .store()
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, dir, size, mtime, state, shape, first_seen, code_anyway) \
                 VALUES (?, 'a/3.dcm', 'a', 1, 0, 'held', '999999999999', '2026-09-16T00:00:00Z', 0)",
                &[Param::Int(ds.id)],
            )
            .unwrap();

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(surveyed.held, 1);
        let why = surveyed.why.clone().unwrap();
        assert!(
            why.starts_with("1 file of the dataset north is held for want of a map"),
            "{why}"
        );
        assert!(
            why.contains("the only copy of that person's scans"),
            "{why}"
        );

        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.message, why);
        // nothing was deleted, and no job was claimed for a refusal
        let originals = ds.tree_path("originals").unwrap();
        assert!(originals.join("a/1.dcm").is_file());
        assert!(job::list(registry.store(), true, 10).unwrap().is_empty());
        // a vault is not refused for it: it keeps every file
        assert!(check(&mut registry, &ds, Act::Vault, Some("archive")).is_err());
        backup(&mut registry, &dir, "archive");
        assert!(check(&mut registry, &ds, Act::Vault, Some("archive")).is_ok());

        // the files a digest of a dataset read in place holds count the
        // same way: a quarantined row under identity.unmapped
        let anon = ds.tree_path("anon").unwrap();
        let canonical = std::fs::canonicalize(&anon).unwrap().display().to_string();
        registry
            .store()
            .execute(
                "INSERT INTO source (root, root_canonical, first_seen_at) VALUES (?, ?, '2026-09-16T00:00:00Z')",
                &[Param::from(canonical.as_str()), Param::from(canonical.as_str())],
            )
            .unwrap();
        let source = registry
            .store()
            .query("SELECT id FROM source", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        registry
            .store()
            .execute(
                "INSERT INTO source_file (source_id, batch_id, dir, path, size, mtime_ns, status, reason, seen_at) \
                 VALUES (?, 1, 'a', 'a/9.dcm', 1, 1, 'quarantined', ?, '2026-09-16T00:00:00Z')",
                &[
                    Param::Int(source),
                    Param::from(nils_registry::review::UNMAPPED_KIND),
                ],
            )
            .unwrap();
        assert_eq!(survey(&mut registry, &ds).unwrap().held, 2);

        // lab 26c, finding 3: a digest of a dataset read in place records
        // the file it holds in both tables, and one file waits once. The
        // quarantined row below is the held row above, the same file.
        registry
            .store()
            .execute(
                "INSERT INTO source_file (source_id, batch_id, dir, path, size, mtime_ns, status, reason, seen_at) \
                 VALUES (?, 1, 'a', 'a/3.dcm', 1, 1, 'quarantined', ?, '2026-09-16T00:00:00Z')",
                &[
                    Param::Int(source),
                    Param::from(nils_registry::review::UNMAPPED_KIND),
                ],
            )
            .unwrap();
        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(
            surveyed.held, 2,
            "the file held in both tables waits once, not twice"
        );
        assert!(
            surveyed
                .why
                .as_deref()
                .is_some_and(|w| w.starts_with("2 files of the dataset north are held")),
            "the count a person reads is the count of files: {:?}",
            surveyed.why
        );
    }

    #[test]
    fn a_purge_is_refused_when_a_copy_is_no_longer_what_was_recorded() {
        let dir = TempDir::new("originals-digest");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        // the copy in the tree is changed after it was written, to another
        // text of exactly its length: only the digest can tell
        let anon = ds.tree_path("anon").unwrap();
        let copy = anon.join("x/001/00001.dcm");
        std::fs::write(&copy, b"the FIRST original").unwrap();
        assert_eq!(
            std::fs::metadata(&copy).unwrap().len() as usize,
            ORIGINALS[0].1.len()
        );

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!((surveyed.verified, surveyed.unverified), (1, 1));
        let why = surveyed.why.clone().unwrap();
        assert!(
            why.starts_with(
                "1 file of the dataset north is not verified in its pseudonymised tree"
            ),
            "{why}"
        );

        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.message, why);
        let originals = ds.tree_path("originals").unwrap();
        assert!(originals.join("a/1.dcm").is_file());
        assert!(originals.join("a/2.dcm").is_file());
        assert!(job::list(registry.store(), true, 10).unwrap().is_empty());

        // a copy that is gone is not verified either
        std::fs::remove_file(&copy).unwrap();
        assert_eq!(survey(&mut registry, &ds).unwrap().verified, 1);
    }

    /// Lab 26c, finding 1, as the lab wrote it: one original changed in
    /// place to other bytes of exactly its length, without pseudonymising
    /// again. Its copy still verifies on its own, so the check that asked
    /// only about the copy said a purge could run, and the purge then
    /// deleted the original and left the older copy: the changed bytes
    /// were gone from the machine. An original is verified only while it
    /// is still the file that was copied, so the survey says what
    /// happened, the act refuses in those words, and every original stays.
    #[test]
    fn a_purge_is_refused_when_an_original_changed_after_its_copy_was_written() {
        const CHANGED: &[u8] = b"THE FIRST ORIGINAL";
        let dir = TempDir::new("originals-changed");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let originals = ds.tree_path("originals").unwrap();
        let changed = originals.join("a/1.dcm");
        assert!(
            survey(&mut registry, &ds).unwrap().ready(),
            "before the change a purge may run"
        );

        change_in_place(&changed, CHANGED);

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(
            (
                surveyed.files,
                surveyed.verified,
                surveyed.unverified,
                surveyed.changed,
                surveyed.copy_unverified
            ),
            (2, 1, 1, 1, 0),
            "the file whose bytes moved on is counted as changed and not as a broken copy"
        );
        assert!(!surveyed.ready());
        assert_eq!(surveyed.as_json()["changed"], 1);
        let why = surveyed.why.clone().unwrap();
        assert!(
            why.starts_with("1 file of the dataset north changed after being copied"),
            "{why}"
        );
        assert!(why.contains("would destroy what is newer"), "{why}");
        // the cure the refusal names is the one that works
        assert!(why.contains("nils pseudonymize @north"), "{why}");

        // the act refuses in the same words, and claims no job for it
        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.message, why);
        assert!(job::list(registry.store(), true, 10).unwrap().is_empty());

        // every original is still on disk, the changed bytes among them
        assert_eq!(std::fs::read(&changed).unwrap(), CHANGED);
        assert!(originals.join("a/2.dcm").is_file());
        // a vault is not refused for it: it keeps every file it moves
        backup(&mut registry, &dir, "archive");
        assert!(check(&mut registry, &ds, Act::Vault, Some("archive")).is_ok());
    }

    /// Lab 26c, finding 6: an original the reader refused has no copy in
    /// the pseudonymised tree and no run will make one, so a purge stays
    /// refused. It is counted apart from the rest and said as its own
    /// sentence, with what a person can do about it; nothing offers to
    /// delete it.
    #[test]
    fn an_original_the_reader_refused_is_counted_and_said_on_its_own() {
        let dir = TempDir::new("originals-refused");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let stray = dir.file(
            "north/derivatives/dcm-original/a/NOTES.txt",
            b"a file that is not DICOM",
        );
        registry
            .store()
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, dir, size, mtime, state, first_seen, code_anyway) \
                 VALUES (?, 'a/NOTES.txt', 'a', ?, ?, 'refused', '2026-09-16T00:00:00Z', 0)",
                &[
                    Param::Int(ds.id),
                    Param::Int(std::fs::metadata(&stray).unwrap().len() as i64),
                    Param::Int(mtime_of(&stray)),
                ],
            )
            .unwrap();

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(
            (
                surveyed.files,
                surveyed.verified,
                surveyed.unverified,
                surveyed.no_copy,
                surveyed.changed
            ),
            (3, 2, 1, 1, 0)
        );
        let why = surveyed.why.clone().unwrap();
        assert!(
            why.starts_with("1 file of the dataset north is not DICOM and the reader refused it"),
            "{why}"
        );
        assert!(why.contains("Move it out of the originals tree"), "{why}");
        // and nothing offers to delete it
        assert!(!why.contains("delete"), "{why}");

        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.message, why);
        assert!(stray.is_file(), "the file the reader refused stays");
    }

    /// Lab 26c, finding 9 and the ruling: the survey hashes every copy
    /// just before the walk, but the walk asks again as it reaches each
    /// file, so one that changed in between is left rather than destroyed.
    #[test]
    fn the_walk_verifies_each_file_again_and_leaves_what_changed_under_it() {
        let dir = TempDir::new("originals-walk");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let originals = ds.tree_path("originals").unwrap();
        let anon = ds.tree_path("anon").unwrap();
        assert!(survey(&mut registry, &ds).unwrap().ready());

        // the survey has passed; now one copy is corrupted, as it might be
        // in the seconds before its original's turn comes
        std::fs::write(anon.join("x/001/00001.dcm"), b"the FIRST original").unwrap();
        let job_id = claim(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "a purge whose tree changed under it",
        )
        .unwrap();
        let tally = purge(&mut registry, &ds, &originals, job_id, &Cancel::new()).unwrap();

        assert_eq!((tally.files, tally.left), (1, 1));
        assert!(
            tally
                .first
                .as_deref()
                .is_some_and(|w| w.contains("has no verified copy")),
            "{:?}",
            tally.first
        );
        assert!(
            originals.join("a/1.dcm").is_file(),
            "the original whose copy changed under the walk stays"
        );
        assert!(
            !originals.join("a/2.dcm").exists(),
            "the one that verified went"
        );
    }

    /// Lab 26d, finding 1, as a property of one file rather than by weight
    /// of numbers. The walk reads a file's size and modification time when
    /// it lists that file's directory, and the purge judged the file
    /// against those numbers when its turn came, which might be many files
    /// later: an rsync refreshing files in place, a re-export or a
    /// corrected study copied over the old one was destroyed that way, one
    /// file in the lab's first run and five in its second. Here the bytes
    /// are replaced through the seam, in exactly that window and every run:
    /// the file written under the walk is left, holding what was written
    /// into it, the other is removed, and the job says which and why.
    #[test]
    fn a_file_written_under_the_walk_is_read_again_and_left() {
        // eighteen bytes, as the file it replaces, and a modification time
        // that moves with them: what the walk listed is stale either way,
        // and only reading the file again can tell
        const UNDER: &[u8] = b"WRITTEN UNDER WALK";
        let dir = TempDir::new("originals-under-the-walk");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let originals = ds.tree_path("originals").unwrap();
        let written_under = originals.join("a/1.dcm");
        assert_eq!(ORIGINALS[0].1.len(), UNDER.len(), "the same length");
        assert!(
            survey(&mut registry, &ds).unwrap().ready(),
            "every original is verified when the purge is asked for"
        );

        let _seam = write_under_the_walk(originals.clone(), written_under.clone(), UNDER);
        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed and the tree stands",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();

        // the file written under the walk is still there, holding the bytes
        // that were written into it, which exist nowhere else
        assert!(
            written_under.is_file(),
            "a/1.dcm was destroyed on a stale reading"
        );
        assert_eq!(std::fs::read(&written_under).unwrap(), UNDER);
        assert!(
            !originals.join("a/2.dcm").exists(),
            "the one that proved itself went"
        );

        // and the job says so, naming the file and the cure
        assert_eq!(refused.status, 409);
        assert!(
            refused.message.starts_with(
                "1 file under the originals of the dataset north could not be proved when the purge read it again and was left as it is, the first because a/1.dcm changed after its copy was written"
            ),
            "{}",
            refused.message
        );
        assert!(
            refused.message.contains("nils pseudonymize @north"),
            "{}",
            refused.message
        );
        let job = the_job(&mut registry);
        assert_eq!(job.state, State::Failed);
        assert_eq!(job.error.as_deref(), Some(refused.message.as_str()));
        let result = job.result.clone().expect("the job says what it did");
        assert_eq!(
            (result["files"].as_u64(), result["verified"].as_u64()),
            (Some(1), Some(1)),
            "{result}"
        );
        // the dataset keeps the state it had until an act runs to the end
        let after = place::show(registry.store(), ds.id).unwrap().unwrap();
        assert_eq!(after.dataset["originals_kept"], "kept");
    }

    /// Lab 26d, finding 2: an original changed in place and its
    /// modification time then put back to the one the row recorded. The
    /// copy was proved honestly, by a digest hashed on the spot, and the
    /// original by two numbers anything may set, so the file passed as
    /// verified and the purge destroyed bytes that existed nowhere else.
    /// The purge now proves the original by its own digest, read from the
    /// file as it reaches it. The survey is cheap and cannot see this, so
    /// it says in the same breath what it is and what the purge will do.
    #[test]
    fn an_original_changed_under_a_restored_modification_time_is_refused_by_its_digest() {
        const FORGED: &[u8] = b"THE FIRST ORIGINAL";
        let dir = TempDir::new("originals-forged");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let originals = ds.tree_path("originals").unwrap();
        let forged = originals.join("a/1.dcm");
        let was = mtime_of(&forged);

        change_in_place(&forged, FORGED);
        put_back(&forged, was);
        assert_eq!(mtime_of(&forged), was, "the row's own modification time");
        assert_eq!(std::fs::read(&forged).unwrap(), FORGED);

        // the forecast cannot see the bytes, and says as much
        let surveyed = survey(&mut registry, &ds).unwrap();
        assert!(surveyed.ready(), "{:?}", surveyed.why);
        assert_eq!(surveyed.as_json()["forecast"], FORECAST);
        assert!(as_text(&ds, &surveyed).contains(FORECAST));

        // the purge reads the file, cannot prove it, and leaves it
        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed and the tree stands",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.status, 409);
        assert!(
            refused
                .message
                .contains("a/1.dcm changed after its copy was written"),
            "{}",
            refused.message
        );
        assert!(
            refused.message.contains("nils pseudonymize @north"),
            "{}",
            refused.message
        );
        assert_eq!(
            std::fs::read(&forged).unwrap(),
            FORGED,
            "the bytes that exist nowhere else are still here"
        );
        assert!(
            !originals.join("a/2.dcm").exists(),
            "the one that proved itself went"
        );
        let after = place::show(registry.store(), ds.id).unwrap().unwrap();
        assert_eq!(after.dataset["originals_kept"], "kept");
        assert_eq!(the_job(&mut registry).state, State::Failed);
    }

    /// Lab 26d, finding 2, for the rows written before it: a row that says
    /// nothing about what the original hashed to cannot be proved by
    /// anything now. The survey counts those apart, the door refuses in
    /// words naming the run that records the digest, and nothing is
    /// removed. The run itself is proved beside the pseudonymiser, whose
    /// resume reads such a file again.
    #[test]
    fn a_row_without_the_original_s_digest_is_refused_with_the_run_that_records_it() {
        let dir = TempDir::new("originals-unproved");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let originals = ds.tree_path("originals").unwrap();
        registry
            .store()
            .execute(
                "UPDATE pseudonym_file SET original_digest = NULL WHERE path = 'a/1.dcm'",
                &[],
            )
            .unwrap();

        let surveyed = survey(&mut registry, &ds).unwrap();
        assert_eq!(
            (
                surveyed.verified,
                surveyed.unverified,
                surveyed.unproved,
                surveyed.changed,
                surveyed.copy_unverified
            ),
            (1, 1, 1, 0, 0),
            "a row with no digest of its own is neither changed nor a broken copy"
        );
        assert_eq!(surveyed.as_json()["unproved"], 1);
        assert!(
            as_text(&ds, &surveyed).contains(
                "unproved         1 copied before the original's own digest was recorded"
            ),
            "{}",
            as_text(&ds, &surveyed)
        );
        let why = surveyed.why.clone().unwrap();
        assert!(
            why.starts_with(
                "1 file of the dataset north was copied before the pseudonymiser recorded what the original hashed to"
            ),
            "{why}"
        );
        assert!(why.contains("proves every file by its content"), "{why}");
        assert!(
            why.contains("Pseudonymise the dataset again with nils pseudonymize @north, which reads each original and records its digest, then look once more."),
            "{why}"
        );

        let refused = run(
            &mut registry,
            &ds,
            Act::Purge,
            None,
            "the originals are no longer needed",
            WHO,
            &Cancel::new(),
        )
        .unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.message, why);
        assert!(originals.join("a/1.dcm").is_file());
        assert!(
            originals.join("a/2.dcm").is_file(),
            "a refusal at the door removes nothing at all"
        );
        assert!(job::list(registry.store(), true, 10).unwrap().is_empty());
    }

    #[test]
    fn a_dataset_read_in_place_has_no_originals_to_act_on() {
        let dir = TempDir::new("originals-in-place");
        let mut registry = lab(&dir);
        let root = dir.path().join("south");
        dir.file("south/sub-1/1.dcm", b"a file read in place");
        backup(&mut registry, &dir, "archive");
        let id = place::add(
            registry.store(),
            &place::New {
                name: "south",
                role: Role::Source,
                path: &root.display().to_string(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: json!({"arrives": "deidentified"}),
            },
        )
        .unwrap();
        let south = place::show(registry.store(), id).unwrap().unwrap();

        let surveyed = survey(&mut registry, &south).unwrap();
        assert_eq!((surveyed.files, surveyed.verified), (0, 0));
        assert!(!surveyed.ready());
        let why = surveyed.why.clone().unwrap();
        assert!(why.contains("is read in place"), "{why}");
        assert!(why.contains("nothing to vault or purge"), "{why}");
        for act in [Act::Purge, Act::Vault] {
            let refused = check(&mut registry, &south, act, Some("archive")).unwrap_err();
            assert_eq!(refused.message, why);
        }
        // and the file it reads in place is still there
        assert!(root.join("sub-1/1.dcm").is_file());
    }

    #[test]
    fn an_act_asked_to_stop_before_it_began_moves_nothing_and_keeps_the_state() {
        let dir = TempDir::new("originals-stop");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        backup(&mut registry, &dir, "archive");
        let cancel = Cancel::new();
        cancel.request();

        let done = run(
            &mut registry,
            &ds,
            Act::Vault,
            Some("archive"),
            "stopped at once",
            WHO,
            &cancel,
        )
        .unwrap();
        assert!(done.cancelled && done.files == 0);
        let originals = ds.tree_path("originals").unwrap();
        assert!(originals.join("a/1.dcm").is_file(), "nothing moved");
        // the dataset keeps the state it had until an act runs to the end
        let after = place::show(registry.store(), ds.id).unwrap().unwrap();
        assert_eq!(after.dataset["originals_kept"], "kept");
        assert_eq!(after.dataset["originals_vault"], Value::Null);
        assert!(acts(&mut registry, "originals.vault").is_empty());
        let job = the_job(&mut registry);
        assert_eq!(job.state, State::Cancelled);
        assert_eq!(job.result.unwrap()["cancelled"], true);
    }

    #[test]
    fn a_vault_is_refused_where_the_originals_would_not_be_kept_out_of_reach() {
        let dir = TempDir::new("originals-destination");
        let mut registry = lab(&dir);
        let ds = dataset(&mut registry, &dir, "north");
        let elsewhere = dir.path().join("exports");
        std::fs::create_dir_all(&elsewhere).unwrap();
        place::add(
            registry.store(),
            &place::New {
                name: "exports",
                role: Role::Export,
                path: &elsewhere.display().to_string(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: Value::Null,
            },
        )
        .unwrap();
        let mut why = |into: Option<&str>| {
            check(&mut registry, &ds, Act::Vault, into)
                .unwrap_err()
                .message
        };
        assert!(
            why(None).contains("the name of the backup place"),
            "{}",
            why(None)
        );
        assert!(why(Some("nowhere")).contains("no place is named nowhere"));
        assert!(
            why(Some("exports")).contains("the originals go to a backup place"),
            "{}",
            why(Some("exports"))
        );
        assert!(why(Some("north")).contains("into the dataset itself"));
        // and a purge names no destination
        assert!(
            check(&mut registry, &ds, Act::Purge, Some("exports"))
                .unwrap_err()
                .message
                .contains("a purge moves them nowhere")
        );
    }

    #[test]
    fn the_originals_doors_need_their_grants_and_the_acts_are_sensitive() {
        let need = |method: &str, path: &str| {
            let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
            let (need, detail) = crate::serve::door(method, &segs);
            (need.words(), detail)
        };
        assert_eq!(
            need("GET", "/api/places/3/originals"),
            ("the data:see grant".to_string(), Detail::Plain)
        );
        assert_eq!(
            need("POST", "/api/places/3/originals"),
            ("the data:work grant".to_string(), Detail::Sensitive)
        );
        // both doors are in the policy the capabilities carry
        let policy = crate::serve::policy();
        for door in [
            "GET /api/places/{id}/originals",
            "POST /api/places/{id}/originals",
        ] {
            assert!(
                policy.iter().any(|r| r["door"] == door),
                "{door} has no policy row"
            );
        }
        assert_eq!(
            policy
                .iter()
                .find(|r| r["door"] == "POST /api/places/{id}/originals")
                .map(|r| r["detail"].clone()),
            Some(Value::from("sensitive"))
        );
        // the queued verb needs the same, and the queued row is an
        // originals job before it runs as one
        let words: Vec<String> = ["place", "originals", "north", "--purge"]
            .iter()
            .map(|w| w.to_string())
            .collect();
        assert_eq!(
            crate::serve::verb_needs(&words),
            Some(("data:work", Detail::Sensitive))
        );
        assert_eq!(nils_registry::job::kind_of(&words), KIND);
        assert_eq!(
            nils_registry::job::kind_of(&["place".to_string(), "list".to_string()]),
            "place"
        );
    }
}
