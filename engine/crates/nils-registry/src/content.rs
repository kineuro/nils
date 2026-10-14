// SPDX-License-Identifier: AGPL-3.0-only

//! What a source file's bytes are (record 55 E5, spec section 10.4; ruling
//! R3 of 2026-10-10): the sha256 of its content, learnt when a pipeline's
//! image read the file whole and said so, and never by a pass over the
//! archive. The engine's own fingerprint of a file is its size and time
//! (`source_file`), so a hash is kept with the size and time the file had
//! when it was learnt, and holds only while `source_file` says the same: a
//! file the digest saw change has no hash until a run reads it again. The
//! slice cache is keyed by it beside the SOP Instance UID, so a miss only
//! reads a file again, and a wrong hit cannot happen through a stale hash.

use crate::dialect::Conflict;
use crate::schema::table;
use crate::store::{Error, Insert, Param, Store};

/// One file's content as a run's image reported it: the file as
/// `source_file` names it, the size and time it had then, and its sha256
/// (`sha256:<hex>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learnt {
    pub source_id: i64,
    pub path: String,
    pub size: i64,
    pub mtime_ns: i64,
    pub sha256: String,
    pub run_id: Option<i64>,
}

/// Whether a text is `sha256:` and 64 lowercase hex digits.
pub fn is_sha256(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Keep what was learnt, each file's newest hash in place of the one
/// before. A hash that is not `sha256:<hex>` is left out. Answers how many
/// were kept.
pub fn learn(store: &mut Store, rows: &[Learnt]) -> Result<usize, Error> {
    let now = crate::time::now_iso();
    let kept: Vec<Vec<Param>> = rows
        .iter()
        .filter(|r| is_sha256(&r.sha256))
        .map(|r| {
            vec![
                Param::Int(r.source_id),
                Param::from(r.path.as_str()),
                Param::Int(r.size),
                Param::Int(r.mtime_ns),
                Param::from(r.sha256.as_str()),
                Param::from(now.as_str()),
                r.run_id.map_or(Param::Null, Param::Int),
            ]
        })
        .collect();
    if kept.is_empty() {
        return Ok(0);
    }
    let n = kept.len();
    store.insert(
        &Insert::new(
            table("file_content"),
            &[
                "source_id",
                "path",
                "size",
                "mtime_ns",
                "sha256",
                "learnt_at",
                "run_id",
            ],
        )
        .on_conflict(Conflict::Update {
            target: &["source_id", "path"],
            set: &["size", "mtime_ns", "sha256", "learnt_at", "run_id"],
        }),
        &kept,
    )?;
    Ok(n)
}

/// The SQL that reads a file's hash beside it: a `LEFT JOIN` of
/// `file_content` as `alias`, on the `source_file` row aliased `file`,
/// that matches only while the file has the size and time its hash was
/// learnt at. The hash is `<alias>.sha256`, null where none holds.
pub fn join(store: &Store, file: &str, alias: &str) -> String {
    format!(
        "LEFT JOIN {} {alias} ON {alias}.source_id = {file}.source_id AND {alias}.path = {file}.path \
         AND {alias}.size = {file}.size AND {alias}.mtime_ns = {file}.mtime_ns",
        store.qualified("file_content")
    )
}

/// How many files of the registry have a hash that holds now.
pub fn known(store: &mut Store) -> Result<i64, Error> {
    let sql = format!(
        "SELECT COUNT(*) FROM {} f {} WHERE fc.sha256 IS NOT NULL",
        store.qualified("source_file"),
        join(store, "f", "fc")
    );
    store.query(&sql, &[])?[0].int(0)
}

#[cfg(test)]
mod tests {
    use super::is_sha256;

    #[test]
    fn a_hash_is_sha256_and_lowercase_hex() {
        assert!(is_sha256(&format!("sha256:{}", "ab12".repeat(16))));
        assert!(!is_sha256(&"ab12".repeat(16)));
        assert!(!is_sha256(&format!("sha256:{}", "AB12".repeat(16))));
        assert!(!is_sha256("sha256:abc"));
    }
}
