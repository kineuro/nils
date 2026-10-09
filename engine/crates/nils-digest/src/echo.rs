// SPDX-License-Identifier: AGPL-3.0-only

//! Stacks that only the echo number told apart (wave 7a;
//! `docs/specs/wave1-parse-and-digest.md`, §8).
//!
//! The signature is computed per file, and a file cannot say what its
//! EchoNumbers (0018,0086) counts. Not every vendor writes an echo there. One
//! writes the frame of a phase-contrast cine, 1 to 32 at one position, with
//! the echo time stated on the first two frames and zero on the rest: v0's
//! key made 32 stacks of one image. Another writes 1 and 2 in turn on the
//! slices of one 3D EPI volume under one echo time: two half volumes at
//! twice the spacing. A real multi-echo acquisition states an echo time for
//! each echo.
//!
//! So once a run has written its files, the stacks of every series it filed
//! a file in are read back together, and those that agree on all of their
//! signature but the echo number and the echo time, carry two or more echo
//! numbers and never two echo times, become one stack
//! ([`crate::stack::one_echo`]). A zero or absent echo time states none
//! (record 35, S4).
//!
//! The stack that stays is the oldest, then one that states the echo time,
//! then the one of the smallest echo number, which for a cine is its first
//! frame: its row is the one the series is described by. It takes the
//! others' instances and frame rows, their count and the first index any of
//! them had. What was derived from the others alone goes with them, and the
//! fingerprints of the series are marked for the next run to derive again,
//! since they count the series' stacks. A group one of whose stacks a person
//! or a release named stays as it is, as an empty stack does
//! ([`nils_registry::empty`]), and so does a group one file reaches twice.
//!
//! The rule reads the stack rows, not the files, which is what lets a
//! registry digested before it take it: a run that reads one file of a
//! series again folds the whole series.

use std::collections::{BTreeMap, HashMap};

use nils_dicom::catalogue::fields_of;
use nils_dicom::{Converter, Level, Value};
use nils_registry::empty::{kept_stacks, remove_stacks};
use nils_registry::schema::{Column, table};
use nils_registry::store::{Cell, Error, Store};

use crate::stack::{Class, Echo, one_echo};

/// What a fold did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folded {
    /// Stacks that held instances and were folded into another of their
    /// series.
    pub stacks: u64,
    /// The stacks folded away that the run doing the fold had created,
    /// whether they held anything or not: a run that reads a folded series
    /// again makes its frames' own stacks once more, and they go here.
    pub created: u64,
    /// Groups left as they were, as `(series, why)`.
    pub kept: Vec<(i64, &'static str)>,
}

/// One stack row read back, with its echo.
struct Member {
    id: i64,
    series: i64,
    index: i64,
    key: String,
    n_instances: i64,
    batch: i64,
    echo: Echo,
}

impl Member {
    /// Which stack of a group stays: the oldest, then one that states the
    /// echo time, then the smallest echo number, then the key.
    fn rank(&self) -> (i64, bool, i64, &str, &str) {
        (
            self.batch,
            self.echo.time.is_none(),
            self.echo.number.trim().parse().unwrap_or(i64::MAX),
            &self.echo.number,
            &self.key,
        )
    }
}

/// Fold the stacks that only the echo number told apart in `series`, inside
/// the caller's transaction. `batch` is the run doing it.
pub fn fold(store: &mut Store, series: &[i64], batch: i64) -> Result<Folded, Error> {
    let mut out = Folded::default();
    let candidates = more_than_one_echo_number(store, series)?;
    if candidates.is_empty() {
        return Ok(out);
    }
    let t = table("stack");
    let fixed = [
        "id",
        "series_id",
        "stack_index",
        "stack_key",
        "orientation",
        "n_instances",
        "first_batch_id",
    ];
    let fields: Vec<(&'static str, Converter)> = fields_of(Level::Stack)
        .map(|(_, f)| (f.column, f.converter))
        .collect();
    let columns: Vec<&Column> = fixed
        .iter()
        .copied()
        .chain(fields.iter().map(|(c, _)| *c))
        .map(|c| t.column(c).expect("a column of the stack table"))
        .collect();
    let rows = store.select_by_ids(t, &columns, "series_id", &candidates)?;

    // (series, the rest of the signature) → its stacks
    let mut groups: BTreeMap<(i64, String), Vec<Member>> = BTreeMap::new();
    for r in &rows {
        let values: HashMap<&str, Value> = fields
            .iter()
            .enumerate()
            .filter_map(|(i, (column, converter))| {
                value_of(*converter, r.get(fixed.len() + i)).map(|v| (*column, v))
            })
            .collect();
        let class = Class::of_name(r.text(4)?).unwrap_or(Class::Axial);
        let m = Member {
            id: r.int(0)?,
            series: r.int(1)?,
            index: r.int(2)?,
            key: r.text(3)?.to_string(),
            n_instances: r.int(5)?,
            batch: r.int(6)?,
            echo: Echo::of(|c| values.get(c), class),
        };
        groups
            .entry((m.series, m.echo.rest.clone()))
            .or_default()
            .push(m);
    }
    let mut plans: Vec<Vec<Member>> = groups
        .into_values()
        .filter(|g| {
            g.len() > 1
                && one_echo(
                    g.iter()
                        .map(|m| (m.echo.number.as_str(), m.echo.time.as_deref())),
                )
        })
        .collect();
    if plans.is_empty() {
        return Ok(out);
    }
    for g in &mut plans {
        g.sort_by(|a, b| a.rank().cmp(&b.rank()));
    }

    // what keeps a group as it is: a stack of it that would go and that
    // someone named, or one file in two of its stacks
    let mut others: Vec<i64> = plans
        .iter()
        .flat_map(|g| g[1..].iter().map(|m| m.id))
        .collect();
    others.sort_unstable();
    let (kept, items) = kept_stacks(store, &others)?;
    let twice = reached_twice(store, &plans)?;

    let mut moves: Vec<(i64, i64)> = Vec::new();
    let mut counts: Vec<(i64, i64)> = Vec::new();
    let mut indexes: Vec<(i64, i64)> = Vec::new();
    let mut fewer: BTreeMap<i64, i64> = BTreeMap::new();
    let mut gone: Vec<i64> = Vec::new();
    for (n, g) in plans.iter().enumerate() {
        let (stays, rest) = (&g[0], &g[1..]);
        if let Some(why) = rest.iter().find_map(|m| kept.get(&m.id).copied()) {
            out.kept.push((stays.series, why));
            continue;
        }
        if twice.contains(&n) {
            out.kept
                .push((stays.series, "one file's frames are in two of its stacks"));
            continue;
        }
        moves.extend(rest.iter().map(|m| (m.id, stays.id)));
        counts.push((stays.id, rest.iter().map(|m| m.n_instances).sum()));
        let first = g.iter().map(|m| m.index).min().unwrap_or(stays.index);
        if first < stays.index {
            indexes.push((stays.id, first));
        }
        *fewer.entry(stays.series).or_default() += rest.len() as i64;
        gone.extend(rest.iter().map(|m| m.id));
        out.stacks += rest.iter().filter(|m| m.n_instances > 0).count() as u64;
        out.created += rest.iter().filter(|m| m.batch == batch).count() as u64;
    }
    if gone.is_empty() {
        return Ok(out);
    }
    gone.sort_unstable();
    store.update_from_values(table("instance"), "stack_id = v.val", "stack_id", &moves)?;
    store.update_from_values(
        table("instance_frame"),
        "stack_id = v.val",
        "stack_id",
        &moves,
    )?;
    store.update_from_values(
        table("stack"),
        "n_instances = n_instances + v.val",
        "id",
        &counts,
    )?;
    remove_stacks(store, &gone, &items)?;
    // the first index of the group is free now that its stack is gone
    store.update_from_values(table("stack"), "stack_index = v.val", "id", &indexes)?;
    let fewer: Vec<(i64, i64)> = fewer.into_iter().collect();
    store.update_from_values(table("series"), "n_stacks = n_stacks - v.val", "id", &fewer)?;
    // a series' fingerprints count its stacks, and the stacks did not all
    // change their instance counts, which is what the fingerprint takes for
    // fresh
    for chunk in fewer.chunks(500) {
        store.execute(
            &format!(
                "UPDATE {} SET fingerprint_revision = 0 WHERE series_id IN ({})",
                store.qualified("stack_fingerprint"),
                list(chunk.iter().map(|(s, _)| *s))
            ),
            &[],
        )?;
    }
    Ok(out)
}

/// The series among `series` whose stacks carry more than one echo number,
/// counting a missing one as one: the only ones a fold can touch.
fn more_than_one_echo_number(store: &mut Store, series: &[i64]) -> Result<Vec<i64>, Error> {
    let mut out = Vec::new();
    for chunk in series.chunks(500) {
        let sql = format!(
            "SELECT series_id FROM {} WHERE series_id IN ({}) GROUP BY series_id \
             HAVING COUNT(DISTINCT COALESCE(echo_numbers, '')) > 1",
            store.qualified("stack"),
            list(chunk.iter().copied())
        );
        for r in store.query(&sql, &[])? {
            out.push(r.int(0)?);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// The groups (by their place in `plans`) one instance reaches through two
/// of their stacks, by the frame rows of an enhanced object. Such a file
/// would be counted twice in one stack, and its group stays as it is.
fn reached_twice(store: &mut Store, plans: &[Vec<Member>]) -> Result<Vec<usize>, Error> {
    let group_of: HashMap<i64, usize> = plans
        .iter()
        .enumerate()
        .flat_map(|(n, g)| g.iter().map(move |m| (m.id, n)))
        .collect();
    let ids: Vec<i64> = group_of.keys().copied().collect();
    let mut seen: HashMap<(usize, i64), u32> = HashMap::new();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT instance_id, stack_id FROM {} WHERE stack_id IN ({})",
            store.qualified("instance_frame"),
            list(chunk.iter().copied())
        );
        for r in store.query(&sql, &[])? {
            let group = group_of[&r.int(1)?];
            *seen.entry((group, r.int(0)?)).or_default() += 1;
        }
    }
    let mut out: Vec<usize> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|((group, _), _)| group)
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// A stack row's cell as the reader's value, by the catalogue's converter,
/// so that a row read back gives the signature its file gave.
fn value_of(converter: Converter, cell: &Cell) -> Option<Value> {
    Some(match (converter, cell) {
        (_, Cell::Null) => return None,
        (Converter::Double, Cell::Int(i)) => Value::Double(*i as f64),
        (Converter::Int, Cell::Double(d)) if d.fract() == 0.0 => Value::Int(*d as i64),
        (_, Cell::Double(d)) => Value::Double(*d),
        (_, Cell::Int(i)) => Value::Int(*i),
        (_, Cell::Text(s)) => Value::Text(s.clone()),
        (_, Cell::Bool(b)) => Value::Int(i64::from(*b)),
        (_, Cell::Bytes(b)) => Value::Text(String::from_utf8_lossy(b).into_owned()),
    })
}

fn list(ids: impl Iterator<Item = i64>) -> String {
    ids.map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_reads_back_as_the_value_the_reader_gave() {
        assert_eq!(
            value_of(Converter::Double, &Cell::Double(11.6)),
            Some(Value::Double(11.6))
        );
        assert_eq!(
            value_of(Converter::Double, &Cell::Int(20)),
            Some(Value::Double(20.0))
        );
        assert_eq!(
            value_of(Converter::Int, &Cell::Double(5.0)),
            Some(Value::Int(5))
        );
        assert_eq!(
            value_of(Converter::Text, &Cell::Text("1".into())),
            Some(Value::Text("1".into()))
        );
        assert_eq!(value_of(Converter::Text, &Cell::Null), None);
    }
}
