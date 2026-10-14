// SPDX-License-Identifier: AGPL-3.0-only

//! A proposed rule change as a registry object (record 56, sections 5.3 to
//! 5.6, 6.4 and 6.5).
//!
//! Nima, 2026-10-09: "the change in rule should show how it affect the
//! sorting (like how many stack and to what)". A proposal is typed
//! operations (`nils_pack::patch`) with their reason, evidence and scope,
//! kept with the effect report they were rehearsed into, and decided by a
//! person: the pack is the auditable source of truth, and a rule never
//! changes by itself.
//!
//! - **A pack edit** (every operation scoped `pack`): adopted, it is the
//!   pack's next version, written where the engine reads its packs and
//!   recorded here as a rules version to release (`version`); every stack is
//!   sorted again by it.
//! - **A scoped change** (a site's, a dataset's, a scanner's or a batch's):
//!   adopted, its operations stay here, and every sort applies each of them
//!   to the stacks its scope holds and to no other (record 56 section 5.6:
//!   an overlay never leaves its scope); its scope is sorted again.
//!
//! Either re-sort records what it did on the proposal (`applied`). This
//! module keeps the rows; the doors and the sort read and write them.

use serde_json::Value;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Row, Store};
use crate::time::now_iso;

pub const PROPOSED: &str = "proposed";
pub const ADOPTED: &str = "adopted";
pub const REJECTED: &str = "rejected";

/// Every status, in the order a list counts them.
pub const STATUSES: [&str; 3] = [PROPOSED, ADOPTED, REJECTED];

/// A pack edit, which ships as a rules release.
pub const PACK: &str = "pack";
/// A change for a site, a dataset, a scanner or a batch.
pub const SCOPED: &str = "scoped";

/// One proposal, as its row holds it.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub id: i64,
    pub pack: String,
    pub title: String,
    pub kind: String,
    /// The pack version it was last rehearsed against.
    pub on_version: String,
    /// Its operations' scopes, as text.
    pub scopes: Vec<String>,
    /// The patch as rehearsed: `{patch, pack, operations, cases?, ships?}`,
    /// every operation as written with its scope, reason and evidence.
    pub patch: Value,
    pub reason: Option<String>,
    pub evidence: Value,
    /// The datasets it touches, by name.
    pub datasets: Vec<String>,
    pub status: String,
    pub report: Value,
    pub card: Value,
    pub rehearsed_at: Option<String>,
    pub author: String,
    pub author_kind: String,
    pub actor: Value,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub why: Option<String>,
    pub job_id: Option<i64>,
    pub version: Value,
    pub applied: Value,
}

impl Proposal {
    pub fn is_scoped(&self) -> bool {
        self.kind == SCOPED
    }

    /// Whether it touches a dataset, by name.
    pub fn touches(&self, dataset: &str) -> bool {
        self.datasets.iter().any(|d| d == dataset)
    }
}

/// What a new proposal carries.
pub struct New<'a> {
    pub pack: &'a str,
    pub title: &'a str,
    pub kind: &'a str,
    pub on_version: &'a str,
    pub scopes: &'a [String],
    pub patch: &'a Value,
    pub reason: Option<&'a str>,
    pub evidence: &'a Value,
    pub datasets: &'a [String],
    pub report: &'a Value,
    pub card: &'a Value,
    pub author: &'a str,
    pub author_kind: &'a str,
    pub actor: &'a Value,
}

fn texts(v: &[String]) -> String {
    Value::from(v.to_vec()).to_string()
}

fn opt(v: Option<&str>) -> Param {
    match v {
        Some(s) => Param::from(s),
        None => Param::Null,
    }
}

/// Keep a new proposal. Answers its id.
pub fn create(store: &mut Store, p: &New<'_>) -> Result<i64, Error> {
    let now = now_iso();
    let rows = store.insert(
        &Insert::new(
            table("rule_proposal"),
            &[
                "pack",
                "title",
                "kind",
                "on_version",
                "scopes",
                "patch",
                "reason",
                "evidence",
                "datasets",
                "status",
                "report",
                "card",
                "rehearsed_at",
                "author",
                "author_kind",
                "actor",
                "created_at",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(p.pack),
            Param::from(p.title),
            Param::from(p.kind),
            Param::from(p.on_version),
            Param::from(texts(p.scopes)),
            Param::from(p.patch.to_string()),
            opt(p.reason),
            Param::from(p.evidence.to_string()),
            Param::from(texts(p.datasets)),
            Param::from(PROPOSED),
            Param::from(p.report.to_string()),
            Param::from(p.card.to_string()),
            Param::from(now.as_str()),
            Param::from(p.author),
            Param::from(p.author_kind),
            Param::from(p.actor.to_string()),
            Param::from(now.as_str()),
        ]],
    )?;
    rows.first()
        .ok_or(Error::Message("no id returned".into()))?
        .int(0)
}

fn select_sql(store: &Store, filter: &str, order: &str) -> String {
    let d = store.dialect();
    let t = table("rule_proposal");
    let text = |c: &str| d.text_of(t.column(c).expect("rule_proposal column"));
    format!(
        "SELECT id, pack, title, kind, on_version, {}, {}, reason, {}, {}, status, {}, {}, {}, author, author_kind, {}, {}, {}, decided_by, why, job_id, {}, {} FROM {}{filter} ORDER BY {order}",
        text("scopes"),
        text("patch"),
        text("evidence"),
        text("datasets"),
        text("report"),
        text("card"),
        text("rehearsed_at"),
        text("actor"),
        text("created_at"),
        text("decided_at"),
        text("version"),
        text("applied"),
        store.qualified("rule_proposal"),
    )
}

fn of(r: &Row) -> Result<Proposal, Error> {
    let json = |s: Option<&str>| {
        s.and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or(Value::Null)
    };
    let list = |s: Option<&str>| -> Vec<String> {
        json(s)
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(Proposal {
        id: r.int(0)?,
        pack: r.text(1)?.to_string(),
        title: r.text(2)?.to_string(),
        kind: r.text(3)?.to_string(),
        on_version: r.text(4)?.to_string(),
        scopes: list(r.opt_text(5)?),
        patch: json(r.opt_text(6)?),
        reason: r.opt_text(7)?.map(str::to_string),
        evidence: json(r.opt_text(8)?),
        datasets: list(r.opt_text(9)?),
        status: r.text(10)?.to_string(),
        report: json(r.opt_text(11)?),
        card: json(r.opt_text(12)?),
        rehearsed_at: r.opt_text(13)?.map(str::to_string),
        author: r.text(14)?.to_string(),
        author_kind: r.text(15)?.to_string(),
        actor: json(r.opt_text(16)?),
        created_at: r.text(17)?.to_string(),
        decided_at: r.opt_text(18)?.map(str::to_string),
        decided_by: r.opt_text(19)?.map(str::to_string),
        why: r.opt_text(20)?.map(str::to_string),
        job_id: r.opt_int(21)?,
        version: json(r.opt_text(22)?),
        applied: json(r.opt_text(23)?),
    })
}

/// One proposal.
pub fn get(store: &mut Store, id: i64) -> Result<Option<Proposal>, Error> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(" WHERE id = {}", d.param(1, Type::Int)),
        "id",
    );
    match store.query_opt(&sql, &[Param::Int(id)])? {
        Some(r) => Ok(Some(of(&r)?)),
        None => Ok(None),
    }
}

/// A pack's proposals, newest first; every pack's where none is named.
pub fn list(store: &mut Store, pack: Option<&str>) -> Result<Vec<Proposal>, Error> {
    let d = store.dialect();
    match pack {
        Some(p) => {
            let sql = select_sql(
                store,
                &format!(" WHERE pack = {}", d.param(1, Type::Text)),
                "id DESC",
            );
            store
                .query(&sql, &[Param::from(p)])?
                .iter()
                .map(of)
                .collect()
        }
        None => {
            let sql = select_sql(store, "", "id DESC");
            store.query(&sql, &[])?.iter().map(of).collect()
        }
    }
}

/// A pack's adopted scoped changes, in the order they were adopted: what
/// every sort applies, each operation to the stacks its scope holds.
pub fn adopted_scoped(store: &mut Store, pack: &str) -> Result<Vec<Proposal>, Error> {
    let d = store.dialect();
    let sql = select_sql(
        store,
        &format!(
            " WHERE pack = {} AND kind = {} AND status = {}",
            d.param(1, Type::Text),
            d.param(2, Type::Text),
            d.param(3, Type::Text)
        ),
        "decided_at, id",
    );
    store
        .query(
            &sql,
            &[Param::from(pack), Param::from(SCOPED), Param::from(ADOPTED)],
        )?
        .iter()
        .map(of)
        .collect()
}

/// When the newest of a pack's scoped changes was adopted, where one was:
/// a proposal rehearsed before it no longer reads the registry as it is
/// sorted.
pub fn last_scoped_adoption(store: &mut Store, pack: &str) -> Result<Option<String>, Error> {
    Ok(adopted_scoped(store, pack)?
        .into_iter()
        .filter_map(|p| p.decided_at)
        .max())
}

/// A proposal rehearsed again: its report and card, the version and the
/// datasets, while it is still proposed. Answers whether it was.
pub fn rehearsed(
    store: &mut Store,
    id: i64,
    on_version: &str,
    report: &Value,
    card: &Value,
    datasets: &[String],
) -> Result<bool, Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET on_version = {}, report = {}, card = {}, datasets = {}, rehearsed_at = {} WHERE id = {} AND status = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Text),
        d.param(2, Type::Json),
        d.param(3, Type::Json),
        d.param(4, Type::Json),
        d.param(5, Type::Timestamp),
        d.param(6, Type::Int),
        d.param(7, Type::Text),
    );
    let n = store.execute(
        &sql,
        &[
            Param::from(on_version),
            Param::from(report.to_string()),
            Param::from(card.to_string()),
            Param::from(texts(datasets)),
            Param::from(now_iso().as_str()),
            Param::Int(id),
            Param::from(PROPOSED),
        ],
    )?;
    Ok(n > 0)
}

/// What a decision writes.
pub struct Decided<'a> {
    pub status: &'a str,
    pub by: &'a str,
    pub why: Option<&'a str>,
    /// The re-sort an adoption queued.
    pub job: Option<i64>,
    /// The rules version an adopted pack edit wrote.
    pub version: &'a Value,
    /// What the re-sort is expected to do, before it ran.
    pub applied: &'a Value,
}

/// Decide a proposal that is still proposed, and nothing else: two people
/// deciding at once, the second finds it decided. Answers whether it was.
pub fn decide(store: &mut Store, id: i64, d: &Decided<'_>) -> Result<bool, Error> {
    let dialect = store.dialect();
    let sql = format!(
        "UPDATE {} SET status = {}, decided_at = {}, decided_by = {}, why = {}, job_id = {}, version = {}, applied = {} WHERE id = {} AND status = {}",
        store.qualified("rule_proposal"),
        dialect.param(1, Type::Text),
        dialect.param(2, Type::Timestamp),
        dialect.param(3, Type::Text),
        dialect.param(4, Type::Text),
        dialect.param(5, Type::Int),
        dialect.param(6, Type::Json),
        dialect.param(7, Type::Json),
        dialect.param(8, Type::Int),
        dialect.param(9, Type::Text),
    );
    let json = |v: &Value| {
        if v.is_null() {
            Param::Null
        } else {
            Param::from(v.to_string())
        }
    };
    let n = store.execute(
        &sql,
        &[
            Param::from(d.status),
            Param::from(now_iso().as_str()),
            Param::from(d.by),
            opt(d.why),
            d.job.map_or(Param::Null, Param::Int),
            json(d.version),
            json(d.applied),
            Param::Int(id),
            Param::from(PROPOSED),
        ],
    )?;
    Ok(n > 0)
}

/// The re-sort an adoption queued, once it is queued.
pub fn set_job(store: &mut Store, id: i64, job: i64) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET job_id = {} WHERE id = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Int),
        d.param(2, Type::Int),
    );
    store.execute(&sql, &[Param::Int(job), Param::Int(id)])?;
    Ok(())
}

/// What the re-sort after an adoption did: `measured` set beside what was
/// expected, and the state it ended in.
pub fn set_measured(store: &mut Store, id: i64, measured: &Value) -> Result<bool, Error> {
    let Some(p) = get(store, id)? else {
        return Ok(false);
    };
    let mut applied = match p.applied {
        Value::Object(m) => Value::Object(m),
        _ => serde_json::json!({}),
    };
    applied["state"] = measured["state"].clone();
    applied["measured"] = measured.clone();
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET applied = {} WHERE id = {}",
        store.qualified("rule_proposal"),
        d.param(1, Type::Json),
        d.param(2, Type::Int),
    );
    store.execute(&sql, &[Param::from(applied.to_string()), Param::Int(id)])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Store {
        let mut s = Store::sqlite_in_memory().unwrap();
        crate::migrate::migrate(&mut s, crate::migrate::Kind::Registry).unwrap();
        s
    }

    fn new<'a>(
        kind: &'a str,
        scopes: &'a [String],
        patch: &'a Value,
        datasets: &'a [String],
        empty: &'a Value,
    ) -> New<'a> {
        New {
            pack: "mri",
            title: "mdc for contrast",
            kind,
            on_version: "1.2.0",
            scopes,
            patch,
            reason: Some("the partner writes mdc"),
            evidence: empty,
            datasets,
            report: empty,
            card: empty,
            author: "nima@local",
            author_kind: "person",
            actor: empty,
        }
    }

    #[test]
    fn a_proposal_is_kept_listed_decided_once_and_measured() {
        let mut s = store();
        let patch = json!({"patch": 1, "pack": "mri", "operations": [{"op": "add_words"}]});
        let scopes = vec!["dataset:partner".to_string()];
        let datasets = vec!["partner".to_string()];
        let empty = json!({});
        let id = create(&mut s, &new(SCOPED, &scopes, &patch, &datasets, &empty)).unwrap();
        let p = get(&mut s, id).unwrap().unwrap();
        assert_eq!(p.status, PROPOSED);
        assert_eq!(p.scopes, scopes);
        assert!(p.touches("partner") && !p.touches("other"));
        assert_eq!(p.patch["operations"][0]["op"], "add_words");
        assert!(adopted_scoped(&mut s, "mri").unwrap().is_empty());

        // rehearsed again while proposed
        assert!(
            rehearsed(
                &mut s,
                id,
                "1.2.1",
                &json!({"moved": {"stacks": 3}}),
                &empty,
                &datasets
            )
            .unwrap()
        );
        assert_eq!(get(&mut s, id).unwrap().unwrap().on_version, "1.2.1");

        let applied = json!({"state": "queued", "expected": {"moved": {"stacks": 3}}});
        let d = Decided {
            status: ADOPTED,
            by: "nima@local",
            why: Some("fixes 3"),
            job: Some(7),
            version: &Value::Null,
            applied: &applied,
        };
        assert!(decide(&mut s, id, &d).unwrap());
        // the second decision finds it decided
        assert!(!decide(&mut s, id, &d).unwrap());
        // nor is a decided one rehearsed again
        assert!(!rehearsed(&mut s, id, "1.2.2", &empty, &empty, &datasets).unwrap());
        let scoped = adopted_scoped(&mut s, "mri").unwrap();
        assert_eq!(scoped.len(), 1);
        assert!(last_scoped_adoption(&mut s, "mri").unwrap().is_some());

        assert!(set_measured(&mut s, id, &json!({"state": "done", "stacks": 3})).unwrap());
        let p = get(&mut s, id).unwrap().unwrap();
        assert_eq!(p.applied["state"], "done");
        assert_eq!(p.applied["expected"]["moved"]["stacks"], 3);
        assert_eq!(p.applied["measured"]["stacks"], 3);
        assert_eq!(p.job_id, Some(7));
        assert_eq!(list(&mut s, Some("mri")).unwrap().len(), 1);
        assert!(list(&mut s, Some("ct")).unwrap().is_empty());
    }
}
