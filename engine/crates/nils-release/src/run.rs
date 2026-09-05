// SPDX-License-Identifier: AGPL-3.0-only

//! One release: what it selects, what it writes, and what it records
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8).
//!
//! v0 has two exports. Its own runner says the two callers "run the same
//! underlying engine ... the two callers only differ in scope, output root,
//! and pipeline coupling", and all three differences are gone in v1: digest
//! replaced the cohort pipeline, a cohort is a membership fact rather than a
//! pipeline instance, and the root is an argument. So there is one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::name;
use nils_registry::day::Day;
use nils_registry::schema::{Type, table};
use nils_registry::session::{self, Scheme};
use nils_registry::store::{Error as StoreError, Insert, Param, Store};
use nils_registry::{Registry, time::now_iso};

use crate::policy::{Policy, Uids};
use crate::scrub::{self, Plan};
use crate::tags::Category;
use crate::uid::Remap;

/// What a release will write, as a predicate over the registry.
///
/// Every field narrows; an empty selection is everything. v0 has two shapes of
/// scope, a cohort name and a list of stack ids, in two callers; here both are
/// this.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Subject codes.
    pub subjects: Vec<String>,
    /// Stacks holding this value on the disposition axis. Empty means the
    /// release's default, which is everything that is not `excluded`.
    pub dispositions: Vec<String>,
    /// Stacks holding one of these roles.
    pub roles: Vec<String>,
    /// Only the stacks a pick chose (§10).
    pub picked_only: bool,
    pub modality: Option<String>,
}

impl Selection {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "subjects": self.subjects,
            "dispositions": self.dispositions,
            "roles": self.roles,
            "picked_only": self.picked_only,
            "modality": self.modality,
        })
    }

    pub fn is_everything(&self) -> bool {
        self == &Selection::default()
    }
}

/// What a run says when it is done.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Report {
    pub release_id: i64,
    pub name: String,
    /// `YYYY.MM.DD.N` (§8.6).
    pub version: String,
    /// The version this one was worked out against, if there was one.
    pub previous: Option<String>,
    pub root: String,
    pub policy: String,
    pub subjects: i64,
    pub stacks: i64,
    /// Files in the tree at this version, which on a re-run is mostly files
    /// this run did not touch.
    pub files: i64,
    pub bytes: i64,
    /// Files this run actually wrote, which is what it cost.
    pub written: i64,
    /// Stacks by what became of them (§8.6). The first number is the point:
    /// a re-run after a QC decision should leave nearly everything alone.
    pub unchanged: i64,
    pub moved: i64,
    pub rewritten: i64,
    pub added: i64,
    pub removed: i64,
    /// Files a reader could not read or a writer could not write, by reason.
    pub refused: BTreeMap<String, i64>,
    /// Every change, by tag and action, without a value anywhere.
    pub changes: BTreeMap<String, i64>,
    /// Stacks held back because the file says their pixels carry text, and
    /// stacks the file said nothing about (§8.4). The second number is the one
    /// worth reading: "no tag" is not "no text".
    pub burned_in: i64,
    pub unjudged: i64,
    pub seconds: f64,
}

/// What one release needs beyond its policy.
pub struct Settings<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    pub policy: &'a Policy,
    pub categories: Vec<Category>,
    pub selection: Selection,
    pub scheme: &'a Scheme,
    /// The private elements the pack says are worth keeping (§8.4).
    pub private: &'a [nils_pack::private::Allowed],
    /// What to do with a stack whose file says nothing about its pixels.
    pub on_unknown: crate::burned::OnUnknown,
    pub actor: &'a str,
    /// The key the pseudonyms and the UID remapping are derived from.
    pub key: &'a [u8],
    pub pack: &'a str,
    pub pack_version: &'a str,
}

/// What went wrong.
#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    Refused(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Refused(m) => f.write_str(m),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

/// One instance to write.
struct Instance {
    id: i64,
    stack: i64,
    subject: i64,
    code: String,
    study: i64,
    path: String,
    /// What the walker of Wave 1 recorded about the source file. A re-run
    /// decides a stack is unchanged partly from these, so it trusts exactly
    /// what the digest trusts and no more (§8.6).
    size: i64,
    mtime: i64,
}

/// What one stack's release is: where it goes, what decides its bytes, and
/// what became of it since the last version (§8.6).
struct Job {
    stack: i64,
    dir: String,
    content: String,
    change: crate::version::Change,
    /// Where the last version put it, when there was one.
    was: Option<String>,
    code: String,
    offset: crate::dates::Offset,
    instances: Vec<Instance>,
}

/// What the version before this one wrote.
struct Previous {
    id: i64,
    version: String,
    /// Per stack, what it wrote and where: the state this version compares
    /// against.
    stacks: HashMap<i64, crate::version::Was>,
    /// Per instance, the file it wrote. Carried forward unchanged, so that
    /// every version's manifest is the whole tree rather than only the part of
    /// it this run touched (§11).
    files: HashMap<i64, (String, String, i64)>,
}

/// Run one release.
pub fn run(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    let started = std::time::Instant::now();
    // §4.3, before anything is read: a release that shifts dates and preserves
    // UIDs has shifted nothing, and a warning is read after the tree exists.
    settings
        .policy
        .check()
        .map_err(|e| Error::Refused(e.to_string()))?;
    // And the other half of §4.3: a session label that is a date under a
    // policy that moves dates would put the true date back in the path.
    if settings.policy.dates.moves_dates() && settings.scheme.naming == session::Naming::Date {
        return Err(Error::Refused(format!(
            "dates {} and a session scheme that labels by the date is not a policy: the tree \
             would carry the date the files no longer do (§4.3). Use a months or ordinal \
             scheme, or keep the dates.",
            settings.policy.dates.name()
        )));
    }

    let instances = select(registry.store(), &settings.selection)?;
    let days = study_days(registry.store())?;
    let pixels = pixel_verdicts(registry.store())?;
    let named = names(registry.store(), &days, settings.scheme)?;
    // The version this run is worked out against, read before anything is
    // written, and the version this run will be.
    let earlier = previous(registry.store(), settings.name, settings.root)?;
    let version = crate::version::next(today(), earlier.as_ref().map(|p| p.version.as_str()));

    let mut report = Report {
        name: settings.name.to_string(),
        version: version.clone(),
        previous: earlier.as_ref().map(|p| p.version.clone()),
        root: settings.root.display().to_string(),
        policy: settings.policy.describe(),
        ..Report::default()
    };
    let remap = match settings.policy.uids {
        Uids::Remap => Some(Remap::new(settings.policy.root.clone(), settings.key)),
        Uids::Preserve => None,
    };
    report.release_id = open_row(registry.store(), settings, &version, earlier.as_ref())?;

    // The parts of a stack's content digest that are the same for every stack
    // in the release (§8.6).
    let categories: String = settings
        .categories
        .iter()
        .map(|c| c.name())
        .collect::<Vec<_>>()
        .join(",");
    let private: String = settings
        .private
        .iter()
        .map(|a| a.text())
        .collect::<Vec<_>>()
        .join(",");
    let pack = format!("{}@{}", settings.pack, settings.pack_version);

    // Which studies are one occasion, per subject, so a file lands in the
    // session it belongs to rather than in a directory named by its date.
    let mut by_subject: BTreeMap<i64, Vec<Instance>> = BTreeMap::new();
    for i in instances {
        by_subject.entry(i.subject).or_default().push(i);
    }
    report.subjects = by_subject.len() as i64;

    // What each stack is, where it goes and what became of it, worked out for
    // the whole release before a single file is touched. Nothing here reads or
    // writes a byte of the tree: the decision is what makes a re-run cheap, so
    // it is taken from the registry alone.
    let mut jobs: Vec<Job> = Vec::new();
    let mut held: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (subject, mine) in by_subject {
        let labels = session_labels(&mine, &days, settings.scheme);
        let offset = match settings.policy.dates {
            crate::dates::Policy::Shift => {
                let o = crate::dates::draw(settings.key, subject);
                remember_offset(registry, subject, o)?;
                o
            }
            _ => crate::dates::Offset(0),
        };
        let code = mine.first().map(|i| i.code.clone()).unwrap_or_default();
        let mut grouped: BTreeMap<i64, Vec<Instance>> = BTreeMap::new();
        for i in mine {
            grouped.entry(i.stack).or_default().push(i);
        }
        for (stack, instances) in grouped {
            // §8.4. The engine does not look at pixels; it reads what the file
            // says about them, and holds what the file will not say. A held
            // stack is simply not in this version, so one an earlier version
            // wrote is removed from the tree rather than left in it.
            match pixels
                .get(&stack)
                .copied()
                .unwrap_or(crate::burned::Verdict::Unknown)
            {
                crate::burned::Verdict::Burned => {
                    held.insert(stack);
                    report.burned_in += 1;
                    continue;
                }
                crate::burned::Verdict::Unknown
                    if settings.on_unknown == crate::burned::OnUnknown::Hold =>
                {
                    held.insert(stack);
                    report.unjudged += 1;
                    continue;
                }
                _ => {}
            }
            let study = instances[0].study;
            let label = labels
                .get(&study)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            // §9.1. A stack with no name is one nothing classified, which is a
            // stack the release should not silently rename into something
            // readable.
            let (folder, stem) = match named.get(&stack) {
                Some(n) => (n.folder.clone(), n.name.clone()),
                None => ("misc".to_string(), format!("stack-{stack:08}")),
            };
            let dir = format!("sub-{code}/ses-{label}/{folder}/{stem}");
            let content = crate::version::content_of(
                &subject_policy(settings, &code, offset),
                &categories,
                &private,
                &pack,
                &instances
                    .iter()
                    .map(|i| (i.id, i.size, i.mtime))
                    .collect::<Vec<_>>(),
            );
            let was = earlier.as_ref().and_then(|p| p.stacks.get(&stack));
            let mut change = crate::version::compare(was, &content, &dir);
            // A stack whose files the last version did not all write is not
            // one this version may carry forward, whatever the digest says:
            // the digest describes the decision and the manifest describes the
            // tree, and only the manifest knows a file was refused.
            let carried = !change.is_work() || change == crate::version::Change::Moved;
            if carried
                && !instances.iter().all(|i| {
                    earlier
                        .as_ref()
                        .is_some_and(|p| p.files.contains_key(&i.id))
                })
            {
                change = crate::version::Change::Rewritten;
            }
            jobs.push(Job {
                stack,
                dir,
                content,
                change,
                was: was.map(|w| w.dir.clone()),
                code: code.clone(),
                offset,
                instances,
            });
        }
    }
    report.stacks = jobs.len() as i64;

    // What the last version wrote and this one does not.
    let here: std::collections::HashSet<i64> = jobs.iter().map(|j| j.stack).collect();
    let mut gone: Vec<(i64, String)> = match &earlier {
        Some(p) => p
            .stacks
            .iter()
            .filter(|(stack, _)| !here.contains(stack))
            .map(|(stack, was)| (*stack, was.dir.clone()))
            .collect(),
        None => Vec::new(),
    };
    gone.sort();

    // Everything that leaves goes first, so that a directory a move is about
    // to land in is free by the time the move happens.
    for (_, dir) in &gone {
        drop_dir(settings.root, dir);
    }
    for job in &jobs {
        if job.change == crate::version::Change::Rewritten {
            if let Some(was) = &job.was {
                drop_dir(settings.root, was);
            }
            drop_dir(settings.root, &job.dir);
        }
    }
    let moved = move_them(settings.root, &jobs);

    // And only now is anything written.
    let mut files: Vec<Vec<Param>> = Vec::new();
    let mut moves: Vec<Vec<Param>> = Vec::new();
    let mut rows: Vec<Vec<Param>> = Vec::new();
    for job in &mut jobs {
        // A move whose source is not where the last version left it is not a
        // move. Somebody emptied the tree, and the stack is written again.
        if job.change == crate::version::Change::Moved && !moved.contains(&job.stack) {
            job.change = crate::version::Change::Rewritten;
        }
        let mut mine: Vec<Vec<Param>> = Vec::new();
        match job.change {
            crate::version::Change::Unchanged | crate::version::Change::Moved => {
                // The bytes are the ones the last version wrote, and the
                // digest with them: nothing was read, so nothing is recomputed.
                for i in &job.instances {
                    let Some((path, digest, bytes)) =
                        earlier.as_ref().and_then(|p| p.files.get(&i.id))
                    else {
                        continue;
                    };
                    let path = match job.change {
                        crate::version::Change::Moved => rebase(path, &job.dir),
                        _ => path.clone(),
                    };
                    report.bytes += bytes;
                    mine.push(vec![
                        Param::Int(report.release_id),
                        Param::Int(i.id),
                        Param::from(path),
                        Param::from(digest.as_str()),
                        Param::Int(*bytes),
                    ]);
                }
            }
            _ => {
                let plan = Plan {
                    policy: settings.policy,
                    categories: &settings.categories,
                    private: settings.private,
                    code: &job.code,
                    offset: job.offset,
                    remap: remap.as_ref(),
                };
                for i in &job.instances {
                    match write_one(i, &plan, settings.root, &job.dir) {
                        Ok(written) => {
                            report.written += 1;
                            report.bytes += written.bytes;
                            for ((tag, action), n) in &written.applied.changes {
                                *report.changes.entry(format!("{tag} {action}")).or_insert(0) += n;
                            }
                            mine.push(vec![
                                Param::Int(report.release_id),
                                Param::Int(i.id),
                                Param::from(written.path.as_str()),
                                Param::from(written.digest.as_str()),
                                Param::Int(written.bytes),
                            ]);
                        }
                        Err(why) => {
                            // By the reason and never by the path: a report
                            // that names the files it could not read names the
                            // source tree, which is the one thing a released
                            // dataset must not carry.
                            *report.refused.entry(why).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        report.files += mine.len() as i64;
        rows.push(vec![
            Param::Int(report.release_id),
            Param::Int(job.stack),
            Param::from(job.content.as_str()),
            Param::from(job.dir.as_str()),
            Param::Int(mine.len() as i64),
        ]);
        match job.change {
            crate::version::Change::Unchanged => report.unchanged += 1,
            crate::version::Change::Moved => report.moved += 1,
            crate::version::Change::Rewritten => report.rewritten += 1,
            crate::version::Change::Added => report.added += 1,
            crate::version::Change::Removed => {}
        }
        if job.change.is_work() {
            moves.push(vec![
                Param::Int(report.release_id),
                Param::Int(job.stack),
                Param::from(job.change.name()),
                match &job.was {
                    Some(was) => Param::from(was.as_str()),
                    None => Param::Null,
                },
                Param::from(job.dir.as_str()),
            ]);
        }
        files.append(&mut mine);
    }
    for (stack, was) in &gone {
        report.removed += 1;
        moves.push(vec![
            Param::Int(report.release_id),
            Param::Int(*stack),
            Param::from(crate::version::Change::Removed.name()),
            Param::from(was.as_str()),
            Param::Null,
        ]);
    }

    write_files(registry.store(), &files)?;
    write_rows(
        registry.store(),
        "release_stack",
        &["release_id", "stack_id", "content", "dir", "files"],
        &rows,
    )?;
    write_rows(
        registry.store(),
        "release_move",
        &["release_id", "stack_id", "action", "was", "now"],
        &moves,
    )?;
    write_changes(registry.store(), &report)?;
    if !held.is_empty() {
        raise_review(registry.store(), &report, &held, &pixels)?;
    }
    close_row(registry.store(), &report)?;
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// The day this release is made, for its version.
fn today() -> Day {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| Day::from_unix(d.as_secs() as i64))
        .unwrap_or_else(|| Day::new(1970, 1, 1).expect("the epoch is a day"))
}

/// The policy, as it applies to one subject.
///
/// The pseudonym and the date offset are in it because both are drawn from the
/// key, and neither is anywhere else in the content digest. A release re-run
/// under a different key writes different bytes into a differently named tree,
/// and a comparison that could not see that would call it unchanged.
fn subject_policy(settings: &Settings, code: &str, offset: crate::dates::Offset) -> String {
    format!(
        "{} code={code} offset={} unknown={}",
        settings.policy.as_json(),
        offset.0,
        settings.on_unknown.name(),
    )
}

/// The newest finished release of this dataset into this root.
///
/// The root has to match. A release of the same name into a different
/// directory is a different tree, and comparing against a state that describes
/// some other directory would leave every unchanged file simply missing.
///
/// Ordered by id and not by the version, because `2026.09.05.10` sorts before
/// `2026.09.05.9` and the tenth version of a day is not the second.
fn previous(store: &mut Store, name: &str, root: &Path) -> Result<Option<Previous>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, version FROM {} WHERE name = {} AND root = {} AND finished_at IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("release"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    let params = [Param::from(name), Param::from(root.display().to_string())];
    let Some(row) = store.query_opt(&sql, &params)? else {
        return Ok(None);
    };
    let (id, version) = (row.int(0)?, row.text(1)?.to_string());

    let d = store.dialect();
    let sql = format!(
        "SELECT stack_id, content, dir FROM {} WHERE release_id = {}",
        store.qualified("release_stack"),
        d.param(1, Type::Int),
    );
    let mut stacks = HashMap::new();
    for r in store.query(&sql, &[Param::Int(id)])? {
        stacks.insert(
            r.int(0)?,
            crate::version::Was {
                content: r.text(1)?.to_string(),
                dir: r.text(2)?.to_string(),
            },
        );
    }

    let d = store.dialect();
    let sql = format!(
        "SELECT instance_id, path, digest, bytes FROM {} WHERE release_id = {}",
        store.qualified("release_file"),
        d.param(1, Type::Int),
    );
    let mut files = HashMap::new();
    for r in store.query(&sql, &[Param::Int(id)])? {
        files.insert(
            r.int(0)?,
            (r.text(1)?.to_string(), r.text(2)?.to_string(), r.int(3)?),
        );
    }
    Ok(Some(Previous {
        id,
        version,
        stacks,
        files,
    }))
}

/// A file keeps its name and changes directory, which is what a move is.
fn rebase(path: &str, dir: &str) -> String {
    match path.rsplit_once('/') {
        Some((_, file)) => format!("{dir}/{file}"),
        None => format!("{dir}/{path}"),
    }
}

/// Rename every moved stack's directory, and say which arrived.
///
/// In two phases, through a staging directory, because two stacks can swap
/// names between versions: a disambiguating suffix moves when a sibling
/// appears or leaves, and renaming one onto the other in place would lose a
/// tree.
fn move_them(root: &Path, jobs: &[Job]) -> std::collections::HashSet<i64> {
    let mut arrived = std::collections::HashSet::new();
    let moving: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.change == crate::version::Change::Moved)
        .collect();
    if moving.is_empty() {
        return arrived;
    }
    let staging = root.join(".nils-moving");
    if std::fs::create_dir_all(&staging).is_err() {
        return arrived;
    }
    let mut staged: Vec<&Job> = Vec::new();
    for job in &moving {
        let Some(was) = &job.was else { continue };
        if std::fs::rename(root.join(was), staging.join(job.stack.to_string())).is_ok() {
            staged.push(job);
        }
    }
    for job in staged {
        let to = root.join(&job.dir);
        if let Some(parent) = to.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        if std::fs::rename(staging.join(job.stack.to_string()), &to).is_ok() {
            arrived.insert(job.stack);
        }
    }
    for job in &moving {
        if let Some(was) = &job.was {
            prune(root, root.join(was).parent());
        }
    }
    std::fs::remove_dir_all(&staging).ok();
    arrived
}

/// Remove a stack's directory, and any parent it leaves empty.
fn drop_dir(root: &Path, dir: &str) {
    let full = root.join(dir);
    std::fs::remove_dir_all(&full).ok();
    prune(root, full.parent());
}

/// Walk up from an emptied directory, removing what is now empty.
///
/// Bounded by the release root, which it never removes: a version that dropped
/// a subject should not leave `sub-x/ses-1/anat` behind, and must not walk
/// above the tree it owns. `remove_dir` refusing a directory that still holds
/// something is the stopping condition.
fn prune(root: &Path, from: Option<&Path>) {
    let mut here = from.map(Path::to_path_buf);
    while let Some(path) = here {
        if path == root || !path.starts_with(root) {
            break;
        }
        if std::fs::remove_dir(&path).is_err() {
            break;
        }
        here = path.parent().map(Path::to_path_buf);
    }
}

/// Every instance the selection names, with what the writer needs.
fn select(store: &mut Store, selection: &Selection) -> Result<Vec<Instance>, Error> {
    let mut wheres: Vec<String> = Vec::new();
    if !selection.subjects.is_empty() {
        wheres.push(format!(
            "su.code IN ({})",
            selection
                .subjects
                .iter()
                .map(|s| format!("'{}'", s.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(m) = &selection.modality {
        wheres.push(format!("se.modality = '{}'", m.replace('\'', "''")));
    }
    let axis = store.qualified("classification_axis");
    // A stack the pack ruled out is not written, and saying so as a default
    // rather than as a flag is what keeps a release from carrying screenshots.
    let dispositions = if selection.dispositions.is_empty() {
        vec!["excluded".to_string()]
    } else {
        selection.dispositions.clone()
    };
    if selection.dispositions.is_empty() {
        wheres.push(format!(
            "NOT EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value = 'excluded')"
        ));
    } else {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'disposition' \
             AND a.value IN ({}))",
            dispositions
                .iter()
                .map(|d| format!("'{}'", d.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !selection.roles.is_empty() {
        let any = selection
            .roles
            .iter()
            .map(|r| {
                format!(
                    "a.value = '{r}' OR a.value LIKE '{r},%' OR a.value LIKE '%,{r}' \
                     OR a.value LIKE '%,{r},%'",
                    r = r.replace('\'', "''")
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {axis} a WHERE a.stack_id = k.id AND a.axis = 'role' \
             AND ({any}))"
        ));
    }
    if selection.picked_only {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM {} ps JOIN {} p ON p.id = ps.pick_id \
             WHERE ps.stack_id = k.id AND p.withdrawn_at IS NULL)",
            store.qualified("pick_stack"),
            store.qualified("pick"),
        ));
    }
    let filter = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        // The stored path is relative to the batch root it was walked from,
        // which is what makes a registry portable; the root is on `source`.
        "SELECT i.id, k.id, se.subject_id, su.code, se.study_id, so.root, sf.path, \
          sf.size, sf.mtime_ns \
         FROM {instance} i \
         JOIN {stack} k ON k.id = i.stack_id \
         JOIN {series} se ON se.id = i.series_id \
         JOIN {subject} su ON su.id = se.subject_id \
         JOIN {source_file} sf ON sf.instance_id = i.id \
         JOIN {source} so ON so.id = sf.source_id{filter} \
         ORDER BY se.subject_id, se.study_id, k.id, i.id",
        instance = store.qualified("instance"),
        stack = store.qualified("stack"),
        series = store.qualified("series"),
        subject = store.qualified("subject"),
        source_file = store.qualified("source_file"),
        source = store.qualified("source"),
    );
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        // An instance may sit under more than one path (§ Wave 1's duplicate
        // handling); one copy of it is written.
        if !seen.insert(id) {
            continue;
        }
        out.push(Instance {
            id,
            stack: r.int(1)?,
            subject: r.int(2)?,
            code: r.text(3)?.to_string(),
            study: r.int(4)?,
            path: Path::new(r.text(5)?).join(r.text(6)?).display().to_string(),
            size: r.int(7)?,
            mtime: r.int(8)?,
        });
    }
    Ok(out)
}

fn study_days(store: &mut Store) -> Result<HashMap<i64, Day>, Error> {
    // Both dates through the dialect's own rendering. Postgres hands a `date`
    // back in a type the store reads only as text, and the release read it raw
    // until a release was run on Postgres.
    let t = table("study");
    let d = store.dialect();
    let day = |c: &str| {
        d.text_of(
            t.column(c)
                .unwrap_or_else(|| panic!("study.{c} is not a column")),
        )
    };
    let (filled, study) = (day("date_filled"), day("study_date"));
    let sql = format!(
        "SELECT id, COALESCE({filled}, {study}) FROM {} \
         WHERE COALESCE({filled}, {study}) IS NOT NULL",
        store.qualified("study")
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(day) = Day::parse(r.text(1)?) {
            out.insert(r.int(0)?, day);
        }
    }
    Ok(out)
}

/// The session label of each of a subject's studies.
fn session_labels(
    mine: &[Instance],
    days: &HashMap<i64, Day>,
    scheme: &Scheme,
) -> HashMap<i64, String> {
    let mut studies: Vec<session::Study> = Vec::new();
    let mut seen: Vec<i64> = Vec::new();
    for i in mine {
        if seen.contains(&i.study) {
            continue;
        }
        let Some(day) = days.get(&i.study) else {
            continue;
        };
        seen.push(i.study);
        studies.push(session::Study::new(i.study, *day));
    }
    let anchor = studies.iter().map(|s| s.day).min();
    let mut out = HashMap::new();
    for occasion in session::sessions(&studies, anchor, scheme) {
        // A session with no label is named by the day it opened, which is what
        // `keep_date` means and why §4.3 refuses the combination that would
        // put a date here under a policy that moves them.
        let label = occasion
            .label
            .clone()
            .unwrap_or_else(|| occasion.first.compact());
        for study in &occasion.studies {
            out.insert(*study, label.clone());
        }
    }
    out
}

struct Written {
    path: String,
    digest: String,
    bytes: i64,
    applied: scrub::Applied,
}

/// Read one file whole, apply the plan, write it out.
///
/// Read whole, because a release writes the pixels. The digest reader stops
/// before them, which is right for reading a header at speed and wrong here.
fn write_one(instance: &Instance, plan: &Plan, root: &Path, dir: &str) -> Result<Written, String> {
    let mut object = open(Path::new(&instance.path))?;
    let applied = scrub::apply(&mut object, plan);

    let relative = PathBuf::from(dir).join(format!("{:08}.dcm", instance.id));
    let target = root.join(&relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "no directory to write into".to_string())?;
    }
    // Written whole and then moved into place, so that a run interrupted
    // halfway leaves no half a file for a handover to checksum.
    let temporary = target.with_extension("dcm.part");
    object
        .write_to_file(&temporary)
        .map_err(|e| format!("unwritable: {}", first_line(&e.to_string())))?;
    let bytes = std::fs::read(&temporary).map_err(|_| "unreadable after writing".to_string())?;
    // BLAKE2b-256 of what was written, so a handover can verify a tree
    // without reading the files back through a DICOM parser (§11).
    let digest = hex::encode(digest_of(&bytes));
    std::fs::rename(&temporary, &target)
        .map_err(|_| "could not be moved into place".to_string())?;

    Ok(Written {
        path: relative.display().to_string(),
        digest,
        bytes: bytes.len() as i64,
        applied,
    })
}

/// Open a file whole, part 10 or bare.
///
/// A release writes conformant part 10 whatever it read, which means a bare
/// data set gains the file meta group it never had, built from what the data
/// set itself says. 152 of the 3,028 files of the test archive are bare, and a
/// release that refused them would silently drop five percent of a dataset.
fn open(path: &Path) -> Result<dicom_object::DefaultDicomObject, String> {
    use dicom_object::file::ReadPreamble;
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject, OpenFileOptions};
    use dicom_transfer_syntax_registry::TransferSyntaxIndex;
    if let Ok(object) = OpenFileOptions::new()
        .read_preamble(ReadPreamble::Auto)
        .open_file(path)
    {
        return Ok(object);
    }
    // No file meta group. The transfer syntax is not written down anywhere in
    // a bare data set, so it is inferred exactly as the reader of Wave 1
    // infers it, by whether the first element carries a VR.
    for name in explicit_first(path) {
        let Some(ts) = dicom_transfer_syntax_registry::TransferSyntaxRegistry.get(name) else {
            continue;
        };
        let Ok(file) = std::fs::File::open(path) else {
            return Err("unreadable".to_string());
        };
        let Ok(dataset) = InMemDicomObject::read_dataset_with_ts(std::io::BufReader::new(file), ts)
        else {
            continue;
        };
        // The class and the instance are what a meta table is built from, and
        // a data set with neither is not an object to release.
        let Ok(object) = dataset.with_meta(FileMetaTableBuilder::new().transfer_syntax(name))
        else {
            continue;
        };
        return Ok(object);
    }
    Err("unreadable".to_string())
}

/// The two transfer syntaxes a bare data set may be in, likeliest first.
fn explicit_first(path: &Path) -> [&'static str; 2] {
    use std::io::Read;
    let mut head = [0u8; 6];
    let explicit = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && head[4].is_ascii_uppercase()
        && head[5].is_ascii_uppercase();
    if explicit {
        ["1.2.840.10008.1.2.1", "1.2.840.10008.1.2"]
    } else {
        ["1.2.840.10008.1.2", "1.2.840.10008.1.2.1"]
    }
}

/// The digest of what was written.
fn digest_of(bytes: &[u8]) -> [u8; 32] {
    use blake2::Digest;
    let mut h = blake2::Blake2s256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&out);
    digest
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

/// The offset a subject's dates moved by, kept with the identifiers rather
/// than beside the images: it is the thing that undoes the policy.
fn remember_offset(
    registry: &mut Registry,
    subject: i64,
    offset: crate::dates::Offset,
) -> Result<(), Error> {
    let mut store = registry
        .open_linkage()
        .map_err(|e| Error::Refused(e.to_string()))?;
    let sql = format!(
        "SELECT offset_days FROM {} WHERE subject_id = {}",
        store.qualified("date_shift"),
        store.dialect().param(1, Type::Int)
    );
    if store.query_opt(&sql, &[Param::Int(subject)])?.is_some() {
        return Ok(());
    }
    store.insert(
        &Insert::new(table("date_shift"), &["subject_id", "offset_days"]),
        &[vec![Param::Int(subject), Param::Int(offset.0)]],
    )?;
    Ok(())
}

fn open_row(
    store: &mut Store,
    settings: &Settings,
    version: &str,
    earlier: Option<&Previous>,
) -> Result<i64, Error> {
    let categories: Vec<&str> = settings.categories.iter().map(|c| c.name()).collect();
    let written = store.insert(
        &Insert::new(
            table("release"),
            &[
                "name",
                "version",
                "previous_id",
                "root",
                "policy",
                "selection",
                "categories",
                "session_scheme",
                "pack",
                "pack_version",
                "actor",
                "started_at",
                "files",
                "subjects",
                "unchanged",
                "moved",
                "rewritten",
                "added",
                "removed",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(settings.name),
            Param::from(version),
            match earlier {
                Some(p) => Param::Int(p.id),
                None => Param::Null,
            },
            Param::from(settings.root.display().to_string()),
            Param::from(settings.policy.as_json().to_string()),
            Param::from(settings.selection.as_json().to_string()),
            Param::from(categories.join(",")),
            Param::from(
                serde_json::to_string(settings.scheme).unwrap_or_else(|_| "{}".to_string()),
            ),
            Param::from(settings.pack),
            Param::from(settings.pack_version),
            Param::from(settings.actor),
            Param::from(now_iso()),
            // Filled in when the run closes; a run that never closed says it
            // did nothing, which is what a tree it half wrote is worth.
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
            Param::Int(0),
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

fn write_files(store: &mut Store, files: &[Vec<Param>]) -> Result<(), Error> {
    if files.is_empty() {
        return Ok(());
    }
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("release_file"),
            &["release_id", "instance_id", "path", "digest", "bytes"],
        ),
        files,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

fn close_row(store: &mut Store, report: &Report) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET finished_at = {}, files = {}, subjects = {}, unchanged = {}, moved = {}, \
         rewritten = {}, added = {}, removed = {} WHERE id = {}",
        store.qualified("release"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
        d.param(5, Type::Int),
        d.param(6, Type::Int),
        d.param(7, Type::Int),
        d.param(8, Type::Int),
        d.param(9, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::from(now_iso()),
            Param::Int(report.files),
            Param::Int(report.subjects),
            Param::Int(report.unchanged),
            Param::Int(report.moved),
            Param::Int(report.rewritten),
            Param::Int(report.added),
            Param::Int(report.removed),
            Param::Int(report.release_id),
        ],
    )?;
    Ok(())
}

/// Insert a batch, or leave the table as it was.
fn write_rows(
    store: &mut Store,
    into: &str,
    columns: &[&str],
    rows: &[Vec<Param>],
) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    store.begin()?;
    match store.insert(&Insert::new(table(into), columns), rows) {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// What every stack in the registry is called under the `descriptive` layout
/// (§9.1), and where it lands.
///
/// **Every** stack, and not only the selected ones. A name has to be unique in
/// the directory it lands in, and that directory holds what the registry holds
/// rather than what this release picked: v0 computes the same thing over the
/// already filtered list, so exporting one echo of a two-echo series drops the
/// echo suffix and the file is named as though it were the only one.
fn names(
    store: &mut Store,
    days: &HashMap<i64, Day>,
    scheme: &Scheme,
) -> Result<HashMap<i64, name::Named>, Error> {
    let axes = axis_values(store)?;
    let t = table("stack_fingerprint");
    let d = store.dialect();
    let text = |c: &str| {
        d.text_of_qualified(
            Some("f"),
            t.column(c)
                .unwrap_or_else(|| panic!("stack_fingerprint.{c} is not a column")),
        )
    };
    // The text columns are cast so that a dialect's own rendering does not
    // decide the name; the numbers are read as numbers.
    let sql = format!(
        "SELECT f.stack_id, f.subject_id, f.study_id, f.series_id, f.stacks_in_series, \
                f.stack_index, {}, {}, {}, {}, {}, \
                f.inversion_time, f.dwi_b_value, f.dwi_directions \
         FROM {} f ORDER BY f.stack_id",
        text("orientation"),
        text("split_reason"),
        text("echo_numbers"),
        text("mr_acquisition_type"),
        text("dwi_pe_direction"),
        store.qualified("stack_fingerprint"),
    );

    // Bucket by subject, session and folder, which is the directory a name has
    // to be unique in.
    let mut buckets: BTreeMap<(i64, String, String), Vec<name::Named>> = BTreeMap::new();
    let mut labels: HashMap<i64, HashMap<i64, String>> = HashMap::new();
    let mut studies_of: BTreeMap<i64, Vec<(i64, Day)>> = BTreeMap::new();
    let rows = store.query(&sql, &[])?;
    for r in &rows {
        if let Some(day) = days.get(&r.int(2)?) {
            let mine = studies_of.entry(r.int(1)?).or_default();
            if !mine.iter().any(|(id, _)| *id == r.int(2).unwrap_or(0)) {
                mine.push((r.int(2)?, *day));
            }
        }
    }
    for (subject, studies) in &studies_of {
        let points: Vec<session::Study> = studies
            .iter()
            .map(|(id, day)| session::Study::new(*id, *day))
            .collect();
        let anchor = points.iter().map(|s| s.day).min();
        let mut mine = HashMap::new();
        for occasion in session::sessions(&points, anchor, scheme) {
            let label = occasion
                .label
                .clone()
                .unwrap_or_else(|| occasion.first.compact());
            for study in &occasion.studies {
                mine.insert(*study, label.clone());
            }
        }
        labels.insert(*subject, mine);
    }

    for r in &rows {
        let stack = r.int(0)?;
        let empty = BTreeMap::new();
        let a = axes.get(&stack).unwrap_or(&empty);
        let get = |k: &str| a.get(k).map(String::as_str).filter(|v| !v.is_empty());
        let folder = folder_of(get("directory_type"), get("provenance"));
        let fields = name::Fields {
            body_part: get("body_part"),
            spinal_cord: get("body_part") == Some("spine"),
            orientation: r.opt_text(6)?,
            base: get("base"),
            acquisition_type: r.opt_text(9)?,
            modifier: get("modifier"),
            technique: get("technique"),
            acceleration: get("acceleration"),
            construct: get("construct"),
            post_contrast: get("post_contrast") == Some("yes"),
            datatype: get("directory_type"),
            dwi_b_value: r.double(12).ok(),
            dwi_pe_direction: r.opt_text(10)?,
            dwi_directions: r.opt_int(13)?,
        };
        let subject = r.int(1)?;
        let study = r.int(2)?;
        let label = labels
            .get(&subject)
            .and_then(|m| m.get(&study))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        buckets
            .entry((subject, label, folder.clone()))
            .or_default()
            .push(name::Named {
                stack,
                name: name::describe(&fields, true, true),
                folder,
                echo: r.opt_text(8)?.and_then(first_int),
                inversion_time: r.double(11).ok(),
                series: r.int(3)?,
                siblings: r.int(4)?,
                split: r.opt_text(7)?.map(str::to_string),
                index: r.int(5)?,
            });
    }

    let mut out = HashMap::new();
    for bucket in buckets.values_mut() {
        name::disambiguate(bucket);
        for n in bucket.iter() {
            out.insert(n.stack, n.clone());
        }
    }
    Ok(out)
}

/// `EchoNumbers` may carry several values; the first is this stack's.
fn first_int(text: &str) -> Option<i64> {
    text.split(['\\', ','])
        .next()?
        .trim()
        .parse()
        .ok()
        .filter(|n: &i64| *n > 0)
}

/// Where a stack lands, which is its intent with v0's provenance routing.
///
/// v0 sends a SyMRI to `anat/SyMRI` under a flag, and an SWI, a STAGE and a
/// projection to `anat` whatever their intent says. The grouping is kept
/// because it is what people have on disk.
fn folder_of(datatype: Option<&str>, provenance: Option<&str>) -> String {
    match provenance {
        Some("SyMRI") => "anat/SyMRI".to_string(),
        Some("SWIRecon") | Some("STAGE") | Some("ProjectionDerived") => "anat".to_string(),
        _ => datatype.unwrap_or("misc").to_string(),
    }
}

/// Every decided axis of every stack.
fn axis_values(store: &mut Store) -> Result<HashMap<i64, BTreeMap<String, String>>, Error> {
    let sql = format!(
        "SELECT stack_id, axis, value FROM {}",
        store.qualified("classification_axis")
    );
    let mut out: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    for r in store.query(&sql, &[])? {
        if let Some(v) = r.opt_text(2)? {
            out.entry(r.int(0)?)
                .or_default()
                .insert(r.text(1)?.to_string(), v.to_string());
        }
    }
    Ok(out)
}

/// What each stack's file says about its own pixels (§8.4).
///
/// Read from the stack and its series, not from the fingerprint, so that a
/// release after a digest alone judges as well as one after a fingerprint. A
/// check that quietly did nothing until some earlier verb had run is a check
/// nobody can rely on, and this is the one standing between a screenshot and a
/// dataset.
///
/// The fingerprint's `image_role` is joined in where it exists, because §6
/// worked the same three tokens out once and three things read it; where it
/// does not, the image type is read here and reaches the same answer.
fn pixel_verdicts(store: &mut Store) -> Result<HashMap<i64, crate::burned::Verdict>, Error> {
    let annotation = table("series")
        .column("burned_in_annotation")
        .expect("series.burned_in_annotation is a column");
    let image_type = table("stack")
        .column("image_type")
        .expect("stack.image_type is a column");
    let series_image_type = table("series")
        .column("image_type")
        .expect("series.image_type is a column");
    let d = store.dialect();
    let sql = format!(
        "SELECT k.id, {}, f.image_role, COALESCE({}, {}) \
         FROM {} k \
         JOIN {} se ON se.id = k.series_id \
         LEFT JOIN {} f ON f.stack_id = k.id",
        d.text_of_qualified(Some("se"), annotation),
        d.text_of_qualified(Some("k"), image_type),
        d.text_of_qualified(Some("se"), series_image_type),
        store.qualified("stack"),
        store.qualified("series"),
        store.qualified("stack_fingerprint"),
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        out.insert(
            r.int(0)?,
            crate::burned::judge(
                r.opt_text(1)?.map(str::trim),
                r.opt_text(2)?,
                r.opt_text(3)?,
            ),
        );
    }
    Ok(out)
}

/// What was changed, by tag and action (§8.5). No old value anywhere.
fn write_changes(store: &mut Store, report: &Report) -> Result<(), Error> {
    if report.changes.is_empty() {
        return Ok(());
    }
    let rows: Vec<Vec<Param>> = report
        .changes
        .iter()
        .map(|(what, n)| {
            let (tag, action) = what.rsplit_once(' ').unwrap_or((what.as_str(), ""));
            vec![
                Param::Int(report.release_id),
                Param::from(tag),
                Param::from(action),
                Param::Int(*n),
            ]
        })
        .collect();
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("release_change"),
            &["release_id", "tag", "action", "count"],
        ),
        &rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// One question per held stack, so a person can answer it and the release can
/// be run again.
fn raise_review(
    store: &mut Store,
    report: &Report,
    held: &std::collections::HashSet<i64>,
    pixels: &HashMap<i64, crate::burned::Verdict>,
) -> Result<(), Error> {
    let now = now_iso();
    let mut ids: Vec<i64> = held.iter().copied().collect();
    ids.sort_unstable();
    let rows: Vec<Vec<Param>> = ids
        .iter()
        .map(|stack| {
            let verdict = pixels
                .get(stack)
                .copied()
                .unwrap_or(crate::burned::Verdict::Unknown);
            vec![
                Param::from(format!("release.{}", verdict.name())),
                Param::from("stack"),
                Param::from(serde_json::json!({"stack_id": stack}).to_string()),
                Param::from(
                    serde_json::json!({
                        "release": report.release_id,
                        "verdict": verdict.name(),
                        "why": "the engine does not look at pixels; the file was asked",
                    })
                    .to_string(),
                ),
                Param::from("open"),
                Param::from(now.as_str()),
            ]
        })
        .collect();
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("review_item"),
            &["kind", "scope", "ref", "evidence", "status", "created_at"],
        ),
        &rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;

    fn job(stack: i64, was: &str, dir: &str) -> Job {
        Job {
            stack,
            dir: dir.to_string(),
            content: "same".to_string(),
            change: crate::version::Change::Moved,
            was: Some(was.to_string()),
            code: "x".to_string(),
            offset: crate::dates::Offset(0),
            instances: Vec::new(),
        }
    }

    fn write(root: &Path, path: &str) {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, path.as_bytes()).unwrap();
    }

    fn read(root: &Path, path: &str) -> Option<String> {
        std::fs::read_to_string(root.join(path)).ok()
    }

    #[test]
    fn two_stacks_can_swap_names_between_versions() {
        // Which is why the moves go through a staging directory. A
        // disambiguating suffix moves when a sibling appears or leaves, and
        // renaming one onto the other in place would lose a tree.
        let dir = TempDir::new("move");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w_1/00000001.dcm");
        write(root, "sub-x/ses-1/anat/T1w_2/00000002.dcm");
        let jobs = vec![
            job(1, "sub-x/ses-1/anat/T1w_1", "sub-x/ses-1/anat/T1w_2"),
            job(2, "sub-x/ses-1/anat/T1w_2", "sub-x/ses-1/anat/T1w_1"),
        ];
        let arrived = move_them(root, &jobs);
        assert_eq!(arrived.len(), 2);
        assert_eq!(
            read(root, "sub-x/ses-1/anat/T1w_2/00000001.dcm").as_deref(),
            Some("sub-x/ses-1/anat/T1w_1/00000001.dcm"),
            "the first went where the second was"
        );
        assert_eq!(
            read(root, "sub-x/ses-1/anat/T1w_1/00000002.dcm").as_deref(),
            Some("sub-x/ses-1/anat/T1w_2/00000002.dcm")
        );
        assert!(!root.join(".nils-moving").exists(), "and it tidied up");
    }

    #[test]
    fn a_move_whose_source_is_gone_is_not_reported_as_a_move() {
        // Somebody emptied the tree. The stack is written again, which the
        // caller does by reading what did not arrive.
        let dir = TempDir::new("move-gone");
        let jobs = vec![job(1, "sub-x/ses-1/anat/T1w", "sub-x/ses-1/anat/SC_T1w")];
        assert!(move_them(dir.path(), &jobs).is_empty());
    }

    #[test]
    fn a_directory_that_empties_takes_its_parents_with_it() {
        // A version that dropped a subject should not leave `sub-x/ses-1/anat`
        // behind. The release root itself is never removed.
        let dir = TempDir::new("drop");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w/00000001.dcm");
        write(root, "sub-y/ses-1/anat/T1w/00000002.dcm");
        drop_dir(root, "sub-x/ses-1/anat/T1w");
        assert!(!root.join("sub-x").exists());
        assert!(root.join("sub-y/ses-1/anat/T1w").exists(), "and only that");
        assert!(root.exists());
    }

    #[test]
    fn a_parent_that_still_holds_something_stays() {
        let dir = TempDir::new("drop-sibling");
        let root = dir.path();
        write(root, "sub-x/ses-1/anat/T1w/00000001.dcm");
        write(root, "sub-x/ses-1/anat/T2w/00000002.dcm");
        drop_dir(root, "sub-x/ses-1/anat/T1w");
        assert!(root.join("sub-x/ses-1/anat/T2w").exists());
    }

    #[test]
    fn a_moved_file_keeps_its_name_and_changes_directory() {
        assert_eq!(
            rebase(
                "sub-x/ses-1/anat/T1w/00000001.dcm",
                "sub-x/ses-1/anat/SC_T1w"
            ),
            "sub-x/ses-1/anat/SC_T1w/00000001.dcm"
        );
    }
}
