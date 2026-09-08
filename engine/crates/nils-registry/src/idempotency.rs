// SPDX-License-Identifier: AGPL-3.0-only
//! Wave 4c §6.3: an `Idempotency-Key` on a writing door. A caller that is
//! at least once by construction (a runtime that settles an unresolved
//! call as unknown and calls again) must not duplicate a handle or a
//! disclosure audit row, so the door remembers, per principal and key,
//! the digest of the body it answered and the answer, for a day.
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use serde_json::Value;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Store};
use crate::time::{iso_of, now_iso, secs_of};

/// How long a key is remembered, in hours.
pub const KEEP_HOURS: i64 = 24;

/// What a door recorded under a key.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub digest: String,
    pub status: i64,
    pub reply: Value,
}

/// The digest of a request body, as the record keeps it.
pub fn digest(body: &str) -> String {
    let mut h = Blake2b::<U32>::new();
    h.update(body.as_bytes());
    hex::encode(h.finalize())
}

/// The record under this principal's key, if the day has not passed.
pub fn lookup(store: &mut Store, principal: &str, key: &str) -> Result<Option<Record>, Error> {
    let d = store.dialect();
    let t = table("idempotency");
    let sql = format!(
        "SELECT body_digest, status, {} FROM {} WHERE principal = {} AND key = {} ORDER BY id DESC LIMIT 1",
        d.text_of(t.column("reply").expect("reply column")),
        store.qualified("idempotency"),
        d.param(1, Type::Text),
        d.param(2, Type::Text)
    );
    let Some(r) = store.query_opt(&sql, &[Param::from(principal), Param::from(key)])? else {
        return Ok(None);
    };
    Ok(Some(Record {
        digest: r.text(0)?.to_string(),
        status: r.int(1)?,
        reply: serde_json::from_str(r.opt_text(2)?.unwrap_or("null")).unwrap_or(Value::Null),
    }))
}

/// Remember an answer under the key, and forget the day-old ones.
pub fn record(
    store: &mut Store,
    principal: &str,
    key: &str,
    body_digest: &str,
    status: i64,
    reply: &Value,
) -> Result<(), Error> {
    let now = now_iso();
    sweep(store, &now)?;
    store.insert(
        &Insert::new(
            table("idempotency"),
            &[
                "principal",
                "key",
                "body_digest",
                "status",
                "reply",
                "created_at",
            ],
        ),
        &[vec![
            Param::from(principal),
            Param::from(key),
            Param::from(body_digest),
            Param::Int(status),
            Param::from(reply.to_string()),
            Param::from(now.as_str()),
        ]],
    )?;
    Ok(())
}

/// Forget the records older than a day before `now`.
pub fn sweep(store: &mut Store, now: &str) -> Result<u64, Error> {
    let cutoff = iso_of(
        secs_of(now)
            .unwrap_or(0)
            .saturating_sub(KEEP_HOURS as u64 * 3600),
    );
    let d = store.dialect();
    let t = table("idempotency");
    let sql = format!(
        "DELETE FROM {} WHERE {} < {}",
        store.qualified("idempotency"),
        d.text_of(t.column("created_at").expect("created_at column")),
        d.param(1, Type::Text)
    );
    store.execute(&sql, &[Param::from(cutoff.as_str())])
}
