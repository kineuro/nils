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
//! A run like this walks nothing, so it marks nothing gone; and the stacks of
//! the series it names are marked for the fingerprint to derive again, since
//! their row counts did not move and the fingerprint would otherwise take
//! them for fresh.

use std::path::Path;

use crossbeam_channel::Sender;
use nils_registry::schema::Type;
use nils_registry::store::{Error, Param, Store};

use crate::batch::{Prior, Task};
use crate::cancel::Cancel;
use crate::report::Counts;
use crate::resume::status;
use crate::walk::mtime_ns_of;

/// How many `source_file` rows one query reads.
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

/// The series the run reads, as a subquery over `series` and `study` whose
/// manufacturer parameters start at `first`.
fn series_of(store: &Store, first: usize, n: usize) -> String {
    let d = store.dialect();
    let list: Vec<String> = (0..n).map(|i| d.param(first + i, Type::Text)).collect();
    format!(
        "SELECT se.id FROM {} AS se JOIN {} AS sy ON sy.id = se.study_id \
         WHERE se.modality = 'MR' AND LOWER(TRIM(sy.manufacturer)) IN ({})",
        store.qualified("series"),
        store.qualified("study"),
        list.join(", ")
    )
}

/// Feed the parsers every file of the source that is its instance's own and
/// belongs to an MR series of one of `manufacturers`, in `source_file`
/// order, until the rows run out or a stop is asked. A file that is no
/// longer where the registry says is a walk error, which the report counts.
pub fn feed(
    mut store: Store,
    source_id: i64,
    root: &Path,
    manufacturers: &[String],
    tx: &Sender<Task>,
    cancel: &Cancel,
) -> Result<Counts, Error> {
    let counts = Counts::default();
    let names = folded(manufacturers);
    if names.is_empty() {
        return Ok(counts);
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT f.id, f.path, f.dir, f.size, f.mtime_ns, f.instance_id \
         FROM {files} AS f JOIN {instance} AS i ON i.id = f.instance_id AND i.source_file_id = f.id \
         WHERE f.source_id = {} AND f.id > {} AND f.status = '{ingested}' \
           AND i.series_id IN ({series}) \
         ORDER BY f.id LIMIT {}",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        // SQLite numbers its parameters by where they stand, so the limit
        // comes after the manufacturers in the text and in the list
        d.param(3 + names.len(), Type::Int),
        files = store.qualified("source_file"),
        instance = store.qualified("instance"),
        ingested = status::INGESTED,
        series = series_of(&store, 3, names.len()),
    );
    let mut after = 0i64;
    loop {
        if cancel.stop() {
            break;
        }
        let mut params = vec![Param::Int(source_id), Param::Int(after)];
        params.extend(names.iter().map(|n| Param::from(n.as_str())));
        params.push(Param::Int(PAGE as i64));
        let rows = store.query(&sql, &params)?;
        let n = rows.len();
        for r in &rows {
            after = r.int(0)?;
            let rel = r.text(1)?.to_string();
            let dir = r.text(2)?.to_string();
            let path = root.join(&rel);
            let task = match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => {
                    let size = m.len();
                    let mtime_ns = mtime_ns_of(&m);
                    Task::Parse {
                        path,
                        rel,
                        dir,
                        size,
                        mtime_ns,
                        prior: Some(Prior {
                            instance_id: r.opt_int(5)?,
                            changed: r.int(3)? != size as i64 || r.int(4)? != mtime_ns,
                        }),
                    }
                }
                Ok(_) => Task::WalkError {
                    error: format!("{rel}: no longer a file"),
                },
                Err(e) => Task::WalkError {
                    error: format!("{rel}: {e}"),
                },
            };
            if tx.send(task).is_err() || cancel.stop() {
                return Ok(counts);
            }
        }
        if n < PAGE {
            break;
        }
    }
    Ok(counts)
}

/// Mark the fingerprints of the series a run read again as stale, so that
/// the next fingerprint derives them from what the files said this time.
pub fn stale_fingerprints(store: &mut Store, manufacturers: &[String]) -> Result<u64, Error> {
    let names = folded(manufacturers);
    if names.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "UPDATE {} SET fingerprint_revision = 0 WHERE series_id IN ({})",
        store.qualified("stack_fingerprint"),
        series_of(store, 1, names.len()),
    );
    let params: Vec<Param> = names.iter().map(|n| Param::from(n.as_str())).collect();
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
}
