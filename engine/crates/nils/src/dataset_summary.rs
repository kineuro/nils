// SPDX-License-Identifier: AGPL-3.0-only

//! Wave 7a, the Data page (2026-10-09): one dataset whole, for the detail
//! under its card and the six steps on the card itself.
//! `GET /api/datasets/{name}/summary` answers what the dataset holds (its
//! subjects, visits and scans, how sure the sort is, the kinds of scan and
//! the body regions the sort found, its files and the refused ones) and
//! where it is: found, pseudonymised where it has originals, read, sorted,
//! body part, post-contrast, main scans, pictures and 3D views, each with
//! its counts, its state and when it last ran. Body part and post-contrast
//! are operations of their own (record 56, section 2, in `operations`), and
//! how sure the sort is counts only the questions the sort asks, by the one
//! definition of a look the card, the sources door, the scans doors and the
//! Grid count by (`certainty`). Every number is a count and nothing
//! quasi-identifying is answered, so the door is Data reading at plain, as
//! the sources door is. A stack of a sample sealed now is never counted by
//! kind or region for a caller who does not read sealed stacks (record 48),
//! as the scans door never lists it.
//!
//! The jobs of a dataset are the ones whose command line names it (`@name`
//! or a tree under it, `--dataset name`, `dataset:name`, `place originals
//! name`) or that carry its name and a date as the desk and a bring-in name
//! them, the ones its digests ran under, the sorts that judged its stacks,
//! the runs of the body-part and post-contrast models over its stacks and
//! the jobs a door queued for those steps (`for` dataset:name), and every
//! job queued after any of those: the next step of a chain, the pictures
//! and the pick run after a sort. `GET /api/jobs?dataset=name` lists the
//! same jobs, newest first.

use std::collections::BTreeSet;
use std::path::Path;

use nils_registry::Registry;
use nils_registry::place::{self, Place};
use nils_registry::store::{Error as StoreError, Store};
use serde_json::{Value, json};

use crate::grants::Access;
use crate::serve::Reply;

/// The kinds of job a dataset's steps are made of: what is looked through
/// for its jobs.
const KINDS: &[&str] = &[
    "pseudonymize",
    "digest",
    "ingest",
    "fingerprint",
    "classify",
    "pick",
    "pyramid",
    "preview",
    "originals",
    "pipeline",
];

/// How many of the newest jobs of those kinds are looked through.
pub(crate) const WINDOW: usize = 2000;

/// The axis the kinds of scan are read from, and the modifier that makes a
/// scan of any base a FLAIR, as a person names it (the MRI pack's axes).
const KIND_AXIS: &str = "base";
const FLAIR: (&str, &str) = ("modifier", "FLAIR");
/// The axis the body regions are read from (record 50: the body part's
/// coarse mode).
const REGION_AXIS: &str = "body_region";

fn failed(e: impl std::fmt::Display) -> Reply {
    Reply::error(500, e.to_string())
}

fn list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One job as the relation and the steps read it: no result, which may be
/// large and is not needed here.
#[derive(Debug, Clone)]
pub(crate) struct Light {
    pub(crate) id: i64,
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) state: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) args: Value,
    pub(crate) progress: Option<Value>,
}

impl Light {
    fn open(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "running" | "cancelling")
    }
}

/// The words of a job's command line, as a card reads them: what a door or
/// the keyboard queued, else the line it ran with the binary and the
/// registry taken off.
fn words(args: &Value) -> Vec<String> {
    let strings = |v: &Value| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    };
    strings(&args["queued"])
        .or_else(|| strings(&args["argv"]).map(|a| nils_registry::job::words_of(&a)))
        .unwrap_or_default()
}

/// Whether a command line names the dataset: by name, or by a path in its
/// folder, as the jobs door locates `@name` before it queues a digest.
pub(crate) fn names(words: &[String], dataset: &str, folders: &[String]) -> bool {
    let at = format!("@{dataset}");
    let under = format!("{at}/");
    let flag = format!("--dataset={dataset}");
    let label = format!("dataset:{dataset}");
    let inside = |w: &str| {
        folders.iter().any(|f| {
            w.strip_prefix(f.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    };
    let named = words.iter().enumerate().any(|(i, w)| {
        *w == at
            || w.starts_with(&under)
            || *w == flag
            || *w == label
            || (w == "--dataset" && words.get(i + 1).is_some_and(|n| n == dataset))
            || (w.starts_with('/') && inside(w))
    });
    let originals = words.first().is_some_and(|w| w == "place")
        && words.get(1).is_some_and(|w| w == "originals")
        && words.get(2).is_some_and(|w| w == dataset);
    named || originals
}

/// Whether a job's name is the dataset's with a day, as the desk and a
/// bring-in name a dataset's steps (`study-2026-10-09`), or the dataset's
/// own name. The day is read whole, so `study-2026-10-09` names `study`
/// and never `study-2026`.
pub(crate) fn named_for(name: Option<&str>, dataset: &str) -> bool {
    let Some(name) = name else {
        return false;
    };
    if name == dataset {
        return true;
    }
    let Some(day) = name
        .strip_prefix(dataset)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    let b = day.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// The value after a flag on a command line, as a job id.
fn flag_id(words: &[String], flag: &str) -> Option<i64> {
    words
        .iter()
        .position(|w| w == flag)
        .and_then(|i| words.get(i + 1))
        .and_then(|v| v.parse::<i64>().ok())
}

/// The newest jobs of the kinds a dataset's steps are made of.
fn newest(store: &mut Store, window: usize) -> Result<Vec<Light>, StoreError> {
    let kinds = KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, kind, name, state, {}, {}, {}, {} FROM {} WHERE kind IN ({kinds}) \
         ORDER BY id DESC LIMIT {window}",
        crate::text_of(store, "job", "started_at"),
        crate::text_of(store, "job", "finished_at"),
        crate::text_of(store, "job", "args"),
        crate::text_of(store, "job", "progress"),
        store.qualified("job"),
    );
    let json = |t: Option<&str>| t.and_then(|t| serde_json::from_str::<Value>(t).ok());
    store
        .query(&sql, &[])?
        .iter()
        .map(|r| {
            Ok(Light {
                id: r.int(0)?,
                kind: r.text(1)?.to_string(),
                name: r.opt_text(2)?.map(str::to_string),
                state: r.text(3)?.to_string(),
                started_at: r.opt_text(4)?.map(str::to_string),
                finished_at: r.opt_text(5)?.map(str::to_string),
                args: json(r.opt_text(6)?).unwrap_or(Value::Null),
                progress: json(r.opt_text(7)?),
            })
        })
        .collect()
}

/// The jobs a dataset's digests ran under, the sorts that judged its
/// stacks, and the runs of the operations' models over them.
fn seeds(store: &mut Store, sources: &[i64]) -> Result<BTreeSet<i64>, StoreError> {
    let mut out = BTreeSet::new();
    if sources.is_empty() {
        return Ok(out);
    }
    let ids = list(sources);
    out.extend(crate::operations::run_jobs(
        store,
        crate::operations::Scope::Sources(&ids),
    )?);
    for r in store.query(
        &format!(
            "SELECT DISTINCT job_id FROM {} WHERE source_id IN ({ids}) AND job_id IS NOT NULL",
            store.qualified("ingest_batch")
        ),
        &[],
    )? {
        out.insert(r.int(0)?);
    }
    for r in store.query(
        &format!(
            "SELECT DISTINCT cl.job_id FROM {} cl JOIN {} x ON x.id = cl.stack_id WHERE {}",
            store.qualified("classification"),
            store.qualified("stack"),
            crate::operations::held_by(store, "x", &ids),
        ),
        &[],
    )? {
        out.insert(r.int(0)?);
    }
    Ok(out)
}

/// A dataset's jobs among the newest `window`, newest first: the ones that
/// name it, its digests' and its sorts', and every one queued after any of
/// those (a chain's next step, the pictures and the pick run after a
/// sort). One pass from the oldest finds every link, since a job is only
/// ever queued after one that came before it.
pub(crate) fn jobs_of(
    store: &mut Store,
    dataset: &Place,
    window: usize,
) -> Result<Vec<Light>, StoreError> {
    let sources = crate::sources::source_ids(store, dataset)?;
    let mut related = seeds(store, &sources)?;
    // the folder as declared and as the disk resolves it, which a linked
    // tree makes two
    let mut folders = vec![dataset.path.trim_end_matches('/').to_string()];
    if let Ok(real) = std::fs::canonicalize(&dataset.path) {
        let real = real.display().to_string();
        if !folders.contains(&real) {
            folders.push(real);
        }
    }
    // a step's run a door queued for it, before its run names its stacks
    let queued_for = format!("dataset:{}", dataset.name);
    // a folder added again after its dataset was removed (2026-10-10): the
    // removed dataset's runs are its own, never the new one's, so where the
    // folder held a dataset before, only the runs since this one was added
    // are this one's
    let added_again = was_a_dataset_before(store, dataset, &folders)?;
    let mut jobs = newest(store, window)?;
    jobs.reverse();
    let mut out = Vec::new();
    for j in jobs {
        // a run made for another dataset is that one's, whatever folder it read
        if j.args["place_id"]
            .as_i64()
            .is_some_and(|id| id != dataset.id)
        {
            continue;
        }
        if added_again
            && j.started_at
                .as_deref()
                .is_some_and(|t| t < dataset.created_at.as_str())
        {
            continue;
        }
        let w = words(&j.args);
        let linked = |id: Option<i64>| id.is_some_and(|id| related.contains(&id));
        let mine = related.contains(&j.id)
            || names(&w, &dataset.name, &folders)
            || named_for(j.name.as_deref(), &dataset.name)
            || j.args["for"].as_str() == Some(queued_for.as_str())
            || linked(j.args["chain_before"].as_i64())
            || linked(j.args["after"].as_i64())
            || linked(flag_id(&w, "--classified"))
            || linked(flag_id(&w, "--after-sort"));
        if mine {
            related.insert(j.id);
            out.push(j);
        }
    }
    out.reverse();
    Ok(out)
}

/// Whether the dataset's folder or its name was another dataset's, removed
/// before this one was added: a source place retired on the same folder, as
/// declared or as the disk resolves it, or one that gave its name up to
/// this one (`place::retired_name`), whose runs name it still.
fn was_a_dataset_before(
    store: &mut Store,
    dataset: &Place,
    folders: &[String],
) -> Result<bool, StoreError> {
    let same = |path: &str| {
        let declared = path.trim_end_matches('/').to_string();
        let real = std::fs::canonicalize(path)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| declared.clone());
        folders.contains(&declared) || folders.contains(&real)
    };
    Ok(place::list(store)?.into_iter().any(|p| {
        p.id != dataset.id
            && p.role == place::Role::Source
            && p.retired_at.is_some()
            && (same(&p.path) || p.name == place::retired_name(&dataset.name, p.id))
    }))
}

/// The ids of a dataset's jobs, newest first, for the jobs door.
pub(crate) fn job_ids(store: &mut Store, dataset: &Place) -> Result<Vec<i64>, StoreError> {
    Ok(jobs_of(store, dataset, WINDOW)?
        .into_iter()
        .map(|j| j.id)
        .collect())
}

fn count(store: &mut Store, sql: &str) -> Result<i64, StoreError> {
    store.query(sql, &[])?[0].int(0)
}

/// What a dataset's found tree holds, as the step and the card count it
/// (2026-10-14): the files, their bytes where counted, whether the count
/// stopped short, and what counted them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Found {
    pub(crate) files: Option<i64>,
    pub(crate) bytes: Option<i64>,
    /// The probe's count stopped short of a large tree (it reads a page's
    /// worth of entries, never a walk of the archive): at least `files`.
    pub(crate) partial: bool,
    /// `read` (a read of the tree), `pseudonymise` (a whole pseudonymise
    /// run of the originals that ended done) or `probe`.
    pub(crate) counted_by: &'static str,
}

/// A tree as the probe last counted it: `(files, bytes, stopped short)`.
/// The probe is bounded (a page's worth of entries, never a walk of the
/// archive), so a large tree's count is at least `files`.
pub(crate) type Probed = (Option<i64>, Option<i64>, bool);

/// The newest finished read (a digest) of the tree NILS reads, which counts
/// every file of it where the probe is bounded, filed under that tree
/// (`anon_root`, canonical); a read of a folder under it walked less than
/// the tree and is not one. The files it saw (seen, unchanged ones too),
/// and their bytes where it measured every one (a read that found none
/// unchanged). An identified dataset's originals are walked by its
/// pseudonymise runs instead ([`Runs::walk`]).
pub(crate) fn walked(
    store: &mut Store,
    sources: &str,
    anon_root: &str,
) -> Result<Option<(i64, Option<i64>)>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT {} FROM {} b JOIN {} s ON s.id = b.source_id \
         WHERE b.source_id IN ({sources}) AND s.root_canonical = {} \
         AND (b.kind IS NULL OR b.kind = 'digest') \
         AND b.state = 'done' ORDER BY b.id DESC LIMIT 1",
        crate::text_of(store, "ingest_batch", "counts"),
        store.qualified("ingest_batch"),
        store.qualified("source"),
        d.param(1, nils_registry::schema::Type::Text),
    );
    let rows = store.query(&sql, &[nils_registry::store::Param::from(anon_root)])?;
    let Some(r) = rows.first() else {
        return Ok(None);
    };
    let counts: Value = r
        .opt_text(0)?
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null);
    Ok(counts["seen"].as_i64().map(|seen| {
        let every = counts["unchanged"].as_i64().unwrap_or(0) == 0;
        (seen, counts["bytes"].as_i64().filter(|_| every))
    }))
}

/// One pseudonymise run of a dataset's originals that ended, done or
/// cancelled, as its batch's counts (the run's report) say it. A run that
/// failed wrote no counts, and a dry run records no batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Run {
    /// A person or an update cancelled it: what it counted is what it
    /// decided before it stopped, and it saw at least `seen` originals.
    pub(crate) cancelled: bool,
    /// It read only the held files a map released or a person coded anyway
    /// (`--held`), never the whole of the originals.
    pub(crate) held_only: bool,
    pub(crate) seen: i64,
    pub(crate) written: i64,
    pub(crate) unchanged: i64,
    pub(crate) refused: i64,
    /// The size of every file it saw, whatever became of it.
    pub(crate) bytes: Option<i64>,
}

impl Run {
    /// A run from its batch's state and counts; none for a dry run's report
    /// or counts that are no run's report.
    pub(crate) fn of(state: &str, counts: &Value) -> Option<Run> {
        if counts["dry_run"].as_bool() == Some(true) {
            return None;
        }
        let files = &counts["files"];
        let n = |k: &str| files[k].as_i64().unwrap_or(0);
        Some(Run {
            cancelled: state == "cancelled",
            held_only: counts["held_only"].as_bool() == Some(true),
            seen: files["seen"].as_i64()?,
            written: n("written"),
            unchanged: n("unchanged"),
            refused: n("refused"),
            bytes: counts["bytes"].as_i64(),
        })
    }
}

/// The pseudonymise runs of a dataset that ended, done or cancelled, newest
/// first: the batches of kind pseudonymize filed under the tree NILS reads
/// (`anon_root`, canonical), where every run files its batch.
pub(crate) fn runs(
    store: &mut Store,
    sources: &str,
    anon_root: &str,
) -> Result<Vec<Run>, StoreError> {
    let d = store.dialect();
    let sql = format!(
        "SELECT b.state, {} FROM {} b JOIN {} s ON s.id = b.source_id \
         WHERE b.source_id IN ({sources}) AND s.root_canonical = {} \
         AND b.kind = 'pseudonymize' AND b.state IN ('done', 'cancelled') \
         ORDER BY b.id DESC",
        crate::text_of(store, "ingest_batch", "counts"),
        store.qualified("ingest_batch"),
        store.qualified("source"),
        d.param(1, nils_registry::schema::Type::Text),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[nils_registry::store::Param::from(anon_root)])? {
        let counts: Value = r
            .opt_text(1)?
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        if let Some(run) = Run::of(r.text(0)?, &counts) {
            out.push(run);
        }
    }
    Ok(out)
}

/// The runs a dataset's originals and copy are counted from (2026-10-17):
/// the newest whole run that ended, done or cancelled (`base`), whose
/// counts are facts about what it decided however it ended; the newest
/// whole run that ended done (`done`, the walk the found step counts); the
/// runs of the held files that ended after the base, newest first; and,
/// where the base was cancelled, those that ended between the run done
/// and the base. A run of the held files reads only those, so it is never
/// the base, nor the found step's walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Runs {
    pub(crate) base: Option<Run>,
    pub(crate) done: Option<Run>,
    pub(crate) held_after: Vec<Run>,
    pub(crate) held_between: Vec<Run>,
}

impl Runs {
    /// The runs that count, from every run that ended, newest first.
    pub(crate) fn of(newest_first: Vec<Run>) -> Runs {
        let mut out = Runs::default();
        for run in newest_first {
            if run.held_only {
                match out.base {
                    None => out.held_after.push(run),
                    Some(_) => out.held_between.push(run),
                }
                continue;
            }
            if out.base.is_none() {
                out.base = Some(run.clone());
            }
            if !run.cancelled {
                out.done = Some(run);
                break;
            }
        }
        out
    }

    /// The found step's walk of the originals: what the newest whole run
    /// that ended done saw, and the size of every file it saw.
    pub(crate) fn walk(&self) -> Option<(i64, Option<i64>)> {
        self.done.as_ref().map(|r| (r.seen, r.bytes))
    }
}

/// A tree's count as the doors answer it: its files, their bytes where
/// measured, and whether the count is a lower bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tree {
    pub(crate) files: i64,
    pub(crate) bytes: Option<i64>,
    pub(crate) partial: bool,
}

/// What an identified dataset's originals and copy count once a whole run
/// has ended (2026-10-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Copied {
    /// The originals: the most of what the base saw, what the found step
    /// counts and what the probe counted last (files that arrived since a
    /// run), with the bytes of the count that gave it. A lower bound where
    /// the base was cancelled, the found step's count is one, or a probe
    /// that stopped short counted more than any run saw.
    pub(crate) originals: Tree,
    /// The copy: the files the base wrote or found unchanged, and those the
    /// runs of the held files after it wrote. Where the base was cancelled,
    /// at least the more of what it decided and what the whole run done
    /// before it and the runs of the held files between them wrote or found
    /// unchanged.
    pub(crate) files: i64,
    /// The files the base and the runs of the held files after it refused,
    /// which are final: no run decides them again. Where the base was
    /// cancelled, at least the more of its and the run done before it's.
    pub(crate) refused: i64,
    /// The copy holds at least `files`: the base was cancelled, so the copy
    /// is a lower bound until a whole run ends done.
    pub(crate) partial: bool,
}

impl Copied {
    /// The pseudonymised step's counts: the copy, and the originals no run
    /// has decided yet, the files held now and the refused ones left out
    /// (each is said elsewhere, and a refused file is final). Where the
    /// originals' count is a lower bound, so is what waits, unless the
    /// copy's is one too: then what waits is not known.
    pub(crate) fn step(&self, held: i64) -> CopyCount {
        CopyCount {
            files: self.files,
            waiting: (self.originals.files - self.files - held - self.refused).max(0),
            partial: self.partial,
            waiting_partial: self.originals.partial,
        }
    }
}

/// The originals and the copy as the runs count them; none before a whole
/// run of the originals has ended.
pub(crate) fn copied(originals: Probed, found: &Found, runs: &Runs) -> Option<Copied> {
    let base = runs.base.as_ref()?;
    let found_files = found.files.unwrap_or(0);
    let probed = originals.0.unwrap_or(0);
    let by_runs = base.seen.max(found_files);
    let most = by_runs.max(probed);
    let bytes = if found.files == Some(most) {
        found.bytes
    } else if base.seen == most {
        base.bytes
    } else {
        originals.1
    };
    let sum = |runs: &[Run], of: fn(&Run) -> i64| runs.iter().map(of).sum::<i64>();
    // a cancelled run decided only some files, and a copy an earlier run
    // wrote stands: the copy holds at least the more of what it decided and
    // what the run done before it and the runs of the held files between
    // them did, and so many are refused at least (2026-10-17)
    let (decided, refused) = match (&runs.done, base.cancelled) {
        (Some(done), true) => (
            (base.written + base.unchanged)
                .max(done.written + done.unchanged + sum(&runs.held_between, |r| r.written)),
            base.refused
                .max(done.refused + sum(&runs.held_between, |r| r.refused)),
        ),
        _ => (base.written + base.unchanged, base.refused),
    };
    Some(Copied {
        originals: Tree {
            files: most,
            bytes,
            partial: base.cancelled || found.partial || (originals.2 && probed > by_runs),
        },
        files: decided + sum(&runs.held_after, |r| r.written),
        refused: refused + sum(&runs.held_after, |r| r.refused),
        partial: base.cancelled,
    })
}

/// What the doors count of a dataset's trees (2026-10-17): the probe's
/// counts as the place keeps them, the found tree's count, and for an
/// identified dataset the runs of its originals. The summary's found and
/// pseudonymised steps and the sources door's trees are all read from it,
/// so the card, its steps and the desk's Originals box say the same; the
/// place's own `probed` is never changed by it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Counted {
    /// The dataset has originals: they are its found tree, and its runs
    /// count the copy.
    pub(crate) originals: bool,
    /// The probe's counts of the originals and of the copy, as kept.
    pub(crate) probed: (Probed, Probed),
    pub(crate) found: Found,
    pub(crate) runs: Runs,
}

impl Counted {
    /// The counts from the probe's and the runs': the found tree's walk is
    /// the newest whole pseudonymise run that ended done for an identified
    /// dataset, else the newest finished read of the tree (`read`).
    pub(crate) fn new(
        originals: bool,
        probed: (Probed, Probed),
        runs: Runs,
        read: Option<(i64, Option<i64>)>,
    ) -> Counted {
        let found = match originals {
            true => found_of(probed.0, runs.walk(), true),
            false => found_of(probed.1, read, false),
        };
        Counted {
            originals,
            probed,
            found,
            runs,
        }
    }

    /// The originals and the copy as the runs count them; none for a
    /// dataset without originals, or before a whole run of them ended.
    pub(crate) fn copied(&self) -> Option<Copied> {
        if !self.originals {
            return None;
        }
        copied(self.probed.0, &self.found, &self.runs)
    }

    /// The pseudonymised step's counts and whether it is done. Once a whole
    /// run has ended they are the runs' (2026-10-17), and the step is done
    /// where the copy holds files and no file waits, is held or may wait
    /// uncounted, and the copy is no lower bound. Before that they are the
    /// probe's, as before: the copy and what the originals hold beyond it,
    /// done where nothing does.
    pub(crate) fn step(&self, held: i64) -> (CopyCount, bool) {
        match self.copied() {
            Some(c) => {
                let copy = c.step(held);
                let done = copy.files > 0
                    && copy.waiting == 0
                    && !copy.waiting_partial
                    && !copy.partial
                    && held == 0;
                (copy, done)
            }
            None => {
                let (originals, anon) = self.probed;
                let copy = copy_of((originals.0, originals.2), (anon.0, anon.2));
                (copy, copy.files > 0 && copy.waiting == 0)
            }
        }
    }

    /// The trees as a door answers them, `{originals, anon}` each with its
    /// path and counts (or null), counted as the summary's steps count
    /// them. An identified dataset's originals and copy, once a whole run of
    /// the originals has ended, are counted as its pseudonymised step counts
    /// them, the copy a lower bound where the step's is, and its bytes the
    /// probe's where it counted the copy whole, else none; a de-identified dataset's tree, where the probe stopped
    /// short of it, as its found step counts it, by its newest finished
    /// read. Any other tree keeps the probe's counts.
    pub(crate) fn overlay(&self, trees: &mut Value) {
        let set = |tree: &mut Value, files: Option<i64>, bytes: Option<i64>, partial: bool| {
            if tree.is_object() {
                tree["files"] = json!(files);
                tree["bytes"] = json!(bytes);
                tree["partial"] = json!(partial);
            }
        };
        if let Some(c) = self.copied() {
            let o = c.originals;
            set(&mut trees["originals"], Some(o.files), o.bytes, o.partial);
            let anon = self.probed.1;
            set(
                &mut trees["anon"],
                Some(c.files),
                anon.1.filter(|_| !anon.2),
                c.partial,
            );
        } else if !self.originals && self.probed.1.2 && self.found.counted_by == "read" {
            let f = &self.found;
            set(&mut trees["anon"], f.files, f.bytes, f.partial);
        }
    }
}

/// A dataset's trees counted for the doors: the probe's counts its place
/// keeps, and the runs or the read that walked its found tree, filed under
/// the tree NILS reads among its sources (`ids`).
pub(crate) fn counted(
    store: &mut Store,
    dataset: &Place,
    ids: &[i64],
) -> Result<Counted, StoreError> {
    let doc = dataset.dataset_doc();
    let probe = |t: &str| -> Probed {
        let tree = &doc["trees"][t];
        (
            tree["files"].as_i64(),
            tree["bytes"].as_i64(),
            tree["partial"].as_bool() == Some(true),
        )
    };
    let originals = !doc["trees"]["originals"].is_null();
    let probed = (probe("originals"), probe("anon"));
    let root = match (ids.is_empty(), dataset.tree_path("anon")) {
        (false, Some(anon)) => Some(
            std::fs::canonicalize(&anon)
                .unwrap_or(anon)
                .display()
                .to_string(),
        ),
        _ => None,
    };
    let sources = list(ids);
    let (runs, read) = match (root, originals) {
        (Some(root), true) => (Runs::of(runs(store, &sources, &root)?), None),
        (Some(root), false) => (Runs::default(), walked(store, &sources, &root)?),
        (None, _) => (Runs::default(), None),
    };
    Ok(Counted::new(originals, probed, runs, read))
}

/// The found tree's count: the newest finished walk of it once one has
/// run, else the probe's, which says where it stopped short. A walk that
/// measured not every file's bytes takes the probe's where the probe
/// counted the tree whole, and none where it stopped short.
pub(crate) fn found_of(probed: Probed, walk: Option<(i64, Option<i64>)>, originals: bool) -> Found {
    match walk {
        Some((files, bytes)) => Found {
            files: Some(files),
            bytes: bytes.or(probed.1.filter(|_| !probed.2)),
            partial: false,
            counted_by: if originals { "pseudonymise" } else { "read" },
        },
        None => Found {
            files: probed.0,
            bytes: probed.1,
            partial: probed.2 && probed.0.is_some(),
            counted_by: "probe",
        },
    }
}

/// The pseudonymised step's counts: the files of the copy and the files of
/// the originals not in it yet. `partial` says the copy holds at least
/// `files`, and `waiting_partial` that `waiting` is no whole count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CopyCount {
    pub(crate) files: i64,
    pub(crate) waiting: i64,
    pub(crate) partial: bool,
    pub(crate) waiting_partial: bool,
}

/// The step's counts before any whole run has ended: from the trees as the
/// probe last counted them, `(files, stopped short)` each. The probe is
/// bounded and stops short of a large tree, so `partial` says where the
/// copy's count did, and `waiting_partial` says `waiting` is at least that
/// where only the originals' count stopped short, and unknown where the
/// copy's did too.
pub(crate) fn copy_of(originals: (Option<i64>, bool), copied: (Option<i64>, bool)) -> CopyCount {
    let (o, c) = (originals.0.unwrap_or(0), copied.0.unwrap_or(0));
    let partial = copied.1 && copied.0.is_some();
    CopyCount {
        files: c,
        waiting: (o - c).max(0),
        partial,
        waiting_partial: partial || (originals.1 && originals.0.is_some()),
    }
}

/// `(label, scans)` by value of an axis over the dataset's stacks, most
/// first; for the kind axis a FLAIR is named as one whatever its base.
fn by_axis(
    store: &mut Store,
    sources: &str,
    axis: &str,
    sealed: Option<&str>,
) -> Result<Vec<(String, i64)>, StoreError> {
    let (class_axis, stack) = (
        store.qualified("classification_axis"),
        store.qualified("stack"),
    );
    let held = crate::operations::held_by(store, "x", sources);
    let label = if axis == KIND_AXIS {
        format!(
            "CASE WHEN EXISTS (SELECT 1 FROM {class_axis} m WHERE m.stack_id = a.stack_id \
             AND m.axis = '{}' AND m.value = '{}') THEN '{}' ELSE a.value END",
            FLAIR.0, FLAIR.1, FLAIR.1
        )
    } else {
        "a.value".to_string()
    };
    let hidden = sealed
        .map(|s| {
            format!(
                " AND NOT EXISTS (SELECT 1 FROM {s} sst WHERE sst.stack_id = a.stack_id AND sst.unsealed_at IS NULL)"
            )
        })
        .unwrap_or_default();
    let sql = format!(
        "SELECT k.label, COUNT(*) FROM (SELECT {label} AS label FROM {class_axis} a \
         JOIN {stack} x ON x.id = a.stack_id \
         WHERE a.axis = '{axis}' AND a.value IS NOT NULL AND a.value <> '' \
         AND {held}{hidden}) k GROUP BY k.label"
    );
    let mut out: Vec<(String, i64)> = store
        .query(&sql, &[])?
        .iter()
        .map(|r| Ok((r.text(0)?.to_string(), r.int(1)?)))
        .collect::<Result<_, StoreError>>()?;
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

/// How many of the stacks have a preview and how many a 3D view built
/// under the working place: a file each, looked at on a few threads, since
/// a working place may be a share.
fn made(working: &Path, stacks: &[i64]) -> (i64, i64) {
    if stacks.is_empty() {
        return (0, 0);
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    let chunk = stacks.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = stacks
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    let mut pictures = 0;
                    let mut views = 0;
                    for stack in part {
                        if crate::preview::path(working, *stack, false).is_file() {
                            pictures += 1;
                        }
                        if crate::pyramid::dir(working, *stack)
                            .join("manifest.json")
                            .is_file()
                        {
                            views += 1;
                        }
                    }
                    (pictures, views)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or((0, 0)))
            .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    })
}

/// A step's state: the open job's, else done or waiting.
fn state_of(open: Option<&Light>, done: bool) -> &'static str {
    match open.map(|j| j.state.as_str()) {
        Some("running" | "cancelling") => "running",
        Some(_) => "queued",
        None if done => "done",
        None => "waiting",
    }
}

/// A step of the dataset's chain (pseudonymise, read, sort, main scans):
/// the open job's state, else `failed` where the step's newest job failed,
/// so a run that stopped says so (2026-10-10: a pseudonymise run that failed
/// at once left the page as if nothing had happened), else done or waiting.
/// The job's own words are the jobs door's, never the summary's.
fn chain_state(open: Option<&Light>, newest: Option<&Light>, done: bool) -> &'static str {
    if open.is_none() && newest.is_some_and(|j| j.state == "failed") {
        return "failed";
    }
    state_of(open, done)
}

/// One step: its name, state and counts, and the job that ran it last (or
/// runs it now) with its times and its progress while it runs.
fn step(name: &str, state: &str, job: Option<&Light>, counts: Value) -> Value {
    let mut doc = json!({
        "step": name,
        "state": state,
        "job": job.map(|j| j.id),
        "started_at": job.and_then(|j| j.started_at.clone()),
        "finished_at": job.and_then(|j| j.finished_at.clone()),
        "progress": job.filter(|j| j.open()).and_then(|j| j.progress.clone()),
    });
    if let (Some(doc), Some(counts)) = (doc.as_object_mut(), counts.as_object()) {
        for (k, v) in counts {
            doc.insert(k.clone(), v.clone());
        }
    }
    doc
}

/// One kind of file a read set aside by policy, in the words of
/// `nils_dicom::set_aside_words`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetAside {
    pub(crate) kind: &'static str,
    pub(crate) words: &'static str,
    pub(crate) count: i64,
}

/// The names of the policy classes, the reasons a read sets a file aside.
fn policy_names() -> Vec<&'static str> {
    nils_dicom::QuarantineClass::ALL
        .into_iter()
        .filter(|c| c.is_policy_class())
        .map(|c| c.name())
        .collect()
}

/// The policy classes as a SQL list of their names.
fn policy_reasons() -> String {
    policy_names()
        .into_iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Record 56 (2026-10-11): the files of the sources the read set aside by
/// policy (a SOP class or a modality outside the batch's knobs), by kind in
/// plain words, most first. They ask nobody anything; the files the reader
/// could not take keep their review item and are not counted here.
pub(crate) fn set_aside_of(store: &mut Store, sources: &str) -> Result<Vec<SetAside>, StoreError> {
    let sql = format!(
        "SELECT reason, detail, COUNT(*) FROM {} WHERE source_id IN ({sources}) \
         AND status = 'quarantined' AND reason IN ({}) GROUP BY reason, detail",
        store.qualified("source_file"),
        policy_reasons()
    );
    let mut by: std::collections::BTreeMap<&'static str, (&'static str, i64)> =
        std::collections::BTreeMap::new();
    for r in store.query(&sql, &[])? {
        let Some(class) = nils_dicom::QuarantineClass::from_name(r.text(0)?) else {
            continue;
        };
        let (kind, words) = nils_dicom::set_aside_words(class, r.opt_text(1)?);
        by.entry(kind).or_insert((words, 0)).1 += r.int(2)?;
    }
    let mut out: Vec<SetAside> = by
        .into_iter()
        .map(|(kind, (words, count))| SetAside { kind, words, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(b.kind)));
    Ok(out)
}

/// The summary of one dataset, as `GET /api/datasets/{name}/summary`
/// answers it.
pub(crate) fn document(
    registry: &mut Registry,
    access: &Access,
    dataset: &Place,
) -> Result<Value, Reply> {
    // what a run of body part or post-contrast would be here, or why a door
    // would refuse it
    let plans = crate::operations::plans(registry).map_err(failed)?;
    let store = registry.store();
    let ids = crate::sources::source_ids(store, dataset).map_err(failed)?;
    let state = dataset.dataset["state"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    // its trees as the probe counted them and as the runs that walked them
    // did, the found and the pseudonymised steps read from one reading as
    // the sources door's trees are (2026-10-17)
    let counts = counted(store, dataset, &ids).map_err(failed)?;
    let has_originals = counts.originals;
    let jobs = jobs_of(store, dataset, WINDOW).map_err(failed)?;
    let newest_of = |kinds: &[&str]| jobs.iter().find(|j| kinds.contains(&j.kind.as_str()));
    let open_of = |kinds: &[&str]| {
        jobs.iter()
            .find(|j| j.open() && kinds.contains(&j.kind.as_str()))
    };
    let held = count(
        store,
        &format!(
            "SELECT COUNT(*) FROM {} WHERE place_id = {} AND state = 'held'",
            store.qualified("pseudonym_file"),
            dataset.id
        ),
    )
    .map_err(failed)?;

    // what the registry holds of it: nothing before its first read
    let window = nils_registry::cohort::built_window(store).map_err(failed)?;
    let sources = list(&ids);
    // the visits are none where nobody has built the session cache, and
    // a count of it where somebody has
    let (mut studies, mut stacks, mut sessions) = (0, 0, window.map(|_| 0));
    // Nima, 2026-10-12: a dataset holds stacks and files, never subjects;
    // the subjects counted in it are those with a stack its tree holds a
    // file of, as every reader counts them (`nils_registry::dataset_subjects`)
    let subjects = nils_registry::dataset_subjects::count(store, &ids).map_err(failed)?;
    let (mut read, mut refused, mut reads, mut classified) = (0, 0, 0, 0);
    // record 55 (2026-10-10): the files read as their instance's own, the
    // copies of an instance another dataset read (`known`) or this one holds
    // a file of already (`twice`), and the files held because the registry
    // holds their instance UID under another subject, study or series
    let (mut new, mut known, mut twice, mut same_instance) = (0, 0, 0, 0);
    // and the files a person let go out of the read, the files gone from the
    // tree since a read, and the open questions about the dataset's held
    // files (the duplicate policy's defaults, 2026-10-10)
    let (mut left_out, mut gone, mut identity_questions) = (0, 0, 0);
    let mut refused_batch: Option<i64> = None;
    // record 56 (2026-10-11): the files the read set aside by policy, which
    // ask nobody anything, by kind in plain words
    let mut set_aside: Vec<SetAside> = Vec::new();
    // and the subjects with no stack in it its set-aside files are of, a
    // fact about those files (2026-10-12)
    let mut set_aside_no_scans = nils_registry::dataset_subjects::SetAside::default();
    let mut certainty = crate::certainty::Certainty::default();
    let mut passes = 0;
    let (mut kinds, mut regions) = (Vec::new(), Vec::new());
    let mut last_digest: Option<(String, Option<String>)> = None;
    if !ids.is_empty() {
        let q = |t: &str| store.qualified(t);
        let (batch, stack, study, file, cache, instance, class) = (
            q("ingest_batch"),
            q("stack"),
            q("study"),
            q("source_file"),
            q("session_cache_study"),
            q("instance"),
            q("classification"),
        );
        // record 55 (2026-10-10): every scan its tree has a file of, whoever
        // read it first
        let in_tree = crate::operations::held_by(store, "x", &sources);
        let studies_in_tree = crate::operations::studies_held_by(store, "x", &sources);
        studies = count(
            store,
            &format!("SELECT COUNT(*) FROM {study} x WHERE {studies_in_tree}"),
        )
        .map_err(failed)?;
        stacks = count(
            store,
            &format!("SELECT COUNT(*) FROM {stack} x WHERE {in_tree}"),
        )
        .map_err(failed)?;
        if let Some(window) = window {
            // 2026-10-12: and none where the cache holds not all of its
            // dated studies yet (read since anything built it), so that a
            // dataset read after the cache was built does not say 0 visits
            let uncached = count(
                store,
                &format!(
                    "SELECT COUNT(*) FROM {study} x WHERE {studies_in_tree} \
                     AND COALESCE(x.date_filled, x.study_date) IS NOT NULL \
                     AND EXISTS (SELECT 1 FROM {series} se WHERE se.study_id = x.id) \
                     AND NOT EXISTS (SELECT 1 FROM {cache} scs WHERE scs.study_id = x.id \
                     AND scs.window_days = {window})",
                    series = store.qualified("series"),
                ),
            )
            .map_err(failed)?;
            sessions = if uncached > 0 {
                None
            } else {
                Some(
                    count(
                        store,
                        &format!(
                            "SELECT COUNT(DISTINCT scs.session_id) FROM {cache} scs \
                             JOIN {study} x ON x.id = scs.study_id WHERE {studies_in_tree} \
                             AND scs.window_days = {window}"
                        ),
                    )
                    .map_err(failed)?,
                )
            };
        }
        read = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) \
                 AND status IN ('ingested', 'duplicate')"
            ),
        )
        .map_err(failed)?;
        let same = nils_registry::review::SAME_INSTANCE_KIND;
        refused = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND (reason IS NULL OR reason NOT LIKE '{same}%')"
            ),
        )
        .map_err(failed)?;
        let drop = nils_registry::review::SAME_INSTANCE_DROP;
        left_out = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND reason = '{drop}'"
            ),
        )
        .map_err(failed)?;
        gone = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'gone'"
            ),
        )
        .map_err(failed)?;
        let item = store.qualified("review_item");
        let keys: Vec<String> = ids
            .iter()
            .map(|id| format!("ri.group_key LIKE 'source:{id}|%'"))
            .collect();
        identity_questions = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {item} ri WHERE ri.kind = '{same}' AND ri.status = 'open' AND ({})",
                keys.join(" OR ")
            ),
        )
        .map_err(failed)?;
        same_instance = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                 AND reason = '{same}'"
            ),
        )
        .map_err(failed)?;
        new = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'ingested'"
            ),
        )
        .map_err(failed)?;
        let copies = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {file} WHERE source_id IN ({sources}) AND status = 'duplicate'"
            ),
        )
        .map_err(failed)?;
        // one copy of each instance whose own file is no live file of this
        // dataset is known; every other copy is this dataset's second file
        known = count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT f.instance_id) FROM {file} f \
                 JOIN {instance} i ON i.id = f.instance_id LEFT JOIN {file} o ON o.id = i.source_file_id \
                 WHERE f.source_id IN ({sources}) AND f.status = 'duplicate' \
                 AND (o.id IS NULL OR o.source_id NOT IN ({sources}) OR o.status NOT IN ('ingested', 'duplicate'))"
            ),
        )
        .map_err(failed)?;
        twice = (copies - known).max(0);
        set_aside = set_aside_of(store, &sources).map_err(failed)?;
        set_aside_no_scans = nils_registry::dataset_subjects::set_aside_without_stacks(
            store,
            dataset.id,
            &ids,
            &policy_names(),
        )
        .map_err(failed)?;
        // the newest batch that refused a file a person may look at: none
        // set aside by policy, none held for its instance
        refused_batch = store
            .query(
                &format!(
                    "SELECT MAX(batch_id) FROM {file} WHERE source_id IN ({sources}) AND status = 'quarantined' \
                     AND (reason IS NULL OR (reason NOT LIKE '{same}%' AND reason NOT IN ({})))",
                    policy_reasons()
                ),
                &[],
            )
            .map_err(failed)?
            .first()
            .map(|r| r.opt_int(0))
            .transpose()
            .map_err(failed)?
            .flatten();
        reads = count(
            store,
            &format!(
                "SELECT COUNT(*) FROM {batch} WHERE source_id IN ({sources}) \
                 AND (kind IS NULL OR kind = 'digest')"
            ),
        )
        .map_err(failed)?;
        last_digest = store
            .query(
                &format!(
                    "SELECT {}, {} FROM {batch} WHERE source_id IN ({sources}) \
                     AND (kind IS NULL OR kind = 'digest') ORDER BY id DESC LIMIT 1",
                    crate::text_of(store, "ingest_batch", "started_at"),
                    crate::text_of(store, "ingest_batch", "finished_at"),
                ),
                &[],
            )
            .map_err(failed)?
            .first()
            .map(|r| -> Result<_, StoreError> {
                Ok((
                    r.opt_text(0)?.unwrap_or_default().to_string(),
                    r.opt_text(1)?.map(str::to_string),
                ))
            })
            .transpose()
            .map_err(failed)?;
        classified = count(
            store,
            &format!("SELECT COUNT(*) FROM {stack} x JOIN {class} cl ON cl.stack_id = x.id WHERE {in_tree}"),
        )
        .map_err(failed)?;
        // record 56: how sure the sort is counts what the sort asks, never
        // a question about body part or post-contrast, which are steps of
        // their own; counted as the card and the sources door count it, so
        // the two agree, and a pass's questions beside it
        let asks = crate::certainty::Asks::sort(store).map_err(failed)?;
        certainty = crate::certainty::of_sources(store, &asks, &sources, stacks).map_err(failed)?;
        let holds = crate::operations::Scope::Sources(&sources).holds(store);
        passes = crate::certainty::passes(store, &holds).map_err(failed)?;
        let sealed = (!crate::sealed::reads(access)).then(|| store.qualified("sealed_stack"));
        kinds = by_axis(store, &sources, KIND_AXIS, sealed.as_deref()).map_err(failed)?;
        regions = by_axis(store, &sources, REGION_AXIS, sealed.as_deref()).map_err(failed)?;
    }

    // the main scans of its subjects, as the picks summary door says them
    let picks = crate::pick_after::summary(store, dataset, None).map_err(failed)?;
    let (picked, borders) = picks["roles"]
        .as_object()
        .map(|roles| {
            roles.values().fold((0, 0), |a, r| {
                (
                    a.0 + r["picked"].as_i64().unwrap_or(0),
                    a.1 + r["review_items"].as_i64().unwrap_or(0),
                )
            })
        })
        .unwrap_or((0, 0));

    // the pictures and the 3D views, under the working place where one is bound
    let working = crate::pyramid::working_place(store, None).ok();
    let (pictures, views) = match &working {
        Some(w) if stacks > 0 => {
            let all = crate::preview::dataset_stacks(store, dataset).map_err(failed)?;
            made(Path::new(&w.path), &all)
        }
        _ => (0, 0),
    };

    let mut steps = Vec::new();
    // found: the tree NILS reads, or the originals of an identified dataset,
    // counted by the newest walk of it once one has run (2026-10-14): the
    // probe reads a page's worth of entries and says where it stopped short;
    // an identified dataset's walk is its newest whole pseudonymise run that
    // ended done, never a run of its held files alone (2026-10-17)
    let found_tree = if has_originals { "originals" } else { "anon" };
    let found_count = &counts.found;
    let found_files = found_count.files;
    let found_state = if state == "unknown" {
        "waiting"
    } else if found_files.unwrap_or(0) > 0 || reads > 0 {
        "done"
    } else {
        "waiting"
    };
    let mut found = json!({
        "step": "found",
        "state": found_state,
        "job": null,
        "started_at": null,
        "finished_at": dataset.probed_at.clone().unwrap_or_else(|| dataset.created_at.clone()),
        "progress": null,
        "files": found_files,
        "bytes": found_count.bytes,
        "partial": found_count.partial,
        "counted_by": found_count.counted_by,
    });
    found["tree"] = json!(found_tree);
    steps.push(found);
    if has_originals {
        // the copy and what waits, from the newest whole run that ended and
        // the runs of the held files after it, once one has (2026-10-17): the
        // probe stops short of a large tree, and two counts that stopped short
        // never made a large dataset's step done
        let (copy, done) = counts.step(held);
        let open = open_of(&["pseudonymize"]);
        steps.push(step(
            "pseudonymised",
            chain_state(open, newest_of(&["pseudonymize"]), done),
            open.or_else(|| newest_of(&["pseudonymize"])),
            json!({
                "files": copy.files,
                "waiting": copy.waiting,
                "held": held,
                "partial": copy.partial,
                "waiting_partial": copy.waiting_partial,
            }),
        ));
    }
    let set_aside_total: i64 = set_aside.iter().map(|k| k.count).sum();
    let open = open_of(&["digest", "ingest"]);
    let mut read_step = step(
        "read",
        chain_state(open, newest_of(&["digest", "ingest"]), reads > 0),
        open.or_else(|| newest_of(&["digest", "ingest"])),
        json!({
            "files": read, "new": new, "known": known, "twice": twice,
            "same_instance": same_instance, "left_out": left_out, "gone": gone,
            "refused": refused, "set_aside": set_aside_total, "reads": reads,
        }),
    );
    if read_step["job"].is_null()
        && let Some((started, finished)) = &last_digest
    {
        read_step["started_at"] = json!(started);
        read_step["finished_at"] = json!(finished);
    }
    steps.push(read_step);
    let unsorted = certainty.unsorted;
    let open = open_of(&["fingerprint", "classify"]);
    let sort_failed = newest_of(&["fingerprint", "classify"]).filter(|j| j.state == "failed");
    steps.push(step(
        "sorted",
        chain_state(
            open,
            newest_of(&["fingerprint", "classify"]),
            stacks > 0 && unsorted == 0,
        ),
        open.or(sort_failed).or_else(|| newest_of(&["classify"])),
        json!({
            "scans": classified, "of": stacks, "look": certainty.to_sort,
            "passes": passes, "unsorted": unsorted,
        }),
    ));
    // body part and post-contrast, the operations of their own, over its
    // stacks
    steps.extend(
        crate::operations::steps(
            store,
            crate::operations::Scope::Sources(&sources),
            &format!("dataset:{}", dataset.name),
            stacks,
            &plans,
            // counted as the card counts them (record 48: never by kind); a
            // cohort's steps leave a sealed sample out
            false,
        )
        .map_err(failed)?,
    );
    let open = open_of(&["pick"]);
    let picks_off = !place::picks_after_sort(&dataset.dataset);
    let mut main = step(
        "main_scans",
        if picks_off && open.is_none() {
            "off"
        } else {
            chain_state(
                open,
                newest_of(&["pick"]),
                picked > 0 || !picks["last_run"].is_null(),
            )
        },
        open.or_else(|| newest_of(&["pick"])),
        json!({"picked": picked, "borders": borders}),
    );
    if main["job"].is_null() && !picks["last_run"].is_null() {
        main["job"] = picks["last_run"]["job"].clone();
        main["finished_at"] = picks["last_run"]["finished_at"].clone();
    }
    steps.push(main);
    // the pictures of what a sort judged are a job of their own on the
    // pictures lane, queued as the sort ends and before its 3D views
    // (2026-10-17: never made in the sort's own run, so the sort is done
    // once it has judged): the step is that job's while it is queued or
    // runs, with its progress, and its counts are the previews under the
    // working place
    let open = open_of(&["preview"]);
    let mut picture_step = step(
        "pictures",
        if working.is_none() {
            "off"
        } else {
            state_of(open, pictures > 0)
        },
        open.or_else(|| newest_of(&["preview"])),
        json!({"made": pictures, "of": stacks}),
    );
    // a sort makes no picture in its own run any more
    picture_step["in_sort"] = json!(false);
    steps.push(picture_step);
    let open = open_of(&["pyramid"]);
    steps.push(step(
        "views",
        if working.is_none() {
            "off"
        } else {
            state_of(open, views > 0)
        },
        open.or_else(|| newest_of(&["pyramid"])),
        json!({"made": views, "of": stacks}),
    ));

    let pairs = |v: Vec<(String, i64)>, key: &str| -> Vec<Value> {
        v.into_iter()
            .map(|(label, n)| {
                let mut m = serde_json::Map::new();
                m.insert(key.to_string(), json!(label));
                m.insert("scans".to_string(), json!(n));
                Value::Object(m)
            })
            .collect()
    };
    // record 56 §5.3 ("3 proposed rule fixes from this read"): the
    // proposed rule changes of every pack that touch the dataset, by status
    let mut proposals = serde_json::Map::new();
    for s in nils_registry::rule_proposal::STATUSES {
        proposals.insert(s.to_string(), json!(0));
    }
    for p in nils_registry::rule_proposal::list(registry.store(), None).map_err(failed)? {
        if p.touches(&dataset.name)
            && let Some(n) = proposals.get_mut(&p.status)
        {
            *n = json!(n.as_i64().unwrap_or(0) + 1);
        }
    }
    Ok(json!({
        "dataset": dataset.name,
        "dataset_id": dataset.id,
        "detail": access.detail.name(),
        "state": state,
        "added_at": dataset.created_at,
        "subjects": subjects,
        "sessions": sessions,
        "studies": studies,
        "scans": stacks,
        "sure": certainty.sure,
        "need_a_look": certainty.to_sort,
        "unsorted": unsorted,
        "look_kinds": certainty.need_a_look,
        "identity_questions": identity_questions,
        "kinds": pairs(kinds, "kind"),
        "body_regions": pairs(regions, "region"),
        "files": {
            "found": found_files,
            "bytes": found_count.bytes,
            "found_partial": found_count.partial,
            "read": read,
            "new": new,
            "known": known,
            "twice": twice,
            "same_instance": same_instance,
            "left_out": left_out,
            "gone": gone,
            "refused": refused,
            "refused_batch": refused_batch,
            "set_aside": set_aside_total,
            "set_aside_kinds": set_aside
                .iter()
                .map(|k| json!({"kind": k.kind, "words": k.words, "count": k.count}))
                .collect::<Vec<_>>(),
            "set_aside_no_scans": {
                "subjects": set_aside_no_scans.subjects,
                "files": set_aside_no_scans.files,
            },
            "held": held,
        },
        "proposals": proposals,
        "pictures_place": working.map(|w| w.name),
        "steps": steps,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        CopyCount, Counted, Found, Light, Probed, Run, Runs, chain_state, copy_of, flag_id,
        found_of, named_for, names,
    };
    use serde_json::{Value, json};

    #[test]
    fn a_found_count_is_the_newest_walk_s_once_one_ran_and_says_where_a_probe_stopped_short() {
        // 2026-10-14: a production card said 49,836 files where its read
        // saw 1,112,026; the probe reads a page's worth of entries
        let probe = (Some(49_836), Some(16_000_000_000), true);
        assert_eq!(
            found_of(probe, None, false),
            Found {
                files: Some(49_836),
                bytes: Some(16_000_000_000),
                partial: true,
                counted_by: "probe"
            }
        );
        assert_eq!(
            found_of(probe, Some((1_112_026, Some(9))), false),
            Found {
                files: Some(1_112_026),
                bytes: Some(9),
                partial: false,
                counted_by: "read"
            }
        );
        // a walk that measured not every file takes no bytes from a probe
        // that stopped short, and a whole probe's where there is one
        assert_eq!(found_of(probe, Some((1_112_026, None)), false).bytes, None);
        assert_eq!(
            found_of((Some(4), Some(9), false), Some((4, None)), false).bytes,
            Some(9)
        );
        // an identified dataset's originals, walked by a pseudonymise run
        assert_eq!(
            found_of(probe, Some((7, None)), true).counted_by,
            "pseudonymise"
        );
        // a probe that counted the tree whole, or counted nothing, stopped
        // short of nothing
        assert!(!found_of((Some(4), Some(9), false), None, false).partial);
        assert!(!found_of((None, None, true), None, false).partial);
    }

    #[test]
    fn the_pseudonymised_step_says_where_its_counts_stopped_short() {
        // whole counts: the copy and what waits are exact
        assert_eq!(
            copy_of((Some(10), false), (Some(4), false)),
            CopyCount {
                files: 4,
                waiting: 6,
                partial: false,
                waiting_partial: false
            }
        );
        // the originals' count stopped short: at least that many wait
        let c = copy_of((Some(200_000), true), (Some(4), false));
        assert_eq!(
            (c.files, c.waiting, c.partial, c.waiting_partial),
            (4, 199_996, false, true)
        );
        // the copy's stopped short: at least that many copied, and what
        // waits is not known
        let c = copy_of((Some(200_000), true), (Some(200_000), true));
        assert_eq!(
            (c.files, c.waiting, c.partial, c.waiting_partial),
            (200_000, 0, true, true)
        );
        // a tree nobody counted stopped short of nothing
        assert_eq!(
            copy_of((None, true), (None, true)),
            CopyCount {
                files: 0,
                waiting: 0,
                partial: false,
                waiting_partial: false
            }
        );
    }

    /// A run's report as its batch keeps it: `[seen, written, unchanged,
    /// held, refused]`, and the bytes of every file it saw.
    fn report(held_only: bool, files: [i64; 5], bytes: i64) -> Value {
        let [seen, written, unchanged, held, refused] = files;
        json!({
            "dry_run": false,
            "held_only": held_only,
            "files": {
                "seen": seen, "written": written, "unchanged": unchanged,
                "held": held, "refused": refused, "skipped": 0,
            },
            "bytes": bytes,
        })
    }

    /// A whole run, or a run of the held files, that ended in `state`.
    fn run(state: &str, held_only: bool, files: [i64; 5], bytes: i64) -> Run {
        Run::of(state, &report(held_only, files, bytes)).unwrap()
    }

    /// An identified dataset's counts from its probe and its runs, newest
    /// first.
    fn identified(originals: Probed, copy: Probed, runs: Vec<Run>) -> Counted {
        Counted::new(true, (originals, copy), Runs::of(runs), None)
    }

    /// 2026-10-17: a pseudonymise run of a dataset of 30 million files ended
    /// with nothing held, and the step stayed waiting with no file copied:
    /// the run's own count of the trees after it stopped short of both
    /// (78,570 originals, and none of the copy in its two seconds). Counted
    /// from the run, the copy is what it wrote and nothing waits, since its
    /// refused files are final.
    #[test]
    fn a_whole_run_counts_the_copy_where_the_probe_stopped_short_of_both_trees() {
        let probe = (Some(78_570), Some(12_299_736_493), true);
        let copy = (Some(0), Some(0), true);
        let whole = run(
            "done",
            false,
            [30_689_981, 30_654_432, 0, 0, 35_549],
            4_915_477_070_489,
        );
        let c = identified(probe, copy, vec![whole.clone()]);
        // before: two counts that stopped short, and the step never done
        assert_eq!(
            copy_of((probe.0, probe.2), (copy.0, copy.2)),
            CopyCount {
                files: 0,
                waiting: 78_570,
                partial: true,
                waiting_partial: true
            }
        );
        let (step, done) = c.step(0);
        assert_eq!(
            step,
            CopyCount {
                files: 30_654_432,
                waiting: 0,
                partial: false,
                waiting_partial: false
            }
        );
        assert!(done);
        assert_eq!(chain_state(None, Some(&light(69, "done")), done), "done");
        // the found step counts what the run saw, with every file's bytes
        assert_eq!(
            c.found,
            Found {
                files: Some(30_689_981),
                bytes: Some(4_915_477_070_489),
                partial: false,
                counted_by: "pseudonymise"
            }
        );
        // and the sources door's trees say the same
        let mut trees = json!({
            "originals": {"path": "o", "files": 78_570, "bytes": 12_299_736_493i64,
                          "last_written": "w", "partial": true},
            "anon": {"path": "a", "files": 0, "bytes": 0, "last_written": null, "partial": true},
        });
        c.overlay(&mut trees);
        assert_eq!(
            trees,
            json!({
                "originals": {"path": "o", "files": 30_689_981, "bytes": 4_915_477_070_489i64,
                              "last_written": "w", "partial": false},
                "anon": {"path": "a", "files": 30_654_432, "bytes": null,
                         "last_written": null, "partial": false},
            })
        );
        // a copy the probe counted whole keeps the probe's bytes
        let c = identified(probe, (Some(9), Some(900), false), vec![whole]);
        let mut trees = json!({"originals": {}, "anon": {}});
        c.overlay(&mut trees);
        assert_eq!(
            trees["anon"],
            json!({"files": 30_654_432, "bytes": 900, "partial": false})
        );
    }

    /// A held file is in the step's `held`, never in `waiting`, and keeps
    /// the step from done; a refused file is in neither, since it is final.
    /// Files that arrived since the run, which the probe counts, wait.
    #[test]
    fn what_waits_leaves_out_the_held_and_the_refused_files() {
        let probe = (Some(100), Some(1_000), false);
        let c = identified(
            probe,
            (Some(80), Some(800), false),
            vec![run("done", false, [100, 80, 0, 15, 5], 1_000)],
        );
        let (step, done) = c.step(15);
        assert_eq!(
            step,
            CopyCount {
                files: 80,
                waiting: 0,
                partial: false,
                waiting_partial: false
            }
        );
        assert!(!done, "15 files are held");
        // before, the probe's count put the held and the refused in waiting
        assert_eq!(copy_of((Some(100), false), (Some(80), false)).waiting, 20);
        // a dataset whose refused files are its only ones not copied is done
        let c = identified(
            probe,
            (Some(95), Some(950), false),
            vec![run("done", false, [100, 95, 0, 0, 5], 1_000)],
        );
        let (step, done) = c.step(0);
        assert_eq!((step.files, step.waiting, done), (95, 0, true));
        // ten files arrived since and the probe counted them: they wait
        let c = identified(
            (Some(110), Some(1_100), false),
            (Some(95), Some(950), false),
            vec![run("done", false, [100, 95, 0, 0, 5], 1_000)],
        );
        let (step, done) = c.step(0);
        assert_eq!(
            (step.waiting, step.waiting_partial, done),
            (10, false, false)
        );
        let mut trees = json!({"originals": {}, "anon": {}});
        c.overlay(&mut trees);
        assert_eq!(
            trees["originals"],
            json!({"files": 110, "bytes": 1_100, "partial": false})
        );
        // a probe that stopped short past what the run saw: at least that many
        let c = identified(
            (Some(200_000), Some(1), true),
            (Some(95), Some(950), false),
            vec![run("done", false, [100, 95, 0, 0, 5], 1_000)],
        );
        let (step, done) = c.step(0);
        assert_eq!(
            (step.waiting, step.waiting_partial, done),
            (199_900, true, false)
        );
    }

    /// An update or a person may cancel a run: what it decided is counted,
    /// and neither the copy nor what waits is a whole count, so the step is
    /// not done until a whole run ends done.
    #[test]
    fn a_cancelled_whole_run_counts_what_it_decided_and_says_its_counts_are_partial() {
        let probe = (Some(100), Some(1_000), false);
        let cancelled = run("cancelled", false, [40, 30, 10, 0, 0], 400);
        let c = identified(probe, (Some(40), Some(400), false), vec![cancelled.clone()]);
        // no whole run ended done: the found step is the probe's
        assert_eq!(c.found.counted_by, "probe");
        let (step, done) = c.step(0);
        assert_eq!(
            step,
            CopyCount {
                files: 40,
                waiting: 60,
                partial: true,
                waiting_partial: true
            }
        );
        assert!(!done);
        let mut trees = json!({"originals": {}, "anon": {}});
        c.overlay(&mut trees);
        assert_eq!(
            trees["anon"],
            json!({"files": 40, "bytes": 400, "partial": true})
        );
        // a whole run done after the cancelled one is the step's again
        let c = identified(
            probe,
            (Some(98), Some(980), false),
            vec![run("done", false, [100, 0, 98, 0, 2], 1_000), cancelled],
        );
        let (step, done) = c.step(0);
        assert_eq!((step.files, step.partial, done), (98, false, true));
    }

    /// 2026-10-17: a run cancelled early decided few files, and the copy an
    /// earlier run wrote stands. The copy is at least the more of what the
    /// cancelled run decided and what the whole run done before it and the
    /// runs of the held files between them wrote or found unchanged, with
    /// what the runs of the held files after it wrote; the refused files the
    /// same way. It stays a lower bound until a whole run ends done.
    #[test]
    fn after_a_cancelled_run_the_copy_is_at_least_what_the_runs_before_it_wrote() {
        let probe = (Some(7), Some(70), true);
        let done_before = run("done", false, [100, 98, 0, 0, 2], 1_000);
        let early = run("cancelled", false, [40, 0, 40, 0, 0], 400);
        let c = identified(
            probe,
            (Some(0), Some(0), true),
            vec![early.clone(), done_before.clone()],
        );
        // the run done is the found step's walk
        assert_eq!(c.found.files, Some(100));
        assert_eq!(c.found.counted_by, "pseudonymise");
        let (step, done) = c.step(0);
        assert_eq!(
            step,
            CopyCount {
                files: 98,
                waiting: 0,
                partial: true,
                waiting_partial: true
            }
        );
        assert!(!done);
        let mut trees = json!({"originals": {}, "anon": {}});
        c.overlay(&mut trees);
        assert_eq!(
            trees,
            json!({
                "originals": {"files": 100, "bytes": 1_000, "partial": true},
                "anon": {"files": 98, "bytes": null, "partial": true},
            })
        );
        // the held files a run wrote between the two count on the side of
        // the run done, and those written after the cancelled run on both
        let whole = run("done", false, [100, 80, 0, 20, 0], 1_000);
        let between = run("done", true, [20, 15, 0, 5, 0], 200);
        let after = run("done", true, [5, 5, 0, 0, 0], 50);
        let c = identified(
            (Some(100), Some(1_000), false),
            (Some(100), Some(1_000), false),
            vec![after.clone(), early.clone(), between, whole],
        );
        let (step, done) = c.step(0);
        assert_eq!(
            step,
            CopyCount {
                files: 100,
                waiting: 0,
                partial: true,
                waiting_partial: true
            }
        );
        assert!(!done);
        // a cancelled run that decided more than the run done before it
        let late = run("cancelled", false, [100, 3, 96, 0, 1], 1_000);
        let c = identified(
            (Some(100), Some(1_000), false),
            (Some(99), Some(990), false),
            vec![late, done_before],
        );
        let (step, _) = c.step(0);
        assert_eq!((step.files, step.waiting, step.partial), (99, 0, true));
    }

    /// A run of the held files reads those alone: what it writes joins the
    /// copy and the found step still counts the whole run's originals.
    #[test]
    fn a_run_of_the_held_files_after_a_whole_run_adds_what_it_wrote() {
        let probe = (Some(100), Some(1_000), false);
        let whole = run("done", false, [100, 80, 0, 20, 0], 1_000);
        let held = run("done", true, [20, 15, 0, 5, 0], 200);
        let c = identified(
            probe,
            (Some(95), Some(950), false),
            vec![held, whole.clone()],
        );
        assert_eq!(c.found.files, Some(100), "never the held run's 20");
        assert_eq!(c.found.bytes, Some(1_000));
        let (step, done) = c.step(5);
        assert_eq!(
            step,
            CopyCount {
                files: 95,
                waiting: 0,
                partial: false,
                waiting_partial: false
            }
        );
        assert!(!done, "5 files are still held");
        // the rest written by another run of them, one refused: done
        let rest = run("done", true, [5, 4, 0, 0, 1], 50);
        let held = run("done", true, [20, 15, 0, 5, 0], 200);
        let c = identified(
            probe,
            (Some(99), Some(990), false),
            vec![rest, held, whole.clone()],
        );
        assert_eq!(
            c.step(0),
            (
                CopyCount {
                    files: 99,
                    waiting: 0,
                    partial: false,
                    waiting_partial: false
                },
                true
            )
        );
        // a run of the held files before the whole run adds nothing: the
        // whole run read those files again
        let before = run("done", true, [3, 3, 0, 0, 0], 30);
        let c = identified(probe, (Some(80), Some(800), false), vec![whole, before]);
        assert_eq!(c.step(20).0.files, 80);
    }

    /// Before a whole run has ended the step is the probe's, as before; a
    /// dry run records no batch, and a report that says it was one is none.
    #[test]
    fn with_no_whole_run_the_step_is_the_probe_s() {
        for (originals, copy) in [
            ((Some(10), None, false), (Some(4), None, false)),
            ((Some(200_000), None, true), (Some(4), None, false)),
            ((Some(200_000), None, true), (Some(200_000), None, true)),
            ((None, None, true), (None, None, true)),
        ] {
            let held_only = run("done", true, [4, 4, 0, 0, 0], 40);
            for runs in [vec![], vec![held_only]] {
                let c = identified(originals, copy, runs);
                let expected = copy_of((originals.0, originals.2), (copy.0, copy.2));
                assert_eq!(
                    c.step(0),
                    (expected, expected.files > 0 && expected.waiting == 0)
                );
                assert_eq!(c.found, found_of(originals, None, true));
                let mut trees = json!({"originals": {"files": 1}, "anon": {"files": 2}});
                c.overlay(&mut trees);
                assert_eq!(
                    trees,
                    json!({"originals": {"files": 1}, "anon": {"files": 2}})
                );
            }
        }
        let mut dry = report(false, [4, 4, 0, 0, 0], 40);
        dry["dry_run"] = json!(true);
        assert_eq!(Run::of("done", &dry), None);
        assert_eq!(Run::of("done", &json!({"files": {}})), None);
    }

    /// A de-identified dataset's tree, where the probe stopped short, is
    /// counted on the sources door as its found step counts it: by the
    /// newest finished read.
    #[test]
    fn a_de_identified_tree_the_probe_stopped_short_of_is_counted_by_its_read() {
        let short = (Some(49_836), Some(16_000_000_000), true);
        let c = Counted::new(
            false,
            ((None, None, false), short),
            Runs::default(),
            Some((1_112_026, Some(9))),
        );
        let mut trees = json!({"originals": null, "anon": {"files": 49_836, "partial": true}});
        c.overlay(&mut trees);
        assert_eq!(
            trees,
            json!({"originals": null, "anon": {"files": 1_112_026, "bytes": 9, "partial": false}})
        );
        // a probe that counted it whole, or no read yet: the probe's
        let whole = (Some(4), Some(9), false);
        for c in [
            Counted::new(
                false,
                ((None, None, false), whole),
                Runs::default(),
                Some((3, None)),
            ),
            Counted::new(false, ((None, None, false), short), Runs::default(), None),
        ] {
            let mut trees = json!({"originals": null, "anon": {"files": 4}});
            c.overlay(&mut trees);
            assert_eq!(trees, json!({"originals": null, "anon": {"files": 4}}));
        }
    }

    fn w(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_command_line_names_a_dataset_by_its_tree_its_flag_its_label_or_its_folder() {
        let folders = ["/srv/in/study".to_string()];
        let names = |line: &str, dataset: &str| names(&w(line), dataset, &folders);
        assert!(names("digest @study --name study-2026-10-09", "study"));
        assert!(names("pseudonymize @study/originals", "study"));
        assert!(names("pick run --dataset study", "study"));
        assert!(names("pick run --dataset=study", "study"));
        assert!(names("preview dataset:study", "study"));
        assert!(names("place originals study --purge --why x", "study"));
        // the jobs door locates @study before it queues a digest
        assert!(names(
            "digest /srv/in/study/derivatives/dcm-anon --name x",
            "study"
        ));
        assert!(names("digest /srv/in/study", "study"));
        assert!(!names(
            "digest /srv/in/study-big/derivatives/dcm-anon",
            "study"
        ));
        assert!(!names("digest @study-big", "study"));
        assert!(!names("pick run --dataset study-big", "study"));
        assert!(!names("classify --pack mri", "study"));
    }

    #[test]
    fn a_name_with_a_whole_day_is_the_dataset_s() {
        assert!(named_for(Some("study-2026-10-09"), "study"));
        assert!(named_for(Some("study"), "study"));
        assert!(!named_for(Some("study-2026-10-09"), "study-2026"));
        assert!(!named_for(Some("study-big-2026-10-09"), "study"));
        assert!(!named_for(Some("pictures after job 4"), "study"));
        assert!(!named_for(None, "study"));
    }

    #[test]
    fn a_flag_s_value_is_read_as_an_id() {
        assert_eq!(
            flag_id(
                &w("pyramid build --classified 42 --place w"),
                "--classified"
            ),
            Some(42)
        );
        assert_eq!(flag_id(&w("pick run --after-sort x"), "--after-sort"), None);
        assert_eq!(flag_id(&w("pick run"), "--after-sort"), None);
    }

    /// A job of a step as the summary reads it, in one state.
    fn light(id: i64, state: &str) -> Light {
        Light {
            id,
            kind: "pseudonymize".into(),
            name: None,
            state: state.into(),
            started_at: None,
            finished_at: None,
            args: serde_json::json!({}),
            progress: None,
        }
    }

    /// 2026-10-10: a pseudonymise run that failed at once left the step
    /// waiting, as if nothing had happened; the chain's step says failed.
    #[test]
    fn a_chain_step_whose_newest_job_failed_says_so() {
        let failed = light(154, "failed");
        let done = light(150, "done");
        let running = light(155, "running");
        assert_eq!(chain_state(None, Some(&failed), false), "failed");
        // a failure is said even where an earlier run had done the step
        assert_eq!(chain_state(None, Some(&failed), true), "failed");
        // a run of it going again is what the step is
        assert_eq!(
            chain_state(Some(&running), Some(&running), false),
            "running"
        );
        // the newest that ended well, or none, is done or waiting as before
        assert_eq!(chain_state(None, Some(&done), true), "done");
        assert_eq!(chain_state(None, Some(&done), false), "waiting");
        assert_eq!(chain_state(None, None, false), "waiting");
    }
}
