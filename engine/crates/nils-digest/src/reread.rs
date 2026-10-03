// SPDX-License-Identifier: AGPL-3.0-only

//! `nils digest --reread`: read again only the files of the MR series of
//! some manufacturers, found in the registry rather than by a walk.
//!
//! The 2026-09-28 sequence research added fields that GE and Siemens XA
//! series need (GE's private sequence and ASL elements, XA's
//! PulseSequenceName) and that a registry digested before holds nowhere.
//! Every file has to be opened again for them, but only those vendors'
//! files, and the registry already says which files those are: each
//! instance's own `source_file` row, under a series of modality MR whose
//! study names one of the manufacturers. This stage stands where the walker
//! and the resume check stand, and hands the parsers each such file with its
//! instance beside it, as `--restart` does, so the writer folds what it reads
//! into the rows the registry holds under the rule every run follows: a null
//! is filled, and two files that disagree keep the smaller value.
//!
//! Every file of such a series is read, and not a sample of it. A series
//! row keeps the smaller of two disagreeing values and `series_private`
//! lists the element as varied, and both hold only if every file is seen:
//! GE writes its ASL's two passes into one series with different contrast
//! techniques, and a magnitude and a phase with different private image
//! types.
//!
//! A manufacturer is matched in one of two ways. `--reread` compares it
//! trimmed and without case, which is what a vendor that spells itself one
//! way wants. `--reread-exact` compares it trimmed and with case, because
//! Siemens spells its XA11 and XA20 scanners `Siemens` and every older one
//! `SIEMENS`: a match without case would take the whole of the older fleet
//! with the few XA files. The registry does not carry SoftwareVersions, so
//! the spelling is the handle the registry has.
//!
//! The target series are resolved once, at the start, and the files are then
//! read series by series: each page is one series' instances, through the
//! index on `instance.series_id`, joined to their own `source_file` rows by
//! id, after the last file id the page before handed on, so a page costs
//! what its series holds and never a pass over the archive.
//!
//! A re-read that was stopped, failed or killed is continued by the next
//! one: the writer sets `source_file.batch_id` on every file it files, so a
//! file whose row carries the batch of an unfinished re-read before this one
//! (with nothing but such re-reads between them, on the same private
//! elements) was read already, and is counted as unchanged instead of read
//! again. `--restart` beside `--reread` reads everything again.
//!
//! A run like this walks nothing, so it marks nothing gone; and the stacks of
//! the series it names are marked for the fingerprint to derive again, since
//! their row counts did not move and the fingerprint would otherwise take
//! them for fresh.
//!
//! Three choices narrow or widen it (the 2026-10-03 fingerprint fields).
//! `--reread-every` takes every MR series whatever its manufacturer, for a
//! field every vendor writes. `--reread-one` reads one file of each series
//! instead of all of them: enough for a value the series holds once (a
//! series column, a private element that holds still inside the series), at
//! a cost of one file a series rather than every image, and the files are
//! found a chunk of series at a time rather than a query per series. It
//! cannot see that two files of a series disagree, so it is not for a value
//! that varies inside one. `--reread-missing` takes only the series whose row
//! has no SamplesPerPixel (0028,0002), which every image writes: the series a
//! binary before registry schema 79 read, so a re-read run again, or over a
//! registry that has since digested new series, reads only what still lacks
//! the new columns.

use std::path::Path;

use crossbeam_channel::Sender;
use nils_registry::schema::Type;
use nils_registry::store::{Error, Param, Row, Store};

use crate::batch::{Prior, Task};
use crate::cancel::Cancel;
use crate::knobs::Settings;
use crate::progress::Progress;
use crate::report::Counts;
use crate::resume::status;
use crate::walk::mtime_ns_of;

/// How many `source_file` rows one query reads at most.
pub const PAGE: usize = 10_000;

/// The manufacturers as the query compares them: trimmed and lower case,
/// empty ones dropped, each once.
pub fn folded(manufacturers: &[String]) -> Vec<String> {
    let mut out: Vec<String> = manufacturers
        .iter()
        .map(|m| m.trim().to_lowercase())
        .filter(|m| !m.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The manufacturers as an exact match compares them: trimmed, case kept,
/// empty ones dropped, each once.
pub fn exact(manufacturers: &[String]) -> Vec<String> {
    let mut out: Vec<String> = manufacturers
        .iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Which studies' manufacturers a re-read takes: some compared without
/// case, some with, or every one; and whether only the series that still
/// lack the columns a binary before registry schema 79 did not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub folded: Vec<String>,
    pub exact: Vec<String>,
    /// Every MR series, whatever its study's manufacturer says.
    pub every: bool,
    /// Only the series whose row has no SamplesPerPixel.
    pub missing: bool,
}

impl Selection {
    pub fn new(any_case: &[String], exact_case: &[String]) -> Selection {
        Selection {
            folded: folded(any_case),
            exact: exact(exact_case),
            every: false,
            missing: false,
        }
    }

    /// Every MR series, or not.
    pub fn every(mut self, every: bool) -> Selection {
        self.every = every;
        self
    }

    /// Only the series still missing what schema 79 added, or not.
    pub fn missing(mut self, missing: bool) -> Selection {
        self.missing = missing;
        self
    }

    pub fn of(settings: &Settings) -> Selection {
        Selection::new(&settings.reread, &settings.reread_exact)
            .every(settings.reread_every)
            .missing(settings.reread_missing)
    }

    pub fn is_empty(&self) -> bool {
        !self.every && self.folded.is_empty() && self.exact.is_empty()
    }

    /// The condition on `sy.manufacturer`, its parameters numbered from
    /// `first`, and the parameters in the order they stand.
    fn condition(&self, store: &Store, first: usize) -> (String, Vec<Param>) {
        if self.every {
            return ("(1 = 1)".to_string(), Vec::new());
        }
        let d = store.dialect();
        let mut n = first;
        let mut list = |k: usize| -> Vec<String> {
            (0..k)
                .map(|_| {
                    n += 1;
                    d.param(n - 1, Type::Text)
                })
                .collect()
        };
        let mut parts = Vec::new();
        if !self.folded.is_empty() {
            let l = list(self.folded.len());
            parts.push(format!(
                "LOWER(TRIM(sy.manufacturer)) IN ({})",
                l.join(", ")
            ));
        }
        if !self.exact.is_empty() {
            let l = list(self.exact.len());
            parts.push(format!("TRIM(sy.manufacturer) IN ({})", l.join(", ")));
        }
        let params = self
            .folded
            .iter()
            .chain(&self.exact)
            .map(|m| Param::from(m.as_str()))
            .collect();
        (format!("({})", parts.join(" OR ")), params)
    }
}

/// The series a re-read reads, as a subquery over `series` and `study`,
/// with its parameters numbered from `first`. `missing` narrows it to the
/// series whose row has no SamplesPerPixel, where the selection asks; the
/// fingerprints a run marks stale are those of the whole selection, since by
/// then the series it read have the value.
fn series_of(
    store: &Store,
    selection: &Selection,
    first: usize,
    missing: bool,
) -> (String, Vec<Param>) {
    let (cond, params) = selection.condition(store, first);
    let missing = match missing && selection.missing {
        true => " AND se.samples_per_pixel IS NULL",
        false => "",
    };
    (
        format!(
            "SELECT se.id FROM {} AS se JOIN {} AS sy ON sy.id = se.study_id \
             WHERE se.modality = 'MR' AND {cond}{missing}",
            store.qualified("series"),
            store.qualified("study"),
        ),
        params,
    )
}

/// The ids of the series a re-read reads, in order: resolved once, over
/// `series` and `study` alone, before any file is read.
pub fn targets(store: &mut Store, selection: &Selection) -> Result<Vec<i64>, Error> {
    if selection.is_empty() {
        return Ok(Vec::new());
    }
    let (sql, params) = series_of(store, selection, 1, true);
    let rows = store.query(&format!("{sql} ORDER BY se.id"), &params)?;
    rows.iter().map(|r| r.int(0)).collect()
}

/// One page: a series' own ingested files of the source after a file id,
/// in id order. Its parameters are the series, the source, the last id and
/// the limit. The series' instances are looked up first, through the index
/// on `instance.series_id`, and materialised, so that no planner walks
/// `source_file` in id order looking for them; each is then joined to its
/// `source_file` row by primary key. A page costs what one series holds.
pub fn page_sql(store: &Store) -> String {
    let d = store.dialect();
    format!(
        "WITH own AS MATERIALIZED (\
           SELECT i.id AS instance_id, i.source_file_id AS file_id FROM {instance} AS i \
           WHERE i.series_id = {} AND i.source_file_id IS NOT NULL) \
         SELECT f.id, f.path, f.dir, f.size, f.mtime_ns, f.instance_id, f.batch_id \
         FROM own JOIN {files} AS f ON f.id = own.file_id AND f.instance_id = own.instance_id \
         WHERE f.source_id = {} AND f.id > {} AND f.status = '{ingested}' \
         ORDER BY f.id LIMIT {}",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Int),
        files = store.qualified("source_file"),
        instance = store.qualified("instance"),
        ingested = status::INGESTED,
    )
}

/// How many series one query of a one-file re-read covers.
pub const CHUNK: usize = 500;

/// One query of a one-file re-read: for each series of a chunk, the lowest
/// file id any of its instances names as its own, with that file's row. Its
/// parameters are the chunk's series ids, `n` of them. One aggregate over the
/// series' instances through the index on `instance.series_id`, then a
/// primary-key join, so a chunk costs what its series hold and no query is
/// made per series. The file's source and status come back unfiltered: the
/// caller reads a file of its own source that is ingested, asks the series'
/// first page for one whose lowest file is of its source and not ingested,
/// and leaves a series whose lowest file is another source's to that
/// source's run.
pub fn first_sql(store: &Store, n: usize) -> String {
    let d = store.dialect();
    let list = (1..=n)
        .map(|i| d.param(i, Type::Int))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "WITH own AS MATERIALIZED (\
           SELECT i.series_id, MIN(i.source_file_id) AS file_id FROM {instance} AS i \
           WHERE i.series_id IN ({list}) AND i.source_file_id IS NOT NULL \
           GROUP BY i.series_id) \
         SELECT f.id, f.path, f.dir, f.size, f.mtime_ns, f.instance_id, f.batch_id, \
                own.series_id, f.source_id, f.status \
         FROM own JOIN {files} AS f ON f.id = own.file_id \
         WHERE f.instance_id IS NOT NULL ORDER BY f.id",
        files = store.qualified("source_file"),
        instance = store.qualified("instance"),
    )
}

/// A file a page names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub id: i64,
    pub path: String,
    pub dir: String,
    pub size: i64,
    pub mtime_ns: i64,
    pub instance_id: Option<i64>,
    pub batch_id: i64,
}

impl File {
    fn of(r: &Row) -> Result<File, Error> {
        Ok(File {
            id: r.int(0)?,
            path: r.text(1)?.to_string(),
            dir: r.text(2)?.to_string(),
            size: r.int(3)?,
            mtime_ns: r.int(4)?,
            instance_id: r.opt_int(5)?,
            batch_id: r.int(6)?,
        })
    }
}

/// The target series' files, page by page: each series from its first file
/// on, then the next series; or, for a one-file re-read, the first file of
/// each series of a chunk, a chunk a page.
pub struct Pages {
    series: Vec<i64>,
    at: usize,
    after: i64,
    source_id: i64,
    page: usize,
    sql: String,
    one: bool,
}

impl Pages {
    pub fn new(store: &Store, source_id: i64, series: Vec<i64>, page: usize) -> Pages {
        Pages {
            series,
            at: 0,
            after: 0,
            source_id,
            page: page.max(1),
            sql: page_sql(store),
            one: false,
        }
    }

    /// One file of each series rather than every file, `chunk` series a
    /// page.
    pub fn one_each(store: &Store, source_id: i64, series: Vec<i64>, chunk: usize) -> Pages {
        Pages {
            one: true,
            ..Pages::new(store, source_id, series, chunk)
        }
    }

    /// The next page with a file in it, or `None` when every series is done.
    pub fn next(&mut self, store: &mut Store) -> Result<Option<Vec<File>>, Error> {
        if self.one {
            return self.next_chunk(store);
        }
        while let Some(&series) = self.series.get(self.at) {
            let rows = store.query(
                &self.sql,
                &[
                    Param::Int(series),
                    Param::Int(self.source_id),
                    Param::Int(self.after),
                    Param::Int(self.page as i64),
                ],
            )?;
            let files = rows.iter().map(File::of).collect::<Result<Vec<_>, _>>()?;
            if files.len() < self.page {
                self.at += 1;
                self.after = 0;
            } else if let Some(last) = files.last() {
                self.after = last.id;
            }
            if !files.is_empty() {
                return Ok(Some(files));
            }
        }
        Ok(None)
    }

    /// The next chunk's first files, or `None` when every series is done.
    fn next_chunk(&mut self, store: &mut Store) -> Result<Option<Vec<File>>, Error> {
        while self.at < self.series.len() {
            let end = (self.at + self.page).min(self.series.len());
            let chunk = &self.series[self.at..end];
            self.at = end;
            let params: Vec<Param> = chunk.iter().map(|&s| Param::Int(s)).collect();
            let rows = store.query(&first_sql(store, chunk.len()), &params)?;
            let mut files = Vec::with_capacity(rows.len());
            let mut elsewhere: Vec<i64> = Vec::new();
            for r in &rows {
                if r.int(8)? != self.source_id {
                    continue;
                }
                if r.text(9)? == status::INGESTED {
                    files.push(File::of(r)?);
                } else {
                    elsewhere.push(r.int(7)?);
                }
            }
            // A series whose lowest file is no longer read as ingested: its
            // first page, which looks past such a file, gives the first one
            // that is.
            for series in elsewhere {
                let rows = store.query(
                    &self.sql,
                    &[
                        Param::Int(series),
                        Param::Int(self.source_id),
                        Param::Int(0),
                        Param::Int(1),
                    ],
                )?;
                for r in &rows {
                    files.push(File::of(r)?);
                }
            }
            files.sort_by_key(|f| f.id);
            if !files.is_empty() {
                return Ok(Some(files));
            }
        }
        Ok(None)
    }
}

/// The first batch of the re-read this one continues, if any: walking back
/// from this batch over the source's earlier ones, every batch that is a
/// re-read on the same private elements and did not end `done`. A file
/// whose row carries that batch or a later one was read already. Anything
/// else in between (an ordinary digest, a finished re-read, other private
/// elements) ends the chain.
pub fn continues(
    store: &mut Store,
    source_id: i64,
    batch_id: i64,
    settings: &Settings,
) -> Result<Option<i64>, Error> {
    if settings.restart {
        return Ok(None);
    }
    let private = &settings.config()["private"];
    let d = store.dialect();
    let sql = format!(
        "SELECT id, state, config FROM {} WHERE source_id = {} AND id < {} ORDER BY id DESC",
        store.qualified("ingest_batch"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
    );
    let mut floor = None;
    store.query_stream(&sql, &[Param::Int(source_id), Param::Int(batch_id)], |r| {
        let config: serde_json::Value =
            serde_json::from_str(r.text(2).unwrap_or("{}")).unwrap_or_default();
        let named = |k: &str| config[k].as_array().is_some_and(|a| !a.is_empty());
        let flag = |k: &str| config[k].as_bool().unwrap_or(false);
        let reread = named("reread") || named("reread_exact") || flag("reread_every");
        let open = r.text(1).map(|s| s != "done").unwrap_or(false);
        // a re-read of one file a series is not one of every file, and the
        // other way round: what one read, the other did not
        let same_reach = flag("reread_one") == settings.reread_one;
        if reread && open && same_reach && &config["private"] == private {
            floor = Some(r.int(0)?);
            Ok(true)
        } else {
            Ok(false)
        }
    })?;
    Ok(floor)
}

/// Feed the parsers every file of the source that is its instance's own and
/// belongs to a target series, series by series, until the rows run out or
/// a stop is asked. A file a re-read this one continues read already is
/// counted as unchanged and not read. A file that is no longer where the
/// registry says is a walk error, which the report counts.
#[allow(clippy::too_many_arguments)]
pub fn feed(
    mut store: Store,
    source_id: i64,
    batch_id: i64,
    root: &Path,
    settings: &Settings,
    tx: &Sender<Task>,
    progress: &Progress,
    cancel: &Cancel,
) -> Result<Counts, Error> {
    let mut counts = Counts::default();
    let selection = Selection::of(settings);
    let series = targets(&mut store, &selection)?;
    let floor = continues(&mut store, source_id, batch_id, settings)?;
    let mut pages = match settings.reread_one {
        true => Pages::one_each(&store, source_id, series, CHUNK),
        false => Pages::new(&store, source_id, series, PAGE),
    };
    while !cancel.stop() {
        let Some(files) = pages.next(&mut store)? else {
            break;
        };
        for f in files {
            if floor.is_some_and(|floor| f.batch_id >= floor) {
                counts.unchanged();
                progress.unchanged();
                continue;
            }
            let path = root.join(&f.path);
            let task = match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => {
                    let size = m.len();
                    let mtime_ns = mtime_ns_of(&m);
                    Task::Parse {
                        path,
                        rel: f.path,
                        dir: f.dir,
                        size,
                        mtime_ns,
                        prior: Some(Prior {
                            instance_id: f.instance_id,
                            changed: f.size != size as i64 || f.mtime_ns != mtime_ns,
                        }),
                    }
                }
                Ok(_) => Task::WalkError {
                    error: format!("{}: no longer a file", f.path),
                },
                Err(e) => Task::WalkError {
                    error: format!("{}: {e}", f.path),
                },
            };
            if tx.send(task).is_err() || cancel.stop() {
                return Ok(counts);
            }
        }
    }
    Ok(counts)
}

/// Mark the fingerprints of the series a run read again as stale, so that
/// the next fingerprint derives them from what the files said this time.
pub fn stale_fingerprints(store: &mut Store, selection: &Selection) -> Result<u64, Error> {
    if selection.is_empty() {
        return Ok(0);
    }
    let (series, params) = series_of(store, selection, 1, false);
    let sql = format!(
        "UPDATE {} SET fingerprint_revision = 0 WHERE series_id IN ({series})",
        store.qualified("stack_fingerprint"),
    );
    store.execute(&sql, &params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manufacturer_is_compared_trimmed_and_folded_once() {
        assert_eq!(
            folded(&[
                " GE MEDICAL SYSTEMS".into(),
                "Siemens Healthineers".into(),
                "ge medical systems ".into(),
                "  ".into(),
            ]),
            ["ge medical systems", "siemens healthineers"]
        );
    }

    #[test]
    fn an_exact_manufacturer_keeps_its_case() {
        assert_eq!(
            exact(&[" Siemens ".into(), "SIEMENS".into(), "Siemens".into()]),
            ["SIEMENS", "Siemens"]
        );
    }
}
