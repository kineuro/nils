// SPDX-License-Identifier: AGPL-3.0-only

//! The executor (§11.4, §11.5): one compiled statement run inside a read
//! transaction, on a one-shot path that never enters the statement cache,
//! with the timeout as the one bound on work (a watchdog on SQLite, `SET
//! LOCAL statement_timeout` on Postgres), the row and byte caps as bounds on
//! what comes back, and the content hash of a complete answer.

use std::time::{Duration, Instant};

use blake2::digest::consts::{U8, U32};
use blake2::{Blake2b, Digest};
use nils_registry::store::{Cell, Error as StoreError, Row, Store};
use serde::{Deserialize, Serialize};

use crate::compile::Compiled;

#[derive(Debug)]
pub enum ExecError {
    Store(StoreError),
    /// The timeout stopped the statement.
    Timeout(u64),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Store(e) => write!(f, "{e}"),
            ExecError::Timeout(ms) => write!(
                f,
                "the question ran past {ms} ms and was stopped; queue it as a job"
            ),
        }
    }
}

impl std::error::Error for ExecError {}

impl From<StoreError> for ExecError {
    fn from(e: StoreError) -> ExecError {
        ExecError::Store(e)
    }
}

/// The bounds of one run.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub timeout_ms: u64,
    pub max_rows: u64,
    pub max_bytes: u64,
}

/// What a run returns.
#[derive(Debug, Clone)]
pub struct Answer {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    /// The rows were cut by a cap; a truncated answer has no hash and may
    /// not be released, pinned or used as a gold baseline.
    pub truncated: bool,
    pub content_hash: Option<String>,
    pub elapsed_ms: u64,
    /// Record 55 K7: the places of the columns answered as their shapes, a
    /// quasi identifying field read below detail quasi, so that every door
    /// says which they are (2026-10-12).
    pub shaped: Vec<usize>,
    /// Record 55 K7, the ruling of 2026-10-12: the places of the columns
    /// with a value held back, a group's mean, spread or count of distinct
    /// values of such a field where the group holds fewer than
    /// `MEASURE_K` subjects with a value; the value is null.
    pub held_back: Vec<usize>,
}

/// A column of an answer with values held back, and why (record 55 K7, the
/// ruling of 2026-10-12), so the chat can tell the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldBack {
    pub column: String,
    pub why: String,
}

impl Answer {
    /// The names of the columns answered as their shapes, in order.
    pub fn shaped_names(&self) -> Vec<String> {
        self.shaped
            .iter()
            .filter_map(|&i| self.columns.get(i).cloned())
            .collect()
    }

    /// The columns with a value held back, in order, each with why.
    pub fn held_back_named(&self) -> Vec<HeldBack> {
        let why = crate::validate::held_back_why();
        self.held_back
            .iter()
            .filter_map(|&i| self.columns.get(i))
            .map(|c| HeldBack {
                column: c.clone(),
                why: why.clone(),
            })
            .collect()
    }
}

/// Record 55 K7, the ruling of 2026-10-12: which of the columns held to a
/// group's subjects hold a value back in these rows. Such a value is null,
/// and a mean or a count of distinct values is never null where the group
/// holds the subjects it needs.
pub fn held_back_in(held: &[usize], rows: &[Row]) -> Vec<usize> {
    held.iter()
        .copied()
        .filter(|&i| rows.iter().any(|r| matches!(r.0.get(i), Some(Cell::Null))))
        .collect()
}

fn cell_bytes(c: &Cell) -> usize {
    match c {
        Cell::Null => 4,
        Cell::Text(t) => t.len(),
        Cell::Bytes(b) => b.len(),
        _ => 8,
    }
}

/// One renderer for the hash: every backend's cell as the same text.
pub fn render(c: &Cell) -> String {
    match c {
        Cell::Null => String::new(),
        Cell::Int(i) => i.to_string(),
        // nine decimals, trailing zeros trimmed: the two engines sum an
        // average in different orders and disagree in the last bit
        Cell::Double(d) => {
            if d.fract() == 0.0 && d.abs() < 1e15 {
                format!("{}", *d as i64)
            } else {
                let text = format!("{d:.9}");
                let trimmed = text.trim_end_matches('0').trim_end_matches('.');
                trimmed.to_string()
            }
        }
        Cell::Bool(b) => {
            if *b {
                "1".into()
            } else {
                "0".into()
            }
        }
        Cell::Text(t) => t.clone(),
        Cell::Bytes(b) => hex::encode(b),
    }
}

/// A subject code, digested (§11.5): the hash never carries one.
pub fn digest_code(code: &str) -> String {
    let mut h = Blake2b::<U8>::new();
    h.update(code.as_bytes());
    hex::encode(h.finalize())
}

/// The content hash of a complete answer: BLAKE2b over the rows in the
/// answer's order, cells rendered by the one renderer, codes digested.
pub fn content_hash(compiled: &Compiled, rows: &[Row]) -> String {
    let mut h = Blake2b::<U32>::new();
    for row in rows {
        for (i, cell) in row.0.iter().enumerate() {
            if i > 0 {
                h.update(b"\t");
            }
            if compiled.code_columns.contains(&i)
                && let Cell::Text(t) = cell
            {
                h.update(digest_code(t).as_bytes());
                continue;
            }
            h.update(render(cell).as_bytes());
        }
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// Record 55 K7 (spec §7.1): the columns that read a quasi identifying
/// field the caller may not project raw are answered as their shapes, before
/// the hash, so the handle, its pages and every door that reads them carry
/// the shapes alone. A null stays null.
pub fn shape_columns(compiled: &Compiled, rows: &mut [Row]) {
    if compiled.shaped_columns.is_empty() {
        return;
    }
    for row in rows.iter_mut() {
        for &i in &compiled.shaped_columns {
            if let Some(cell) = row.0.get_mut(i)
                && !matches!(cell, Cell::Null)
            {
                *cell = Cell::Text(crate::validate::shape(&render(cell)));
            }
        }
    }
}

/// Run a compiled ask inside a read transaction, under the bounds.
pub fn run(store: &mut Store, compiled: &Compiled, bounds: Bounds) -> Result<Answer, ExecError> {
    let started = Instant::now();
    store.begin_read()?;
    let is_pg = matches!(store.dialect(), nils_registry::dialect::Dialect::Postgres);
    if is_pg {
        store.batch(&format!(
            "SET LOCAL statement_timeout = {}",
            bounds.timeout_ms
        ))?;
    }
    // H9 on SQLite: a watchdog holding the interrupt handle
    let cancel = store.cancel_handle();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let timeout = Duration::from_millis(bounds.timeout_ms);
    let watchdog = (!is_pg).then(|| {
        std::thread::spawn(move || {
            if stop_rx.recv_timeout(timeout).is_err() {
                let _ = cancel.cancel();
            }
        })
    });
    let mut rows: Vec<Row> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    let result = store.query_stream(&compiled.sql, &compiled.params, |row| {
        if rows.len() as u64 >= bounds.max_rows {
            truncated = true;
            return Ok(false);
        }
        bytes += row.0.iter().map(cell_bytes).sum::<usize>();
        if bytes as u64 > bounds.max_bytes {
            truncated = true;
            return Ok(false);
        }
        rows.push(row);
        Ok(true)
    });
    let _ = stop_tx.send(());
    if let Some(w) = watchdog {
        let _ = w.join();
    }
    let ended = store.end_read();
    match result {
        Ok(_) => {}
        Err(e) => {
            store.rollback().ok();
            let text = e.to_string();
            if started.elapsed() >= timeout
                || text.contains("interrupted")
                || text.contains("statement timeout")
            {
                return Err(ExecError::Timeout(bounds.timeout_ms));
            }
            return Err(ExecError::Store(e));
        }
    }
    ended?;
    shape_columns(compiled, &mut rows);
    let content_hash = (!truncated).then(|| content_hash(compiled, &rows));
    let held_back = held_back_in(&compiled.held_columns, &rows);
    Ok(Answer {
        columns: compiled.columns.clone(),
        rows,
        truncated,
        content_hash,
        elapsed_ms: started.elapsed().as_millis() as u64,
        shaped: compiled.shaped_columns.clone(),
        held_back,
    })
}
