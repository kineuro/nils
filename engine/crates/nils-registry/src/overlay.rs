// SPDX-License-Identifier: AGPL-3.0-only

//! An overlay as a registry object (Wave 4c §6.6, D45).
//!
//! A site's amendment to a pack is proposed with the rehearsal that
//! justified it, stands beside a review item, and is adopted by an
//! operator, which queues the reclassify over its scope. The document is
//! the overlay as written; the pack directory is where an adopted one is
//! exported to, by a command, so that a pack directory never changes under
//! a running engine.

use serde_json::{Value, json};

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Store};
use crate::time::now_iso;

/// The review item kind beside a proposal. Dotted, so it is not a
/// classifier kind (contract v4 adds only the `overlay` scope).
pub const REVIEW_KIND: &str = "overlay.proposed";

pub const PROPOSED: &str = "proposed";
pub const ADOPTED: &str = "adopted";
pub const REFUSED: &str = "refused";

#[derive(Debug, Clone)]
pub struct Overlay {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub pack: String,
    pub author: String,
    pub author_kind: String,
    pub actor: Value,
    pub scope: Value,
    pub status: String,
    pub document: Value,
    pub tried: Value,
    pub why: Option<String>,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub review_item: Option<i64>,
    pub job_id: Option<i64>,
}

impl Overlay {
    /// The row as a document: the list entry and the show, one shape.
    pub fn as_json(&self, full: bool) -> Value {
        let mut doc = json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "pack": self.pack,
            "status": self.status,
            "scope": self.scope,
            "author": self.author,
            "author_kind": self.author_kind,
            "actor": self.actor,
            "created_at": self.created_at,
            "decided_at": self.decided_at,
            "decided_by": self.decided_by,
            "review_item": self.review_item,
            "job": self.job_id,
        });
        if full {
            doc["document"] = self.document.clone();
            doc["tried"] = self.tried.clone();
            doc["why"] = json!(self.why);
        }
        doc
    }
}

/// What a proposal carries.
pub struct Proposal<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub pack: &'a str,
    pub author: &'a str,
    pub author_kind: &'a str,
    pub actor: Value,
    pub scope: Value,
    pub document: Value,
    pub tried: Value,
    pub why: Option<&'a str>,
}

/// Store a proposal and the review item beside it. Answers the overlay id
/// and the item id.
pub fn propose(store: &mut Store, p: &Proposal<'_>) -> Result<(i64, i64), Error> {
    let now = now_iso();
    let rows = store.insert(
        &Insert::new(
            table("overlay"),
            &[
                "name",
                "version",
                "pack",
                "author",
                "author_kind",
                "actor",
                "scope",
                "status",
                "document",
                "tried",
                "why",
                "created_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(p.name),
            Param::from(p.version),
            Param::from(p.pack),
            Param::from(p.author),
            Param::from(p.author_kind),
            Param::from(p.actor.to_string()),
            Param::from(p.scope.to_string()),
            Param::from(PROPOSED),
            Param::from(p.document.to_string()),
            Param::from(p.tried.to_string()),
            match p.why {
                Some(w) => Param::from(w),
                None => Param::Null,
            },
            Param::from(now.as_str()),
        ]],
    )?;
    let id = rows
        .first()
        .ok_or(Error::Message("no id returned".into()))?
        .int(0)?;
    let item = store.insert(
        &Insert::new(
            table("review_item"),
            &["kind", "scope", "ref", "evidence", "status", "created_at"],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(REVIEW_KIND),
            Param::from("overlay"),
            Param::from(json!({"overlay_id": id}).to_string()),
            Param::from(
                json!({
                    "name": p.name,
                    "version": p.version,
                    "pack": p.pack,
                    "scope": p.scope,
                    "author_kind": p.author_kind,
                    "tried": summary(&p.tried),
                })
                .to_string(),
            ),
            Param::from("open"),
            Param::from(now.as_str()),
        ]],
    )?;
    let item_id = item
        .first()
        .ok_or(Error::Message("no id returned".into()))?
        .int(0)?;
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET review_item = {} WHERE id = {}",
        store.qualified("overlay"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store.execute(&sql, &[Param::Int(item_id), Param::Int(id)])?;
    Ok((id, item_id))
}

/// The counts of a rehearsal, for the item's evidence: never the moves.
fn summary(tried: &Value) -> Value {
    json!({
        "moves": tried["moves"].as_array().map(|m| m.len()).unwrap_or(0),
        "stacks_moved": tried["moves"].as_array().map(|m| m.iter().map(|x| x["stacks"].as_i64().unwrap_or(0)).sum::<i64>()).unwrap_or(0),
        "review_items": tried["review_items"].clone(),
        "cases": tried["cases"].clone(),
        "sample": tried["sample"].clone(),
    })
}

fn select_sql(store: &Store, filter: &str) -> String {
    let d = store.dialect();
    let t = table("overlay");
    let text = |c: &str| d.text_of(t.column(c).expect("overlay column"));
    format!(
        "SELECT id, name, version, pack, author, author_kind, {}, {}, status, {}, {}, why, {}, {}, decided_by, review_item, job_id FROM {}{filter} ORDER BY id",
        text("actor"),
        text("scope"),
        text("document"),
        text("tried"),
        text("created_at"),
        text("decided_at"),
        store.qualified("overlay"),
    )
}

fn of(r: &crate::store::Row) -> Result<Overlay, Error> {
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or(Value::Null)
    };
    Ok(Overlay {
        id: r.int(0)?,
        name: r.text(1)?.to_string(),
        version: r.text(2)?.to_string(),
        pack: r.text(3)?.to_string(),
        author: r.text(4)?.to_string(),
        author_kind: r.text(5)?.to_string(),
        actor: json(r.opt_text(6)?),
        scope: json(r.opt_text(7)?),
        status: r.text(8)?.to_string(),
        document: json(r.opt_text(9)?),
        tried: json(r.opt_text(10)?),
        why: r.opt_text(11)?.map(str::to_string),
        created_at: r.text(12)?.to_string(),
        decided_at: r.opt_text(13)?.map(str::to_string),
        decided_by: r.opt_text(14)?.map(str::to_string),
        review_item: r.opt_int(15)?,
        job_id: r.opt_int(16)?,
    })
}

pub fn list(store: &mut Store) -> Result<Vec<Overlay>, Error> {
    let sql = select_sql(store, "");
    store.query(&sql, &[])?.iter().map(of).collect()
}

pub fn show(store: &mut Store, id: i64) -> Result<Option<Overlay>, Error> {
    let d = store.dialect();
    let sql = select_sql(store, &format!(" WHERE id = {}", d.param(1, Type::Int)));
    match store.query_opt(&sql, &[Param::Int(id)])? {
        Some(r) => Ok(Some(of(&r)?)),
        None => Ok(None),
    }
}

/// Mark an overlay adopted or refused, by whom, and close the item beside
/// it. Adoption records the job that reclassifies.
pub fn decide(
    store: &mut Store,
    id: i64,
    status: &str,
    by: &str,
    job_id: Option<i64>,
    why: Option<&str>,
) -> Result<(), Error> {
    let now = now_iso();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, decided_at = {}, decided_by = {}, job_id = {}, why = COALESCE({}, why) WHERE id = {}",
        store.qualified("overlay"),
        d.param(1, Type::Text),
        d.param(2, Type::Timestamp),
        d.param(3, Type::Text),
        d.param(4, Type::Int),
        d.param(5, Type::Text),
        d.param(6, Type::Int),
    );
    store.execute(
        &sql,
        &[
            Param::from(status),
            Param::from(now.as_str()),
            Param::from(by),
            match job_id {
                Some(j) => Param::Int(j),
                None => Param::Null,
            },
            match why {
                Some(w) => Param::from(w),
                None => Param::Null,
            },
            Param::Int(id),
        ],
    )?;
    let item_status = if status == ADOPTED {
        "accepted"
    } else {
        "rejected"
    };
    // The item beside it, by the id the proposal recorded: never by matching
    // the JSON text of a ref, which each backend renders its own way.
    let sql = format!(
        "SELECT review_item FROM {} WHERE id = {}",
        store.qualified("overlay"),
        d.param(1, Type::Int)
    );
    let item = store
        .query_opt(&sql, &[Param::Int(id)])?
        .and_then(|r| r.opt_int(0).ok().flatten());
    if let Some(item) = item {
        let sql = format!(
            "UPDATE {} SET status = {}, decided_at = {}, actor = {}, decision = {} WHERE id = {} AND status = 'open'",
            store.qualified("review_item"),
            d.param(1, Type::Text),
            d.param(2, Type::Timestamp),
            d.param(3, Type::Text),
            d.param(4, Type::Json),
            d.param(5, Type::Int),
        );
        store.execute(
            &sql,
            &[
                Param::from(item_status),
                Param::from(now.as_str()),
                Param::from(by),
                Param::from(
                    json!({"status": status, "job": job_id, "why": why, "actor_detail": crate::actor::current()})
                        .to_string(),
                ),
                Param::Int(item),
            ],
        )?;
    }
    Ok(())
}
